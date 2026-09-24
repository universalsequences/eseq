//! Silence sleep for tail effects (reverbs, delays).
//!
//! An effect whose input has been silent, and whose internal signal has stayed
//! below `QUIET_LEVEL` for longer than its longest internal delay, holds no
//! audible energy: every sample still in its buffers is below the threshold
//! and will only decay further. It can output silence and skip its DSP until
//! input arrives again, which is what keeps an idle send bus nearly free.
//!
//! The quiet run length lives in one `f32` state cell of the effect. Callers
//! measure their internal (pre-mix) signal, so a reverb at 0% mix whose tank
//! still rings never sleeps.

/// Inputs at or below this magnitude count as silence (about -140 dBFS).
pub const INPUT_SILENCE: f32 = 1.0e-7;
/// Internal signal at or below this counts as decayed (-120 dBFS).
pub const QUIET_LEVEL: f32 = 1.0e-6;
/// Quiet frame counts saturate here, exactly representable in `f32`.
const QUIET_FRAMES_CAP: f32 = 16_777_216.0;

/// Largest magnitude in `nf` samples of each non-null channel.
///
/// # Safety
/// Each non-null pointer must be valid for `nf` reads.
pub unsafe fn peak(channels: &[*const f32], nf: usize) -> f32 {
    let mut peak = 0.0f32;
    let mut nan = false;
    for &channel in channels {
        if channel.is_null() {
            continue;
        }
        for &sample in std::slice::from_raw_parts(channel, nf) {
            peak = peak.max(sample.abs());
            nan |= sample.is_nan();
        }
    }
    // `max` skips NaN; a NaN signal is not silence.
    if nan {
        f32::INFINITY
    } else {
        peak
    }
}

/// True when every channel is silent for the whole block.
///
/// # Safety
/// As for [`peak`].
pub unsafe fn inputs_silent(channels: &[*const f32], nf: usize) -> bool {
    peak(channels, nf) <= INPUT_SILENCE
}

/// Whether the effect may skip this block: its input is silent and its
/// internal signal has been quiet for at least `hold_frames`.
///
/// # Safety
/// `quiet_cell` must point at the effect's quiet-frame state cell.
pub unsafe fn asleep(quiet_cell: *const f32, input_silent: bool, hold_frames: f32) -> bool {
    input_silent && *quiet_cell >= hold_frames
}

/// Record one processed block: extend the quiet run when the input was silent
/// and the internal signal stayed below `QUIET_LEVEL`, restart it otherwise.
///
/// # Safety
/// `quiet_cell` must point at the effect's quiet-frame state cell.
pub unsafe fn note_block(quiet_cell: *mut f32, input_silent: bool, internal_peak: f32, nf: usize) {
    *quiet_cell = if input_silent && internal_peak <= QUIET_LEVEL {
        (*quiet_cell + nf as f32).min(QUIET_FRAMES_CAP)
    } else {
        0.0
    };
}
