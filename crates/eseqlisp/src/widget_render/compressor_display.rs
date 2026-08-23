//! Activity display for the Compressor builtin: Ableton-style scrolling
//! output-level envelope (gray fill), gain-reduction trace hanging from the
//! top (orange), and the threshold line (cyan) on a shared dB axis.
//!
//! Data comes from the effect node's fine-grained meter ring, published by
//! the host as `comp-meter:`-keyed `CompressorMeterFrame`s (see
//! `sequencer::effects::compressor` and `ui::live_audio_analyzer`).

use std::collections::HashMap;

use super::live_audio::{LiveAudioSourceSelector, source_from_props};
use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use super::{GpuPrimitive, GpuRectPrimitive, GpuTrianglePrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::live_audio::CompressorMeterFrame;
use crate::theme;
use crate::vm::Value;

pub struct CompressorDisplayWidget;

pub static COMPRESSOR_DISPLAY_WIDGET: CompressorDisplayWidget = CompressorDisplayWidget;

/// Shared dB axis: +6 dB at the top edge, -60 dB at the bottom.
const AXIS_MAX_DB: f32 = 6.0;
const AXIS_MIN_DB: f32 = -60.0;
/// Seconds of history shown across the widget width.
const DEFAULT_WINDOW_SECONDS: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub struct CompressorMeterRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
}

pub fn request_from_props(props: &HashMap<String, Value>) -> CompressorMeterRequest {
    let source = source_from_props(props);
    let data_key = format!("comp-meter:{}", source.key_fragment());
    CompressorMeterRequest { data_key, source }
}

