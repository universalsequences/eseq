//! Sample-clocked, cancellable pattern audition. The immutable score is private
//! to the preview; project patterns, transport, tempo and history stay untouched.

use super::*;
use std::sync::Mutex;

pub struct AuditionLoop {
    pub snapshot: SequencerSnapshot,
    pub steps: usize,
}

pub struct AuditionMailbox {
    sequence: AtomicU64,
    active: AtomicU64,
    pending: Mutex<Option<(u64, AuditionLoop)>>,
    pub(crate) queue: ScheduledEventQueue<4096>,
    position: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Default for AuditionMailbox {
    fn default() -> Self {
        Self { sequence: AtomicU64::new(1), active: AtomicU64::new(0),
            pending: Mutex::new(None), queue: ScheduledEventQueue::new(),
            position: AtomicU64::new(0), error: Mutex::new(None) }
    }
}

impl AuditionMailbox {
    pub(crate) fn start(&self, score: AuditionLoop) {
        *self.error.lock().unwrap() = None;
        let mut pending = self.pending.lock().unwrap();
        let generation = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.active.store(generation, Ordering::Release);
        self.position.store(0, Ordering::Release);
        *pending = Some((generation, score));
    }

    pub fn stop(&self) {
        let mut pending = self.pending.lock().unwrap();
        self.active.store(0, Ordering::Release);
        *pending = None;
    }

    pub fn generation(&self) -> u64 { self.active.load(Ordering::Acquire) }
    pub fn is_current(&self, generation: u64) -> bool {
        generation == 0 || generation == self.generation()
    }
    pub fn take_error(&self) -> Option<String> { self.error.lock().unwrap().take() }
    pub(super) fn report_error(&self, error: String) { *self.error.lock().unwrap() = Some(error); }
    pub fn position(&self) -> f64 { f64::from_bits(self.position.load(Ordering::Acquire)) }

