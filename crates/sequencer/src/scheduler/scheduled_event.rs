use crate::accumulator::ResolvedStep;
use crate::effects::{MAX_SLOT_PARAMS, MAX_SLOT_TENSOR_PARAMS};
use crate::audio::MAX_VOICES;
use arrayvec::ArrayVec;
use std::cmp::Ordering as CmpOrdering;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use crossbeam_queue::ArrayQueue;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledChordData {
    /// Index zero also carries the source of a single (count == 0) note.
    pub live_origins: [Option<crate::sequencer::LiveNoteOrigin>; MAX_VOICES],
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

/// Note allocation and gate behavior resolved from the same row as the note.
/// These must not change when the UI mirrors a later scene while a note is
/// still in lookahead. Mixer controls retain their independent live path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledVoicePolicy {
    pub gate: bool,
    pub polyphonic: bool,
    pub max_polyphony: usize,
    pub mono_trigger: crate::sequencer::MonoTrigger,
    pub voice_priority: crate::sequencer::VoicePriority,
    pub base_note_offset: f32,
}

impl ScheduledVoicePolicy {
    pub fn from_track(track: &crate::sequencer::SequencerTrackSnapshot) -> Self {
        Self {
            gate: track.params.gate,
            polyphonic: track.params.polyphonic,
            max_polyphony: track.params.max_polyphony,
            mono_trigger: track.params.mono_trigger,
            voice_priority: track.params.voice_priority,
            base_note_offset: track.instrument_base_note_offset,
        }
    }
}

