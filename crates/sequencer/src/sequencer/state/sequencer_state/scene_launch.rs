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
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .track_pattern_cells(track)
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

    pub fn launch_scene_profiled(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
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
            scenes.save_scene_snapshot(current_scene, current_snapshot);
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
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
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
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
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
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
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
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
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
        overrides: &[(usize, PatternId)],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        bump_pattern_epoch: bool,
    ) -> Result<Vec<(i32, String, u32)>, String> {
        if overrides.iter().any(|(track, _)| *track >= num_tracks) {
            return Err("Song row override targets a track that does not exist".to_string());
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let launched: Vec<(usize, Option<TrackPatternData>)> = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scene >= scenes.scene_count() {
                return Err(format!("Song row references scene {} which does not exist", scene + 1));
            }
            // Resolve the complete row state before mutating anything so a
            // rejected row leaves scenes, overrides, and live state intact.
            let mut resolved: Vec<(usize, Option<PatternId>, Option<TrackPatternData>)> =
                Vec::with_capacity(num_tracks);
            for track in 0..num_tracks {
                let override_id = overrides
                    .iter()
                    .find(|(over_track, _)| *over_track == track)
                    .map(|(_, id)| *id);
                let effective = override_id.or_else(|| {
                    scenes
                        .scenes
                        .get(scene)
                        .and_then(|scene| scene.cells.get(track))
                        .copied()
                        .flatten()
                });
                let data = match effective {
                    Some(id) => Some(
                        scenes
                            .track_pools
                            .get(track)
                            .and_then(|pool| pool.get(id))
                            .cloned()
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
                resolved.push((track, override_id, data));
            }
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return Err("Could not save the outgoing session state".to_string());
            }
            scenes.current_scene = scene;
            for slot in scenes.track_overrides.iter_mut() {
                *slot = None;
            }
            for (track, override_id, _) in &resolved {
                if override_id.is_some() {
                    if let Some(slot) = scenes.track_overrides.get_mut(*track) {
                        *slot = *override_id;
                    }
                }
            }
            resolved
                .into_iter()
                .map(|(track, _, data)| (track, data))
                .collect()
        };
        let sample_ids = launched
            .iter()
            .map(|(_, data)| {
                data.as_ref()
                    .map(|data| data.sample_id.clone())
                    .unwrap_or((-1, String::new(), 44_100))
            })
            .collect();
        for (track, data) in launched {
            match data {
                Some(data) => {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                }
                None => self.set_scene_silenced(track, true),
            }
        }
        self.pattern
            .current_pattern
            .store(scene as u32, Ordering::Relaxed);
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
        let id = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
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
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_into_current_scene(track)?;
            let data = scenes.effective_track_pattern(track)?.clone();
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
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_id_into_current_scene(track, source_id)?;
            let data = scenes.effective_track_pattern(track)?.clone();
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
        let (was_effective, replacement) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let was_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !scenes.delete_track_pattern(track, pattern_id) {
                return Err(format!(
                    "Track {} has no pattern {}",
                    track + 1,
                    pattern_id.0
                ));
            }
            let replacement = if was_effective {
                scenes.effective_track_pattern(track).cloned()
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
        let restore_current_track = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            if !scenes.set_cell(scene, track, pattern_id) {
                return false;
            }
            if scene == current_scene {
                if let Some(override_slot) = scenes.track_overrides.get_mut(track) {
                    *override_slot = None;
                }
                scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.get(pattern_id))
                    .cloned()
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
        let (cleared, should_silence) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
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
            scenes.save_scene_snapshot(cur, current_snapshot);
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
            scenes.save_scene_snapshot(cur, current_snapshot);
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
        scenes.save_scene_snapshot(cur, current_snapshot);
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
