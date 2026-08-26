//! Realtime-safe handoff of the immutable scheduler snapshot (bead eseq-sj01).
//!
//! The audio callback used to read the published snapshot through
//! `Mutex<Arc<SequencerSnapshot>>` and assign it over its previous `Arc`. Both
//! halves of that line violate realtime discipline:
//!
//! * the assignment DROPS the outgoing `Arc` on the audio thread, and when the
//!   callback holds the last reference the whole deep structure (per-step chord
//!   `Vec`s, per-step effect p-lock `Vec<Vec<Option<f32>>>`, `String`-bearing
//!   effect descriptors) is freed there — order tens of thousands of frees
//!   inside a ~10.7 ms block budget;
//! * `std::sync::Mutex` has no priority inheritance, so a publish landing at the
//!   wrong moment can futex-wait the realtime thread behind a non-realtime one.
//!
//! This type removes both. Publishers push into a bounded lock-free ring; the
//! callback pops the newest entry (never touching the mutex) and hands its
//! outgoing `Arc` to a second bounded ring that non-realtime threads drain and
//! drop. Neither realtime operation allocates, takes a lock, or makes a syscall
//! — but the rings are lock-free, not wait-free. The retire ring has two
//! producers (the callback retiring the outgoing snapshot, the publisher
//! retiring superseded ones), so if a non-realtime peer — a drain's pop or the
//! publisher's push — is preempted mid-slot, the callback's push spins for the
//! few instructions between that peer's CAS and its stamp store, and only
//! yields after many consecutive lost rounds. That window is the whole cost
//! today; adding further producers or consumers widens it and needs a
//! re-audit. When the retire
//! ring is full the callback falls back to the inline drop — bounded
//! degradation, never a leak — and counts it so instrumentation can surface the
//! fallback.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;

use crate::sequencer::SequencerSnapshot;

/// Publications in flight toward the audio thread. Each publisher supersedes
/// whatever is already queued, so one live slot plus slack for a concurrent
/// realtime pop is all this needs.
const PUBLISHED_CAPACITY: usize = 4;

/// Snapshots awaiting their non-realtime free. Sized for the same burst plus the
/// slack of a scheduler iteration (1-2 ms) that has not drained yet.
const RETIRED_CAPACITY: usize = 64;

pub struct SchedulerSnapshotHandoff {
    /// Non-realtime publishers -> audio thread. Carries the version each
    /// snapshot was published under so the reader can ignore anything it has
    /// already observed.
    published: ArrayQueue<(u64, Arc<SequencerSnapshot>)>,
    /// Audio thread -> non-realtime reclaimer.
    retired: ArrayQueue<Arc<SequencerSnapshot>>,
    /// Times the retire ring was full and the audio thread had to drop inline.
    retired_inline: AtomicU64,
}

impl Default for SchedulerSnapshotHandoff {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerSnapshotHandoff {
    pub fn new() -> Self {
        Self {
            published: ArrayQueue::new(PUBLISHED_CAPACITY),
            retired: ArrayQueue::new(RETIRED_CAPACITY),
            retired_inline: AtomicU64::new(0),
        }
    }

    /// Hand a freshly published snapshot to the audio thread. Non-realtime.
    ///
    /// Only the newest publication is ever consumed, so anything the audio
    /// thread has not picked up yet is superseded and retired here for a
    /// non-realtime drain. That keeps the ring's retention at the one snapshot
    /// actually in flight instead of a queue's worth of dead projects, and keeps
    /// the deep free off this call — publishers run inside the published-snapshot
    /// lock, which the scheduler worker contends on every 1-2 ms.
    pub fn publish(&self, version: u64, snapshot: Arc<SequencerSnapshot>) {
        while let Some((_, superseded)) = self.published.pop() {
            self.retire(superseded);
        }
        let mut entry = (version, snapshot);
        while let Err(rejected) = self.published.push(entry) {
            // Unreachable in practice — publishers serialize behind the
            // published-snapshot lock and the drain above just emptied the ring
            // — but evicting the oldest keeps the newest reaching the reader
            // rather than being refused.
            let _ = self.published.pop();
            entry = rejected;
        }
    }