/// Collects the live meter requests for every visible compressor-display
/// widget so the host can watch the effect nodes behind them.
pub fn collect_compressor_meter_requests(layout: &LayoutNode) -> Vec<CompressorMeterRequest> {
    fn collect(node: &LayoutNode, requests: &mut Vec<CompressorMeterRequest>) {
        if node.widget_type == "compressor-display"
            && node.rect.width > 0.0
            && node.rect.height > 0.0
        {
            requests.push(request_from_props(&node.props));
        }
        for child in &node.children {
            collect(child, requests);
        }
    }
    let mut requests = Vec::new();
    collect(layout, &mut requests);
    requests
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

/// 0 at the top edge (`AXIS_MAX_DB`) to 1 at the bottom (`AXIS_MIN_DB`).
fn axis_norm(db: f32) -> f32 {
    ((AXIS_MAX_DB - db) / (AXIS_MAX_DB - AXIS_MIN_DB)).clamp(0.0, 1.0)
}

/// Per-column (output dB, gain-reduction dB) pairs, oldest..newest, resampled
/// from the frame's history so `columns` spans `window_seconds`.
///
/// Wide bins average their entries and narrow bins interpolate between
/// neighbors, then a light 3-tap pass smooths both series — the trace should
/// read as a continuous envelope, not a comb of per-bin extremes.
fn column_values(
    frame: Option<&CompressorMeterFrame>,
    columns: usize,
    window_seconds: f32,
) -> Vec<[f32; 2]> {
    let columns = columns.max(2);
    let Some(frame) = frame else {
        return vec![[AXIS_MIN_DB, 0.0]; columns];
    };
    let entry_seconds = frame.stride as f32 / frame.sample_rate.max(1.0);
    let entries_in_window =
        ((window_seconds.max(0.25) / entry_seconds.max(1.0e-6)) as usize).max(2);
    let history = frame.history.as_slice();
    let available = history.len().min(entries_in_window);
    let start = history.len() - available;
    let window = &history[start..];
    let span = window.len() as f32 / columns as f32;
    let mut values: Vec<[f32; 2]> = (0..columns)
        .map(|column| {
            let f0 = column as f32 * span;
            let f1 = f0 + span;
            if span >= 1.0 {
                let lo = f0 as usize;
                let hi = ((f1.ceil() as usize).max(lo + 1)).min(window.len());
                let mut out_db = 0.0f32;
                let mut gr_db = 0.0f32;
                for entry in &window[lo..hi] {
                    out_db += entry[0];
                    gr_db += entry[1];
                }
                let count = (hi - lo) as f32;
                [out_db / count, gr_db / count]
            } else {
                let mid = ((f0 + f1) * 0.5).min(window.len() as f32 - 1.0);
                let index = mid as usize;
                let frac = mid - index as f32;
                let a = window[index];
                let b = window[(index + 1).min(window.len() - 1)];
                [a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]
            }
        })
        .collect();
    let raw = values.clone();
    for i in 1..values.len().saturating_sub(1) {
        for channel in 0..2 {
            values[i][channel] =
                0.25 * raw[i - 1][channel] + 0.5 * raw[i][channel] + 0.25 * raw[i + 1][channel];
        }
    }
    values
}

impl WidgetDefinition for CompressorDisplayWidget {
    fn names(&self) -> &'static [&'static str] {
        &["compressor-display"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["threshold"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(6.0)
                .max(2.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let request = request_from_props(props);
        let frame = crate::live_audio::compressor_meter_frame(&request.data_key);
        let width = rect.width.floor().max(2.0) as usize;
        let height = rect.height.floor().max(2.0) as usize;
        let window = prop_num(props, "window", DEFAULT_WINDOW_SECONDS);
        let values = column_values(frame.as_deref(), width, window);
        let fg = resolve_named_color(props, "level-color", theme::FG_MUTED());
        let gr_color = resolve_named_color(props, "gr-color", theme::WIDGET_SLIDER_FILLED());
        for (column, value) in values.into_iter().enumerate() {
            let level_row = (axis_norm(value[0]) * (height - 1) as f32).round() as usize;
            for row in level_row..height {
                buf.set(
                    rect.row.floor() as u16 + row as u16,
                    rect.col.floor() as u16 + column as u16,
                    styled_cell('▒', fg, None),
                );
            }
            let gr_row = (axis_norm(AXIS_MAX_DB + value[1]) * (height - 1) as f32).round() as usize;
            buf.set(
                rect.row.floor() as u16 + gr_row.min(height - 1) as u16,
                rect.col.floor() as u16 + column as u16,
                styled_cell('─', gr_color, None),
            );
        }
    }

    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let request = request_from_props(&node.props);
        let frame = crate::live_audio::compressor_meter_frame(&request.data_key);
        // One sample point per ~2.5 device pixels; the geometry below joins
        // the points with shared vertices so the silhouette stays continuous.
        let columns = (node.rect.width * viewport.cell_w / 2.5)
            .round()
            .clamp(96.0, 384.0) as usize;
        let window = prop_num(&node.props, "window", DEFAULT_WINDOW_SECONDS);
        let values = column_values(frame.as_deref(), columns, window);
        let threshold = prop_num(&node.props, "threshold", 0.0).clamp(AXIS_MIN_DB, AXIS_MAX_DB);

        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.02, 0.022, 0.025, 1.0),
        );
        let level_color = resolve_named_color(
            &node.props,
            "level-color",
            Color::rgba(0.52, 0.54, 0.56, 0.88),
        );
        let gr_color =
            resolve_named_color(&node.props, "gr-color", Color::rgba(1.0, 0.62, 0.25, 1.0));
        let threshold_color = resolve_named_color(
            &node.props,
            "threshold-color",
            Color::rgba(0.45, 0.78, 0.95, 1.0),
        );

        let rect = node.rect;
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect,
            color: background,
        })];
        let bottom = rect.row + rect.height;
        let column_width = rect.width / (values.len() - 1) as f32;
        let point = |column: usize, db: f32| -> [f32; 2] {
            [
                rect.col + column as f32 * column_width,
                rect.row + axis_norm(db) * rect.height,
            ]
        };
        // Gray output-level fill: one trapezoid (two triangles) per segment,
        // sharing vertices with its neighbors so the sloped silhouette is
        // gapless and continuous (waveform-style envelope, not bars).
        for column in 1..values.len() {
            let p0 = point(column - 1, values[column - 1][0]);
            let p1 = point(column, values[column][0]);
            if p0[1] >= bottom - 0.01 && p1[1] >= bottom - 0.01 {
                continue;
            }
            primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                points: [p0, p1, [p1[0], bottom]],
                color: level_color,
            }));
            primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                points: [p0, [p1[0], bottom], [p0[0], bottom]],
                color: level_color,
            }));
        }
        // Orange gain-reduction trace hanging from the top edge: a proper
        // constant-width stroke. Work in pixel space (cells are not square),
        // offset each vertex along its miter-joined normal, and emit shared-
        // vertex triangles — an SVG-style path, uniform through steep cliffs.
        let (cell_w, cell_h) = (viewport.cell_w.max(1.0), viewport.cell_h.max(1.0));
        let points_px: Vec<[f32; 2]> = (0..values.len())
            .map(|column| {
                let p = point(column, AXIS_MAX_DB + values[column][1]);
                [p[0] * cell_w, p[1] * cell_h]
            })
            .collect();
        let half_px = 0.9;
        let normalize = |v: [f32; 2]| -> [f32; 2] {
            let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if len > 1.0e-6 {
                [v[0] / len, v[1] / len]
            } else {
                [1.0, 0.0]
            }
        };
        let mut left = Vec::with_capacity(points_px.len());
        let mut right = Vec::with_capacity(points_px.len());
        for i in 0..points_px.len() {
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
            let dir_next = if i + 1 < points_px.len() {
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
            let reach = half_px / denom;
            let to_cells = |p: [f32; 2]| [p[0] / cell_w, p[1] / cell_h];
            left.push(to_cells([
                points_px[i][0] + miter[0] * reach,
                points_px[i][1] + miter[1] * reach,
            ]));
            right.push(to_cells([
                points_px[i][0] - miter[0] * reach,
                points_px[i][1] - miter[1] * reach,
            ]));
        }
        for i in 1..values.len() {
            primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                points: [left[i - 1], left[i], right[i]],
                color: gr_color,
            }));
            primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                points: [left[i - 1], right[i], right[i - 1]],
                color: gr_color,
            }));
        }
        // Threshold line.
        primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect {
                row: rect.row + axis_norm(threshold) * rect.height,
                col: rect.col,
                width: rect.width,
                height: (1.5 / viewport.cell_h).max(0.07),
            },
            color: threshold_color,
        }));
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn request_key_uses_comp_meter_prefix() {
        let request = request_from_props(&HashMap::new());
        assert!(request.data_key.starts_with("comp-meter:"));
    }

    #[test]
    fn columns_resample_history_smoothly() {
        // Constant history stays constant through averaging and smoothing,
        // whether the bins are wider (columns < entries) or narrower
        // (columns > entries) than the history resolution.
        let frame = CompressorMeterFrame {
            revision: 1,
            gr_db: -3.0,
            out_db: -30.0,
            sample_rate: 48_000.0,
            stride: 128,
            history: Arc::new(vec![[-30.0, -3.0]; 64]),
        };
        for columns in [8, 256] {
            let values = column_values(Some(&frame), columns, 4.0);
            assert_eq!(values.len(), columns);
            for value in &values {
                assert!((value[0] + 30.0).abs() < 1.0e-4, "{value:?}");
                assert!((value[1] + 3.0).abs() < 1.0e-4, "{value:?}");
            }
        }
    }

    #[test]
    fn an_isolated_event_survives_resampling() {
        let mut history = vec![[-60.0, 0.0]; 64];
        for entry in history[32..40].iter_mut() {
            *entry = [-6.0, -12.0];
        }
        let frame = CompressorMeterFrame {
            revision: 1,
            gr_db: -12.0,
            out_db: -6.0,
            sample_rate: 48_000.0,
            stride: 128,
            history: Arc::new(history),
        };
        let values = column_values(Some(&frame), 32, 64.0 * 128.0 / 48_000.0);
        let peak = values
            .iter()
            .map(|value| value[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let deepest = values.iter().map(|value| value[1]).fold(0.0f32, f32::min);
        assert!(peak > -20.0, "event should remain visible, peak {peak}");
        assert!(
            deepest < -8.0,
            "gain reduction should remain visible, {deepest}"
        );
    }

    #[test]
    fn missing_frame_renders_flat_floor() {
        let values = column_values(None, 3, 4.0);
        assert!(values.iter().all(|v| *v == [AXIS_MIN_DB, 0.0]));
    }
}
