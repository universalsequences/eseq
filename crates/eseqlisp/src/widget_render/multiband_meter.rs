//! Live band activity display for the OTT multiband dynamics builtin
//! (Ableton Multiband Dynamics-style).
//!
//! Three rows (high / mid / low, top to bottom) on a shared -80..0 dB axis:
//! per-band L/R level bars, an orange marker showing the applied dynamics
//! gain, and a lighter zone between the band's below/above thresholds.
//! Levels and gains arrive as `BandMeterFrame`s published by the host from
//! the effect node's state; thresholds are reactive parameter bindings so
//! dragging the threshold fields moves the zones without a rebuild.

use std::collections::HashMap;

use super::live_audio::{LiveAudioSourceSelector, source_from_props};
use super::{
    CellBuffer, GpuPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::LayoutNode;
use crate::layout::{Constraints, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::live_audio::BandMeterFrame;
use crate::theme;
use crate::vm::Value;

pub struct MultibandMeterWidget;

pub static MULTIBAND_METER_WIDGET: MultibandMeterWidget = MultibandMeterWidget;

const LEVEL_FLOOR_DB: f32 = -80.0;

#[derive(Clone, Debug, PartialEq)]
pub struct BandMeterRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
}

pub fn request_from_props(props: &HashMap<String, Value>) -> BandMeterRequest {
    let source = source_from_props(props);
    let data_key = format!("band-meter:{}", source.key_fragment());
    BandMeterRequest { data_key, source }
}

pub fn collect_band_meter_requests(layout: &LayoutNode) -> Vec<BandMeterRequest> {
    let mut requests = Vec::new();
    collect_band_meter_requests_into(layout, &mut requests);
    requests
}

fn collect_band_meter_requests_into(layout: &LayoutNode, requests: &mut Vec<BandMeterRequest>) {
    if layout.widget_type == "multiband-meter"
        && layout.rect.width > 0.0
        && layout.rect.height > 0.0
    {
        requests.push(request_from_props(&layout.props));
    }
    for child in &layout.children {
        collect_band_meter_requests_into(child, requests);
    }
}

fn value_num(value: &Value) -> Option<f32> {
    match value {
        Value::Number(n) => Some(*n as f32),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot) as f32),
        _ => None,
    }
}

fn prop_num(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    props.get(key).and_then(value_num).unwrap_or(default)
}

fn db_to_x(db: f32) -> f32 {
    ((db - LEVEL_FLOOR_DB) / -LEVEL_FLOOR_DB).clamp(0.0, 1.0)
}

fn tui_bar_filled_cells(level_x: f32, width: usize) -> usize {
    (level_x.clamp(0.0, 1.0) * width as f32)
        .round()
        .clamp(0.0, width as f32) as usize
}

struct Display {
    // Per band (low, mid, high): L/R levels and gain as normalized axis x.
    level_x: [[f32; 2]; 3],
    gain_x: [f32; 3],
    below_x: [f32; 3],
    above_x: [f32; 3],
    // Bit 0..2: band dynamics on; bit 3: low split; bit 4: high split.
    flags: u32,
}

fn display_from_props(props: &HashMap<String, Value>, frame: Option<BandMeterFrame>) -> Display {
    let frame = frame.unwrap_or(BandMeterFrame {
        revision: 0,
        level_db: [[LEVEL_FLOOR_DB; 2]; 3],
        gain_db: [0.0; 3],
    });
    let mut level_x = [[0.0f32; 2]; 3];
    let mut gain_x = [0.0f32; 3];
    let mut below_x = [0.0f32; 3];
    let mut above_x = [0.0f32; 3];
    let mut flags = 0u32;
    for (band, prefix) in ["low", "mid", "high"].into_iter().enumerate() {
        for ch in 0..2 {
            level_x[band][ch] = db_to_x(frame.level_db[band][ch]);
        }
        // Gain stays in axis units relative to the level bar tip.
        gain_x[band] = frame.gain_db[band].clamp(-80.0, 80.0) / -LEVEL_FLOOR_DB;
        below_x[band] = db_to_x(prop_num(
            props,
            &format!("{prefix}-below-thr"),
            LEVEL_FLOOR_DB,
        ));
        above_x[band] = db_to_x(prop_num(props, &format!("{prefix}-above-thr"), 0.0));
        if prop_num(props, &format!("{prefix}-on"), 1.0) > 0.5 {
            flags |= 1 << band;
        }
    }
    if prop_num(props, "low-split", 1.0) > 0.5 {
        flags |= 1 << 3;
    }
    if prop_num(props, "high-split", 1.0) > 0.5 {
        flags |= 1 << 4;
    }
    Display {
        level_x,
        gain_x,
        below_x,
        above_x,
        flags,
    }
}

