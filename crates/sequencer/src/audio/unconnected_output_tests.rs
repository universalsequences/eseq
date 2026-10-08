//! Unconnected output ports (eseq-d1xr.8): each one gets a private discard
//! buffer, so a host can read an unrouted channel back after `process` (DGen
//! probes, `@amp`) without seeing a sibling port's or another node's data.

use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::audiograph;

const BLOCK: usize = 64;

struct Graph(*mut audiograph::LiveGraph);

impl Drop for Graph {
    fn drop(&mut self) {
        unsafe { audiograph::destroy_live_graph(self.0) };
    }
}

/// Node state: its identity.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TagState {
    tag: f32,
}

unsafe extern "C" fn tag_init(state: *mut c_void, _sr: c_int, _bs: c_int, initial: *const c_void) {
    if !initial.is_null() {
        *(state as *mut TagState) = *(initial as *const TagState);
    }
}

fn new_graph(label: &str) -> Graph {
    audiograph::initialize_engine_for_test(BLOCK as c_int, 44_100);
    let label = std::ffi::CString::new(label).unwrap();
    let graph = Graph(unsafe { audiograph::create_live_graph(32, BLOCK as c_int, label.as_ptr(), 2) });
    assert!(!graph.0.is_null());
    graph
}

fn add_tag_node(graph: &Graph, process: audiograph::NodeVTable, tag: f32, outputs: c_int) -> i32 {
    let name = std::ffi::CString::new(format!("tag{tag}")).unwrap();
    let initial = TagState { tag };
    let node = unsafe {
        audiograph::add_node(
            graph.0,
            process,
            std::mem::size_of::<TagState>(),
            name.as_ptr(),
            0,
            outputs,
            &initial as *const TagState as *const c_void,
            std::mem::size_of::<TagState>(),
        )
    };
    assert!(node > 0);
    node
}

fn render(graph: &Graph, blocks: usize) {
    let mut output = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        unsafe { audiograph::process_next_block(graph.0, output.as_mut_ptr(), BLOCK as c_int) };
    }
}

static READ_BACK: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Port 0 is routed; ports 1 and 2 are not. Writes every port, then reads the
/// unconnected ones back the way the DGen wrappers do.
unsafe extern "C" fn two_unconnected_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    _state: *mut c_void,
    _buffers: *mut c_void,
) {
    let n = nframes as usize;
    for (ch, value) in [(0usize, 0.1f32), (1, 0.75), (2, 0.25)] {
        std::slice::from_raw_parts_mut(*out.add(ch), n).fill(value);
    }
    for (slot, ch) in READ_BACK.iter().zip([1usize, 2]) {
        slot.store((*(*out.add(ch)).add(n - 1)).to_bits(), Ordering::Relaxed);
    }
}

#[test]
fn unconnected_outputs_of_one_node_keep_distinct_data() {
    let graph = new_graph("unconnected-distinct");
    let vtable = audiograph::NodeVTable {
        process: Some(two_unconnected_process),
        init: Some(tag_init),
        ..audiograph::NodeVTable::default()
    };
    let node = add_tag_node(&graph, vtable, 1.0, 3);
    unsafe { assert!(audiograph::graph_connect(graph.0, node, 0, 0, 0)) };
    render(&graph, 2);

    let read_back = READ_BACK.each_ref().map(|slot| f32::from_bits(slot.load(Ordering::Relaxed)));
    assert_eq!(read_back, [0.75, 0.25], "each unconnected port keeps its own channel");
}

static MISREADS: AtomicUsize = AtomicUsize::new(0);
static PROCESSED: AtomicU32 = AtomicU32::new(0);

/// Port 0 feeds the DAC (so the node is scheduled); port 1 is unconnected.
/// Writes its tag to port 1, gives other workers time to run, and checks the
/// tag is still there when it reads the port back.
unsafe extern "C" fn racing_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let tag = (*(state as *const TagState)).tag;
    let n = nframes as usize;
    std::slice::from_raw_parts_mut(*out.add(0), n).fill(0.0);
    let unconnected = std::slice::from_raw_parts_mut(*out.add(1), n);
    unconnected.fill(tag);
    for _ in 0..200 {
        std::hint::spin_loop();
    }
    std::thread::yield_now();
    if unconnected.iter().any(|&v| v != tag) {
        MISREADS.fetch_add(1, Ordering::Relaxed);
    }
    PROCESSED.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn unconnected_outputs_are_private_under_multi_worker_rendering() {
    let graph = new_graph("unconnected-workers");
    let vtable = audiograph::NodeVTable {
        process: Some(racing_process),
        init: Some(tag_init),
        ..audiograph::NodeVTable::default()
    };
    // choose_active_worker_count wakes one worker per 24 jobs.
    const NODES: usize = 96;
    for i in 0..NODES {
        let node = add_tag_node(&graph, vtable, i as f32 + 1.0, 2);
        unsafe { assert!(audiograph::graph_connect(graph.0, node, 0, 0, (i % 2) as c_int)) };
    }
    unsafe { audiograph::engine_start_workers(4) };
    struct StopWorkers;
    impl Drop for StopWorkers {
        fn drop(&mut self) {
            unsafe { audiograph::engine_stop_workers() };
        }
    }
    let _stop = StopWorkers;

    render(&graph, 50);
    assert!(
        PROCESSED.load(Ordering::Relaxed) >= (NODES * 40) as u32,
        "the racing nodes actually ran"
    );
    assert_eq!(
        MISREADS.load(Ordering::Relaxed),
        0,
        "no node saw another node's write in its unconnected output"
    );
}

