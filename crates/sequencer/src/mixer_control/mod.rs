/*!
Sequenced mixer controls: timed track/group mute & solo holds emitted by
Lisp generators (docs/jaki-mixer-control-routes-spec.md).

A generator `:tick` calls `seq-emit-control`, producing an
[`EmittedMixerControl`] (boundary-relative musical time). The scheduler
lookahead resolves it to absolute engage/release samples and pushes a
[`ScheduledMixerControl`] into the [`MixerControlMailbox`] on
`SequencerState`. The app thread drains due controls once per frame and
applies them through the same code paths as the mixer buttons. Hold
bookkeeping belongs to the independent [`MixerControlHolds`] timeline;
the remaining graph application still lives in `app`.
*/

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// A musical control transition carries its source sample even when its
/// consumer advances in larger blocks. Equal-time releases precede engages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldTransition<K> {
    pub sample: u64,
    pub key: K,
    pub engaged: bool,
}

/// Driver-independent hold state. The scheduler/render driver supplies
/// resolved destinations in chronological order; no UI, graph, or wall clock
/// participates in overlap union or release timing.
pub struct MixerControlHolds<K> {
    active: Vec<(K, u64)>,
}

impl<K> Default for MixerControlHolds<K> {
    fn default() -> Self { Self { active: Vec::new() } }
}

impl<K: Copy + Eq> MixerControlHolds<K> {
    pub fn is_empty(&self) -> bool { self.active.is_empty() }

    /// Submit an ordered, nonempty hold. Release all earlier windows before
    /// engaging it, even when both edges fall inside one driver's block.
    pub fn engage(&mut self, key: K, sample: u64, release: u64) -> Result<Vec<HoldTransition<K>>, String> {
        if release <= sample {
            return Err("mixer control release must be after its engage sample".to_string());
        }
        let mut transitions = self.release_through(sample);
        if let Some((_, until)) = self.active.iter_mut().find(|(active, _)| *active == key) {
            *until = (*until).max(release);
        } else {
            self.active.push((key, release));
            transitions.push(HoldTransition { sample, key, engaged: true });
        }
        Ok(transitions)
    }

    pub fn release_through(&mut self, sample: u64) -> Vec<HoldTransition<K>> {
        let mut transitions = Vec::new();
        self.active.retain(|(key, until)| {
            if *until <= sample {
                transitions.push(HoldTransition { sample: *until, key: *key, engaged: false });
                false
            } else { true }
        });
        // Stable sort preserves engage order for equal-time releases, unlike
        // iteration through a randomized hash map.
        transitions.sort_by_key(|transition| transition.sample);
        transitions
    }

    pub fn release_all(&mut self, sample: u64) -> Vec<HoldTransition<K>> {
        self.active.drain(..).map(|(key, _)| HoldTransition {
            sample, key, engaged: false,
        }).collect()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_preserve_exact_edges_across_driver_block_sizes() {
        let holds = [(0, 0, 10), (1, 2, 12), (0, 5, 15), (0, 15, 20), (0, 25, 30)];
        let expected = [
            (0, 0, true), (2, 1, true), (12, 1, false), (15, 0, false),
            (15, 0, true), (20, 0, false), (25, 0, true), (30, 0, false),
        ];
        for block in [1, 7, 16, 64] {
            let mut runtime = MixerControlHolds::default();
            let mut pending = holds.into_iter().peekable();
            let mut trace = Vec::new();
            for frontier in (0..=64).step_by(block) {
                while pending.peek().is_some_and(|(_, sample, _)| *sample <= frontier) {
                    let (key, sample, release) = pending.next().unwrap();
                    trace.extend(runtime.engage(key, sample, release).unwrap());
                }
                trace.extend(runtime.release_through(frontier));
            }
            let trace: Vec<_> = trace.into_iter().map(|edge| (edge.sample, edge.key, edge.engaged)).collect();
            assert_eq!(trace, expected, "block={block}");
            assert!(runtime.is_empty());
        }
    }

    #[test]
    fn holds_reject_invalid_windows_without_changing_active_state() {
        let mut runtime = MixerControlHolds::default();
        runtime.engage(0, 0, 10).unwrap();
        assert!(runtime.engage(1, 10, 10).is_err());
        assert!(runtime.engage(1, 20, 5).is_err());
        assert_eq!(runtime.release_all(4), [HoldTransition { key: 0, sample: 4, engaged: false }]);
        assert!(runtime.release_through(100).is_empty());
    }
}
