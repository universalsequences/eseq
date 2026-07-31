use super::super::*;

impl SequencerState {
    pub fn set_neural_reset_step(
        &self,
        track: usize,
        step: usize,
        enabled: bool,
    ) -> Result<bool, String> {
        if track >= self.active_track_count() {
            return Err("track out of range".to_string());
        }
        if step >= MAX_STEPS {
            return Err("step out of range".to_string());
        }
        self.pattern.neural_reset_patterns[track].set_step_active(step, enabled);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Ok(enabled)
    }

    pub fn request_track_delete_boundary(&self, track_idx: usize) -> u64 {
        let request_id = self
            .transport
            .topology_edit_request_id
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.transport
            .topology_edit_track
            .store(track_idx as u32, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_DELETE_TRACK, Ordering::Release);
        request_id
    }

    pub fn topology_edit_ready(&self, request_id: u64) -> bool {
        self.transport
            .topology_edit_ready_id
            .load(Ordering::Acquire)
            >= request_id
    }

    pub fn topology_edit_in_flight(&self) -> bool {
        self.transport.topology_edit_kind.load(Ordering::Acquire) != TOPOLOGY_EDIT_NONE
    }

    pub fn complete_topology_edit(&self, request_id: u64) {
        self.transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.transport
            .topology_edit_applied_id
            .store(request_id, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_NONE, Ordering::Release);
        self.transport
            .topology_edit_track
            .store(u32::MAX, Ordering::Release);
    }

    fn reset_track_params_to_default(&self, track: usize) {
        let defaults = TrackParamsSnapshot::default();
        let params = &self.pattern.track_params[track];
        params.gate.store(defaults.gate, Ordering::Relaxed);
        params.set_attack_ms(defaults.attack_ms);
        params.set_release_ms(defaults.release_ms);
        params.set_swing(defaults.swing);
        params.set_swing_resolution(defaults.swing_resolution);
        params.set_num_steps(defaults.num_steps);
        params.set_volume(defaults.volume);
        params.set_pan(defaults.pan);
        params.set_mute(defaults.mute);
        params.set_solo(false);
        params.set_send(defaults.send);
        params.set_output(defaults.output);
        params.set_sends(defaults.sends);
        params
            .polyphonic
            .store(defaults.polyphonic, Ordering::Relaxed);
        params.set_max_polyphony(defaults.max_polyphony);
        params.set_timebase(defaults.timebase);
        params.set_accumulator_idx(defaults.accumulator_idx);
        params.set_script_accumulator_name(defaults.script_accumulator_name);
        params.set_midi_fx_chain(defaults.midi_fx_chain);
        params.set_midi_fx_position(defaults.midi_fx_position);
        params.set_accum_limit(defaults.accum_limit);
        params.set_accum_mode(defaults.accum_mode);
        params.set_fts_scale(defaults.fts_scale);
        params.set_mute_group(defaults.mute_group);
        params.set_global_transpose(defaults.global_transpose);
    }

    pub fn clear_live_track_state(&self, track_count: usize) {
        for track in 0..track_count.min(MAX_TRACKS) {
            self.clear_live_track_lane(track);
            self.clear_runtime_track_binding_in_place(track);
        }
    }

