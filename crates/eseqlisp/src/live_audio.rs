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

#[cfg(test)]
mod tests {
    use super::*;

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
}
