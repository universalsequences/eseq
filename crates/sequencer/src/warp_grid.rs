//! Beat-grid math for the sampler's Beats warp mode.
//!
//! The grid is uniform: one BPM, one downbeat anchor. Boundaries are computed
//! on demand as `anchor + k * division_frames` (all in SOURCE frames), so the
//! audio thread needs no precomputed table and bpm/preserve edits take effect
//! live. Preserve=Transients reuses the uniform 1/16 grid and snaps each
//! boundary to a detected onset when one lies within tolerance, so a missed
//! onset degrades to "grid-perfect" timing instead of wrong timing.

/// Preserve setting (matches the Beats-mode "preserve" param labels).
pub const PRESERVE_1_BAR: i32 = 0;
pub const PRESERVE_1_2: i32 = 1;
pub const PRESERVE_1_4: i32 = 2;
pub const PRESERVE_1_8: i32 = 3;
pub const PRESERVE_1_16: i32 = 4;
pub const PRESERVE_1_32: i32 = 5;
pub const PRESERVE_TRANSIENTS: i32 = 6;

/// Onsets within this many milliseconds of a grid point replace it
/// (Preserve=Transients only).
pub const TRANSIENT_SNAP_MS: f64 = 25.0;
const MIN_BOUNDARY_ADVANCE_FRAMES: f64 = 0.5;

/// Beats per grid division for a preserve setting. Assumes 4/4 for "1 bar".
/// Transients mode runs on the 1/16 base grid before snapping.
pub fn beats_per_division(preserve: i32) -> f64 {
    match preserve {
        PRESERVE_1_BAR => 4.0,
        PRESERVE_1_2 => 2.0,
        PRESERVE_1_4 => 1.0,
        PRESERVE_1_8 => 0.5,
        PRESERVE_1_32 => 0.125,
        // PRESERVE_1_16, PRESERVE_TRANSIENTS, and anything out of range
        _ => 0.25,
    }
}

/// Grid division length in source frames.
pub fn division_src_frames(sample_bpm: f64, source_sample_rate: f64, preserve: i32) -> f64 {
    let bpm = sample_bpm.clamp(20.0, 400.0);
    (60.0 / bpm) * beats_per_division(preserve) * source_sample_rate.max(1.0)
}

/// Smallest grid boundary meaningfully greater than `after_src_frame`.
/// The grid extends in both directions from the anchor (k may be negative),
/// so samples whose content starts before the downbeat still get boundaries.
pub fn next_grid_boundary(after_src_frame: f64, anchor_src_frame: f64, div_src_frames: f64) -> f64 {
    debug_assert!(div_src_frames > 0.0);
    let div = div_src_frames.max(1.0);
    let min_after = after_src_frame + MIN_BOUNDARY_ADVANCE_FRAMES.min(div * 0.5);
    let k = ((min_after - anchor_src_frame) / div).floor() + 1.0;
    let boundary = anchor_src_frame + k * div;
    // Guard against float rounding placing the boundary at/below `min_after`.
    if boundary <= min_after {
        boundary + div
    } else {
        boundary
    }
}

/// Snap a grid boundary to the nearest onset within `tolerance_frames`.
/// `onsets` must be sorted ascending (the analysis worker emits them sorted).
/// Returns the boundary unchanged when no onset is close enough — timing
/// stays grid-perfect where detection missed.
pub fn snap_boundary_to_onset(boundary: f64, onsets: &[u32], tolerance_frames: f64) -> f64 {
    if onsets.is_empty() || tolerance_frames <= 0.0 {
        return boundary;
    }
    let target = boundary.max(0.0) as u32;
    let idx = onsets.partition_point(|&f| f < target);
    let mut best = boundary;
    let mut best_dist = tolerance_frames;
    for candidate in [idx.checked_sub(1), Some(idx)].into_iter().flatten() {
        if let Some(&frame) = onsets.get(candidate) {
            let dist = (frame as f64 - boundary).abs();
            if dist <= best_dist {
                best_dist = dist;
                best = frame as f64;
            }
        }
    }
    best
}