    fn clear_live_track_lane(&self, track: usize) {
        self.pattern.patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        self.pattern.neural_reset_patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        for step in 0..MAX_STEPS {
            for param in StepParam::ALL {
                self.pattern.step_data[track].set(step, param, param.default_value());
            }
            self.pattern.chord_data[track].clear_step(step);
            self.pattern.timebase_plocks[track].clear(step);
            self.pattern.swing_plocks[track].clear(step);
            self.pattern.swing_resolution_plocks[track].clear(step);
        }
        self.reset_track_params_to_default(track);
        for slot in &self.pattern.effect_chains[track] {
            slot.clear();
        }
        for slot in &self.pattern.midi_fx_slots[track] {
            slot.clear();
        }
        self.pattern.instrument_slots[track].clear();
        self.pattern.instrument_base_note_offsets[track].store(0.0f32.to_bits(), Ordering::Relaxed);
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        if let Some(sound) = self
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *sound = TrackSoundState::default();
        }
        if let Some(rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            *rack_track = None;
        }
        if let Some(chain) = self.pattern.process_chains.lock().unwrap().get_mut(track) {
            *chain = crate::process::TrackProcessChain::default();
        }
        if let Some(overrides) = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get_mut(track)
        {
            overrides.clear();
        }
        if let Some(registry) = self
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
        if let Some(registry) = self
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
    }

    fn clear_runtime_track_binding_in_place(&self, track: usize) {
        self.set_scene_silenced(track, false);
        self.transport.track_playheads[track].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[track].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[track].store(0, Ordering::Relaxed);
        self.runtime.voice_counts[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[track].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[track].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[track][slot].store(0, Ordering::Relaxed);
        }
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[track][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[track].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][track].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][track]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][track][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    fn shift_runtime_track_bindings_left(&self, track_idx: usize, old_count: usize) {
        for idx in track_idx..old_count.saturating_sub(1) {
            let next = idx + 1;
            self.transport.track_playheads[idx].store(
                self.transport.track_playheads[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.transport.trigger_flash[idx].store(
                self.transport.trigger_flash[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.pattern.scene_silenced[idx].store(
                self.pattern.scene_silenced[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_lids[idx].store(
                self.runtime.sampler_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.modulator_lids[idx].store(
                self.runtime.modulator_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.pan_lids[idx].store(
                self.runtime.pan_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.delay_lids[idx].store(
                self.runtime.delay_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.send_lids[idx].store(
                self.runtime.send_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for slot in 0..MAX_RACK_SLOTS {
                self.runtime.rack_slot_pan_lids[idx][slot].store(
                    self.runtime.rack_slot_pan_lids[next][slot].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.runtime.voice_counts[idx].store(
                self.runtime.voice_counts[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_type_flags[idx].store(
                self.runtime.instrument_type_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_run_mode_flags[idx].store(
                self.runtime.instrument_run_mode_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.track_engine_ids[idx].store(
                self.runtime.track_engine_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_buffer_ids[idx].store(
                self.runtime.sampler_analysis_buffer_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_bpm[idx].store(
                self.runtime.sampler_analysis_bpm[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_lo[idx].store(
                self.runtime.sampler_onset_ptr_lo[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_hi[idx].store(
                self.runtime.sampler_onset_ptr_hi[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_status[idx].store(
                self.runtime.sampler_analysis_status[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for voice in 0..MAX_VOICES {
                self.runtime.voice_lids[idx][voice].store(
                    self.runtime.voice_lids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.synth_node_ids[idx][voice].store(
                    self.runtime.synth_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_gatepitch_node_ids[idx][voice].store(
                    self.runtime.sampler_gatepitch_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_modulator_node_ids[idx][voice].store(
                    self.runtime.sampler_modulator_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.pending_accumulator_reset_tracks[idx].store(
                self.pending_accumulator_reset_tracks[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for engine_id in 0..self.runtime.engine_route_lids.len() {
                for voice in 0..MAX_VOICES {
                    self.runtime.engine_route_lids[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    self.runtime.engine_route_lids_r[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids_r[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.runtime.engine_ext_route_lids[engine_id][voice][idx][input].store(
                            self.runtime.engine_ext_route_lids[engine_id][voice][next][input]
                                .load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        );
                    }
                }
            }
        }

        if old_count == 0 {
            return;
        }
        let last = old_count - 1;
        self.set_scene_silenced(last, false);
        self.transport.track_playheads[last].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[last].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[last].store(0, Ordering::Relaxed);
        self.runtime.pan_lids[last].store(0, Ordering::Relaxed);
        self.runtime.delay_lids[last].store(0, Ordering::Relaxed);
        self.runtime.send_lids[last].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[last][slot].store(0, Ordering::Relaxed);
        }
        self.runtime.voice_counts[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[last].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[last].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[last].store(0, Ordering::Relaxed);
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[last][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[last].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][last].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][last]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][last][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    /// Delete one track from live sequencer state and compact higher track indices.
    ///
    /// This state-side helper is the non-graph half of track deletion semantics:
    /// the deleted lane disappears from the current pattern and all snapshots in
    /// memory, higher lanes shift down immediately, and the old trailing lane is
    /// cleared so stale state cannot leak back after future restores.
    pub fn capture_track_pattern_lane_state(
        &self,
        track_idx: usize,
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> Result<TrackPatternLaneState, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get(track_idx)
            .cloned()
            .ok_or_else(|| format!("Track {} has no pattern pool", track_idx + 1))?;
        let scene_cells = scenes.scenes.iter()
            .map(|scene| scene.cells.get(track_idx).copied().flatten())
            .collect();
        let cell_sounds = scenes.scenes.iter()
            .map(|scene| {
                scene.cell_sounds.get(track_idx).copied().ok_or_else(|| {
                    format!("Track {} scene cell sound is missing", track_idx + 1)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let track_override = scenes.track_overrides.get(track_idx).copied().flatten();
        let scene_references = scenes.scenes.iter().map(|scene| SceneTrackReferenceState {
            mod_connections: scene.mod_connections.clone(),
            neural_networks: scene.neural_networks.clone(),
            graph_overrides: scene.graph_overrides.clone(),
        }).collect();
        let mut sidechains = Vec::new();
        for (owner_track, owner_pool) in scenes.track_pools.iter().enumerate() {
            if owner_track == track_idx {
                continue;
            }
            let Some(descriptors) = effect_descriptors.get(owner_track) else {
                continue;
            };
            let sidechain_slots = descriptors.iter().enumerate()
                .filter(|(_, descriptor)| descriptor.params.iter().any(|param| {
                    matches!(param.host_control, Some(HostControl::FxSidechain { .. }))
                }))
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>();
            if sidechain_slots.is_empty() {
                continue;
            }
            for (pattern, stored) in &owner_pool.patterns {
                let Some(patch) = owner_pool.sounds.patches.get(&stored.sound.patch) else {
                    continue;
                };
                let slots = sidechain_slots.iter().filter_map(|slot| {
                    patch.effect_slots.get(*slot).cloned().map(|state| (*slot, state))
                }).collect::<Vec<_>>();
                if !slots.is_empty() {
                    sidechains.push(TrackSidechainPatternState {
                        owner_track,
                        pattern: *pattern,
                        slots,
                    });
                }
            }
        }
        Ok(TrackPatternLaneState {
            pool,
            scene_cells,
            cell_sounds,
            track_override,
            scene_references,
            sidechains,
        })
    }

    pub fn replace_appended_track_pattern_lane(
        &self,
        snapshot: &TrackPatternLaneState,
    ) -> Result<usize, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let track = scenes.track_pools.len().checked_sub(1)
            .ok_or_else(|| "Cannot restore a track lane into an empty project".to_string())?;
        scenes.track_pools[track] = snapshot.pool.clone();
        scenes.track_overrides[track] = snapshot.track_override;
        if scenes.scenes.len() != snapshot.scene_cells.len() {
            return Err("Track history scene topology no longer matches the project".to_string());
        }
        for ((scene, cell), cell_sound) in scenes
            .scenes
            .iter_mut()
            .zip(&snapshot.scene_cells)
            .zip(&snapshot.cell_sounds)
        {
            scene.cells[track] = *cell;
            scene.cell_sounds[track] = *cell_sound;
        }
        debug_assert!(
            scenes.validate_sound_refs().is_ok(),
            "sound refs invalid after track lane restore: {:?}",
            scenes.validate_sound_refs().err()
        );
        let current = scenes.current_scene;
        let live = scenes.scene_snapshot(current)
            .ok_or_else(|| "Current scene is missing during track restore".to_string())?;
        drop(scenes);
        live.restore(self);
        Ok(track)
    }

    pub fn move_appended_track_pattern_lane_to(
        &self,
        target: usize,
        snapshot: &TrackPatternLaneState,
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let last = scenes.track_pools.len().checked_sub(1)
            .ok_or_else(|| "Cannot move a track lane in an empty project".to_string())?;
        if target > last || scenes.scenes.len() != snapshot.scene_references.len() {
            return Err("Track history topology no longer matches the project".to_string());
        }
        scenes.move_track_take_pool(last, target);
        let pool = scenes.track_pools.remove(last);
        scenes.track_pools.insert(target, pool);
        let track_override = scenes.track_overrides.remove(last);
        scenes.track_overrides.insert(target, track_override);
        for ((scene, references), expected_cell) in scenes.scenes.iter_mut()
            .zip(&snapshot.scene_references)
            .zip(&snapshot.scene_cells)
        {
            let cell = scene.cells.remove(last);
            scene.cells.insert(target, cell);
            let cell_sound = scene.cell_sounds.remove(last);
            scene.cell_sounds.insert(target, cell_sound);
            if scene.cells[target] != *expected_cell {
                return Err("Restored Track Pattern assignment changed during insertion".to_string());
            }
            scene.mod_connections = references.mod_connections.clone();
            scene.neural_networks = references.neural_networks.clone();
            scene.graph_overrides = references.graph_overrides.clone();
        }
        for saved in &snapshot.sidechains {
            let Some(patch) = scenes.track_pools.get_mut(saved.owner_track)
                .and_then(|pool| {
                    let refs = pool.refs(saved.pattern)?;
                    pool.sounds.patches.get_mut(&refs.patch)
                }) else {
                return Err(format!(
                    "Sidechain history target track {} pattern {:?} is missing",
                    saved.owner_track + 1,
                    saved.pattern,
                ));
            };
            for (slot, state) in &saved.slots {
                let Some(target_slot) = patch.effect_slots.get_mut(*slot) else {
                    return Err(format!("Sidechain history effect slot {} is missing", slot + 1));
                };
                *target_slot = state.clone();
            }
        }
        debug_assert!(
            scenes.validate_sound_refs().is_ok(),
            "sound refs invalid after track move: {:?}",
            scenes.validate_sound_refs().err()
        );
        let current = scenes.current_scene;
        let live = scenes.scene_snapshot(current)
            .ok_or_else(|| "Current scene is missing during track insertion".to_string())?;
        drop(scenes);
        self.with_committed_song_mut(|song| {
            if let Some(song) = song {
                remap_song_overrides_after_track_move(song, last, target);
            }
        });
        self.with_committed_arrangement_mut(|arrangement| {
            if let Some(arrangement) = arrangement {
                remap_arrangement_after_track_move(arrangement, last, target);
            }
        });
        live.restore(self);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        Ok(())
    }

    pub fn remove_track(
        &self,
        track_idx: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let old_count = self.active_track_count();
        if old_count <= 1 || track_idx >= old_count {
            return false;
        }

        let current_pattern = self.current_scene_index();
        let mut current_snapshot = self.capture_current_pattern_snapshot(
            old_count,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        // Track indices are about to shift; the index-keyed device loans
        // must not survive the shift (the App re-binds on its next sync).
        self.release_bound_device_state();
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes.save_scene_snapshot_masked(current_pattern, current_snapshot.clone(), self.stale_live_lane_mask());
            for owner_track in 0..old_count {
                if owner_track == track_idx {
                    continue;
                }
                let Some(track_descs) = effect_descriptors.get(owner_track) else {
                    continue;
                };
                let Some(pool) = scenes.track_pools.get_mut(owner_track) else {
                    continue;
                };
                // Non-idempotent index remap: visit each Patch entity once
                // (take chunks share one), never once per pattern.
                for patch in pool.sounds.patches.values_mut() {
                    patch.remap_sidechain_references_after_track_delete(
                        owner_track,
                        track_descs,
                        track_idx,
                        old_count,
                    );
                }
            }
            scenes.remove_track(track_idx);
            debug_assert!(
                scenes.validate_sound_refs().is_ok(),
                "sound refs invalid after track delete: {:?}",
                scenes.validate_sound_refs().err()
            );
        }
        self.with_committed_song_mut(|song| {
            if let Some(song) = song {
                remap_song_overrides_after_track_delete(song, track_idx);
            }
        });
        self.with_committed_arrangement_mut(|arrangement| {
            if let Some(arrangement) = arrangement {
                remap_arrangement_after_track_delete(arrangement, track_idx);
            }
        });

        remap_snapshot_sidechain_references_after_track_delete(
            &mut current_snapshot,
            effect_descriptors,
            track_idx,
            old_count,
        );
        current_snapshot.remove_track(track_idx);
        current_snapshot.restore(self);
        // Solo is live-only (§17.8) and absent from the snapshot, so the
        // restore above cannot shift it; move each bit down with its track.
        // The deleted track's own bit simply disappears, and the trailing
        // lane is cleared by `clear_live_track_lane` below.
        for idx in track_idx..old_count - 1 {
            let next = self.pattern.track_params[idx + 1].is_solo();
            self.pattern.track_params[idx].set_solo(next);
        }
        self.shift_runtime_track_bindings_left(track_idx, old_count);
        self.clear_live_track_lane(old_count - 1);
        self.transport
            .num_tracks
            .store((old_count - 1) as u32, Ordering::Release);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }

    pub fn clear_track_in_place(
        &self,
        track_idx: usize,
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let track_count = self.active_track_count();
        if track_idx >= track_count {
            return false;
        }

        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let Some(pool) = scenes.track_pools.get_mut(track_idx) else {
                return false;
            };
            pool.edit_all(|_, data| {
                data.clear(track_idx, effect_descriptors, InstrumentType::Sampler);
            });
        }

        self.clear_live_track_lane(track_idx);
        self.clear_runtime_track_binding_in_place(track_idx);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }
}
