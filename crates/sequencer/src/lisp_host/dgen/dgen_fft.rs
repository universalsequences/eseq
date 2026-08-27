/*!
rustfft-backed DGen ABI v1 host-services table (eseq-linux.78).

The four host callbacks generated DGen spectral code calls are backed by
Accelerate/vDSP on Apple platforms (`audiograph/dgen_host_services.c`, still
the shipped table there) and by this module everywhere else. It replaces the
scalar radix-2 kernel in `audiograph/dgen_fft.c`, which measured ~16x slower
than rustfft on x86_64 (125 us vs 7.9 us for a 2048-point transform) — enough
that a Filter Table spectral hop landed ~1 ms of FFT in a single render block
and underran ALSA.

Conventions are the ones `audiograph/dgen_fft.h` documents, because generated
code's gain compensation is tuned to them, and they are exactly rustfft's:

  forward:  C[k] = sum(A[j] * e**(-i*2*pi*j*k/N))   (`FftDirection::Forward`)
  inverse:  C[k] = sum(A[j] * e**(+i*2*pi*j*k/N))   (`FftDirection::Inverse`)

neither direction scaled, natural bin order, no real-FFT packing of bin N/2.
`audiograph/dgen_fft.c` stays the portable reference the numeric tests pin
both backends against, and is still linked here as the fallback described
below.

Real-time rules this has to honour:

  * Setup creation may allocate; transforms may not. Both directions are
    planned for every length up to the setup's at creation time, along with
    `process_with_scratch` scratch, so a transform only reads plans and
    writes preallocated buffers.
  * A setup is shared. Generated code caches one setup per call site in a
    static, and the audiograph engine runs graph nodes on a worker pool, so
    two threads can be inside the same setup at once. The portable C setup is
    immutable and was safe by construction; rustfft needs mutable scratch, so
    each setup owns a small pool of workspaces claimed by CAS. If every
    workspace is busy the call falls back to the portable C kernel rather
    than blocking — slower, correct, and allocation-free.

`ESEQ_DGEN_PORTABLE_FFT=1` in the environment forces every transform down the
portable fallback, for A/B triage of this backend against the old one.
*/

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftDirection, FftPlanner};

use super::dgen_ffi::{DGenFFTSetupV1, DGenHostServicesV1, DGEN_ABI_VERSION_V1};

extern "C" {
    fn eseq_dgen_portable_fft_setup_create(log2_size: u32) -> DGenFFTSetupV1;
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
}

/// Mirrors `ESEQ_DGEN_FFT_MAX_LOG2_SIZE`; requests above it return NULL, the
/// way `vDSP_create_fftsetup` fails and generated code already NULL-checks.
const MAX_LOG2_SIZE: u32 = 24;

/// Workspace pool bounds. The pool only has to cover threads that can be
/// inside one setup simultaneously — the audio callback plus graph workers —
/// so it tracks `available_parallelism`, floored so a single-core box still
/// has slack and capped so a wide box does not blow the byte budget below.
const MIN_WORKSPACES: usize = 4;
const MAX_WORKSPACES: usize = 16;
/// Above this the pool shrinks instead of growing; large setups get fewer
/// workspaces (and lean on the portable fallback more) rather than allocating
/// hundreds of megabytes of scratch.
const WORKSPACE_BYTE_BUDGET: usize = 4 << 20;

fn portable_fft_forced() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    *FORCED.get_or_init(|| {
        std::env::var_os("ESEQ_DGEN_PORTABLE_FFT")
            .map(|value| !matches!(value.to_str(), Some("") | Some("0")))
            .unwrap_or(false)
    })
}

/// One interleave/scratch pair, claimed for the duration of a single
/// transform. `in_use` is the only thing that makes the `UnsafeCell`s sound:
/// a claim is an acquire CAS, a release is a release store.
struct Workspace {
    in_use: AtomicBool,
    interleaved: UnsafeCell<Box<[Complex32]>>,
    scratch: UnsafeCell<Box<[Complex32]>>,
}

