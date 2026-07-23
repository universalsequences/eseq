//! Song playback control-side surface on `SequencerState`: preflight
//! (docs/song-mode-spec.md 10.1), the internal start/stop API handed to the
//! scheduler through the `SongPlaybackMailbox`, notice draining, and the
//! render-rate `song-position-beats` read (spec 10.2). Slice B wires the
//! transport UI to these; nothing here touches app-layer code.

use super::super::*;

/// Per-row data staged under one scenes lock so snapshot materialization can
/// run after the lock is dropped (snapshot capture takes other state locks).
struct RowStaging {
    resolved_pattern_ids: Vec<Option<PatternId>>,
    track_data: Vec<TrackPatternData>,
    silenced: Vec<bool>,
    mod_connections: Vec<ModConnection>,
    neural_networks: Vec<crate::neural::ProjectNeuralNetwork>,
    graph_overrides: Vec<crate::graph::ProjectGraphOverrides>,
    project_process_chain: crate::process::TrackProcessChain,
}

impl SequencerState {
    /// Build the immutable runtime song for the committed song (spec 10.1):
    /// validate every reference against the live project, resolve every
    /// effective per-track pattern (override else scene cell), and
    /// materialize one complete `Arc<SequencerSnapshot>` per row — all
    /// outside the audio callback. Fails with an actionable error before
    /// transport start if any row cannot be prepared (this is also the guard
    /// that catches song overrides left dangling by live pool renumbering).
    ///
    /// Callers should persist the live session into the current scene first
    /// (`save_current_pattern_snapshot`) so rows referencing the current
    /// scene resolve against up-to-date pattern data.
    pub fn preflight_runtime_song(&self) -> Result<Arc<RuntimeSong>, String> {
        let song = self
            .committed_song()
            .ok_or_else(|| "The project has no committed song".to_string())?;

        let staged: Vec<RowStaging> = {
            let scenes = self.pattern.scenes.lock().unwrap();
            song.validate(&*scenes)?;
            let track_count = scenes.track_pools.len();
            let placeholder = PatternSnapshot::new_default(1, &[])
                .track_pattern_data(0)
                .ok_or_else(|| "could not build a placeholder track pattern".to_string())?;
            song.rows
                .iter()
                .enumerate()
                .map(|(row_idx, row)| {
                    let scene = scenes.scenes.get(row.scene).ok_or_else(|| {
                        format!(
                            "Song row {} references scene {} which no longer exists",
                            row_idx + 1,
                            row.scene + 1
                        )
                    })?;
                    let mut resolved_pattern_ids = Vec::with_capacity(track_count);
                    let mut track_data = Vec::with_capacity(track_count);
                    let mut silenced = Vec::with_capacity(track_count);
                    for track in 0..track_count {
                        // An explicit-empty override (`pattern_id: None`)
                        // silences the track for the row; only an ABSENT
                        // override falls back to the scene cell.
                        let effective = match row
                            .overrides
                            .iter()
                            .find(|over| over.track == track)
                        {
                            Some(over) => over.pattern_id.map(PatternId),
                            None => scene.cells.get(track).copied().flatten(),
                        };
                        match effective {
                            Some(id) => {
                                let data = scenes
                                    .track_pools
                                    .get(track)
                                    .and_then(|pool| pool.get(id))
                                    .cloned()
                                    .ok_or_else(|| {
                                        format!(
                                            "Song row {} resolves track {} to pattern {} \
                                             which is not in the track's pattern pool; \
                                             update or clear the row",
                                            row_idx + 1,
                                            track + 1,
                                            id.0
                                        )
                                    })?;
                                resolved_pattern_ids.push(Some(id));
                                track_data.push(data);
                                silenced.push(false);
                            }
                            None => {
                                resolved_pattern_ids.push(None);
                                track_data.push(placeholder.clone());
                                silenced.push(true);
                            }
                        }
                    }
                    Ok(RowStaging {
                        resolved_pattern_ids,
                        track_data,
                        silenced,
                        mod_connections: scene.mod_connections.clone(),
                        neural_networks: scene.neural_networks.clone(),
                        graph_overrides: scene.graph_overrides.clone(),
                        project_process_chain: scene.project_process_chain.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        };

        let mut rows = Vec::with_capacity(song.rows.len());
        for (row, staging) in song.rows.iter().zip(staged) {
            let mut snapshot = SequencerSnapshot::capture_from_track_pattern_data(
                self,
                &staging.track_data,
                staging.mod_connections,
                staging.neural_networks,
                staging.graph_overrides,
                staging.project_process_chain,
            );
            // Row snapshots are only ever scheduled while the song transport
            // is playing; stamp them so the deterministic clock treats them
            // as playing regardless of the transport state at preflight time.
            snapshot.transport.playing = true;
            snapshot.transport.current_pattern = row.scene;
            for (track, silenced) in staging.silenced.iter().enumerate() {
                if *silenced {
                    let mut track_snapshot = (*snapshot.tracks[track]).clone();
                    track_snapshot.scene_silenced = true;
                    snapshot.tracks[track] = Arc::new(track_snapshot);
                }
            }
            rows.push(RuntimeSongRow {
                id: row.id,
                start_beat: row.start_beat,
                scene: row.scene,
                overrides: row
                    .overrides
                    .iter()
                    .map(|over| (over.track, over.pattern_id.map(PatternId)))
                    .collect(),
                resolved_pattern_ids: staging.resolved_pattern_ids,
                scheduler_snapshot: Arc::new(snapshot),
            });
        }
        Ok(Arc::new(RuntimeSong {
            rows,
            end_beat: song.end_beat,
            loop_enabled: song.loop_enabled,
        }))
    }

    pub fn song_playback(&self) -> &SongPlaybackMailbox {
        &self.song_playback
    }

    /// Hand a preflighted song to the scheduler thread. The initial row is
    /// found via `state_at_beat` semantics on the runtime rows; V1 callers
    /// pass `start_beat = 0.0` (spec 10.1). The scheduler installs the song
    /// and owns every subsequent row transition; callers start the transport
    /// separately (Slice B).
    pub fn start_song_playback(
        &self,
        song: Arc<RuntimeSong>,
        start_beat: f64,
    ) -> Result<(), String> {
        // Validate eagerly so the caller gets the error, not the scheduler.
        // The nominal samples-per-quarter only has to be positive here; the
        // scheduler rebuilds the runtime with its real tempo mapping.
        SongPlaybackRuntime::new(Arc::clone(&song), start_beat, 1.0)?;
        self.song_playback
            .send_command(SongPlaybackCommand::Start { song, start_beat })
    }

    /// Tear down scheduler-side song playback. Callers stop the transport
    /// separately (Slice B).
    pub fn stop_song_playback(&self) -> Result<(), String> {
        self.song_playback.send_command(SongPlaybackCommand::Stop)
    }

    /// Control side: drain scheduler-authoritative row-applied / ended
    /// notices. The control thread mirrors each `RowApplied` through
    /// `apply_song_row` for UI-visible state and stops the transport on
    /// `Ended`; the audible transition never waits on this.
    pub fn drain_song_playback_notices(&self) -> Vec<SongPlaybackNotice> {
        self.song_playback.drain_notices()
    }

    /// Render-rate `song-position-beats` read (spec 10.2): derived from the
    /// scheduler-published anchor atomics plus the audio-published rendered
    /// sample clock. No locks. `None` while song playback is inactive.
    pub fn song_position_beats(&self) -> Option<f64> {
        let rendered = self.audio_rendered_sample.load(Ordering::Acquire);
        self.song_playback.shared().position_beats(rendered)
    }
}
