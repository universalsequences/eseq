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

/// Quiet time before a reverb-like effect sleeps: past a 250 ms predelay
/// ceiling plus its longest tank loop, so nothing audible is still in flight.
pub const DEFAULT_HOLD_SECONDS: f32 = 1.0;

/// Every input port `0..ports` reads silence this block: declared silent by
/// its source (cheap), or else a peak at or below `INPUT_SILENCE`.
///
/// # Safety
/// `inp` must hold `ports` pointers (null ones count as silent), each valid
/// for `nf` reads.
pub unsafe fn ports_silent(inp: *const *mut f32, ports: usize, nf: usize) -> bool {
    (0..ports).all(|port| {
        super::silence::input_silent(port) || inputs_silent(&[*inp.add(port) as *const f32], nf)
    })
}

/// Whether the effect may skip this block: its input is silent and its
/// internal signal has been quiet for at least `hold_frames`.
///
/// # Safety
/// `quiet_cell` must point at the effect's quiet-frame state cell.
pub unsafe fn asleep(quiet_cell: *const f32, input_silent: bool, hold_frames: f32) -> bool {
    if force_awake(FORCE_NO_SLEEP) {
        return false;
    }
    input_silent && *quiet_cell >= hold_frames
}

/// The start of a sleeping block, folded: reads whether inputs `0..in_ports`
/// are silent and, when `allowed` and the quiet run has passed `hold_frames`,
/// emits declared silence on outputs `0..out_ports` and returns `None` (skip
/// the DSP). Otherwise returns whether this block counts toward the quiet
/// run, to pass to [`note_block`] once the block is rendered.
///
/// # Safety
/// As for [`ports_silent`], [`asleep`] and `silence::emit`.
pub unsafe fn sleep_or_count(
    quiet_cell: *const f32,
    inp: *const *mut f32,
    in_ports: usize,
    out: *const *mut f32,
    out_ports: usize,
    nframes: std::os::raw::c_int,
    hold_frames: f32,
    allowed: bool,
) -> Option<bool> {
    let input_silent = ports_silent(inp, in_ports, nframes.max(0) as usize);
    if allowed && asleep(quiet_cell, input_silent, hold_frames) {
        super::silence::emit(out, out_ports, nframes);
        return None;
    }
    Some(input_silent && allowed)
}

/// `FORCE_AWAKE` bit: settled bypasses run their full DSP.
pub const FORCE_NO_BYPASS_SKIP: u32 = 1;
/// `FORCE_AWAKE` bit: nothing tail-sleeps.
pub const FORCE_NO_SLEEP: u32 = 2;
/// `FORCE_AWAKE` bit: gatepitch renders its clock frame by frame and declares
/// no lane shapes, so idle voice modulators scan every frame (eseq-ka5t.16).
pub const FORCE_REFERENCE_VOICE_CONTROL: u32 = 4;

/// Whether the test-only A/B switch has `bit` set. Always false in
/// production builds.
#[inline]
pub fn force_awake(bit: u32) -> bool {
    #[cfg(test)]
    {
        FORCE_AWAKE.load(std::sync::atomic::Ordering::Relaxed) & bit != 0
    }
    #[cfg(not(test))]
    {
        let _ = bit;
        false
    }
}

/// Test builds only: render as if no effect ever slept or skipped a settled
/// bypass, for an A/B of what those change in a whole project
/// (`bounce::session::park_probe`). A process-wide atomic rather than a
/// thread-local: the graph's worker threads run kernels too, and a render's
/// nodes land on either. Set it only through [`ForceAwake`].
#[cfg(test)]
static FORCE_AWAKE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Sets the `FORCE_AWAKE` bits for its lifetime and restores the old value.
#[cfg(test)]
pub struct ForceAwake(u32);