    /// Take the newest publication strictly newer than `current_version`, if
    /// any. Realtime-safe: lock-free, allocation-free, and every entry it skips
    /// is retired rather than dropped here. Lock-free is not wait-free: with a
    /// single publisher whose supersede drains this ring empty before each push,
    /// the pop's yielding back-off path is unreachable in practice, but a second
    /// producer or consumer on `published` would put it back in play.
    pub fn take_latest(
        &self,
        current_version: u64,
    ) -> Option<(u64, Arc<SequencerSnapshot>)> {
        let mut newest: Option<(u64, Arc<SequencerSnapshot>)> = None;
        while let Some((version, snapshot)) = self.published.pop() {
            match newest.take() {
                Some(previous) if previous.0 >= version => {
                    self.retire(snapshot);
                    newest = Some(previous);
                }
                Some(previous) => {
                    self.retire(previous.1);
                    newest = Some((version, snapshot));
                }
                None => newest = Some((version, snapshot)),
            }
        }
        match newest {
            Some((version, snapshot)) if version > current_version => Some((version, snapshot)),
            Some((_, snapshot)) => {
                self.retire(snapshot);
                None
            }
            None => None,
        }
    }

    /// Swap `current`/`current_version` up to the newest publication, handing
    /// the outgoing `Arc` to the reclaimer instead of dropping it here.
    ///
    /// This is the whole realtime side of the seam: the audio callback calls it
    /// once per block and never touches the published `Mutex` or frees a
    /// snapshot itself. Returns whether a newer snapshot was installed.
    pub fn refresh(
        &self,
        current: &mut Arc<SequencerSnapshot>,
        current_version: &mut u64,
    ) -> bool {
        let Some((version, snapshot)) = self.take_latest(*current_version) else {
            return false;
        };
        let outgoing = std::mem::replace(current, snapshot);
        *current_version = version;
        self.retire(outgoing);
        true
    }

    /// Hand an outgoing snapshot to the reclaimer instead of dropping it on the
    /// audio thread. Lock-free and allocation-free, but not wait-free: the
    /// `retired` ring has two producers — the audio callback (via `take_latest`)
    /// and the publisher (via `publish` superseding) — so the callback's push
    /// can spin against a publisher or `drain_retired` pop preempted mid-slot,
    /// and after enough lost rounds the back-off yields. The window is a few
    /// instructions on a non-realtime thread. Falls back to an inline drop
    /// (counted) when the ring is full.
    pub fn retire(&self, snapshot: Arc<SequencerSnapshot>) {
        if let Err(snapshot) = self.retired.push(snapshot) {
            self.retired_inline.fetch_add(1, Ordering::Relaxed);
            drop(snapshot);
        }
    }

    /// Drop every retired snapshot. Call only from non-realtime threads;
    /// returns how many were freed.
    pub fn drain_retired(&self) -> usize {
        let mut freed = 0;
        while let Some(snapshot) = self.retired.pop() {
            drop(snapshot);
            freed += 1;
        }
        freed
    }

    /// How many snapshots the audio thread had to free inline because the
    /// retire ring was full.
    pub fn retired_inline_count(&self) -> u64 {
        self.retired_inline.load(Ordering::Relaxed)
    }

    /// Snapshots currently queued for reclamation. Test/instrumentation only.
    pub fn retired_len(&self) -> usize {
        self.retired.len()
    }

