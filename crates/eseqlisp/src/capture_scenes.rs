//! Backend-neutral inputs for the deterministic shader captures.
//!
//! The scene list, the procedurally generated textures and sample data, and
//! the vertex/instance buffers are identical for every backend: the whole
//! point of the capture set is that the only thing that varies between two
//! captures is the shader language and the GPU that ran it. The wgpu renderer
//! lives in [`crate::shader_capture`] and the Metal one in
//! [`crate::metal_shader_capture`]; both draw exactly the data below.
//!
//! Nothing here touches a graphics API — no fonts, no clock, no sample files —
//! so two runs on one host produce byte-identical buffers.

use crate::ui::gpu_geometry::{
    ImageVertex, LiveSpectrogramInstance, PatchCableInstance, Vertex, WaveformInstance,
    WavetableInstance,
};
use crate::widget_render::{self, ShaderBackend, WidgetInstance};

/// The MSL half of the editable button surface, paired here with
/// [`crate::ui::wgsl_shaders::BUTTON_SURFACE_WGSL`].
const BUTTON_SURFACE_MSL: &str = include_str!("../shaders/button_surface.metal");

pub const WIDTH: u32 = 512;
pub const HEIGHT: u32 = 256;

/// One capture per core pipeline followed by one per distinct retained-mode
/// widget fragment. Widget aliases that share a body are intentionally omitted.
pub const SCENES: &[&str] = &[
    "text",
    "proportional-text",
    "image",
    "patch-cable",
    "widget-surface",
    "wavetable",
    "waveform",
    "live-spectrogram",
    "widget-adsr-editor",
    "widget-box",
    "widget-button",
    "widget-button-icon",
    "widget-dropdown-chevron",
    "widget-slider",
    "widget-knob",
    "widget-matrix",
    "widget-modulator-curve",
    "widget-lfo-curve",
    "widget-multiband-meter",
    "widget-response-curve-editor",
    "widget-scroll",
    "widget-sound-glyph",
    "widget-timeline-cursor-marker",
    "widget-toggle",
    "widget-tree",
    "widget-vslider",
    "widget-phaser-notch",
    "widget-roar-shaper",
    "widget-knob-number",
    "widget-knob-number-mod-range",
    "widget-knob-number-mod-dot",
    "widget-roar-filter",
    "widget-number-picker-tri",
    "widget-tile-chrome",
    "widget-tile-tab",
    "widget-patcher-panel",
    "widget-patcher-port",
    "widget-patcher-back-chevron",
    "widget-patcher-node",
    "widget-dropdown-checkmark",
];

/// The `(vertex, fragment)` sources a `widget-<name>` scene draws with, in the
/// requested shader language, or `None` when the scene is not a widget scene.
///
/// A widget scene that resolves to `None` for one backend but not the other is
/// exactly the "this fragment was never ported" case the capture set exists to
/// catch, so callers must not silently fall back to another widget's shader.
pub fn widget_scene_shader(
    scene: &str,
    backend: ShaderBackend,
) -> Option<(Option<&'static str>, &'static str)> {
    let name = scene.strip_prefix("widget-")?;
    if name == "surface" {
        // Not a registered widget: `widget-surface` draws the editable button
        // surface, which each backend loads from its own file under
        // `shaders/`. Both files are hot-reload overrides concatenated onto the
        // widget preamble, so they pair the same way here as at runtime.
        return Some((
            None,
            match backend {
                ShaderBackend::Msl => BUTTON_SURFACE_MSL,
                ShaderBackend::Wgsl => crate::ui::wgsl_shaders::BUTTON_SURFACE_WGSL,
            },
        ));
    }
    widget_render::widget_shader_sources(backend)
        .into_iter()
        .find_map(|(shader_name, vertex, fragment)| {
            (shader_name == name).then_some((vertex, fragment))
        })
}

/// Pixel (top-left origin, y down) to normalized device coordinates.
pub fn ndc(x: f32, y: f32) -> [f32; 2] {
    [x / WIDTH as f32 * 2.0 - 1.0, 1.0 - y / HEIGHT as f32 * 2.0]
}

