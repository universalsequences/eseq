//! Node output meters (graph_node_meter_*): the audio thread peak-scans a
//! metered node's outputs after `process`, the reader takes and resets.

use std::os::raw::{c_int, c_void};

use crate::audiograph;

unsafe extern "C" fn constant_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    _state: *mut c_void,
    _buffers: *mut c_void,
) {
    for (ch, value) in [(0usize, 0.5f32), (1, -0.25)] {
        std::slice::from_raw_parts_mut(*out.add(ch), nframes as usize).fill(value);
    }
}

struct Graph(*mut audiograph::LiveGraph);

impl Drop for Graph {
    fn drop(&mut self) {
        unsafe { audiograph::destroy_live_graph(self.0) };
    }
}

const BLOCK: usize = 64;

fn graph_with_source() -> (Graph, i32) {
    audiograph::initialize_engine_for_test(BLOCK as c_int, 44_100);
    let label = std::ffi::CString::new("node-meter").unwrap();
    let graph = Graph(unsafe { audiograph::create_live_graph(32, BLOCK as c_int, label.as_ptr(), 2) });
    assert!(!graph.0.is_null());
    let name = std::ffi::CString::new("constant").unwrap();
    let vtable = audiograph::NodeVTable {
        process: Some(constant_process),
        ..audiograph::NodeVTable::default()
    };
    let node = unsafe {
        audiograph::add_node(graph.0, vtable, 0, name.as_ptr(), 0, 2, std::ptr::null(), 0)
    };
    assert!(node > 0);
    unsafe {
        assert!(audiograph::graph_connect(graph.0, node, 0, 0, 0));
        assert!(audiograph::graph_connect(graph.0, node, 1, 0, 1));
    }
    (graph, node)
}

fn render(graph: &Graph, blocks: usize) {
    let mut output = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        unsafe { audiograph::process_next_block(graph.0, output.as_mut_ptr(), BLOCK as c_int) };
    }
}

fn take(graph: &Graph, slot: i32) -> (f32, f32, u32) {
    let (mut l, mut r, mut blocks) = (0.0, 0.0, 0);
    assert!(unsafe { audiograph::graph_node_meter_take(graph.0, slot, &mut l, &mut r, &mut blocks) });
    (l, r, blocks)
}

#[test]
fn metered_node_reports_stereo_output_peaks_and_take_resets() {
    let (graph, node) = graph_with_source();
    let slot = unsafe { audiograph::graph_node_meter_attach(graph.0, node) };
    assert!(slot >= 0);
    render(&graph, 3);
    let (l, r, blocks) = take(&graph, slot);
    assert_eq!((l, r), (0.5, 0.25));
    assert_eq!(blocks, 3);
    let (l, r, _) = take(&graph, slot);
    assert_eq!((l, r), (0.0, 0.0), "take hands over and resets the peaks");
    render(&graph, 1);
    assert_eq!(take(&graph, slot).0, 0.5);
}

#[test]
fn detached_slot_stops_metering_and_is_reusable() {
    let (graph, node) = graph_with_source();
    let slot = unsafe { audiograph::graph_node_meter_attach(graph.0, node) };
    render(&graph, 1);
    assert!(unsafe { audiograph::graph_node_meter_detach(graph.0, slot) });
    let (_, _, blocks_at_detach) = take(&graph, slot);
    render(&graph, 2);
    assert_eq!(take(&graph, slot), (0.0, 0.0, blocks_at_detach), "detached node is no longer scanned");
    let again = unsafe { audiograph::graph_node_meter_attach(graph.0, node) };
    assert_eq!(again, slot, "the first free slot is reclaimed");
    let (l, _, blocks) = take(&graph, again);
    assert_eq!((l, blocks), (0.0, 0), "a reclaimed slot starts clean");
    render(&graph, 2);
    assert_eq!(take(&graph, again), (0.5, 0.25, 2));
}

#[test]
fn unmetered_nodes_never_touch_the_table() {
    let (graph, _node) = graph_with_source();
    render(&graph, 2);
    for slot in [0, 1, 255] {
        assert_eq!(take(&graph, slot), (0.0, 0.0, 0));
    }
    assert!(!unsafe { audiograph::graph_node_meter_reattach(graph.0, 0) });
}
