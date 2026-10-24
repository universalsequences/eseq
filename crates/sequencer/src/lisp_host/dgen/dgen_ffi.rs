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
    /// Portable C host-services table; process-lifetime static on every target.
    fn eseq_dgen_portable_host_services_v1() -> *const DGenHostServicesV1;

    #[cfg(target_vendor = "apple")]
    /// ESeq's Accelerate-backed host-services table
    /// (audiograph/dgen_host_services.c); process-lifetime static.
    fn eseq_dgen_host_services_v1() -> *const DGenHostServicesV1;
}

/// The ABI v1 host-services table generated dylibs are handed on every
/// `dgen_process_v1` call, so this stays a pointer read with no work in it.
///
/// Apple keeps the Accelerate/vDSP table in `dgen_host_services.c`. Everywhere
/// else the table is the rustfft-backed one in `dgen_fft.rs`: the portable C
/// kernel it replaces measured ~16x slower than rustfft on x86_64 and pushed
/// spectral effects into ALSA underruns (eseq-linux.78).
#[cfg(target_vendor = "apple")]
pub(in crate::lisp_host) fn dgen_host_services_v1() -> *const DGenHostServicesV1 {
    unsafe { eseq_dgen_host_services_v1() }
}

#[cfg(not(target_vendor = "apple"))]
pub(in crate::lisp_host) fn dgen_host_services_v1() -> *const DGenHostServicesV1 {
    super::dgen_fft::host_services_v1()
}

/// The portable C implementation, independent of the platform's shipped
/// table. Keeping this selectable lets callers load and prewarm a fresh dylib
/// image against the same table they will use to render it.
pub(in crate::lisp_host) fn dgen_portable_host_services_v1() -> *const DGenHostServicesV1 {
    unsafe { eseq_dgen_portable_host_services_v1() }
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
// state[0] = host-local slot identity (diagnostics only, read by nothing);
//            for an effect with probes, a negative probe code instead
//            (`probe_capture::encode_effect_probe_token`). Every header index
//            is taken: growing HEADER_SLOTS would shift every saved dgen
//            param's node-state index (`LEGACY_DGEN_HEADER_SLOTS` in
//            app/projects.rs), and a trailer past the redzone is clobbered by
//            audiograph's legacy 5-slot-header param mirror (graph_engine.c
//            `apply_params`) whenever memory cell 0 is written.
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4] = enabled (0 = bypass/silent, 1 = active)
// state[5] = host sample rate
// state[6..10] = immutable process function pointer (four numeric u16 chunks)
// state[10..10+N] = DGenLisp memory span (generated code's `float *memory`)
// state[10+N..10+N+256] = redzone
// state[10+N+256..+3] = host tail-sleep trailer (`DGEN_TAIL_SLOTS`): quiet
//            frames, the `(effect-tail-sleep [seconds])` opt-in and hold, and
//            the node's last seen state-write count. Never params; audiograph's dead legacy param
//            mirror over this range was removed with it.

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

/// Host-runtime slots after the redzone (the tail-sleep trailer).
pub const DGEN_TAIL_SLOTS: usize = 3;
/// Trailer cell: frames of silent input with output below -120 dB.
pub(in crate::lisp_host) const DGEN_TAIL_QUIET: usize = 0;
/// Trailer cell: the `(effect-tail-sleep [seconds])` opt-in. 0 = not opted
/// in; a positive value is the author-declared hold in seconds;
/// [`DGEN_TAIL_HOLD_FALLBACK`] = opted in without a hold (see
/// [`dgen_tail_hold_frames`]).
pub(in crate::lisp_host) const DGEN_TAIL_OPT_IN: usize = 1;
/// `DGEN_TAIL_OPT_IN` value for a bare `(effect-tail-sleep)`.
pub(in crate::lisp_host) const DGEN_TAIL_HOLD_FALLBACK: f32 = -1.0;
/// Floor and cap, in seconds, of the undeclared-hold fallback.
const DGEN_TAIL_FALLBACK_MIN_SECONDS: f32 = crate::effects::tail_sleep::DEFAULT_HOLD_SECONDS;
const DGEN_TAIL_FALLBACK_MAX_SECONDS: f32 = 10.0;
/// Trailer cell: the node's state-write count (bits) when last processed.
pub(in crate::lisp_host) const DGEN_TAIL_WRITES: usize = 2;

pub fn dgen_total_state_slots(total_memory_slots: usize) -> usize {
    HEADER_SLOTS + total_memory_slots + DGEN_STATE_REDZONE_SLOTS + DGEN_TAIL_SLOTS
}