// ── Procedural inputs ────────────────────────────────────────────────────

/// A 64×64 single-channel atlas of sixteen 16×16 cells, each holding an
/// antialiased disc whose radius grows with the cell index. It stands in for a
/// glyph atlas: the only thing the text pipelines read is `.r` coverage, and a
/// coverage ramp exercises both the nearest and the linear sampler.
pub fn glyph_atlas_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 64 * 64];
    for cell in 0..16u32 {
        let (cx, cy) = (cell % 4, cell / 4);
        let radius = 2.0 + cell as f32 * 0.35;
        for y in 0..16u32 {
            for x in 0..16u32 {
                let dx = x as f32 + 0.5 - 8.0;
                let dy = y as f32 + 0.5 - 8.0;
                let d = (dx * dx + dy * dy).sqrt() - radius;
                let coverage = (0.5 - d).clamp(0.0, 1.0);
                pixels[((cy * 16 + y) * 64 + cx * 16 + x) as usize] = (coverage * 255.0) as u8;
            }
        }
    }
    pixels
}

/// A 64×64 RGBA checkerboard tinted by a diagonal gradient, so the image
/// pipeline's rotation and rounding are both visible against known content.
pub fn image_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 64 * 64 * 4];
    for y in 0..64u32 {
        for x in 0..64u32 {
            let checker = ((x / 8) + (y / 8)) % 2 == 0;
            let t = (x + y) as f32 / 126.0;
            let base = if checker { 0.85 } else { 0.20 };
            let offset = ((y * 64 + x) * 4) as usize;
            pixels[offset] = (base * 255.0) as u8;
            pixels[offset + 1] = (base * t * 255.0) as u8;
            pixels[offset + 2] = (base * (1.0 - t) * 255.0) as u8;
            pixels[offset + 3] = 255;
        }
    }
    pixels
}

pub const WAVETABLE_FRAME_LEN: u32 = 256;
pub const WAVETABLE_WAVES: u32 = 4;

/// Four classic single-cycle shapes, so morphing between neighbours is visible.
pub fn wavetable_bank() -> Vec<f32> {
    let mut bank = Vec::with_capacity((WAVETABLE_FRAME_LEN * WAVETABLE_WAVES) as usize);
    for wave in 0..WAVETABLE_WAVES {
        for i in 0..WAVETABLE_FRAME_LEN {
            let phase = i as f32 / WAVETABLE_FRAME_LEN as f32;
            let value = match wave {
                0 => (phase * std::f32::consts::TAU).sin(),
                1 => phase * 2.0 - 1.0,
                2 => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                _ => 1.0 - (phase * 4.0 - 1.0).abs().min(1.0) * 2.0,
            };
            bank.push(value);
        }
    }
    bank
}

pub const WAVEFORM_BUCKETS: u32 = 256;

/// Min/max pairs from a decaying sine burst: amplitude sweeps from full scale
/// down to near silence, so the fill, the edge highlight and the minimum
/// thickness clamp are all exercised in one capture.
pub fn waveform_buckets() -> Vec<f32> {
    let mut data = Vec::with_capacity(WAVEFORM_BUCKETS as usize * 2);
    for i in 0..WAVEFORM_BUCKETS {
        let t = i as f32 / (WAVEFORM_BUCKETS - 1) as f32;
        let envelope = (1.0 - t).powf(1.6) * (0.35 + 0.65 * (t * 22.0).sin().abs());
        data.push(-envelope);
        data.push(envelope);
    }
    data
}

pub const SPECTROGRAM_BINS: u32 = 128;
pub const SPECTROGRAM_SLICES: u32 = 64;

/// A drifting formant peak over time: the waterfall rows sweep the peak up the
/// spectrum, so a row-ordering mistake shows as a discontinuity.
pub fn spectrogram_waterfall() -> Vec<f32> {
    let mut data = Vec::with_capacity((SPECTROGRAM_BINS * SPECTROGRAM_SLICES) as usize);
    for row in 0..SPECTROGRAM_SLICES {
        let center = 12.0 + row as f32 * 1.4;
        for bin in 0..SPECTROGRAM_BINS {
            let d = (bin as f32 - center) / 9.0;
            let peak = (-d * d).exp();
            let floor = 0.05 * (1.0 - bin as f32 / SPECTROGRAM_BINS as f32);
            data.push((peak + floor).min(1.0));
        }
    }
    data
}

