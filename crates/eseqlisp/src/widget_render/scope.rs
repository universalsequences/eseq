use std::collections::HashMap;

use super::live_audio::{
    LiveAudioSourceSelector, TapPoint, source_from_props, tap_point_from_props,
};
use super::stroke::ShadedMesh;
use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use super::{GpuPrimitive, GpuRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct ScopeWidget;

pub static SCOPE_WIDGET: ScopeWidget = ScopeWidget;

/// Half-width of the trace ribbon in design pixels. A hair over one pixel wide
/// once the anti-aliasing fringe is added: crisp, but never dotted.
const TRACE_HALF_WIDTH_PX: f32 = 0.7;

/// Half-width of the faint reference grid, in design pixels.
const GRID_HALF_WIDTH_PX: f32 = 0.5;

/// Upper bound on trace points per widget; beyond this the ribbon is already
/// resolving finer than the screen can show.
const MAX_TRACE_POINTS: usize = 1024;

fn default_grid_color(trace: Color) -> Color {
    Color::rgba(trace.r, trace.g, trace.b, 0.10)
}

/// Widest sampling step that still keeps roughly one trace point per design
/// pixel of plot width; finer only adds vertices.
fn trace_points_for_width(width_cells: f32, viewport: WidgetViewport) -> usize {
    let width_px = width_cells * viewport.cell_w.max(1.0) / super::ui_px_scale().max(0.1);
    (width_px * 0.8)
        .round()
        .clamp(32.0, MAX_TRACE_POINTS as f32) as usize
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ScopeRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
    pub tap_point: TapPoint,
    pub frame_count: usize,
    /// 1 = mono mix (the `scope` widget); 2 = interleaved L/R (`xy-scope`).
    pub channels: usize,
}

fn prop_string(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| match value {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

pub fn request_from_props(props: &HashMap<String, Value>) -> ScopeRequest {
    request_from_props_with_channels(props, 1)
}

pub fn request_from_props_with_channels(
    props: &HashMap<String, Value>,
    channels: usize,
) -> ScopeRequest {
    let source = source_from_props(props);
    let tap_point = tap_point_from_props(props);
    let frame_count = props
        .get("frames")
        .and_then(|value| match value {
            Value::Number(value) if value.is_finite() && *value > 0.0 => Some(*value as usize),
            _ => None,
        })
        .unwrap_or(1024)
        .clamp(64, 16_384);
    let data_key = prop_string(props, "data-key").unwrap_or_else(|| {
        let suffix = if channels == 2 { ":stereo" } else { "" };
        format!(
            "scope:{}:{}:{frame_count}{suffix}",
            source.key_fragment(),
            tap_point.key_fragment()
        )
    });
    ScopeRequest {
        data_key,
        source,
        tap_point,
        frame_count,
        channels,
    }
}

pub fn collect_scope_requests(layout: &LayoutNode) -> Vec<ScopeRequest> {
    fn collect(node: &LayoutNode, requests: &mut Vec<ScopeRequest>) {
        if node.widget_type == "scope" {
            requests.push(request_from_props(&node.props));
        } else if node.widget_type == "xy-scope" {
            requests.push(request_from_props_with_channels(&node.props, 2));
        }
        for child in &node.children {
            collect(child, requests);
        }
    }
    let mut requests = Vec::new();
    collect(layout, &mut requests);
    requests
}

fn sampled_values(data_key: &str, width: usize) -> Vec<f32> {
    let Some(frame) = crate::live_audio::scope_frame(data_key) else {
        return vec![0.0; width.max(2)];
    };
    if width <= 1 {
        return vec![*frame.samples.last().unwrap_or(&0.0)];
    }
    (0..width)
        .map(|index| {
            let source_index = index * frame.samples.len().saturating_sub(1) / (width - 1);
            frame.samples[source_index].clamp(-1.0, 1.0)
        })
        .collect()
}

impl WidgetDefinition for ScopeWidget {
    fn names(&self) -> &'static [&'static str] {
        &["scope"]
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
        let width = rect.width.floor().max(2.0) as usize;
        let height = rect.height.floor().max(2.0) as usize;
        let samples = sampled_values(&request.data_key, width);
        let fg = resolve_named_color(props, "waveform-color", theme::WIDGET_SLIDER_FILLED());
        let mid = (height - 1) as f32 * 0.5;
        for (column, sample) in samples.into_iter().enumerate() {
            let row = (mid - sample * mid).round().clamp(0.0, (height - 1) as f32) as u16;
            buf.set(
                rect.row.floor() as u16 + row,
                rect.col.floor() as u16 + column as u16,
                styled_cell('•', fg, None),
            );
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let request = request_from_props(&node.props);
        let sample_count = trace_points_for_width(node.rect.width, viewport);
        let samples = sampled_values(&request.data_key, sample_count);
        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.025, 0.03, 0.035, 0.92),
        );
        let waveform = resolve_named_color(
            &node.props,
            "waveform-color",
            Color::rgba(0.25, 0.9, 0.72, 1.0),
        );
        let grid = resolve_named_color(&node.props, "grid-color", default_grid_color(waveform));
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: background,
        })];
        let plot_height = (node.rect.height - 0.8).max(0.2);
        let top = node.rect.row + 0.4;
        let mut mesh = ShadedMesh::new();
        // Faint zero line under the trace, so a quiet signal still reads as a
        // scope rather than an empty box.
        let zero_y = top + plot_height * 0.5;
        mesh.push_polyline(
            &[
                [node.rect.col, zero_y],
                [node.rect.col + node.rect.width, zero_y],
            ],
            grid,
            viewport,
            GRID_HALF_WIDTH_PX,
        );
        let last = samples.len().saturating_sub(1).max(1) as f32;
        let points: Vec<[f32; 2]> = samples
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let x = node.rect.col + node.rect.width * index as f32 / last;
                let y = top + (1.0 - (sample + 1.0) * 0.5) * plot_height;
                [x, y]
            })
            .collect();
        mesh.push_polyline(&points, waveform, viewport, TRACE_HALF_WIDTH_PX);
        mesh.push_into(&mut primitives);
        primitives
    }
}

