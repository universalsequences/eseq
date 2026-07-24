//! Song playback control-side surface on `SequencerState`: preflight
//! (docs/song-mode-spec.md 10.1), the internal start/stop API handed to the
//! scheduler through the `SongPlaybackMailbox`, notice draining, and the
//! render-rate `song-position-beats` read (spec 10.2). Slice B wires the
//! transport UI to these; nothing here touches app-layer code.

use super::super::*;

/// Per-row data staged under one scenes lock so snapshot materialization can
/// run after the lock is dropped (snapshot capture takes other state locks).
struct RowStaging {
    id: SongRowId,
    start_beat: f64,
    scene: usize,
    overrides: Vec<(usize, Option<PatternId>)>,
    resolved_pattern_ids: Vec<Option<PatternId>>,
    resolved_sources: Vec<LaneSource>,
    lane_offsets: Vec<f64>,
    track_data: Vec<TrackPatternData>,
    silenced: Vec<bool>,
    mod_connections: Vec<ModConnection>,
    neural_networks: Vec<crate::neural::ProjectNeuralNetwork>,
    graph_overrides: Vec<crate::graph::ProjectGraphOverrides>,
    project_process_chain: crate::process::TrackProcessChain,
}

/// One track's resolved lane inside a project row, before chunk expansion.
enum LaneResolution {
    Silent,
    Pattern {
        id: PatternId,
        offset_steps: f64,
        /// `(steps_per_beat, num_steps)` under the pattern's base timebase —
        /// the `steps()` mapping offsets are stamped in (takes spec 7.2).
        mapping: (f64, f64),
    },
    Take {
        id: TakeId,
        chunks: Vec<PatternId>,
        total_len_steps: f64,
        offset_steps: f64,
        /// Steps-per-beat of the take's chunk domain (chunks are
        /// `MAX_STEPS`-long patterns; the first chunk's base timebase
        /// defines the take's `steps()` mapping).
        steps_per_beat: f64,
    },
}

/// Snap floating-point step positions that landed within epsilon of an
/// integer (chunk-boundary beats are derived from step counts, so the
/// round-trip must not put a boundary on the wrong side of `floor`).
fn snap_steps(p: f64) -> f64 {
    let rounded = p.round();
    if (p - rounded).abs() < 1e-6 {
        rounded
    } else {
        p
    }
}