/// A single smoothed spectrum row for the EQ mode, with two resonances.
pub fn spectrogram_smoothed() -> Vec<f32> {
    (0..SPECTROGRAM_BINS)
        .map(|bin| {
            let x = bin as f32 / SPECTROGRAM_BINS as f32;
            let low = (-((x - 0.18) / 0.10).powi(2)).exp();
            let high = 0.7 * (-((x - 0.62) / 0.06).powi(2)).exp();
            (0.12 + low + high).min(1.0)
        })
        .collect()
}

// ── Scene geometry ───────────────────────────────────────────────────────

/// Six vertices covering one axis-aligned quad, carrying uv, fg and bg.
fn text_quad(
    x: f32,
    y: f32,
    size: f32,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    fg: [f32; 4],
    bg: [f32; 4],
) -> [Vertex; 6] {
    let corners = [
        ([0.0, 0.0], [uv_min[0], uv_min[1]]),
        ([0.0, 1.0], [uv_min[0], uv_max[1]]),
        ([1.0, 0.0], [uv_max[0], uv_min[1]]),
        ([1.0, 0.0], [uv_max[0], uv_min[1]]),
        ([0.0, 1.0], [uv_min[0], uv_max[1]]),
        ([1.0, 1.0], [uv_max[0], uv_max[1]]),
    ];
    std::array::from_fn(|i| {
        let (corner, uv) = corners[i];
        Vertex {
            position: ndc(x + corner[0] * size, y + corner[1] * size),
            uv,
            fg,
            bg,
        }
    })
}

/// Eight glyph cells across the frame, each with its own foreground color and
/// a background that alternates so the monospace fragment's `mix(bg, fg, cov)`
/// is visibly doing both halves of the blend.
pub fn text_vertices() -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for cell in 0..8u32 {
        let (cx, cy) = (cell % 4, cell / 4);
        let uv_min = [cx as f32 * 0.25, cy as f32 * 0.25];
        let uv_max = [uv_min[0] + 0.25, uv_min[1] + 0.25];
        let hue = cell as f32 / 7.0;
        let fg = [
            0.25 + 0.75 * hue,
            0.90 - 0.55 * hue,
            0.35 + 0.5 * (1.0 - hue),
            1.0,
        ];
        let bg = if cell % 2 == 0 {
            [0.10, 0.12, 0.18, 1.0]
        } else {
            [0.24, 0.10, 0.12, 1.0]
        };
        vertices.extend(text_quad(
            16.0 + cell as f32 * 60.0,
            96.0,
            56.0,
            uv_min,
            uv_max,
            fg,
            bg,
        ));
    }
    vertices
}

fn image_quad(
    x: f32,
    y: f32,
    size: f32,
    opacity: f32,
    radius: f32,
    rotation: f32,
    clip_circle: f32,
) -> [ImageVertex; 6] {
    let half = size * 0.5;
    let corners = [
        [0.0, 0.0],
        [0.0, 1.0],
        [1.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 1.0],
    ];
    std::array::from_fn(|i| {
        let corner = corners[i];
        ImageVertex {
            position: ndc(x + corner[0] * size, y + corner[1] * size),
            uv: corner,
            opacity,
            local_pos: [(corner[0] - 0.5) * size, (corner[1] - 0.5) * size],
            half_size: [half, half],
            radius,
            rotation,
            clip_circle,
        }
    })
}

/// Four quads: unclipped, rounded, circle-clipped, and rotated with partial
/// opacity — one per branch of the image fragment.
pub fn image_vertices() -> Vec<ImageVertex> {
    let mut vertices = Vec::new();
    vertices.extend(image_quad(24.0, 64.0, 96.0, 1.0, 0.0, 0.0, 0.0));
    vertices.extend(image_quad(144.0, 64.0, 96.0, 1.0, 24.0, 0.0, 0.0));
    vertices.extend(image_quad(264.0, 64.0, 96.0, 1.0, 0.0, 0.0, 1.0));
    vertices.extend(image_quad(384.0, 64.0, 96.0, 0.55, 16.0, 0.4, 0.0));
    vertices
}

