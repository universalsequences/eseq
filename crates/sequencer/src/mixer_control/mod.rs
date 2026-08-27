/*!
Sequenced mixer controls: timed track/group mute & solo holds emitted by
Lisp generators (docs/jaki-mixer-control-routes-spec.md).

A generator `:tick` calls `seq-emit-control`, producing an
[`EmittedMixerControl`] (boundary-relative musical time). The scheduler
lookahead resolves it to absolute engage/release samples and pushes a
[`ScheduledMixerControl`] into the [`MixerControlMailbox`] on
`SequencerState`. The app thread drains due controls once per frame and
applies them through the same code paths as the mixer buttons; hold
bookkeeping (union of overlapping windows, release scheduling) lives with
the drain side in `app`.
*/

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixerControlOp {
    Mute,
    Solo,
}

/// Control destination. Track indices match jaki note-route indices; groups
/// travel by name and resolve to their stable group id (and backing bus) at
/// apply time, failing loudly when unknown.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MixerControlTarget {
    Track(usize),
    Group(String),
}

/// One control hold as emitted from a generator tick, in musical time
/// relative to the tick's grid boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct EmittedMixerControl {
    pub op: MixerControlOp,
    pub target: MixerControlTarget,
    pub offset_beats: f32,
    pub duration_beats: f32,
}

/// A hold resolved to absolute sample times by the scheduler lookahead.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledMixerControl {
    pub engage_sample: u64,
    pub release_sample: u64,
    pub generator_index: usize,
    /// Mailbox arrival order; the deterministic tie-breaker after
    /// `(engage_sample, generator_index)`.
    pub seq: u64,
    pub op: MixerControlOp,
    pub target: MixerControlTarget,
}

/// Scheduler → app mailbox. The scheduler pushes resolved holds as it
/// schedules chunks (ahead of the transport); the app drains those whose
/// engage sample the transport has reached.
#[derive(Default)]
pub struct MixerControlMailbox {
    pending: Mutex<Vec<ScheduledMixerControl>>,
    next_seq: AtomicU64,
}

impl MixerControlMailbox {
    pub fn push(
        &self,
        engage_sample: u64,
        release_sample: u64,
        generator_index: usize,
        op: MixerControlOp,
        target: MixerControlTarget,
    ) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut pending = self.pending.lock().unwrap();
        pending.push(ScheduledMixerControl {
            engage_sample,
            release_sample,
            generator_index,
            seq,
            op,
            target,
        });
    }

    /// Remove and return every hold whose engage sample the transport has
    /// reached, in deterministic `(engage_sample, generator_index, seq)`
    /// order. Already-elapsed holds (release in the past too) are still
    /// returned; the apply side treats them as an engage+release pair so
    /// ordering stays consistent under a slow frame.
    pub fn drain_due(&self, rendered_sample: u64) -> Vec<ScheduledMixerControl> {
        let mut pending = self.pending.lock().unwrap();
        let mut due: Vec<ScheduledMixerControl> = Vec::new();
        pending.retain(|control| {
            if control.engage_sample <= rendered_sample {
                due.push(control.clone());
                false
            } else {
                true
            }
        });
        due.sort_by(|a, b| {
            (a.engage_sample, a.generator_index, a.seq)
                .cmp(&(b.engage_sample, b.generator_index, b.seq))
        });
        due
    }

    /// Drop every pending hold (transport stop / pattern switch: stale holds
    /// must not fire after a restart).
    pub fn clear(&self) {
        self.pending.lock().unwrap().clear();
    }

    pub fn pending_len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}
