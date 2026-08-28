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
    /// `None` for an unscened row (empty-arrangement spec 4.2).
    scene: Option<usize>,
    overrides: Vec<(usize, Option<PatternId>)>,
    resolved_pattern_ids: Vec<Option<PatternId>>,
    resolved_sources: Vec<LaneSource>,
    lane_offsets: Vec<f64>,
    track_data: Vec<TrackPatternData>,
    silenced: Vec<bool>,
    mod_connections: Vec<ModConnection>,
    neural_networks: Vec<crate::neural::ProjectNeuralNetwork>,
    graph_overrides: Vec<crate::graph::ProjectGraphOverrides>,
    scene_slots: SceneSlotStore,
    project_process_chain: crate::process::TrackProcessChain,
}

/// One track's resolved lane inside a project row, before chunk expansion.
enum LaneResolution {
    Silent,
    Pattern {
        id: PatternId,
        offset_steps: f64,
        /// The pattern's real beat↔step geometry (per-step timebase and sync
        /// plocks included) — the `steps()` mapping offsets are stamped and
        /// advanced in (takes spec 7.1/7.2).
        geometry: PatternStepGeometry,
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
                // An unscened row has no scene to resolve against: absent
                // overrides fall back to silence, and the scene-owned graph
                // state (mod connections, networks, process chain) is empty
                // (empty-arrangement spec 4.2).
                let scene = match row.scene {
                    Some(scene_idx) => Some(scenes.scenes.get(scene_idx).ok_or_else(|| {
                        format!(
                            "Song row {} references scene {} which no longer exists",
                            row_idx + 1,
                            scene_idx + 1
                        )
                    })?),
                    None => None,
                };
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
                                .and_then(|scene| scene.cells.get(track))
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
                            LaneResolution::Pattern {
                                id,
                                offset_steps,
                                geometry: data.step_geometry(),
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
                                geometry,
                            } => {
                                let advanced = if delta_beats > 0.0 {
                                    // Advance in the pattern's real geometry,
                                    // snap before wrapping (boundary-coincident
                                    // rows must land exactly on the step), then
                                    // wrap through the one shared window helper
                                    // (clip-edit-target spec 5.1).
                                    pattern_play_step(
                                        snap_steps(geometry.advance(*offset_steps, delta_beats)),
                                        0.0,
                                        (0.0, geometry.num_steps() as f64),
                                    )
                                } else {
                                    *offset_steps
                                };
                                let data = scenes
                                    .track_pools
                                    .get(track)
                                    .and_then(|pool| pool.get(*id))
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
                        mod_connections: scene
                            .map(|scene| scene.mod_connections.clone())
                            .unwrap_or_default(),
                        neural_networks: scene
                            .map(|scene| scene.neural_networks.clone())
                            .unwrap_or_default(),
                        graph_overrides: scene
                            .map(|scene| scene.graph_overrides.clone())
                            .unwrap_or_default(),
                        scene_slots: scene
                            .map(|scene| scene.scene_slots.clone())
                            .unwrap_or_default(),
                        project_process_chain: scene
                            .map(|scene| scene.project_process_chain.clone())
                            .unwrap_or_default(),
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
                staging.scene_slots,
                staging.project_process_chain,
            );
            // Row snapshots are only ever scheduled while the song transport
            // is playing; stamp them so the deterministic clock treats them
            // as playing regardless of the transport state at preflight time.
            snapshot.transport.playing = true;
            // An unscened row keeps the captured live scene as its transport
            // stamp — there is no scene to switch the display to.
            if let Some(scene) = staging.scene {
                snapshot.transport.current_pattern = scene;
            }
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

    /// Bitmask of lanes whose live-grid content does not belong to the
    /// current scene: song-latched lanes (the performer's launch is live
    /// there), scene-silenced lanes (an explicit-empty row override left
    /// stale STEP content in the live grid on purpose — see the mirror
    /// comment in `apply_song_row_latched`), and — in arrangement context —
    /// track-owned lanes, whose mirror is the TRACK SOUND by construction
    /// (track-sound spec §2.2.2). Pass this to `save_scene_snapshot_masked`
    /// so a live-grid save-back never clones foreign content over a scene
    /// cell's real pattern.
    pub fn stale_live_lane_mask(&self) -> u64 {
        self.song_manual_latch_mask()
            | self.scene_silenced_mask()
            | self.track_owned_lane_mask()
            | self.arrangement_borrowed_lane_mask()
    }

    /// Borrowed lanes while the user stands in the ARRANGEMENT view. There
    /// the live grid holds rule-1/2 content — the selection's or the audible
    /// arrangement source's — and the current scene's cell is
    /// inert-but-visible (track-sound spec §2.2.2), so a masked save-back
    /// must treat the lane like a latched one: no cell write (the grid's
    /// STEP content is the arrangement's, not the cell's — storing it
    /// clobbers the session pattern's notes) and no track-sound write (the
    /// mirror's device half is the borrow's, which the selection's own
    /// edit-time write-through already persists). In Seq context a borrow
    /// replaces only the device half and the capture substitutes it back
    /// (`capture_current_pattern_snapshot`), so the cell save there stays a
    /// self-write and the mask is empty.
    pub(crate) fn arrangement_borrowed_lane_mask(&self) -> u64 {
        if self.arrangement_context() {
            self.sound_binding_borrowed_mask()
        } else {
            0
        }
    }

    /// The `(stale, latched, track_owned)` masks `save_scene_snapshot_masked`
    /// consumes, captured together. Every capture → release-device-loans →
    /// masked-save path MUST read this BEFORE the release: the snapshot
    /// substituted each borrowed lane's device half with its CELL's entity
    /// state (`capture_current_pattern_snapshot`), so a lane whose borrow is
    /// released in between must still count as claimed at save time.
    /// Recomputing the masks after the release promotes the lane into
    /// `track_owned_lane_mask`, and the save then writes the cell's stock
    /// device state into the shared track-sound entities — retuning the
    /// track sound and every take sharing it (track-sound spec §2.8 litmus:
    /// the user never heard or dialed that in).
    pub(crate) fn masked_save_masks(&self) -> (u64, u64, u64) {
        (
            self.stale_live_lane_mask(),
            // Arrangement-context borrows ride the latched mask: same
            // treatment (skip the cell, skip the track sound), same reason
            // (the mirror is the claim's, not the owner's).
            self.song_manual_latch_mask() | self.arrangement_borrowed_lane_mask(),
            self.track_owned_lane_mask(),
        )
    }

    /// Scene-silenced lanes as a bitmask. Playback DISPLAY state only since
    /// rev 4 (the silenced flag no longer gates ownership, §2.2.1's
    /// supersession note) — kept for the surfaces that render "this lane is
    /// not sounding its cell".
    pub fn scene_silenced_mask(&self) -> u64 {
        let mut mask = 0u64;
        for track in 0..self.pattern.scene_silenced.len().min(64) {
            if self.is_scene_silenced(track) {
                mask |= 1 << track;
            }
        }
        mask
    }

    /// True while the user is standing in the arrangement view — the rev-4
    /// ownership discriminator (track-sound spec §2.2.2). Written by the App
    /// on every view switch.
    pub fn arrangement_context(&self) -> bool {
        self.arrangement_context.load(Ordering::Acquire)
    }

    pub fn set_arrangement_context(&self, arrangement: bool) {
        self.arrangement_context.store(arrangement, Ordering::Release);
    }

    /// Lanes whose sound the TRACK owns right now (track-sound spec §2.2.2):
    /// in arrangement context every lane rules 1/2 do not claim; in Seq
    /// context none — the classic scene+pattern world, where the track sound
    /// is dormant.
    ///
    /// Rule 1/2 claims are read here in their machine-readable state-side
    /// form: a LATCHED lane (the performer's own launch — it keeps its
    /// self-write carve-out through its override pin) and a BORROWED lane
    /// (the sound binding installed a selected clip's or an audible take's
    /// devices into the mirror, so the mirror is not the track's sound).
    pub fn track_owned_lane_mask(&self) -> u64 {
        if !self.arrangement_context() {
            return 0;
        }
        !(self.song_manual_latch_mask() | self.sound_binding_borrowed_mask())
    }

    /// Pin `track`'s session override to the pattern the lane currently
    /// resolves (override, else the scene cell). Loop overdub claims a lane
    /// this way (unified-transport spec 5.1): a latched lane is skipped by
    /// every masked scene save-back as stale — UNLESS its override pins the
    /// pattern it is actually playing, which turns the save into a
    /// self-write. Without the pin, live-recorded content exists only in
    /// the live grid and the stop resync re-launches the scene from the
    /// pool, silently discarding the recording.
    pub fn pin_track_override_to_effective(&self, track: usize) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(id) = scenes.effective_pattern_id(track) else {
            return false;
        };
        scenes.launch_track_pattern(track, id).is_some()
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

    /// Lanes whose live device state is on loan to a sound binding (takes
    /// spec 16.2).
    pub fn sound_binding_borrowed_mask(&self) -> u64 {
        self.sound_binding_borrowed.load(Ordering::Acquire)
    }

    /// Load `pattern`'s devices into `track`'s live mirror and mark the lane
    /// borrowed, so a later session save-back restores the scene pattern's
    /// sound first instead of writing this one over it.
    pub fn borrow_track_device_state(
        &self,
        track: usize,
        pattern: PatternId,
        data: &TrackPatternData,
    ) -> bool {
        if track >= 64 || !data.restore_device_state_to(self, track) {
            return false;
        }
        self.sound_binding_patterns
            .lock()
            .unwrap()
            .insert(track, pattern);
        self.sound_binding_borrowed
            .fetch_or(1u64 << track, Ordering::AcqRel);
        true
    }

    /// The pool pattern whose device state the live mirror currently shows
    /// for `track`: the bound source's pattern while a sound binding holds
    /// the lane (takes spec 16.2), the effective scene pattern otherwise —
    /// and on a bare lane (no cell, no override) the TRACK SOUND's carrier
    /// pattern (track-sound spec §2.3: the live mirror on a bare lane *is*
    /// the track sound). Device edits use this to decide whether the live
    /// surface IS the target.
    pub(crate) fn mirror_device_pattern_id(
        &self,
        track: usize,
        scenes: &ProjectScenes,
    ) -> Option<PatternId> {
        if track < 64 && self.sound_binding_borrowed.load(Ordering::Acquire) >> track & 1 == 1 {
            if let Some(pattern) = self.sound_binding_patterns.lock().unwrap().get(&track) {
                return Some(*pattern);
            }
        }
        // §2.2.2 (rev 4): in arrangement context the mirror is the TRACK
        // SOUND on every unborrowed lane — a resolving cell is inert-but-
        // visible there and must not name what the live surface is showing.
        // Seq context is the classic scene+pattern world.
        if !self.arrangement_context() {
            if let Some(id) = scenes.effective_pattern_id(track) {
                return Some(id);
            }
        }
        scenes.track_sound_pattern(track)
    }

    /// Put every borrowed lane's effective scene pattern back behind the
    /// device panel. Idempotent, and a no-op in the overwhelmingly common
    /// case where nothing is borrowed.
    pub fn release_bound_device_state(&self) {
        self.release_bound_device_state_except(0);
    }

    /// `release_bound_device_state` sparing the lanes in `hold_mask`: their
    /// loans stay claimed and their mirrors keep the borrowed sound.
    ///
    /// The song row mirror uses this for lanes the next row resolves to the
    /// source that is already borrowed (takes spec §17.3's gap hold is the
    /// audible case). Releasing them would repaint the mirror from the lane
    /// owner and push that at the engine, and only the binding sync that
    /// runs *after* the row apply would put the held sound back — an audible
    /// snap to a sound the user never dialed in.
    pub fn release_bound_device_state_except(&self, hold_mask: u64) {
        self.release_borrowed_lanes(
            self.sound_binding_borrowed
                .fetch_and(hold_mask, Ordering::AcqRel)
                & !hold_mask,
        );
    }

    /// Release one lane (its binding fell back to the scene pattern).
    pub fn release_bound_track_device_state(&self, track: usize) {
        if track >= 64 {
            return;
        }
        let bit = 1u64 << track;
        self.release_borrowed_lanes(self.sound_binding_borrowed.fetch_and(!bit, Ordering::AcqRel) & bit);
    }

    fn release_borrowed_lanes(&self, mask: u64) {
        if mask == 0 {
            return;
        }
        {
            let mut bound = self.sound_binding_patterns.lock().unwrap();
            bound.retain(|track, _| *track >= 64 || mask >> *track & 1 == 0);
        }
        let restore: Vec<(usize, TrackPatternData)> = {
            let scenes = self.pattern.scenes.lock().unwrap();
            (0..64)
                .filter(|track| mask >> track & 1 == 1)
                .filter_map(|track| {
                    // §2.8 borrow-release seam, re-keyed to the owner (§2.9):
                    // the released lane is repainted by whoever owns it —
                    // the cell in Seq context, the TRACK SOUND in arrangement
                    // context. Restoring nothing would leave the borrow's
                    // device state behind for the next save-back to write
                    // into the track sound.
                    let installed = !self.arrangement_context();
                    let data = installed
                        .then(|| scenes.effective_track_pattern(track))
                        .flatten()
                        .or_else(|| {
                            scenes
                                .track_sound_pattern(track)
                                .and_then(|id| scenes.track_pools.get(track)?.get(id))
                        })?;
                    Some((track, data))
                })
                .collect()
        };
        for (track, data) in restore {
            data.restore_device_state_to(self, track);
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
        self.latch_song_scene_override();
    }

    /// Scene-scoped latch (spec 10): a manual SCENE launch suspends the
    /// song's scene-level authority too — the row mirror must leave the
    /// current scene, the `current_pattern` atomic, and the bus pattern with
    /// the performer (scene-keyed reactive bindings and bus/group fx recall
    /// hang off them and would audibly re-apply the row's scene).
    pub fn latch_song_scene_override(&self) {
        self.song_scene_latch.store(true, Ordering::Release);
    }

    pub fn song_scene_latch(&self) -> bool {
        self.song_scene_latch.load(Ordering::Acquire)
    }

    /// Back to Song / punch-out: the song resumes launch authority for
    /// every lane (takes spec 10). Transient state; never serialized.
    pub fn clear_song_manual_latch(&self) {
        self.song_manual_latch.store(0, Ordering::Release);
        self.song_scene_latch.store(false, Ordering::Release);
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
    /// `open_ended` opts out of the song-end stop for arrangement capture
    /// (docs/song-mode-spec.md 7.4): recording must not be cut off and
    /// committed at the old song length.
    pub fn start_song_playback(
        &self,
        song: Arc<RuntimeSong>,
        start_beat: f64,
        open_ended: bool,
    ) -> Result<(), String> {
        // Validate eagerly so the caller gets the error, not the scheduler.
        // The nominal samples-per-quarter only has to be positive here; the
        // scheduler rebuilds the runtime with its real tempo mapping.
        SongPlaybackRuntime::new(Arc::clone(&song), start_beat, 1.0)?;
        self.song_playback
            .send_command(SongPlaybackCommand::Start {
                song,
                start_beat,
                open_ended,
            })
    }

    /// Hand the scheduler re-preflighted rows for the song already playing
    /// (takes spec 16.7 edit-through). Content-only: the scheduler ignores
    /// it if the row layout moved.
    pub fn refresh_song_playback(&self, song: Arc<RuntimeSong>) -> Result<(), String> {
        self.song_playback
            .send_command(SongPlaybackCommand::Refresh { song })
    }

    /// Hand the scheduler a structurally rebuilt song. Unlike `Refresh`,
    /// this command remaps the playback cursor and may defer installation
    /// until the sounding row reaches its next boundary.
    pub fn rebuild_song_playback(&self, song: Arc<RuntimeSong>) -> Result<(), String> {
        self.song_playback
            .send_command(SongPlaybackCommand::Rebuild { song })
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
