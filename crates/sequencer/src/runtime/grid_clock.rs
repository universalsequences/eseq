//! Shared grid-boundary timebase for the self-clocked runtime engines.
//!
//! Extracted from `crate::neural` so the lisp-authorable engines
//! ([`super::generator`], [`super::graph`], [`super::process`]) don't depend on
//! the legacy builtin neural machine for their clock. The stepping logic is
//! shared verbatim with the per-neuron clock in `crate::neural` so all runtimes
//! step identically.

/// Advance `last_index` past any grid boundaries already at or before `start_beats`,
/// then return the next boundary that falls in `(start_beats, end_beats]`, if any.
///
/// This is the single-grid core of the per-neuron boundary detection used by
/// `crate::neural::NeuralRuntime::next_eval_boundary`; it is shared verbatim by
/// [`GridBoundaryClock`] so lisp generators and neurons step their clocks
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
/// Used by lisp-defined generators (one clock per generator's `:resolution`).
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
        let sample_offset = ((boundary_beats - start_beats) * samples_per_quarter)
            .round()
            .max(0.0) as u64;
        let sample_time = block_start_sample.saturating_add(sample_offset);
        callback(boundary_beats, grid_index, sample_time);
        clock.last_index = grid_index;
    }
}