impl SequencerState {
    /// Build the immutable runtime song for the committed song (spec 10.1):
    /// validate every reference against the live project, resolve every
    /// effective per-track source (override else scene cell), and
    /// materialize one complete `Arc<SequencerSnapshot>` per runtime row —
    /// all outside the audio callback.
    ///
    /// Take lanes (takes spec 6.1/7.3) are resolved by CHUNK EXPANSION: a
    /// project row whose take content crosses a chunk boundary (or the take
    /// end) is split into several runtime rows sharing the project row's id,
    /// each carrying the governing chunk's pattern as its content with the
    /// chunk-local step offset. Every other lane's offset advances across
    /// the synthetic split (`steps(delta)`), so the expansion is
    /// phase-transparent and — because `resolved_sources` carries the
    /// `TakeId`, not the chunk pattern id — accumulator-transparent. The
    /// audio-thread scheduler needs no take awareness at all.
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
            let mut staged = Vec::with_capacity(song.rows.len());
            for (row_idx, row) in song.rows.iter().enumerate() {
                let scene = scenes.scenes.get(row.scene).ok_or_else(|| {
                    format!(
                        "Song row {} references scene {} which no longer exists",
                        row_idx + 1,
                        row.scene + 1
                    )
                })?;
                let row_end = song
                    .rows
                    .get(row_idx + 1)
                    .map(|next| next.start_beat)
                    .unwrap_or(song.end_beat);

                // Resolve every lane of the project row once.
                let mut lanes = Vec::with_capacity(track_count);
                for track in 0..track_count {
                    let override_entry = row
                        .overrides
                        .iter()
                        .find(|over| over.track == track);
                    let (source, offset_steps) = match override_entry {
                        Some(over) => (over.source(), over.offset_steps),
                        None => (
                            scene
                                .cells
                                .get(track)
                                .copied()
                                .flatten()
                                .map(LaneSource::Pattern)
                                .unwrap_or(LaneSource::Empty),
                            0.0,
                        ),
                    };
                    let lane = match source {
                        LaneSource::Empty => LaneResolution::Silent,
                        LaneSource::Pattern(id) => {
                            let data = scenes
                                .track_pools
                                .get(track)
                                .and_then(|pool| pool.get(id))
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
                            let num_steps = data.track_params.num_steps.max(1) as usize;
                            let step_beats =
                                data.track_params.timebase.step_beats(num_steps);
                            let mapping = if step_beats > 0.0 {
                                (1.0 / step_beats, num_steps as f64)
                            } else {
                                (0.0, num_steps as f64)
                            };
                            LaneResolution::Pattern {
                                id,
                                offset_steps,
                                mapping,
                            }
                        }
                        LaneSource::Take(take_id) => {
                            let take = scenes
                                .take_pools
                                .get(track)
                                .and_then(|takes| takes.get(take_id))
                                .ok_or_else(|| {
                                    format!(
                                        "Song row {} resolves track {} to take {} which \
                                         is not in the track's take pool; update or \
                                         clear the row",
                                        row_idx + 1,
                                        track + 1,
                                        take_id.0
                                    )
                                })?;
                            let first_chunk = take
                                .chunks
                                .first()
                                .and_then(|id| {
                                    scenes.track_pools.get(track).and_then(|pool| pool.get(*id))
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "Take {} on track {} has no resolvable chunk \
                                         patterns",
                                        take_id.0,
                                        track + 1
                                    )
                                })?;
                            // The take's steps() mapping is the chunk domain:
                            // MAX_STEPS-long patterns under the chunk's base
                            // timebase (takes spec 6.1).
                            let step_beats =
                                first_chunk.track_params.timebase.step_beats(MAX_STEPS);
                            LaneResolution::Take {
                                id: take_id,
                                chunks: take.chunks.clone(),
                                total_len_steps: take.total_len_steps as f64,
                                offset_steps,
                                steps_per_beat: if step_beats > 0.0 {
                                    1.0 / step_beats
                                } else {
                                    0.0
                                },
                            }
                        }
                    };
                    lanes.push(lane);
                }

                // Chunk expansion (takes spec 7.3): split this row wherever a
                // take lane crosses a chunk boundary or its end.
                let mut splits: Vec<f64> = Vec::new();
                for lane in &lanes {
                    let LaneResolution::Take {
                        chunks,
                        total_len_steps,
                        offset_steps,
                        steps_per_beat,
                        ..
                    } = lane
                    else {
                        continue;
                    };
                    if *steps_per_beat <= 0.0 {
                        continue;
                    }
                    let step_beats = 1.0 / steps_per_beat;
                    let span_steps = (row_end - row.start_beat) * steps_per_beat;
                    let end_p = (offset_steps + span_steps).min(*total_len_steps);
                    let mut chunk = (snap_steps(*offset_steps) / MAX_STEPS as f64).floor()
                        as usize
                        + 1;
                    while chunk <= chunks.len() {
                        let boundary_p = (chunk * MAX_STEPS) as f64;
                        if boundary_p >= end_p - 1e-6 {
                            break;
                        }
                        splits.push(row.start_beat + (boundary_p - offset_steps) * step_beats);
                        chunk += 1;
                    }
                    // Take end inside the row span: the tail is silent.
                    if *total_len_steps < offset_steps + span_steps - 1e-6 {
                        splits
                            .push(row.start_beat + (total_len_steps - offset_steps) * step_beats);
                    }
                }
                splits.retain(|beat| {
                    *beat > row.start_beat + 1e-9 && *beat < row_end - 1e-9
                });
                splits.sort_by(|a, b| a.partial_cmp(b).expect("split beats are finite"));
                splits.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

