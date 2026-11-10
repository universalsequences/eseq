//! The per-device input ring and the stream clocks that align it with the
//! output (docs/audio-tracks-spec.md §12.3, §13).

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

/// How much input a ring keeps: what a take writer may fall behind by, and
/// how far back a take can reach for audio played before it was noticed.
pub const RING_SECONDS: usize = 4;

/// Frames a writer may be filling but has not published yet; readers stay
/// this far clear of the oldest frame.
const RING_SLACK_FRAMES: usize = 1 << 13;

/// Where a stream's frame 0 sits on the shared stream clock, in
/// nanoseconds: frame `n` is at `nanos + n / rate`. One stream callback
/// writes it; anyone reads it.
///
/// Input and output callbacks each report when their buffer's first frame
/// crosses the converter, on the same clock, so two epochs give the
/// relation between an input frame index and the output sample that was
/// playing when it was captured, without knowing when either callback ran.
pub struct StreamEpoch {
    nanos: AtomicI64,
}

const EPOCH_UNKNOWN: i64 = i64::MIN;
/// A jump larger than this is a discontinuity (dropped buffers, a device
/// restart), not jitter: follow it at once.
const EPOCH_SNAP_NANOS: i64 = 20_000_000;

impl Default for StreamEpoch {
    fn default() -> Self {
        Self { nanos: AtomicI64::new(EPOCH_UNKNOWN) }
    }
}

impl StreamEpoch {
    /// The stream callback: the buffer whose first frame is frame `frame`
    /// crosses the converter at `nanos`. Smoothed, since each callback's
    /// timestamp jitters around a constant.
    pub fn observe(&self, nanos: i64, frame: u64, sample_rate: u32) {
        let raw = nanos - (frame as f64 * 1e9 / f64::from(sample_rate.max(1))) as i64;
        let held = self.nanos.load(Ordering::Relaxed);
        let next = if held == EPOCH_UNKNOWN || (raw - held).abs() > EPOCH_SNAP_NANOS {
            raw
        } else {
            held + (raw - held) / 8
        };
        self.nanos.store(next, Ordering::Release);
    }

    pub fn get(&self) -> Option<i64> {
        let nanos = self.nanos.load(Ordering::Acquire);
        (nanos != EPOCH_UNKNOWN).then_some(nanos)
    }

    pub fn clear(&self) {
        self.nanos.store(EPOCH_UNKNOWN, Ordering::Release);
    }

    /// Pin the epoch (tests, and streams with exact timing).
    pub fn set(&self, nanos: i64) {
        self.nanos.store(nanos, Ordering::Release);
    }
}

/// The output stream's epoch: rendered sample 0 on the stream clock.
pub fn output_epoch() -> &'static StreamEpoch {
    static EPOCH: OnceLock<StreamEpoch> = OnceLock::new();
    EPOCH.get_or_init(StreamEpoch::default)
}

/// A cpal stream instant as nanoseconds on one process-wide scale (the
/// first instant seen is 0), so input and output instants compare.
pub fn stream_nanos(instant: cpal::StreamInstant) -> i64 {
    static BASE: OnceLock<cpal::StreamInstant> = OnceLock::new();
    let base = BASE.get_or_init(|| instant);
    match instant.duration_since(base) {
        Some(after) => after.as_nanos() as i64,
        None => -(base.duration_since(&instant).map_or(0, |before| before.as_nanos() as i64)),
    }
}

/// Everything an input device has delivered lately: interleaved frames in a
/// ring one writer (the device's input callback) fills and any number of
/// readers follow, each with its own cursor — a track's monitor in the
/// graph, a take writer on its thread. Nothing blocks and nothing
/// allocates after construction.
pub struct InputRing {
    channels: usize,
    sample_rate: u32,
    /// Frames.
    capacity: usize,
    /// Interleaved samples as f32 bits.
    data: Box<[AtomicU32]>,
    /// Frames written since the ring was made; published after the samples.
    written: AtomicU64,
    /// Frame 0's capture time.
    epoch: StreamEpoch,
    /// The size of the last buffer the device delivered.
    block_frames: AtomicU32,
    /// Per channel: the loudest sample since the meter last read it.
    peaks: Box<[AtomicU32]>,
    /// Blocks a monitor found the ring short for.
    underruns: AtomicU64,
}

