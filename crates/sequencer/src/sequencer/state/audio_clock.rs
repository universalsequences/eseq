//! The audio clock as a continuous function of wall time, for display.
//!
//! `audio_rendered_sample` advances one whole block per callback (512 frames
//! is ~11ms at 48kHz), so a playhead read straight from it moves by zero,
//! one or two blocks between 60Hz frames: a scroll that judders by up to two
//! thirds of a frame. Each callback also stamps WHEN it started; a reader
//! extends the block it is playing by the wall time since, which lands
//! exactly on the next block boundary when the next callback arrives on time.

use std::sync::atomic::{fence, AtomicU64, Ordering};
use std::time::Instant;

use super::core::record_clock_nanos as nanos_since_origin;

/// One writer (the audio thread), any number of readers: a seqlock like
/// `RecordClockAnchor`'s (which carries transport BEATS for note stamping;
/// the song position is a sample-domain function), so a reader never pairs
/// one block's sample with another block's start time.
#[derive(Default)]
pub(crate) struct AudioClockStamp {
    sequence: AtomicU64,
    block_start_sample: AtomicU64,
    frames: AtomicU64,
    started_nanos: AtomicU64,
    sample_rate_bits: AtomicU64,
}

impl AudioClockStamp {
    /// Audio thread: the callback that started at `started` renders
    /// `[block_start_sample, block_start_sample + frames)`.
    pub(crate) fn stamp(&self, block_start_sample: u64, frames: u64, sample_rate: f64, started: Instant) {
        let started_nanos = nanos_since_origin(started);
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.block_start_sample.store(block_start_sample, Ordering::Relaxed);
        self.frames.store(frames, Ordering::Relaxed);
        self.started_nanos.store(started_nanos, Ordering::Relaxed);
        self.sample_rate_bits.store(sample_rate.to_bits(), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// The sample playing at `now`: the stamped block's start advanced by the
    /// wall time since its callback, held at the block's end if the next
    /// callback is late (never running ahead of what has been rendered).
    /// `None` before the first callback.
    pub(crate) fn sample_at(&self, now: Instant) -> Option<u64> {
        for _ in 0..4 {
            let before = self.sequence.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let start = self.block_start_sample.load(Ordering::Relaxed);
            let frames = self.frames.load(Ordering::Relaxed);
            let started_nanos = self.started_nanos.load(Ordering::Relaxed);
            let sample_rate = f64::from_bits(self.sample_rate_bits.load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) != before {
                continue;
            }
            let elapsed_secs =
                nanos_since_origin(now).saturating_sub(started_nanos) as f64 / 1e9;
            let advanced = (elapsed_secs * sample_rate).clamp(0.0, frames as f64) as u64;
            return Some(start + advanced);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_clock_advances_with_wall_time_within_the_block_and_holds_at_its_end() {
        let clock = AudioClockStamp::default();
        // Pin the shared origin first: an origin set after `started` would
        // saturate the stamp to zero.
        nanos_since_origin(Instant::now());
        let started = Instant::now();
        assert_eq!(clock.sample_at(started), None, "no callback yet");

        clock.stamp(48_000, 480, 48_000.0, started);
        assert_eq!(clock.sample_at(started), Some(48_000));
        assert_eq!(clock.sample_at(started + Duration::from_millis(5)), Some(48_240));
        assert_eq!(
            clock.sample_at(started + Duration::from_millis(50)),
            Some(48_480),
            "a late callback holds the clock at the rendered end"
        );

        // The next callback, on time, continues from exactly where the
        // interpolation reached.
        let next = started + Duration::from_millis(10);
        clock.stamp(48_480, 480, 48_000.0, next);
        assert_eq!(clock.sample_at(next), Some(48_480));
    }
}
