/*!
Defines the FFI surface between the audio engine and compiled DGenLisp dylibs.

A compiled effect or instrument is a C dylib exporting the DGen ABI v1 process
function (`dgen_process_v1`, bound here as `DGenProcessFn`; the ABI structs are
vendored in `audiograph/dgen_abi_v1.h` and mirrored as `#[repr(C)]` types
below). This file owns the raw state-buffer contract shared with the generated
C: the header slot layout (enabled flag, host sample rate, canary) followed by
the single memory span the generated code casts `state` to, the trick of
smuggling the process-fn pointer through f32 state slots
(`process_fn_pointer_chunks` / `dgen_process_fn_from_state`), and the
`dgenlisp_wrapper_process` / `dgenlisp_init` shims plus `dgenlisp_vtable()`
that adapt a dylib into a `LiveGraph` node. Also home to engine-wide limits
(`MAX_CUSTOM_FX`, `MAX_MIDI_FX_SLOTS`, ...) and the queued tensor/sample-rate
write helpers used to mutate dgen state at runtime.
*/

use super::super::*;

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
pub(in crate::lisp_host) static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(in crate::lisp_host) static MIDI_FX_DESCRIPTOR_CACHE: OnceLock<Mutex<HashMap<String, Vec<EffectDescriptor>>>> =
    OnceLock::new();

pub(in crate::lisp_host) fn read_eseqlisp_init_source() -> String {
    eseqlisp_init_candidates()
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

pub(in crate::lisp_host) fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    crate::paths::eseqlisp_init_candidates()
}

// ── dlopen FFI (macOS) ──

extern "C" {
    pub(in crate::lisp_host) fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub(in crate::lisp_host) fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub(in crate::lisp_host) fn dlerror() -> *const c_char;
}

pub(in crate::lisp_host) const RTLD_NOW: c_int = 2;

// ── DGen ABI v1 (mirrors audiograph/dgen_abi_v1.h, vendored from the staged
// toolchain's include/dgen_runtime.h) ──

pub const DGEN_ABI_VERSION_V1: u32 = 1;

/// Manifest `processAbi` value the vendored DGenLisp emits for ABI v1.
pub const DGEN_PROCESS_ABI_V1: &str = "dgen-host-abi-v1";

pub(in crate::lisp_host) type DGenFFTSetupV1 = *mut c_void;

#[repr(C)]
pub struct DGenProcessContextV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub sample_rate: f32,
    pub reserved: u32,
}

#[repr(C)]
pub struct DGenHostServicesV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub fft_setup_create_fn: Option<unsafe extern "C" fn(log2_size: u32) -> DGenFFTSetupV1>,
    pub fft_forward_fn: Option<
        unsafe extern "C" fn(
            setup: DGenFFTSetupV1,
            real: *mut f32,
            imaginary: *mut f32,
            log2_size: u32,
        ),
    >,
    pub fft_inverse_fn: Option<
        unsafe extern "C" fn(
            setup: DGenFFTSetupV1,
            real: *mut f32,
            imaginary: *mut f32,
            log2_size: u32,
        ),
    >,
    pub complex_multiply_accumulate_fn: Option<
        unsafe extern "C" fn(
            lhs_real: *const f32,
            lhs_imaginary: *const f32,
            rhs_real: *const f32,
            rhs_imaginary: *const f32,
            accumulator_real: *mut f32,
            accumulator_imaginary: *mut f32,
            element_count: u32,
        ),
    >,
}

/// `dgen_process_v1` — the ABI v1 export of a compiled DGen dylib. `state` is
/// the single `float *memory` span (generated code casts it directly).
pub(in crate::lisp_host) type DGenProcessFn = unsafe extern "C" fn(
    inputs: *const *const f32,
    outputs: *const *mut f32,
    frame_count: u32,
    state: *mut c_void,
    context: *const DGenProcessContextV1,
    host: *const DGenHostServicesV1,
);

