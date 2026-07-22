use super::*;

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
pub(super) static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) static MIDI_FX_DESCRIPTOR_CACHE: OnceLock<Mutex<HashMap<String, Vec<EffectDescriptor>>>> =
    OnceLock::new();

pub(super) fn read_eseqlisp_init_source() -> String {
    eseqlisp_init_candidates()
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

pub(super) fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    crate::paths::eseqlisp_init_candidates()
}

// ── dlopen FFI (macOS) ──

extern "C" {
    pub(super) fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub(super) fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub(super) fn dlerror() -> *const c_char;
}

pub(super) const RTLD_NOW: c_int = 2;

pub(super) type DGenProcessFn = unsafe extern "C" fn(
    inputs: *const *mut f32,
    outputs: *const *mut f32,
    frame_count: c_int,
    memory_read: *mut c_void,
    memory_write: *mut c_void,
    host_sample_rate: c_float,
);

pub const MAX_CUSTOM_FX: usize = 8;
pub const MAX_MIDI_FX_SLOTS: usize = 4;
pub const MAX_BUS_FX_CHAINS: usize = 64;

// ── Node state layout ──
// state[0] = host-local slot identity (diagnostics only)
// state[1] = total_memory_slots (f32)
// state[2] = canary
// state[3] = declared input count (f32)
// state[4] = enabled (0 = bypass/silent, 1 = active)
// state[5] = host sample rate
// state[6..10] = immutable process function pointer (four numeric u16 chunks)
// state[10..10+N] = DGenLisp read buffer
// state[...]     = DGenLisp write buffer (separate to respect `restrict`)

pub const DGEN_ENABLED_PARAM_IDX: usize = 4;
pub const DGEN_HOST_SAMPLE_RATE_IDX: usize = 5;
pub(super) const DGEN_PROCESS_FN_START_IDX: usize = 6;
pub(super) const DGEN_PROCESS_FN_CHUNKS: usize = 4;
pub const HEADER_SLOTS: usize = 10;
pub const DGEN_STATE_REDZONE_SLOTS: usize = 256;
pub(super) const HEADER_CANARY: f32 = f32::from_bits(0x4cd35a1d);

pub(super) fn ensure_enabled_param(params: &mut Vec<crate::effects::ParamDescriptor>) {
    if params
        .iter()
        .any(|param| param.name.eq_ignore_ascii_case("enabled"))
    {
        return;
    }
    params.push(EffectDescriptor::enabled_param(params.len() as u32, 1.0));
}

pub fn dgen_buffer_span_slots(total_memory_slots: usize) -> usize {
    total_memory_slots + DGEN_STATE_REDZONE_SLOTS
}

pub fn dgen_total_state_slots(total_memory_slots: usize) -> usize {
    HEADER_SLOTS + dgen_buffer_span_slots(total_memory_slots) * 2
}

pub(super) unsafe fn dgen_read_buffer_ptr(state: *mut f32) -> *mut f32 {
    state.add(HEADER_SLOTS)
}

pub(super) unsafe fn dgen_write_buffer_ptr(state: *mut f32, total_memory_slots: usize) -> *mut f32 {
    state.add(HEADER_SLOTS + dgen_buffer_span_slots(total_memory_slots))
}

pub(super) unsafe fn dgen_host_sample_rate(state: *mut f32) -> f32 {
    let sample_rate = *state.add(DGEN_HOST_SAMPLE_RATE_IDX);
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        44_100.0
    }
}

pub(super) fn process_fn_pointer_chunks(process_fn: DGenProcessFn) -> [f32; DGEN_PROCESS_FN_CHUNKS] {
    let pointer = process_fn as usize as u64;
    std::array::from_fn(|chunk| ((pointer >> (chunk * 16)) & 0xffff) as f32)
}

pub(super) unsafe fn dgen_process_fn_from_state(state: *mut f32) -> Option<DGenProcessFn> {
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

pub(super) unsafe extern "C" fn dgenlisp_wrapper_process(
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
        let _total_memory_slots = *s.add(1) as usize;
        let memory_read = dgen_read_buffer_ptr(s) as *mut c_void;
        let memory_write = dgen_write_buffer_ptr(s, _total_memory_slots) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        process_fn(
            inp,
            out,
            nframes,
            memory_read,
            memory_write,
            dgen_host_sample_rate(s),
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
pub(super) unsafe extern "C" fn dgenlisp_init(
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
    let total_memory_slots = *dst.add(1) as usize;
    let mem = dgen_read_buffer_ptr(dst);
    for i in 0..num_entries {
        let idx = (*src.add(10 + i * 2)) as usize;
        let val = *src.add(10 + i * 2 + 1);
        *mem.add(idx) = val;
    }
    let write_mem = dgen_write_buffer_ptr(dst, total_memory_slots);
    std::ptr::copy_nonoverlapping(mem as *const f32, write_mem, total_memory_slots);
}

/// Queue a bulk write of `data` into a live dgenlisp effect node's state at the
/// given tensor `cell_offset` (from the manifest's `tensors[]`). The write lands
/// in the read-state buffer (`HEADER_SLOTS + cell_offset`) — the same region
/// params are written to and the buffer the DSP reads constant inputs from. The
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

pub(super) fn dgenlisp_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}