/// Stereo X/Y (vectorscope / Lissajous) view of a live tap: left drives x,
/// right drives y, consecutive samples joined into one anti-aliased trace
/// whose older stretches are dimmer, the way a phosphor scope trails. Reads
/// the same tap the `scope` widget does but asks the host for interleaved
/// L/R (`channels: 2`).
pub struct XyScopeWidget;

pub static XY_SCOPE_WIDGET: XyScopeWidget = XyScopeWidget;

/// Grid cells across each axis of the X/Y plot.
const XY_GRID_DIVISIONS: usize = 6;

/// How many brightness steps the phosphor trail is split into.
const XY_TRAIL_RUNS: usize = 24;

fn stereo_pairs(data_key: &str, max_points: usize) -> Vec<(f32, f32)> {
    let Some(frame) = crate::live_audio::scope_frame(data_key) else {
        return Vec::new();
    };
    let pairs = frame.samples.len() / 2;
    if pairs == 0 {
        return Vec::new();
    }
    let step = (pairs / max_points.max(1)).max(1);
    (0..pairs)
        .step_by(step)
        .map(|index| {
            (
                frame.samples[index * 2].clamp(-1.0, 1.0),
                frame.samples[index * 2 + 1].clamp(-1.0, 1.0),
            )
        })
        .collect()
}

