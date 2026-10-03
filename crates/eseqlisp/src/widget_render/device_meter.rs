//! Thin stereo output meter drawn between devices in the FX panel, after
//! Ableton's inter-device meters: instrument [meter] effect [meter] effect ...
//!
//! `:source` is the device's `:meter` selector (the same dict shape as the
//! OTT/compressor meters' `:source`, plus `{:kind "rack-slot"}` for a rack
//! slot's instrument). It names the device, not a graph node: the host
//! resolves it to the device's current output node on every poll, so a rack
//! slot or effect rebuilt behind a panel that did not rerun keeps metering.
//! The host meters exactly the visible meters and publishes their levels to
//! `live_audio::device_meter_level`, so drawing is paint-only: no reactive
//! field, no panel rerun. A missing source draws the empty track.

use std::collections::HashMap;

use super::live_audio::{LiveAudioSourceSelector, optional_source_from_props};
use super::{
    CellBuffer, GpuPrimitive, GpuRectPrimitive, WidgetDefinition, WidgetViewport, get_f32_prop,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct DeviceMeterWidget;

pub static DEVICE_METER_WIDGET: DeviceMeterWidget = DeviceMeterWidget;

/// Display-level boundaries (meter_display_level units, -60..0 dBFS) where the
/// fill turns yellow and red: about -18 dBFS and -7 dBFS.
const YELLOW_FROM: f32 = 0.70;
const RED_FROM: f32 = 0.88;

/// Device selectors of every visible `device-meter` widget, so the host
/// meters only what is on screen.
pub fn collect_device_meter_sources(layout: &LayoutNode, sources: &mut Vec<LiveAudioSourceSelector>) {
    if layout.widget_type == "device-meter" && layout.rect.width > 0.0 && layout.rect.height > 0.0
    {
        if let Some(source) = optional_source_from_props(&layout.props) {
            sources.push(source);
        }
    }
    for child in &layout.children {
        collect_device_meter_sources(child, sources);
    }
}

fn read_levels(props: &HashMap<String, Value>) -> [f32; 2] {
    optional_source_from_props(props)
        .and_then(|source| crate::live_audio::device_meter_level(&source.key_fragment()))
        .map(|[l, r]| [l.clamp(0.0, 1.0), r.clamp(0.0, 1.0)])
        .unwrap_or([0.0, 0.0])
}

/// One bar's fill, bottom-up: green, then yellow, then red above the
/// boundaries the level reaches.
fn push_bar(prims: &mut Vec<GpuPrimitive>, track: Rect, level: f32, track_color: Color) {
    prims.push(GpuPrimitive::Rect(GpuRectPrimitive { rect: track, color: track_color }));
    let bands = [
        (0.0, YELLOW_FROM, Color::rgba(0.10, 0.85, 0.30, 1.0)),
        (YELLOW_FROM, RED_FROM, Color::rgba(0.96, 0.82, 0.18, 1.0)),
        (RED_FROM, 1.0, Color::rgba(0.95, 0.18, 0.16, 1.0)),
    ];
    for (from, to, color) in bands {
        let top = level.min(to);
        if top <= from {
            break;
        }
        let height = (top - from) * track.height;
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect {
                row: track.row + track.height * (1.0 - top),
                col: track.col,
                width: track.width,
                height,
            },
            color,
        }));
    }
}

impl WidgetDefinition for DeviceMeterWidget {
    fn names(&self) -> &'static [&'static str] {
        &["device-meter"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width").map(f64_to_f32).unwrap_or(0.9);
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_height.min(8.0));
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let [l, r] = read_levels(props);
        let rows = rect.height.round().max(1.0) as u16;
        let lit = ((l.max(r)) * rows as f32).round() as u16;
        for row in 0..rows {
            let glyph = if rows - row <= lit { '█' } else { '│' };
            buf.set(
                rect.row.round() as u16 + row,
                rect.col.round() as u16,
                styled_cell(glyph, theme::FG_MUTED(), None),
            );
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let [level_l, level_r] = read_levels(&node.props);
        let track_color =
            resolve_named_color(&node.props, "track-color", Color::rgba(0.045, 0.048, 0.052, 1.0));
        let bar_gap = get_f32_prop(&node.props, "bar-gap", 0.08);
        let inset = get_f32_prop(&node.props, "inset", 0.2);
        let bar_w = ((node.rect.width - bar_gap) * 0.5).max(0.05);
        let bar = |col: f32| Rect {
            row: node.rect.row + inset,
            col,
            width: bar_w,
            height: (node.rect.height - inset * 2.0).max(0.0),
        };
        let mut prims = Vec::with_capacity(8);
        push_bar(&mut prims, bar(node.rect.col), level_l, track_color);
        push_bar(&mut prims, bar(node.rect.col + bar_w + bar_gap), level_r, track_color);
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(fields: Vec<(&str, Value)>) -> Value {
        Value::Map(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_string(), std::rc::Rc::new(std::cell::RefCell::new(value))))
                .collect(),
        )
    }

    fn rack_slot(index: f64, rack_slot: f64) -> Value {
        source(vec![
            ("kind", Value::String("rack-slot".to_string())),
            ("index", Value::Number(index)),
            ("rack-slot", Value::Number(rack_slot)),
        ])
    }

    fn layout(source: Option<Value>, width: f32) -> LayoutNode {
        let mut props = HashMap::new();
        if let Some(source) = source {
            props.insert("source".to_string(), source);
        }
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "device-meter".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width, height: 10.0 },
            props,
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        }
    }

    #[test]
    fn collects_only_visible_meters_with_a_node() {
        let mut root = layout(None, 1.0);
        root.widget_type = "h-stack".to_string();
        root.children = vec![
            layout(Some(rack_slot(2.0, 1.0)), 0.9),
            layout(Some(rack_slot(3.0, 0.0)), 0.0),
            layout(None, 0.9),
            layout(Some(Value::Number(41.0)), 0.9),
        ];
        let mut sources = Vec::new();
        collect_device_meter_sources(&root, &mut sources);
        assert_eq!(sources, vec![LiveAudioSourceSelector::RackSlot { index: 2, rack_slot: 1 }]);
    }

    #[test]
    fn fill_reaches_red_only_above_its_boundary() {
        let track = Rect { row: 0.0, col: 0.0, width: 0.4, height: 10.0 };
        let colors = |level: f32| {
            let mut prims = Vec::new();
            push_bar(&mut prims, track, level, Color::rgba(0.0, 0.0, 0.0, 1.0));
            prims.len() - 1
        };
        assert_eq!(colors(0.0), 0);
        assert_eq!(colors(0.5), 1);
        assert_eq!(colors(0.8), 2);
        assert_eq!(colors(1.0), 3);
    }
}
