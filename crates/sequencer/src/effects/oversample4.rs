//! 4x oversampling and idle bypass shared by the solved-circuit shapers
//! (`triode.rs`, `opamp_clip.rs`, `fuzz.rs`, `hysteresis.rs`).
//!
//! Two linear-phase halfband stages each way (host <-> 2x <-> 4x), in
//! polyphase form. A halfband of length 2c+1 (c odd) has a 0.5 centre tap and
//! c+1 nonzero "side" taps at the even indices; every other tap is 0.

// Stage A, 35 taps (c = 17): 70 dB stopband from 30 kHz at a 96 kHz rate,
// -0.25 dB at 20 kHz (`scipy.signal.remez`, edges 0.1875 / 0.3125).
const HB_A_SIDE: [f32; 18] = side_taps([
    0.315160696, -0.097198172, 0.049796017, -0.027872135, 0.015439162, -0.008044156,
    0.003766351, -0.001483923, 0.000436161,
]);
// Stage B up, 19 taps (c = 9, 83 dB), and down, 15 taps (c = 7, 66 dB): the
// band is already empty above 24 kHz at a 192 kHz rate, so these are short.
// The up filter is the stricter one: a clipping stage turns any image left
// in its input into intermodulation that lands back in the audio band.
const HB_B_UP_SIDE: [f32; 10] =
    side_taps([0.30774095, -0.077946953, 0.02619617, -0.007112038, 0.001121871]);
const HB_B_DOWN_SIDE: [f32; 8] =
    side_taps([0.305516134, -0.072356402, 0.020579707, -0.003739439]);

/// Lay out a halfband's side taps (given centre-outward) in index order.
const fn side_taps<const H: usize, const N: usize>(outward: [f32; H]) -> [f32; N] {
    assert!(N == 2 * H);
    let mut taps = [0.0; N];
    let mut k = 0;
    while k < H {
        taps[H + k] = outward[k];
        taps[H - 1 - k] = outward[k];
        k += 1;
    }
    taps
}

const RING_A: usize = 64;
const RING_B: usize = 16;
const OS_A_UP: usize = 0;
const OS_B_UP: usize = OS_A_UP + RING_A;
const OS_B_DOWN: usize = OS_B_UP + RING_B;
const OS_A_DOWN: usize = OS_B_DOWN + RING_B;
const OS_POS_A_UP: usize = OS_A_DOWN + RING_A;
const OS_POS_B_UP: usize = OS_POS_A_UP + 1;
const OS_POS_B_DOWN: usize = OS_POS_B_UP + 1;
const OS_POS_A_DOWN: usize = OS_POS_B_DOWN + 1;
// Consecutive silent input samples, for the idle bypass.
const OS_QUIET: usize = OS_POS_A_DOWN + 1;

/// Floats of oversampler state per channel.
pub const STATE_LEN: usize = OS_QUIET + 1;
/// Host-rate samples [`process`] delays the signal by (the four linear-phase
/// halfbands: 17 + 17 samples at 2x, 9 + 7 at 4x).
pub const LATENCY_SAMPLES: usize = 21;
/// The solver runs at this multiple of the host rate.
pub const FACTOR: f64 = 4.0;

// Input this quiet for this long (seconds) means a circuit's capacitors are
// back at rest and its output is far below audibility: stop solving until
// signal returns.
const QUIET_LEVEL: f32 = 1.0e-9;
const QUIET_SECONDS: f64 = 2.0;

/// Host-rate samples of silence after which [`idle`] parks a circuit.
pub fn quiet_samples(host_rate: f64) -> f32 {
    (QUIET_SECONDS * host_rate) as f32
}

/// Whether a bias control counts as centred for [`idle`]. Not `== 0.0`: the
/// host's one-pole smoother decays toward zero and stalls on a subnormal
/// (denormals are only flushed on x86), which would keep the circuit solving
/// on silence forever once the knob had been moved and put back.
#[inline]
pub fn bias_at_rest(bias: f32) -> bool {
    bias.abs() < 1.0e-6
}

