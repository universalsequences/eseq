use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// How much master output `capture-resample` can reach back into.
pub const RESAMPLE_WINDOW: Duration = Duration::from_secs(30);

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
    history: Option<MasterHistory>,
}

/// Always-on stereo ring of the master output (SP-404 style resampling).
/// The audio thread is the only writer and never blocks; a reader copies the
/// ring and discards whatever the writer may have overwritten meanwhile.
struct MasterHistory {
    /// Stereo frames the reader may return.
    window: usize,
    /// Ring capacity in frames: the window plus slack, so frames written
    /// during a copy lap the slack rather than the returned window.
    capacity: usize,
    /// Interleaved L/R as f32 bits.
    data: Box<[AtomicU32]>,
    /// Frames written since start; publishes a block after its samples.
    written: AtomicU64,
}

/// A frozen copy of the ring: interleaved stereo, oldest first.
pub struct HistoryPrint {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Slack for frames the writer may be filling but has not published yet.
const HISTORY_SLACK_FRAMES: usize = 1 << 15;

impl MasterHistory {
    fn new(sample_rate: u32, window: Duration) -> Self {
        let window = (window.as_secs_f64() * f64::from(sample_rate)).ceil() as usize;
        let capacity = window + HISTORY_SLACK_FRAMES;
        Self {
            window,
            capacity,
            data: (0..capacity * 2).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
        }
    }

    fn write(&self, output: &[f32], channels: usize) {
        if channels == 0 { return; }
        let start = self.written.load(Ordering::Relaxed);
        let frames = output.len() / channels;
        for frame in 0..frames {
            let l = output[frame * channels];
            let r = if channels > 1 { output[frame * channels + 1] } else { l };
            let slot = ((start + frame as u64) % self.capacity as u64) as usize * 2;
            self.data[slot].store(l.to_bits(), Ordering::Relaxed);
            self.data[slot + 1].store(r.to_bits(), Ordering::Relaxed);
        }
        self.written.store(start + frames as u64, Ordering::Release);
    }

    fn read(&self) -> Vec<f32> {
        let end = self.written.load(Ordering::Acquire);
        let start = end.saturating_sub(self.window as u64);
        let mut out = Vec::with_capacity((end - start) as usize * 2);
        for frame in start..end {
            let slot = (frame % self.capacity as u64) as usize * 2;
            out.push(f32::from_bits(self.data[slot].load(Ordering::Relaxed)));
            out.push(f32::from_bits(self.data[slot + 1].load(Ordering::Relaxed)));
        }
        // Frames below `after - capacity + slack` may have been overwritten
        // (published or in flight) while copying.
        let after = self.written.load(Ordering::Acquire);
        let safe = (after + HISTORY_SLACK_FRAMES as u64).saturating_sub(self.capacity as u64);
        let skip = safe.saturating_sub(start).min(end - start) as usize;
        out.drain(..skip * 2);
        out
    }
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
            history: None,
        }
    }

    /// A recorder that also keeps the last `window` of master output for
    /// resampling. Allocates the ring up front; the audio thread never does.
    pub fn with_history(sample_rate: u32, channels: u16, window: Duration) -> Self {
        Self { history: Some(MasterHistory::new(sample_rate, window)), ..Self::new(sample_rate, channels) }
    }

    /// Copy the resample ring. `None` when this recorder keeps no history.
    pub fn print_history(&self) -> Option<HistoryPrint> {
        self.history.as_ref().map(|history| HistoryPrint {
            samples: history.read(),
            sample_rate: self.sample_rate,
        })
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
        if let Some(history) = &self.history {
            history.write(output, usize::from(self.channels));
        }
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
        crate::app_paths::app_paths().recordings_dir().join(path)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_ramp(from: usize, frames: usize) -> Vec<f32> {
        (from..from + frames).flat_map(|i| [i as f32, -(i as f32)]).collect()
    }

    #[test]
    fn history_keeps_only_the_most_recent_window_in_order() {
        // 10 Hz, 2 s window: 20 frames returned.
        let recorder = MasterRecorder::with_history(10, 2, Duration::from_secs(2));
        let mut written = 0;
        for block in [7, 13, 40_000, 5] {
            recorder.capture(&stereo_ramp(written, block));
            written += block;
        }
        let print = recorder.print_history().unwrap();
        assert_eq!(print.sample_rate, 10);
        assert_eq!(print.samples, stereo_ramp(written - 20, 20));
    }

    #[test]
    fn history_before_the_window_fills_is_everything_played() {
        let recorder = MasterRecorder::with_history(10, 2, Duration::from_secs(30));
        recorder.capture(&stereo_ramp(0, 3));
        assert_eq!(recorder.print_history().unwrap().samples, stereo_ramp(0, 3));
    }

    #[test]
    fn mono_output_is_duplicated_and_extra_channels_dropped() {
        let mono = MasterRecorder::with_history(10, 1, Duration::from_secs(1));
        mono.capture(&[0.5, 0.25]);
        assert_eq!(mono.print_history().unwrap().samples, vec![0.5, 0.5, 0.25, 0.25]);
        let quad = MasterRecorder::with_history(10, 4, Duration::from_secs(1));
        quad.capture(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(quad.print_history().unwrap().samples, vec![1.0, 2.0]);
    }

    #[test]
    fn history_runs_without_an_active_take_and_plain_recorders_keep_none() {
        let recorder = MasterRecorder::with_history(10, 2, Duration::from_secs(1));
        assert!(!recorder.is_active());
        recorder.capture(&[0.1, 0.2]);
        assert_eq!(recorder.print_history().unwrap().samples.len(), 2);
        assert!(MasterRecorder::new(10, 2).print_history().is_none());
    }
}
