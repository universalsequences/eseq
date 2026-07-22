use super::super::*;

impl SequencerState {
    pub fn current_step(&self) -> usize {
        self.transport.playhead.load(Ordering::Relaxed) as usize
    }
    pub fn track_step(&self, track: usize) -> usize {
        self.transport.track_playheads[track].load(Ordering::Relaxed) as usize
    }

    /// Resolve a track-local step and phase from the transport beat clock.
    /// This mirrors the scheduler's timebase-override and sync-boundary rules
    /// so live recording does not accidentally use the global 16th-note phase.
    pub fn record_position_at_beat(&self, track: usize, beats: f64) -> Option<RecordPosition> {
        if track >= self.active_track_count() || !beats.is_finite() {
            return None;
        }
        let params = &self.pattern.track_params[track];
        let num_steps = params.get_num_steps().clamp(1, MAX_STEPS);
        let default_timebase = params.get_timebase();
        let mut boundaries = [0.0_f64; MAX_STEPS + 1];
        let mut step_ends = [0.0_f64; MAX_STEPS];
        let mut accumulated = 0.0_f64;
        for step in 0..num_steps {
            let timebase = self.pattern.timebase_plocks[track]
                .get(step)
                .unwrap_or(default_timebase);
            let sync = sync_beats(self.pattern.step_data[track].get(step, StepParam::Sync));
            if sync > f64::EPSILON {
                accumulated = (accumulated / sync).ceil() * sync;
            }
            boundaries[step] = accumulated;
            let step_beats = timebase.step_beats(num_steps).max(f64::EPSILON);
            step_ends[step] = accumulated + step_beats;
            accumulated += step_beats;
        }
        boundaries[num_steps] = accumulated;
        let initial_sync = sync_beats(self.pattern.step_data[track].get(0, StepParam::Sync));
        let cycle_beats = if initial_sync > f64::EPSILON {
            (accumulated / initial_sync).ceil() * initial_sync
        } else {
            accumulated
        }
        .max(f64::EPSILON);
        let position = beats.max(0.0) % cycle_beats;
        let idx = boundaries[..=num_steps].partition_point(|&boundary| boundary <= position);
        let step = idx.saturating_sub(1).min(num_steps - 1);
        (position < step_ends[step]).then(|| RecordPosition {
            step,
            phase: ((position - boundaries[step]) / (step_ends[step] - boundaries[step]))
                .clamp(0.0, 1.0) as f32,
        })
    }

    /// Interpolate the audio clock at a keyboard press and compensate the
    /// configured render-ahead latency before resolving a track-local phase.
    pub fn record_position_at_instant(
        &self,
        track: usize,
        timestamp: Instant,
    ) -> Option<RecordPosition> {
        let (anchor_beats, elapsed) = self.transport.record_clock.sample(timestamp)?;
        let bpm = self.transport.bpm.load(Ordering::Relaxed) as f64;
        let latency_seconds = f32::from_bits(
            self.transport
                .record_latency_seconds
                .load(Ordering::Relaxed),
        )
        .max(0.0) as f64;
        let beats =
            anchor_beats + elapsed.as_secs_f64() * bpm / 60.0 - latency_seconds * bpm / 60.0;
        self.record_position_at_beat(track, beats)
    }

    pub fn is_playing(&self) -> bool {
        self.transport.playing.load(Ordering::Relaxed)
    }

    pub fn start_playback(&self) {
        self.reset_playheads();
        self.transport.playing.store(true, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn stop_playback(&self) {
        self.transport.playing.store(false, Ordering::Relaxed);
        self.reset_playheads();
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn toggle_play(&self) -> bool {
        let playing = self.toggle_play_no_publish();
        self.publish_scheduler_snapshot();
        playing
    }

    pub(crate) fn toggle_play_no_publish(&self) -> bool {
        if self.is_playing() {
            self.transport.playing.store(false, Ordering::Relaxed);
            self.reset_playheads();
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            self.reset_playheads();
            self.transport.playing.store(true, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    pub fn reset_playheads(&self) {
        self.transport.playhead.store(0, Ordering::Relaxed);
        self.transport
            .playhead_phase
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        for playhead in &self.transport.track_playheads {
            playhead.store(0, Ordering::Relaxed);
        }
        for phase in &self.transport.track_playhead_phases {
            phase.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        for playhead in &self.transport.sampler_playheads {
            playhead.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }
    /// Publish a snapshot of all pattern/transport atomics so that future
    /// snapshot-based audio-thread readers can pick up the latest state.
    ///
    /// Currently this is a **no-op** because the audio thread reads atomics
    /// directly from `SequencerState`.  The method exists as a hook for the
    /// planned `Arc<SequencerSnapshot>` architecture — once that lands, this
    pub fn schedule_mod_resync(&self) {
        if self.is_playing() {
            self.transport
                .pending_mod_resync
                .store(true, Ordering::Relaxed);
        } else {
            self.transport
                .mod_reset_counter
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}
