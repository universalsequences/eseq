use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use crate::backend::Color;
use crate::layout::{Constraints, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{MetalPrimitive, MetalRectPrimitive, MetalWavetablePrimitive, WidgetViewport};
#[cfg(target_os = "macos")]
use crate::layout::LayoutNode;

pub struct WavetableViewerWidget;

pub static WAVETABLE_VIEWER_WIDGET: WavetableViewerWidget = WavetableViewerWidget;

/// A wavetable bank loaded from a JSON file: `frame_len` samples per wave,
/// `data` laid out wave-major (`wave * frame_len + sample`).
pub struct WavetableBank {
    pub frame_len: usize,
    pub wave_count: usize,
    pub data: Arc<Vec<f32>>,
}

fn bank_cache() -> &'static Mutex<HashMap<String, Option<Arc<WavetableBank>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<WavetableBank>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_bank(path: &str) -> Option<Arc<WavetableBank>> {
    if let Some(cached) = bank_cache().lock().ok()?.get(path) {
        return cached.clone();
    }
    let loaded = read_bank_file(path).map(Arc::new);
    if loaded.is_none() {
        eprintln!("[wavetable-viewer] failed to load bank: {path}");
    }
    bank_cache()
        .lock()
        .ok()?
        .insert(path.to_string(), loaded.clone());
    loaded
}

fn read_bank_file(path: &str) -> Option<WavetableBank> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let (shape, data) = match &json {
        serde_json::Value::Object(map) => (map.get("shape"), map.get("data")?),
        serde_json::Value::Array(_) => (None, &json),
        _ => return None,
    };
    let data: Vec<f32> = data
        .as_array()?
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
        .collect();
    let frame_len = shape
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .and_then(|v| v.as_u64())
        .unwrap_or(512) as usize;
    if frame_len == 0 || data.len() < frame_len {
        return None;
    }
    let wave_count = data.len() / frame_len;
    Some(WavetableBank {
        frame_len,
        wave_count,
        data: Arc::new(data),
    })
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

fn prop_str(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    match props.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Möbius phase bend; identity at warp 0, energy pushed toward the cycle
/// start as warp -> 1. Reference implementation for the math used by both
/// the wavetable dsp.lisp and the Metal fragment shader — keep all three
/// in sync.
#[allow(dead_code)]
fn warp_phase(p: f32, warp: f32) -> f32 {
    let k = 1.0 + 6.0 * warp.clamp(0.0, 1.0);
    (k * p) / (1.0 + (k - 1.0) * p)
}

/// Triangle wavefolder; identity on [-1, 1] at fold 0. Reference for
/// dsp.lisp and the Metal shader — keep in sync.
#[allow(dead_code)]
fn fold_sample(y: f32, fold: f32) -> f32 {
    let g = 1.0 + 6.0 * fold.clamp(0.0, 1.0);
    let v = y * g + 1.0;
    1.0 - (v.rem_euclid(4.0) - 2.0).abs()
}

impl WidgetDefinition for WavetableViewerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["wavetable-viewer"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["set", "wave", "warp", "fold"]
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

    fn tui_render(
        &self,
        props: &HashMap<String, Value>,
        rect: crate::layout::Rect,
        buf: &mut CellBuffer,
    ) {
        let set = prop_num(props, "set", 0.0);
        let wave = prop_num(props, "wave", 0.0);
        let label = format!("wavetable set {set:.0} wave {wave:.1}");
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            if i >= rect.width.round() as usize {
                break;
            }
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, theme::FG_MUTED(), None),
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
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.035, 0.038, 0.042, 1.0),
        );
        let mut primitives = vec![MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: bg_color,
        })];

        let Some(path) = prop_str(&node.props, "file") else {
            return primitives;
        };
        let Some(bank) = load_bank(&path) else {
            return primitives;
        };
        if bank.wave_count == 0 {
            return primitives;
        }

        let waves_per_set = prop_num(&node.props, "waves-per-set", 16.0).max(1.0) as usize;
        let set_count = (bank.wave_count / waves_per_set).max(1);
        let set = (prop_num(&node.props, "set", 0.0).round() as usize).min(set_count - 1);
        let waves_in_set = waves_per_set.min(bank.wave_count - set * waves_per_set);
        if waves_in_set == 0 {
            return primitives;
        }
        let wave_pos = prop_num(&node.props, "wave", 0.0);
        let warp = prop_num(&node.props, "warp", 0.0).clamp(0.0, 1.0);
        let fold = prop_num(&node.props, "fold", 0.0).clamp(0.0, 1.0);

        let selected_color =
            resolve_named_color(&node.props, "wave-color", Color::rgba(1.0, 0.64, 0.22, 1.0));
        let inactive_color = resolve_named_color(
            &node.props,
            "inactive-color",
            Color::rgba(0.46, 0.46, 0.48, 0.55),
        );

        primitives.push(MetalPrimitive::Wavetable(MetalWavetablePrimitive {
            rect: node.rect,
            bank_key: path,
            data: bank.data.clone(),
            frame_len: bank.frame_len as u32,
            set_base: (set * waves_per_set) as u32,
            waves_in_set: waves_in_set as u32,
            wave_pos: wave_pos.clamp(0.0, (waves_in_set - 1) as f32),
            warp,
            fold,
            selected_color,
            inactive_color,
            bg_color,
        }));
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_zero_is_identity() {
        for i in 0..=10 {
            let p = i as f32 / 10.0;
            assert!((warp_phase(p, 0.0) - p).abs() < 1e-6);
        }
    }

    #[test]
    fn warp_pushes_phase_forward() {
        assert!(warp_phase(0.25, 1.0) > 0.5);
    }

    #[test]
    fn fold_zero_is_identity_in_range() {
        for i in -10..=10 {
            let y = i as f32 / 10.0;
            assert!((fold_sample(y, 0.0) - y).abs() < 1e-5, "y={y}");
        }
    }

    #[test]
    fn fold_reflects_out_of_range() {
        // y*g = 2.0 should fold back down to 0.0
        let folded = fold_sample(1.0, 1.0 / 6.0);
        assert!(folded.abs() < 1e-4, "folded={folded}");
        assert!(fold_sample(1.0, 1.0) <= 1.0);
    }
}
