use std::collections::HashMap;

use super::live_audio::{
    LiveAudioSourceSelector, TapPoint, source_from_props, tap_point_from_props,
};
use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, MetalRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct ScopeWidget;

pub static SCOPE_WIDGET: ScopeWidget = ScopeWidget;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ScopeRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
    pub tap_point: TapPoint,
    pub frame_count: usize,
}

fn prop_string(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| match value {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

pub fn request_from_props(props: &HashMap<String, Value>) -> ScopeRequest {
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
        format!(
            "scope:{}:{}:{frame_count}",
            source.key_fragment(),
            tap_point.key_fragment()
        )
    });
    ScopeRequest {
        data_key,
        source,
        tap_point,
        frame_count,
    }
}

pub fn collect_scope_requests(layout: &LayoutNode) -> Vec<ScopeRequest> {
    fn collect(node: &LayoutNode, requests: &mut Vec<ScopeRequest>) {
        if node.widget_type == "scope" {
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

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let request = request_from_props(&node.props);
        let sample_count = (node.rect.width * 4.0).round().clamp(32.0, 512.0) as usize;
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
        let mut primitives = vec![MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: background,
        })];
        let plot_height = (node.rect.height - 0.8).max(0.2);
        let top = node.rect.row + 0.4;
        for index in 1..samples.len() {
            let x0 =
                node.rect.col + node.rect.width * (index - 1) as f32 / (samples.len() - 1) as f32;
            let x1 = node.rect.col + node.rect.width * index as f32 / (samples.len() - 1) as f32;
            let y0 = top + (1.0 - (samples[index - 1] + 1.0) * 0.5) * plot_height;
            let y1 = top + (1.0 - (samples[index] + 1.0) * 0.5) * plot_height;
            primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: y0.min(y1),
                    col: x0,
                    width: (x1 - x0).max(0.04),
                    height: (y1 - y0).abs().max(0.07),
                },
                color: waveform,
            }));
        }
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
}
