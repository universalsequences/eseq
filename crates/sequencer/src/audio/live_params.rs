//! Live instrument writes keep their consumer identity until the audio thread
//! resolves voice ownership. Broadcasting directly from the UI would race
//! voice stealing and overwrite other consumers of a shared engine.

use super::*;
use crossbeam_queue::ArrayQueue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstrumentParamVoice {
    pub synth_id: u32,
    pub modulator_id: u32,
    pub gatepitch_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstrumentParamOwner {
    pub engine_id: usize,
    pub route: usize,
    // Graph identities reject pending writes after an engine or route rebuild,
    // including deletion/reordering that reuses the same consumer index.
    pub voices: [InstrumentParamVoice; MAX_VOICES],
    pub route_lid: u64,
    pub free_patch: bool,
}

#[derive(Clone)]
pub(crate) enum InstrumentParamWrite {
    Scalar(ScheduledInstrumentParam),
    Tensor { cell_offset: usize, values: Arc<[f32]> },
}

#[derive(Clone)]
struct Write {
    sequence: u64,
    owner: InstrumentParamOwner,
    value: InstrumentParamWrite,
}

struct Batch {
    sequence: u64,
    writes: Vec<Write>,
}

#[derive(Default)]
struct Producer {
    sequence: u64,
    // One pending value per consumer and descriptor parameter. Tensor indices
    // occupy a separate namespace from scalar indices.
    pending: BTreeMap<(usize, bool, usize), Write>,
    retained: Vec<Arc<Batch>>,
}

pub(crate) struct LiveInstrumentParams {
    producer: Mutex<Producer>,
    ready: ArrayQueue<Arc<Batch>>,
    consumed: AtomicU64,
}

impl Default for LiveInstrumentParams {
    fn default() -> Self {
        Self { producer: Mutex::new(Producer::default()), ready: ArrayQueue::new(1),
            consumed: AtomicU64::new(0) }
    }
}

impl LiveInstrumentParams {
    /// Control thread only. Coalesce an undelivered edit without losing edits
    /// to other parameters. A paused/stalled consumer cannot overflow a FIFO
    /// or lose the final knob value. Pending storage is bounded by the edited
    /// parameters, not by gesture duration.
    pub(crate) fn publish(
        &self,
        owner: InstrumentParamOwner,
        param_index: usize,
        value: InstrumentParamWrite,
    ) {
        let mut producer = self.producer.lock().unwrap();
        let consumed = self.consumed.load(Ordering::Acquire);
        producer.pending.retain(|_, write| write.sequence > consumed
            && (write.owner.route != owner.route || write.owner == owner));
        // Only the producer frees batches, including their tensor payloads.
        // The ready slot and the one audio consumer each hold an extra Arc;
        // a strong count of one proves neither can still acquire this batch.
        drop(self.ready.pop());
        producer.retained.retain(|batch| Arc::strong_count(batch) > 1);
        producer.sequence += 1;
        let sequence = producer.sequence;
        let tensor = matches!(value, InstrumentParamWrite::Tensor { .. });
        producer.pending.insert((owner.route, tensor, param_index), Write { sequence, owner, value });
        let mut writes: Vec<_> = producer.pending.values().cloned().collect();
        writes.sort_unstable_by_key(|write| write.sequence);
        let batch = Arc::new(Batch { sequence, writes });
        producer.retained.push(Arc::clone(&batch));
        // Publishers serialize and just emptied the slot. The consumer can
        // only remove from it, so admission cannot fail.
        assert!(self.ready.push(batch).is_ok());
    }

    /// One audio consumer. A producer may publish a newer cumulative batch
    /// while this one is in use; its already-consumed prefix is skipped on
    /// the next callback so edits do not reapply over subsequent p-locks.
    fn consume(&self, mut apply: impl FnMut(InstrumentParamOwner, &InstrumentParamWrite)) {
        let Some(batch) = self.ready.pop() else { return; };
        let consumed = self.consumed.load(Ordering::Relaxed);
        for write in &batch.writes {
            if write.sequence > consumed { apply(write.owner, &write.value); }
        }
        self.consumed.store(batch.sequence, Ordering::Release);
        // The producer retains this Arc even if another batch replaced it.
        drop(batch);
    }
}

pub(super) fn apply_live_instrument_params(data: &mut AudioCallbackData) {
    let state = Arc::clone(&data.state);
    state.live_instrument_params.consume(|owner, value| {
        let Some(pool) = data.custom_engine_pools.get_mut(owner.engine_id) else { return; };
        if state.runtime.engine_synth_node_ids[owner.engine_id][0].load(Ordering::Acquire)
            != owner.voices[0].synth_id
        {
            return;
        }
        let mut route_matches = false;
        for_each_custom_route_lid(&state, owner.engine_id, 0, owner.route, |lid| {
            route_matches |= lid == owner.route_lid;
        });
        if !route_matches { return; }
        for voice_idx in 0..pool.num_voices {
            let voice = &mut pool.voices[voice_idx];
            let nodes = owner.voices[voice_idx];
            // Use immutable node IDs from the publication. Reading fresh
            // runtime arrays here could mix generations if the UI rebuilds
            // the graph after the route check. The callback-owned gatepitch
            // identity proves this pool slot belongs to that same runtime.
            if voice.logical_id != nodes.gatepitch_id as u64 { continue; }
            let targeted = if owner.free_patch { voice_idx == 0 } else {
                voice.active && voice.assigned_route == Some(owner.route)
            };
            if !targeted { continue; }
            let synth = nodes.synth_id as u64;
            let modulator = nodes.modulator_id as u64;
            if synth == 0 { continue; }
            unsafe {
                match value {
                    InstrumentParamWrite::Scalar(param) => dispatch_instrument_params_to_voice(
                        data.lg.0, synth, modulator, std::slice::from_ref(param)),
                    InstrumentParamWrite::Tensor { cell_offset, values } => {
                        crate::lisp_host::queue_tensor_write(data.lg.0, synth as i32, *cell_offset, values);
                    }
                }
            }
            // Next allocation must restore its full sound, even if the voice
            // is reused for an otherwise identical trigger/preset fingerprint.
            voice.fingerprint = 0;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(route: usize) -> InstrumentParamOwner {
        InstrumentParamOwner { engine_id: 0, route, route_lid: 2, free_patch: false,
            voices: [InstrumentParamVoice { synth_id: 1, modulator_id: 2, gatepitch_id: 3 }; MAX_VOICES] }
    }

    fn scalar(value: f32) -> InstrumentParamWrite {
        InstrumentParamWrite::Scalar(ScheduledInstrumentParam {
            target: ScheduledInstrumentParamTarget::Synth, idx: 10, span: 1, value,
        })
    }

    #[test]
    fn stalled_consumer_keeps_latest_values_for_every_parameter_and_owner() {
        let mailbox = LiveInstrumentParams::default();
        for value in 0..1000 {
            mailbox.publish(owner(0), 0, scalar(value as f32));
            mailbox.publish(owner(1), 0, scalar(-(value as f32)));
        }
        mailbox.publish(owner(0), 1, scalar(0.5));
        let mut received = Vec::new();
        mailbox.consume(|owner, write| {
            if let InstrumentParamWrite::Scalar(param) = write { received.push((owner.route, param.value)); }
        });
        assert_eq!(received, vec![(0, 999.0), (1, -999.0), (0, 0.5)]);
        assert_eq!(mailbox.producer.lock().unwrap().retained.len(), 1);
        mailbox.consume(|_, _| panic!("a consumed edit must not replay"));
    }

    #[test]
    fn publication_during_consumption_does_not_replay_the_consumed_prefix() {
        let mailbox = LiveInstrumentParams::default();
        mailbox.publish(owner(0), 0, scalar(0.1));
        mailbox.consume(|_, _| {
            mailbox.publish(owner(1), 0, scalar(0.2));
            mailbox.publish(owner(1), 1, scalar(0.3));
        });
        let mut received = Vec::new();
        mailbox.consume(|owner, write| {
            if let InstrumentParamWrite::Scalar(param) = write { received.push((owner.route, param.value)); }
        });
        assert_eq!(received, vec![(1, 0.2), (1, 0.3)]);
    }

    #[test]
    fn scalar_and_tensor_consumption_never_allocates_or_frees() {
        let mailbox = LiveInstrumentParams::default();
        mailbox.publish(owner(0), 0, scalar(0.1));
        mailbox.publish(owner(0), 0, InstrumentParamWrite::Tensor {
            cell_offset: 20, values: vec![0.25; 4096].into(),
        });
        let (count, allocations) = crate::test_alloc::measure(|| {
            let mut count = 0;
            mailbox.consume(|_, write| { std::hint::black_box(write); count += 1; });
            count
        });
        assert_eq!(count, 2);
        assert_eq!(allocations, crate::test_alloc::Counts::default());
        // A subsequent publication reclaims the old batch on the producer.
        mailbox.publish(owner(1), 0, scalar(0.2));
        assert_eq!(mailbox.producer.lock().unwrap().retained.len(), 1);
    }
}
