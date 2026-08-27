/*!
Numeric tests for the DGen ABI v1 host-services FFT (eseq-linux.9).

The host-services table is Accelerate/vDSP-backed on Apple platforms and
backed by the portable `audiograph/dgen_fft.c` everywhere else, so "does the
replacement match vDSP?" is checked transitively against a shared reference
rather than by linking both backends at once: every test here compares against
a naive double-precision DFT written straight from `vDSP_fft_zip`'s documented
pseudocode. `host_services_*` tests drive whichever backend this platform ships
(vDSP on macOS, rustfft elsewhere); `portable_*` tests drive the portable C
implementation directly and `rustfft_*` tests drive the rustfft-backed table
directly — both compile everywhere. Run the suite once on each host and all
three backends have been pinned to the same reference.

The conventions being pinned are the ones easy to get silently wrong:
unscaled in *both* directions (so a round trip scales by N, and generated
spectral gain compensation stays correct), natural bin ordering with no
real-FFT packing of bin N/2, and a true — not conjugated — complex product in
the multiply-accumulate.
*/

use super::super::{dgen_host_services_v1, DGenFFTSetupV1, DGEN_ABI_VERSION_V1};
use super::dgen_fft;

extern "C" {
    fn eseq_dgen_portable_fft_setup_create(log2_size: u32) -> DGenFFTSetupV1;
    fn eseq_dgen_portable_fft_setup_destroy(setup: DGenFFTSetupV1);
    fn eseq_dgen_portable_fft_forward(
        setup: DGenFFTSetupV1,
        real: *mut f32,
        imaginary: *mut f32,
        log2_size: u32,
    );
    fn eseq_dgen_portable_fft_inverse(
        setup: DGenFFTSetupV1,
        real: *mut f32,
        imaginary: *mut f32,
        log2_size: u32,
    );
    fn eseq_dgen_portable_complex_multiply_accumulate(
        lhs_real: *const f32,
        lhs_imaginary: *const f32,
        rhs_real: *const f32,
        rhs_imaginary: *const f32,
        accumulator_real: *mut f32,
        accumulator_imaginary: *mut f32,
        element_count: u32,
    );
}

/// The transforms are unscaled, so a bin can reach `n * max|x|`; with unit-
/// magnitude test inputs that makes `n` the natural error scale. 5e-6 of it is
/// roughly 40x f32 epsilon — tight enough to catch a wrong twiddle, a dropped
/// stage, or a 1/N normalisation, loose enough not to be flaky.
fn tolerance(n: usize) -> f64 {
    5e-6 * n as f64
}

/// Deterministic pseudo-random unit-ish samples; xorshift so the vectors are
/// identical on every platform the comparison has to hold across.
fn pseudo_random_signal(n: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f64 / u32::MAX as f64) * 2.0 - 1.0
    };
    let mut real = Vec::with_capacity(n);
    let mut imaginary = Vec::with_capacity(n);
    for _ in 0..n {
        real.push(next() as f32);
        imaginary.push(next() as f32);
    }
    (real, imaginary)
}

/// `vDSP_fft_zip`'s documented pseudocode, in f64:
///   forward  C[k] = sum(A[j] * e**(-i*2*pi*j*k/N))
///   inverse  C[k] = sum(A[j] * e**(+i*2*pi*j*k/N))
/// Neither direction scales.
fn naive_dft(real: &[f32], imaginary: &[f32], inverse: bool) -> (Vec<f64>, Vec<f64>) {
    let n = real.len();
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut out_real = vec![0.0_f64; n];
    let mut out_imaginary = vec![0.0_f64; n];
    for k in 0..n {
        let (mut sum_real, mut sum_imaginary) = (0.0_f64, 0.0_f64);
        for j in 0..n {
            let angle = sign * 2.0 * std::f64::consts::PI * (j as f64) * (k as f64) / (n as f64);
            let (sin, cos) = angle.sin_cos();
            let (jr, ji) = (real[j] as f64, imaginary[j] as f64);
            sum_real += jr * cos - ji * sin;
            sum_imaginary += jr * sin + ji * cos;
        }
        out_real[k] = sum_real;
        out_imaginary[k] = sum_imaginary;
    }
    (out_real, out_imaginary)
}

