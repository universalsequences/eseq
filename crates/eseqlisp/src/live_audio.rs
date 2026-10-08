use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use crate::widget_render::paint_resources::PaintResourceStore;

#[derive(Clone, Debug)]
pub struct SpectrogramFrame {
    pub revision: u64,
    pub bins: u32,
    pub time_slices: u32,
    pub write_head: u32,
    pub sample_rate: f32,
    pub waterfall: Arc<Vec<f32>>,
    pub smoothed: Arc<Vec<f32>>,
}

impl SpectrogramFrame {
    pub fn is_well_formed(&self) -> bool {
        self.bins > 0
            && self.time_slices > 0
            && self.write_head < self.time_slices
            && self.waterfall.len() == self.bins as usize * self.time_slices as usize
            && self.smoothed.len() == self.bins as usize
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
    }
}

static SPECTROGRAM_FRAMES: OnceLock<PaintResourceStore<Arc<SpectrogramFrame>>> = OnceLock::new();

fn spectrogram_frames() -> &'static PaintResourceStore<Arc<SpectrogramFrame>> {
    SPECTROGRAM_FRAMES.get_or_init(PaintResourceStore::default)
}

pub fn publish_spectrogram_frame(key: impl Into<String>, frame: SpectrogramFrame) {
    if !frame.is_well_formed() {
        return;
    }
    spectrogram_frames().publish(key.into(), Arc::new(frame));
}

pub fn spectrogram_frame(key: &str) -> Option<Arc<SpectrogramFrame>> {
    spectrogram_frames().get(key)
}

pub fn retain_spectrogram_frames(active_keys: &HashSet<String>) {
    spectrogram_frames().retain(|key| active_keys.contains(key));
}

pub fn clear_spectrogram_frames() {
    spectrogram_frames().clear();
}

#[derive(Clone, Debug)]
pub struct ScopeFrame {
    pub revision: u64,
    pub sample_rate: f32,
    pub samples: Arc<Vec<f32>>,
}

impl ScopeFrame {
    pub fn is_well_formed(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && !self.samples.is_empty()
            && self.samples.iter().all(|sample| sample.is_finite())
    }
}

static SCOPE_FRAMES: OnceLock<PaintResourceStore<Arc<ScopeFrame>>> = OnceLock::new();

fn scope_frames() -> &'static PaintResourceStore<Arc<ScopeFrame>> {
    SCOPE_FRAMES.get_or_init(PaintResourceStore::default)
}

pub fn publish_scope_frame(key: impl Into<String>, frame: ScopeFrame) {
    if !frame.is_well_formed() {
        return;
    }
    scope_frames().publish(key.into(), Arc::new(frame));
}

pub fn scope_frame(key: &str) -> Option<Arc<ScopeFrame>> {
    scope_frames().get(key)
}

pub fn retain_scope_frames(active_keys: &HashSet<String>) {
    scope_frames().retain(|key| active_keys.contains(key));
}

pub fn clear_scope_frames() {
    scope_frames().clear();
}

/// Live meter snapshot for one multiband dynamics effect instance: per-band
/// (low, mid, high) L/R detector levels and the applied dynamics gain, in dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandMeterFrame {
    pub revision: u64,
    pub level_db: [[f32; 2]; 3],
    pub gain_db: [f32; 3],
}

static BAND_METER_FRAMES: OnceLock<PaintResourceStore<BandMeterFrame>> = OnceLock::new();

fn band_meter_frames() -> &'static PaintResourceStore<BandMeterFrame> {
    BAND_METER_FRAMES.get_or_init(PaintResourceStore::default)
}

pub fn publish_band_meter_frame(key: impl Into<String>, frame: BandMeterFrame) {
    band_meter_frames().publish(key.into(), frame);
}

pub fn band_meter_frame(key: &str) -> Option<BandMeterFrame> {
    band_meter_frames().get(key)
}

pub fn retain_band_meter_frames(active_keys: &HashSet<String>) {
    band_meter_frames().retain(|key| active_keys.contains(key));
}

pub fn clear_band_meter_frames() {
    band_meter_frames().clear();
}

/// Live meter history for one compressor effect instance: fine-grained
/// (output dB, gain-reduction dB) entries recorded by the DSP every `stride`
/// samples, oldest first, plus the latest block meters.
#[derive(Clone, Debug)]
pub struct CompressorMeterFrame {
    pub revision: u64,
    pub gr_db: f32,
    pub out_db: f32,
    pub sample_rate: f32,
    /// Samples per history entry.
    pub stride: usize,
    /// (output dB, gain-reduction dB) pairs, oldest..newest.
    pub history: Arc<Vec<[f32; 2]>>,
}

impl CompressorMeterFrame {
    pub fn is_well_formed(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.stride > 0
            && !self.history.is_empty()
    }
}

