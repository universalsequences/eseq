//! Backend-neutral scene geometry for full-frame rendering.
//!
//! These are portable ports of the API-agnostic helper functions that grew up
//! inside `ui/metal_backend.rs` (`mod inner`, macOS-gated). The wgpu app
//! backend consumes them directly; the Metal backend still carries its own
//! copies because that file can only be compile-verified on a macOS host.
//! Unifying the two is deliberately deferred to the macOS verification sweep
//! (eseq-linux.26) — if you change behavior here, mirror it there.
//!
//! Everything in this module is pure CPU geometry: cells → pixels → NDC,
//! producing `Vertex`/instance data from the shared display-list types.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ops::Range;

use crate::backend::{Cell, Color, RenderFrame};
use crate::layout::LayoutNode;
use crate::theme;
use crate::ui::gpu_geometry::{
    ClipStack, ImageVertex, PatchCableInstance, ScissorRect, Vertex, push_solid_quad_vertices,
    push_solid_rect_vertices,
};
use crate::ui::glyph_atlas::{self, GlyphAtlas, ProportionalGlyphAtlas};
use crate::vm::Value;
use crate::widget_render::{self, WidgetInstance};

pub(crate) const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET: &str = "agent-instrument-stub-bg";
pub(crate) const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SUFFIX: &str =
    "__agent-instrument-stub-bg";
pub(crate) const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SAFE_SUFFIX: &str =
    "__agent_instrument_stub_bg";
pub(crate) const AGENT_INSTRUMENT_STUB_SKELETON_DEBUG_NAME: &str = "agent-instrument-stub-skeleton";

#[derive(Clone, Copy)]
pub(crate) struct CharCtx {
    pub cell_w: f32,
    pub cell_h: f32,
    pub vp_w: f32,
    pub vp_h: f32,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
}

/// Text placement in layout-cell coordinates. The origin is in the normal
/// widget/status grid; text rows and columns advance by the zoomed text
/// atlas cell dimensions.
#[derive(Clone, Copy, Default)]
pub(crate) struct TextOffset {
    pub origin_col: f32,
    pub origin_row: f32,
    pub scroll_left: f32,
}

pub(crate) fn rasterize_char(
    atlas: &mut GlyphAtlas,
    ch: char,
    (col, row): (f32, f32),
    ctx: &CharCtx,
    out: &mut Vec<Vertex>,
) {
    rasterize_char_px(
        atlas,
        ch,
        col * ctx.cell_w,
        row * ctx.cell_h,
        ctx.cell_w,
        ctx.cell_h,
        ctx,
        out,
    );
}

pub(crate) fn rasterize_char_px(
    atlas: &mut GlyphAtlas,
    ch: char,
    x0_px: f32,
    y0_px: f32,
    cell_w: f32,
    cell_h: f32,
    ctx: &CharCtx,
    out: &mut Vec<Vertex>,
) {
    let Some(entry) = atlas.get_or_rasterize(ch) else {
        return;
    };
    let [u0, v0] = entry.uv_min;
    let [u1, v1] = entry.uv_max;

    let ndc_x = |px: f32| px / ctx.vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / ctx.vp_h * 2.0;
    let x0 = ndc_x(x0_px);
    let x1 = ndc_x(x0_px + cell_w);
    let y0 = ndc_y(y0_px);
    let y1 = ndc_y(y0_px + cell_h);

    let gv = |px, py, u, v| Vertex {
        position: [px, py],
        uv: [u, v],
        fg: ctx.fg,
        bg: ctx.bg,
    };
    out.extend_from_slice(&[
        gv(x0, y0, u0, v0),
        gv(x0, y1, u0, v1),
        gv(x1, y0, u1, v0),
        gv(x1, y0, u1, v0),
        gv(x0, y1, u0, v1),
        gv(x1, y1, u1, v1),
    ]);
}

/// Per-tile text layer: background quads plus glyph quads for every cell,
/// shifted by the tile origin and horizontal scroll. Mirrors the Metal
/// `build_text_quads_offset` minus the legacy single-tile status/completion
/// path (the tiled renderer draws those itself).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tile_text_quads(
    frame: &RenderFrame,
    text_atlas: &mut GlyphAtlas,
    layout_cell_w: f32,
    layout_cell_h: f32,
    vp_w: f32,
    vp_h: f32,
    offset: TextOffset,
    default_bg: Color,
) -> Vec<Vertex> {
    let text_cell_w = (layout_cell_w * frame.text_cell_width_scale).max(1.0);
    let text_cell_h = (layout_cell_h * frame.text_cell_height_scale).max(1.0);
    let origin_x = offset.origin_col * layout_cell_w;
    let origin_y = offset.origin_row * layout_cell_h;
    let scroll_left = offset.scroll_left;
    let mut verts = Vec::with_capacity(frame.lines.len() * 80 * 6);

    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];

    for (row, line) in frame.lines.iter().enumerate() {
        for (col, cell) in line.iter().enumerate() {
            let text_col = col as f32 - scroll_left;
            let text_row = row as f32;
            let is_cursor = frame.cursor == Some((row, col));

            let x0_px = origin_x + text_col * text_cell_w;
            let x1_px = x0_px + text_cell_w;
            let y0_px = origin_y + text_row * text_cell_h;
            let y1_px = y0_px + text_cell_h;
            let x0 = ndc_x(x0_px);
            let x1 = ndc_x(x1_px);
            let y0 = ndc_y(y0_px);
            let y1 = ndc_y(y1_px);

            // Use a dedicated cursor fill so it stays legible over selection
            // and syntax colors.
            let (fg, bg) = if is_cursor {
                let cursor_bg = theme::CURSOR();
                let cursor_fg = if cursor_bg.luma() >= 0.55 {
                    theme::BG()
                } else {
                    theme::FG()
                };
                (to_rgba(cursor_fg), to_rgba(cursor_bg))
            } else {
                (
                    to_rgba(cell.style.fg),
                    to_rgba(cell.style.bg.unwrap_or(default_bg)),
                )
            };

            // Background quad — solid color, zero coverage glyph UV.
            let bg_v = |px, py| Vertex {
                position: [px, py],
                uv: [0.0, 0.0],
                fg: bg,
                bg,
            };
            verts.extend_from_slice(&[
                bg_v(x0, y0),
                bg_v(x0, y1),
                bg_v(x1, y0),
                bg_v(x1, y0),
                bg_v(x0, y1),
                bg_v(x1, y1),
            ]);

            if cell.ch == ' ' {
                continue;
            }

            rasterize_char_px(
                text_atlas,
                cell.ch,
                x0_px,
                y0_px,
                text_cell_w,
                text_cell_h,
                &CharCtx {
                    cell_w: text_cell_w,
                    cell_h: text_cell_h,
                    vp_w,
                    vp_h,
                    fg,
                    bg,
                },
                &mut verts,
            );
        }
    }
    verts
}

/// Status-bar row for one tile: filled background, styled cells, edge rules.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_status_row_quads(
    status_cells: &[Cell],
    atlas: &mut GlyphAtlas,
    status_left_px: f32,
    status_top_px: f32,
    status_right_px: f32,
    status_bottom_px: f32,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Vec<Vertex> {
    let mut verts = Vec::new();
    let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];
    let status_bg = to_rgba(theme::STATUS_BG());
    let status_col = status_left_px / cell_w;
    let status_row = status_top_px / cell_h;
    let status_width_px = (status_right_px - status_left_px).max(0.0);
    push_rect_px_rgba(
        &mut verts,
        status_left_px,
        status_top_px,
        status_width_px,
        (status_bottom_px - status_top_px).max(0.0),
        status_bg,
        vp_w,
        vp_h,
    );
    for (i, cell) in status_cells.iter().enumerate() {
        let ch_col = status_col + i as f32;
        if (ch_col + 1.0) * cell_w > status_right_px {
            continue;
        }
        if cell.ch == ' ' {
            continue;
        }
        let fg = to_rgba(cell.style.fg);
        let bg = to_rgba(cell.style.bg.unwrap_or(theme::STATUS_BG()));
        rasterize_char(
            atlas,
            cell.ch,
            (ch_col, status_row),
            &CharCtx {
                cell_w,
                cell_h,
                vp_w,
                vp_h,
                fg,
                bg,
            },
            &mut verts,
        );
    }
    // Edge lines AFTER cell backgrounds so they render on top.
    push_horizontal_rule(
        &mut verts,
        status_left_px,
        status_top_px,
        status_width_px,
        1.0,
        theme::STATUS_EDGE(),
        vp_w,
        vp_h,
    );
    push_horizontal_rule(
        &mut verts,
        status_left_px,
        status_bottom_px - 1.0,
        status_width_px,
        1.0,
        theme::STATUS_EDGE(),
        vp_w,
        vp_h,
    );
    verts
}