impl InputRing {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let channels = channels.max(1);
        let capacity = RING_SECONDS * sample_rate.max(1) as usize + RING_SLACK_FRAMES;
        Self {
            channels,
            sample_rate,
            capacity,
            data: (0..capacity * channels).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            epoch: StreamEpoch::default(),
            block_frames: AtomicU32::new(0),
            peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
            underruns: AtomicU64::new(0),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    pub fn epoch(&self) -> &StreamEpoch {
        &self.epoch
    }

    pub fn block_frames(&self) -> u32 {
        self.block_frames.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// The device callback: append `interleaved` (whole frames of this
    /// ring's channel count). `capture_nanos` is when its first frame was
    /// captured ([`stream_nanos`]), if the stream says.
    pub fn write(&self, interleaved: &[f32], capture_nanos: Option<i64>) {
        let start = self.written.load(Ordering::Relaxed);
        let frames = interleaved.len() / self.channels;
        if frames == 0 {
            return;
        }
        if let Some(nanos) = capture_nanos {
            self.epoch.observe(nanos, start, self.sample_rate);
        }
        for (offset, frame) in interleaved.chunks_exact(self.channels).enumerate() {
            let slot = ((start + offset as u64) % self.capacity as u64) as usize * self.channels;
            for (channel, sample) in frame.iter().enumerate() {
                self.data[slot + channel].store(sample.to_bits(), Ordering::Relaxed);
            }
        }
        for (channel, peak) in self.peaks.iter().enumerate() {
            let block = interleaved
                .iter()
                .skip(channel)
                .step_by(self.channels)
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            if block > f32::from_bits(peak.load(Ordering::Relaxed)) {
                peak.store(block.to_bits(), Ordering::Relaxed);
            }
        }
        self.block_frames.store(frames as u32, Ordering::Relaxed);
        self.written.store(start + frames as u64, Ordering::Release);
    }

    /// Channel `channel` of frame `frame`. The caller keeps `frame` inside
    /// `[oldest(), written())`.
    #[inline]
    pub fn sample(&self, frame: u64, channel: usize) -> f32 {
        let slot = (frame % self.capacity as u64) as usize * self.channels;
        f32::from_bits(self.data[slot + channel.min(self.channels - 1)].load(Ordering::Relaxed))
    }

    /// The oldest frame a reader can still trust.
    pub fn oldest(&self) -> u64 {
        (self.written() + RING_SLACK_FRAMES as u64).saturating_sub(self.capacity as u64)
    }

    /// Channel `channel`'s loudest sample since the last call (the input
    /// meter; one reader).
    pub fn take_peak(&self, channel: usize) -> f32 {
        self.peaks
            .get(channel)
            .map_or(0.0, |peak| f32::from_bits(peak.swap(0, Ordering::Relaxed)))
    }

    /// How far behind the newest frame a monitor reads, so the jitter
    /// between the input and output callbacks never empties the ring: one
    /// device buffer and a little.
    pub fn monitor_margin(&self) -> u64 {
        u64::from(self.block_frames().max(64)) + 32
    }

    pub(crate) fn count_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// `K` such that input frame `f` was captured while output sample
    /// `f + K` was playing, from the two stream clocks. `rendered_now` (the
    /// output's rendered-sample counter) and `output_block` give the
    /// fallback for a stream that reports no timing: the newest input frame
    /// is taken as heard one output buffer and one input buffer ago.
    pub fn output_offset(&self, output: &StreamEpoch, rendered_now: u64, output_block: u32) -> i64 {
        match (self.epoch.get(), output.get()) {
            (Some(input), Some(output)) => {
                ((input - output) as f64 * f64::from(self.sample_rate) / 1e9).round() as i64
            }
            _ => {
                rendered_now as i64
                    - self.written() as i64
                    - i64::from(output_block)
                    - i64::from(self.block_frames())
            }
        }
    }
}
