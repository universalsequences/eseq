use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

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

static SPECTROGRAM_FRAMES: OnceLock<Mutex<HashMap<String, Arc<SpectrogramFrame>>>> =
    OnceLock::new();

fn spectrogram_frames() -> &'static Mutex<HashMap<String, Arc<SpectrogramFrame>>> {
    SPECTROGRAM_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn publish_spectrogram_frame(key: impl Into<String>, frame: SpectrogramFrame) {
    if !frame.is_well_formed() {
        return;
    }
    let mut frames = spectrogram_frames().lock().unwrap();
    frames.insert(key.into(), Arc::new(frame));
}

pub fn spectrogram_frame(key: &str) -> Option<Arc<SpectrogramFrame>> {
    let frames = spectrogram_frames().lock().unwrap();
    frames.get(key).cloned()
}

pub fn retain_spectrogram_frames(active_keys: &HashSet<String>) {
    let mut frames = spectrogram_frames().lock().unwrap();
    frames.retain(|key, _| active_keys.contains(key));
}

pub fn clear_spectrogram_frames() {
    spectrogram_frames().lock().unwrap().clear();
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

static SCOPE_FRAMES: OnceLock<Mutex<HashMap<String, Arc<ScopeFrame>>>> = OnceLock::new();

fn scope_frames() -> &'static Mutex<HashMap<String, Arc<ScopeFrame>>> {
    SCOPE_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn publish_scope_frame(key: impl Into<String>, frame: ScopeFrame) {
    if !frame.is_well_formed() {
        return;
    }
    scope_frames()
        .lock()
        .unwrap()
        .insert(key.into(), Arc::new(frame));
    crate::widget_render::bump_widget_state_generation();
}

pub fn scope_frame(key: &str) -> Option<Arc<ScopeFrame>> {
    scope_frames().lock().unwrap().get(key).cloned()
}

pub fn retain_scope_frames(active_keys: &HashSet<String>) {
    scope_frames()
        .lock()
        .unwrap()
        .retain(|key, _| active_keys.contains(key));
}

pub fn clear_scope_frames() {
    scope_frames().lock().unwrap().clear();
}

/// Live meter snapshot for one multiband dynamics effect instance: per-band
/// (low, mid, high) L/R detector levels and the applied dynamics gain, in dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandMeterFrame {
    pub revision: u64,
    pub level_db: [[f32; 2]; 3],
    pub gain_db: [f32; 3],
}

static BAND_METER_FRAMES: OnceLock<Mutex<HashMap<String, BandMeterFrame>>> = OnceLock::new();

fn band_meter_frames() -> &'static Mutex<HashMap<String, BandMeterFrame>> {
    BAND_METER_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn publish_band_meter_frame(key: impl Into<String>, frame: BandMeterFrame) {
    {
        let mut frames = band_meter_frames().lock().unwrap();
        frames.insert(key.into(), frame);
    }
    // Meter widgets fold the frame into their primitives at build time, so a
    // new frame must invalidate the compiled primitive cache to repaint.
    crate::widget_render::bump_widget_state_generation();
}

pub fn band_meter_frame(key: &str) -> Option<BandMeterFrame> {
    let frames = band_meter_frames().lock().unwrap();
    frames.get(key).copied()
}

pub fn retain_band_meter_frames(active_keys: &HashSet<String>) {
    let mut frames = band_meter_frames().lock().unwrap();
    frames.retain(|key, _| active_keys.contains(key));
}

pub fn clear_band_meter_frames() {
    band_meter_frames().lock().unwrap().clear();
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
}
