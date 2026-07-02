use crate::accumulator::ResolvedStep;
use crate::effects::{MAX_SLOT_PARAMS, MAX_SLOT_TENSOR_PARAMS};
use crate::voice::MAX_VOICES;
use arrayvec::ArrayVec;
use std::cell::UnsafeCell;
use std::cmp::Ordering as CmpOrdering;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledChordData {
    pub count: usize,
    pub notes: [f32; MAX_VOICES],
    pub durations: [f32; MAX_VOICES],
    pub delays: [f32; MAX_VOICES],
    pub step_transpose: f32,
}

pub fn resolved_chord_transpose(
    chord_transpose: f32,
    step_transpose: f32,
    resolved_transpose: f32,
) -> f32 {
    chord_transpose + (resolved_transpose - step_transpose)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEffectParam {
    pub logical_id: u64,
    pub idx: u64,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScheduledInstrumentParamTarget {
    Synth,
    Modulator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledInstrumentParam {
    pub target: ScheduledInstrumentParamTarget,
    pub idx: u64,
    pub span: u32,
    pub value: f32,
}

pub type ScheduledInstrumentParams = ArrayVec<ScheduledInstrumentParam, MAX_SLOT_PARAMS>;

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledInstrumentTensorParam {
    pub cell_offset: usize,
    pub values: Vec<f32>,
}

pub type ScheduledInstrumentTensorParams =
    ArrayVec<ScheduledInstrumentTensorParam, MAX_SLOT_TENSOR_PARAMS>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledSamplerParams {
    pub attack_ms: f32,
    pub release_ms: f32,
    pub start_point: f32,
    pub end_point: f32,
    pub instrument_enabled: f32,
    pub reverse: f32,
    pub loop_mode: f32,
    pub loop_xfade_ms: f32,
    pub sr_hz: f32,
    pub warp_enabled: f32,
    pub warp_mode: f32,
    pub sample_bpm: f32,
    pub playback_speed: f32,
    pub scrub: f32,
}

impl Default for ScheduledSamplerParams {
    fn default() -> Self {
        Self {
            attack_ms: 0.0,
            release_ms: 0.0,
            start_point: 0.0,
            end_point: 1.0,
            instrument_enabled: 1.0,
            reverse: 0.0,
            loop_mode: 0.0,
            loop_xfade_ms: 0.0,
            sr_hz: 0.0,
            warp_enabled: 0.0,
            warp_mode: 0.0,
            sample_bpm: 120.0,
            playback_speed: 1.0,
            scrub: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventSource {
    Step {
        track: usize,
        step: usize,
        instrument_fingerprint: u64,
    },
    Network {
        seed: Option<(usize, usize)>,
        neuron: usize,
        instrument_fingerprint: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepEvent {
    pub track: usize,
    pub samples_per_step: f32,
    pub resolved: ResolvedStep,
    pub chord: ScheduledChordData,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: ScheduledInstrumentParams,
    pub instrument_tensor_params: ScheduledInstrumentTensorParams,
    pub sampler_params: ScheduledSamplerParams,
    pub source: EventSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScheduledEventKind {
    ResolvedTrigger {
        track: usize,
        step: usize,
        samples_per_step: f32,
        resolved: ResolvedStep,
        chord: ScheduledChordData,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: ScheduledInstrumentParams,
        instrument_tensor_params: ScheduledInstrumentTensorParams,
        instrument_fingerprint: u64,
    },
    NetworkTrigger {
        track: usize,
        source_neuron: usize,
        seed: Option<(usize, usize)>,
        samples_per_step: f32,
        resolved: ResolvedStep,
        chord: ScheduledChordData,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: ScheduledInstrumentParams,
        instrument_tensor_params: ScheduledInstrumentTensorParams,
        sampler_params: ScheduledSamplerParams,
        instrument_fingerprint: u64,
    },
    InstrumentParams {
        track: usize,
        instrument_params: ScheduledInstrumentParams,
        instrument_tensor_params: ScheduledInstrumentTensorParams,
    },
    EffectParams {
        track: usize,
        effect_params: Vec<ScheduledEffectParam>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub pattern_epoch: u64,
    pub sample_time: u64,
    pub kind: ScheduledEventKind,
}

#[derive(Debug)]
pub struct TimedEvent {
    pub sample_time: u64,
    pub seq: u64,
    pub event: ScheduledEvent,
}

impl PartialEq for TimedEvent {
    fn eq(&self, other: &Self) -> bool {
        self.sample_time == other.sample_time && self.seq == other.seq
    }
}

impl Eq for TimedEvent {}

impl PartialOrd for TimedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimedEvent {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.sample_time
            .cmp(&other.sample_time)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

pub struct ScheduledEventQueue<const CAPACITY: usize> {
    slots: [UnsafeCell<MaybeUninit<ScheduledEvent>>; CAPACITY],
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl<const CAPACITY: usize> Sync for ScheduledEventQueue<CAPACITY> {}

impl<const CAPACITY: usize> ScheduledEventQueue<CAPACITY> {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn next_index(index: usize) -> usize {
        (index + 1) % CAPACITY
    }

    pub fn push(&self, event: ScheduledEvent) -> Result<(), ScheduledEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        let next_tail = Self::next_index(tail);
        let head = self.head.load(Ordering::Acquire);
        if next_tail == head {
            return Err(event);
        }

        unsafe {
            (*self.slots[tail].get()).write(event);
        }
        self.tail.store(next_tail, Ordering::Release);
        Ok(())
    }

    pub fn pop(&self) -> Option<ScheduledEvent> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }

        let event = unsafe { (*self.slots[head].get()).assume_init_read() };
        self.head.store(Self::next_index(head), Ordering::Release);
        Some(event)
    }

    pub fn clear(&self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ScheduledChordData, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
        ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
        ScheduledInstrumentParams, ScheduledInstrumentTensorParams,
    };
    use crate::accumulator::ResolvedStep;
    use crate::voice::MAX_VOICES;

    fn empty_effect_params() -> Vec<ScheduledEffectParam> {
        Vec::new()
    }

    fn empty_instrument_params() -> ScheduledInstrumentParams {
        ScheduledInstrumentParams::new()
    }

    fn empty_instrument_tensor_params() -> ScheduledInstrumentTensorParams {
        ScheduledInstrumentTensorParams::new()
    }

    #[test]
    fn queue_preserves_fifo_order() {
        let queue = ScheduledEventQueue::<8>::new();
        queue
            .push(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 10,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track: 0,
                    step: 1,
                    samples_per_step: 120.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: vec![ScheduledEffectParam {
                        logical_id: 7,
                        idx: 1,
                        value: 0.5,
                    }],
                    instrument_params: ScheduledInstrumentParams::from_iter([
                        ScheduledInstrumentParam {
                            target: ScheduledInstrumentParamTarget::Synth,
                            idx: 2,
                            span: 1,
                            value: 0.75,
                        },
                    ]),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    instrument_fingerprint: 11,
                },
            })
            .unwrap();
        queue
            .push(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 11,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track: 0,
                    step: 2,
                    samples_per_step: 120.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: empty_effect_params(),
                    instrument_params: empty_instrument_params(),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    instrument_fingerprint: 0,
                },
            })
            .unwrap();

        assert_eq!(
            queue.pop(),
            Some(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 10,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track: 0,
                    step: 1,
                    samples_per_step: 120.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: vec![ScheduledEffectParam {
                        logical_id: 7,
                        idx: 1,
                        value: 0.5,
                    }],
                    instrument_params: ScheduledInstrumentParams::from_iter([
                        ScheduledInstrumentParam {
                            target: ScheduledInstrumentParamTarget::Synth,
                            idx: 2,
                            span: 1,
                            value: 0.75,
                        }
                    ]),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    instrument_fingerprint: 11,
                },
            })
        );
        assert_eq!(
            queue.pop(),
            Some(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 11,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track: 0,
                    step: 2,
                    samples_per_step: 120.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: empty_effect_params(),
                    instrument_params: empty_instrument_params(),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    instrument_fingerprint: 0,
                },
            })
        );
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn queue_reports_full() {
        let queue = ScheduledEventQueue::<2>::new();
        queue
            .push(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 1,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track: 0,
                    step: 0,
                    samples_per_step: 120.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: empty_effect_params(),
                    instrument_params: empty_instrument_params(),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    instrument_fingerprint: 0,
                },
            })
            .unwrap();

        let overflow = queue.push(ScheduledEvent {
            pattern_epoch: 0,
            sample_time: 2,
            kind: ScheduledEventKind::ResolvedTrigger {
                track: 0,
                step: 1,
                samples_per_step: 120.0,
                resolved: ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                chord: ScheduledChordData {
                    count: 0,
                    notes: [0.0; MAX_VOICES],
                    durations: [0.0; MAX_VOICES],
                    delays: [0.0; MAX_VOICES],
                    step_transpose: 0.0,
                },
                effect_params: empty_effect_params(),
                instrument_params: empty_instrument_params(),
                instrument_tensor_params: empty_instrument_tensor_params(),
                instrument_fingerprint: 0,
            },
        });
        assert!(overflow.is_err());
    }
}