/// Memory-span index (as init entries address it) of trailer cell `cell`.
pub(in crate::lisp_host) fn dgen_tail_slot(total_memory_slots: usize, cell: usize) -> usize {
    total_memory_slots + DGEN_STATE_REDZONE_SLOTS + cell
}

/// Trailer cell `cell` of a node state. `state[1]` holds the memory span the
/// node was sized for (`dgen_total_state_slots`).
unsafe fn dgen_tail_cell(state: *mut f32, cell: usize) -> *mut f32 {
    let total_memory_slots = (*state.add(1)).max(0.0) as usize;
    dgen_memory_ptr(state).add(dgen_tail_slot(total_memory_slots, cell))
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

/// Quiet frames an opted-in node must see before it sleeps, or `None` when
/// it did not opt in.
///
/// A declared `(effect-tail-sleep <seconds>)` is used as given (the author
/// knows the longest delay their kernel holds), clamped to the quiet-run
/// saturation `tail_sleep::QUIET_FRAMES_CAP` so a long hold at a high rate
/// (60 s at 384 kHz) still sleeps instead of never being reached. Without one the host cannot
/// see the kernel's delay lengths, only its memory span (`state[1]`, every
/// float cell including tensors and FFT buffers). A delay line holding `d`
/// frames needs at least `d` cells, so the span is a conservative upper bound
/// on in-flight frames, not an estimate: tensor-heavy kernels overshoot it by
/// far. It is floored at the 1 s reverb hold and capped at 10 s so such
/// kernels still sleep (and so the hold stays under the quiet-run saturation
/// in `tail_sleep::note_block`); an effect with longer delays should declare
/// its hold.
unsafe fn dgen_tail_hold_frames(state: *mut f32) -> Option<f32> {
    let opt_in = *dgen_tail_cell(state, DGEN_TAIL_OPT_IN);
    let sample_rate = dgen_host_sample_rate(state);
    if opt_in > 0.0 {
        Some((opt_in * sample_rate).min(crate::effects::tail_sleep::QUIET_FRAMES_CAP))
    } else if opt_in < 0.0 {
        let span_frames = (*state.add(1)).max(0.0);
        Some(span_frames.clamp(
            DGEN_TAIL_FALLBACK_MIN_SECONDS * sample_rate,
            DGEN_TAIL_FALLBACK_MAX_SECONDS * sample_rate,
        ))
    } else {
        None // 0 or NaN: not opted in.
    }
}

/// Tail sleep for an opted-in DGen effect inside a graph. Returns `None`
/// when the block was skipped (silence already emitted), else whether the
/// block counts toward the quiet run. Any param or tensor write that changes
/// the node's state restarts the run (audiograph counts only value-changing
/// writes): the kernel's output may change with its state.
unsafe fn dgen_sleep_or_count(state: *mut f32, inp: *const *mut f32, out: *const *mut f32, nframes: c_int) -> Option<bool> {
    let (n_inputs, n_outputs) = crate::effects::silence::node_ports();
    // Outside a graph the port counts are unknown; a zero-input effect is a
    // generator, never a tail. (The graph hands a zero-input node a null
    // input array, which the wrapper already never runs, so this guard is
    // defensive.)
    if n_inputs == 0 || n_outputs == 0 {
        return Some(false);
    }
    let Some(hold) = dgen_tail_hold_frames(state) else {
        return Some(false);
    };
    let writes = f32::from_bits(crate::effects::silence::node_state_writes());
    let seen = dgen_tail_cell(state, DGEN_TAIL_WRITES);
    if (*seen).to_bits() != writes.to_bits() {
        *seen = writes;
        *dgen_tail_cell(state, DGEN_TAIL_QUIET) = 0.0;
    }
    let quiet = dgen_tail_cell(state, DGEN_TAIL_QUIET);
    let input_silent = crate::effects::tail_sleep::ports_silent(inp, n_inputs, nframes.max(0) as usize);
    if crate::effects::tail_sleep::asleep(quiet, input_silent, hold) {
        crate::effects::silence::emit_node_outputs(out, nframes);
        return None;
    }
    Some(input_silent)
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
        let input_count = (*s.add(3)).max(1.0) as usize;
        crate::effects::silence::passthrough(inp, out, input_count.min(2), nframes);
        return;
    }
    if let Some(process_fn) = dgen_process_fn_from_state(s) {
        let memory = dgen_memory_ptr(s) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        let Some(counts_quiet) = dgen_sleep_or_count(s, inp, out, nframes) else {
            return;
        };
        let context = dgen_process_context_v1(dgen_host_sample_rate(s));
        process_fn(
            inp as *const *const f32,
            out,
            nframes.max(0) as u32,
            memory,
            &context,
            dgen_host_services_v1(),
        );
        // A negative `state[0]` is a probe code (see `probe_capture`); an
        // effect without probes skips capture on this plain state read.
        super::probe_capture::record_dgen_effect_probes(*s, process_fn as usize, out, nframes);
        if counts_quiet {
            // Only a silent-input block can extend the run, so only then is
            // the output peak worth reading.
            let nf = nframes.max(0) as usize;
            let (_, n_outputs) = crate::effects::silence::node_ports();
            let peak = (0..n_outputs)
                .map(|port| crate::effects::tail_sleep::peak(&[*out.add(port) as *const f32], nf))
                .fold(0.0, f32::max);
            crate::effects::tail_sleep::note_block(dgen_tail_cell(s, DGEN_TAIL_QUIET), true, peak, nf);
        } else if dgen_tail_hold_frames(s).is_some() {
            *dgen_tail_cell(s, DGEN_TAIL_QUIET) = 0.0;
        }
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

#[cfg(test)]
mod tail_sleep_tests {
    use super::*;
    use crate::audiograph as graph;
    use std::sync::atomic::AtomicU32;

    /// Kernel calls, so a test can tell a skipped block from a computed one.
    static KERNEL_CALLS: AtomicU32 = AtomicU32::new(0);
    /// 0 = declared silence, 1 = 1.0 at frame 0 (undeclared).
    static FEED_MODE: AtomicU32 = AtomicU32::new(0);

    /// A one-pole feedback "tail": y = x + 0.999 y, kept in memory cell 0.
    unsafe extern "C" fn one_pole_process(
        inp: *const *const f32,
        out: *const *mut f32,
        nframes: u32,
        state: *mut c_void,
        _context: *const DGenProcessContextV1,
        _host: *const DGenHostServicesV1,
    ) {
        KERNEL_CALLS.fetch_add(1, Ordering::Relaxed);
        let memory = state as *mut f32;
        for frame in 0..nframes as usize {
            *memory = *(*inp).add(frame) + 0.999 * *memory;
            *(*out).add(frame) = *memory;
        }
    }

    unsafe extern "C" fn feed_process(
        _inp: *const *mut f32,
        out: *const *mut f32,
        nframes: c_int,
        _state: *mut c_void,
        _buffers: *mut c_void,
    ) {
        let lane = std::slice::from_raw_parts_mut(*out, nframes as usize);
        if FEED_MODE.load(Ordering::Relaxed) == 0 {
            crate::effects::silence::emit_port(0, lane);
        } else {
            lane.fill(0.0);
            lane[0] = 1.0;
        }
    }

    const TOTAL_MEMORY_SLOTS: usize = 2;
    /// 1 s hold at 48 kHz in 64-frame blocks.
    const HOLD_BLOCKS: usize = 750;

    struct Rig {
        lg: *mut graph::LiveGraph,
        node: i32,
    }

    impl Rig {
        /// `opt_in` is the `DGEN_TAIL_OPT_IN` cell (0 = not opted in).
        fn new(label: &str, kernel: DGenProcessFn, inputs: i32, opt_in: f32) -> Self {
            graph::initialize_engine_for_test(64, 48_000);
            let label = CString::new(label).unwrap();
            let lg = unsafe { graph::create_live_graph(32, 64, label.as_ptr(), 1) };
            let mut init = vec![0.0f32; 10];
            init[1] = TOTAL_MEMORY_SLOTS as f32;
            init[2] = HEADER_CANARY;
            init[3] = inputs as f32;
            init[4] = 1.0;
            init[5..9].copy_from_slice(&process_fn_pointer_chunks(kernel));
            if opt_in != 0.0 {
                init[9] = 1.0;
                init.extend([dgen_tail_slot(TOTAL_MEMORY_SLOTS, DGEN_TAIL_OPT_IN) as f32, opt_in]);
            }
            let name = CString::new("dgen-node").unwrap();
            let feed_name = CString::new("feed").unwrap();
            let node = unsafe {
                let node = graph::add_node(
                    lg, dgenlisp_vtable(),
                    dgen_total_state_slots(TOTAL_MEMORY_SLOTS) * std::mem::size_of::<f32>(),
                    name.as_ptr(), inputs, 1, init.as_ptr().cast(), init.len() * std::mem::size_of::<f32>(),
                );
                assert!(node > 0);
                if inputs > 0 {
                    let feed = graph::add_node(
                        lg,
                        graph::NodeVTable { process: Some(feed_process), ..graph::NodeVTable::default() },
                        4, feed_name.as_ptr(), 0, 1, std::ptr::null(), 0,
                    );
                    assert!(feed > 0 && graph::graph_connect(lg, feed, 0, node, 0));
                }
                assert!(graph::graph_connect(lg, node, 0, 0, 0));
                node
            };
            Rig { lg, node }
        }

        /// One block; returns the output and whether the kernel ran.
        fn render(&self, mode: u32) -> (Vec<f32>, bool) {
            FEED_MODE.store(mode, Ordering::Relaxed);
            let before = KERNEL_CALLS.load(Ordering::Relaxed);
            let mut output = vec![f32::NAN; 64];
            unsafe { graph::process_next_block(self.lg, output.as_mut_ptr(), 64) };
            (output, KERNEL_CALLS.load(Ordering::Relaxed) != before)
        }

        fn write_param(&self, memory_cell: usize, value: f32) {
            unsafe {
                assert!(graph::params_push_wrapper(self.lg, graph::ParamMsg {
                    idx: (HEADER_SLOTS + memory_cell) as u64, logical_id: self.node as u64, fvalue: value,
                }));
            }
        }

        /// Silent blocks until the kernel stops running; `None` if it never does.
        fn blocks_until_sleep(&self, limit: usize) -> Option<usize> {
            (0..limit).find(|_| {
                let (output, ran) = self.render(0);
                if !ran {
                    assert!(output.iter().all(|v| *v == 0.0), "a sleeping effect outputs exact zeros");
                }
                !ran
            })
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            unsafe { graph::destroy_live_graph(self.lg) };
        }
    }

    #[test]
    fn opted_in_dgen_effect_sleeps_after_its_tail_and_wakes_on_input() {
        let rig = Rig::new("dgen-tail-sleep", one_pole_process, 1, DGEN_TAIL_HOLD_FALLBACK);
        // An impulse rings out as 0.999^n; the kernel keeps rendering that
        // tail until it has sat below -120 dB for the hold (a 2-cell span
        // falls back to the 1 s floor).
        let (output, ran) = rig.render(1);
        assert!(ran && output[0] == 1.0);
        let mut expected = output[63];
        let mut blocks = None;
        for block in 0..2000 {
            let (output, ran) = rig.render(0);
            if !ran {
                assert!(output.iter().all(|v| *v == 0.0), "a sleeping effect outputs exact zeros");
                blocks = Some(block);
                break;
            }
            for v in output {
                expected *= 0.999;
                assert!((v - expected).abs() <= 1e-6, "awake tail must render unchanged");
            }
        }
        let blocks = blocks.expect("the decayed tail never slept");
        // 0.999^n reaches 1e-6 after ~13.8k frames, then the 48k-frame hold.
        assert!(blocks * 64 >= 13_000 + 48_000, "slept after only {blocks} blocks");
        // Input wakes it in the same block with its state untouched.
        let (output, ran) = rig.render(1);
        assert!(ran && (output[0] - 1.0).abs() <= 1e-6, "wake block: {}", output[0]);
        assert!(rig.render(0).1, "the fresh tail must render");
    }

    #[test]
    fn a_param_write_wakes_a_sleeping_dgen_effect_and_restarts_its_hold() {
        let rig = Rig::new("dgen-param-wake", one_pole_process, 1, DGEN_TAIL_HOLD_FALLBACK);
        assert!(rig.render(1).1);
        assert!(rig.blocks_until_sleep(2000).is_some());
        assert!(!rig.render(0).1);
        // Memory cell 1 is unused by the kernel, but the host cannot know
        // that: any write may change what the kernel renders.
        rig.write_param(1, 0.5);
        assert!(rig.render(0).1, "a param write must wake the effect");
        let again = rig.blocks_until_sleep(2000).expect("it sleeps again");
        assert!(again + 1 >= HOLD_BLOCKS, "the hold restarted: slept after {again} blocks");
    }

    #[test]
    fn unchanged_param_and_tensor_writes_leave_a_sleeping_dgen_effect_asleep() {
        let rig = Rig::new("dgen-unchanged-write", one_pole_process, 1, DGEN_TAIL_HOLD_FALLBACK);
        assert!(rig.render(1).1);
        assert!(rig.blocks_until_sleep(2000).is_some());
        // A host re-pushing the value a cell already holds changes nothing
        // the kernel could render, so it must not restart the hold.
        rig.write_param(1, 0.0);
        assert!(!rig.render(0).1, "an unchanged param write woke the effect");
        unsafe { assert!(queue_tensor_write(rig.lg, rig.node, 1, &[0.0])) };
        assert!(!rig.render(0).1, "an unchanged tensor write woke the effect");
        unsafe { assert!(queue_tensor_write(rig.lg, rig.node, 1, &[0.25])) };
        assert!(rig.render(0).1, "a changed tensor write must wake the effect");
    }

    #[test]
    fn a_declared_hold_replaces_the_memory_span_fallback() {
        // `(effect-tail-sleep 0.25)`: a quarter-second hold after the decay.
        let rig = Rig::new("dgen-declared-hold", one_pole_process, 1, 0.25);
        assert!(rig.render(1).1);
        let blocks = rig.blocks_until_sleep(2000).expect("it sleeps");
        // ~13.8k frames of decay, then 12k frames of hold.
        assert!(blocks * 64 >= 13_000 + 12_000 && blocks * 64 < 13_000 + 48_000, "slept after {blocks} blocks");
    }

    #[test]
    fn the_undeclared_hold_is_the_memory_span_floored_at_1s_and_capped_at_10s() {
        for (span, expected_seconds) in [(2usize, 1.0f32), (96_000, 2.0), (4_000_000, 10.0)] {
            let mut state = vec![0.0f32; dgen_total_state_slots(span)];
            state[1] = span as f32;
            state[DGEN_HOST_SAMPLE_RATE_IDX] = 48_000.0;
            let state = state.as_mut_ptr();
            unsafe {
                assert_eq!(dgen_tail_hold_frames(state), None);
                *dgen_tail_cell(state, DGEN_TAIL_OPT_IN) = DGEN_TAIL_HOLD_FALLBACK;
                assert_eq!(dgen_tail_hold_frames(state), Some(expected_seconds * 48_000.0), "span {span}");
                *dgen_tail_cell(state, DGEN_TAIL_OPT_IN) = 3.0;
                assert_eq!(dgen_tail_hold_frames(state), Some(3.0 * 48_000.0));
            }
        }
    }

    #[test]
    fn a_declared_hold_is_clamped_to_the_quiet_run_saturation() {
        use crate::effects::tail_sleep::{asleep, note_block, QUIET_FRAMES_CAP};
        let span = 2usize;
        let mut state = vec![0.0f32; dgen_total_state_slots(span)];
        state[1] = span as f32;
        state[DGEN_HOST_SAMPLE_RATE_IDX] = 384_000.0;
        let state = state.as_mut_ptr();
        unsafe {
            *dgen_tail_cell(state, DGEN_TAIL_OPT_IN) = super::super::effect_latency::MAX_TAIL_SLEEP_SECONDS as f32;
            let hold = dgen_tail_hold_frames(state).unwrap();
            assert_eq!(hold, QUIET_FRAMES_CAP, "60 s at 384 kHz is past the cap");
            // A quiet run that saturates reaches the clamped hold.
            let mut quiet = QUIET_FRAMES_CAP - 64.0;
            note_block(&mut quiet, true, 0.0, 4096);
            assert!(asleep(&quiet, true, hold));
        }
    }

    #[test]
    fn dgen_effects_that_did_not_opt_in_never_sleep() {
        let rig = Rig::new("dgen-no-opt-in", one_pole_process, 1, 0.0);
        assert!(rig.render(1).1);
        assert_eq!(rig.blocks_until_sleep(3 * HOLD_BLOCKS), None);
    }

    #[test]
    fn dgen_bypass_passes_declared_silence_without_running_the_kernel() {
        let rig = Rig::new("dgen-bypass", one_pole_process, 1, 0.0);
        unsafe {
            assert!(graph::params_push_wrapper(rig.lg, graph::ParamMsg {
                idx: DGEN_ENABLED_PARAM_IDX as u64, logical_id: rig.node as u64, fvalue: 0.0,
            }));
        }
        let (output, ran) = rig.render(1);
        assert!(!ran && output[0] == 1.0 && output[1..].iter().all(|v| *v == 0.0));
        let (output, ran) = rig.render(0);
        assert!(!ran && output.iter().all(|v| *v == 0.0));
    }
}
