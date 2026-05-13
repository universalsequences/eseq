use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

pub struct RecordingTake {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub dropped_blocks: usize,
}

struct RecorderState {
    samples: Vec<f32>,
}

pub struct MasterRecorder {
    sample_rate: u32,
    channels: u16,
    active: AtomicBool,
    dropped_blocks: AtomicUsize,
    state: Mutex<RecorderState>,
}

impl MasterRecorder {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            active: AtomicBool::new(false),
            dropped_blocks: AtomicUsize::new(0),
            state: Mutex::new(RecorderState {
                samples: Vec::new(),
            }),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn start(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Recorder state lock poisoned".to_string())?;
        state.samples.clear();
        self.dropped_blocks.store(0, Ordering::Release);
        self.active.store(true, Ordering::Release);
        Ok(())
    }

    pub fn capture(&self, output: &[f32]) {
        if !self.is_active() {
            return;
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
            return;
        };
        state.samples.extend_from_slice(output);
    }

    pub fn stop(&self) -> Result<RecordingTake, String> {
        self.active.store(false, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Recorder state lock poisoned".to_string())?;
        Ok(RecordingTake {
            samples: std::mem::take(&mut state.samples),
            sample_rate: self.sample_rate,
            channels: self.channels,
            dropped_blocks: self.dropped_blocks.swap(0, Ordering::AcqRel),
        })
    }
}

pub fn save_recording_wav(path: &Path, take: &RecordingTake) -> Result<(), String> {
    if take.samples.is_empty() {
        return Err("Recording is empty".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create folder: {e}"))?;
    }
    let spec = hound::WavSpec {
        channels: take.channels,
        sample_rate: take.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| format!("Failed to create WAV: {e}"))?;
    for sample in &take.samples {
        writer
            .write_sample((*sample).clamp(-1.0, 1.0))
            .map_err(|e| format!("Failed to write WAV data: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("Failed to finalize WAV: {e}"))?;
    Ok(())
}

pub fn resolve_recording_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    let mut path = PathBuf::from(trimmed);
    if path.as_os_str().is_empty() {
        path = PathBuf::from(default_recording_name());
    }
    if path.extension().is_none() {
        path.set_extension("wav");
    }
    if path.components().count() == 1 {
        PathBuf::from("recordings").join(path)
    } else {
        path
    }
}

pub fn default_recording_name() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_secs())
        .unwrap_or(0);
    format!("recording-{secs}.wav")
}
