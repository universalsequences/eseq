use super::super::*;

impl SequencerState {
    pub fn latest_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        self.scheduler_snapshot.lock().unwrap().clone()
    }

    pub fn set_neural_visualization(&self, snapshot: NeuralVisualizationSnapshot) {
        *self.neural_visualization.lock().unwrap() = snapshot;
    }

    pub fn neural_visualization(&self) -> NeuralVisualizationSnapshot {
        self.neural_visualization.lock().unwrap().clone()
    }

    pub fn has_neural_visualization(&self) -> bool {
        self.neural_visualization.lock().unwrap().num_neurons > 0
    }

    pub fn set_graph_visualizations(&self, snapshots: Vec<GraphVisualizationSnapshot>) {
        *self.graph_visualizations.lock().unwrap() = snapshots;
    }

    pub fn graph_visualizations(&self) -> Vec<GraphVisualizationSnapshot> {
        self.graph_visualizations.lock().unwrap().clone()
    }

    pub fn has_graph_visualizations(&self) -> bool {
        !self.graph_visualizations.lock().unwrap().is_empty()
    }

    pub fn push_graph_control_command(&self, command: crate::graph::GraphControlCommand) {
        self.graph_control_commands.lock().unwrap().push(command);
    }

    pub fn drain_graph_control_commands(&self) -> Vec<crate::graph::GraphControlCommand> {
        std::mem::take(&mut *self.graph_control_commands.lock().unwrap())
    }

    pub fn push_roll_command(&self, command: crate::sequencer::RollCommand) {
        self.roll_commands.lock().unwrap().push(command);
    }

    pub fn drain_roll_commands(&self) -> Vec<crate::sequencer::RollCommand> {
        std::mem::take(&mut *self.roll_commands.lock().unwrap())
    }

    pub fn push_roll_recorded_hit(&self, hit: crate::sequencer::RollHitRecorded) {
        let mut hits = self.roll_recorded_hits.lock().unwrap();
        // Backstop for a control thread that never drains (headless
        // schedulers): recording cannot be on there, so dropping is safe.
        if hits.len() < 4096 {
            hits.push(hit);
        }
    }

    pub fn drain_roll_recorded_hits(&self) -> Vec<crate::sequencer::RollHitRecorded> {
        std::mem::take(&mut *self.roll_recorded_hits.lock().unwrap())
    }

    /// Realtime-safe: called from the audio callback for every live note-on
    /// it consumes while the transport plays (bead eseq-2awi).
    pub fn push_live_trigger_stamp(&self, track: usize, transpose: f32, beat: f64) {
        self.live_trigger_stamps.push(track, transpose, beat);
    }

    /// Control-thread consumer of the live note-on stamps; see
    /// [`crate::sequencer::LiveTriggerStampRing`].
    pub fn drain_live_trigger_stamps(&self, consume: impl FnMut(crate::sequencer::LiveTriggerStamp)) {
        self.live_trigger_stamps.drain(consume);
    }

    pub fn append_track_output_events(&self, events: impl IntoIterator<Item = TrackOutputEvent>) {
        let mut history = self.track_output_events.lock().unwrap();
        history.extend(events);
        let overflow = history.len().saturating_sub(TRACK_OUTPUT_EVENT_HISTORY_CAP);
        if overflow > 0 {
            history.drain(0..overflow);
        }
    }

    pub fn clear_track_output_events(&self) {
        self.track_output_events.lock().unwrap().clear();
    }

    pub fn track_output_events(&self) -> Vec<TrackOutputEvent> {
        self.track_output_events.lock().unwrap().clone()
    }

    pub fn has_track_output_events(&self) -> bool {
        !self.track_output_events.lock().unwrap().is_empty()
    }

    pub fn set_track_output_current_beat(&self, beat: f64) {
        self.track_output_current_beat_bits
            .store(beat.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn track_output_current_beat(&self) -> f64 {
        f64::from_bits(self.track_output_current_beat_bits.load(Ordering::Relaxed))
    }

    /// Publish the audio clock used to expire scheduled-note activity without
    /// taking a lock on the realtime thread.
    pub fn set_audio_rendered_sample(&self, sample: u64) {
        self.audio_rendered_sample.store(sample, Ordering::Release);
    }

    /// The audio clock published above — the "now" that due-ness of sequenced
    /// mixer controls is measured against on the app thread.
    pub fn audio_rendered_sample(&self) -> u64 {
        self.audio_rendered_sample.load(Ordering::Acquire)
    }

    /// Publish the scheduler's rendered-beat clock (the `rendered_beats`
    /// domain quantized-launch deadlines live in). Called once per scheduler
    /// loop from scheduler/worker.rs; song capture reads it back for
    /// immediate launches (docs/song-mode-spec.md 8.2).
    pub fn set_scheduler_rendered_beats(&self, beats: f64) {
        self.scheduler_rendered_beats_bits
            .store(beats.to_bits(), Ordering::Release);
    }

    /// The last published scheduler rendered-beat position (see
    /// `set_scheduler_rendered_beats`).
    pub fn scheduler_rendered_beats(&self) -> f64 {
        f64::from_bits(self.scheduler_rendered_beats_bits.load(Ordering::Acquire))
    }

    /// Keep a scheduled MIDI note active through its gate end. `fetch_max`
    /// preserves overlapping/retriggered instances of the same pitch.
    pub fn mark_scheduled_note_active_until(
        &self,
        track: usize,
        note: u8,
        sample: u64,
        velocity: f32,
    ) {
        let (Some(notes), Some(velocities)) = (
            self.active_note_until_samples.get(track),
            self.active_note_velocity_bits.get(track),
        ) else {
            return;
        };
        velocities[note as usize].store(velocity.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.mark_note_trigger(track, note);
        notes[note as usize].fetch_max(sample, Ordering::Release);
    }

    /// Publish a live note-on independently from the live active-note snapshot.
    /// This preserves retrigger edges when a pitch remains active continuously.
    pub fn mark_live_note_trigger(&self, track: usize, note: u8) {
        self.mark_note_trigger(track, note);
    }

    fn mark_note_trigger(&self, track: usize, note: u8) {
        let Some(trigger_ids) = self.active_note_trigger_ids.get(track) else {
            return;
        };
        let trigger_id = self
            .active_note_trigger_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        trigger_ids[note as usize].store(trigger_id, Ordering::Release);
    }

    /// Live notes have explicit note-off events, so replace their velocity
    /// table independently from scheduled expirations. Zero means inactive;
    /// the two sources can overlap.
    pub fn replace_live_notes(
        &self,
        track: usize,
        notes: impl IntoIterator<Item = (u8, f32)>,
    ) {
        let Some(velocities) = self.live_note_velocity_bits.get(track) else {
            return;
        };
        let mut next_velocities = [0.0_f32; 128];
        for (note, velocity) in notes {
            next_velocities[note as usize] =
                next_velocities[note as usize].max(velocity.clamp(0.0, 1.0));
        }
        for (slot, velocity) in velocities.iter().zip(next_velocities) {
            slot.store(velocity.to_bits(), Ordering::Release);
        }
    }

    pub fn active_notes(&self, track: usize) -> Vec<u8> {
        self.active_note_activity(track)
            .into_iter()
            .map(|activity| activity.note)
            .collect()
    }

    pub fn active_note_activity(&self, track: usize) -> Vec<ActiveNoteActivity> {
        let (
            Some(until),
            Some(scheduled_velocities),
            Some(live_velocities),
            Some(trigger_ids),
        ) = (
            self.active_note_until_samples.get(track),
            self.active_note_velocity_bits.get(track),
            self.live_note_velocity_bits.get(track),
            self.active_note_trigger_ids.get(track),
        ) else {
            return Vec::new();
        };
        let rendered = self.audio_rendered_sample.load(Ordering::Acquire);
        (0_u8..=127)
            .filter_map(|note| {
                let idx = note as usize;
                let scheduled_active = until[idx].load(Ordering::Acquire) > rendered;
                let live_velocity =
                    f32::from_bits(live_velocities[idx].load(Ordering::Acquire)).clamp(0.0, 1.0);
                if live_velocity <= 0.0 && !scheduled_active {
                    return None;
                }
                let scheduled_velocity = if scheduled_active {
                    f32::from_bits(scheduled_velocities[idx].load(Ordering::Relaxed))
                } else {
                    0.0
                };
                Some(ActiveNoteActivity {
                    note,
                    velocity: scheduled_velocity.max(live_velocity).clamp(0.0, 1.0),
                    trigger_id: trigger_ids[idx].load(Ordering::Acquire),
                })
            })
            .collect()
    }

    pub fn publish_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        if self.publish_coalesce_depth.load(Ordering::Acquire) > 0 {
            // Inside a coalescing scope (see `coalesce_publishes`): record the
            // intent and let the scope's exit pay for one capture. Every
            // mutation this publish would have described is still in live
            // state, so the deferred full capture supersedes it.
            self.pending_coalesced_publish.store(true, Ordering::Release);
            return self.latest_scheduler_snapshot();
        }
        let snapshot = Arc::new(SequencerSnapshot::capture(self));
        self.publish_scheduler_snapshot_arc(snapshot)
    }

    /// Run `body` with scheduler-snapshot publications coalesced into a single
    /// capture performed at the end (bead eseq-sj01).
    ///
    /// Transport transitions like `App::song_transport_stop` publish several
    /// times in a row — `stop_playback`, then `resync_live_grid_to_current_scene`
    /// after its epoch bumps — and every publication costs a whole-project deep
    /// capture on the control thread plus a whole-project deep free once the
    /// audio thread lets go. Only the last one describes the state the user
    /// ends up in.
    ///
    /// The deferred publish runs after `body` returns, so it observes every
    /// epoch bump the body performed. Scopes nest; only the outermost publishes.
    pub fn coalesce_publishes<F, R>(&self, body: F) -> R
    where
        F: FnOnce() -> R,
    {
        struct Scope<'a>(&'a SequencerState);
        impl Drop for Scope<'_> {
            fn drop(&mut self) {
                if self.0.publish_coalesce_depth.fetch_sub(1, Ordering::AcqRel) == 1
                    && self.0.pending_coalesced_publish.swap(false, Ordering::AcqRel)
                {
                    self.0.publish_scheduler_snapshot();
                }
            }
        }
        self.publish_coalesce_depth.fetch_add(1, Ordering::AcqRel);
        let scope = Scope(self);
        let result = body();
        drop(scope);
        result
    }

    /// Publish a transport-only change through a copy-on-write snapshot
    /// (bead eseq-sj01).
    ///
    /// `start_playback`, `stop_playback` and `toggle_play` mutate transport
    /// atomics only, so every track's captured payload is unchanged and its
    /// existing `Arc` can be reused exactly the way `publish_scheduler_track`
    /// reuses the tracks it did not edit. That removes one whole-project deep
    /// capture from the control thread AND one whole-project deep free from the
    /// audio thread per play/stop.
    ///
    /// Falls back to a full capture when the published track count disagrees
    /// with `active_track_count()` — the same guard `publish_scheduler_track`
    /// uses, since a stale track vector must not be republished as current.
    pub fn publish_transport_only(&self) -> Arc<SequencerSnapshot> {
        if self.publish_coalesce_depth.load(Ordering::Acquire) > 0 {
            self.pending_coalesced_publish.store(true, Ordering::Release);
            return self.latest_scheduler_snapshot();
        }
        let current = self.scheduler_snapshot.lock().unwrap().clone();
        if current.tracks.len() != self.active_track_count() {
            return self.publish_scheduler_snapshot();
        }
        let mut next = (*current).clone();
        next.transport = self.capture_transport_snapshot();
        self.publish_scheduler_snapshot_arc(Arc::new(next))
    }

    fn capture_transport_snapshot(&self) -> SequencerTransportSnapshot {
        SequencerTransportSnapshot {
            bpm: self.transport.bpm.load(Ordering::Relaxed),
            playing: self.transport.playing.load(Ordering::Relaxed),
            current_pattern: self.current_scene_index(),
            pattern_epoch: self.transport.pattern_epoch.load(Ordering::Relaxed),
            topology_epoch: self.transport.topology_epoch.load(Ordering::Relaxed),
            num_tracks: self.active_track_count(),
        }
    }

    /// Free every snapshot the audio thread retired. Non-realtime callers only;
    /// returns how many were freed (bead eseq-sj01).
    pub fn drain_retired_scheduler_snapshots(&self) -> usize {
        self.snapshot_handoff.drain_retired()
    }

    /// Handoff rings between the publishers, the audio callback and the
    /// reclaimer (bead eseq-sj01).
    pub fn snapshot_handoff(&self) -> &SchedulerSnapshotHandoff {
        &self.snapshot_handoff
    }

    /// Publish one complete track through a copy-on-write scheduler snapshot.
    /// Unchanged tracks keep their existing `Arc`, while the edited track is
    /// recaptured with its step payloads, device p-locks, and process state.
    pub fn publish_scheduler_track(&self, track: usize) -> Arc<SequencerSnapshot> {
        let current = self.scheduler_snapshot.lock().unwrap().clone();
        if track >= self.active_track_count()
            || track >= current.tracks.len()
            || current.tracks.len() != self.active_track_count()
        {
            return self.publish_scheduler_snapshot();
        }
        let mut next = (*current).clone();
        let Some(next_track) = SequencerSnapshot::capture_live_track(self, track) else {
            return self.publish_scheduler_snapshot();
        };
        next.tracks[track] = Arc::new(next_track);
        next.transport = self.capture_transport_snapshot();
        self.publish_scheduler_snapshot_arc(Arc::new(next))
    }

    /// Replaces the command-thread macro layer and immediately publishes a
    /// scheduler snapshot containing those effective defaults.
    pub fn publish_macro_overrides(
        &self,
        overrides: HashMap<crate::macro_engine::MacroParamKey, f32>,
    ) -> Arc<SequencerSnapshot> {
        *self.live_macro_overrides.lock().unwrap() = overrides;
        self.publish_scheduler_snapshot()
    }

    pub(in crate::sequencer) fn live_macro_overrides(&self) -> HashMap<crate::macro_engine::MacroParamKey, f32> {
        self.live_macro_overrides.lock().unwrap().clone()
    }

    fn publish_scheduler_snapshot_arc(
        &self,
        snapshot: Arc<SequencerSnapshot>,
    ) -> Arc<SequencerSnapshot> {
        // Free whatever the audio thread retired since the last publish. This
        // thread is already non-realtime and already about to allocate a
        // capture, so it is the cheapest reliable reclamation point; the
        // scheduler worker drains too, for the case where nothing publishes.
        self.snapshot_handoff.drain_retired();
        {
            let mut published = self.scheduler_snapshot.lock().unwrap();
            *published = Arc::clone(&snapshot);
            // Version bump and ring push happen under the same lock the store
            // does, so concurrent publishers cannot interleave a lower version
            // behind a higher one in the ring. Non-realtime readers still
            // observe the counter after the store, exactly as before, and the
            // audio thread reads only the ring — never this mutex.
            let version = self
                .scheduler_snapshot_version
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            self.snapshot_handoff
                .publish(version, Arc::clone(&snapshot));
        }
        snapshot
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn publish_scheduler_snapshot_from_track_pattern_data(
        &self,
        tracks: &[TrackPatternData],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
        scene_slots: SceneSlotStore,
        project_process_chain: crate::process::TrackProcessChain,
    ) -> Arc<SequencerSnapshot> {
        self.publish_scheduler_snapshot_arc(Arc::new(
            SequencerSnapshot::capture_from_track_pattern_data(
                self,
                tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                scene_slots,
                project_process_chain,
            ),
        ))
    }

    pub fn current_neural_networks(&self) -> Vec<ProjectNeuralNetwork> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_neural_networks()
    }

    pub fn edit_current_neural_networks<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectNeuralNetwork>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_neural_networks(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }

    pub fn current_graph_overrides(&self) -> Vec<ProjectGraphOverrides> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_graph_overrides()
    }

    pub fn edit_current_graph_overrides<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectGraphOverrides>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_graph_overrides(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }
}
