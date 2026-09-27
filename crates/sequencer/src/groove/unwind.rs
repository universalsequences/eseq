//! Record unwind (docs/rack-groove-spec.md §Sites 5, beads eseq-groove.6 and
//! eseq-k0v8): turning a HEARD position back into the straight step phase
//! that playback will move onto that heard position again.
//!
//! Playback moves a recorded hit by the feel of the step it sits on: the
//! member's rack groove at the step's straight transport boundary, or (with
//! no groove) the track swing of the step's bucket. Per-note delays add on
//! top. So a hit stored at `(step s, phase φ)` sounds at
//! `boundary[s] + φ * len[s] + shift[s]`. A live key press or a roll hit is
//! stamped where it was HEARD, which already contains the feel; storing that
//! position unchanged makes playback add the feel a second time. The unwind
//! solves the mapping backwards: find the step whose shifted span contains
//! the heard position and store `heard - shift[s]`.
//!
//! The mapping is not one-to-one near feel changes:
//! - two shifted spans can overlap (a late step followed by a straight one):
//!   both readings reproduce the heard time exactly, and the one with the
//!   smaller phase wins, the hit sits ON a step rather than at the tail of
//!   the one before;
//! - a late step leaves a gap before its shifted start that no stored phase
//!   reaches: a hit heard there reads as slightly early for that step and is
//!   stored at its phase 0 (the nearest reproducible position after it).
//!   A gap wider than the feel (a Sync wait) stays unresolved, as it was
//!   before any feel.

use crate::sequencer::SwingResolution;

use super::TrackGrooveSnapshot;

const EPS: f64 = 1.0e-9;

/// The swing delay, in beats, that the step scheduler applies to a step
/// whose cycle-start beat is `cycle_start_beats`: odd buckets of the
/// resolution grid are late by `(pct/100 - 0.5) * 2` buckets; even buckets
/// and swing at or below 50 do not move.
pub fn swing_shift_beats(
    swing_pct: f32,
    resolution: SwingResolution,
    cycle_start_beats: f64,
) -> f64 {
    if !(swing_pct > 50.0) {
        return 0.0;
    }
    let bucket = ((cycle_start_beats + EPS) / resolution.step_beats()).floor() as u64;
    if bucket % 2 == 0 {
        return 0.0;
    }
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution.step_beats()
}

/// A heard position resolved to where it must be STORED: the step and its
/// straight phase (0..1 of the step's span).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnwoundPosition {
    pub step: usize,
    pub phase: f64,
}

/// The straight `(step, phase)` whose playback lands on `heard_local`, a
/// position in the track's local cycle beats (`0 <= heard_local < cycle`).
///
/// `boundaries[s]..step_ends[s]` is step `s`'s straight span in local beats
/// (the same geometry the scheduler steps through). `shift_beats(s, base)` is
/// the feel playback adds to step `s` in the cycle that starts `base` local
/// beats from the heard cycle's start (`base` is `-cycle`, `0` or `cycle`):
/// a heard position near a cycle edge can belong to the last step of the
/// previous cycle (dragged late past the loop point) or the first step of
/// the next one (pushed early before it).
///
/// `None` only when the position is in a straight gap no feel explains (a
/// Sync wait), exactly where the un-felt lookup also has no step.
pub fn unwind_step_feel(
    heard_local: f64,
    cycle_beats: f64,
    boundaries: &[f64],
    step_ends: &[f64],
    mut shift_beats: impl FnMut(usize, f64) -> f64,
) -> Option<UnwoundPosition> {
    let num_steps = boundaries.len().min(step_ends.len());
    if num_steps == 0 || !heard_local.is_finite() || !(cycle_beats > 0.0) {
        return None;
    }
    // Best exact reading: (phase, |shift|, step).
    let mut exact: Option<(f64, f64, usize)> = None;
    // Best gap reading: (distance to the shifted start, step).
    let mut gap: Option<(f64, usize)> = None;
    for base in [-cycle_beats, 0.0, cycle_beats] {
        for step in 0..num_steps {
            let start = boundaries[step];
            let span = step_ends[step] - start;
            if !(span > 0.0) {
                continue;
            }
            let shift = shift_beats(step, base);
            let shift = if shift.is_finite() { shift } else { 0.0 };
            let straight = heard_local - shift - base;
            let rel = straight - start;
            if rel >= -EPS && rel < span - EPS {
                let phase = (rel / span).clamp(0.0, 1.0);
                let better = match exact {
                    None => true,
                    Some((best_phase, best_shift, _)) => {
                        phase < best_phase - EPS
                            || (phase <= best_phase + EPS && shift.abs() < best_shift)
                    }
                };
                if better {
                    exact = Some((phase, shift.abs(), step));
                }
            } else if rel < 0.0 {
                // The gap a step opens when it is later than the step before
                // it: from where the previous step's shift lands this
                // step's straight start up to its own shifted start. Heard
                // there, the hit is early for this step. A straight Sync
                // wait before the step is not part of it.
                let (prev, prev_base) = if step == 0 {
                    (num_steps - 1, base - cycle_beats)
                } else {
                    (step - 1, base)
                };
                let prev_shift = shift_beats(prev, prev_base);
                let prev_shift = if prev_shift.is_finite() {
                    prev_shift
                } else {
                    0.0
                };
                let gap_start = start + base + prev_shift;
                if prev_shift < shift && heard_local >= gap_start - EPS {
                    if gap.is_none_or(|(distance, _)| -rel < distance) {
                        gap = Some((-rel, step));
                    }
                }
            }
        }
    }
    if let Some((phase, _, step)) = exact {
        return Some(UnwoundPosition { step, phase });
    }
    gap.map(|(_, step)| UnwoundPosition { step, phase: 0.0 })
}

impl TrackGrooveSnapshot {
    /// The groove's deterministic pocket at `boundary_beats`, in beats:
    /// [`offset_beats`](Self::offset_beats) without the Random jitter.
    ///
    /// Recording unwinds this, not the jittered offset: Random is the
    /// groove's noise, not the player's timing. Unwinding one bar's jitter
    /// would print it into the pattern and playback would add a fresh
    /// jitter on top, doubling the spread; unwinding the pocket keeps the
    /// recorded hit on the player's position relative to the mean feel.
    pub fn pocket_offset_beats(&self, boundary_beats: f64) -> f64 {
        self.offset_beats_with_random(boundary_beats, false)
    }
}
