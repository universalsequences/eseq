use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering};

/// Mirrors C `NodeVTable` — function pointers for a DSP node.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct NodeVTable {
    pub process: Option<
        unsafe extern "C" fn(
            inp: *const *mut f32,
            out: *const *mut f32,
            nframes: c_int,
            state: *mut c_void,
            buffers: *mut c_void,
        ),
    >,
    pub init: Option<
        unsafe extern "C" fn(
            state: *mut c_void,
            sample_rate: c_int,
            max_block: c_int,
            initial_state: *const c_void,
        ),
    >,
    pub reset: Option<unsafe extern "C" fn(state: *mut c_void)>,
    pub migrate: Option<unsafe extern "C" fn(new_state: *mut c_void, old_state: *const c_void)>,
}

/// Mirrors C `ParamMsg`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ParamMsg {
    pub idx: u64,
    pub logical_id: u64,
    pub fvalue: f32,
}

/// Mirrors C `BufferDesc`.
#[repr(C)]
pub struct BufferDesc {
    pub buffer: *mut f32,
    pub size: c_int,
    pub channel_count: c_int,
}

/// Opaque handle — we only ever hold `*mut LiveGraph`.
#[repr(C)]
pub struct LiveGraph {
    _opaque: [u8; 0],
}

unsafe impl Send for LiveGraphPtr {}
unsafe impl Sync for LiveGraphPtr {}

/// Wrapper so we can send `*mut LiveGraph` across threads.
#[derive(Copy, Clone)]
pub struct LiveGraphPtr(pub *mut LiveGraph);

impl LiveGraphPtr {
    pub unsafe fn process_next_block(self, output_buffer: *mut f32, nframes: c_int) {
        process_next_block(self.0, output_buffer, nframes);
    }
}

extern "C" {
    // Engine lifecycle
    pub fn initialize_engine(block_size: c_int, sample_rate: c_int);
    pub fn engine_start_workers(workers: c_int);
    pub fn engine_stop_workers();
    #[allow(dead_code)]
    pub fn engine_set_os_workgroup(oswg: *mut c_void);
    pub fn engine_clear_os_workgroup();
    pub fn engine_enable_rt_logging(enable: c_int);
    pub fn engine_enable_graph_logging(enable: c_int);
    pub fn engine_enable_rt_time_constraint(enable: c_int);

    // Graph lifecycle
    pub fn create_live_graph(
        initial_capacity: c_int,
        block_size: c_int,
        label: *const c_char,
        num_channels: c_int,
    ) -> *mut LiveGraph;
    pub fn destroy_live_graph(lg: *mut LiveGraph);

    // Queue-based node API
    pub fn add_node(
        lg: *mut LiveGraph,
        vtable: NodeVTable,
        state_size: usize,
        name: *const c_char,
        n_inputs: c_int,
        n_outputs: c_int,
        initial_state: *const c_void,
        initial_state_size: usize,
    ) -> c_int;
    pub fn add_gain_node(lg: *mut LiveGraph, gain_value: f32, name: *const c_char) -> c_int;

    // Connections
    pub fn graph_connect(
        lg: *mut LiveGraph,
        src_node: c_int,
        src_port: c_int,
        dst_node: c_int,
        dst_port: c_int,
    ) -> bool;
    pub fn begin_graph_edit_batch(lg: *mut LiveGraph);
    pub fn end_graph_edit_batch(lg: *mut LiveGraph);

    // Buffer management
    pub fn create_buffer(
        lg: *mut LiveGraph,
        size: c_int,
        channel_count: c_int,
        source_data: *const f32,
    ) -> c_int;

    // Bulk-write a block of floats into a node's state memory at dest_offset
    // (in floats). Queued and applied on the audio thread at a block boundary;
    // source_data is copied internally, so the caller may free it immediately.
    pub fn write_node_state(
        lg: *mut LiveGraph,
        node_id: c_int,
        dest_offset: usize,
        source_data: *const f32,
        count: usize,
    ) -> bool;

    // Built-in node factories
    pub fn live_add_gain(lg: *mut LiveGraph, gain_value: f32, name: *const c_char) -> c_int;

    // Audio processing
    pub fn process_next_block(lg: *mut LiveGraph, output_buffer: *mut f32, nframes: c_int);
    pub fn add_node_to_watchlist(lg: *mut LiveGraph, node_id: c_int) -> bool;
    pub fn remove_node_from_watchlist(lg: *mut LiveGraph, node_id: c_int) -> bool;
    pub fn get_node_state(
        lg: *mut LiveGraph,
        node_id: c_int,
        state_size: *mut usize,
    ) -> *mut c_void;
    pub fn get_node_state_into(
        lg: *mut LiveGraph,
        node_id: c_int,
        out: *mut c_void,
        out_capacity: usize,
        state_size: *mut usize,
    ) -> bool;

    // Wrapper for the static-inline params_push
    #[link_name = "params_push_wrapper"]
    fn params_push_wrapper_raw(lg: *mut LiveGraph, m: ParamMsg) -> bool;

    // Disconnect
    pub fn graph_disconnect(
        lg: *mut LiveGraph,
        src_node: c_int,
        src_port: c_int,
        dst_node: c_int,
        dst_port: c_int,
    ) -> bool;

    // Delete
    pub fn delete_node(lg: *mut LiveGraph, node_id: c_int) -> bool;
    fn free(ptr: *mut c_void);
}

static PARAM_TRACE_COUNT: AtomicU64 = AtomicU64::new(0);

fn param_trace_enabled() -> bool {
    std::env::var("ESEQ_AUDIOGRAPH_PARAM_TRACE")
        .map(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0"
        })
        .unwrap_or(false)
}

pub unsafe fn params_push_wrapper(lg: *mut LiveGraph, m: ParamMsg) -> bool {
    if param_trace_enabled() {
        let count = PARAM_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!(
            "[audiograph-param] push={count} logical={} idx={} value={:.9}",
            m.logical_id, m.idx, m.fvalue
        );
    }
    params_push_wrapper_raw(lg, m)
}

#[allow(dead_code)]
pub unsafe fn set_os_workgroup(oswg: *mut c_void) {
    engine_set_os_workgroup(oswg);
}

pub unsafe fn clear_os_workgroup() {
    engine_clear_os_workgroup();
}

pub unsafe fn free_c_ptr(ptr: *mut c_void) {
    free(ptr);
}

pub unsafe fn enable_rt_logging(enable: bool) {
    engine_enable_rt_logging(if enable { 1 } else { 0 });
}

pub unsafe fn enable_graph_logging(enable: bool) {
    engine_enable_graph_logging(if enable { 1 } else { 0 });
}

pub unsafe fn enable_rt_time_constraint(enable: bool) {
    engine_enable_rt_time_constraint(if enable { 1 } else { 0 });
}