    fn cancel(&self, generation: u64) {
        let _ = self.active.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

struct PlayingLoop {
    generation: u64,
    score: AuditionLoop,
    origin: u64,
    next_step: u64,
    quantizer: MidiFxQuantizerState,
}

#[derive(Default)]
pub(super) struct AuditionPlayer {
    playing: Option<PlayingLoop>,
}

impl AuditionPlayer {
    pub(super) fn advance(
        &mut self, state: &SequencerState, current: &SequencerSnapshot,
        rendered: u64, horizon: u64, sample_rate: u32, block_size: usize,
        mut runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    ) -> Result<(), String> {
        let mailbox = &state.note_audition;
        if let Some((generation, score)) = mailbox.pending.lock().unwrap().take() {
            self.playing = Some(PlayingLoop { generation, score,
                origin: rendered.saturating_add(2 * block_size as u64), next_step: 0,
                quantizer: MidiFxQuantizerState::default() });
        }
        let Some(playing) = self.playing.as_mut() else { return Ok(()); };
        let snapshot = &playing.score.snapshot;
        if !mailbox.is_current(playing.generation) || current.transport.playing
            || snapshot.transport.pattern_epoch != current.transport.pattern_epoch
            || snapshot.transport.topology_epoch != current.transport.topology_epoch
        {
            mailbox.cancel(playing.generation);
            self.playing = None;
            return Ok(());
        }
        let samples_per_quarter = sample_rate as f64 * 60.0 / snapshot.transport.bpm as f64;
        let samples_per_step = samples_per_quarter / 4.0;
        let loop_samples = samples_per_step * playing.score.steps as f64;
        let elapsed = rendered.saturating_sub(playing.origin) as f64;
        mailbox.position.store((elapsed.rem_euclid(loop_samples) / loop_samples).to_bits(), Ordering::Release);
        let sink = scheduled_event::AuditionSink { queue: &mailbox.queue, generation: playing.generation };
        // Each timestamp is calculated from the fixed origin, so fractional
        // sample durations cannot accumulate drift across repeats.
        loop {
            let time = playing.origin.saturating_add((playing.next_step as f64 * samples_per_step).round() as u64);
            if time >= horizon { break; }
            if time < rendered {
                mailbox.cancel(playing.generation);
                self.playing = None;
                return Err("Loop preview fell behind the audio clock. Restart the preview".into());
            }
            let step = (playing.next_step % playing.score.steps as u64) as usize;
            let beat = playing.next_step as f64 / 4.0;
            let mut output = Vec::new();
            for (track, lane) in snapshot.tracks.iter().enumerate() {
                let data = &lane.steps[step];
                if !data.active { continue; }
                let param = |key: StepParam| data.params[key.index()];
                let resolved = ResolvedStep {
                    duration: param(StepParam::Duration), velocity: param(StepParam::Velocity),
                    speed: param(StepParam::Speed), aux_a: param(StepParam::AuxA),
                    aux_b: param(StepParam::AuxB), transpose: param(StepParam::Transpose),
                    pan: param(StepParam::Pan), chop: param(StepParam::Chop),
                    retrig: param(StepParam::Retrig), retrig_rate: param(StepParam::RetrigRate),
                };
                let chord = chord_data_from_parts(&data.chord, &data.chord_durations,
                    &data.chord_delays, resolved.duration, resolved.transpose);
                let event = step_event_from_resolved(snapshot, track, step, samples_per_step as f32,
                    resolved, chord, resolve_effect_params(snapshot, track, step, None),
                    resolve_instrument_params(snapshot, track, step, None),
                    resolve_instrument_tensor_params(snapshot, track, step));
                if !enqueue_step_event_with_midi_fx(&sink, snapshot, &mut output, runtime.as_deref_mut(),
                    Some(&mut playing.quantizer), snapshot.transport.pattern_epoch, time, beat, samples_per_quarter as f32,
                    0.0, beat as f32, event, Vec::new(), false)
                {
                    mailbox.cancel(playing.generation);
                    self.playing = None;
                    return Err("Loop preview exceeded the event queue capacity".into());
                }
            }
            playing.next_step += 1;
        }
        if let Some(runtime) = runtime {
            let horizon_beats = horizon.saturating_sub(playing.origin) as f64 / samples_per_quarter;
            for pending in playing.quantizer.drain_due(horizon_beats) {
                let time = playing.origin.saturating_add((pending.deadline_beats * samples_per_quarter).round() as u64);
                let events = run_midi_fx_chain_for_track_inner(runtime, snapshot, pending.source_track,
                    vec![pending.event], Some(&mut playing.quantizer), pending.resume_stage_idx,
                    0, [false; MAX_TRACKS], false);
                if !enqueue_midi_fx_events(&sink, snapshot, &mut Vec::new(), snapshot.transport.pattern_epoch,
                    time, pending.deadline_beats, samples_per_quarter as f32, 0.0, events) {
                    mailbox.cancel(playing.generation);
                    self.playing = None;
                    return Err("Loop preview exceeded the event queue capacity".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrospective_audition_routes_midi_fx_with_cancellable_ownership() {
        let state = Arc::new(SequencerState::new(2, vec![]));
        state.pattern.patterns[0].toggle_step(0);
        state.pattern.track_params[0].set_midi_fx_chain(vec!["preview-route".into()]);
        let current = SequencerSnapshot::capture(&state);
        let mut score = current.clone();
        score.transport.bpm = 120;
        let mut runtime = lisp_host::ScratchControlRuntime::new(state.clone(), vec![vec![], vec![]],
            vec![EffectDescriptor::builtin_sampler(), EffectDescriptor::builtin_sampler()], 0, 0);
        runtime.eval(r#"(def-midi-fx "preview-route" (do (fx-suppress) (fx-emit 0 :track 1)))"#).unwrap();
        state.note_audition.start(AuditionLoop { snapshot: score, steps: 16 });
        let mut player = AuditionPlayer::default();
        player.advance(&state, &current, 0, 2048, 48_000, 256, Some(&mut runtime)).unwrap();
        let event = state.note_audition.queue.pop().expect("routed note");
        assert_eq!(event.audition_generation, state.note_audition.generation());
        assert_eq!(event.sample_time, 512);
        assert!(matches!(event.kind, ScheduledEventKind::ResolvedTrigger { track: 1, .. }));
        assert!(state.note_audition.queue.pop().is_none());
    }

    #[test]
    fn retrospective_audition_repeats_without_drift_and_cancels_on_topology_change() {
        let state = SequencerState::new(1, vec![]);
        state.pattern.patterns[0].toggle_step(0);
        state.pattern.chord_data[0].add_note_with_timing(0, 0.0, 0.4, 0.123);
        state.pattern.chord_data[0].add_note_with_timing(0, 7.0, 0.6, 0.789);
        let current = SequencerSnapshot::capture(&state);
        let mut score = current.clone();
        score.transport.bpm = 137;
        state.note_audition.start(AuditionLoop { snapshot: score, steps: 16 });
        let generation = state.note_audition.generation();
        let mut player = AuditionPlayer::default();
        let rate = 48_000;
        let block = 256;
        let samples_per_step = rate as f64 * 60.0 / 137.0 / 4.0;
        let end = (samples_per_step * 16.0 * 12.0) as u64;
        let mut events = Vec::new();
        for rendered in (0..end).step_by(block) {
            player.advance(&state, &current, rendered, rendered + 2048, rate, block, None).unwrap();
            while let Some(event) = state.note_audition.queue.pop() { events.push(event); }
        }
        assert!(events.len() >= 24);
        for (index, event) in events.iter().take(24).enumerate() {
            let delay = if index % 2 == 0 { 0.123_f32 } else { 0.789_f32 };
            let expected = 512 + ((index / 2) as f64 * 16.0 * samples_per_step).round() as u64
                + (delay as f64 * samples_per_step as f32 as f64).round() as u64;
            assert_eq!(event.sample_time, expected);
            assert_eq!(event.audition_generation, generation);
        }
        let mut changed = current.clone();
        changed.transport.topology_epoch += 1;
        player.advance(&state, &changed, end, end + 2048, rate, block, None).unwrap();
        assert_eq!(state.note_audition.generation(), 0);
        assert!(!state.note_audition.is_current(generation));
        assert!(state.note_audition.is_current(0), "ordinary notes keep their lifetime");
        assert!(state.note_audition.queue.pop().is_none());
    }
}