/// Next Beats-mode segment boundary after `after_src_frame`: the uniform grid
/// boundary, transient-snapped when preserve is Transients.
pub fn next_segment_boundary(
    after_src_frame: f64,
    anchor_src_frame: f64,
    sample_bpm: f64,
    source_sample_rate: f64,
    preserve: i32,
    onsets: &[u32],
) -> f64 {
    let div = division_src_frames(sample_bpm, source_sample_rate, preserve);
    let boundary = next_grid_boundary(after_src_frame, anchor_src_frame, div);
    if preserve == PRESERVE_TRANSIENTS {
        let tolerance = TRANSIENT_SNAP_MS / 1000.0 * source_sample_rate;
        let snapped = snap_boundary_to_onset(boundary, onsets, tolerance);
        // A snap can only move a boundary backwards past the read position if
        // the onset predates `after`; keep the segment advancing far enough to
        // survive the sampler's f32 state storage.
        if snapped > after_src_frame + MIN_BOUNDARY_ADVANCE_FRAMES {
            snapped
        } else {
            boundary
        }
    } else {
        boundary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_lengths_follow_bpm_and_preserve() {
        // 120 BPM at 44.1k: one beat = 22050 frames.
        assert_eq!(division_src_frames(120.0, 44_100.0, PRESERVE_1_4), 22_050.0);
        assert_eq!(
            division_src_frames(120.0, 44_100.0, PRESERVE_1_BAR),
            88_200.0
        );
        assert_eq!(division_src_frames(120.0, 44_100.0, PRESERVE_1_16), 5_512.5);
        assert_eq!(
            division_src_frames(120.0, 44_100.0, PRESERVE_1_32),
            2_756.25
        );
        // Transients rides the 1/16 base grid.
        assert_eq!(
            division_src_frames(120.0, 44_100.0, PRESERVE_TRANSIENTS),
            5_512.5
        );
    }

    #[test]
    fn grid_boundaries_are_strictly_increasing_from_any_position() {
        let div = 5_512.5;
        let anchor = 1_000.0;
        // Mid-segment.
        assert_eq!(next_grid_boundary(3_000.0, anchor, div), anchor + div);
        // Exactly on a boundary → the NEXT one.
        assert_eq!(
            next_grid_boundary(anchor + div, anchor, div),
            anchor + 2.0 * div
        );
        // Before the anchor (content before the downbeat): grid extends back.
        assert_eq!(next_grid_boundary(0.0, anchor, div), anchor);
        let b = next_grid_boundary(-20_000.0, anchor, div);
        assert!(b > -20_000.0 && b < -20_000.0 + div + 0.001);
        // Grid-aligned relative to anchor.
        assert!(((b - anchor) / div).fract().abs() < 1e-9);
    }

    #[test]
    fn grid_boundaries_advance_after_f32_round_tripped_boundary() {
        let anchor = 11_674.0;
        let div = division_src_frames(121.896, 44_100.0, PRESERVE_1_16);
        let second_boundary = anchor + 2.0 * div;
        let after = second_boundary as f32 as f64;

        let next = next_grid_boundary(after, anchor, div);

        assert!(
            next > second_boundary + div - 0.001,
            "next boundary should skip the f32-rounded current boundary: after={after}, second={second_boundary}, next={next}"
        );
        assert_eq!(next as f32, (anchor + 3.0 * div) as f32);
    }

    #[test]
    fn ratio_orientation_a_174_break_in_a_120_project_consumes_source_slower() {
        // The warp ratio convention: ratio = project_bpm / sample_bpm =
        // source frames consumed per host frame (at equal sample rates).
        // 174 BPM sample, 120 BPM project → 120/174 ≈ 0.6897: SLOW-DOWN.
        let ratio = 120.0_f64 / 174.0;
        assert!((ratio - 0.689_655).abs() < 0.000_01);
        // One project beat (0.5 s at 120 BPM = 22050 host frames at 44.1k)
        // must consume exactly one sample beat of source (60/174 s).
        let host_frames_per_project_beat = 22_050.0;
        let source_consumed = host_frames_per_project_beat * ratio;
        let sample_beat_frames = division_src_frames(174.0, 44_100.0, PRESERVE_1_4);
        assert!((source_consumed - sample_beat_frames).abs() < 0.001);
    }

    #[test]
    fn transient_snap_uses_nearby_onset_and_ignores_far_ones() {
        let onsets = vec![100, 5_400, 11_200];
        let tol = 25.0 / 1000.0 * 44_100.0; // ≈ 1102 frames
                                            // Grid point 5512.5: onset 5400 is 112.5 frames away → snap.
        assert_eq!(snap_boundary_to_onset(5_512.5, &onsets, tol), 5_400.0);
        // Grid point 16537.5: nearest onset 11200 is way out of tolerance.
        assert_eq!(snap_boundary_to_onset(16_537.5, &onsets, tol), 16_537.5);
        // Empty onset table: unchanged.
        assert_eq!(snap_boundary_to_onset(5_512.5, &[], tol), 5_512.5);
    }

    #[test]
    fn segment_boundaries_snap_only_in_transients_mode_and_always_advance() {
        let onsets = vec![5_400, 10_900];
        // 120 BPM, 1/16 grid = 5512.5 frames, anchor at 0.
        let b = next_segment_boundary(0.0, 0.0, 120.0, 44_100.0, PRESERVE_TRANSIENTS, &onsets);
        assert_eq!(b, 5_400.0);
        // Same query on the plain 1/16 grid: no snapping.
        let b = next_segment_boundary(0.0, 0.0, 120.0, 44_100.0, PRESERVE_1_16, &onsets);
        assert_eq!(b, 5_512.5);
        // Read position already past the snapped onset (5400 < 5450): the
        // snap may not move the boundary behind the playhead.
        let b = next_segment_boundary(5_450.0, 0.0, 120.0, 44_100.0, PRESERVE_TRANSIENTS, &onsets);
        assert!(b > 5_450.0);
    }
}