extern "C" {
    fn ap_debug_node_null_outputs(
        lg: *mut audiograph::LiveGraph,
        node_id: c_int,
        capacity: *mut c_int,
    ) -> *const f32;
    fn hot_swap_node(
        lg: *mut audiograph::LiveGraph,
        node_id: c_int,
        vt: audiograph::NodeVTable,
        state_size: usize,
        nin: c_int,
        nout: c_int,
        xfade: bool,
        migrate: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        initial_state: *const c_void,
        initial_state_size: usize,
    ) -> bool;
}

/// Floats per discard run: the graph block plus EDGE_BUFFER_PAD_FLOATS.
const NULL_STRIDE: usize = BLOCK + 64;

fn null_outputs(graph: &Graph, node: i32) -> (usize, c_int) {
    let mut capacity = 0;
    let ptr = unsafe { ap_debug_node_null_outputs(graph.0, node, &mut capacity) };
    (ptr as usize, capacity)
}

static OUT_PTRS: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

/// Records the output pointers it was handed, so a test can check where each
/// unconnected port landed.
unsafe extern "C" fn record_out_ptrs_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    _state: *mut c_void,
    _buffers: *mut c_void,
) {
    let n = nframes as usize;
    std::slice::from_raw_parts_mut(*out.add(0), n).fill(0.0);
    for (ch, slot) in OUT_PTRS.iter().enumerate() {
        slot.store(*out.add(ch) as usize, Ordering::Relaxed);
    }
}

fn recorded_out_ptrs() -> [usize; 3] {
    OUT_PTRS.each_ref().map(|slot| slot.load(Ordering::Relaxed))
}

fn record_vtable() -> audiograph::NodeVTable {
    audiograph::NodeVTable {
        process: Some(record_out_ptrs_process),
        init: Some(tag_init),
        ..audiograph::NodeVTable::default()
    }
}

fn run_addr(base: usize, run: usize) -> usize {
    base + run * NULL_STRIDE * std::mem::size_of::<f32>()
}

#[test]
fn unconnected_output_buffers_are_reserved_by_the_edit_that_adds_the_node() {
    let graph = new_graph("unconnected-reserved-on-add");
    let node = add_tag_node(&graph, record_vtable(), 1.0, 3);
    // Apply the add edit; nothing has rendered and nothing is connected.
    unsafe { assert!(audiograph::prepare_graph_for_render(graph.0)) };
    let (base, capacity) = null_outputs(&graph, node);
    assert_ne!(base, 0, "the add edit reserved discard buffers");
    assert_eq!(capacity, 3, "one discard run per unconnected port");

    // Routing port 0 leaves two unconnected ports, which take the first two
    // runs in port order. Rendering and connect/disconnect churn only repoint
    // the cached outputs into the reservation; nothing reallocates it.
    unsafe { assert!(audiograph::graph_connect(graph.0, node, 0, 0, 0)) };
    render(&graph, 2);
    let ptrs = recorded_out_ptrs();
    assert!(ptrs[0] < base || ptrs[0] >= run_addr(base, 3), "the routed port writes its edge buffer");
    assert_eq!([ptrs[1], ptrs[2]], [run_addr(base, 0), run_addr(base, 1)]);

    unsafe { assert!(audiograph::graph_disconnect(graph.0, node, 0, 0, 0)) };
    unsafe { assert!(audiograph::graph_connect(graph.0, node, 0, 0, 1)) };
    render(&graph, 2);
    assert_eq!(null_outputs(&graph, node), (base, 3), "rebuilds reuse the reservation");
    let ptrs = recorded_out_ptrs();
    assert_eq!([ptrs[1], ptrs[2]], [run_addr(base, 0), run_addr(base, 1)]);
}

#[test]
fn hot_swap_reserves_discard_buffers_for_added_ports_before_render() {
    let graph = new_graph("unconnected-reserved-on-hot-swap");
    let node = add_tag_node(&graph, record_vtable(), 1.0, 1);
    unsafe { assert!(audiograph::graph_connect(graph.0, node, 0, 0, 0)) };
    render(&graph, 1);

    let initial = TagState { tag: 2.0 };
    let swapped = unsafe {
        hot_swap_node(
            graph.0,
            node,
            record_vtable(),
            std::mem::size_of::<TagState>(),
            0,
            3,
            false,
            None,
            &initial as *const TagState as *const c_void,
            std::mem::size_of::<TagState>(),
        )
    };
    assert!(swapped);
    // A hot swap leaves the topology clean, so applying it runs no IO-cache
    // rebuild (that one runs lazily at process): the reservation must come
    // from the edit itself.
    unsafe { assert!(audiograph::prepare_graph_for_render(graph.0)) };
    let (base, capacity) = null_outputs(&graph, node);
    assert!(capacity >= 2, "hot swap reserved its added ports in the edit phase, got {capacity}");

    render(&graph, 1);
    let ptrs = recorded_out_ptrs();
    assert_eq!([ptrs[1], ptrs[2]], [run_addr(base, 0), run_addr(base, 1)]);
    assert_eq!(null_outputs(&graph, node), (base, capacity), "the lazy rebuild did not reallocate");
}