static COMPRESSOR_METER_FRAMES: OnceLock<PaintResourceStore<Arc<CompressorMeterFrame>>> = OnceLock::new();

fn compressor_meter_frames() -> &'static PaintResourceStore<Arc<CompressorMeterFrame>> {
    COMPRESSOR_METER_FRAMES.get_or_init(PaintResourceStore::default)
}

pub fn publish_compressor_meter_frame(key: impl Into<String>, frame: CompressorMeterFrame) {
    if !frame.is_well_formed() {
        return;
    }
    compressor_meter_frames().publish(key.into(), Arc::new(frame));
}

pub fn compressor_meter_frame(key: &str) -> Option<Arc<CompressorMeterFrame>> {
    compressor_meter_frames().get(key)
}

pub fn retain_compressor_meter_frames(active_keys: &HashSet<String>) {
    compressor_meter_frames().retain(|key| active_keys.contains(key));
}

pub fn clear_compressor_meter_frames() {
    compressor_meter_frames().clear();
}

/// Output level of one device (instrument or effect) for the FX panel's
/// `device-meter` widgets: L/R in meter display units (0 = floor, 1 = 0 dBFS),
/// keyed by the device's source selector (`LiveAudioSourceSelector`
/// `key_fragment`) and published by the host's device-meter poller.
static DEVICE_METER_LEVELS: OnceLock<PaintResourceStore<[f32; 2]>> = OnceLock::new();

fn device_meter_levels() -> &'static PaintResourceStore<[f32; 2]> {
    DEVICE_METER_LEVELS.get_or_init(PaintResourceStore::default)
}

pub fn publish_device_meter_level(key: &str, levels: [f32; 2]) {
    device_meter_levels().publish(format!("device-meter:{key}"), levels);
}

pub fn device_meter_level(key: &str) -> Option<[f32; 2]> {
    device_meter_levels().get(&format!("device-meter:{key}"))
}

pub fn retain_device_meter_levels(active_keys: &HashSet<String>) {
    device_meter_levels().retain(|key| {
        key.strip_prefix("device-meter:").is_some_and(|key| active_keys.contains(key))
    });
}

/// Live value of one DGen `(probe …)` tap in the instance a visible patcher
/// edits (docs/patcher-probes-spec.md §6.1). Published by the host's live
/// audio analyzer under [`probe_frame_key`], only when a value moves past an
/// epsilon, the capture goes stale or resumes, or the scope trace changes.
///
/// Values may be non-finite: a probe exists to show what flows down a cable,
/// NaN and ±inf included.
/// How a probe's live value is drawn (`@view` in DGenLisp,
/// docs/patcher-probes-spec.md). Parsed once from the compiled manifest.
///
/// An unrecognized `@view` string parses to `None` in [`ProbeView::parse`];
/// manifest readers map that to [`ProbeView::Number`] (the default), so a
/// probe whose view this build does not know still shows its number instead
/// of disappearing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ProbeView {
    #[default]
    Number,
    Scope,
    Meter,
}

impl ProbeView {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeView::Number => "number",
            ProbeView::Scope => "scope",
            ProbeView::Meter => "meter",
        }
    }

    /// `None` for an unknown view; callers treat that as [`ProbeView::Number`].
    pub fn parse(view: &str) -> Option<ProbeView> {
        match view {
            "number" => Some(ProbeView::Number),
            "scope" => Some(ProbeView::Scope),
            "meter" => Some(ProbeView::Meter),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeFrame {
    /// Bumped on every publish of this key.
    pub revision: u64,
    /// Last sample of the latest captured block.
    pub last: f32,
    /// Minimum over the latest captured block.
    pub min: f32,
    /// Maximum over the latest captured block.
    pub max: f32,
    /// Blocks captured so far (the capture's own counter).
    pub seq: u64,
    /// The capture has not advanced recently: the display voice went idle,
    /// the effect is bypassed, or the instance stopped rendering. `last`/`min`
    /// /`max` are the held values.
    pub stale: bool,
    /// For `@view scope` probes: decimated (min, max) pairs, oldest first.
    pub scope: Option<Arc<Vec<(f32, f32)>>>,
    /// For `@view scope` probes: the eased y-range `(lo, hi)` to draw
    /// `scope` in, computed by the publisher so every painter of this frame
    /// agrees on it (widens at once, narrows gradually across revisions).
    /// `None` for other views or when the trace has no finite value.
    pub display_range: Option<(f32, f32)>,
}

/// Instance-key fragment for a custom instrument engine's probes.
pub fn probe_instance_key_for_instrument(engine_id: usize) -> String {
    format!("eng{engine_id}")
}

/// Instance-key fragment for a DGen effect node's probes. The node id changes
/// whenever the effect is recompiled, so this key does too.
pub fn probe_instance_key_for_effect(node_id: i32) -> String {
    format!("fx{node_id}")
}

/// `probe:<instance>:<id>#<occurrence>`: the store key of one probe's frame.
pub fn probe_frame_key(instance_key: &str, id: &str, occurrence: u32) -> String {
    format!("probe:{instance_key}:{id}#{occurrence}")
}

static PROBE_FRAMES: OnceLock<PaintResourceStore<Arc<ProbeFrame>>> = OnceLock::new();

fn probe_frames() -> &'static PaintResourceStore<Arc<ProbeFrame>> {
    PROBE_FRAMES.get_or_init(PaintResourceStore::default)
}

pub fn publish_probe_frame(key: impl Into<String>, frame: ProbeFrame) {
    probe_frames().publish(key.into(), Arc::new(frame));
}

pub fn probe_frame(key: &str) -> Option<Arc<ProbeFrame>> {
    probe_frames().get(key)
}

pub fn retain_probe_frames(active_keys: &HashSet<String>) {
    probe_frames().retain(|key| active_keys.contains(key));
}

pub fn clear_probe_frames() {
    probe_frames().clear();
}

/// Patch source path (a `patcher` widget's `:path`) → instance key of the
/// live DGen instance that path is being edited on. Absent when no patcher
/// for the path is visible or its edit target has no live instance; probes
/// then show `—`.
static PATCH_PROBE_INSTANCES: OnceLock<PaintResourceStore<Arc<str>>> = OnceLock::new();

fn patch_probe_instances() -> &'static PaintResourceStore<Arc<str>> {
    PATCH_PROBE_INSTANCES.get_or_init(PaintResourceStore::default)
}