impl WidgetDefinition for XyScopeWidget {
    fn names(&self) -> &'static [&'static str] {
        &["xy-scope"]
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
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(8.0)
                .max(2.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let request = request_from_props_with_channels(props, 2);
        let width = rect.width.floor().max(2.0) as usize;
        let height = rect.height.floor().max(2.0) as usize;
        let fg = resolve_named_color(props, "waveform-color", theme::WIDGET_SLIDER_FILLED());
        for (left, right) in stereo_pairs(&request.data_key, 256) {
            let column = ((left + 1.0) * 0.5 * (width - 1) as f32).round() as u16;
            let row = ((1.0 - (right + 1.0) * 0.5) * (height - 1) as f32).round() as u16;
            buf.set(
                rect.row.floor() as u16 + row,
                rect.col.floor() as u16 + column,
                styled_cell('•', fg, None),
            );
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let request = request_from_props_with_channels(&node.props, 2);
        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.025, 0.03, 0.035, 0.92),
        );
        let trace = resolve_named_color(
            &node.props,
            "waveform-color",
            Color::rgba(0.25, 0.9, 0.72, 1.0),
        );
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: background,
        })];
        // Square plot centred in the widget; cells are not square on screen
        // (`aspect` = cell width / cell height in layout units), so the plot
        // width in columns is the height scaled by it.
        let aspect = if viewport.cell_h > 0.0 {
            viewport.cell_w / viewport.cell_h
        } else {
            0.5
        };
        let plot_h = (node.rect.height - 0.6).max(0.4);
        let plot_w = (plot_h / aspect.max(0.05))
            .min(node.rect.width - 0.6)
            .max(0.4);
        let cx = node.rect.col + node.rect.width * 0.5;
        let cy = node.rect.row + node.rect.height * 0.5;
        let grid = resolve_named_color(&node.props, "grid-color", default_grid_color(trace));

        let mut mesh = ShadedMesh::new();
        // Reference grid: GRID_DIVISIONS cells each way across the square
        // plot, drawn first so the trace paints over it.
        let left = cx - plot_w * 0.5;
        let top = cy - plot_h * 0.5;
        for division in 0..=XY_GRID_DIVISIONS {
            let fraction = division as f32 / XY_GRID_DIVISIONS as f32;
            let x = left + plot_w * fraction;
            let y = top + plot_h * fraction;
            mesh.push_polyline(
                &[[x, top], [x, top + plot_h]],
                grid,
                viewport,
                GRID_HALF_WIDTH_PX,
            );
            mesh.push_polyline(
                &[[left, y], [left + plot_w, y]],
                grid,
                viewport,
                GRID_HALF_WIDTH_PX,
            );
        }

        let points: Vec<[f32; 2]> = stereo_pairs(&request.data_key, MAX_TRACE_POINTS)
            .into_iter()
            .map(|(left, right)| [cx + left * plot_w * 0.5, cy - right * plot_h * 0.5])
            .collect();
        // Phosphor trail: the trace is one continuous path, but it is pushed
        // as consecutive runs that share their endpoints so each run can
        // carry its own brightness. Newest run is fully lit; older runs fade.
        if points.len() >= 2 {
            let last = points.len() - 1;
            let run = (points.len() / XY_TRAIL_RUNS).max(2);
            let mut start = 0;
            while start < last {
                let end = (start + run).min(last);
                let age = end as f32 / last as f32; // 0 = oldest, 1 = newest
                let alpha = 0.12 + 0.88 * age * age;
                mesh.push_polyline(
                    &points[start..=end],
                    Color::rgba(trace.r, trace.g, trace.b, trace.a * alpha),
                    viewport,
                    TRACE_HALF_WIDTH_PX,
                );
                start = end;
            }
        }
        mesh.push_into(&mut primitives);
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_identity_includes_source_tap_and_frame_count() {
        let props = HashMap::from([
            ("source".to_string(), Value::Keyword("master".to_string())),
            (
                "tap-point".to_string(),
                Value::Keyword("pre-fx".to_string()),
            ),
            ("frames".to_string(), Value::Number(512.0)),
        ]);
        let request = request_from_props(&props);
        assert_eq!(request.data_key, "scope:master:pre-fx:512");
        assert_eq!(request.frame_count, 512);
    }

    use std::sync::Arc;

    use crate::live_audio::{ScopeFrame, publish_scope_frame};

    fn viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
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

    fn layout_node(widget_type: &str, data_key: &str) -> LayoutNode {
        LayoutNode {
            widget_id: 7,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: widget_type.to_string(),
            rect: Rect {
                row: 2.0,
                col: 3.0,
                width: 40.0,
                height: 10.0,
            },
            props: HashMap::from([("data-key".to_string(), Value::String(data_key.to_string()))]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        }
    }

    /// Every primitive after the background is exactly one shaded mesh, and
    /// every vertex of it lies inside the widget rect.
    fn assert_single_mesh_inside(primitives: &[GpuPrimitive], rect: Rect) -> usize {
        assert!(matches!(primitives.first(), Some(GpuPrimitive::Rect(_))));
        assert_eq!(primitives.len(), 2, "background rect plus one mesh");
        let GpuPrimitive::ForegroundMesh(mesh) = &primitives[1] else {
            panic!("trace should be one anti-aliased mesh, not stacked rects");
        };
        // Allow the fringe to overhang by a fraction of a cell.
        let slack = 0.25;
        for vertex in &mesh.vertices {
            let [x, y] = vertex.point;
            assert!(
                x >= rect.col - slack
                    && x <= rect.col + rect.width + slack
                    && y >= rect.row - slack
                    && y <= rect.row + rect.height + slack,
                "vertex {:?} escapes {:?}",
                vertex.point,
                rect
            );
        }
        mesh.vertices.len()
    }

    #[test]
    fn scope_draws_one_anti_aliased_ribbon_through_the_frame() {
        let key = "test:scope:ribbon";
        let samples: Vec<f32> = (0..1024).map(|index| (index as f32 * 0.05).sin()).collect();
        publish_scope_frame(
            key,
            ScopeFrame {
                revision: 1,
                sample_rate: 48_000.0,
                samples: Arc::new(samples),
            },
        );
        let node = layout_node("scope", key);
        let primitives = SCOPE_WIDGET.build_primitives("scope", &node, viewport());
        let vertices = assert_single_mesh_inside(&primitives, node.rect);
        // 18 vertices per segment; at least the zero line plus a few hundred
        // trace segments for a 40-column widget.
        assert!(vertices > 18 * 200, "only {vertices} vertices");
    }

    #[test]
    fn xy_scope_joins_samples_into_a_fading_trace_over_a_grid() {
        let key = "test:xy-scope:trace";
        let samples: Vec<f32> = (0..1024)
            .flat_map(|index| {
                let phase = index as f32 * 0.03;
                [phase.sin() * 0.9, (phase * 1.5).cos() * 0.9]
            })
            .collect();
        publish_scope_frame(
            key,
            ScopeFrame {
                revision: 1,
                sample_rate: 48_000.0,
                samples: Arc::new(samples),
            },
        );
        let node = layout_node("xy-scope", key);
        let primitives = XY_SCOPE_WIDGET.build_primitives("xy-scope", &node, viewport());
        let vertices = assert_single_mesh_inside(&primitives, node.rect);
        let GpuPrimitive::ForegroundMesh(mesh) = &primitives[1] else {
            unreachable!()
        };
        let grid_vertices = 18 * 2 * (XY_GRID_DIVISIONS + 1);
        assert!(
            vertices > grid_vertices + 18 * 500,
            "only {vertices} vertices"
        );
        // Older runs are dimmer than the newest: the trail fades. Fringe
        // vertices sit at alpha 0, so probe the opaque core of the oldest
        // segment against the brightest core anywhere in the trace.
        let oldest_core_alpha = mesh.vertices[grid_vertices..grid_vertices + 6]
            .iter()
            .map(|vertex| vertex.color.a)
            .fold(0.0_f32, f32::max);
        let brightest_alpha = mesh.vertices[grid_vertices..]
            .iter()
            .map(|vertex| vertex.color.a)
            .fold(0.0_f32, f32::max);
        assert!(oldest_core_alpha > 0.0, "oldest run is invisible");
        assert!(
            oldest_core_alpha < brightest_alpha * 0.5,
            "trail does not fade: oldest {oldest_core_alpha} vs newest {brightest_alpha}"
        );
        assert!(
            (brightest_alpha - 1.0).abs() < 1e-3,
            "newest run is not fully lit"
        );
    }
}
