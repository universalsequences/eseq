//! Per-event work counters for interpolated widget drags.
//!
//! The counters are active only while `Editor::try_handle_widget_drag_segment`
//! is running. They make pointer-distance amplification and deep layout cloning
//! visible without coupling callers to a particular renderer or widget type.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DragPathStats {
    pub interpolation_subsamples: u64,
    pub hit_tests: u64,
    /// Number of individual `LayoutNode` values cloned, including descendants
    /// copied by a deep clone of a layout root.
    pub layout_node_clones: u64,
}

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static CURRENT: Cell<DragPathStats> = const { Cell::new(DragPathStats {
        interpolation_subsamples: 0,
        hit_tests: 0,
        layout_node_clones: 0,
    }) };
    static LAST: Cell<Option<DragPathStats>> = const { Cell::new(None) };
}

pub(crate) struct DragPathGuard;

pub(crate) fn begin_drag_path() -> DragPathGuard {
    ACTIVE.with(|active| {
        debug_assert!(!active.get(), "drag-path profiling must not nest");
        active.set(true);
    });
    CURRENT.set(DragPathStats::default());
    DragPathGuard
}

impl Drop for DragPathGuard {
    fn drop(&mut self) {
        ACTIVE.set(false);
        let stats = CURRENT.get();
        LAST.set(Some(stats));
        if std::env::var_os("ESEQLISP_PROFILE_UI").is_some() {
            eprintln!(
                "[ui-profile][drag] interpolation={} hit_tests={} layout_node_clones={}",
                stats.interpolation_subsamples, stats.hit_tests, stats.layout_node_clones,
            );
        }
    }
}

fn note(update: impl FnOnce(&mut DragPathStats)) {
    ACTIVE.with(|active| {
        if !active.get() {
            return;
        }
        CURRENT.with(|current| {
            let mut stats = current.get();
            update(&mut stats);
            current.set(stats);
        });
    });
}

pub(crate) fn note_interpolation_subsamples(count: usize) {
    note(|stats| stats.interpolation_subsamples += count as u64);
}

pub(crate) fn note_hit_test() {
    note(|stats| stats.hit_tests += 1);
}

pub(crate) fn note_layout_node_clone() {
    note(|stats| stats.layout_node_clones += 1);
}

/// Return the most recently completed drag event's counters on this thread.
///
/// Taking clears the slot, preventing a caller from accidentally attributing
/// stale counters to a later event that never entered the drag path.
pub fn take_last_drag_path_stats() -> Option<DragPathStats> {
    LAST.take()
}
