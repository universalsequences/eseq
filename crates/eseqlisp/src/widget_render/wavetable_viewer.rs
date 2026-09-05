use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use crate::backend::Color;
use crate::layout::{Constraints, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

use super::{GpuPrimitive, GpuRectPrimitive, GpuWavetablePrimitive, WidgetViewport};
use crate::layout::LayoutNode;

pub struct WavetableViewerWidget;

pub static WAVETABLE_VIEWER_WIDGET: WavetableViewerWidget = WavetableViewerWidget;

/// A wavetable bank loaded from a JSON file: `frame_len` samples per wave,
/// `data` laid out wave-major (`wave * frame_len + sample`).
pub struct WavetableBank {
    pub frame_len: usize,
    pub wave_count: usize,
    pub waves_per_set: Option<usize>,
    pub data: Arc<Vec<f32>>,
    pub revision: u64,
}

fn live_banks() -> &'static Mutex<HashMap<String, Arc<WavetableBank>>> {
    static BANKS: OnceLock<Mutex<HashMap<String, Arc<WavetableBank>>>> = OnceLock::new();
    BANKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Publish frame-major data for GPU widgets without serializing large banks
/// through Lisp values. Re-publishing the same Arc is free; replacing it bumps
/// the revision so the Metal buffer is updated in place.
pub fn publish_bank(key: impl Into<String>, frame_len: usize, data: Arc<Vec<f32>>) -> bool {
    if frame_len < 2 || data.len() < frame_len || data.len() % frame_len != 0 {
        return false;
    }
    let key = key.into();
    if let Ok(mut retired) = retired_bank_keys().lock() {
        // Republished before the renderer drained the retirement: keep the
        // GPU buffer instead of evicting one that is live again.
        retired.remove(&key);
    }
    let Ok(mut banks) = live_banks().lock() else {
        return false;
    };
    if banks
        .get(&key)
        .is_some_and(|bank| bank.frame_len == frame_len && Arc::ptr_eq(&bank.data, &data))
    {
        return true;
    }
    static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
    let revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    banks.insert(
        key,
        Arc::new(WavetableBank {
            frame_len,
            wave_count: data.len() / frame_len,
            waves_per_set: None,
            data,
            revision,
        }),
    );
    true
}

/// Keys unpublished since the renderer last drained this set. GPU backends
/// cache one buffer per bank key; without this they would never learn that a
/// key is gone, and per-node keys (`filter-table:{node_id}:…`) never repeat,
/// so every device rebuild would strand a buffer.
fn retired_bank_keys() -> &'static Mutex<HashSet<String>> {
    static RETIRED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    RETIRED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn remove_published_bank(key: &str) {
    if let Ok(mut banks) = live_banks().lock() {
        banks.remove(key);
    }
    if let Ok(mut retired) = retired_bank_keys().lock() {
        retired.insert(key.to_string());
    }
}

/// Drain the keys unpublished since the last call, for a renderer to evict
/// from its GPU cache. Single-consumer: whoever drains owns the notification.
/// Evicting is always safe — a key that came back is simply re-uploaded on
/// its next draw.
pub fn take_retired_bank_keys() -> Vec<String> {
    retired_bank_keys()
        .lock()
        .map(|mut retired| retired.drain().collect())
        .unwrap_or_default()
}

/// Pub so hosts can assert that what a widget displays is exactly what they
/// published (the Filter Table editor's displayed-vs-runtime checks).
pub fn published_bank(key: &str) -> Option<Arc<WavetableBank>> {
    live_banks().lock().ok()?.get(key).cloned()
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
    // Use the same factory/user asset roots as metadata and the compiler.
    // Lisp load roots deliberately exclude user asset libraries, so using
    // them here made saved user instruments show names but no wave display.
    let raw = std::path::Path::new(path);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        super::patcher::resolve_asset_reference(path, Some(std::path::Path::new(".")))?
    };
    let text = std::fs::read_to_string(&resolved).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let (shape, data, waves_per_set) = match &json {
        serde_json::Value::Object(map) => (
            map.get("shape"),
            map.get("data")?,
            map.get("waves_per_set")
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
                .and_then(|value| usize::try_from(value).ok()),
        ),
        serde_json::Value::Array(_) => (None, &json, None),
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
        waves_per_set,
        data: Arc::new(data),
        revision: 0,
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

fn waves_per_set(props: &HashMap<String, Value>, bank: &WavetableBank) -> usize {
    props
        .get("waves-per-set")
        .and_then(value_num)
        .map(|value| value.max(1.0) as usize)
        .or(bank.waves_per_set)
        .unwrap_or(16)
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

// ── Color vocabulary ──────────────────────────────────────────────────────
//
// Every color the viewer draws with is a prop, resolved through
// `resolve_named_color` like the other widgets in this module, so a caller can
// pass a literal `(rgba …)`, a theme keyword (`:accent`), or omit it. The
// defaults below are the values these colors were hardcoded to before they
// became props: omitting a prop renders exactly what it rendered before.
//
//   :background-color    plot background, and the widget's fill when the bank
//                        is missing or empty
//   :wave-color          the highlighted / current wave, morph-interpolated at
//                        the fractional `:wave` position
//   :inactive-color      the other wave strokes behind it (its alpha is the
//                        stroke opacity, hence the < 1.0 default)
//   :label-color         the terminal fallback's text (the GPU path draws no
//                        text)
//
// The shader also paints a soft halo behind the highlighted wave; that is
// derived from `:background-color` rather than being its own color, so it
// follows the background automatically and needs no prop.

/// Default `:background-color` — the near-black plot ground.
pub const DEFAULT_BACKGROUND_COLOR: Color = Color::rgba(0.035, 0.038, 0.042, 1.0);
/// Default `:wave-color` — the amber highlight on the current wave.
pub const DEFAULT_WAVE_COLOR: Color = Color::rgba(1.0, 0.64, 0.22, 1.0);
/// Default `:inactive-color` — dim gray strokes for the rest of the set.
pub const DEFAULT_INACTIVE_COLOR: Color = Color::rgba(0.46, 0.46, 0.48, 0.55);

impl WidgetDefinition for WavetableViewerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["wavetable-viewer"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["set", "wave", "warp", "fold", "cutoff", "resonance"]
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
        let label_color = resolve_named_color(props, "label-color", theme::FG_MUTED());
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            if i >= rect.width.round() as usize {
                break;
            }
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, label_color, None),
            );
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let bg_color =
            resolve_named_color(&node.props, "background-color", DEFAULT_BACKGROUND_COLOR);
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: bg_color,
        })];

        let (bank_key, bank) = if let Some(key) = prop_str(&node.props, "data-key") {
            let Some(bank) = published_bank(&key) else {
                return primitives;
            };
            (key, bank)
        } else if let Some(path) = prop_str(&node.props, "file") {
            let Some(bank) = load_bank(&path) else {
                return primitives;
            };
            (path, bank)
        } else {
            return primitives;
        };
        if bank.wave_count == 0 {
            return primitives;
        }

        let waves_per_set = waves_per_set(&node.props, &bank);
        let set_count = (bank.wave_count / waves_per_set).max(1);
        let set = (prop_num(&node.props, "set", 0.0).round() as usize).min(set_count - 1);
        let waves_in_set = waves_per_set.min(bank.wave_count - set * waves_per_set);
        if waves_in_set == 0 {
            return primitives;
        }
        let mut wave_pos = prop_num(&node.props, "wave", 0.0);
        if matches!(node.props.get("wave-normalized"), Some(Value::Bool(true))) {
            wave_pos *= waves_in_set.saturating_sub(1) as f32;
        }
        let warp = prop_num(&node.props, "warp", 0.0).clamp(0.0, 1.0);
        let fold = prop_num(&node.props, "fold", 0.0).clamp(0.0, 1.0);

        let selected_color = resolve_named_color(&node.props, "wave-color", DEFAULT_WAVE_COLOR);
        let inactive_color =
            resolve_named_color(&node.props, "inactive-color", DEFAULT_INACTIVE_COLOR);

        let magnitude = matches!(node.props.get("domain"), Some(Value::Keyword(value)) if value == "magnitude");
        primitives.push(GpuPrimitive::Wavetable(GpuWavetablePrimitive {
            rect: node.rect,
            bank_key,
            data: bank.data.clone(),
            data_revision: bank.revision,
            frame_len: bank.frame_len as u32,
            set_base: (set * waves_per_set) as u32,
            waves_in_set: waves_in_set as u32,
            wave_pos: wave_pos.clamp(0.0, (waves_in_set - 1) as f32),
            warp,
            fold,
            domain: if magnitude { 1 } else { 0 },
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

    fn test_viewport() -> WidgetViewport {
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

    #[test]
    fn file_viewer_uses_the_same_user_asset_fallback_as_metadata() {
        let root = std::env::temp_dir().join(format!("eseqlisp-viewer-user-roots-{}", std::process::id()));
        let factory = root.join("content");
        let user = root.join("user");
        let reference = "instruments/retained/waves/bank.json";
        std::fs::create_dir_all(&factory).unwrap();
        std::fs::create_dir_all(user.join("instruments/retained/waves")).unwrap();
        std::fs::write(user.join(reference),
            r#"{"shape":[4,2],"waves_per_set":2,"data":[0,1,1,0,0,-1,-1,0]}"#,
        ).unwrap();
        super::super::patcher::set_asset_roots(
            vec![factory.clone(), user.clone()], root.join("user-assets"), root.join("factory-assets"),
        );
        assert!(matches!(crate::editor::asset_metadata_lisp_value(reference, None), Value::Map(_)));
        let bank = read_bank_file(reference).expect("viewer resolves the same retained user bank");
        assert_eq!(bank.frame_len, 4);
        assert_eq!(bank.waves_per_set, Some(2));
        // Explicit paths retain the same result, independent of content roots.
        let absolute = read_bank_file(user.join(reference).to_str().unwrap()).unwrap();
        assert_eq!(absolute.frame_len, bank.frame_len);
        assert_eq!(absolute.waves_per_set, bank.waves_per_set);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_metadata_defaults_waves_per_set_when_the_prop_is_absent() {
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-wavetable-viewer-metadata-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        std::fs::write(
            &path,
            r#"{"shape":[2,4],"waves_per_set":2,"data":[0,1,1,0,0,-1,-1,0]}"#,
        )
        .expect("write wavetable fixture");
        let path = path.to_str().expect("UTF-8 temp path");
        let bank = read_bank_file(path).expect("read bank");
        assert_eq!(bank.waves_per_set, Some(2));

        let node = LayoutNode {
            widget_id: 7,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "wavetable-viewer".to_string(),
            rect: crate::layout::Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: 4.0,
            },
            props: HashMap::from([
                ("file".to_string(), Value::String(path.to_string())),
                ("set".to_string(), Value::Number(1.0)),
            ]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };
        let primitive = WAVETABLE_VIEWER_WIDGET
            .build_primitives("wavetable-viewer", &node, test_viewport())
            .into_iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Wavetable(wavetable) => Some(wavetable),
                _ => None,
            })
            .expect("wavetable primitive");
        std::fs::remove_file(path).expect("remove wavetable fixture");

        assert_eq!(primitive.set_base, 2);
        assert_eq!(primitive.waves_in_set, 2);

        assert_eq!(
            waves_per_set(
                &HashMap::from([("waves-per-set".to_string(), Value::Number(4.0))]),
                &bank,
            ),
            4,
            "an explicit prop must override file metadata"
        );
    }

    #[test]
    fn published_banks_replace_data_with_a_new_revision() {
        let key = "wavetable-viewer-published-bank-test";
        let first = Arc::new(vec![0.0, 1.0, 0.0, -1.0]);
        assert!(publish_bank(key, 4, first.clone()));
        let initial = published_bank(key).expect("published bank");
        assert!(Arc::ptr_eq(&initial.data, &first));
        assert!(publish_bank(key, 4, first));
        assert_eq!(published_bank(key).unwrap().revision, initial.revision);
        assert!(publish_bank(key, 4, Arc::new(vec![1.0, 0.0, -1.0, 0.0])));
        assert!(published_bank(key).unwrap().revision > initial.revision);
        remove_published_bank(key);
        assert!(published_bank(key).is_none());
    }

    #[test]
    fn unpublishing_a_bank_retires_its_key_for_the_renderer() {
        // Per-node keys never repeat, so a renderer that is never told a key
        // is gone keeps its GPU buffer alive for the life of the process.
        let key = "wavetable-viewer-retired-key-test";
        let data = Arc::new(vec![0.0, 1.0, 0.0, -1.0]);
        assert!(publish_bank(key, 4, data.clone()));
        remove_published_bank(key);
        assert!(take_retired_bank_keys().contains(&key.to_string()));
        assert!(
            !take_retired_bank_keys().contains(&key.to_string()),
            "draining hands each retirement out once"
        );
        // Republishing before the drain keeps the live buffer.
        assert!(publish_bank(key, 4, data.clone()));
        remove_published_bank(key);
        assert!(publish_bank(key, 4, data));
        assert!(!take_retired_bank_keys().contains(&key.to_string()));
        remove_published_bank(key);
        take_retired_bank_keys();
    }

    fn color_test_node(extra: &[(&str, Value)]) -> LayoutNode {
        let key = "wavetable-viewer-color-prop-test";
        // Two waves of two samples: enough for `waves_in_set >= 1` so
        // `build_primitives` reaches the Wavetable primitive.
        assert!(publish_bank(key, 2, Arc::new(vec![0.0, 1.0, 1.0, 0.0])));
        let mut props = HashMap::from([
            ("data-key".to_string(), Value::String(key.to_string())),
            ("waves-per-set".to_string(), Value::Number(2.0)),
        ]);
        for (name, value) in extra {
            props.insert((*name).to_string(), value.clone());
        }
        LayoutNode {
            widget_id: 7,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "wavetable-viewer".to_string(),
            rect: crate::layout::Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: 4.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        }
    }

    fn color_test_primitives(extra: &[(&str, Value)]) -> (Color, GpuWavetablePrimitive) {
        let primitives = WAVETABLE_VIEWER_WIDGET.build_primitives(
            "wavetable-viewer",
            &color_test_node(extra),
            test_viewport(),
        );
        let background = primitives
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Rect(rect) => Some(rect.color),
                _ => None,
            })
            .expect("background rect primitive");
        let wavetable = primitives
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Wavetable(wavetable) => Some(wavetable.clone()),
                _ => None,
            })
            .expect("wavetable primitive");
        (background, wavetable)
    }

    /// The color props' defaults are the values the viewer had hardcoded
    /// before they were props, so the Wavetable instrument panel and the
    /// Filter Table panel — neither of which passes every color — keep
    /// rendering exactly what they rendered before.
    #[test]
    fn omitted_color_props_fall_back_to_the_previously_hardcoded_values() {
        let (background, wavetable) = color_test_primitives(&[]);
        assert_eq!(background, Color::rgba(0.035, 0.038, 0.042, 1.0));
        assert_eq!(wavetable.bg_color, Color::rgba(0.035, 0.038, 0.042, 1.0));
        assert_eq!(wavetable.selected_color, Color::rgba(1.0, 0.64, 0.22, 1.0));
        assert_eq!(
            wavetable.inactive_color,
            Color::rgba(0.46, 0.46, 0.48, 0.55)
        );
        // The exported defaults are the same values, so callers and the
        // shader-facing primitive can never drift apart.
        assert_eq!(background, DEFAULT_BACKGROUND_COLOR);
        assert_eq!(wavetable.selected_color, DEFAULT_WAVE_COLOR);
        assert_eq!(wavetable.inactive_color, DEFAULT_INACTIVE_COLOR);
    }

    /// Every color the GPU path draws with is overridable, and each prop takes
    /// the standard color vocabulary (`(rgba …)` lists and `#rrggbbaa`
    /// strings) through `resolve_named_color`.
    #[test]
    fn color_props_override_every_drawn_color() {
        let rgba = |r: f64, g: f64, b: f64, a: f64| {
            Value::List(
                [r, g, b, a]
                    .into_iter()
                    .map(|n| std::rc::Rc::new(std::cell::RefCell::new(Value::Number(n))))
                    .collect(),
            )
        };
        let (background, wavetable) = color_test_primitives(&[
            ("background-color", rgba(0.1, 0.2, 0.3, 1.0)),
            ("wave-color", rgba(0.4, 0.5, 0.6, 1.0)),
            ("inactive-color", Value::String("#20304080".to_string())),
        ]);
        assert_eq!(background, Color::rgba(0.1, 0.2, 0.3, 1.0));
        assert_eq!(wavetable.bg_color, Color::rgba(0.1, 0.2, 0.3, 1.0));
        assert_eq!(wavetable.selected_color, Color::rgba(0.4, 0.5, 0.6, 1.0));
        // The halo behind the highlighted wave is derived from the background
        // in the shader, so overriding the background moves it too — there is
        // no separate halo color to set.
        let inactive = wavetable.inactive_color;
        assert!(
            (inactive.r - 32.0 / 255.0).abs() < 1e-6
                && (inactive.b - 64.0 / 255.0).abs() < 1e-6
                && (inactive.a - 128.0 / 255.0).abs() < 1e-6,
            "hex inactive-color should round-trip, got {inactive:?}"
        );
    }

    /// The terminal fallback's label was the one color in the render path
    /// that no prop could reach. It still defaults to the theme's muted
    /// foreground, so an existing caller sees no change.
    #[test]
    fn label_color_prop_overrides_the_terminal_label_default() {
        let render = |props: HashMap<String, Value>| {
            let mut buf = CellBuffer::new(40, 4);
            WAVETABLE_VIEWER_WIDGET.tui_render(
                &props,
                crate::layout::Rect {
                    row: 0.0,
                    col: 0.0,
                    width: 40.0,
                    height: 1.0,
                },
                &mut buf,
            );
            buf.get(0, 0).expect("label cell").style.fg
        };
        assert_eq!(render(HashMap::new()), theme::FG_MUTED());
        assert_eq!(
            render(HashMap::from([(
                "label-color".to_string(),
                Value::String("#ff0000".to_string()),
            )])),
            Color::rgb(1.0, 0.0, 0.0)
        );
    }

    /// The Wavetable instrument's UI names its bank by the content-relative
    /// path `instruments/core/wavetable/waves/bank.json`. Since the factory
    /// `content/` split that no longer resolves against the process cwd
    /// (`crates/sequencer`), so the viewer must fall back to the installed
    /// content roots or `build_primitives` bails before drawing anything.
    /// Filter Table instances are unaffected — they pass `data-key`.
    #[test]
    fn relative_file_banks_resolve_against_the_content_roots() {
        let bank = load_bank("instruments/core/wavetable/waves/bank.json")
            .expect("wavetable instrument bank should resolve through the content roots");
        assert!(bank.frame_len >= 2, "frame_len={}", bank.frame_len);
        assert!(
            bank.wave_count >= 16,
            "the viewer reads 16 waves per set, got {}",
            bank.wave_count
        );
    }

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