extern "C" {
    /// ESeq's Accelerate-backed host-services table
    /// (audiograph/dgen_host_services.c); process-lifetime static.
    fn eseq_dgen_host_services_v1() -> *const DGenHostServicesV1;
}

pub(in crate::lisp_host) fn dgen_host_services_v1() -> *const DGenHostServicesV1 {
    unsafe { eseq_dgen_host_services_v1() }
}

pub(in crate::lisp_host) fn dgen_process_context_v1(sample_rate: f32) -> DGenProcessContextV1 {
    DGenProcessContextV1 {
        abi_version: DGEN_ABI_VERSION_V1,
        struct_size: std::mem::size_of::<DGenProcessContextV1>() as u32,
        sample_rate,
        reserved: 0,
    }
}

pub const MAX_CUSTOM_FX: usize = 8;
pub const MAX_MIDI_FX_SLOTS: usize = 4;
pub const MAX_BUS_FX_CHAINS: usize = 64;

// ── Node state layout (ABI v1: single memory span) ──
// state[0] = host-local slot identity (diagnostics only)
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4] = enabled (0 = bypass/silent, 1 = active)
// state[5] = host sample rate
// state[6..10] = immutable process function pointer (four numeric u16 chunks)
// state[10..10+N] = DGenLisp memory span (generated code's `float *memory`)
// state[10+N..]   = redzone

pub const DGEN_ENABLED_PARAM_IDX: usize = 4;
pub const DGEN_HOST_SAMPLE_RATE_IDX: usize = 5;
pub(in crate::lisp_host) const DGEN_PROCESS_FN_START_IDX: usize = 6;
pub(in crate::lisp_host) const DGEN_PROCESS_FN_CHUNKS: usize = 4;
pub const HEADER_SLOTS: usize = 10;
pub const DGEN_STATE_REDZONE_SLOTS: usize = 256;
pub(in crate::lisp_host) const HEADER_CANARY: f32 = f32::from_bits(0x4cd35a1d);

pub(in crate::lisp_host) fn ensure_enabled_param(params: &mut Vec<crate::effects::ParamDescriptor>) {
    if params
        .iter()
        .any(|param| param.name.eq_ignore_ascii_case("enabled"))
    {
        return;
    }
    params.push(EffectDescriptor::enabled_param(params.len() as u32, 1.0));
}

pub fn dgen_total_state_slots(total_memory_slots: usize) -> usize {
    HEADER_SLOTS + total_memory_slots + DGEN_STATE_REDZONE_SLOTS
}

pub(in crate::lisp_host) unsafe fn dgen_memory_ptr(state: *mut f32) -> *mut f32 {
    state.add(HEADER_SLOTS)
}

pub(in crate::lisp_host) unsafe fn dgen_host_sample_rate(state: *mut f32) -> f32 {
    let sample_rate = *state.add(DGEN_HOST_SAMPLE_RATE_IDX);
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        44_100.0
    }
}

pub(in crate::lisp_host) fn process_fn_pointer_chunks(process_fn: DGenProcessFn) -> [f32; DGEN_PROCESS_FN_CHUNKS] {
    let pointer = process_fn as usize as u64;
    std::array::from_fn(|chunk| ((pointer >> (chunk * 16)) & 0xffff) as f32)
}

pub(in crate::lisp_host) unsafe fn dgen_process_fn_from_state(state: *mut f32) -> Option<DGenProcessFn> {
    let mut pointer = 0u64;
    for chunk in 0..DGEN_PROCESS_FN_CHUNKS {
        let value = *state.add(DGEN_PROCESS_FN_START_IDX + chunk);
        if !value.is_finite() || !(0.0..=u16::MAX as f32).contains(&value) {
            return None;
        }
        pointer |= (value as u16 as u64) << (chunk * 16);
    }
    (pointer != 0).then(|| std::mem::transmute::<usize, DGenProcessFn>(pointer as usize))
}

