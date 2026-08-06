use super::super::*;

impl SequencerState {
    pub fn scene_track_pattern_id(&self, scene: usize, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .get(scene)?
            .cells
            .get(track)
            .copied()
            .flatten()
    }

    pub fn track_pattern_cells(&self, track: usize) -> Vec<TrackPatternCellView> {
        let mut cells = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .track_pattern_cells(track);
        // No grid clip is "playing" while the lane is silenced (an
        // explicit-empty song row / deleted timeline clip) or while the
        // mirrored song row is playing a take on it (takes spec 11.2) —
        // the scene-cell fallback in `active_effective` would otherwise
        // show the scene's clip as audible when it isn't.
        let take_lane = track < 64 && self.song_take_lane_mask() >> track & 1 == 1;
        if take_lane || self.is_scene_silenced(track) {
            for cell in &mut cells {
                cell.active_effective = false;
            }
        }
        cells
    }

    /// Repaint the live grid from the current scene's cells, dropping
    /// whatever song playback left there. Song rows own the live lanes while
    /// they play — including silencing a lane the row resolves nothing for
    /// (takes spec 6.1) — and that state is transport state, not the scene's:
    /// leaving song playback with it still applied shows a scene whose clips
    /// are visible but not launched.
    ///
    /// Unlike `launch_scene` this never saves the mirror back: the mirror
    /// holds the song's row content, not this scene's, so capturing it would
    /// write the arrangement over the scene's patterns.
    pub fn resync_live_grid_to_current_scene(&self) {
        // The mirror is about to be rewritten from the scene cells; a
        // borrowed device lane would be silently clobbered (takes spec 16.2).
        // The App rebinds on its next tick.
        self.release_bound_device_state();
        let scene_idx = self.current_scene_index();
        // Latched lanes are the performer's (Ableton back-to-arrangement
        // semantics: the latch survives transport stop) — their live grid
        // must not be re-launched from the scene until the latch clears.
        let latched = self.song_manual_latch_mask();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        // The latch survives the stop, and so must its override pin:
        // `launch_scene` clears every override, but a latched lane that
        // loses its pin is stale with no self-write carve-out — every
        // masked save-back skips it, device edits made while stopped never
        // reach the pool entity, and the next Play re-launches the pool's
        // stale sound over them.
        let pinned: Vec<(usize, PatternId)> = scenes
            .track_overrides
            .iter()
            .enumerate()
            .take(64)
            .filter(|(track, _)| latched >> track & 1 == 1)
            .filter_map(|(track, id)| id.map(|id| (track, id)))
            .collect();
        let Some(launched) = scenes.launch_scene(scene_idx) else {
            return;
        };
        for (track, id) in pinned {
            if let Some(slot) = scenes.track_overrides.get_mut(track) {
                *slot = Some(id);
            }
        }
        for (track, data) in launched.into_iter().enumerate() {
            if latched >> track.min(63) & 1 == 1 {
                continue;
            }
            match data {
                Some(data) => {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                }
                None => {
                    self.clear_live_track_note_content(track);
                    self.set_scene_silenced(track, true);
                }
            }
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        drop(scenes);
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
    }

    pub fn launch_scene(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.launch_scene_profiled(
            scene_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    /// `launch_scene` for the control-side mirror of a scheduler-applied
    /// boundary launch: no pattern-epoch bump (the audio callback drops
    /// in-flight scheduled events whose stamped epoch no longer matches, and
    /// the scheduler already made the scene audible at the boundary from its
    /// prebuilt snapshot — the same contract as `apply_song_row`).
    pub fn launch_scene_mirror(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.launch_scene_profiled_with_epoch(
            scene_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            false,
        )
        .map(|result| result.sample_ids)
    }

    pub fn launch_scene_profiled(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        self.launch_scene_profiled_with_epoch(
            scene_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_scene_profiled_with_epoch(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        bump_pattern_epoch: bool,
    ) -> Option<PatternSwitchResult> {
        let total_started = Instant::now();
        let mut profile = PatternSwitchProfile::default();

        let started = Instant::now();
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        profile.capture_current_snapshot = started.elapsed();

        let (sample_ids, snapshot_source) = {
            let started = Instant::now();
            let mut scenes = self.pattern.scenes.lock().unwrap();
            profile.scene_lock_wait = started.elapsed();

            let current_scene = self.current_scene_index();
            if scene_idx >= scenes.scene_count() {
                return None;
            }

            let started = Instant::now();
            scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask());
            profile.save_current_snapshot = started.elapsed();

            let started = Instant::now();
            let launched = scenes.launch_scene(scene_idx)?;
            profile.launch_scene_data = started.elapsed();

            let started = Instant::now();
            for (track, data) in launched.iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    // No pattern in this scene (bare/cleared cell): present
                    // an empty step grid, not the previous scene's notes.
                    self.clear_live_track_note_content(track);
                    self.set_scene_silenced(track, true);
                }
            }
            profile.restore_tracks = started.elapsed();

            let started = Instant::now();
            let sample_ids = scenes.scene_sample_ids(scene_idx).unwrap_or_default();
            profile.collect_sample_ids = started.elapsed();

            let started = Instant::now();
            self.pattern
                .current_pattern
                .store(scene_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            if bump_pattern_epoch {
                self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            }
            profile.update_pattern_atoms = started.elapsed();

            let metadata = scenes.current_scene_metadata();
            let project_process_chain = scenes.current_project_process_chain();
            let snapshot_source = launched
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .map(|tracks| {
                    (
                        tracks,
                        metadata.0,
                        metadata.1,
                        metadata.2,
                        project_process_chain,
                    )
                });

            (sample_ids, snapshot_source)
        };

        let started = Instant::now();
        self.schedule_mod_resync();
        profile.schedule_mod_resync = started.elapsed();

        let started = Instant::now();
        if let Some((
            tracks,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        )) = snapshot_source
        {
            self.publish_scheduler_snapshot_from_track_pattern_data(
                &tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                project_process_chain,
            );
        } else {
            self.publish_scheduler_snapshot();
        }
        profile.publish_scheduler_snapshot = started.elapsed();
        profile.total = total_started.elapsed();

        Some(PatternSwitchResult {
            sample_ids,
            profile,
        })
    }

    pub fn launch_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask()) {
                return false;
            }
            scenes.launch_track_pattern(track, pattern_id)
        };
        let Some(data) = launched else {
            return false;
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn launch_scene_tracks(
        &self,
        scene: usize,
        tracks: &[usize],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        self.launch_scene_tracks_with_epoch(
            scene,
            tracks,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            true,
        )
    }

    /// `launch_scene_tracks` for the control-side mirror of a
    /// scheduler-applied boundary launch — no pattern-epoch bump (see
    /// `launch_scene_mirror`).
    #[allow(clippy::too_many_arguments)]
    pub fn launch_scene_tracks_mirror(
        &self,
        scene: usize,
        tracks: &[usize],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        self.launch_scene_tracks_with_epoch(
            scene,
            tracks,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_scene_tracks_with_epoch(
        &self,
        scene: usize,
        tracks: &[usize],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        bump_pattern_epoch: bool,
    ) -> bool {
        if tracks.is_empty() || tracks.iter().any(|track| *track >= num_tracks) {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scene >= scenes.scene_count() {
                return false;
            }
            // Validate the target before saving the current live state. Saving
            // is a mutation too, and a rejected launch must be side-effect free.
            if tracks.iter().any(|track| {
                scenes
                    .scenes
                    .get(scene)
                    .and_then(|scene| scene.cells.get(*track))
                    .copied()
                    .flatten()
                    .and_then(|id| scenes.track_pools.get(*track)?.get(id))
                    .is_none()
            }) {
                return false;
            }
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask()) {
                return false;
            }
            scenes.launch_scene_tracks(scene, tracks)
        };
        let Some(launched) = launched else {
            return false;
        };
        for (track, data) in launched {
            data.restore_to(self, track);
            self.set_scene_silenced(track, false);
        }
        if bump_pattern_epoch {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        }
        self.publish_scheduler_snapshot();
        true
    }

    /// Prebuild the scheduler snapshot a quantized launch would make audible
    /// (the per-row preflight pattern, docs/song-mode-spec.md 9, applied to
    /// session launches): the target scene's cells resolved against the
    /// pattern pools, materialized as one complete `Arc<SequencerSnapshot>`
    /// outside the audio path. The scheduler swaps to it exactly at the
    /// quantize boundary; the control-side mirror follows via the due drain.
    ///
    /// Read-only — nothing is launched, saved, or published here; the launch
    /// may still be replaced or canceled before its boundary.
    ///
    /// For a `SceneTracks` target the snapshot still carries the full target
    /// scene; the scheduler merges only the masked tracks over the live base
    /// snapshot per chunk. Returns `None` when the target does not fully
    /// resolve (missing scene, missing masked cell) — the caller falls back
    /// to the legacy control-side apply, which surfaces the launch error.
    pub fn preflight_pattern_launch_snapshot(
        &self,
        target: &crate::quantized_launch::PatternLaunchTarget,
    ) -> Option<Arc<SequencerSnapshot>> {
        use crate::quantized_launch::PatternLaunchTarget;
        let (scene_idx, mask) = match target {
            PatternLaunchTarget::Scene { scene } => (*scene, None),
            PatternLaunchTarget::SceneTracks { scene, tracks } => (*scene, Some(tracks)),
        };
        let staged = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let scene = scenes.scenes.get(scene_idx)?;
            let track_count = scenes.track_pools.len();
            let placeholder = PatternSnapshot::new_default(1, &[]).track_pattern_data(0)?;
            let mut track_data = Vec::with_capacity(track_count);
            let mut silenced = Vec::with_capacity(track_count);
            for track in 0..track_count {
                let cell = scene.cells.get(track).copied().flatten();
                match cell {
                    Some(id) => {
                        let Some(data) = scenes
                            .track_pools
                            .get(track)
                            .and_then(|pool| pool.get(id))
                        else {
                            // A launched cell that doesn't resolve: bail so
                            // the control-side apply reports the error.
                            if mask.is_none_or(|tracks| tracks.contains(&track)) {
                                return None;
                            }
                            track_data.push(placeholder.clone());
                            silenced.push(true);
                            continue;
                        };
                        track_data.push(data);
                        silenced.push(false);
                    }
                    None => {
                        // A track-mask launch requires every masked cell
                        // (`MissingSceneCell` on the apply path).
                        if mask.is_some_and(|tracks| tracks.contains(&track)) {
                            return None;
                        }
                        track_data.push(placeholder.clone());
                        silenced.push(true);
                    }
                }
            }
            (
                track_data,
                silenced,
                scene.mod_connections.clone(),
                scene.neural_networks.clone(),
                scene.graph_overrides.clone(),
                scene.project_process_chain.clone(),
            )
        };
        let (track_data, silenced, mod_connections, neural_networks, graph_overrides, chain) =
            staged;
        let mut snapshot = SequencerSnapshot::capture_from_track_pattern_data(
            self,
            &track_data,
            mod_connections,
            neural_networks,
            graph_overrides,
            chain,
        );
        // Only ever scheduled while the transport is playing; stamp it so
        // the deterministic clock treats it as playing regardless of the
        // transport state at preflight time (mirrors song-row preflight).
        snapshot.transport.playing = true;
        snapshot.transport.current_pattern = scene_idx;
        for (track, silenced) in silenced.iter().enumerate() {
            if *silenced {
                let mut track_snapshot = (*snapshot.tracks[track]).clone();
                track_snapshot.scene_silenced = true;
                snapshot.tracks[track] = Arc::new(track_snapshot);
            }
        }
        Some(Arc::new(snapshot))
    }

    /// Apply one song row as a single operation (docs/song-mode-spec.md 9):
    /// resolve the scene plus the row's COMPLETE override set (an override
    /// absent from the row is inactive even if one was live), mutate
    /// `ProjectScenes` current scene and overrides atomically, restore every
    /// track's live state, and publish exactly one scheduler snapshot. Never
    /// a launch sequence — a rejected row is side-effect free.
    ///
    /// `bump_pattern_epoch` must be true when applying a row while the
    /// transport is stopped (song start) and false for the control-side
    /// mirror of a scheduler-driven row transition: the audio callback drops
    /// in-flight scheduled events whose stamped epoch no longer matches, and
    /// during song playback the scheduler has already made the row audible
    /// sample-accurately from its prebuilt snapshot.
    ///
    /// Returns the per-track effective sample bindings (buffer id, name,
    /// sample rate) so the caller can rebind sampler buffers, mirroring
    /// `launch_scene`.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_song_row(
        &self,
        scene: usize,
        overrides: &[(usize, Option<PatternId>)],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        bump_pattern_epoch: bool,
    ) -> Result<Vec<(i32, String, u32)>, String> {
        self.apply_song_row_latched(
            scene,
            overrides,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            bump_pattern_epoch,
            0,
            false,
        )
    }

    /// `apply_song_row` with a manual-override latch mask (takes spec 10):
    /// latched tracks keep their live state, their session override slot,
    /// and their silencing untouched — the song's mirror leaves them to the
    /// performer until Back to Song clears the latch.
    ///
    /// `scene_latched` marks a manual SCENE launch holding the scene-level
    /// authority too: the session's current scene, the `current_pattern`
    /// atomic, and the row save-back all stay the performer's — only the
    /// per-lane restores for non-latched lanes (take lanes, per-track Back
    /// to Song) still follow the row. Moving the scene identity here would
    /// audibly re-key every scene-indexed reactive binding and misfile the
    /// next save-back into the row's scene.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_song_row_latched(
        &self,
        scene: usize,
        overrides: &[(usize, Option<PatternId>)],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        bump_pattern_epoch: bool,
        latched_mask: u64,
        scene_latched: bool,
    ) -> Result<Vec<(i32, String, u32)>, String> {
        let latched = |track: usize| track < 64 && latched_mask >> track & 1 == 1;
        if overrides.iter().any(|(track, _)| *track >= num_tracks) {
            return Err("Song row override targets a track that does not exist".to_string());
        }
        let current_snapshot = (!scene_latched).then(|| {
            self.capture_current_pattern_snapshot(
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
            )
        });
        // The capture no longer releases device loans (it captures the
        // scene-effective device state for borrowed lanes instead, takes
        // spec 18.1 step 3); the per-lane restores below overwrite the
        // mirror, so the loans must still be dropped here.
        self.release_bound_device_state();
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scene >= scenes.scene_count() {
                return Err(format!("Song row references scene {} which does not exist", scene + 1));
            }
            // Resolve the complete row state before mutating anything so a
            // rejected row leaves scenes, overrides, and live state intact.
            let mut resolved: Vec<(usize, Option<PatternId>, Option<TrackPatternData>, bool)> =
                Vec::with_capacity(num_tracks);
            for track in 0..num_tracks {
                if latched(track) {
                    continue;
                }
                // `Some(None)` is an explicit-empty override: the track is
                // silenced for the row and must NOT fall back to the scene
                // cell. Only an absent override resolves through the scene.
                let override_entry = overrides
                    .iter()
                    .find(|(over_track, _)| *over_track == track)
                    .map(|(_, id)| *id);
                // Take-claimed chunks never reach the session surface (takes
                // spec 11.2): the scheduler plays the take from the runtime
                // song's own row snapshots, so the mirror must NOT paint the
                // chunk into the live grid or the session override slot —
                // doing so leaks the take into the step sequencer, and the
                // next row's save-back then writes take content over pool
                // patterns (or mints one for a bare track). A take lane's
                // session identity stays the scene cell.
                let take_lane = matches!(
                    override_entry,
                    Some(Some(id)) if scenes
                        .take_pools
                        .get(track)
                        .is_some_and(|takes| takes.is_claimed(id))
                );
                let override_entry = if take_lane { None } else { override_entry };
                let override_id = override_entry.flatten();
                let effective = match override_entry {
                    Some(explicit) => explicit,
                    None => scenes
                        .scenes
                        .get(scene)
                        .and_then(|scene| scene.cells.get(track))
                        .copied()
                        .flatten(),
                };
                let data = match effective {
                    Some(id) => Some(
                        scenes
                            .track_pools
                            .get(track)
                            .and_then(|pool| pool.get(id))
                            .ok_or_else(|| {
                                format!(
                                    "Song row resolves track {} to pattern {} which is not \
                                     in the track's pattern pool",
                                    track + 1,
                                    id.0
                                )
                            })?,
                    ),
                    None => None,
                };
                resolved.push((track, override_id, data, take_lane));
            }
            if let Some(current_snapshot) = current_snapshot {
                let current_scene = self.current_scene_index();
                if !scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask()) {
                    return Err("Could not save the outgoing session state".to_string());
                }
                scenes.current_scene = scene;
            }
            for (track, slot) in scenes.track_overrides.iter_mut().enumerate() {
                if !latched(track) {
                    *slot = None;
                }
            }
            for (track, override_id, _, _) in &resolved {
                if override_id.is_some() {
                    if let Some(slot) = scenes.track_overrides.get_mut(*track) {
                        *slot = *override_id;
                    }
                }
            }
            let sample_ids: Vec<(i32, String, u32)> = (0..num_tracks)
                .map(|track| {
                    if latched(track) {
                        // Latched lanes keep their current (performer's)
                        // binding.
                        scenes
                            .effective_track_pattern(track)
                            .map(|data| data.sample_id.clone())
                            .unwrap_or((-1, String::new(), 44_100))
                    } else {
                        let entry = resolved.iter().find(|(t, _, _, _)| *t == track);
                        match entry {
                            Some((_, _, Some(data), _)) => data.sample_id.clone(),
                            // A take lane with no scene cell keeps its
                            // current binding — the lane is audibly playing
                            // its take, not being silenced or rebound.
                            Some((_, _, None, true)) => scenes
                                .effective_track_pattern(track)
                                .map(|data| data.sample_id.clone())
                                .unwrap_or((-1, String::new(), 44_100)),
                            _ => (-1, String::new(), 44_100),
                        }
                    }
                })
                .collect();
            let launched: Vec<(usize, Option<TrackPatternData>, bool)> = resolved
                .into_iter()
                .map(|(track, _, data, take_lane)| (track, data, take_lane))
                .collect();
            (launched, sample_ids)
        };
        let (launched, sample_ids) = launched;
        // Publish which lanes this row plays a take on (takes spec 11.2 UX):
        // the clip grid suppresses its "playing" marker for them. Latched
        // lanes are absent from `launched` and their bits were cleared when
        // the latch was set — the performer's launch is what plays there.
        let mut take_mask = 0u64;
        for (track, _, take_lane) in &launched {
            if *take_lane && *track < 64 {
                take_mask |= 1u64 << *track;
            }
        }
        self.set_song_take_lane_mask(take_mask);
        for (track, data, take_lane) in launched {
            match data {
                Some(data) => {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                }
                // A take lane whose scene cell is bare: the lane is audibly
                // playing its take from the runtime song, so it is neither
                // silenced nor repainted — the live grid keeps the track's
                // session (bare/empty) state.
                None if take_lane => self.set_scene_silenced(track, false),
                // Silence WITHOUT blanking the live grid. This mirror saves
                // the live snapshot into the current scene before applying
                // each row, so a lane silenced by an explicit-empty override
                // must keep its live content — blanking here would be saved
                // back over the scene cell's real pattern on the next row
                // application (destroying it). The session-mode launch path
                // blanks empty lanes safely because its save happens before
                // the blank and a bare cell is never written back.
                None => self.set_scene_silenced(track, true),
            }
        }
        if !scene_latched {
            self.pattern
                .current_pattern
                .store(scene as u32, Ordering::Relaxed);
        }
        if bump_pattern_epoch {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        }
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Ok(sample_ids)
    }

    pub fn fork_current_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let id = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask());
            scenes.fork_track_pattern(track)?
        };
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_current_scene_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask());
            let id = scenes.clone_track_pattern_into_current_scene(track)?;
            let data = scenes.effective_track_pattern(track)?;
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_track_pattern_id_into_current_scene(
        &self,
        track: usize,
        source_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask());
            let id = scenes.clone_track_pattern_id_into_current_scene(track, source_id)?;
            let data = scenes.effective_track_pattern(track)?;
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn delete_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Result<(), String> {
        if track >= num_tracks {
            return Err(format!("Track {} is out of range", track + 1));
        }
        // Deleting a pattern referenced by the committed song is rejected
        // with the referencing row positions (docs/song-mode-spec.md 5.4).
        if let Some(song) = self.committed_song() {
            let rows = song_rows_referencing_track_pattern(&song, track, pattern_id.0);
            if !rows.is_empty() {
                return Err(format!(
                    "Track {} pattern {} is used by song row(s) {}; \
                     update or clear those rows first",
                    track + 1,
                    pattern_id.0,
                    format_song_row_positions(&rows)
                ));
            }
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let (was_effective, replacement) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask());
            let was_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !scenes.delete_track_pattern(track, pattern_id) {
                return Err(format!(
                    "Track {} has no pattern {}",
                    track + 1,
                    pattern_id.0
                ));
            }
            let replacement = if was_effective {
                scenes.effective_track_pattern(track)
            } else {
                None
            };
            (was_effective, replacement)
        };

        if was_effective {
            if let Some(data) = replacement {
                data.restore_to(self, track);
                self.set_scene_silenced(track, false);
            } else {
                self.set_scene_silenced(track, true);
            }
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn set_scene_cell(
        &self,
        scene: usize,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        self.set_scene_cell_with_launch(
            scene,
            track,
            pattern_id,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            false,
        )
    }

    /// `set_scene_cell` for a quantized clip launch into the current scene:
    /// the cell assignment (the edit) lands now, but the audible restore is
    /// deferred to the pending `SceneTracks` boundary launch. Until that
    /// launch applies, the lane's override stays pinned to the pattern it is
    /// actually playing — the cell already names the new pattern, so an
    /// unpinned lane would let any masked save-back (the boundary launch's
    /// own save included) clone the outgoing pattern's live content over the
    /// newly assigned one.
    #[allow(clippy::too_many_arguments)]
    pub fn set_scene_cell_queued(
        &self,
        scene: usize,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        self.set_scene_cell_with_launch(
            scene,
            track,
            pattern_id,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn set_scene_cell_with_launch(
        &self,
        scene: usize,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        defer_launch: bool,
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let restore_current_track = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask()) {
                return false;
            }
            // The lane's audible identity before the cell moves — the
            // deferred-launch pin below must name the OUTGOING pattern.
            let playing = scenes.effective_pattern_id(track);
            if !scenes.set_cell(scene, track, pattern_id) {
                return false;
            }
            if scene == current_scene {
                if defer_launch {
                    if let Some(override_slot) = scenes.track_overrides.get_mut(track) {
                        if override_slot.is_none() {
                            *override_slot = playing;
                        }
                    }
                    None
                } else {
                    if let Some(override_slot) = scenes.track_overrides.get_mut(track) {
                        *override_slot = None;
                    }
                    scenes
                        .track_pools
                        .get(track)
                        .and_then(|pool| pool.get(pattern_id))
                }
            } else {
                None
            }
        };

        if let Some(data) = restore_current_track {
            data.restore_to(self, track);
            self.set_scene_silenced(track, false);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }

    pub fn clear_scene_cell(
        &self,
        scene: usize,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // The capture no longer releases device loans (takes spec 18.1 step
        // 3: it captures the scene-effective device state for borrowed lanes
        // instead); this path overwrites the mirror below, so drop the loans
        // here — the App re-binds on its next sync.
        self.release_bound_device_state();
        let (cleared, should_silence) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot_masked(current_scene, current_snapshot, self.stale_live_lane_mask()) {
                return None;
            }
            let cleared = scenes.clear_cell(scene, track)?;
            let should_silence =
                scene == current_scene && scenes.effective_pattern_id(track).is_none();
            (cleared, should_silence)
        };

        if should_silence {
            self.set_scene_silenced(track, true);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        Some(cleared)
    }

    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.switch_pattern_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    pub fn switch_pattern_profiled(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        let cur = self.current_scene_index();
        if new_idx == cur {
            return None;
        }
        self.launch_scene_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
    }

    pub fn clone_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> usize {
        let new_idx = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot_masked(cur, current_snapshot, self.stale_live_lane_mask());
            let new_idx = scenes.new_scene();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            new_idx
        };
        self.publish_scheduler_snapshot();
        new_idx
    }

    /// Reorder scenes while keeping the currently playing scene active and
    /// leaving all per-track pattern pools untouched.
    pub fn reorder_scene(&self, source: usize, target: usize) -> Option<usize> {
        let _ = self.quantized_launches.cancel_all();
        let current_scene = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes.reorder_scene(source, target)?
        };
        self.pattern
            .current_pattern
            .store(current_scene as u32, Ordering::Relaxed);
        Some(current_scene)
    }

    pub fn rename_scene(&self, scene: usize, name: String) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(target) = scenes.scenes.get_mut(scene) else {
            return false;
        };
        if target.name == name {
            return false;
        }
        target.name = name.to_string();
        true
    }

    pub fn delete_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Result<Vec<(i32, String, u32)>, String> {
        let _ = self.quantized_launches.cancel_all();
        // Deleting a scene referenced by the committed song is rejected with
        // the referencing row positions (docs/song-mode-spec.md 5.4).
        {
            let cur = self.current_scene_index();
            if let Some(song) = self.committed_song() {
                let rows = song_rows_referencing_scene(&song, cur);
                if !rows.is_empty() {
                    return Err(format!(
                        "Scene {} is used by song row(s) {}; \
                         reassign or clear those rows first",
                        cur + 1,
                        format_song_row_positions(&rows)
                    ));
                }
            }
        }
        let sample_ids = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scenes.scene_count() <= 1 {
                return Err("The last scene cannot be deleted".to_string());
            }
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot_masked(cur, current_snapshot, self.stale_live_lane_mask());
            let new_idx = scenes
                .delete_scene(cur)
                .ok_or_else(|| "The last scene cannot be deleted".to_string())?;
            // Higher scene indices shift down; keep song references pointed
            // at the same scenes in the same transaction.
            self.with_committed_song_mut(|song| {
                if let Some(song) = song {
                    remap_song_after_scene_delete(song, cur);
                }
            });
            self.with_committed_arrangement_mut(|arrangement| {
                if let Some(arrangement) = arrangement {
                    remap_arrangement_after_scene_delete(arrangement, cur);
                }
            });
            let launched = scenes
                .launch_scene(new_idx)
                .ok_or_else(|| "Could not launch the replacement scene".to_string())?;
            for (track, data) in launched.into_iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            let sample_ids = scenes
                .scene_snapshot(new_idx)
                .map(|snapshot| snapshot.sample_ids)
                .unwrap_or_default();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            sample_ids
        };
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Ok(sample_ids)
    }

    pub fn propagate_track_to_all_patterns(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let cur = self.current_scene_index();
        if cur >= scenes.scene_count() || track >= num_tracks {
            return false;
        }
        let current_metadata = scenes.current_scene_metadata();
        let current_snapshot = PatternSnapshot::capture_with_mod_connections(
            self,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            current_metadata.0,
            current_metadata.1,
            current_metadata.2,
        );
        scenes.save_scene_snapshot_masked(cur, current_snapshot, self.stale_live_lane_mask());
        let Some(source) = scenes.scene_snapshot(cur) else {
            return false;
        };
        let mut snapshots = scenes.snapshots();
        for (pattern_idx, snapshot) in snapshots.iter_mut().enumerate() {
            if pattern_idx != cur {
                snapshot.clone_track_lane_from(&source, track);
            }
        }
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, cur);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        true
    }
}