// ── Widget primitive vertex builders ─────────────────────────────────────────

pub(crate) fn build_widget_primitive_quads(
    primitives: &[widget_render::GpuPrimitive],
    atlas: &mut GlyphAtlas,
    vp_w: f32,
    vp_h: f32,
) -> Vec<Vertex> {
    let cell_w = atlas.cell_w as f32;
    let cell_h = atlas.cell_h as f32;
    let mut verts = Vec::new();
    for primitive in primitives {
        match widget_render::innermost_primitive(primitive) {
            widget_render::GpuPrimitive::Rect(rect) => {
                push_solid_rect_vertices(
                    rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts,
                );
            }
            widget_render::GpuPrimitive::Quad(quad) => {
                push_solid_quad_vertices(*quad, cell_w, cell_h, vp_w, vp_h, &mut verts);
            }
            widget_render::GpuPrimitive::Triangle(triangle) => {
                push_solid_triangle_vertices(*triangle, cell_w, cell_h, vp_w, vp_h, &mut verts);
            }
            widget_render::GpuPrimitive::GlyphRun(run) => {
                for (idx, ch) in run.text.chars().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    rasterize_char(
                        atlas,
                        ch,
                        ((run.col + idx as i32) as f32, run.row),
                        &CharCtx {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            fg: run.fg.to_rgba(),
                            bg: run.bg.to_rgba(),
                        },
                        &mut verts,
                    );
                }
            }
            _ => {}
        }
    }
    verts
}