fn assert_close(
    label: &str,
    actual_real: &[f32],
    actual_imaginary: &[f32],
    expected_real: &[f64],
    expected_imaginary: &[f64],
    tolerance: f64,
) {
    for k in 0..actual_real.len() {
        let real_error = (actual_real[k] as f64 - expected_real[k]).abs();
        let imaginary_error = (actual_imaginary[k] as f64 - expected_imaginary[k]).abs();
        assert!(
            real_error <= tolerance && imaginary_error <= tolerance,
            "{label}: bin {k} was {}{:+}i, expected {}{:+}i \
             (error {real_error:.3e}/{imaginary_error:.3e} > {tolerance:.3e})",
            actual_real[k],
            actual_imaginary[k],
            expected_real[k],
            expected_imaginary[k],
        );
    }
}

/// Every transform the tests exercise: length 1 (the degenerate identity), the
/// first few radices, and enough stages to expose twiddle-table indexing bugs.
const LOG2_SIZES: [u32; 8] = [0, 1, 2, 3, 4, 6, 8, 10];

// ── the portable implementation, on every platform ──

#[test]
fn portable_fft_forward_matches_unscaled_dft() {
    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x9e37_79b9 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, false);

        let setup = unsafe { eseq_dgen_portable_fft_setup_create(log2_size) };
        assert!(!setup.is_null(), "setup for log2 {log2_size}");
        unsafe {
            eseq_dgen_portable_fft_forward(
                setup,
                real.as_mut_ptr(),
                imaginary.as_mut_ptr(),
                log2_size,
            );
            eseq_dgen_portable_fft_setup_destroy(setup);
        }

        assert_close(
            &format!("forward log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }
}

#[test]
fn portable_fft_inverse_matches_unscaled_conjugate_dft() {
    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x1234_5677 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, true);

        let setup = unsafe { eseq_dgen_portable_fft_setup_create(log2_size) };
        unsafe {
            eseq_dgen_portable_fft_inverse(
                setup,
                real.as_mut_ptr(),
                imaginary.as_mut_ptr(),
                log2_size,
            );
            eseq_dgen_portable_fft_setup_destroy(setup);
        }

        assert_close(
            &format!("inverse log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }
}

/// Neither direction normalises, so the round trip must come back scaled by N.
/// Generated DGen spectral code compensates for exactly this factor; a 1/N
/// slipped into either direction would silently rescale every spectral patch.
#[test]
fn portable_fft_round_trip_scales_by_length() {
    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let (original_real, original_imaginary) = pseudo_random_signal(n, 0x0bad_c0de ^ log2_size);
        let (mut real, mut imaginary) = (original_real.clone(), original_imaginary.clone());

        let setup = unsafe { eseq_dgen_portable_fft_setup_create(log2_size) };
        unsafe {
            eseq_dgen_portable_fft_forward(
                setup,
                real.as_mut_ptr(),
                imaginary.as_mut_ptr(),
                log2_size,
            );
            eseq_dgen_portable_fft_inverse(
                setup,
                real.as_mut_ptr(),
                imaginary.as_mut_ptr(),
                log2_size,
            );
            eseq_dgen_portable_fft_setup_destroy(setup);
        }

        let expected_real: Vec<f64> = original_real.iter().map(|x| *x as f64 * n as f64).collect();
        let expected_imaginary: Vec<f64> = original_imaginary
            .iter()
            .map(|x| *x as f64 * n as f64)
            .collect();
        assert_close(
            &format!("round trip log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n) * n as f64,
        );
    }
}

/// vDSP setups serve every transform at most as long as the length they were
/// created for, and generated DGen code leans on that: one lazily created
/// static setup per call site. The portable twiddle table has to be indexed by
/// the *transform's* length, not the setup's.
#[test]
fn portable_fft_setup_serves_shorter_transforms() {
    let setup = unsafe { eseq_dgen_portable_fft_setup_create(10) };
    assert!(!setup.is_null());

    for log2_size in [0, 1, 2, 3, 4, 6, 8, 10] {
        let n = 1usize << log2_size;
        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x5eed_1234 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, false);
        unsafe {
            eseq_dgen_portable_fft_forward(
                setup,
                real.as_mut_ptr(),
                imaginary.as_mut_ptr(),
                log2_size,
            );
        }
        assert_close(
            &format!("oversized setup, log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }

    unsafe { eseq_dgen_portable_fft_setup_destroy(setup) };
}

/// `vDSP_create_fftsetup` returns NULL on failure and generated DGen code
/// NULL-checks the setup before running the spectral op, so an unreasonable
/// request has to fail the same way rather than trapping.
#[test]
fn portable_fft_setup_rejects_oversized_request() {
    let setup = unsafe { eseq_dgen_portable_fft_setup_create(31) };
    assert!(setup.is_null(), "log2 31 must be refused, not attempted");
}

/// `vDSP_zvma(A, B, C, D)` is `D = A * B + C` with the table wired
/// `C == D == accumulator`: an accumulate, with an ordinary complex product —
/// no conjugation of either operand.
#[test]
fn portable_complex_multiply_accumulate_matches_reference() {
    let n = 37usize; // deliberately not a power of two or a vector multiple
    let (lhs_real, lhs_imaginary) = pseudo_random_signal(n, 0x00c0_ffee);
    let (rhs_real, rhs_imaginary) = pseudo_random_signal(n, 0x00de_1e7e);
    let (seed_real, seed_imaginary) = pseudo_random_signal(n, 0x00ab_cdef);

    let mut accumulator_real = seed_real.clone();
    let mut accumulator_imaginary = seed_imaginary.clone();
    unsafe {
        eseq_dgen_portable_complex_multiply_accumulate(
            lhs_real.as_ptr(),
            lhs_imaginary.as_ptr(),
            rhs_real.as_ptr(),
            rhs_imaginary.as_ptr(),
            accumulator_real.as_mut_ptr(),
            accumulator_imaginary.as_mut_ptr(),
            n as u32,
        );
    }

    for i in 0..n {
        let expected_real = seed_real[i] as f64
            + (lhs_real[i] as f64 * rhs_real[i] as f64
                - lhs_imaginary[i] as f64 * rhs_imaginary[i] as f64);
        let expected_imaginary = seed_imaginary[i] as f64
            + (lhs_real[i] as f64 * rhs_imaginary[i] as f64
                + lhs_imaginary[i] as f64 * rhs_real[i] as f64);
        assert!(
            (accumulator_real[i] as f64 - expected_real).abs() <= 1e-6
                && (accumulator_imaginary[i] as f64 - expected_imaginary).abs() <= 1e-6,
            "element {i}: {}{:+}i, expected {expected_real}{expected_imaginary:+}i",
            accumulator_real[i],
            accumulator_imaginary[i],
        );
    }
}

#[test]
fn portable_complex_multiply_accumulate_tolerates_empty_span() {
    unsafe {
        eseq_dgen_portable_complex_multiply_accumulate(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        );
    }
}

// ── the rustfft-backed table, on every platform (eseq-linux.78) ──
//
// `dgen_fft::host_services_v1()` is the table the app installs off Apple. It
// is driven directly here (rather than only through `dgen_host_services_v1()`)
// so a macOS run pins it to the same reference DFT the shipped vDSP table is
// pinned to — the whole point of the shared-reference scheme above.

#[test]
fn rustfft_table_matches_unscaled_dft_convention() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let forward = table.fft_forward_fn.expect("fft_forward_fn");
    let inverse = table.fft_inverse_fn.expect("fft_inverse_fn");

    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let setup = unsafe { setup_create(log2_size) };
        assert!(!setup.is_null(), "rustfft setup for log2 {log2_size}");

        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x51fe_0001 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, false);
        unsafe { forward(setup, real.as_mut_ptr(), imaginary.as_mut_ptr(), log2_size) };
        assert_close(
            &format!("rustfft forward log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );

        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x51fe_0002 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, true);
        unsafe { inverse(setup, real.as_mut_ptr(), imaginary.as_mut_ptr(), log2_size) };
        assert_close(
            &format!("rustfft inverse log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }
}

/// Same contract as `portable_fft_setup_serves_shorter_transforms`: generated
/// code creates one setup per call site and runs every length up to it.
#[test]
fn rustfft_table_setup_serves_shorter_transforms() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let forward = table.fft_forward_fn.expect("fft_forward_fn");

    let setup = unsafe { setup_create(10) };
    assert!(!setup.is_null());

    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x51fe_0003 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, false);
        unsafe { forward(setup, real.as_mut_ptr(), imaginary.as_mut_ptr(), log2_size) };
        assert_close(
            &format!("rustfft oversized setup, log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }
}

/// vDSP returns NULL for a request it cannot serve and generated code
/// NULL-checks before running the spectral op; the replacement has to agree.
#[test]
fn rustfft_table_setup_rejects_oversized_request() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let setup = unsafe { setup_create(31) };
    assert!(setup.is_null(), "log2 31 must be refused, not attempted");
}

/// The rustfft table's multiply-accumulate is a Rust loop with an AVX2/FMA
/// specialisation; 37 elements is neither a power of two nor a vector multiple,
/// so this exercises the scalar tail of whichever path this CPU takes.
#[test]
fn rustfft_table_complex_multiply_accumulate_matches_reference() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let multiply_accumulate = table
        .complex_multiply_accumulate_fn
        .expect("complex_multiply_accumulate_fn");

    let n = 37usize;
    let (lhs_real, lhs_imaginary) = pseudo_random_signal(n, 0x00c0_ffee);
    let (rhs_real, rhs_imaginary) = pseudo_random_signal(n, 0x00de_1e7e);
    let (seed_real, seed_imaginary) = pseudo_random_signal(n, 0x00ab_cdef);

    let mut accumulator_real = seed_real.clone();
    let mut accumulator_imaginary = seed_imaginary.clone();
    unsafe {
        multiply_accumulate(
            lhs_real.as_ptr(),
            lhs_imaginary.as_ptr(),
            rhs_real.as_ptr(),
            rhs_imaginary.as_ptr(),
            accumulator_real.as_mut_ptr(),
            accumulator_imaginary.as_mut_ptr(),
            n as u32,
        );
    }

    for i in 0..n {
        let expected_real = seed_real[i] as f64
            + (lhs_real[i] as f64 * rhs_real[i] as f64
                - lhs_imaginary[i] as f64 * rhs_imaginary[i] as f64);
        let expected_imaginary = seed_imaginary[i] as f64
            + (lhs_real[i] as f64 * rhs_imaginary[i] as f64
                + lhs_imaginary[i] as f64 * rhs_real[i] as f64);
        assert!(
            (accumulator_real[i] as f64 - expected_real).abs() <= 1e-6
                && (accumulator_imaginary[i] as f64 - expected_imaginary).abs() <= 1e-6,
            "element {i}: {}{:+}i, expected {expected_real}{expected_imaginary:+}i",
            accumulator_real[i],
            accumulator_imaginary[i],
        );
    }
}

#[test]
fn rustfft_table_complex_multiply_accumulate_tolerates_empty_span() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let multiply_accumulate = table
        .complex_multiply_accumulate_fn
        .expect("complex_multiply_accumulate_fn");
    unsafe {
        multiply_accumulate(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        );
    }
}

/// Generated code caches one setup per call site in a static and the audiograph
/// engine runs graph nodes on a worker pool, so one setup is driven from many
/// threads at once. The portable C setup was immutable and safe by
/// construction; the rustfft setup owns mutable scratch, so this hammers a
/// single setup from well over its workspace-pool size — which also forces the
/// pool-exhausted fallback — and requires every thread's result to match the
/// single-threaded one exactly.
#[test]
fn rustfft_table_setup_is_safe_to_share_across_threads() {
    let table = unsafe { &*dgen_fft::host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let forward = table.fft_forward_fn.expect("fft_forward_fn");

    const LOG2_SIZE: u32 = 8;
    let n = 1usize << LOG2_SIZE;
    let setup = unsafe { setup_create(LOG2_SIZE) };
    assert!(!setup.is_null());
    let setup = setup as usize; // raw pointers are not Send; the setup is.

    // Both the pooled rustfft path and the pool-exhausted portable fallback are
    // checked against the shared f64 reference rather than against each other:
    // they agree to tolerance, not bit-for-bit.
    let (signal_real, signal_imaginary) = pseudo_random_signal(n, 0x51fe_0004);
    let (expected_real, expected_imaginary) = naive_dft(&signal_real, &signal_imaginary, false);
    let expected = std::sync::Arc::new((expected_real, expected_imaginary));

    let threads: Vec<_> = (0..32)
        .map(|thread_index| {
            let expected = std::sync::Arc::clone(&expected);
            std::thread::spawn(move || {
                for _ in 0..64 {
                    let (mut real, mut imaginary) = pseudo_random_signal(n, 0x51fe_0004);
                    unsafe {
                        forward(
                            setup as DGenFFTSetupV1,
                            real.as_mut_ptr(),
                            imaginary.as_mut_ptr(),
                            LOG2_SIZE,
                        )
                    };
                    assert_close(
                        &format!("concurrent forward on thread {thread_index}"),
                        &real,
                        &imaginary,
                        &expected.0,
                        &expected.1,
                        tolerance(n),
                    );
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("worker thread");
    }
}

// ── the table this platform actually ships (vDSP on macOS, rustfft here) ──

#[test]
fn host_services_table_exposes_all_four_abi_v1_callbacks() {
    let table = unsafe { &*dgen_host_services_v1() };
    assert_eq!(table.abi_version, DGEN_ABI_VERSION_V1);
    assert_eq!(
        table.struct_size as usize,
        std::mem::size_of::<super::super::DGenHostServicesV1>(),
        "the ABI v1 table layout must stay byte-identical — generated dylibs \
         gate on `struct_size >= sizeof(DGenHostServicesV1)`"
    );
    assert!(table.fft_setup_create_fn.is_some());
    assert!(table.fft_forward_fn.is_some());
    assert!(table.fft_inverse_fn.is_some());
    assert!(table.complex_multiply_accumulate_fn.is_some());
}

/// Drives the shipped backend exactly the way generated DGen code does. On
/// macOS this pins vDSP to the same reference the `portable_*` tests pin the
/// replacement to, which is what makes the two backends comparable.
#[test]
fn host_services_fft_matches_unscaled_dft_convention() {
    let table = unsafe { &*dgen_host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let forward = table.fft_forward_fn.expect("fft_forward_fn");
    let inverse = table.fft_inverse_fn.expect("fft_inverse_fn");

    for log2_size in LOG2_SIZES {
        let n = 1usize << log2_size;
        let setup = unsafe { setup_create(log2_size) };
        assert!(!setup.is_null(), "host setup for log2 {log2_size}");

        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x7777_0001 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, false);
        unsafe { forward(setup, real.as_mut_ptr(), imaginary.as_mut_ptr(), log2_size) };
        assert_close(
            &format!("host forward log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );

        let (mut real, mut imaginary) = pseudo_random_signal(n, 0x7777_0002 ^ log2_size);
        let (expected_real, expected_imaginary) = naive_dft(&real, &imaginary, true);
        unsafe { inverse(setup, real.as_mut_ptr(), imaginary.as_mut_ptr(), log2_size) };
        assert_close(
            &format!("host inverse log2 {log2_size}"),
            &real,
            &imaginary,
            &expected_real,
            &expected_imaginary,
            tolerance(n),
        );
    }
}

#[test]
fn host_services_complex_multiply_accumulate_accumulates_a_true_product() {
    let table = unsafe { &*dgen_host_services_v1() };
    let multiply_accumulate = table
        .complex_multiply_accumulate_fn
        .expect("complex_multiply_accumulate_fn");

    // (1+2i) * (3+4i) = -5+10i, added onto a non-zero accumulator. A conjugated
    // product would give 11+2i, an overwrite would drop the seed.
    let lhs_real = [1.0_f32];
    let lhs_imaginary = [2.0_f32];
    let rhs_real = [3.0_f32];
    let rhs_imaginary = [4.0_f32];
    let mut accumulator_real = [100.0_f32];
    let mut accumulator_imaginary = [200.0_f32];
    unsafe {
        multiply_accumulate(
            lhs_real.as_ptr(),
            lhs_imaginary.as_ptr(),
            rhs_real.as_ptr(),
            rhs_imaginary.as_ptr(),
            accumulator_real.as_mut_ptr(),
            accumulator_imaginary.as_mut_ptr(),
            1,
        );
    }
    assert_eq!(accumulator_real[0], 95.0);
    assert_eq!(accumulator_imaginary[0], 210.0);
}

// ── throughput (eseq-linux.78) ──

/// Not a correctness test and not part of the default run — the reproducible
/// measurement behind eseq-linux.78, kept so the claim can be re-checked on a
/// new host. Release only; a debug build measures the optimiser being off.
///
///   cargo nextest run -p sequencer --release --run-ignored all \
///     -E 'test(=lisp_host::dgen::dgen_host_services_tests::\
///              throughput_of_the_shipped_table_against_the_portable_kernel)' \
///     --no-capture
///
/// These are hot-cache micro-benchmarks — every buffer stays resident across
/// iterations — so read them as a ratio between the two kernels, not as the
/// cost of a real spectral hop. Measured on the i5-8250U (Surface, x86_64
/// Linux) 2026-08-26: 4.99 us per 2048-point forward transform through the
/// shipped table (rustfft, including the split <-> interleaved conversion)
/// against 34.1 us through the portable kernel, and 16.8 us for the
/// 128x513-bin partition sweep through the shipped multiply-accumulate.
#[test]
#[ignore = "benchmark; run explicitly with --run-ignored all --no-capture"]
fn throughput_of_the_shipped_table_against_the_portable_kernel() {
    use std::time::Instant;

    const LOG2_SIZE: u32 = 11; // 2048, the Filter Table spectral length
    const TRANSFORMS: u32 = 2_000;
    let n = 1usize << LOG2_SIZE;

    let table = unsafe { &*dgen_fft::host_services_v1() };
    let setup_create = table.fft_setup_create_fn.expect("fft_setup_create_fn");
    let forward = table.fft_forward_fn.expect("fft_forward_fn");
    let multiply_accumulate = table
        .complex_multiply_accumulate_fn
        .expect("complex_multiply_accumulate_fn");

    let shipped = unsafe { setup_create(LOG2_SIZE) };
    let portable = unsafe { eseq_dgen_portable_fft_setup_create(LOG2_SIZE) };
    assert!(!shipped.is_null() && !portable.is_null());

    let (signal_real, signal_imaginary) = pseudo_random_signal(n, 0x51fe_0005);

    let time_transform = |label: &str, run: &mut dyn FnMut(&mut [f32], &mut [f32])| {
        let (mut real, mut imaginary) = (signal_real.clone(), signal_imaginary.clone());
        run(&mut real, &mut imaginary); // warm up
        let started = Instant::now();
        for _ in 0..TRANSFORMS {
            real.copy_from_slice(&signal_real);
            imaginary.copy_from_slice(&signal_imaginary);
            run(&mut real, &mut imaginary);
        }
        let per_transform = started.elapsed() / TRANSFORMS;
        println!("{label}: {per_transform:?} per {n}-point forward transform");
    };

    time_transform("shipped table", &mut |real, imaginary| unsafe {
        forward(
            shipped,
            real.as_mut_ptr(),
            imaginary.as_mut_ptr(),
            LOG2_SIZE,
        )
    });
    time_transform("portable kernel", &mut |real, imaginary| unsafe {
        eseq_dgen_portable_fft_forward(
            portable,
            real.as_mut_ptr(),
            imaginary.as_mut_ptr(),
            LOG2_SIZE,
        )
    });

    // The conv-reverb partition sweep the bead measured: 128 partitions of 513
    // bins each, accumulated into one spectrum.
    const PARTITIONS: usize = 128;
    const BINS: usize = 513;
    const SWEEPS: u32 = 200;
    let (lhs_real, lhs_imaginary) = pseudo_random_signal(BINS, 0x51fe_0006);
    let (rhs_real, rhs_imaginary) = pseudo_random_signal(BINS, 0x51fe_0007);
    let mut accumulator_real = vec![0.0_f32; BINS];
    let mut accumulator_imaginary = vec![0.0_f32; BINS];

    let mut sweep = || unsafe {
        for _ in 0..PARTITIONS {
            multiply_accumulate(
                lhs_real.as_ptr(),
                lhs_imaginary.as_ptr(),
                rhs_real.as_ptr(),
                rhs_imaginary.as_ptr(),
                accumulator_real.as_mut_ptr(),
                accumulator_imaginary.as_mut_ptr(),
                BINS as u32,
            );
        }
    };
    sweep(); // warm up
    let started = Instant::now();
    for _ in 0..SWEEPS {
        sweep();
    }
    println!(
        "shipped table: {:?} per {PARTITIONS}x{BINS} multiply-accumulate sweep",
        started.elapsed() / SWEEPS
    );

    unsafe { eseq_dgen_portable_fft_setup_destroy(portable) };
}
