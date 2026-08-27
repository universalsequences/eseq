//! Small round gate/lamp LED fed by the host's effect state meters.
//!
//! The widget reads a `filterbank-meter:` keyed `BandMeterFrame` published by
//! the sequencer's live-audio analyzer for the effect node named by its
//! `:source` prop (same selector dict as `multiband-meter` / `roar-shaper`):
//! `gain_db[0]` carries the envelope value and `gain_db[1]` the 0/1 gate
//! flag. Lit color comes from `:on-color` (with the envelope magnitude
//! adding a little extra glow), unlit from `:off-color`.

use std::collections::HashMap;

use super::live_audio::{LiveAudioSourceSelector, source_from_props};
use super::{
    CellBuffer, GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive, WidgetDefinition,
    WidgetViewport, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct GateLedWidget;

pub static GATE_LED_WIDGET: GateLedWidget = GateLedWidget;

#[derive(Clone, Debug, PartialEq)]
pub struct GateLedRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
}

pub fn request_from_props(props: &HashMap<String, Value>) -> GateLedRequest {
    let source = source_from_props(props);
    let data_key = format!("filterbank-meter:{}", source.key_fragment());
    GateLedRequest { data_key, source }
}

/// Collects the live meter requests for every visible gate-led widget so the
/// host can watch the effect nodes behind them.
pub fn collect_gate_led_requests(layout: &LayoutNode) -> Vec<GateLedRequest> {
    let mut requests = Vec::new();
    collect_gate_led_requests_into(layout, &mut requests);
    requests
}

fn collect_gate_led_requests_into(layout: &LayoutNode, requests: &mut Vec<GateLedRequest>) {
    if layout.widget_type == "gate-led" && layout.rect.width > 0.0 && layout.rect.height > 0.0 {
        requests.push(request_from_props(&layout.props));
    }
    for child in &layout.children {
        collect_gate_led_requests_into(child, requests);
    }
}

/// (gate lit?, envelope magnitude 0..1) from the published frame, if any.
fn read_gate_env(data_key: &str) -> (bool, f32) {
    match crate::live_audio::band_meter_frame(data_key) {
        Some(frame) => (
            frame.gain_db[1] > 0.5,
            frame.gain_db[0].abs().clamp(0.0, 1.0),
        ),
        None => (false, 0.0),
    }
}

impl WidgetDefinition for GateLedWidget {
    fn names(&self) -> &'static [&'static str] {
        &["gate-led"]
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
            .unwrap_or(0.9)
            .clamp(0.4, constraints.max_width.max(0.4));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(0.9)
            .max(0.4);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        props: &HashMap<String, Value>,
        rect: crate::layout::Rect,
        buf: &mut CellBuffer,
    ) {
        let request = request_from_props(props);
        let (gate_on, _env) = read_gate_env(&request.data_key);
        let glyph = if gate_on { '●' } else { '○' };
        buf.set(
            rect.row.round() as u16,
            rect.col.round() as u16,
            styled_cell(glyph, theme::FG_MUTED(), None),
        );
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let request = request_from_props(&node.props);
        let (gate_on, env) = read_gate_env(&request.data_key);
        let on_color =
            resolve_named_color(&node.props, "on-color", Color::rgba(0.98, 0.78, 0.14, 1.0));
        let off_color =
            resolve_named_color(&node.props, "off-color", Color::rgba(0.24, 0.21, 0.12, 1.0));
        let bezel_color = resolve_named_color(
            &node.props,
            "bezel-color",
            Color::rgba(0.03, 0.03, 0.035, 1.0),
        );

        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let center = [
            node.rect.col + node.rect.width * 0.5,
            node.rect.row + node.rect.height * 0.5,
        ];
        let bezel_inset_px = super::ui_design_px(1.0);
        let lamp_inset_px = super::ui_design_px(2.0);
        let outer_px = ((node.rect.width * cell_w).min(node.rect.height * cell_h) * 0.5
            - bezel_inset_px)
            .max(super::ui_design_px(2.0));
        let lamp_px = (outer_px - lamp_inset_px).max(super::ui_design_px(1.5));
        let lamp_color = if gate_on {
            // The envelope magnitude adds a little glow on top of the lit base.
            let boost = 1.0 + 0.25 * env;
            Color::rgba(
                (on_color.r * boost).min(1.0),
                (on_color.g * boost).min(1.0),
                (on_color.b * boost).min(1.0),
                on_color.a,
            )
        } else {
            off_color
        };
        vec![
            GpuPrimitive::Circle(GpuCirclePrimitive {
                center,
                radius_px: outer_px,
                color: bezel_color,
                visible_half: GpuCircleVisibleHalf::Full,
            }),
            GpuPrimitive::Circle(GpuCirclePrimitive {
                center,
                radius_px: lamp_px,
                color: lamp_color,
                visible_half: GpuCircleVisibleHalf::Full,
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_audio::{BandMeterFrame, publish_band_meter_frame};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn source_props() -> HashMap<String, Value> {
        let mut source = HashMap::new();
        source.insert(
            "kind".to_string(),
            Rc::new(RefCell::new(Value::Keyword("track-effect".to_string()))),
        );
        source.insert(
            "index".to_string(),
            Rc::new(RefCell::new(Value::Number(2.0))),
        );
        source.insert(
            "slot".to_string(),
            Rc::new(RefCell::new(Value::Number(1.0))),
        );
        let mut props = HashMap::new();
        props.insert("source".to_string(), Value::Map(source));
        props
    }

    #[test]
    fn request_key_uses_filterbank_meter_prefix() {
        let request = request_from_props(&source_props());
        assert_eq!(request.data_key, "filterbank-meter:track-effect:2:1");
    }

    #[test]
    fn read_gate_env_decodes_published_frame() {
        // Unique key: the frame store is a process-global shared with other
        // tests, so never clear it wholesale here.
        let key = "filterbank-meter:test-gate-led";
        assert_eq!(read_gate_env(key), (false, 0.0));
        let mut frame = BandMeterFrame {
            revision: 1,
            level_db: [[0.0; 2]; 3],
            gain_db: [0.0; 3],
        };
        frame.gain_db[0] = -0.5; // bipolar env
        frame.gain_db[1] = 1.0; // gate on
        publish_band_meter_frame(key, frame);
        let (gate, env) = read_gate_env(key);
        assert!(gate);
        assert!((env - 0.5).abs() < 1e-6);
    }
}