pub fn patch_cable_instances() -> Vec<PatchCableInstance> {
    let cable = |bounds: [f32; 4],
                 start: [f32; 2],
                 control1: [f32; 2],
                 control2: [f32; 2],
                 end: [f32; 2],
                 color: [f32; 4],
                 radius_px: f32,
                 is_segmented: f32,
                 segment_y_px: f32| PatchCableInstance {
        ndc_min: ndc(bounds[0], bounds[1]),
        ndc_max: ndc(bounds[2], bounds[3]),
        bounds_min: [bounds[0], bounds[1]],
        bounds_max: [bounds[2], bounds[3]],
        start,
        control1,
        control2,
        end,
        color,
        radius_px,
        is_segmented,
        segment_y_px,
        corner_radius_px: 12.0,
    };
    vec![
        // Bezier: a long horizontal S-curve.
        cable(
            [16.0, 16.0, 240.0, 120.0],
            [32.0, 40.0],
            [140.0, 40.0],
            [120.0, 100.0],
            [224.0, 100.0],
            [0.20, 0.75, 1.00, 1.0],
            4.0,
            0.0,
            0.0,
        ),
        // Thicker bezier, warmer, so radius and color both vary between draws.
        cable(
            [16.0, 128.0, 240.0, 240.0],
            [32.0, 224.0],
            [180.0, 224.0],
            [80.0, 152.0],
            [224.0, 152.0],
            [1.00, 0.55, 0.18, 1.0],
            7.0,
            0.0,
            0.0,
        ),
        // Segmented: the orthogonal router with rounded corners.
        cable(
            [264.0, 16.0, 496.0, 240.0],
            [296.0, 48.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [464.0, 208.0],
            [0.55, 1.00, 0.45, 1.0],
            5.0,
            1.0,
            128.0,
        ),
    ]
}

pub fn widget_instances() -> Vec<WidgetInstance> {
    let instance =
        |x: f32, y: f32, w: f32, h: f32, shape: f32, radius: f32, tint: [f32; 4]| WidgetInstance {
            ndc_min: ndc(x, y + h),
            ndc_max: ndc(x + w, y),
            value_t: 0.5,
            orientation: 0.0,
            itime: 0.0,
            uniform_a: [shape, 0.0, 0.0, 0.0],
            uniform_b: [0.0; 4],
            uniform_c: [0.0, 0.0, 1.0, 1.0],
            uniform_d: [0.0; 4],
            color_a: tint,
            color_b: [0.85, 0.88, 0.95, 1.0],
            color_c: [1.0, 1.0, 1.0, 0.65],
            color_d: [0.02, 0.03, 0.06, 0.8],
            corner_radius: radius,
            pixel_aspect: w / h,
        };
    vec![
        instance(24.0, 40.0, 128.0, 80.0, 0.0, 0.0, [0.24, 0.42, 0.78, 1.0]),
        instance(184.0, 40.0, 128.0, 80.0, 0.0, 0.35, [0.72, 0.28, 0.34, 1.0]),
        instance(344.0, 40.0, 128.0, 80.0, 1.0, 0.30, [0.26, 0.60, 0.42, 1.0]),
        instance(
            104.0,
            148.0,
            304.0,
            72.0,
            0.0,
            0.55,
            [0.52, 0.46, 0.20, 1.0],
        ),
    ]
}

pub fn wavetable_instance() -> WavetableInstance {
    WavetableInstance {
        ndc_min: ndc(16.0, 240.0),
        ndc_max: ndc(496.0, 16.0),
        widget_px_w: 480.0,
        widget_px_h: 224.0,
        frame_len: WAVETABLE_FRAME_LEN,
        set_base: 0,
        waves_in_set: WAVETABLE_WAVES,
        wave_pos: 1.6,
        // Mild on purpose: enough warp and fold that both terms are exercised,
        // little enough that the four base shapes stay recognisable when the
        // capture is judged by eye.
        warp: 0.12,
        fold: 0.08,
        domain: 0,
        selected_color: [1.00, 0.78, 0.22, 1.0],
        inactive_color: [0.45, 0.50, 0.58, 0.85],
        bg_color: [0.05, 0.06, 0.09, 1.0],
    }
}

pub fn waveform_instance() -> WaveformInstance {
    WaveformInstance {
        ndc_min: ndc(16.0, 240.0),
        ndc_max: ndc(496.0, 16.0),
        sample_start: 0.0,
        sample_end: 1.0,
        bucket_count: WAVEFORM_BUCKETS,
        aspect_ratio: 224.0 / 480.0,
        selection_start: 0.25,
        selection_end: 0.70,
        show_selection_start: 1,
        show_selection_end: 1,
        playhead_position: 0.45,
        show_playhead: 1,
        waveform_color: [0.36, 0.82, 1.00, 1.0],
        inactive_waveform_color: [0.30, 0.34, 0.40, 1.0],
        marker_color: [0.90, 0.90, 0.95, 1.0],
        active_marker_color: [1.00, 0.72, 0.20, 1.0],
        active_selection_start: 1,
        active_selection_end: 0,
        selection_color: [0.55, 0.70, 1.00, 1.0],
        bg_color: [0.05, 0.06, 0.09, 1.0],
        border_color: [0.35, 0.40, 0.50, 1.0],
    }
}

/// Waterfall on the left, EQ curve on the right, so both branches of the
/// fragment land in one capture.
pub fn live_spectrogram_instances() -> Vec<LiveSpectrogramInstance> {
    let instance = |x0: f32, x1: f32, mode: u32| LiveSpectrogramInstance {
        ndc_min: ndc(x0, 240.0),
        ndc_max: ndc(x1, 16.0),
        widget_px_w: x1 - x0,
        widget_px_h: 224.0,
        bins: SPECTROGRAM_BINS,
        time_slices: SPECTROGRAM_SLICES,
        write_head: 20,
        mode,
        freq_scale: 0,
        sample_rate: 48_000.0,
        display_hz: [40.0, 18_000.0],
        display_hz_padding: [0.0, 0.0],
        min_color: [0.04, 0.05, 0.16, 1.0],
        mid_color: [0.20, 0.55, 0.75, 1.0],
        max_color: [1.00, 0.90, 0.45, 1.0],
        eq_line_color: [0.40, 0.95, 0.80, 1.0],
        eq_fill_color: [0.16, 0.45, 0.42, 0.8],
        background_color: [0.05, 0.06, 0.09, 1.0],
    };
    vec![instance(16.0, 248.0, 0), instance(264.0, 496.0, 1)]
}

/// The widget instances one scene draws: the shared four, with the per-scene
/// uniform overrides a few fragments need to show anything interesting.
///
/// Both renderers call this rather than tweaking [`widget_instances`]
/// themselves, so an override can never apply to one backend's capture only.
pub fn widget_instances_for_scene(scene: &str) -> Vec<WidgetInstance> {
    let mut widgets = widget_instances();
    match scene {
        "widget-slider" => {
            // Keep the fill endpoint away from the exact half-pixel boundary
            // produced by the generic 0.5 value. Mesa can round that one
            // antialiased pixel differently across fresh device processes.
            for widget in &mut widgets {
                widget.value_t = 0.47;
            }
        }
        // Both roar fragments select a mode with `round(value_t)`, and the
        // shared 0.5 is an exact tie — which MSL rounds away from zero and
        // WGSL rounds to even, so the two backends drew different modes and
        // the scene compared nothing (`eseq-linux.76`). Spreading four real
        // mode indices across the instances removes the tie and covers four
        // branches of each fragment instead of one.
        "widget-roar-shaper" => {
            for (index, widget) in widgets.iter_mut().enumerate() {
                widget.value_t = [0.0, 3.0, 7.0, 11.0][index];
                // amount, bias, drive min/max: a mid drive so the transfer
                // curve, the bias marker and the drive band all render.
                widget.uniform_a = [0.35, 0.15, -1.0, 1.0];
            }
        }
        "widget-roar-filter" => {
            for (index, widget) in widgets.iter_mut().enumerate() {
                widget.value_t = [0.0, 2.0, 5.0, 8.0][index];
                // cutoff Hz and resonance: mid-band and resonant, so the
                // response curve crosses the frame instead of pinning to an
                // edge at the 20 Hz clamp the generic uniforms produce.
                widget.uniform_a = [1200.0, 0.65, 0.0, 0.0];
            }
        }
        // shape, pulse width, phase offset (cycles), marker phase: one
        // instance per shape, with the marker off the exact cycle boundary so
        // the capture shows the dot riding the curve.
        "widget-lfo-curve" => {
            for (index, widget) in widgets.iter_mut().enumerate() {
                widget.uniform_a = [[0.0, 1.0, 2.0, 3.0][index], 0.35, 0.25, 0.62];
            }
        }
        "widget-knob-number-mod-range" => {
            for widget in &mut widgets {
                widget.uniform_b = [0.82, 0.15, 0.75, 1.0];
            }
        }
        // The dot fragment reads its whole geometry out of `uniform_b`, and
        // the generic all-zero value clamps to the smallest legal ring and dot
        // (0.10 and 0.005), which draws a couple of pixels at the arc's start
        // angle. Spread the normalized value across the four instances so one
        // capture covers the arc rather than one point on it, and keep
        // `MOD_DOT_RING_RADIUS`'s 0.58 so the dot rides the arc the knob
        // fragment draws.
        //
        // The dot radius is deliberately NOT the widget's `MOD_DOT_RADIUS`
        // (0.084). That is a correct radius relative to a knob's own rect, but
        // these instances are 128×80 and 304×72, so it renders as a ~10×3 px
        // smear: too small to judge the edge falloff or the arc position by
        // eye, and under the "the pipeline drew almost nothing" floor in
        // `shader_capture`'s scene sweep. 0.34 is the same shape, drawn large
        // enough to review.
        "widget-knob-number-mod-dot" => {
            for (index, widget) in widgets.iter_mut().enumerate() {
                widget.uniform_b = [[0.15, 0.4, 0.62, 0.88][index], 0.58, 0.34, 0.0];
            }
        }
        _ => {}
    }
    widgets
}

/// RGBA at pixel (x, y) of a capture readback, top-left origin.
pub fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * WIDTH + x) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