impl Workspace {
    fn new(max_size: usize, scratch_len: usize) -> Self {
        Workspace {
            in_use: AtomicBool::new(false),
            interleaved: UnsafeCell::new(
                vec![Complex32::new(0.0, 0.0); max_size].into_boxed_slice(),
            ),
            scratch: UnsafeCell::new(
                vec![Complex32::new(0.0, 0.0); scratch_len].into_boxed_slice(),
            ),
        }
    }
}

struct WorkspaceGuard<'a> {
    workspace: &'a Workspace,
}

impl Drop for WorkspaceGuard<'_> {
    fn drop(&mut self) {
        self.workspace.in_use.store(false, Ordering::Release);
    }
}

/// The opaque `DGenFFTSetupV1` this table hands back. Leaked on purpose:
/// setups are process-lifetime statics in generated code, exactly as with
/// vDSP, and the table never offers a destroy.
struct RustFftSetup {
    max_log2_size: u32,
    /// Indexed by `log2_size`; empty when the portable fallback is forced.
    forward: Vec<Arc<dyn Fft<f32>>>,
    inverse: Vec<Arc<dyn Fft<f32>>>,
    workspaces: Box<[Workspace]>,
    /// Reference kernel for the pool-exhausted (and forced) paths.
    portable: DGenFFTSetupV1,
}

// The `UnsafeCell`s inside `Workspace` are arbitrated by its `in_use` flag,
// and the plans are `Arc<dyn Fft<f32>>`, which rustfft declares `Send + Sync`.
unsafe impl Send for RustFftSetup {}
unsafe impl Sync for RustFftSetup {}

impl RustFftSetup {
    fn new(log2_size: u32) -> Option<Self> {
        // Every setup keeps a portable setup, forced or not: it is what the
        // pool-exhausted path runs, and it must exist before any transform.
        let portable = unsafe { eseq_dgen_portable_fft_setup_create(log2_size) };
        if portable.is_null() {
            return None;
        }

        if portable_fft_forced() {
            return Some(RustFftSetup {
                max_log2_size: log2_size,
                forward: Vec::new(),
                inverse: Vec::new(),
                workspaces: Vec::new().into_boxed_slice(),
                portable,
            });
        }

        // One plan per direction per length, so an oversized setup serves
        // shorter transforms the way vDSP's does. Twiddle memory is a
        // geometric series in the largest length, not a multiple of it.
        let mut planner = FftPlanner::<f32>::new();
        let mut forward = Vec::with_capacity(log2_size as usize + 1);
        let mut inverse = Vec::with_capacity(log2_size as usize + 1);
        let mut scratch_len = 0usize;
        for level in 0..=log2_size {
            let size = 1usize << level;
            let plan_forward = planner.plan_fft(size, FftDirection::Forward);
            let plan_inverse = planner.plan_fft(size, FftDirection::Inverse);
            scratch_len = scratch_len
                .max(plan_forward.get_inplace_scratch_len())
                .max(plan_inverse.get_inplace_scratch_len());
            forward.push(plan_forward);
            inverse.push(plan_inverse);
        }

        let max_size = 1usize << log2_size;
        let per_workspace_bytes =
            (max_size + scratch_len).saturating_mul(std::mem::size_of::<Complex32>());
        let wanted = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(MIN_WORKSPACES)
            .clamp(MIN_WORKSPACES, MAX_WORKSPACES);
        let affordable = (WORKSPACE_BYTE_BUDGET / per_workspace_bytes.max(1)).max(1);
        let count = wanted.min(affordable);

        let workspaces = (0..count)
            .map(|_| Workspace::new(max_size, scratch_len))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Some(RustFftSetup {
            max_log2_size: log2_size,
            forward,
            inverse,
            workspaces,
            portable,
        })
    }

    fn claim(&self) -> Option<WorkspaceGuard<'_>> {
        for workspace in self.workspaces.iter() {
            if workspace
                .in_use
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(WorkspaceGuard { workspace });
            }
        }
        None
    }
}

