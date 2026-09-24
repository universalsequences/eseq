//! Synchronous scheduler/audio driving for a freshly prepared isolated graph.
//! Project capture and row-control preparation belong to the job layer; this
//! type does not load a project or create a second global C engine in-process.

use super::*;
use crate::scheduler::{SchedulerDriver, SchedulerInput};
use std::io;

pub(crate) struct OfflineAudioSession<'a> {
    engine: &'a engine::HeadlessEngine,
    data: Box<AudioCallbackData>,
    scheduler: SchedulerDriver,
    source_end: u64,
    topology_epoch: u64,
    bpm: u32,
    failed: bool,
}

impl<'a> OfflineAudioSession<'a> {
    /// The caller must prepare an isolated, fresh graph and publish its initial
    /// playing snapshot before construction. The engine outlives all DSP use.
    pub(crate) fn new(engine: &'a engine::HeadlessEngine, source_end: u64) -> io::Result<Self> {
        let snapshot = engine.state.latest_scheduler_snapshot();
        if engine.channels != 2 || source_end == 0 || !snapshot.transport.playing {
            return Err(io::Error::other("Offline rendering requires a stereo playing session and a nonempty source range"));
        }
        if !unsafe { prepare_graph_for_render(engine.lg_ptr.0) } {
            return Err(io::Error::other("Offline graph preparation failed or an edit batch is still open"));
        }
        let queue = Arc::new(ScheduledEventQueue::new());
        let (_tx, rx) = std::sync::mpsc::channel();
        let data = new_audio_callback_data(
            engine.lg_ptr.0, Arc::clone(&engine.state), engine.sample_rate, 2, engine.block_size,
            Arc::clone(&engine.master_recorder), rx,
            Arc::clone(&engine.buses.bus_effect_runtime), Arc::clone(&queue),
            Arc::new(AtomicU64::new(0)),
        );
        let scheduler = SchedulerDriver::new(
            Arc::clone(&engine.state), engine.sample_rate, engine.block_size, queue,
        );
        let session = Self {
            engine, data, scheduler, source_end,
            topology_epoch: snapshot.transport.topology_epoch,
            bpm: snapshot.transport.bpm,
            failed: false,
        };
        session.check_delivery()?;
        Ok(session)
    }

    fn check_delivery(&self) -> io::Result<()> {
        let (submission, delivery, edits) = unsafe {
            (graph_control_submission_failures(self.data.lg.0),
             graph_block_event_delivery_failures(self.data.lg.0),
             graph_edit_delivery_failures(self.data.lg.0))
        };
        if submission != 0 || delivery != 0 || edits != 0 || self.data.dropped_scheduled_events != 0
            || self.data.late_scheduled_events != 0
        {
            return Err(io::Error::other(format!(
                "Incomplete audio event delivery: {submission} graph submissions, {delivery} graph deliveries, {edits} failed graph edits, {} dropped and {} late scheduled events",
                self.data.dropped_scheduled_events, self.data.late_scheduled_events,
            )));
        }
        Ok(())
    }

    pub(crate) fn render_block(&mut self, start: u64, output: &mut [f32]) -> io::Result<()> {
        self.render_block_with_controls(start, output, |_| Ok(()))
    }

    pub(crate) fn render_block_with_controls(
        &mut self, start: u64, output: &mut [f32],
        controls: impl FnOnce(u64) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.failed { return Err(io::Error::other("Offline render session has already failed")); }
        let result = self.render_next(start, output, controls);
        self.failed = result.is_err();
        result
    }