/// Hex sha256 of `bytes`, for the per-PNG digests in a capture manifest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed scene list must retain one capture for every distinct widget
    /// fragment, in both shader languages. Aliases can share a scene, but
    /// adding a shader body without a capture would leave it visually
    /// unreviewed, and a fragment present in one language only would be a
    /// half-finished port that no capture could catch.
    #[test]
    fn widget_scenes_cover_every_distinct_fragment_in_both_languages() {
        use std::collections::BTreeSet;

        for backend in [ShaderBackend::Msl, ShaderBackend::Wgsl] {
            let captured = SCENES
                .iter()
                .filter_map(|scene| widget_scene_shader(scene, backend))
                .map(|(_, fragment)| fragment)
                .collect::<BTreeSet<_>>();
            let missing = widget_render::widget_shader_sources(backend)
                .into_iter()
                .filter_map(|(name, _, fragment)| (!captured.contains(fragment)).then_some(name))
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty(),
                "{backend:?} widget shaders missing captures: {missing:?}"
            );
        }
    }

    /// Every `widget-*` scene must resolve in both languages, or the two
    /// capture directories are not comparable scene for scene.
    #[test]
    fn every_widget_scene_resolves_in_both_languages() {
        for scene in SCENES.iter().filter(|scene| scene.starts_with("widget-")) {
            for backend in [ShaderBackend::Msl, ShaderBackend::Wgsl] {
                assert!(
                    widget_scene_shader(scene, backend).is_some(),
                    "{scene} has no {backend:?} fragment"
                );
            }
        }
    }
}