#[cfg(test)]
impl ForceAwake {
    pub fn set(bits: u32) -> Self {
        Self(FORCE_AWAKE.swap(bits, std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
impl Drop for ForceAwake {
    fn drop(&mut self) {
        FORCE_AWAKE.store(self.0, std::sync::atomic::Ordering::Relaxed);
    }
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

/// Test harness: one effect instance that may sleep against a never-sleeping
/// twin (its quiet cell forced to 0 before every block), both fed the same
/// excitation, a silent tail long enough to sleep, then a wake block.
#[cfg(test)]
pub(crate) mod test_support {
    use std::os::raw::{c_int, c_void};

    pub(crate) type Kernel =
        unsafe extern "C" fn(*const *mut f32, *const *mut f32, c_int, *mut c_void, *mut c_void);

    pub(crate) struct Instance {
        pub state: *mut c_void,
        pub quiet: *mut f32,
    }

    #[derive(Debug)]
    pub(crate) struct SleepReport {
        /// Silent blocks the sleeper skipped (quiet run past the hold at entry).
        pub slept_blocks: usize,
        /// Every output sample of the skipped blocks was exactly 0.
        pub slept_output_exact_zero: bool,
        /// Largest sample the reference rendered while the sleeper slept.
        pub reference_peak_while_asleep: f32,
        /// Largest |sleeper - reference| over the whole wake block.
        pub wake_max_diff: f32,
        /// Largest |reference| over the wake block (scale for `wake_max_diff`).
        pub wake_reference_peak: f32,
        /// The sleeper's own quiet run after the wake block (0 = woke).
        pub quiet_after_wake: f32,
    }

    unsafe fn run(kernel: Kernel, inst: &Instance, inputs: usize, stereo: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let nf = stereo.len();
        let mut lanes: Vec<Vec<f32>> = (0..inputs).map(|_| vec![0.0; nf]).collect();
        for (frame, [l, r]) in stereo.iter().enumerate() {
            lanes[0][frame] = *l;
            lanes[1][frame] = *r;
        }
        let ins: Vec<*mut f32> = lanes.iter_mut().map(|lane| lane.as_mut_ptr()).collect();
        let mut out_l = vec![f32::NAN; nf];
        let mut out_r = vec![f32::NAN; nf];
        let outs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        kernel(ins.as_ptr(), outs.as_ptr(), nf as c_int, inst.state, std::ptr::null_mut());
        out_l.into_iter().zip(out_r).map(|(l, r)| [l, r]).collect()
    }

    /// Deterministic noise burst, `frames` long, at `level`.
    pub(crate) fn burst(frames: usize, level: f32, seed: u32) -> Vec<[f32; 2]> {
        let mut x = seed.max(1);
        (0..frames)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let v = (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * level;
                [v, -0.5 * v]
            })
            .collect()
    }

    /// Feed `excite`, then `silent_blocks` silent blocks of `block` frames,
    /// then `wake`, to both instances; `hold` is the sleeper's hold in frames.
    pub(crate) unsafe fn sleep_and_wake(
        kernel: Kernel,
        sleeper: &Instance,
        reference: &Instance,
        inputs: usize,
        excite: &[[f32; 2]],
        block: usize,
        silent_blocks: usize,
        hold: f32,
        wake: &[[f32; 2]],
    ) -> SleepReport {
        for chunk in excite.chunks(block) {
            *reference.quiet = 0.0;
            let a = run(kernel, sleeper, inputs, chunk);
            let b = run(kernel, reference, inputs, chunk);
            assert_eq!(
                a.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "an awake effect must render exactly like the reference"
            );
        }
        let silence = vec![[0.0f32; 2]; block];
        let mut report = SleepReport {
            slept_blocks: 0,
            slept_output_exact_zero: true,
            reference_peak_while_asleep: 0.0,
            wake_max_diff: 0.0,
            wake_reference_peak: 0.0,
            quiet_after_wake: 0.0,
        };
        for _ in 0..silent_blocks {
            let asleep = *sleeper.quiet >= hold;
            *reference.quiet = 0.0;
            let a = run(kernel, sleeper, inputs, &silence);
            let b = run(kernel, reference, inputs, &silence);
            if asleep {
                report.slept_blocks += 1;
                report.slept_output_exact_zero &= a.iter().flatten().all(|v| v.to_bits() == 0);
                for v in b.iter().flatten() {
                    report.reference_peak_while_asleep = report.reference_peak_while_asleep.max(v.abs());
                }
            }
        }
        *reference.quiet = 0.0;
        let a = run(kernel, sleeper, inputs, wake);
        let b = run(kernel, reference, inputs, wake);
        for (x, y) in a.iter().flatten().zip(b.iter().flatten()) {
            report.wake_max_diff = report.wake_max_diff.max((x - y).abs());
            report.wake_reference_peak = report.wake_reference_peak.max(y.abs());
        }
        report.quiet_after_wake = *sleeper.quiet;
        report
    }
}