/// Shared body of `fft_forward` / `fft_inverse`.
///
/// # Safety
/// `real` and `imaginary` must each address `1 << log2_size` writable floats,
/// and `setup` must be a live pointer from `fft_setup_create`.
unsafe fn run_transform(
    setup: DGenFFTSetupV1,
    real: *mut f32,
    imaginary: *mut f32,
    log2_size: u32,
    inverse: bool,
) {
    if setup.is_null() || real.is_null() || imaginary.is_null() {
        return;
    }
    let setup = &*(setup as *const RustFftSetup);
    if log2_size > setup.max_log2_size {
        return;
    }
    let size = 1usize << log2_size;
    // Length 1 is the identity, and the portable kernel returns early on it
    // too; leaving the buffer untouched is that identity.
    if size < 2 {
        return;
    }

    let plans = if inverse {
        &setup.inverse
    } else {
        &setup.forward
    };
    let plan = match plans.get(log2_size as usize) {
        Some(plan) => plan,
        None => return run_portable(setup.portable, real, imaginary, log2_size, inverse),
    };
    let guard = match setup.claim() {
        Some(guard) => guard,
        // Every workspace is busy on another thread. Blocking here would be a
        // priority-inversion hazard on the audio thread; the reference kernel
        // is slower but needs no shared state.
        None => return run_portable(setup.portable, real, imaginary, log2_size, inverse),
    };

    let interleaved: &mut Box<[Complex32]> = &mut *guard.workspace.interleaved.get();
    let buffer = &mut interleaved[..size];
    let scratch_store: &mut Box<[Complex32]> = &mut *guard.workspace.scratch.get();
    let scratch = &mut scratch_store[..plan.get_inplace_scratch_len()];

    // Scoped so the shared views of the caller's buffers are gone before the
    // exclusive ones below are made over the same memory.
    {
        let real_in = std::slice::from_raw_parts(real, size);
        let imaginary_in = std::slice::from_raw_parts(imaginary, size);
        for index in 0..size {
            buffer[index] = Complex32::new(real_in[index], imaginary_in[index]);
        }
    }

    plan.process_with_scratch(buffer, scratch);

    let real_out = std::slice::from_raw_parts_mut(real, size);
    let imaginary_out = std::slice::from_raw_parts_mut(imaginary, size);
    for index in 0..size {
        real_out[index] = buffer[index].re;
        imaginary_out[index] = buffer[index].im;
    }
}

unsafe fn run_portable(
    portable: DGenFFTSetupV1,
    real: *mut f32,
    imaginary: *mut f32,
    log2_size: u32,
    inverse: bool,
) {
    if inverse {
        eseq_dgen_portable_fft_inverse(portable, real, imaginary, log2_size);
    } else {
        eseq_dgen_portable_fft_forward(portable, real, imaginary, log2_size);
    }
}

