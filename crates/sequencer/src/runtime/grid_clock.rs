//! Grid-boundary timebases for the self-clocked runtime engines.
//!
//! Extracted from `crate::neural` so the lisp-authorable engines
//! ([`super::graph`], [`super::process`]) don't depend on the legacy builtin
//! neural machine for their clock. [`GridBoundaryClock`] shares its stepping
//! logic verbatim with the per-neuron clock in `crate::neural` (blocks cover
//! `(start, end]`). Lisp generators ([`super::generator`]) use
//! [`DownbeatClock`] instead (`[start, end)`), so their tick 0 is the downbeat.

/// Advance `last_index` past any grid boundaries already at or before `start_beats`,
/// then return the next boundary that falls in `(start_beats, end_beats]`, if any.
///
/// This is the single-grid core of the per-neuron boundary detection used by
/// `crate::neural::NeuralRuntime::next_eval_boundary`; it is shared verbatim by
/// [`GridBoundaryClock`] so processes and neurons step their clocks
/// identically. `last_index` is only advanced for boundaries that are *skipped*
/// (already passed); the returned boundary is left for the caller to consume.
pub fn next_grid_boundary(
    last_index: &mut u64,
    resolution_beats: f64,
    start_beats: f64,
    end_beats: f64,
) -> Option<f64> {
    let resolution_beats = resolution_beats.max(1e-9);
    loop {
        let next_index = last_index.saturating_add(1);
        let boundary_beats = next_index as f64 * resolution_beats;
        if boundary_beats <= start_beats {
            *last_index = next_index;
            continue;
        }
        if boundary_beats <= end_beats {
            return Some(boundary_beats);
        }
        return None;
    }
}

/// A single timebase grid that emits boundary crossings against the transport's
/// `total_beats`, sharing the exact stepping logic of the neural per-neuron clock.
/// Used by the process runtime (one clock per `:every` period).
#[derive(Clone, Copy, Debug)]
pub struct GridBoundaryClock {
    pub resolution_beats: f64,
    pub last_index: u64,
}

impl GridBoundaryClock {
    pub fn new(resolution_beats: f64) -> Self {
        Self {
            resolution_beats: resolution_beats.max(1e-9),
            last_index: 0,
        }
    }

    /// Return the next boundary in `(start_beats, end_beats]`, skipping any already
    /// passed. Does not advance past the returned boundary — the driver
    /// ([`process_grid_boundaries`]) advances once the boundary is handled.
    pub fn next_boundary(&mut self, start_beats: f64, end_beats: f64) -> Option<f64> {
        next_grid_boundary(
            &mut self.last_index,
            self.resolution_beats,
            start_beats,
            end_beats,
        )
    }

    /// Re-anchor the clock to the current transport position without emitting the
    /// boundaries in between (mirrors `NeuralRuntime::realign_timing_to_config`).
    pub fn realign(&mut self, total_beats: f64) {
        self.last_index = (total_beats / self.resolution_beats.max(1e-9)).floor() as u64;
    }
}

/// Drive `callback(boundary_beats, grid_index, sample_time)` for every boundary of
/// `clock` within `(start_beats, end_beats]`, ascending, computing sample-accurate
/// timing exactly as the neural runtime does (`(beats * spq).round().max(0)` offset
/// from `block_start_sample`). The clock is advanced past each boundary as it is
/// delivered, so a boundary landing on `end_beats` fires once and is not repeated at
/// the next block's `start_beats`.
pub fn process_grid_boundaries<F: FnMut(f64, u64, u64)>(
    clock: &mut GridBoundaryClock,
    start_beats: f64,
    end_beats: f64,
    block_start_sample: u64,
    samples_per_quarter: f64,
    mut callback: F,
) {
    while let Some(boundary_beats) = clock.next_boundary(start_beats, end_beats) {
        let grid_index = (boundary_beats / clock.resolution_beats).round() as u64;
        let sample_time = sample_time_for_beat(start_beats, block_start_sample, samples_per_quarter, boundary_beats);
        callback(boundary_beats, grid_index, sample_time);
        clock.last_index = grid_index;
    }
}

/// The sample `beat` plays at in a block starting at `start_beats` /
/// `block_start_sample` (`(beats * spq).round().max(0)` from the block start).
pub(crate) fn sample_time_for_beat(
    start_beats: f64,
    block_start_sample: u64,
    samples_per_quarter: f64,
    beat: f64,
) -> u64 {
    let sample_offset = ((beat - start_beats) * samples_per_quarter)
        .round()
        .max(0.0) as u64;
    block_start_sample.saturating_add(sample_offset)
}

/// Slack for comparing a computed boundary with a block edge derived from
/// sample counts.
const BOUNDARY_EPSILON: f64 = 1e-9;

/// How far (in beats) a block edge may sit from the boundary it stands for:
/// just under one sample. Block edges are whole samples, so a boundary
/// between two samples belongs to the block holding the sample it rounds up
/// to: the song puts a loop's end on `ceil(end * spq)`, and the boundary at
/// the loop end must stay out of the old cycle, which a wrap then replays as
/// beat 0.
pub(crate) fn boundary_tolerance(samples_per_quarter: f64) -> f64 {
    if samples_per_quarter.is_finite() && samples_per_quarter > 0.0 {
        (1.0 - 1e-6) / samples_per_quarter
    } else {
        BOUNDARY_EPSILON
    }
}