    fn render_next(
        &mut self, start: u64, output: &mut [f32],
        controls: impl FnOnce(u64) -> io::Result<()>,
    ) -> io::Result<()> {
        if start != self.data.rendered_samples.load(Ordering::Acquire)
            || output.len() != self.engine.block_size * 2
        {
            return Err(io::Error::other("Offline rendering requires consecutive complete DSP blocks"));
        }
        let next = start.checked_add(self.engine.block_size as u64)
            .ok_or_else(|| io::Error::other("Offline sample clock overflow"))?;
        let snapshot = self.engine.state.latest_scheduler_snapshot();
        if snapshot.transport.topology_epoch != self.topology_epoch
            || snapshot.transport.bpm != self.bpm || !snapshot.transport.playing
        {
            return Err(io::Error::other("Offline session topology, tempo, or transport changed during rendering"));
        }
        if start < self.source_end {
            let horizon = next.min(self.source_end);
            let advance = self.scheduler.advance(start, horizon, SchedulerInput::Offline);
            let errors = self.scheduler.take_runtime_errors();
            if !errors.is_empty() {
                return Err(io::Error::other(format!("Offline scheduler failed: {}", errors.join("; "))));
            }
            let ticks = self.engine.state.drain_generator_tick_errors();
            if !ticks.is_empty() {
                return Err(io::Error::other(format!("Offline generator failed: {}",
                    ticks.iter().map(|tick| format!("{}: {}", tick.name, tick.error))
                        .collect::<Vec<_>>().join("; "))));
            }
            if advance.queue_rejections != 0 || advance.scheduled_until_sample != horizon {
                return Err(io::Error::other(format!(
                    "Offline scheduler did not complete sample {horizon}: reached {}, rejected {} events",
                    advance.scheduled_until_sample, advance.queue_rejections,
                )));
            }
        }
        // Normal controls use the same complete-block application boundary
        // as playback. The scheduler has produced source-time notices first.
        controls(start)?;
        self.check_delivery()?;
        render_audio_block(&mut self.data, output,
            AudioOutputPurpose::Export { source_end_sample: self.source_end });
        self.check_delivery()?;
        if output.iter().any(|sample| !sample.is_finite()) {
            return Err(io::Error::other(format!("Non-finite audio in offline block at sample {start}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph as graph;

    fn sampler_engine() -> engine::HeadlessEngine {
        let mut engine = engine::init_headless_engine(48_000, 2).unwrap();
        engine.state = Arc::new(SequencerState::new(1,
            vec![crate::sequencer::default_empty_effect_chain()]));
        let sample = vec![0.5_f32; 48_000 * 2];
        let buffer = unsafe { graph::create_buffer(engine.lg_ptr.0, 48_000, 2, sample.as_ptr()) };
        assert!(buffer >= 0);
        let node = crate::instruments::sampler::create_sampler_node(
            engine.lg_ptr.0, buffer, 48_000, "offline-sampler",
        ).unwrap();
        unsafe {
            assert!(graph::graph_connect(engine.lg_ptr.0, node.node_id, 0, 0, 0));
            assert!(graph::graph_connect(engine.lg_ptr.0, node.node_id, 1, 0, 1));
        }
        let runtime = &engine.state.runtime;
        runtime.instrument_type_flags[0].store(InstrumentType::Sampler.runtime_flag(), Ordering::Relaxed);
        runtime.sampler_lids[0].store(node.logical_id, Ordering::Relaxed);
        runtime.voice_counts.store(0, 1, Ordering::Relaxed);
        runtime.voice_lids[0][0].store(node.logical_id, Ordering::Relaxed);
        runtime.synth_node_ids[0][0].store(node.node_id as u32, Ordering::Relaxed);
        engine.state.toggle_step_and_clear_plocks(0, 0);
        engine.state.transport.playing.store(true, Ordering::Relaxed);
        engine.state.publish_scheduler_snapshot();
        engine
    }

    #[test]
    fn synchronous_scheduler_renders_transient_slice_sampler() {
        use crate::analysis::{pack_ptr, OnsetTableShared};

        let engine = sampler_engine();
        let slot = &engine.state.pattern.instrument_slots[0];
        let lid = engine.state.runtime.sampler_lids[0].load(Ordering::Relaxed);
        slot.apply_descriptor(&crate::effects::EffectDescriptor::builtin_sampler(), lid as u32);
        slot.defaults.set(crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 1.0);
        slot.defaults.set(crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY, 1.0);
        slot.defaults.set(6, 1.0);
        engine.state.pattern.track_params[0].gate.store(false, Ordering::Relaxed);
        engine.state.set_step_param(0, 0, StepParam::Transpose, 1.0);
        let table = OnsetTableShared {
            onsets_frames: vec![0, 24_000],
            sample_len_frames: 48_000,
            sample_rate: 48_000,
            bpm: 120.0,
            downbeat_frame: Some(0),
            manual_edits: None,
        };
        let (lo, hi) = pack_ptr(&table as *const OnsetTableShared);
        engine.state.runtime.sampler_onset_ptr_lo[0].store(lo.to_bits(), Ordering::Relaxed);
        engine.state.runtime.sampler_onset_ptr_hi[0].store(hi.to_bits(), Ordering::Relaxed);
        engine.state.runtime.sampler_analysis_status[0].store(2, Ordering::Release);
        engine.state.publish_scheduler_snapshot();
        let mut session = OfflineAudioSession::new(&engine, 4_096).unwrap();
        let mut output = vec![0.0; engine.block_size * 2];
        let mut peak = 0.0_f32;
        for start in (0..4_096).step_by(engine.block_size) {
            session.render_block(start as u64, &mut output).unwrap();
            for sample in &output {
                assert!(sample.is_finite());
                peak = peak.max(sample.abs());
            }
        }
        assert!(peak > 0.1, "scheduled slice must produce audio, peak={peak}");
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn synchronous_scheduler_renders_sampler_through_transactional_wav_sink() {
        let engine = sampler_engine();
        let plan = crate::bounce::BouncePlan::new(48_000, engine.block_size,
            engine.state.latest_scheduler_snapshot().transport.bpm,
            1.0, Some((0.01, 0.05)), 0, 0.01).unwrap();
        let mut session = OfflineAudioSession::new(&engine, plan.source_range().end).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("render.wav");
        let summary = crate::bounce::render_to_wav(&plan, &path,
            crate::bounce::Publication::CreateNew, &crate::bounce::BounceCancellation::default(),
            |start, output| session.render_block(start, output), |_| {}).unwrap();
        assert_eq!(summary.frames, plan.file_frames());
        let bytes = std::fs::read(path).unwrap();
        let samples: Vec<_> = bytes[56..].chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())).collect();
        assert_eq!(samples.len() as u64, plan.file_frames() * 2);
        assert!(samples.iter().any(|sample| *sample > 0.1), "scheduled note must reach WAV");
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert_eq!(session.data.dropped_scheduled_events, 0);
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn queued_notes_keep_row_voice_policy_after_ui_state_changes() {
        let engine = sampler_engine();
        let params = &engine.state.pattern.track_params[0];
        params.gate.store(true, Ordering::Relaxed);
        params.polyphonic.store(false, Ordering::Relaxed);
        engine.state.publish_scheduler_snapshot();
        let mut session = OfflineAudioSession::new(&engine, 100_000).unwrap();
        let advanced = session.scheduler.advance(0, 512, SchedulerInput::Offline);
        assert_eq!(advanced.scheduled_until_sample, 512);
        // Simulate the UI's row mirror overtaking an already queued note.
        // Its gate and allocation policy must still come from its source row.
        params.gate.store(false, Ordering::Relaxed);
        params.polyphonic.store(true, Ordering::Relaxed);
        engine.state.pattern.instrument_base_note_offsets[0]
            .store(12.0_f32.to_bits(), Ordering::Relaxed);
        let mut output = vec![0.0; 1024];
        render_audio_block(&mut session.data, &mut output,
            AudioOutputPurpose::Export { source_end_sample: 100_000 });
        assert!(output.iter().any(|sample| *sample > 0.1));
        assert!(!session.data.voice_pools[0].polyphonic);
        assert!(session.data.countdown_events.iter().any(|event|
            matches!(event.kind, CountdownEventKind::GateOff(_))), "source row's gate must be armed");
        assert!(engine.state.active_notes(0).contains(&60));
        assert!(!engine.state.active_notes(0).contains(&72));
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn failed_or_discontinuous_sessions_cannot_resume() {
        let engine = sampler_engine();
        let mut session = OfflineAudioSession::new(&engine, 1000).unwrap();
        let error = session.render_block(1, &mut vec![0.0; 1024]).unwrap_err();
        assert!(error.to_string().contains("consecutive"));
        assert!(session.render_block(0, &mut vec![0.0; 1024]).unwrap_err()
            .to_string().contains("already failed"));
        drop(session);
        let mut session = OfflineAudioSession::new(&engine, 1000).unwrap();
        engine.state.transport.bpm.store(137, Ordering::Relaxed);
        engine.state.publish_scheduler_snapshot();
        assert!(session.render_block(0, &mut vec![0.0; 1024]).unwrap_err()
            .to_string().contains("tempo"));
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn offline_script_failure_aborts_without_replacing_destination() {
        let engine = sampler_engine();
        engine.state.set_scratch_source("(unbound-bounce-function)");
        let plan = crate::bounce::BouncePlan::new(48_000, engine.block_size,
            engine.state.latest_scheduler_snapshot().transport.bpm, 1.0, None, 0, 0.0).unwrap();
        let mut session = OfflineAudioSession::new(&engine, plan.source_range().end).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("render.wav");
        std::fs::write(&path, b"existing take").unwrap();
        let error = crate::bounce::render_to_wav(&plan, &path,
            crate::bounce::Publication::ReplaceConfirmed, &crate::bounce::BounceCancellation::default(),
            |start, output| session.render_block(start, output), |_| {}).unwrap_err();
        assert!(error.to_string().contains("project scratch"), "{error}");
        assert_eq!(std::fs::read(path).unwrap(), b"existing take");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(session.render_block(0, &mut vec![0.0; 1024]).is_err());
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn offline_process_error_cannot_be_silently_rendered() {
        let engine = sampler_engine();
        engine.state.set_scratch_source(r#"
            (def-process broken-export-process
              :every (beats 1)
              :run (unbound-export-process-function))
            (def broken-instance (broken-export-process))
            (start broken-instance)
        "#);
        let mut session = OfflineAudioSession::new(&engine, 50_000).unwrap();
        let mut output = vec![0.0; 1024];
        let error = (0..100).find_map(|block| session.render_block(block * 512, &mut output).err())
            .expect("the clocked process must report its failed invocation");
        assert!(error.to_string().contains("process run"), "{error}");
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn queued_graph_edit_failure_aborts_render() {
        let engine = sampler_engine();
        let mut session = OfflineAudioSession::new(&engine, 1000).unwrap();
        assert!(unsafe { graph::graph_connect(engine.lg_ptr.0, 999_999, 0, 0, 0) },
            "submission succeeds; the target is validated when applied");
        let error = session.render_block(0, &mut vec![0.0; 1024]).unwrap_err();
        assert!(error.to_string().contains("1 failed graph edits"), "{error}");
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn invalid_initial_graph_is_rejected_before_session_starts() {
        let engine = sampler_engine();
        assert!(unsafe { graph::graph_connect(engine.lg_ptr.0, 999_999, 0, 0, 0) });
        let error = OfflineAudioSession::new(&engine, 1000).err().unwrap();
        assert!(error.to_string().contains("graph preparation failed"), "{error}");
        unsafe { engine.destroy(); }
    }

    #[test]
    fn graph_edit_submission_overflow_is_fatal_before_delivery() {
        let engine = sampler_engine();
        let mut session = OfflineAudioSession::new(&engine, 1000).unwrap();
        let available = unsafe { graph::graph_edit_queue_available(engine.lg_ptr.0) };
        for _ in 0..available {
            assert!(unsafe { graph::graph_connect(engine.lg_ptr.0, 0, 0, 0, 0) });
        }
        assert!(!unsafe { graph::graph_connect(engine.lg_ptr.0, 0, 0, 0, 0) });
        assert_eq!(unsafe { graph::graph_control_submission_failures(engine.lg_ptr.0) }, 1);
        let error = session.render_block(0, &mut vec![0.0; 1024]).unwrap_err();
        assert!(error.to_string().contains("1 graph submissions"), "{error}");
        assert_eq!(unsafe { graph::graph_edit_delivery_failures(engine.lg_ptr.0) }, 0,
            "the incomplete edit queue must never be applied");
        drop(session);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn graph_submission_overflow_is_fatal_even_when_caller_ignores_return_value() {
        let engine = sampler_engine();
        let mut session = OfflineAudioSession::new(&engine, 1000).unwrap();
        let event = graph::GraphBlockEvent {
            logical_id: 1, frame_offset: 0, sequence: 0, kind: graph::GBE_GATE_OFF,
            aux_count: 0, aux: [0.0; graph::GBE_AUX_CAP],
        };
        let mut rejected = false;
        for _ in 0..100_000 {
            if !unsafe { graph::push_block_event(engine.lg_ptr.0, event) } {
                rejected = true;
                break;
            }
        }
        assert!(rejected, "fixture must fill the actual graph queue");
        assert_eq!(unsafe { graph::graph_control_submission_failures(engine.lg_ptr.0) }, 1);
        let error = session.render_block(0, &mut vec![0.0; 1024]).unwrap_err();
        assert!(error.to_string().contains("graph submissions"), "{error}");
        drop(session);
        unsafe { engine.destroy(); }
    }
}