                let mut sub_starts = Vec::with_capacity(splits.len() + 1);
                sub_starts.push(row.start_beat);
                sub_starts.extend(splits);

                for sub_start in sub_starts {
                    let delta_beats = sub_start - row.start_beat;
                    let mut resolved_pattern_ids = Vec::with_capacity(track_count);
                    let mut resolved_sources = Vec::with_capacity(track_count);
                    let mut lane_offsets = Vec::with_capacity(track_count);
                    let mut track_data = Vec::with_capacity(track_count);
                    let mut silenced = Vec::with_capacity(track_count);
                    let mut overrides: Vec<(usize, Option<PatternId>)> = row
                        .overrides
                        .iter()
                        .map(|over| (over.track, over.pattern_id.map(PatternId)))
                        .collect();
                    for (track, lane) in lanes.iter().enumerate() {
                        match lane {
                            LaneResolution::Silent => {
                                resolved_pattern_ids.push(None);
                                resolved_sources.push(LaneSource::Empty);
                                lane_offsets.push(0.0);
                                track_data.push(placeholder.clone());
                                silenced.push(true);
                            }
                            LaneResolution::Pattern {
                                id,
                                offset_steps,
                                mapping: (steps_per_beat, num_steps),
                            } => {
                                let advanced = if delta_beats > 0.0 && *steps_per_beat > 0.0 {
                                    snap_steps(offset_steps + delta_beats * steps_per_beat)
                                        .rem_euclid(*num_steps)
                                } else {
                                    *offset_steps
                                };
                                let data = scenes
                                    .track_pools
                                    .get(track)
                                    .and_then(|pool| pool.get(*id))
                                    .cloned()
                                    .expect("pattern lane resolved above");
                                resolved_pattern_ids.push(Some(*id));
                                resolved_sources.push(LaneSource::Pattern(*id));
                                lane_offsets.push(advanced);
                                track_data.push(data);
                                silenced.push(false);
                            }
                            LaneResolution::Take {
                                id,
                                chunks,
                                total_len_steps,
                                offset_steps,
                                steps_per_beat,
                            } => {
                                let p =
                                    snap_steps(offset_steps + delta_beats * steps_per_beat);
                                let chunk_idx = (p / MAX_STEPS as f64).floor() as usize;
                                let audible = p < total_len_steps - 1e-6
                                    && chunk_idx < chunks.len();
                                let mirror = overrides
                                    .iter_mut()
                                    .find(|(over_track, _)| *over_track == track);
                                if audible {
                                    let chunk_id = chunks[chunk_idx];
                                    let data = scenes
                                        .track_pools
                                        .get(track)
                                        .and_then(|pool| pool.get(chunk_id))
                                        .cloned()
                                        .ok_or_else(|| {
                                            format!(
                                                "Take {} on track {} references chunk \
                                                 pattern {} which is not in the track's \
                                                 pattern pool",
                                                id.0,
                                                track + 1,
                                                chunk_id.0
                                            )
                                        })?;
                                    resolved_pattern_ids.push(Some(chunk_id));
                                    resolved_sources.push(LaneSource::Take(*id));
                                    lane_offsets.push(p - (chunk_idx * MAX_STEPS) as f64);
                                    track_data.push(data);
                                    silenced.push(false);
                                    if let Some((_, mirror_id)) = mirror {
                                        *mirror_id = Some(chunk_id);
                                    }
                                } else {
                                    // Past the take end: silent, never
                                    // wrapped (takes spec 6.1).
                                    resolved_pattern_ids.push(None);
                                    resolved_sources.push(LaneSource::Empty);
                                    lane_offsets.push(0.0);
                                    track_data.push(placeholder.clone());
                                    silenced.push(true);
                                    if let Some((_, mirror_id)) = mirror {
                                        *mirror_id = None;
                                    }
                                }
                            }
                        }
                    }
                    staged.push(RowStaging {
                        id: row.id,
                        start_beat: sub_start,
                        scene: row.scene,
                        overrides,
                        resolved_pattern_ids,
                        resolved_sources,
                        lane_offsets,
                        track_data,
                        silenced,
                        mod_connections: scene.mod_connections.clone(),
                        neural_networks: scene.neural_networks.clone(),
                        graph_overrides: scene.graph_overrides.clone(),
                        project_process_chain: scene.project_process_chain.clone(),
                    });
                }
            }
            staged
        };

        let mut rows = Vec::with_capacity(staged.len());
        for staging in staged {
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
            snapshot.transport.current_pattern = staging.scene;
            for (track, silenced) in staging.silenced.iter().enumerate() {
                if *silenced {
                    let mut track_snapshot = (*snapshot.tracks[track]).clone();
                    track_snapshot.scene_silenced = true;
                    snapshot.tracks[track] = Arc::new(track_snapshot);
                }
            }
            rows.push(RuntimeSongRow {
                id: staging.id,
                start_beat: staging.start_beat,
                scene: staging.scene,
                overrides: staging.overrides,
                resolved_pattern_ids: staging.resolved_pattern_ids,
                resolved_sources: staging.resolved_sources,
                lane_offsets: staging.lane_offsets,
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

    /// Manual-override latch bitmask (takes spec 10). While a track's bit is
    /// set, the scheduler schedules it from the live session snapshot
    /// (free-running) instead of the active song row.
    pub fn song_manual_latch_mask(&self) -> u64 {
        self.song_manual_latch.load(Ordering::Acquire)
    }

    /// Latch specific tracks (a manual track launch during song playback).
    pub fn latch_song_manual_override(&self, tracks: impl IntoIterator<Item = usize>) {
        let mut bits = 0u64;
        for track in tracks {
            if track < 64 {
                bits |= 1 << track;
            }
        }
        if bits != 0 {
            self.song_manual_latch.fetch_or(bits, Ordering::AcqRel);
            // A latched lane plays the performer's launch, not the row's
            // take — the clip grid must show the launched clip again.
            self.song_take_lane_mask.fetch_and(!bits, Ordering::AcqRel);
        }
    }

    /// Which lanes the currently mirrored song row resolves to a take chunk
    /// (takes spec 11.2 UX). Written by the control-side row mirror.
    pub fn song_take_lane_mask(&self) -> u64 {
        self.song_take_lane_mask.load(Ordering::Acquire)
    }

    pub fn set_song_take_lane_mask(&self, mask: u64) {
        self.song_take_lane_mask.store(mask, Ordering::Release);
    }

    /// Latch every track (a manual scene launch latches globally, spec 10).
    pub fn latch_song_manual_override_all(&self, track_count: usize) {
        self.latch_song_manual_override(0..track_count.min(64));
    }

    /// Back to Song / punch-out: the song resumes launch authority for
    /// every lane (takes spec 10). Transient state; never serialized.
    pub fn clear_song_manual_latch(&self) {
        self.song_manual_latch.store(0, Ordering::Release);
        // Every caller is a transport boundary (stop, cancel, punch-out) or
        // Back to Song, which re-applies the current row immediately after
        // (recomputing the mask) — so the take-lane bits reset here too.
        self.song_take_lane_mask.store(0, Ordering::Release);
    }

    /// Per-track Back to Song (takes spec 10 UX): the song resumes launch
    /// authority for one lane while other latched lanes stay the performer's.
    pub fn clear_song_manual_latch_track(&self, track: usize) {
        if track < 64 {
            self.song_manual_latch
                .fetch_and(!(1u64 << track), Ordering::AcqRel);
        }
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