/// Idle bypass. `st` is a circuit's whole state block of `len` floats, laid
/// out so that all-zero is the circuit at rest, with this module's state at
/// `os`. Returns true when the caller should output silence without solving.
/// `at_rest_setting` is false while a control holds the circuit at some other
/// resting state, which the solve then has to find.
///
/// # Safety
/// `st` must point to `len` valid floats, `os + STATE_LEN <= len`.
#[inline]
pub unsafe fn idle(
    st: *mut f32,
    len: usize,
    os: usize,
    x: f32,
    at_rest_setting: bool,
    quiet_samples: f32,
) -> bool {
    let counter = st.add(os + OS_QUIET);
    if x.abs() <= QUIET_LEVEL && at_rest_setting {
        let quiet = *counter;
        if quiet >= quiet_samples {
            if quiet == quiet_samples {
                // First idle sample: park every state at rest (all zero).
                std::ptr::write_bytes(st, 0, len);
                *st.add(os + OS_QUIET) = quiet_samples + 1.0;
            }
            return true;
        }
        *counter = quiet + 1.0;
    } else {
        *counter = 0.0;
    }
    false
}

/// Advance a ring and store `x`; returns the new head.
#[inline]
unsafe fn ring_push(ring: *mut f32, mask: usize, pos: *mut f32, x: f32) -> usize {
    let p = (*pos as usize + 1) & mask;
    *pos = p as f32;
    *ring.add(p) = x;
    p
}

/// Sum of `taps[j] * ring[head - j * stride]`.
#[inline]
unsafe fn ring_dot(ring: *const f32, mask: usize, head: usize, stride: usize, taps: &[f32]) -> f32 {
    let mut acc = 0.0;
    for (j, &h) in taps.iter().enumerate() {
        acc += h * *ring.add((head + mask + 1 - j * stride) & mask);
    }
    acc
}

#[inline]
unsafe fn ring_at(ring: *const f32, mask: usize, head: usize, back: usize) -> f32 {
    *ring.add((head + mask + 1 - back) & mask)
}

/// One host-rate sample through `step`, which is called four times at 4x the
/// host rate. The output lags the input by [`LATENCY_SAMPLES`].
///
/// # Safety
/// `os` must point to [`STATE_LEN`] valid floats that `step` does not touch.
#[inline]
pub unsafe fn process(os: *mut f32, x: f32, mut step: impl FnMut(f32) -> f32) -> f32 {
    const MASK_A: usize = RING_A - 1;
    const MASK_B: usize = RING_B - 1;
    // Upsampling a zero-stuffed stream by a halfband (gain 2): the even
    // output phase is the side taps over the input history, the odd phase is
    // the centre tap alone, a pure delay of (c - 1) / 2 input samples.
    let a_up = os.add(OS_A_UP);
    let head_a = ring_push(a_up, MASK_A, os.add(OS_POS_A_UP), x);
    let mut out = 0.0;
    for a in 0..2 {
        let u = if a == 0 {
            2.0 * ring_dot(a_up, MASK_A, head_a, 1, &HB_A_SIDE)
        } else {
            ring_at(a_up, MASK_A, head_a, 8)
        };
        let b_up = os.add(OS_B_UP);
        let head_b = ring_push(b_up, MASK_B, os.add(OS_POS_B_UP), u);
        for b in 0..2 {
            let v = if b == 0 {
                2.0 * ring_dot(b_up, MASK_B, head_b, 1, &HB_B_UP_SIDE)
            } else {
                ring_at(b_up, MASK_B, head_b, 4)
            };
            let w = step(v);
            // Decimating on the first phase of each pair makes the total
            // delay a whole number of host samples. A halfband read is its
            // side taps (every second sample) plus half the centre one.
            let b_down = os.add(OS_B_DOWN);
            let head = ring_push(b_down, MASK_B, os.add(OS_POS_B_DOWN), w);
            if b == 0 {
                let d = ring_dot(b_down, MASK_B, head, 2, &HB_B_DOWN_SIDE)
                    + 0.5 * ring_at(b_down, MASK_B, head, 7);
                let a_down = os.add(OS_A_DOWN);
                let head = ring_push(a_down, MASK_A, os.add(OS_POS_A_DOWN), d);
                if a == 0 {
                    out = ring_dot(a_down, MASK_A, head, 2, &HB_A_SIDE)
                        + 0.5 * ring_at(a_down, MASK_A, head, 17);
                }
            }
        }
    }
    out
}