/// Bind (`Some`) or unbind (`None`) the instance a patch path's probes read
/// from. Republishes only on change, so a steady binding never invalidates
/// the patcher's paint. Returns whether the binding changed.
pub fn publish_patch_probe_instance(path: &str, instance_key: Option<&str>) -> bool {
    let store = patch_probe_instances();
    let current = store.get(path);
    match instance_key {
        Some(key) if current.as_deref() != Some(key) => {
            store.publish(path.to_string(), Arc::from(key));
            true
        }
        None if current.is_some() => {
            store.retain(|existing| existing != path);
            true
        }
        _ => false,
    }
}

/// The instance key a patcher for `path` reads probe frames from. Reading it
/// while painting registers a paint dependency, so a rebinding (the effect
/// was recompiled onto a new node) repaints the patcher. Cheap (an `Arc`
/// clone): resolve it once per patcher paint, then look each probe node up
/// with [`probe_frame_for`].
pub fn patch_probe_instance(path: &str) -> Option<Arc<str>> {
    patch_probe_instances().get(path)
}

/// Drop every patch binding whose path is not in `active_paths`.
pub fn retain_patch_probe_instances(active_paths: &HashSet<String>) {
    patch_probe_instances().retain(|path| active_paths.contains(path));
}

/// The live frame of probe `id`#`occurrence` in the patch at `path`: the
/// one-call read for the patcher widget. `None` = show `—`.
pub fn patch_probe_frame(path: &str, id: &str, occurrence: u32) -> Option<Arc<ProbeFrame>> {
    let instance = patch_probe_instance(path)?;
    probe_frame_for(&instance, id, occurrence)
}

