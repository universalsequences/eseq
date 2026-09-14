//! Allocation-free timing publication at the device callback boundary.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::sequencer::SequencerState;

pub(super) fn record_output_callback(
    state: &SequencerState,
    elapsed: Duration,
    frames: usize,
    sample_rate: f64,
) {
    let transport = &state.transport;
    if frames == 0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return;
    }
    // Use the actual device request, including fixed-block adaptation and all
    // graph renders needed to satisfy it. A graph block is not a device period.
    let budget_secs = frames as f64 / sample_rate;
    let elapsed_secs = elapsed.as_secs_f64();
    if elapsed_secs > budget_secs {
        transport.audio_deadline_misses.fetch_add(1, Ordering::Relaxed);
    }
    let raw_load_pct = (elapsed_secs / budget_secs * 100.0) as f32;
    let previous = f32::from_bits(transport.cpu_load_pct.load(Ordering::Relaxed));
    let smoothed = if previous <= 0.0 {
        raw_load_pct
    } else {
        previous * 0.97 + raw_load_pct * 0.03
    };
    transport.cpu_load_pct.store(smoothed.to_bits(), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_deadline_uses_actual_device_frames_and_sample_rate() {
        let state = SequencerState::new(0, vec![]);
        let transport = &state.transport;
        // 48 frames at 48kHz gives exactly 1ms. Completing at the budget is
        // allowed, crossing it by one nanosecond is a miss.
        record_output_callback(&state, Duration::from_millis(1), 48, 48_000.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 0);
        record_output_callback(&state, Duration::from_nanos(1_000_001), 48, 48_000.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 1);
        // An odd device size must not use the engine's 512-frame deadline.
        record_output_callback(&state, Duration::from_millis(5), 235, 48_000.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 2);
        record_output_callback(&state, Duration::from_millis(5), 235, 44_100.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn callback_miss_survives_fast_blocks_and_smoothed_cpu() {
        let state = SequencerState::new(0, vec![]);
        let transport = &state.transport;
        record_output_callback(&state, Duration::from_millis(2), 480, 48_000.0);
        record_output_callback(&state, Duration::from_millis(12), 480, 48_000.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 1);
        assert!((f32::from_bits(transport.cpu_load_pct.load(Ordering::Relaxed)) - 23.0).abs() < 0.001);
        for _ in 0..100 {
            record_output_callback(&state, Duration::from_millis(2), 480, 48_000.0);
        }
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 1);
        record_output_callback(&state, Duration::from_secs(1), 0, 48_000.0);
        record_output_callback(&state, Duration::from_secs(1), 480, 0.0);
        assert_eq!(transport.audio_deadline_misses.load(Ordering::Relaxed), 1);
    }
}