#[cfg(test)]
impl Default for ScheduledVoicePolicy {
    fn default() -> Self {
        Self { gate: true, polyphonic: false, max_polyphony: 6,
            mono_trigger: crate::sequencer::MonoTrigger::Retrig,
            voice_priority: crate::sequencer::VoicePriority::Last, base_note_offset: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScheduledEventKind {
    ResolvedTrigger {
        voice_policy: ScheduledVoicePolicy,
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
        voice_policy: ScheduledVoicePolicy,
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
    /// Off-step boundary on an Instrument Rack track: the rack's p-locks at
    /// `step` (macros, slot params, slot instrument and effect params) and
    /// any held rack-macro print latch apply to the rack's sounding voices
    /// and per-slot chains without a trigger — the rack analog of
    /// `InstrumentParams` + `EffectParams`. The audio side resolves the step
    /// against its own copy of the scheduler snapshot.
    RackParams {
        track: usize,
        step: usize,
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
    ready: ArrayQueue<Arc<ScheduledEvent>>,
    // Only producers access this mutex. A retained owner guarantees that
    // dropping a callback's event (including cancellation) cannot free its
    // Vecs, tensors or live-send cells. Admission counts ALL outstanding
    // events, including those already moved into callback countdown storage.
    // Thus reclamation needs neither an overflow-prone garbage queue nor a
    // leak fallback. The callback owns the queue until stream teardown.
    retained: Mutex<Vec<Arc<ScheduledEvent>>>,
    rejected_events: AtomicU64,
}

impl<const CAPACITY: usize> ScheduledEventQueue<CAPACITY> {
    pub fn new() -> Self {
        assert!(CAPACITY >= 2, "scheduled event queue needs at least two slots");
        Self {
            ready: ArrayQueue::new(CAPACITY - 1),
            retained: Mutex::new(Vec::with_capacity(CAPACITY - 1)),
            rejected_events: AtomicU64::new(0),
        }
    }

    /// Producer only: prepare payloads and reclaim completed events here.
    pub fn push(&self, mut event: ScheduledEvent) -> Result<(), ScheduledEvent> {
        let mut retained = self.retained.lock().unwrap();
        // Once this is the sole owner, no consumer can obtain another one:
        // both ready entries and callback leases hold their own strong ref.
        retained.retain(|event| Arc::strong_count(event) != 1);
        if retained.len() == CAPACITY - 1 {
            self.rejected_events.fetch_add(1, Ordering::Relaxed);
            return Err(event);
        }
        match &mut event.kind {
            ScheduledEventKind::ResolvedTrigger { effect_params, .. }
            | ScheduledEventKind::NetworkTrigger { effect_params, .. }
            | ScheduledEventKind::EffectParams { effect_params, .. } => {
                // Stable ordering preserves authored precedence at equal
                // targets. Live send values are still read at dispatch time.
                effect_params.sort_by_key(|param| (param.logical_id, param.idx));
            }
            _ => {}
        }
        let event = Arc::new(event);
        retained.push(Arc::clone(&event));
        // ready.len() <= retained.len(); producer admission reserves this slot.
        self.ready.push(event).expect("admitted scheduled event has queue capacity");
        Ok(())
    }

    /// Consumer: borrow an immutable payload whose destruction stays with the
    /// producer. Retain this owner for the entire countdown/dispatch lifetime.
    pub fn pop(&self) -> Option<Arc<ScheduledEvent>> {
        self.ready.pop()
    }

    /// Scheduler tests inspect owned events without borrowing their fixtures.
    #[cfg(test)]
    pub(crate) fn pop_owned(&self) -> Option<ScheduledEvent> {
        self.pop().map(|event| (*event).clone())
    }

    /// Cumulative admission failures, including failures a producer logs and
    /// continues past. Clearing pending events must not erase render failures.
    pub fn rejected_events(&self) -> u64 {
        self.rejected_events.load(Ordering::Relaxed)
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
    fn queue_reports_rejections_across_clear_and_reuse() {
        let queue = ScheduledEventQueue::<2>::new();
        let event = ScheduledEvent {
            pattern_epoch: 0, sample_time: 1,
            kind: ScheduledEventKind::RackParams { track: 0, step: 0 },
        };
        queue.push(event.clone()).unwrap();
        assert_eq!(queue.push(event.clone()), Err(event.clone()));
        assert_eq!(queue.rejected_events(), 1);
        queue.clear();
        queue.push(event.clone()).unwrap();
        assert_eq!(queue.push(event.clone()), Err(event));
        assert_eq!(queue.rejected_events(), 2);
    }

    #[test]
    fn queue_drop_releases_unconsumed_event_payloads() {
        let params = crate::sequencer::TrackParams::new();
        params.set_sends(vec![crate::sequencer::TrackSendSnapshot {
            destination: crate::sequencer::BusId::DEFAULT_A, amount: 0.5,
        }]);
        let baseline = params.send_baseline(crate::sequencer::BusId::DEFAULT_A).unwrap();
        drop(params);
        let retained = std::sync::Arc::downgrade(&baseline);
        let queue = ScheduledEventQueue::<2>::new();
        queue.push(ScheduledEvent {
            pattern_epoch: 0, sample_time: 1000,
            kind: ScheduledEventKind::EffectParams {
                track: 0,
                effect_params: vec![ScheduledEffectParam {
                    logical_id: 1, idx: 0, value: 0.5,
                    live_value: Some(super::LiveScheduledEffectValue::new(baseline)),
                }],
            },
        }).unwrap();
        assert!(retained.upgrade().is_some());
        drop(queue);
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn queue_reclaims_payloads_only_on_producer_and_bounds_inflight_events() {
        let queue = ScheduledEventQueue::<3>::new();
        let event = |sample_time| ScheduledEvent {
            pattern_epoch: 0, sample_time,
            kind: ScheduledEventKind::InstrumentParams {
                track: 0, instrument_params: ScheduledInstrumentParams::new(),
                instrument_tensor_params: ScheduledInstrumentTensorParams::from_iter([
                    super::ScheduledInstrumentTensorParam {
                        cell_offset: 0, values: vec![1.0; 1024],
                    },
                ]),
            },
        };
        queue.push(event(0)).unwrap();
        queue.push(event(1)).unwrap();
        let ((first, second), counts) = crate::test_alloc::measure(|| {
            (queue.pop().unwrap(), queue.pop().unwrap())
        });
        assert_eq!(counts, crate::test_alloc::Counts::default());
        let first_weak = std::sync::Arc::downgrade(&first);
        let second_weak = std::sync::Arc::downgrade(&second);
        // Empty ready queue does not release admission capacity while a
        // callback still holds either event in its countdown storage.
        let rejected = queue.push(event(2)).unwrap_err();
        let (_, counts) = crate::test_alloc::measure(|| drop(second));
        assert_eq!(counts, crate::test_alloc::Counts::default());
        assert!(second_weak.upgrade().is_some());
        queue.push(rejected).unwrap();
        assert!(second_weak.upgrade().is_none());
        assert!(first_weak.upgrade().is_some());
        let (_, counts) = crate::test_alloc::measure(|| {
            drop(first);
            queue.clear();
        });
        assert_eq!(counts, crate::test_alloc::Counts::default());
        drop(queue);
        assert!(first_weak.upgrade().is_none());
    }

    #[test]
    fn queue_prepares_stable_effect_order_and_keeps_live_send_reads() {
        let params = crate::sequencer::TrackParams::new();
        params.set_sends(vec![crate::sequencer::TrackSendSnapshot {
            destination: crate::sequencer::BusId::DEFAULT_A, amount: 0.5,
        }]);
        let baseline = params.send_baseline(crate::sequencer::BusId::DEFAULT_A).unwrap();
        let queue = ScheduledEventQueue::<2>::new();
        queue.push(ScheduledEvent {
            pattern_epoch: 0, sample_time: 0,
            kind: ScheduledEventKind::EffectParams {
                track: 0, effect_params: vec![
                    ScheduledEffectParam::fixed(2, 1, 1.0),
                    ScheduledEffectParam::fixed(1, 3, 2.0),
                    ScheduledEffectParam::fixed(2, 1, 3.0),
                    ScheduledEffectParam { logical_id: 1, idx: 0, value: 0.5,
                        live_value: Some(super::LiveScheduledEffectValue::new(baseline)) },
                ],
            },
        }).unwrap();
        params.set_sends(vec![crate::sequencer::TrackSendSnapshot {
            destination: crate::sequencer::BusId::DEFAULT_A, amount: 0.75,
        }]);
        let (_, counts) = crate::test_alloc::measure(|| {
            let event = queue.pop().unwrap();
            let ScheduledEventKind::EffectParams { effect_params, .. } = &event.kind else {
                panic!("expected effect params");
            };
            assert_eq!(effect_params.iter().map(|param| param.current_value())
                .collect::<arrayvec::ArrayVec<_, 4>>().as_slice(), &[0.75, 2.0, 1.0, 3.0]);
        });
        assert_eq!(counts, crate::test_alloc::Counts::default());
    }

    #[test]
    fn queue_concurrent_reuse_never_reclaims_on_consumer() {
        let queue = std::sync::Arc::new(ScheduledEventQueue::<8>::new());
        let consumer_queue = std::sync::Arc::clone(&queue);
        let consumer = std::thread::spawn(move || crate::test_alloc::measure(|| {
            for expected in 0..4096 {
                let event = loop {
                    if let Some(event) = consumer_queue.pop() { break event; }
                    std::thread::yield_now();
                };
                assert_eq!(event.sample_time, expected);
            }
        }).1);
        for sample_time in 0..4096 {
            let mut event = ScheduledEvent {
                pattern_epoch: 0, sample_time,
                kind: ScheduledEventKind::EffectParams {
                    track: 0, effect_params: vec![ScheduledEffectParam::fixed(1, 0, 0.5); 256],
                },
            };
            loop {
                match queue.push(event) {
                    Ok(()) => break,
                    Err(rejected) => { event = rejected; std::thread::yield_now(); }
                }
            }
        }
        assert_eq!(consumer.join().unwrap(), crate::test_alloc::Counts::default());
    }

    #[test]
    fn queue_preserves_fifo_order() {
        let queue = ScheduledEventQueue::<8>::new();
        queue
            .push(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 10,
                kind: ScheduledEventKind::ResolvedTrigger {
                    voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                        live_origins: [None; crate::audio::MAX_VOICES],
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
                    voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                        live_origins: [None; crate::audio::MAX_VOICES],
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
            queue.pop_owned(),
            Some(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 10,
                kind: ScheduledEventKind::ResolvedTrigger {
                    voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                        live_origins: [None; crate::audio::MAX_VOICES],
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
            queue.pop_owned(),
            Some(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 11,
                kind: ScheduledEventKind::ResolvedTrigger {
                    voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                        live_origins: [None; crate::audio::MAX_VOICES],
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
        assert_eq!(queue.pop_owned(), None);
    }

    #[test]
    fn queue_reports_full() {
        let queue = ScheduledEventQueue::<2>::new();
        queue
            .push(ScheduledEvent {
                pattern_epoch: 0,
                sample_time: 1,
                kind: ScheduledEventKind::ResolvedTrigger {
                    voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                        live_origins: [None; crate::audio::MAX_VOICES],
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
                voice_policy: crate::scheduled_event::ScheduledVoicePolicy::default(),
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
                    live_origins: [None; crate::audio::MAX_VOICES],
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