/// The live frame of probe `id`#`occurrence` in `instance` (from
/// [`patch_probe_instance`]). Registers a paint dependency on that one key,
/// published or not, so a later first publish repaints. The key is built in
/// a reused per-thread buffer: no allocation per lookup.
pub fn probe_frame_for(instance: &str, id: &str, occurrence: u32) -> Option<Arc<ProbeFrame>> {
    use std::fmt::Write as _;
    thread_local! {
        static KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    KEY.with(|key| {
        let mut key = key.borrow_mut();
        key.clear();
        let _ = write!(key, "probe:{instance}:{id}#{occurrence}");
        probe_frame(&key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_meter_frames_round_trip_and_retain() {
        clear_band_meter_frames();
        let frame = BandMeterFrame {
            revision: 3,
            level_db: [[-12.0, -13.0], [-24.0, -25.0], [-36.0, -37.0]],
            gain_db: [-6.0, 0.0, 3.0],
        };
        publish_band_meter_frame("meter", frame);
        assert_eq!(band_meter_frame("meter"), Some(frame));
        retain_band_meter_frames(&HashSet::new());
        assert!(band_meter_frame("meter").is_none());
    }

    #[test]
    fn rejects_malformed_spectrogram_frame() {
        clear_spectrogram_frames();
        publish_spectrogram_frame(
            "bad",
            SpectrogramFrame {
                revision: 1,
                bins: 4,
                time_slices: 4,
                write_head: 0,
                sample_rate: 48_000.0,
                waterfall: Arc::new(vec![0.0; 8]),
                smoothed: Arc::new(vec![0.0; 4]),
            },
        );
        assert!(spectrogram_frame("bad").is_none());
    }

    #[test]
    fn stores_and_retrieves_well_formed_spectrogram_frame() {
        clear_spectrogram_frames();
        publish_spectrogram_frame(
            "ok",
            SpectrogramFrame {
                revision: 7,
                bins: 4,
                time_slices: 3,
                write_head: 2,
                sample_rate: 48_000.0,
                waterfall: Arc::new(vec![0.0; 12]),
                smoothed: Arc::new(vec![0.0; 4]),
            },
        );
        assert_eq!(spectrogram_frame("ok").unwrap().revision, 7);
    }

    #[test]
    fn scope_frames_round_trip_and_retain() {
        clear_scope_frames();
        publish_scope_frame(
            "scope",
            ScopeFrame {
                revision: 2,
                sample_rate: 48_000.0,
                samples: Arc::new(vec![-0.5, 0.0, 0.5]),
            },
        );
        assert_eq!(scope_frame("scope").unwrap().revision, 2);
        retain_scope_frames(&HashSet::new());
        assert!(scope_frame("scope").is_none());
    }

    #[test]
    fn probe_keys_encode_instance_id_and_occurrence() {
        assert_eq!(probe_instance_key_for_instrument(7), "eng7");
        assert_eq!(probe_instance_key_for_effect(123), "fx123");
        assert_eq!(probe_frame_key("eng7", "cut", 0), "probe:eng7:cut#0");
        assert_eq!(probe_frame_key("fx123", "env", 2), "probe:fx123:env#2");
    }

    #[test]
    fn patch_probe_frame_follows_the_bound_instance() {
        clear_probe_frames();
        let frame = ProbeFrame {
            revision: 1,
            last: 0.5,
            min: 0.25,
            max: 0.75,
            seq: 9,
            stale: false,
            scope: None,
            display_range: None,
        };
        publish_probe_frame(probe_frame_key("fx5", "cut", 0), frame.clone());
        publish_probe_frame(
            probe_frame_key("fx6", "cut", 0),
            ProbeFrame { last: 1.5, ..frame.clone() },
        );
        assert!(patch_probe_frame("/p/dsp.lisp", "cut", 0).is_none(), "unbound path");

        assert!(publish_patch_probe_instance("/p/dsp.lisp", Some("fx5")));
        assert!(!publish_patch_probe_instance("/p/dsp.lisp", Some("fx5")), "steady binding");
        assert_eq!(patch_probe_frame("/p/dsp.lisp", "cut", 0).as_deref(), Some(&frame));
        assert!(patch_probe_frame("/p/dsp.lisp", "cut", 1).is_none());

        // A recompile rebinds the path to the new node.
        publish_patch_probe_instance("/p/dsp.lisp", Some("fx6"));
        assert_eq!(patch_probe_frame("/p/dsp.lisp", "cut", 0).unwrap().last, 1.5);

        assert!(publish_patch_probe_instance("/p/dsp.lisp", None));
        assert!(!publish_patch_probe_instance("/p/dsp.lisp", None));
        assert!(patch_probe_instance("/p/dsp.lisp").is_none());

        publish_patch_probe_instance("/p/dsp.lisp", Some("fx6"));
        retain_patch_probe_instances(&HashSet::new());
        assert!(patch_probe_instance("/p/dsp.lisp").is_none());

        retain_probe_frames(&HashSet::from([probe_frame_key("fx6", "cut", 0)]));
        assert!(probe_frame(&probe_frame_key("fx5", "cut", 0)).is_none());
        assert!(probe_frame(&probe_frame_key("fx6", "cut", 0)).is_some());
        clear_probe_frames();
    }

    #[test]
    fn probe_frame_for_matches_the_formatted_key() {
        let frame = ProbeFrame { revision: 4, last: -2.0, ..ProbeFrame::default() };
        publish_probe_frame(probe_frame_key("fx801", "env", 3), frame.clone());
        let instance = Arc::<str>::from("fx801");
        assert_eq!(probe_frame_for(&instance, "env", 3).as_deref(), Some(&frame));
        assert!(probe_frame_for(&instance, "env", 2).is_none());
        assert!(probe_frame_for("fx80", "1:env", 3).is_none());
        retain_probe_frames(&HashSet::new());
    }

    #[test]
    fn probe_view_round_trips_and_rejects_unknown_views() {
        for view in [ProbeView::Number, ProbeView::Scope, ProbeView::Meter] {
            assert_eq!(ProbeView::parse(view.as_str()), Some(view));
        }
        assert_eq!(ProbeView::parse("spectrum"), None);
        assert_eq!(ProbeView::parse(""), None);
        assert_eq!(ProbeView::default(), ProbeView::Number);
    }
}
