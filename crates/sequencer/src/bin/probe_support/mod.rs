//! Headless-graph scaffolding shared by the probe binaries.

use sequencer::audiograph;

const BLOCK_SIZE: usize = 512;

/// Render `blocks` blocks so queued graph edits apply.
pub fn pump_graph(lg: audiograph::LiveGraphPtr, blocks: usize) {
    let mut output = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..blocks {
        unsafe {
            lg.process_next_block(output.as_mut_ptr(), BLOCK_SIZE as i32);
        }
    }
}

/// Stops the engine workers and frees the graph on drop.
pub struct GraphGuard {
    pub lg: *mut audiograph::LiveGraph,
}

impl Drop for GraphGuard {
    fn drop(&mut self) {
        unsafe {
            audiograph::clear_os_workgroup();
            audiograph::engine_stop_workers();
            audiograph::destroy_live_graph(self.lg);
        }
    }
}