    /// Publications the audio thread has not consumed yet. Test-only.
    #[cfg(test)]
    pub fn published_len(&self) -> usize {
        self.published.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Arc<SequencerSnapshot> {
        Arc::new(SequencerSnapshot::empty())
    }

    #[test]
    fn refresh_retires_the_outgoing_snapshot_instead_of_freeing_it() {
        let handoff = SchedulerSnapshotHandoff::new();
        let mut current = snapshot();
        let mut version = 0;
        let outgoing = Arc::clone(&current);

        handoff.publish(1, snapshot());
        assert!(handoff.refresh(&mut current, &mut version));
        assert_eq!(version, 1);

        // The audio thread handed the old value over rather than dropping it:
        // the retire ring holds a live reference, so this thread never ran the
        // deep free.
        assert_eq!(handoff.retired_len(), 1);
        assert!(Arc::strong_count(&outgoing) >= 2);
        assert_eq!(handoff.retired_inline_count(), 0);

        assert_eq!(handoff.drain_retired(), 1);
        assert_eq!(handoff.retired_len(), 0);
        assert_eq!(Arc::strong_count(&outgoing), 1);
    }

    #[test]
    fn refresh_is_a_no_op_without_a_newer_publication() {
        let handoff = SchedulerSnapshotHandoff::new();
        let mut current = snapshot();
        let mut version = 7;
        let installed = Arc::clone(&current);

        assert!(!handoff.refresh(&mut current, &mut version));
        assert!(Arc::ptr_eq(&current, &installed));

        // A publication the reader already observed is retired, not installed.
        handoff.publish(7, snapshot());
        assert!(!handoff.refresh(&mut current, &mut version));
        assert_eq!(version, 7);
        assert!(Arc::ptr_eq(&current, &installed));
        assert_eq!(handoff.retired_len(), 1);
    }

    #[test]
    fn publishing_supersedes_whatever_the_reader_has_not_taken() {
        let handoff = SchedulerSnapshotHandoff::new();
        let superseded = snapshot();
        let witness = Arc::clone(&superseded);
        let newest = snapshot();

        handoff.publish(1, superseded);
        handoff.publish(2, Arc::clone(&newest));
        // The unconsumed publication left the published ring rather than
        // staying there for the audio thread to trip over, and it went to the
        // retire ring so the deep free happens outside the publisher's lock.
        assert_eq!(handoff.published_len(), 1);
        assert_eq!(handoff.retired_len(), 1);
        assert_eq!(Arc::strong_count(&witness), 2);

        assert_eq!(handoff.drain_retired(), 1);
        assert_eq!(Arc::strong_count(&witness), 1);

        let mut current = snapshot();
        let mut version = 0;
        assert!(handoff.refresh(&mut current, &mut version));
        assert!(Arc::ptr_eq(&current, &newest));
        assert_eq!(version, 2);
    }

    /// `take_latest` still has to reduce a multi-entry ring correctly — a
    /// realtime pop can race a publisher's supersede — and every entry it
    /// skips must be retired rather than freed on the audio thread.
    #[test]
    fn take_latest_installs_the_newest_and_retires_the_rest() {
        let handoff = SchedulerSnapshotHandoff::new();
        let newest = snapshot();
        handoff.published.push((1, snapshot())).expect("slot 1");
        handoff.published.push((3, Arc::clone(&newest))).expect("slot 2");
        handoff.published.push((2, snapshot())).expect("slot 3");

        let (version, taken) = handoff.take_latest(0).expect("a newer publication");
        assert_eq!(version, 3);
        assert!(Arc::ptr_eq(&taken, &newest));
        assert_eq!(handoff.retired_len(), 2);
        assert_eq!(handoff.retired_inline_count(), 0);
    }

    #[test]
    fn a_full_retire_ring_counts_the_inline_drop_rather_than_leaking() {
        let handoff = SchedulerSnapshotHandoff::new();
        for _ in 0..RETIRED_CAPACITY {
            handoff.retire(snapshot());
        }
        assert_eq!(handoff.retired_len(), RETIRED_CAPACITY);
        assert_eq!(handoff.retired_inline_count(), 0);

        let overflow = snapshot();
        let witness = Arc::clone(&overflow);
        handoff.retire(overflow);
        // Bounded degradation: the audio thread freed this one itself, and said
        // so. Nothing leaked and nothing stayed queued.
        assert_eq!(handoff.retired_inline_count(), 1);
        assert_eq!(handoff.retired_len(), RETIRED_CAPACITY);
        assert_eq!(Arc::strong_count(&witness), 1);

        assert_eq!(handoff.drain_retired(), RETIRED_CAPACITY);
    }
}