pub(in crate::lisp_host) unsafe extern "C" fn dgenlisp_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let s = state as *mut f32;
    if (*s.add(2)).to_bits() != HEADER_CANARY.to_bits() {
        return;
    }
    if *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5 {
        if inp.is_null() || out.is_null() {
            return;
        }
        let nf = nframes as usize;
        let input_count = (*s.add(3)).max(1.0) as usize;
        for ch in 0..input_count.min(2) {
            let in_ch = *inp.add(ch);
            let out_ch = *out.add(ch);
            if !in_ch.is_null() && !out_ch.is_null() {
                std::ptr::copy_nonoverlapping(in_ch as *const f32, out_ch, nf);
            }
        }
        return;
    }
    if let Some(process_fn) = dgen_process_fn_from_state(s) {
        let memory = dgen_memory_ptr(s) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        let context = dgen_process_context_v1(dgen_host_sample_rate(s));
        process_fn(
            inp as *const *const f32,
            out,
            nframes.max(0) as u32,
            memory,
            &context,
            dgen_host_services_v1(),
        );
    } else {
        // Passthrough: copy input to output
        let nf = nframes as usize;
        let in0 = *inp.add(0);
        let out0 = *out.add(0);
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
    }
}

/// Initial state message format (compact, not full-size):
///   [0] = slot_id
///   [1] = total_memory_slots
///   [2] = canary
///   [3] = declared input count
///   [4] = enabled
///   [5..9] = process function pointer (four numeric u16 chunks)
///   [9] = num_entries (N)
///   [10..10+2N] = pairs of (index, value)
pub(in crate::lisp_host) unsafe extern "C" fn dgenlisp_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    if initial_state.is_null() {
        return;
    }
    let src = initial_state as *const f32;
    let dst = state as *mut f32;

    // Copy header
    *dst = *src; // slot_id
    *dst.add(1) = *src.add(1); // total_memory_slots
    *dst.add(2) = *src.add(2); // canary
    *dst.add(3) = *src.add(3); // declared input count
    *dst.add(DGEN_ENABLED_PARAM_IDX) = *src.add(4); // enabled
    *dst.add(DGEN_HOST_SAMPLE_RATE_IDX) = (sample_rate.max(1)) as f32;
    for chunk in 0..DGEN_PROCESS_FN_CHUNKS {
        *dst.add(DGEN_PROCESS_FN_START_IDX + chunk) = *src.add(5 + chunk);
    }

    // Apply sparse index/value pairs into the memory region
    let num_entries = (*src.add(9)) as usize;
    let mem = dgen_memory_ptr(dst);
    for i in 0..num_entries {
        let idx = (*src.add(10 + i * 2)) as usize;
        let val = *src.add(10 + i * 2 + 1);
        *mem.add(idx) = val;
    }
}

/// Queue a bulk write of `data` into a live dgenlisp effect node's state at the
/// given tensor `cell_offset` (from the manifest's `tensors[]`). The write lands
/// in the memory span (`HEADER_SLOTS + cell_offset`) — the same region
/// params are written to and the span the DSP reads constant inputs from. The
/// engine applies it on the audio thread at a block boundary and copies the data
/// internally, so `data` may be freed immediately after this returns.
pub unsafe fn queue_tensor_write(
    lg: *mut LiveGraph,
    node_id: i32,
    cell_offset: usize,
    data: &[f32],
) -> bool {
    audiograph::write_node_state(
        lg,
        node_id,
        HEADER_SLOTS + cell_offset,
        data.as_ptr(),
        data.len(),
    )
}

pub unsafe fn queue_dgen_host_sample_rate_update(
    lg: *mut LiveGraph,
    node_id: i32,
    sample_rate: u32,
) -> bool {
    let value = sample_rate.max(1) as f32;
    audiograph::write_node_state(lg, node_id, DGEN_HOST_SAMPLE_RATE_IDX, &value, 1)
}

pub(in crate::lisp_host) fn dgenlisp_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}
