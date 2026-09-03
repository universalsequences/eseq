use crate::accumulator::ResolvedStep;
use crate::effects::{MAX_SLOT_PARAMS, MAX_SLOT_TENSOR_PARAMS};
use crate::audio::MAX_VOICES;
use arrayvec::ArrayVec;
use std::cell::UnsafeCell;
use std::cmp::Ordering as CmpOrdering;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

#[derive(Clone)]
pub struct LiveScheduledEffectValue(Arc<crate::sequencer::TrackSendBaseline>);

impl LiveScheduledEffectValue {
    pub(crate) fn new(baseline: Arc<crate::sequencer::TrackSendBaseline>) -> Self {
        Self(baseline)
    }

    fn load(&self) -> f32 {
        self.0.load()
    }
}

impl std::fmt::Debug for LiveScheduledEffectValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("LiveScheduledEffectValue")
            .field(&self.load())
            .finish()
    }
}

impl PartialEq for LiveScheduledEffectValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEffectParam {
    pub logical_id: u64,
    pub idx: u64,
    pub value: f32,
    /// Present for an unlocked bus-send restoration. The copied `value` is
    /// useful for diagnostics, while dispatch loads this cell so a mixer edit
    /// made after scheduling wins over stale lookahead.
    pub live_value: Option<LiveScheduledEffectValue>,
}

impl ScheduledEffectParam {
    pub fn fixed(logical_id: u64, idx: u64, value: f32) -> Self {
        Self {
            logical_id,
            idx,
            value,
            live_value: None,
        }
    }

    pub fn current_value(&self) -> f32 {
        self.live_value
            .as_ref()
            .map(LiveScheduledEffectValue::load)
            .unwrap_or(self.value)
    }
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
    pub slice_mode: f32,
    pub slice_sensitivity: f32,
    pub slice_base: f32,
    pub start_point_locked: bool,
    pub end_point_locked: bool,
    pub warp_preserve: f32,
    pub warp_seg_loop_mode: f32,
    pub warp_seg_envelope: f32,
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
            slice_mode: 0.0,
            slice_sensitivity: 0.5,
            slice_base: 0.0,
            start_point_locked: false,
            end_point_locked: false,
            warp_preserve: crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
            warp_seg_loop_mode: crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
            warp_seg_envelope: crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
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
    pub rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
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
        sampler_params: ScheduledSamplerParams,
        instrument_fingerprint: u64,
        rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
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
        rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
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
    // The queue can hold large event payloads. Keeping its fixed-capacity
    // storage inline made construction reserve tens of MiB on the caller's
    // stack in debug builds before Arc could move it to the heap.
    slots: Box<[UnsafeCell<MaybeUninit<ScheduledEvent>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl<const CAPACITY: usize> Sync for ScheduledEventQueue<CAPACITY> {}

impl<const CAPACITY: usize> ScheduledEventQueue<CAPACITY> {
    pub fn new() -> Self {
        let slots = (0..CAPACITY)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
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
        ScheduledInstrumentParams, ScheduledInstrumentTensorParams, ScheduledSamplerParams,
    };
    use crate::accumulator::ResolvedStep;
    use crate::audio::MAX_VOICES;

    fn empty_effect_params() -> Vec<ScheduledEffectParam> {
        Vec::new()
    }

    fn empty_instrument_params() -> ScheduledInstrumentParams {
        ScheduledInstrumentParams::new()
    }

    fn empty_instrument_tensor_params() -> ScheduledInstrumentTensorParams {
        ScheduledInstrumentTensorParams::new()
    }

    fn default_sampler_params() -> ScheduledSamplerParams {
        ScheduledSamplerParams::default()
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
                        retrig: crate::sequencer::StepParam::Retrig.default_value(),
                        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: vec![ScheduledEffectParam::fixed(7, 1, 0.5)],
                    instrument_params: ScheduledInstrumentParams::from_iter([
                        ScheduledInstrumentParam {
                            target: ScheduledInstrumentParamTarget::Synth,
                            idx: 2,
                            span: 1,
                            value: 0.75,
                        },
                    ]),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    sampler_params: default_sampler_params(),
                    instrument_fingerprint: 11,
                    rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
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
                        retrig: crate::sequencer::StepParam::Retrig.default_value(),
                        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
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
                    sampler_params: default_sampler_params(),
                    instrument_fingerprint: 0,
                    rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
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
                        retrig: crate::sequencer::StepParam::Retrig.default_value(),
                        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; MAX_VOICES],
                        durations: [0.0; MAX_VOICES],
                        delays: [0.0; MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: vec![ScheduledEffectParam::fixed(7, 1, 0.5)],
                    instrument_params: ScheduledInstrumentParams::from_iter([
                        ScheduledInstrumentParam {
                            target: ScheduledInstrumentParamTarget::Synth,
                            idx: 2,
                            span: 1,
                            value: 0.75,
                        }
                    ]),
                    instrument_tensor_params: empty_instrument_tensor_params(),
                    sampler_params: default_sampler_params(),
                    instrument_fingerprint: 11,
                    rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
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
                        retrig: crate::sequencer::StepParam::Retrig.default_value(),
                        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
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
                    sampler_params: default_sampler_params(),
                    instrument_fingerprint: 0,
                    rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
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
                        retrig: crate::sequencer::StepParam::Retrig.default_value(),
                        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
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
                    sampler_params: default_sampler_params(),
                    instrument_fingerprint: 0,
                    rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
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
                    retrig: crate::sequencer::StepParam::Retrig.default_value(),
                    retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
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
                sampler_params: default_sampler_params(),
                instrument_fingerprint: 0,
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            },
        });
        assert!(overflow.is_err());
    }
}
