//! Skip rewriting output lanes that already hold what a kernel would write.
//!
//! An edge buffer is written only by its source node, and the engine bumps
//! the node's IO generation (`ap_current_node_io_generation`) whenever its
//! buffers are rebound. So while the generation is unchanged, a lane that a
//! previous call filled with one constant still holds it, and filling it
//! again is pure memory traffic: gatepitch's gate/pitch/velocity between
//! notes, a modulator's zeroed idle slots. Per voice and per block that
//! traffic was a large share of small graphs' cost (eseq-v6te).
//!
//! State layout, in the owning node's `f32` state: one generation cell (raw
//! `u32` bits) followed by `[value, covered_frames]` per lane.

extern "C" {
    fn ap_current_node_io_generation() -> u32;
}

pub const fn state_cells(lanes: usize) -> usize {
    1 + lanes * 2
}

/// Per-call view of one node's lane cache.
pub struct OutputLanes {
    cells: *mut f32,
    /// False outside a graph kernel or after a rebind: every fill writes.
    trusted: bool,
}

impl OutputLanes {
    /// # Safety
    /// `cells` must point at `state_cells(lanes)` cells of the running
    /// node's own state.
    pub unsafe fn begin(cells: *mut f32) -> Self {
        let generation = ap_current_node_io_generation();
        let trusted = generation != 0 && (*cells).to_bits() == generation;
        if !trusted {
            *cells = f32::from_bits(generation);
        }
        Self { cells, trusted }
    }

    /// Fill `lane` (frames `0..lane.len()`) with `value`, unless the buffer
    /// already holds exactly that over at least as many frames.
    ///
    /// # Safety
    /// `index` must be within the lane count the cells were sized for.
    pub unsafe fn fill(&self, index: usize, lane: &mut [f32], value: f32) {
        let cached = self.cells.add(1 + index * 2);
        let covered = self.cells.add(2 + index * 2);
        if self.trusted
            && (*cached).to_bits() == value.to_bits()
            && *covered >= lane.len() as f32
        {
            return;
        }
        lane.fill(value);
        *cached = value;
        *covered = lane.len() as f32;
    }

    /// Record that the caller wrote arbitrary data into lane `index`.
    ///
    /// # Safety
    /// As for [`fill`](Self::fill).
    pub unsafe fn written(&self, index: usize) {
        *self.cells.add(2 + index * 2) = 0.0;
    }
}