/// A generator's grid clock. Blocks cover `[start, end)`, so tick `k` falls
/// on boundary `k`: a clock realigned to beat 0 (Play, a loop wrap) ticks
/// the downbeat first, and a loop's end boundary belongs to the next
/// cycle's beat 0 rather than to the last tick of the old one. The shared
/// `(start, end]` clock skipped the boundary a block starts on, which put
/// `gen-tick` 0 on the second step (bead eseq-z5x4).
#[derive(Clone, Copy, Debug)]
pub struct DownbeatClock {
    pub resolution_beats: f64,
    /// The next boundary not yet ticked.
    next_index: u64,
}

impl DownbeatClock {
    pub fn new(resolution_beats: f64) -> Self {
        Self { resolution_beats: resolution_beats.max(1e-9), next_index: 0 }
    }

    /// Re-anchor to the transport position: the first boundary at or after
    /// `total_beats` ticks next, the one exactly at it included.
    pub fn realign(&mut self, total_beats: f64) {
        self.next_index = self.first_index_at_or_after(total_beats);
    }

    fn first_index_at_or_after(&self, beats: f64) -> u64 {
        (beats / self.resolution_beats - BOUNDARY_EPSILON).ceil().max(0.0) as u64
    }

    /// Call `callback(boundary_beats, grid_index, sample_time)` for every
    /// boundary in `[start_beats, end_beats)`, ascending, the edges within
    /// [`boundary_tolerance`]; a boundary before `start_beats` (the
    /// transport jumped ahead) is skipped.
    pub fn process<F: FnMut(f64, u64, u64)>(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        mut callback: F,
    ) {
        let tolerance = boundary_tolerance(samples_per_quarter);
        if (self.next_index as f64) * self.resolution_beats <= start_beats - tolerance {
            // The first boundary later than the tolerance before the start.
            let before = ((start_beats - tolerance) / self.resolution_beats).floor();
            self.next_index = if before < 0.0 { 0 } else { before as u64 + 1 };
        }
        loop {
            let boundary_beats = self.next_index as f64 * self.resolution_beats;
            // Written so a NaN end stops the loop instead of spinning it.
            if !(boundary_beats <= end_beats - tolerance) {
                return;
            }
            let sample_time = sample_time_for_beat(start_beats, block_start_sample, samples_per_quarter, boundary_beats);
            callback(boundary_beats, self.next_index, sample_time);
            self.next_index += 1;
        }
    }
}

#[cfg(test)]
mod downbeat_tests {
    use super::DownbeatClock;

    fn ticks(clock: &mut DownbeatClock, blocks: &[(f64, f64)]) -> Vec<f64> {
        let mut beats = Vec::new();
        for (start, end) in blocks {
            clock.process(*start, *end, 0, 48_000.0, |beat, _, _| beats.push(beat));
        }
        beats
    }

    #[test]
    fn a_clock_at_zero_ticks_the_downbeat_first_and_each_boundary_once() {
        let mut clock = DownbeatClock::new(0.25);
        clock.realign(0.0);
        let beats = ticks(&mut clock, &[(0.0, 0.3), (0.3, 0.5), (0.5, 1.0)]);
        assert_eq!(beats, vec![0.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn a_loop_wrap_hands_the_end_boundary_to_the_next_cycle() {
        let mut clock = DownbeatClock::new(0.25);
        clock.realign(0.0);
        let first = ticks(&mut clock, &[(0.0, 1.0)]);
        clock.realign(0.0);
        let second = ticks(&mut clock, &[(0.0, 1.0)]);
        assert_eq!(first, second);
        assert_eq!(first.first(), Some(&0.0));
        assert_eq!(first.last(), Some(&0.75), "beat 1.0 is the next cycle's 0");
    }

    #[test]
    fn a_loop_end_a_fraction_of_a_sample_late_stays_out_of_the_old_cycle() {
        // 123 BPM at 48 kHz: 23414.63 samples per quarter. The song puts a
        // one-bar loop's end on ceil(4 * spq), a hair past beat 4.
        let spq = 48_000.0 * 60.0 / 123.0;
        let end = (4.0 * spq as f64).ceil() / spq;
        assert!(end > 4.0);
        let mut clock = DownbeatClock::new(0.25);
        clock.realign(0.0);
        let mut beats = Vec::new();
        clock.process(0.0, end, 0, spq, |beat, _, _| beats.push(beat));
        assert_eq!(beats.len(), 16, "beat 4.0 is the next cycle's downbeat");
        assert_eq!(beats.last(), Some(&3.75));
    }

    #[test]
    fn realigning_mid_bar_starts_at_the_next_boundary_and_a_jump_skips_ahead() {
        let mut clock = DownbeatClock::new(0.25);
        clock.realign(1.1);
        assert_eq!(ticks(&mut clock, &[(1.1, 1.6)]), vec![1.25, 1.5]);
        assert_eq!(ticks(&mut clock, &[(3.0, 3.3)]), vec![3.0, 3.25]);
    }
}