pub(crate) fn build_foreground_rect_quads(
    primitives: &[widget_render::GpuPrimitive],
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Vec<Vertex> {
    let mut verts = Vec::new();
    for primitive in primitives {
        let widget_render::GpuPrimitive::ForegroundRect(rect) =
            widget_render::innermost_primitive(primitive)
        else {
            continue;
        };
        push_solid_rect_vertices(rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts);
    }
    verts
}

pub(crate) fn build_circle_quads(
    primitives: &[widget_render::GpuPrimitive],
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Vec<Vertex> {
    let mut verts = Vec::new();
    for primitive in primitives {
        let widget_render::GpuPrimitive::Circle(circle) =
            widget_render::innermost_primitive(primitive)
        else {
            continue;
        };
        push_circle_fill_px(
            &mut verts,
            circle.center[0] * cell_w,
            circle.center[1] * cell_h,
            circle.radius_px,
            circle.color,
            circle.visible_half,
            vp_w,
            vp_h,
        );
    }
    verts
}

// ── Proportional text ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ProportionalTextLayoutKey {
    text: String,
    size_tenths: u16,
}

struct CachedGlyphPlacement {
    pen_x: f32,
    offset_x: f32,
    raster_w: usize,
    raster_h: usize,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
}

struct CachedProportionalTextLayout {
    text_width_px: f32,
    line_height_px: f32,
    descent_px: f32,
    cap_height_px: f32,
    glyphs: Vec<CachedGlyphPlacement>,
    last_used_frame: u64,
}

/// Layout-only cache for proportional text (glyph placement per text+size).
/// This is the portable subset of the Metal backend's
/// `ProportionalTextLayoutCache`; vertex-run caching is a later optimization.
pub(crate) struct PropTextLayoutCache {
    layouts: HashMap<ProportionalTextLayoutKey, CachedProportionalTextLayout>,
    frame_index: u64,
}

impl PropTextLayoutCache {
    const MAX_ENTRIES: usize = 8192;
    const MAX_UNUSED_FRAMES: u64 = 600;

    pub fn new() -> Self {
        Self {
            layouts: HashMap::new(),
            frame_index: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_index = self.frame_index.wrapping_add(1);
        if self.layouts.len() > Self::MAX_ENTRIES {
            let cutoff = self.frame_index.saturating_sub(Self::MAX_UNUSED_FRAMES);
            self.layouts
                .retain(|_, layout| layout.last_used_frame >= cutoff);
            if self.layouts.len() > Self::MAX_ENTRIES {
                self.layouts.clear();
            }
        }
    }

    fn layout_for_run(
        &mut self,
        run: &widget_render::GpuProportionalTextPrimitive,
        prop_atlas: &mut ProportionalGlyphAtlas,
    ) -> Option<&CachedProportionalTextLayout> {
        let key = ProportionalTextLayoutKey {
            text: run.text.clone(),
            size_tenths: (run.font_size * 10.0).round() as u16,
        };
        if !self.layouts.contains_key(&key) {
            let mut pen_x = 0.0_f32;
            let mut glyphs = Vec::new();
            for ch in key.text.chars() {
                let Some(entry) = prop_atlas.get_or_rasterize(ch, key.size_tenths) else {
                    continue;
                };
                glyphs.push(CachedGlyphPlacement {
                    pen_x,
                    offset_x: entry.offset_x,
                    raster_w: entry.raster_w,
                    raster_h: entry.raster_h,
                    uv_min: entry.uv_min,
                    uv_max: entry.uv_max,
                });
                pen_x += entry.advance;
            }
            let layout = CachedProportionalTextLayout {
                text_width_px: pen_x,
                line_height_px: prop_atlas.line_height(key.size_tenths),
                descent_px: prop_atlas.descent(key.size_tenths),
                cap_height_px: prop_atlas.cap_height(key.size_tenths),
                glyphs,
                last_used_frame: self.frame_index,
            };
            self.layouts.insert(key.clone(), layout);
        }
        let frame = self.frame_index;
        let layout = self.layouts.get_mut(&key)?;
        layout.last_used_frame = frame;
        Some(layout)
    }
}

/// Build vertices for proportional text primitives. Each glyph is rendered as
/// a separate quad with alpha blending (transparent background).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_proportional_text_quads(
    primitives: &[widget_render::GpuPrimitive],
    prop_atlas: &mut ProportionalGlyphAtlas,
    layout_cache: &mut PropTextLayoutCache,
    mono_cell_w: f32,
    mono_cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Vec<Vertex> {
    let mut verts = Vec::new();
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;

    for prim in primitives {
        let widget_render::GpuPrimitive::ProportionalText(run) =
            widget_render::innermost_primitive(prim)
        else {
            continue;
        };

        let scale = run.scale.max(0.001);
        let fg = run.fg.to_rgba();
        let bg = [0.0, 0.0, 0.0, 0.0]; // Transparent — alpha blending handles bg
        let Some(layout) = layout_cache.layout_for_run(run, prop_atlas) else {
            continue;
        };

        let text_width_px = layout.text_width_px * scale;
        let align_extra_px = if run.align_width > 0.0 {
            (run.align_width * mono_cell_w - text_width_px).max(0.0) * run.h_align.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let base_x_px = run.col * mono_cell_w + align_extra_px;
        let base_y_px = run.row * mono_cell_h;

        // Vertical centering: place the baseline so the cap band is centered
        // within one mono cell height (widgets center text assuming 1.0 cell
        // units), then back out to the top of the glyph raster, whose own
        // baseline sits `descent` above its bottom edge.
        let baseline_px =
            glyph_atlas::centered_text_baseline_px(mono_cell_h, layout.cap_height_px, scale);
        let y_offset = baseline_px - (layout.line_height_px - layout.descent_px) * scale;

        for glyph in &layout.glyphs {
            if glyph.raster_w == 0 || glyph.raster_h == 0 {
                continue;
            }
            let [u0, v0] = glyph.uv_min;
            let [u1, v1] = glyph.uv_max;
            let gx0 = base_x_px + (glyph.pen_x + glyph.offset_x) * scale;
            let gy0 = base_y_px + y_offset;
            let gx1 = gx0 + glyph.raster_w as f32 * scale;
            let gy1 = gy0 + glyph.raster_h as f32 * scale;

            let x0 = ndc_x(gx0);
            let x1 = ndc_x(gx1);
            let y0 = ndc_y(gy0);
            let y1 = ndc_y(gy1);

            let gv = |px, py, u, v| Vertex {
                position: [px, py],
                uv: [u, v],
                fg,
                bg,
            };
            verts.extend_from_slice(&[
                gv(x0, y0, u0, v0),
                gv(x0, y1, u0, v1),
                gv(x1, y0, u1, v0),
                gv(x1, y0, u1, v0),
                gv(x0, y1, u0, v1),
                gv(x1, y1, u1, v1),
            ]);
        }
    }
    verts
}

// ── Primitive collectors ─────────────────────────────────────────────────────

pub(crate) fn collect_image_primitives(
    primitives: &[widget_render::GpuPrimitive],
) -> Vec<widget_render::GpuImagePrimitive> {
    primitives
        .iter()
        .filter_map(
            |primitive| match widget_render::innermost_primitive(primitive) {
                widget_render::GpuPrimitive::Image(image) => Some(image.clone()),
                _ => None,
            },
        )
        .collect()
}

pub(crate) fn collect_waveform_primitives(
    primitives: &[widget_render::GpuPrimitive],
) -> Vec<widget_render::GpuWaveformPrimitive> {
    primitives
        .iter()
        .filter_map(
            |primitive| match widget_render::innermost_primitive(primitive) {
                widget_render::GpuPrimitive::Waveform(waveform) => Some(waveform.clone()),
                _ => None,
            },
        )
        .collect()
}

pub(crate) fn collect_wavetable_primitives(
    primitives: &[widget_render::GpuPrimitive],
) -> Vec<widget_render::GpuWavetablePrimitive> {
    primitives
        .iter()
        .filter_map(
            |primitive| match widget_render::innermost_primitive(primitive) {
                widget_render::GpuPrimitive::Wavetable(wavetable) => Some(wavetable.clone()),
                _ => None,
            },
        )
        .collect()
}

pub(crate) fn collect_live_spectrogram_primitives(
    primitives: &[widget_render::GpuPrimitive],
) -> Vec<widget_render::GpuLiveSpectrogramPrimitive> {
    primitives
        .iter()
        .filter_map(
            |primitive| match widget_render::innermost_primitive(primitive) {
                widget_render::GpuPrimitive::LiveSpectrogram(spectrogram) => {
                    Some(spectrogram.clone())
                }
                _ => None,
            },
        )
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct PatchCableDrawInstance {
    pub instance: PatchCableInstance,
    pub clip: ScissorRect,
}

pub(crate) fn collect_patch_cable_primitives(
    primitives: &[widget_render::GpuPrimitive],
    base_clip: ScissorRect,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Vec<PatchCableDrawInstance> {
    // Some render paths draw a whole scene in one pass instead of first
    // splitting it into clip segments. Preserve the clip stack here too,
    // otherwise patcher cables escape their widget and paint over sibling
    // panes in captures and single-frame rendering.
    let mut clips = ClipStack::new(base_clip);
    let mut cables = Vec::new();
    for primitive in primitives {
        match primitive {
            widget_render::GpuPrimitive::PushClipRect(rect) => {
                clips.push_cells(*rect, cell_w, cell_h);
            }
            widget_render::GpuPrimitive::PopClipRect => clips.pop(),
            _ => {
                if let widget_render::GpuPrimitive::PatchCable(cable) =
                    widget_render::innermost_primitive(primitive)
                    && let Some(instance) = patch_cable_draw_instance_from_primitive(
                        cable,
                        clips.current(),
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    )
                {
                    cables.push(instance);
                }
            }
        }
    }
    cables
}

fn patch_cable_draw_instance_from_primitive(
    cable: &widget_render::GpuPatchCablePrimitive,
    clip: ScissorRect,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> Option<PatchCableDrawInstance> {
    let start = (cable.start[0] * cell_w, cable.start[1] * cell_h);
    let c1 = (cable.control1[0] * cell_w, cable.control1[1] * cell_h);
    let c2 = (cable.control2[0] * cell_w, cable.control2[1] * cell_h);
    let end = (cable.end[0] * cell_w, cable.end[1] * cell_h);
    let segment_y_px = cable.segment_row * cell_h;
    let corner_radius_px = cable.corner_radius_cells * cell_w.min(cell_h);
    let padding = cable.radius_px
        + widget_render::ui_design_px(widget_render::PATCH_CABLE_DRAW_PADDING_PX);
    let (min_x, max_x, min_y, max_y) = if cable.is_segmented {
        let needs_five = end.1 < segment_y_px;
        if needs_five {
            let clearance = corner_radius_px * 2.0;
            let turnaround_y = end.1 - clearance;
            let turnaround_x = end.0 - clearance;
            (
                start.0.min(end.0).min(turnaround_x) - padding,
                start.0.max(end.0).max(turnaround_x) + padding,
                start.1.min(end.1).min(segment_y_px).min(turnaround_y) - padding,
                start.1.max(end.1).max(segment_y_px).max(turnaround_y) + padding,
            )
        } else {
            (
                start.0.min(end.0) - padding,
                start.0.max(end.0) + padding,
                start.1.min(end.1).min(segment_y_px) - padding,
                start.1.max(end.1).max(segment_y_px) + padding,
            )
        }
    } else {
        (
            start.0.min(end.0).min(c1.0).min(c2.0) - padding,
            start.0.max(end.0).max(c1.0).max(c2.0) + padding,
            start.1.min(end.1).min(c1.1).min(c2.1) - padding,
            start.1.max(end.1).max(c1.1).max(c2.1) + padding,
        )
    };
    if min_x >= vp_w || max_x <= 0.0 || min_y >= vp_h || max_y <= 0.0 {
        return None;
    }
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    Some(PatchCableDrawInstance {
        instance: PatchCableInstance {
            ndc_min: [ndc_x(min_x), ndc_y(min_y)],
            ndc_max: [ndc_x(max_x), ndc_y(max_y)],
            bounds_min: [min_x, min_y],
            bounds_max: [max_x, max_y],
            start: [start.0, start.1],
            control1: [c1.0, c1.1],
            control2: [c2.0, c2.1],
            end: [end.0, end.1],
            color: cable.color.to_rgba(),
            radius_px: cable.radius_px,
            is_segmented: if cable.is_segmented { 1.0 } else { 0.0 },
            segment_y_px,
            corner_radius_px,
        },
        clip,
    })
}

// ── Mod-matrix patch cables ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModPatchPortDirection {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub(crate) struct ModPatchPort {
    pub direction: ModPatchPortDirection,
    pub track: usize,
    pub dest_kind: String,
    pub dest: usize,
    pub input: usize,
    pub active: bool,
    pub pending: bool,
    pub center_px: (f32, f32),
    pub clip: ScissorRect,
    pub connected_sources: Vec<usize>,
    pub selected_sources: Vec<usize>,
}

pub(crate) fn collect_mod_patch_ports(
    node: &LayoutNode,
    col_off: f32,
    row_off: f32,
    cell_w: f32,
    cell_h: f32,
    visible_scissor: ScissorRect,
    out: &mut Vec<ModPatchPort>,
) {
    if layout_node_bool_prop(node, "patch-port")
        && let Some(direction) = mod_patch_port_direction(node)
    {
        let track = layout_node_usize_prop(node, "track");
        let dest_kind =
            layout_node_string_prop(node, "dest-kind").unwrap_or_else(|| "track".into());
        let dest = layout_node_usize_prop(node, "dest").or(track);
        let Some(track_or_dest) = track.or(dest) else {
            for child in &node.children {
                collect_mod_patch_ports(
                    child,
                    col_off,
                    row_off,
                    cell_w,
                    cell_h,
                    visible_scissor,
                    out,
                );
            }
            return;
        };
        let center_col = col_off + node.rect.col + node.rect.width * 0.5;
        let center_row = row_off + node.rect.row + node.rect.height * 0.5;
        let center_px = (center_col * cell_w, center_row * cell_h);
        if center_px.0.is_finite() && center_px.1.is_finite() {
            out.push(ModPatchPort {
                direction,
                track: track.unwrap_or(track_or_dest),
                dest_kind,
                dest: dest.unwrap_or(track_or_dest),
                input: layout_node_usize_prop(node, "input").unwrap_or(0),
                active: layout_node_bool_prop(node, "active"),
                pending: layout_node_bool_prop(node, "pending"),
                center_px,
                clip: visible_scissor,
                connected_sources: layout_node_usize_list_prop(node, "connected-sources"),
                selected_sources: layout_node_usize_list_prop(node, "selected-sources"),
            });
        }
    }

    for child in &node.children {
        collect_mod_patch_ports(child, col_off, row_off, cell_w, cell_h, visible_scissor, out);
    }
}

fn mod_patch_port_direction(node: &LayoutNode) -> Option<ModPatchPortDirection> {
    match node.props.get("direction") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "in" => {
            Some(ModPatchPortDirection::In)
        }
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "out" => {
            Some(ModPatchPortDirection::Out)
        }
        _ => None,
    }
}

fn layout_node_usize_prop(node: &LayoutNode, key: &str) -> Option<usize> {
    match node.props.get(key) {
        Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn layout_node_string_prop(node: &LayoutNode, key: &str) -> Option<String> {
    match node.props.get(key) {
        Some(Value::String(value)) | Some(Value::Keyword(value)) => Some(value.clone()),
        _ => None,
    }
}

fn layout_node_bool_prop(node: &LayoutNode, key: &str) -> bool {
    matches!(node.props.get(key), Some(Value::Bool(true)))
}

fn layout_node_usize_list_prop(node: &LayoutNode, key: &str) -> Vec<usize> {
    let Some(Value::List(values)) = node.props.get(key) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match &*value.borrow() {
            Value::Number(n) if n.is_finite() && *n >= 0.0 => Some(*n as usize),
            _ => None,
        })
        .collect()
}

pub(crate) fn build_mod_patch_cables(
    ports: &[ModPatchPort],
    vp_w: f32,
    vp_h: f32,
    cursor_px: (f32, f32),
) -> Vec<PatchCableDrawInstance> {
    let mut outputs = HashMap::new();
    for port in ports {
        if port.direction == ModPatchPortDirection::Out && port.active {
            outputs.insert(port.track, (port.center_px, port.clip));
        }
    }

    let mut cables = Vec::new();
    let base_color = Color::rgba(0.10, 0.58, 1.0, 0.92);
    let highlight_color = Color::rgba(0.78, 0.94, 1.0, 0.38);
    let shadow_color = Color::rgba(0.0, 0.0, 0.0, 0.34);
    let selected_color = Color::rgba(1.0, 0.16, 0.10, 0.96);
    let selected_highlight_color = Color::rgba(1.0, 0.66, 0.58, 0.42);
    let preview_color = Color::rgba(0.42, 0.84, 1.0, 0.84);
    let preview_highlight_color = Color::rgba(0.88, 0.98, 1.0, 0.36);
    let tension = 0.30;
    for port in ports {
        if port.direction != ModPatchPortDirection::In {
            continue;
        }
        for source in &port.connected_sources {
            let Some((start, source_clip)) = outputs.get(source).copied() else {
                continue;
            };
            let cable_clip = shared_endpoint_clip(source_clip, port.clip, vp_w, vp_h);
            push_mod_patch_cable_instance(
                (start.0 + 1.4, start.1 + 2.2),
                (port.center_px.0 + 1.4, port.center_px.1 + 2.2),
                3.6,
                shadow_color,
                cable_clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
            let lane_tint = (port.input as f32 * 0.08).min(0.24);
            let is_selected = port
                .selected_sources
                .iter()
                .any(|selected| selected == source);
            let color = if is_selected {
                selected_color
            } else {
                Color {
                    r: (base_color.r + lane_tint).min(1.0),
                    g: (base_color.g + lane_tint * 0.35).min(1.0),
                    b: base_color.b,
                    a: base_color.a,
                }
            };
            push_mod_patch_cable_instance(
                start,
                port.center_px,
                1.85,
                color,
                cable_clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
            push_mod_patch_cable_instance(
                (start.0, start.1 - 0.7),
                (port.center_px.0, port.center_px.1 - 0.7),
                0.55,
                if is_selected {
                    selected_highlight_color
                } else {
                    highlight_color
                },
                cable_clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
        }
    }
    if let Some(source_port) = ports
        .iter()
        .find(|port| port.direction == ModPatchPortDirection::Out && port.active && port.pending)
    {
        push_mod_patch_cable_instance(
            (source_port.center_px.0 + 1.4, source_port.center_px.1 + 2.2),
            (cursor_px.0 + 1.4, cursor_px.1 + 2.2),
            3.6,
            shadow_color,
            source_port.clip,
            &mut cables,
            vp_w,
            vp_h,
            tension,
        );
        push_mod_patch_cable_instance(
            source_port.center_px,
            cursor_px,
            1.85,
            preview_color,
            source_port.clip,
            &mut cables,
            vp_w,
            vp_h,
            tension,
        );
        push_mod_patch_cable_instance(
            (source_port.center_px.0, source_port.center_px.1 - 0.7),
            (cursor_px.0, cursor_px.1 - 0.7),
            0.55,
            preview_highlight_color,
            source_port.clip,
            &mut cables,
            vp_w,
            vp_h,
            tension,
        );
    }
    cables
}

pub(crate) fn build_mod_patch_drag_highlight(
    ports: &[ModPatchPort],
    cursor_px: (f32, f32),
    vp_w: f32,
    vp_h: f32,
) -> Option<(Vec<Vertex>, ScissorRect)> {
    let source_port = ports
        .iter()
        .find(|port| port.direction == ModPatchPortDirection::Out && port.active && port.pending)?;
    let input_port = nearest_mod_input_port(ports, source_port.track, cursor_px)?;
    let mut verts = Vec::new();
    let size = 13.5;
    let x = input_port.center_px.0 - size * 0.5;
    let y = input_port.center_px.1 - size * 0.5;
    push_rounded_rect_fill_px(
        &mut verts,
        x,
        y,
        size,
        size,
        size * 0.5,
        Color::rgba(0.30, 0.76, 1.0, 0.14),
        vp_w,
        vp_h,
    );
    push_rounded_rect_border_px(
        &mut verts,
        x,
        y,
        size,
        size,
        2.0,
        size * 0.5,
        Color::rgba(0.72, 0.95, 1.0, 0.92),
        vp_w,
        vp_h,
    );
    Some((verts, input_port.clip))
}

fn shared_endpoint_clip(
    source_clip: ScissorRect,
    dest_clip: ScissorRect,
    vp_w: f32,
    vp_h: f32,
) -> ScissorRect {
    if source_clip == dest_clip {
        source_clip
    } else {
        full_viewport_scissor(vp_w, vp_h)
    }
}

fn nearest_mod_input_port<'a>(
    ports: &'a [ModPatchPort],
    source_track: usize,
    cursor_px: (f32, f32),
) -> Option<&'a ModPatchPort> {
    ports
        .iter()
        .filter(|port| {
            port.direction == ModPatchPortDirection::In
                && port.active
                && !(port.dest_kind == "track" && port.dest == source_track)
        })
        .min_by(|a, b| {
            let da = squared_distance_px(a.center_px, cursor_px);
            let db = squared_distance_px(b.center_px, cursor_px);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn squared_distance_px(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

#[allow(clippy::too_many_arguments)]
fn push_mod_patch_cable_instance(
    start: (f32, f32),
    end: (f32, f32),
    radius_px: f32,
    color: Color,
    clip: ScissorRect,
    cables: &mut Vec<PatchCableDrawInstance>,
    vp_w: f32,
    vp_h: f32,
    tension: f32,
) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let distance = (dx * dx + dy * dy).sqrt();
    let horizontal = dx.abs();
    let slack = (1.0 - tension).clamp(0.0, 1.0);
    let sag = ((28.0 + distance * 0.22) * slack).clamp(18.0, 98.0);
    let handle_x = horizontal.clamp(42.0, 190.0) * (0.30 + 0.14 * slack);
    let direction = if dx >= 0.0 { 1.0 } else { -1.0 };
    let c1 = (start.0 + handle_x * direction, start.1 + sag);
    let c2 = (end.0 - handle_x * direction, end.1 + sag);
    let padding = radius_px + sag * 0.12 + 8.0;
    let min_x = start.0.min(end.0).min(c1.0).min(c2.0) - padding;
    let max_x = start.0.max(end.0).max(c1.0).max(c2.0) + padding;
    let min_y = start.1.min(end.1).min(c1.1).min(c2.1) - padding;
    let max_y = start.1.max(end.1).max(c1.1).max(c2.1) + padding;
    if min_x >= vp_w || max_x <= 0.0 || min_y >= vp_h || max_y <= 0.0 {
        return;
    }
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    cables.push(PatchCableDrawInstance {
        instance: PatchCableInstance {
            ndc_min: [ndc_x(min_x), ndc_y(min_y)],
            ndc_max: [ndc_x(max_x), ndc_y(max_y)],
            bounds_min: [min_x, min_y],
            bounds_max: [max_x, max_y],
            start: [start.0, start.1],
            control1: [c1.0, c1.1],
            control2: [c2.0, c2.1],
            end: [end.0, end.1],
            color: color.to_rgba(),
            radius_px,
            is_segmented: 0.0,
            segment_y_px: 0.0,
            corner_radius_px: 0.0,
        },
        clip,
    });
}

// ── Image geometry ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn image_vertices(
    image: &widget_render::GpuImagePrimitive,
    image_w: u32,
    image_h: u32,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
    rotation: f32,
) -> Vec<ImageVertex> {
    if image.rect.width <= 0.0 || image.rect.height <= 0.0 || image_w == 0 || image_h == 0 {
        return Vec::new();
    }
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let x0 = ndc_x(image.rect.col * cell_w);
    let x1 = ndc_x((image.rect.col + image.rect.width) * cell_w);
    let y0 = ndc_y(image.rect.row * cell_h);
    let y1 = ndc_y((image.rect.row + image.rect.height) * cell_h);

    let dst_aspect = (image.rect.width * cell_w) / (image.rect.height * cell_h).max(0.001);
    let src_aspect = image_w as f32 / (image_h as f32).max(1.0);
    let (mut u0, mut v0, mut u1, mut v1) = (0.0, 0.0, 1.0, 1.0);
    match image.fit {
        widget_render::ImageFit::Cover => {
            if src_aspect > dst_aspect {
                let visible = dst_aspect / src_aspect;
                u0 = (1.0 - visible) * 0.5;
                u1 = 1.0 - u0;
            } else {
                let visible = src_aspect / dst_aspect;
                v0 = (1.0 - visible) * 0.5;
                v1 = 1.0 - v0;
            }
        }
        widget_render::ImageFit::Contain | widget_render::ImageFit::Stretch => {}
    }

    if matches!(image.fit, widget_render::ImageFit::Contain) {
        let mut dx0 = x0;
        let mut dx1 = x1;
        let mut dy0 = y0;
        let mut dy1 = y1;
        if src_aspect > dst_aspect {
            let target_h = (image.rect.width * cell_w / src_aspect) / vp_h * 2.0;
            let mid = (y0 + y1) * 0.5;
            dy0 = mid + target_h * 0.5;
            dy1 = mid - target_h * 0.5;
        } else {
            let target_w = (image.rect.height * cell_h * src_aspect) / vp_w * 2.0;
            let mid = (x0 + x1) * 0.5;
            dx0 = mid - target_w * 0.5;
            dx1 = mid + target_w * 0.5;
        }
        return image_vertex_quad(
            dx0,
            dx1,
            dy0,
            dy1,
            u0,
            u1,
            v0,
            v1,
            image.opacity,
            image.radius_px,
            rotation,
            image.clip_circle,
            image.rect.width * cell_w,
            image.rect.height * cell_h,
        );
    }

    image_vertex_quad(
        x0,
        x1,
        y0,
        y1,
        u0,
        u1,
        v0,
        v1,
        image.opacity,
        image.radius_px,
        rotation,
        image.clip_circle,
        image.rect.width * cell_w,
        image.rect.height * cell_h,
    )
}

pub(crate) fn image_intersects_scissor(
    image: &widget_render::GpuImagePrimitive,
    scissor: ScissorRect,
    cell_w: f32,
    cell_h: f32,
) -> bool {
    let x0 = (image.rect.col * cell_w).floor() as isize;
    let y0 = (image.rect.row * cell_h).floor() as isize;
    let x1 = ((image.rect.col + image.rect.width) * cell_w).ceil() as isize;
    let y1 = ((image.rect.row + image.rect.height) * cell_h).ceil() as isize;
    let sx0 = scissor.x as isize;
    let sy0 = scissor.y as isize;
    let sx1 = scissor.x.saturating_add(scissor.width) as isize;
    let sy1 = scissor.y.saturating_add(scissor.height) as isize;
    x1 > sx0 && x0 < sx1 && y1 > sy0 && y0 < sy1
}

pub(crate) fn angular_distance(a: f32, b: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut d = (a - b).rem_euclid(tau);
    if d > std::f32::consts::PI {
        d = tau - d;
    }
    d.abs()
}

#[allow(clippy::too_many_arguments)]
fn image_vertex_quad(
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    opacity: f32,
    radius: f32,
    rotation: f32,
    clip_circle: bool,
    width_px: f32,
    height_px: f32,
) -> Vec<ImageVertex> {
    let half_size = [width_px.max(0.0) * 0.5, height_px.max(0.0) * 0.5];
    let clip_circle = if clip_circle { 1.0 } else { 0.0 };
    let v = |position, uv, local_pos| ImageVertex {
        position,
        uv,
        opacity,
        local_pos,
        half_size,
        radius,
        rotation,
        clip_circle,
    };
    vec![
        v([x0, y0], [u0, v0], [-half_size[0], -half_size[1]]),
        v([x0, y1], [u0, v1], [-half_size[0], half_size[1]]),
        v([x1, y0], [u1, v0], [half_size[0], -half_size[1]]),
        v([x1, y0], [u1, v0], [half_size[0], -half_size[1]]),
        v([x0, y1], [u0, v1], [-half_size[0], half_size[1]]),
        v([x1, y1], [u1, v1], [half_size[0], half_size[1]]),
    ]
}

// ── Solid geometry helpers ───────────────────────────────────────────────────

pub(crate) fn push_horizontal_rule(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Color,
    vp_w: f32,
    vp_h: f32,
) {
    push_rect_px_rgba(verts, x, y, width, height, color.to_rgba(), vp_w, vp_h);
}

pub(crate) fn push_solid_triangle_vertices(
    triangle: widget_render::GpuTrianglePrimitive,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
    verts: &mut Vec<Vertex>,
) {
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let rgba = triangle.color.to_rgba();
    let v = |point: [f32; 2]| Vertex {
        position: [ndc_x(point[0] * cell_w), ndc_y(point[1] * cell_h)],
        uv: [0.0, 0.0],
        fg: rgba,
        bg: rgba,
    };
    verts.extend_from_slice(&[
        v(triangle.points[0]),
        v(triangle.points[1]),
        v(triangle.points[2]),
    ]);
}

pub(crate) fn push_rect_px(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Color,
    vp_w: f32,
    vp_h: f32,
) {
    push_rect_px_rgba(verts, x, y, width, height, color.to_rgba(), vp_w, vp_h);
}

fn rounded_rect_points_px(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    segments_per_corner: usize,
) -> Vec<(f32, f32)> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let radius = radius.clamp(0.0, width.min(height) * 0.5);
    if radius <= 0.0 {
        return vec![
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
        ];
    }

    let segments = segments_per_corner.max(2);
    let corners = [
        (x + width - radius, y + radius, -90.0f32, 0.0f32),
        (x + width - radius, y + height - radius, 0.0f32, 90.0f32),
        (x + radius, y + height - radius, 90.0f32, 180.0f32),
        (x + radius, y + radius, 180.0f32, 270.0f32),
    ];
    let mut points = Vec::with_capacity(corners.len() * (segments + 1));
    for (cx, cy, start_deg, end_deg) in corners {
        for i in 0..=segments {
            let t = i as f32 / segments as f32;
            let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
            points.push((cx + angle.cos() * radius, cy + angle.sin() * radius));
        }
    }
    points
}

pub(crate) fn push_rounded_rect_fill_px(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    color: Color,
    vp_w: f32,
    vp_h: f32,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let points = rounded_rect_points_px(x, y, width, height, radius, 8);
    if points.len() < 3 {
        return;
    }

    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let rgba = color.to_rgba();
    let center = (x + width * 0.5, y + height * 0.5);
    let v = |point: (f32, f32)| Vertex {
        position: [ndc_x(point.0), ndc_y(point.1)],
        uv: [0.0, 0.0],
        fg: rgba,
        bg: rgba,
    };

    for i in 0..points.len() {
        let j = (i + 1) % points.len();
        verts.extend_from_slice(&[v(center), v(points[i]), v(points[j])]);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_circle_fill_px(
    verts: &mut Vec<Vertex>,
    cx: f32,
    cy: f32,
    radius: f32,
    color: Color,
    visible_half: widget_render::GpuCircleVisibleHalf,
    vp_w: f32,
    vp_h: f32,
) {
    if radius <= 0.0 {
        return;
    }
    let segments = match visible_half {
        widget_render::GpuCircleVisibleHalf::Full => 32usize,
        widget_render::GpuCircleVisibleHalf::Top | widget_render::GpuCircleVisibleHalf::Bottom => {
            16usize
        }
    };
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let rgba = color.to_rgba();
    let v = |px: f32, py: f32| Vertex {
        position: [ndc_x(px), ndc_y(py)],
        uv: [0.0, 0.0],
        fg: rgba,
        bg: rgba,
    };
    for i in 0..segments {
        let (start_angle, sweep) = match visible_half {
            widget_render::GpuCircleVisibleHalf::Full => (0.0, std::f32::consts::TAU),
            widget_render::GpuCircleVisibleHalf::Top => {
                (std::f32::consts::PI, std::f32::consts::PI)
            }
            widget_render::GpuCircleVisibleHalf::Bottom => (0.0, std::f32::consts::PI),
        };
        let a0 = start_angle + (i as f32 / segments as f32) * sweep;
        let a1 = start_angle + ((i + 1) as f32 / segments as f32) * sweep;
        verts.extend_from_slice(&[
            v(cx, cy),
            v(cx + a0.cos() * radius, cy + a0.sin() * radius),
            v(cx + a1.cos() * radius, cy + a1.sin() * radius),
        ]);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_rounded_rect_border_px(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border_width: f32,
    radius: f32,
    color: Color,
    vp_w: f32,
    vp_h: f32,
) {
    if width <= 0.0 || height <= 0.0 || border_width <= 0.0 {
        return;
    }
    let inset = border_width.min(width * 0.5).min(height * 0.5);
    let radius = radius.clamp(0.0, width.min(height) * 0.5);
    let outer = rounded_rect_points_px(x, y, width, height, radius, 8);
    let inner = rounded_rect_points_px(
        x + inset,
        y + inset,
        (width - inset * 2.0).max(0.0),
        (height - inset * 2.0).max(0.0),
        (radius - inset).max(0.0),
        8,
    );
    if outer.len() < 3 || outer.len() != inner.len() {
        return;
    }

    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let rgba = color.to_rgba();
    let v = |point: (f32, f32)| Vertex {
        position: [ndc_x(point.0), ndc_y(point.1)],
        uv: [0.0, 0.0],
        fg: rgba,
        bg: rgba,
    };

    for i in 0..outer.len() {
        let j = (i + 1) % outer.len();
        verts.extend_from_slice(&[
            v(outer[i]),
            v(inner[i]),
            v(outer[j]),
            v(outer[j]),
            v(inner[i]),
            v(inner[j]),
        ]);
    }
}

pub(crate) fn push_rect_px_rgba(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rgba: [f32; 4],
    vp_w: f32,
    vp_h: f32,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let x0 = ndc_x(x);
    let x1 = ndc_x(x + width);
    let y0 = ndc_y(y);
    let y1 = ndc_y(y + height);
    let v = |px, py| Vertex {
        position: [px, py],
        uv: [0.0, 0.0],
        fg: rgba,
        bg: rgba,
    };
    verts.extend_from_slice(&[
        v(x0, y0),
        v(x0, y1),
        v(x1, y0),
        v(x1, y0),
        v(x0, y1),
        v(x1, y1),
    ]);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_text_cells(
    verts: &mut Vec<Vertex>,
    atlas: &mut GlyphAtlas,
    text: &str,
    col: usize,
    row: usize,
    max_cols: usize,
    fg: [f32; 4],
    bg: [f32; 4],
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) {
    for (j, ch) in text.chars().take(max_cols).enumerate() {
        if ch == ' ' {
            continue;
        }
        rasterize_char(
            atlas,
            ch,
            ((col + j) as f32, row as f32),
            &CharCtx {
                cell_w,
                cell_h,
                vp_w,
                vp_h,
                fg,
                bg,
            },
            verts,
        );
    }
}

// ── Overlay / chrome widget-instance builders ────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_rounded_instance_cells(
    instances: &mut Vec<WidgetInstance>,
    col: f32,
    row: f32,
    width: f32,
    height: f32,
    color: Color,
    radius_px: f32,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) {
    push_rounded_instance_cells_rgba(
        instances,
        col,
        row,
        width,
        height,
        color.to_rgba(),
        cell_w,
        cell_h,
        vp_w,
        vp_h,
    );
    if let Some(instance) = instances.last_mut() {
        instance.corner_radius = normalized_overlay_radius(height, cell_h, radius_px);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_tile_tab_instance_cells(
    instances: &mut Vec<WidgetInstance>,
    col: f32,
    row: f32,
    width: f32,
    height: f32,
    fill: Color,
    border: Color,
    highlight: Color,
    shadow: Color,
    selected: f32,
    normalized_radius: f32,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) {
    push_rounded_instance_cells_rgba(
        instances,
        col,
        row,
        width,
        height,
        fill.to_rgba(),
        cell_w,
        cell_h,
        vp_w,
        vp_h,
    );
    if let Some(instance) = instances.last_mut() {
        instance.value_t = selected;
        instance.color_b = border.to_rgba();
        instance.color_c = highlight.to_rgba();
        instance.color_d = shadow.to_rgba();
        instance.corner_radius = normalized_radius.clamp(0.001, 1.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_rounded_instance_cells_rgba(
    instances: &mut Vec<WidgetInstance>,
    col: f32,
    row: f32,
    width: f32,
    height: f32,
    rgba: [f32; 4],
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let x0 = ndc_x(col * cell_w);
    let x1 = ndc_x((col + width) * cell_w);
    let y0 = ndc_y(row * cell_h);
    let y1 = ndc_y((row + height) * cell_h);
    instances.push(WidgetInstance {
        ndc_min: [x0.min(x1), y1.min(y0)],
        ndc_max: [x0.max(x1), y1.max(y0)],
        value_t: 0.0,
        orientation: 0.0,
        itime: 0.0,
        uniform_a: [0.0; 4],
        uniform_b: [0.0; 4],
        uniform_c: [0.0; 4],
        uniform_d: [0.0; 4],
        color_a: rgba,
        color_b: rgba,
        color_c: rgba,
        color_d: rgba,
        corner_radius: normalized_overlay_radius(height, cell_h, 7.0),
        pixel_aspect: if height > 0.0 {
            (width * cell_w) / (height * cell_h)
        } else {
            1.0
        },
    });
}

fn normalized_overlay_radius(height_cells: f32, cell_h: f32, radius_px: f32) -> f32 {
    if radius_px <= 0.0 || height_cells <= 0.0 || cell_h <= 0.0 {
        return 0.0;
    }
    ((radius_px * 2.0) / (height_cells * cell_h)).clamp(0.001, 0.5)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tile_chrome_instance_px(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius_px: f32,
    border_width_px: f32,
    fill: Color,
    border: Color,
    vp_w: f32,
    vp_h: f32,
) -> Option<WidgetInstance> {
    if width <= 0.0 || height <= 0.0 || vp_w <= 0.0 || vp_h <= 0.0 {
        return None;
    }

    let radius_px = radius_px.clamp(0.0, width.min(height) * 0.5);
    let normalized_radius = if radius_px > 0.0 {
        ((radius_px * 2.0) / height).clamp(0.001, 1.0)
    } else {
        0.0
    };
    let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
    let x0 = ndc_x(x);
    let x1 = ndc_x(x + width);
    let y0 = ndc_y(y);
    let y1 = ndc_y(y + height);

    Some(WidgetInstance {
        ndc_min: [x0.min(x1), y1.min(y0)],
        ndc_max: [x0.max(x1), y1.max(y0)],
        value_t: 0.0,
        orientation: 0.0,
        itime: 0.0,
        uniform_a: [border_width_px.max(0.0), 0.0, 0.0, 0.0],
        uniform_b: [width, height, 0.0, 0.0],
        uniform_c: [0.0; 4],
        uniform_d: [0.0; 4],
        color_a: fill.to_rgba(),
        color_b: border.to_rgba(),
        color_c: [0.0; 4],
        color_d: [0.0; 4],
        corner_radius: normalized_radius,
        pixel_aspect: (width / height).max(0.0001),
    })
}

// ── Segmenting, z-ordering, offsets ──────────────────────────────────────────

pub(crate) fn full_viewport_scissor(vp_w: f32, vp_h: f32) -> ScissorRect {
    ScissorRect::full(
        vp_w.ceil().clamp(0.0, u32::MAX as f32) as u32,
        vp_h.ceil().clamp(0.0, u32::MAX as f32) as u32,
    )
}

pub(crate) fn wrap_completion_doc_lines(lines: &[String], width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for line in lines {
        let mut current = String::new();
        for word in line.split_whitespace() {
            let current_len = current.chars().count();
            let word_len = word.chars().count();
            if current_len == 0 {
                current.push_str(word);
            } else if current_len + 1 + word_len <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        } else if line.trim().is_empty() {
            out.push(String::new());
        }
    }
    out
}

/// Split a flat list of primitives into ranges separated by clip markers.
/// The backend-neutral clip stack owns cell-to-pixel rounding and nested
/// intersections; scissor conversion happens only when a range is encoded.
pub(crate) fn split_prim_segment_ranges(
    primitives: &[widget_render::GpuPrimitive],
    base_scissor: ScissorRect,
    cell_w: f32,
    cell_h: f32,
) -> Vec<(ScissorRect, Range<usize>)> {
    if !primitives.iter().any(|primitive| {
        matches!(
            primitive,
            widget_render::GpuPrimitive::PushClipRect(_) | widget_render::GpuPrimitive::PopClipRect
        )
    }) {
        return vec![(base_scissor, 0..primitives.len())];
    }

    let mut segments = Vec::new();
    let mut clips = ClipStack::new(base_scissor);
    let mut seg_start = 0;

    for (i, prim) in primitives.iter().enumerate() {
        match prim {
            widget_render::GpuPrimitive::PushClipRect(rect) => {
                if i > seg_start {
                    segments.push((clips.current(), seg_start..i));
                }
                clips.push_cells(*rect, cell_w, cell_h);
                seg_start = i + 1;
            }
            widget_render::GpuPrimitive::PopClipRect => {
                if i > seg_start {
                    segments.push((clips.current(), seg_start..i));
                }
                clips.pop();
                seg_start = i + 1;
            }
            _ => {}
        }
    }
    if seg_start < primitives.len() {
        segments.push((clips.current(), seg_start..primitives.len()));
    }
    segments
}

pub(crate) fn z_ordered_primitive_layers(
    primitives: &[widget_render::GpuPrimitive],
) -> Vec<Vec<widget_render::GpuPrimitive>> {
    let has_layers = primitives
        .iter()
        .any(|primitive| matches!(primitive, widget_render::GpuPrimitive::ZLayer { .. }));
    if !has_layers {
        return vec![primitives.to_vec()];
    }
    let mut buckets: BTreeMap<i32, Vec<widget_render::GpuPrimitive>> = BTreeMap::new();
    for primitive in primitives {
        match primitive {
            widget_render::GpuPrimitive::ZLayer { z_index, primitive } => {
                buckets
                    .entry(*z_index)
                    .or_default()
                    .push((**primitive).clone());
            }
            primitive => {
                buckets.entry(0).or_default().push(primitive.clone());
            }
        }
    }
    buckets.into_values().collect()
}

/// Partition widget instances into background and foreground runs in a single
/// pass, preserving draw order within each class.
pub(crate) fn partition_widget_instance_runs(
    primitives: &[widget_render::GpuPrimitive],
) -> (
    Vec<(String, Vec<WidgetInstance>)>,
    Vec<(String, Vec<WidgetInstance>)>,
) {
    let mut bg_runs: Vec<(String, Vec<WidgetInstance>)> = Vec::new();
    let mut fg_runs: Vec<(String, Vec<WidgetInstance>)> = Vec::new();
    for primitive in primitives {
        if let widget_render::GpuPrimitive::WidgetInstance {
            widget_type,
            instance,
            is_background,
        } = widget_render::innermost_primitive(primitive)
        {
            let runs = if *is_background {
                &mut bg_runs
            } else {
                &mut fg_runs
            };
            if let Some((run_type, instances)) = runs.last_mut()
                && run_type == widget_type
            {
                instances.push(*instance);
            } else {
                runs.push((widget_type.clone(), vec![*instance]));
            }
        }
    }
    (bg_runs, fg_runs)
}

pub(crate) fn contains_agent_instrument_stub_animation(
    primitives: &[widget_render::GpuPrimitive],
) -> bool {
    primitives.iter().any(|primitive| {
        matches!(
            widget_render::innermost_primitive(primitive),
            widget_render::GpuPrimitive::WidgetInstance { widget_type, .. }
                if is_agent_instrument_stub_animation_widget_type(widget_type)
        )
    })
}

pub(crate) fn layout_contains_agent_instrument_stub_animation(layout: &LayoutNode) -> bool {
    is_agent_instrument_stub_animation_widget_type(&layout.widget_type)
        || layout_debug_name(layout) == Some(AGENT_INSTRUMENT_STUB_SKELETON_DEBUG_NAME)
        || layout
            .children
            .iter()
            .any(layout_contains_agent_instrument_stub_animation)
}

fn is_agent_instrument_stub_animation_widget_type(widget_type: &str) -> bool {
    widget_type == AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET
        || widget_type.ends_with(AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SUFFIX)
        || widget_type.ends_with(AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SAFE_SUFFIX)
}

fn layout_debug_name(layout: &LayoutNode) -> Option<&str> {
    let value = layout.props.get("debug-name")?;
    let Value::String(debug_name) = value else {
        return None;
    };
    Some(debug_name.as_str())
}

pub(crate) fn extend_right_edge_primitive(
    prim: widget_render::GpuPrimitive,
    layout_width: f32,
    extra_cols: f32,
    cell_w: f32,
    vp_w: f32,
) -> widget_render::GpuPrimitive {
    if extra_cols <= 0.001 || layout_width <= 0.0 {
        return prim;
    }
    let reaches_right = |right: f32| (right - layout_width).abs() <= 0.01;
    match prim {
        widget_render::GpuPrimitive::ZLayer { z_index, primitive } => {
            widget_render::GpuPrimitive::ZLayer {
                z_index,
                primitive: Box::new(extend_right_edge_primitive(
                    *primitive,
                    layout_width,
                    extra_cols,
                    cell_w,
                    vp_w,
                )),
            }
        }
        widget_render::GpuPrimitive::Rect(mut r) => {
            if reaches_right(r.rect.col + r.rect.width) {
                r.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::Rect(r)
        }
        widget_render::GpuPrimitive::ForegroundRect(mut r) => {
            if reaches_right(r.rect.col + r.rect.width) {
                r.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::ForegroundRect(r)
        }
        widget_render::GpuPrimitive::Quad(mut q) => {
            if reaches_right(q.x + q.width) {
                q.width += extra_cols;
            }
            widget_render::GpuPrimitive::Quad(q)
        }
        widget_render::GpuPrimitive::Triangle(mut t) => {
            for point in &mut t.points {
                if reaches_right(point[0]) {
                    point[0] += extra_cols;
                }
            }
            widget_render::GpuPrimitive::Triangle(t)
        }
        widget_render::GpuPrimitive::ProportionalText(mut p) => {
            if p.align_width > 0.0 && reaches_right(p.col + p.align_width) {
                p.align_width += extra_cols;
            }
            widget_render::GpuPrimitive::ProportionalText(p)
        }
        widget_render::GpuPrimitive::PatchCable(mut c) => {
            if reaches_right(c.end[0]) {
                c.end[0] += extra_cols;
                c.control2[0] += extra_cols;
            }
            widget_render::GpuPrimitive::PatchCable(c)
        }
        widget_render::GpuPrimitive::Circle(mut c) => {
            if reaches_right(c.center[0]) {
                c.center[0] += extra_cols;
            }
            widget_render::GpuPrimitive::Circle(c)
        }
        widget_render::GpuPrimitive::Waveform(mut w) => {
            if reaches_right(w.rect.col + w.rect.width) {
                w.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::Waveform(w)
        }
        widget_render::GpuPrimitive::Wavetable(mut w) => {
            if reaches_right(w.rect.col + w.rect.width) {
                w.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::Wavetable(w)
        }
        widget_render::GpuPrimitive::LiveSpectrogram(mut s) => {
            if reaches_right(s.rect.col + s.rect.width) {
                s.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::LiveSpectrogram(s)
        }
        widget_render::GpuPrimitive::Image(mut i) => {
            if reaches_right(i.rect.col + i.rect.width) {
                i.rect.width += extra_cols;
            }
            widget_render::GpuPrimitive::Image(i)
        }
        widget_render::GpuPrimitive::WidgetInstance {
            widget_type,
            mut instance,
            is_background,
        } => {
            let local_right_ndc = -1.0 + (layout_width * cell_w / vp_w) * 2.0;
            if is_background && (instance.ndc_max[0] - local_right_ndc).abs() <= 0.002 {
                let old_width = instance.ndc_max[0] - instance.ndc_min[0];
                instance.ndc_max[0] += (extra_cols * cell_w / vp_w) * 2.0;
                let new_width = instance.ndc_max[0] - instance.ndc_min[0];
                if old_width > 0.0 {
                    instance.pixel_aspect *= new_width / old_width;
                }
            }
            widget_render::GpuPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            }
        }
        widget_render::GpuPrimitive::PushClipRect(mut r) => {
            if reaches_right(r.col + r.width) {
                r.width += extra_cols;
            }
            widget_render::GpuPrimitive::PushClipRect(r)
        }
        other => other,
    }
}

/// Offset a GpuPrimitive by (col_off, row_off) cells (signed for scroll).
/// For Rect/Quad/GlyphRun: shift cell coordinates. For WidgetInstance: shift
/// NDC bounds using the pixel conversion.
pub(crate) fn offset_primitive(
    prim: widget_render::GpuPrimitive,
    col_off: f32,
    row_off: f32,
    cell_w: f32,
    cell_h: f32,
    vp_w: f32,
    vp_h: f32,
) -> widget_render::GpuPrimitive {
    match prim {
        widget_render::GpuPrimitive::ZLayer { z_index, primitive } => {
            widget_render::GpuPrimitive::ZLayer {
                z_index,
                primitive: Box::new(offset_primitive(
                    *primitive, col_off, row_off, cell_w, cell_h, vp_w, vp_h,
                )),
            }
        }
        widget_render::GpuPrimitive::Rect(mut r) => {
            r.rect.col += col_off;
            r.rect.row += row_off;
            widget_render::GpuPrimitive::Rect(r)
        }
        widget_render::GpuPrimitive::ForegroundRect(mut r) => {
            r.rect.col += col_off;
            r.rect.row += row_off;
            widget_render::GpuPrimitive::ForegroundRect(r)
        }
        widget_render::GpuPrimitive::Quad(mut q) => {
            q.x += col_off;
            q.y += row_off;
            widget_render::GpuPrimitive::Quad(q)
        }
        widget_render::GpuPrimitive::Triangle(mut t) => {
            for point in &mut t.points {
                point[0] += col_off;
                point[1] += row_off;
            }
            widget_render::GpuPrimitive::Triangle(t)
        }
        widget_render::GpuPrimitive::GlyphRun(mut g) => {
            g.col += col_off.round() as i32;
            g.row += row_off;
            widget_render::GpuPrimitive::GlyphRun(g)
        }
        widget_render::GpuPrimitive::ProportionalText(mut p) => {
            p.col += col_off;
            p.row += row_off;
            widget_render::GpuPrimitive::ProportionalText(p)
        }
        widget_render::GpuPrimitive::PatchCable(mut c) => {
            c.start[0] += col_off;
            c.start[1] += row_off;
            c.control1[0] += col_off;
            c.control1[1] += row_off;
            c.control2[0] += col_off;
            c.control2[1] += row_off;
            c.end[0] += col_off;
            c.end[1] += row_off;
            c.segment_row += row_off;
            widget_render::GpuPrimitive::PatchCable(c)
        }
        widget_render::GpuPrimitive::Circle(mut c) => {
            c.center[0] += col_off;
            c.center[1] += row_off;
            widget_render::GpuPrimitive::Circle(c)
        }
        widget_render::GpuPrimitive::Waveform(mut w) => {
            w.rect.col += col_off;
            w.rect.row += row_off;
            widget_render::GpuPrimitive::Waveform(w)
        }
        widget_render::GpuPrimitive::Wavetable(mut w) => {
            w.rect.col += col_off;
            w.rect.row += row_off;
            widget_render::GpuPrimitive::Wavetable(w)
        }
        widget_render::GpuPrimitive::LiveSpectrogram(mut s) => {
            s.rect.col += col_off;
            s.rect.row += row_off;
            widget_render::GpuPrimitive::LiveSpectrogram(s)
        }
        widget_render::GpuPrimitive::Image(mut i) => {
            i.rect.col += col_off;
            i.rect.row += row_off;
            widget_render::GpuPrimitive::Image(i)
        }
        widget_render::GpuPrimitive::WidgetInstance {
            widget_type,
            mut instance,
            is_background,
        } => {
            let ndc_dx = (col_off * cell_w / vp_w) * 2.0;
            let ndc_dy = -(row_off * cell_h / vp_h) * 2.0;
            instance.ndc_min[0] += ndc_dx;
            instance.ndc_max[0] += ndc_dx;
            instance.ndc_min[1] += ndc_dy;
            instance.ndc_max[1] += ndc_dy;
            widget_render::GpuPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            }
        }
        widget_render::GpuPrimitive::PushClipRect(mut r) => {
            r.col += col_off;
            r.row += row_off;
            widget_render::GpuPrimitive::PushClipRect(r)
        }
        widget_render::GpuPrimitive::PopClipRect => widget_render::GpuPrimitive::PopClipRect,
    }
}