unsafe extern "C" fn fft_setup_create(log2_size: u32) -> DGenFFTSetupV1 {
    if log2_size > MAX_LOG2_SIZE {
        return std::ptr::null_mut();
    }
    match RustFftSetup::new(log2_size) {
        Some(setup) => Box::into_raw(Box::new(setup)) as DGenFFTSetupV1,
        None => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn fft_forward(
    setup: DGenFFTSetupV1,
    real: *mut f32,
    imaginary: *mut f32,
    log2_size: u32,
) {
    run_transform(setup, real, imaginary, log2_size, false);
}

unsafe extern "C" fn fft_inverse(
    setup: DGenFFTSetupV1,
    real: *mut f32,
    imaginary: *mut f32,
    log2_size: u32,
) {
    run_transform(setup, real, imaginary, log2_size, true);
}

/// `accumulator += lhs * rhs`, elementwise, true complex product — vDSP_zvma.
/// Element-independent, so LLVM vectorises it without any fast-math relaxation
/// (the C kernel's 467 us for the conv-reverb partition sweep was `-O2`
/// leaving it scalar, not an inherently scalar loop).
#[inline(always)]
fn multiply_accumulate_slices(
    lhs_real: &[f32],
    lhs_imaginary: &[f32],
    rhs_real: &[f32],
    rhs_imaginary: &[f32],
    accumulator_real: &mut [f32],
    accumulator_imaginary: &mut [f32],
) {
    let count = accumulator_real.len();
    // Re-slice everything to one length so LLVM can drop the bounds checks and
    // vectorise; the caller builds all six from the same element count.
    let lhs_real = &lhs_real[..count];
    let lhs_imaginary = &lhs_imaginary[..count];
    let rhs_real = &rhs_real[..count];
    let rhs_imaginary = &rhs_imaginary[..count];
    let accumulator_imaginary = &mut accumulator_imaginary[..count];
    for index in 0..count {
        let product_real =
            lhs_real[index] * rhs_real[index] - lhs_imaginary[index] * rhs_imaginary[index];
        let product_imaginary =
            lhs_real[index] * rhs_imaginary[index] + lhs_imaginary[index] * rhs_real[index];
        accumulator_real[index] += product_real;
        accumulator_imaginary[index] += product_imaginary;
    }
}

/// Same loop with AVX2+FMA enabled for the inlined body. Baseline x86_64 is
/// SSE2; on the measured i5-8250U this is the difference between 115 us and
/// 63 us on the partition sweep.
///
/// # Safety
/// Caller must have verified `avx2` and `fma` are present.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2,fma")]
unsafe fn multiply_accumulate_avx2(
    lhs_real: &[f32],
    lhs_imaginary: &[f32],
    rhs_real: &[f32],
    rhs_imaginary: &[f32],
    accumulator_real: &mut [f32],
    accumulator_imaginary: &mut [f32],
) {
    multiply_accumulate_slices(
        lhs_real,
        lhs_imaginary,
        rhs_real,
        rhs_imaginary,
        accumulator_real,
        accumulator_imaginary,
    );
}

unsafe extern "C" fn complex_multiply_accumulate(
    lhs_real: *const f32,
    lhs_imaginary: *const f32,
    rhs_real: *const f32,
    rhs_imaginary: *const f32,
    accumulator_real: *mut f32,
    accumulator_imaginary: *mut f32,
    element_count: u32,
) {
    let count = element_count as usize;
    // The portable kernel tolerates a zero-length span with null pointers.
    if count == 0
        || lhs_real.is_null()
        || lhs_imaginary.is_null()
        || rhs_real.is_null()
        || rhs_imaginary.is_null()
        || accumulator_real.is_null()
        || accumulator_imaginary.is_null()
    {
        return;
    }

    let lhs_real = std::slice::from_raw_parts(lhs_real, count);
    let lhs_imaginary = std::slice::from_raw_parts(lhs_imaginary, count);
    let rhs_real = std::slice::from_raw_parts(rhs_real, count);
    let rhs_imaginary = std::slice::from_raw_parts(rhs_imaginary, count);
    let accumulator_real = std::slice::from_raw_parts_mut(accumulator_real, count);
    let accumulator_imaginary = std::slice::from_raw_parts_mut(accumulator_imaginary, count);

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
            return multiply_accumulate_avx2(
                lhs_real,
                lhs_imaginary,
                rhs_real,
                rhs_imaginary,
                accumulator_real,
                accumulator_imaginary,
            );
        }
    }

    multiply_accumulate_slices(
        lhs_real,
        lhs_imaginary,
        rhs_real,
        rhs_imaginary,
        accumulator_real,
        accumulator_imaginary,
    );
}

static HOST_SERVICES_V1: DGenHostServicesV1 = DGenHostServicesV1 {
    abi_version: DGEN_ABI_VERSION_V1,
    struct_size: std::mem::size_of::<DGenHostServicesV1>() as u32,
    fft_setup_create_fn: Some(fft_setup_create),
    fft_forward_fn: Some(fft_forward),
    fft_inverse_fn: Some(fft_inverse),
    complex_multiply_accumulate_fn: Some(complex_multiply_accumulate),
};

/// Process-lifetime static; never NULL. Wired up by
/// `dgen_ffi::dgen_host_services_v1` on non-Apple targets.
pub(in crate::lisp_host) fn host_services_v1() -> *const DGenHostServicesV1 {
    &HOST_SERVICES_V1
}