impl WidgetDefinition for MultibandMeterWidget {
    fn names(&self) -> &'static [&'static str] {
        &["multiband-meter"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "low-below-thr",
            "mid-below-thr",
            "high-below-thr",
            "low-above-thr",
            "mid-above-thr",
            "high-above-thr",
            "low-on",
            "mid-on",
            "high-on",
            "low-split",
            "high-split",
        ]
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
            .clamp(6.0, constraints.max_width.max(6.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(6.0)
            .max(3.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let request = request_from_props(props);
        let display = display_from_props(
            props,
            crate::live_audio::band_meter_frame(&request.data_key),
        );
        let width = rect.width.round().max(1.0) as usize;
        let col_start = rect.col.round() as u16;
        for (row_offset, band) in [2usize, 1, 0].into_iter().enumerate() {
            let row = (rect.row + rect.height * (0.5 + row_offset as f32) / 3.0).round() as u16;
            let filled = tui_bar_filled_cells(
                display.level_x[band][0].max(display.level_x[band][1]),
                width,
            );
            for i in 0..width {
                let ch = if i < filled { '=' } else { '·' };
                buf.set(
                    row,
                    col_start + i as u16,
                    styled_cell(ch, theme::FG_MUTED(), None),
                );
            }
        }
    }

    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(MULTIBAND_METER_SHADER)
    }

    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let request = request_from_props(&node.props);
        let display = display_from_props(
            &node.props,
            crate::live_audio::band_meter_frame(&request.data_key),
        );
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.045, 0.048, 0.052, 1.0),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.34),
        );
        let level_color = resolve_named_color(
            &node.props,
            "level-color",
            Color::rgba(0.36, 0.72, 0.92, 1.0),
        );
        let gain_color =
            resolve_named_color(&node.props, "gain-color", Color::rgba(1.0, 0.62, 0.25, 1.0));
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        vec![GpuPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: display.flags as f32,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [
                    display.level_x[0][0],
                    display.level_x[0][1],
                    display.level_x[1][0],
                    display.level_x[1][1],
                ],
                uniform_b: [
                    display.level_x[2][0],
                    display.level_x[2][1],
                    display.gain_x[0],
                    display.gain_x[1],
                ],
                uniform_c: [
                    display.gain_x[2],
                    display.below_x[0],
                    display.below_x[1],
                    display.below_x[2],
                ],
                uniform_d: [
                    display.above_x[0],
                    display.above_x[1],
                    display.above_x[2],
                    0.0,
                ],
                color_a: level_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: gain_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

const MULTIBAND_METER_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float4 col = in.color_b;
    int flags = int(round(in.value_t));

    // Rows top to bottom: high (band 2), mid (1), low (0).
    float rowF = uv.y * 3.0;
    int row = clamp(int(floor(rowF)), 0, 2);
    int band = 2 - row;
    float rowY = rowF - float(row); // 0 at row top, 1 at row bottom

    float levelL = (band == 0) ? in.uniform_a.x : ((band == 1) ? in.uniform_a.z : in.uniform_b.x);
    float levelR = (band == 0) ? in.uniform_a.y : ((band == 1) ? in.uniform_a.w : in.uniform_b.y);
    float gain   = (band == 0) ? in.uniform_b.z : ((band == 1) ? in.uniform_b.w : in.uniform_c.x);
    float belowX = (band == 0) ? in.uniform_c.y : ((band == 1) ? in.uniform_c.z : in.uniform_c.w);
    float aboveX = (band == 0) ? in.uniform_d.x : ((band == 1) ? in.uniform_d.y : in.uniform_d.z);
    bool bandOn = (flags & (1 << band)) != 0;
    bool bandActive = (band == 1)
        || (band == 0 && (flags & (1 << 3)) != 0)
        || (band == 2 && (flags & (1 << 4)) != 0);
    float dim = bandActive ? (bandOn ? 1.0 : 0.45) : 0.22;

    // Lighter zone between the below and above thresholds.
    float zone = step(belowX, uv.x) * step(uv.x, aboveX);
    col.rgb = mix(col.rgb, col.rgb + float3(0.045, 0.055, 0.06), zone * dim);

    // Vertical grid every 10 dB, horizontal row separators.
    float gridT = fract(uv.x * 8.0);
    float gridDist = min(gridT, 1.0 - gridT) / 8.0;
    float gridAA = max(fwidth(uv.x), 0.0008);
    float gridMask = smoothstep(gridAA * 1.6, 0.0, gridDist) * 0.55;
    col.rgb = mix(col.rgb, in.color_c.rgb, gridMask * in.color_c.a);
    float sepT = fract(uv.y * 3.0);
    float sepDist = min(sepT, 1.0 - sepT) / 3.0;
    float sepAA = max(fwidth(uv.y), 0.0012);
    float sepMask = smoothstep(sepAA * 1.8, 0.0, sepDist) * 0.8;
    col.rgb = mix(col.rgb, in.color_c.rgb, sepMask * in.color_c.a);

    // Two thin level bars (L above R) plus the orange gain marker between
    // them: it spans from the louder channel's tip to tip + gain.
    float barL = (rowY > 0.16 && rowY < 0.42) ? 1.0 : 0.0;
    float barR = (rowY > 0.58 && rowY < 0.84) ? 1.0 : 0.0;
    float gainBar = (rowY > 0.42 && rowY < 0.58) ? 1.0 : 0.0;
    float aa = max(fwidth(uv.x), 0.0008);
    float maskL = barL * step(0.004, levelL) * smoothstep(levelL + aa, levelL - aa, uv.x);
    float maskR = barR * step(0.004, levelR) * smoothstep(levelR + aa, levelR - aa, uv.x);
    col.rgb = mix(col.rgb, in.color_a.rgb, max(maskL, maskR) * dim);

    // Only draw the gain marker once a signal is present.
    float anchor = max(levelL, levelR);
    float present = step(0.02, anchor);
    float gLo = min(anchor, anchor + gain);
    float gHi = max(anchor, anchor + gain);
    float gainMask = gainBar * present * step(gLo - aa, uv.x) * step(uv.x, gHi + aa);
    // Keep a minimal tick visible at the anchor so the marker reads even at
    // unity gain.
    float tick = gainBar * present
        * smoothstep(aa * 2.4, aa * 0.6, abs(uv.x - anchor));
    col.rgb = mix(col.rgb, in.color_d.rgb, max(gainMask, tick) * dim);

    return col;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn db_axis_maps_floor_and_ceiling() {
        assert!(db_to_x(-80.0) < 1.0e-6);
        assert!((db_to_x(0.0) - 1.0).abs() < 1.0e-6);
        assert!((db_to_x(-40.0) - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn tui_bar_is_empty_at_the_meter_floor() {
        assert_eq!(tui_bar_filled_cells(0.0, 12), 0);
        assert_eq!(tui_bar_filled_cells(1.0, 12), 12);
    }

    #[test]
    fn display_defaults_to_floor_without_a_frame() {
        let display = display_from_props(&HashMap::new(), None);
        assert_eq!(display.level_x, [[0.0; 2]; 3]);
        assert_eq!(display.gain_x, [0.0; 3]);
        // All bands on, both splits on.
        assert_eq!(display.flags, 0b11111);
    }

    #[test]
    fn display_reads_frame_levels_and_gains() {
        let frame = BandMeterFrame {
            revision: 1,
            level_db: [[-40.0, -80.0], [-20.0, -20.0], [0.0, -60.0]],
            gain_db: [8.0, -16.0, 0.0],
        };
        let display = display_from_props(&HashMap::new(), Some(frame));
        assert!((display.level_x[0][0] - 0.5).abs() < 1.0e-6);
        assert!((display.level_x[2][0] - 1.0).abs() < 1.0e-6);
        assert!((display.gain_x[0] - 0.1).abs() < 1.0e-6);
        assert!((display.gain_x[1] + 0.2).abs() < 1.0e-6);
    }

    #[test]
    fn thresholds_read_updated_reactive_slots_without_rebuild() {
        let slot = Arc::new(AtomicU64::new((-60.0_f64).to_bits()));
        let props = HashMap::from([(
            "mid-above-thr".to_string(),
            Value::ReactiveRef {
                namespace: "SEQ".to_string(),
                field: "ott-mid-above-thr".to_string(),
                index: None,
                kind: crate::vm::BindingKind::Float,
                slot: Arc::clone(&slot),
            },
        )]);
        let before = display_from_props(&props, None).above_x[1];
        slot.store((-20.0_f64).to_bits(), std::sync::atomic::Ordering::Release);
        let after = display_from_props(&props, None).above_x[1];
        assert!(after > before);
    }

    #[test]
    fn collects_requests_for_visible_meters() {
        let mut node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "multiband-meter".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: 6.0,
            },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };
        let requests = collect_band_meter_requests(&node);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].data_key.starts_with("band-meter:"));

        node.rect.width = 0.0;
        assert!(collect_band_meter_requests(&node).is_empty());
    }
}
