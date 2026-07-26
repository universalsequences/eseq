use super::super::*;

impl SequencerState {
    /// Validates the shared state lanes needed to replace a track's instrument
    /// source without mutating any state.
    pub(super) fn validate_instrument_source_reset_target(&self, track: usize) -> Result<(), String> {
        let require_track = |len: usize, collection: &str| {
            if track < len {
                Ok(())
            } else {
                Err(format!(
                    "Track {} is missing from {collection} (length {len})",
                    track + 1
                ))
            }
        };
        require_track(self.pattern.instrument_slots.len(), "live instrument slots")?;
        require_track(
            self.pattern.instrument_run_modes.len(),
            "pattern instrument run modes",
        )?;
        require_track(
            self.runtime.instrument_run_mode_flags.len(),
            "runtime instrument run modes",
        )?;
        require_track(
            self.runtime.instrument_type_flags.len(),
            "runtime instrument types",
        )?;
        require_track(
            self.runtime.track_engine_ids.len(),
            "runtime track engine bindings",
        )?;
        require_track(
            self.pattern.track_sound_state.lock().unwrap().len(),
            "track sound state",
        )?;
        require_track(
            self.pattern.process_chains.lock().unwrap().len(),
            "process chains",
        )?;
        require_track(
            self.pattern.rack_tracks.lock().unwrap().len(),
            "rack track state",
        )?;
        require_track(
            self.pattern.plock_variant_registries.lock().unwrap().len(),
            "p-lock variant registries",
        )?;
        require_track(
            self.pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .len(),
            "key-lock variant registries",
        )?;
        require_track(
            self.pattern.scenes.lock().unwrap().track_pools.len(),
            "stored track pattern pools",
        )?;
        Ok(())
    }

    pub fn validate_instrument_slot_reset_target(
        &self,
        track: usize,
        engine_id: usize,
    ) -> Result<(), String> {
        u32::try_from(engine_id)
            .map_err(|_| format!("Instrument engine id {engine_id} exceeds the runtime format"))?;
        self.validate_instrument_source_reset_target(track)
    }

    pub fn validate_sampler_slot_reset_target(&self, track: usize) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)
    }

    pub fn reset_instrument_slot_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        engine_id: usize,
        run_mode: CustomInstrumentRunMode,
    ) -> Option<InstrumentSlotResetSummary> {
        self.validate_instrument_slot_reset_target(track, engine_id)
            .ok()?;
        self.reset_instrument_source_all_patterns(
            track,
            descriptor,
            node_id,
            modulator_node_id,
            InstrumentSourceReset::Custom {
                engine_id,
                run_mode,
            },
        )
    }

    pub fn reset_sampler_slot_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        sample_id: (i32, String, u32),
    ) -> Option<InstrumentSlotResetSummary> {
        self.validate_sampler_slot_reset_target(track).ok()?;
        self.reset_instrument_source_all_patterns(
            track,
            descriptor,
            node_id,
            modulator_node_id,
            InstrumentSourceReset::Sampler { sample_id },
        )
    }

    fn reset_instrument_source_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        source: InstrumentSourceReset,
    ) -> Option<InstrumentSlotResetSummary> {
        let engine_id_flag = source
            .engine_id()
            .and_then(|engine_id| u32::try_from(engine_id).ok())
            .unwrap_or(u32::MAX);
        let live_slot = self.pattern.instrument_slots.get(track)?;
        let live_had_locks = instrument_slot_has_locks(&EffectSlotSnapshot::capture(live_slot));

        live_slot.clear();
        live_slot.apply_descriptor_with_modulator(descriptor, node_id, modulator_node_id);
        self.pattern.instrument_run_modes[track]
            .store(source.run_mode().runtime_flag(), Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track]
            .store(source.run_mode().runtime_flag(), Ordering::Release);
        self.runtime.instrument_type_flags[track]
            .store(source.instrument_type().runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(engine_id_flag, Ordering::Release);

        self.pattern.track_sound_state.lock().unwrap()[track] = TrackSoundState {
            engine_id: source.engine_id(),
            loaded_preset: None,
            dirty: false,
        };
        self.pattern.rack_tracks.lock().unwrap()[track] = None;

        self.pattern.plock_variant_registries.lock().unwrap()[track]
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        self.pattern.key_lock_variant_registries.lock().unwrap()[track]
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);

        let live_instrument_slot = EffectSlotSnapshot::capture(live_slot);
        let mut process_bindings_dropped = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            crate::process::rebind_track_process_chain_instrument_param_ids(
                &mut chains[track],
                descriptor,
                &live_instrument_slot,
            )
        };

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let neural_overrides_dropped = scenes
            .scenes
            .iter_mut()
            .map(|scene| {
                crate::neural::remove_instrument_overrides_for_track(
                    &mut scene.neural_networks,
                    track,
                )
            })
            .sum();
        let effective_pattern_id = scenes.effective_pattern_id(track);
        let pool = &mut scenes.track_pools[track];
        let mut patterns_with_cleared_locks = 0;
        for (pattern_id, data) in &mut pool.patterns {
            let stored_had_locks = instrument_slot_has_locks(&data.instrument_slot);
            let (cleared_locks, dropped_bindings) =
                data.reset_instrument_source(descriptor, node_id, modulator_node_id, &source);
            let cleared_locks = if Some(*pattern_id) == effective_pattern_id {
                live_had_locks || stored_had_locks
            } else {
                cleared_locks
            };
            patterns_with_cleared_locks += usize::from(cleared_locks);
            process_bindings_dropped += dropped_bindings;
        }

        Some(InstrumentSlotResetSummary {
            patterns_reset: pool.patterns.len(),
            patterns_with_cleared_locks,
            process_bindings_dropped,
            neural_overrides_dropped,
        })
    }

    pub fn capture_track_instrument_pattern_state(
        &self,
        track: usize,
    ) -> Result<TrackInstrumentPatternStateSnapshot, String> {
        self.validate_instrument_source_reset_target(track)?;
        let (mut live, patterns, neural_overrides) = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let pool = scenes
                .track_pools
                .get(track)
                .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
            let effective_id = scenes
                .effective_pattern_id(track)
                .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
            let live = pool
                .patterns
                .get(&effective_id)
                .ok_or_else(|| format!("Track {} effective pattern is missing", track + 1))?
                .instrument_state();
            let patterns = pool
                .patterns
                .iter()
                .map(|(id, data)| (*id, data.instrument_state()))
                .collect();
            let mut neural_overrides = Vec::new();
            for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
                for (network_idx, network) in scene.neural_networks.iter().enumerate() {
                    for (neuron_idx, neuron) in network.neurons.iter().enumerate() {
                        let entries = neuron
                            .output_overrides
                            .instrument
                            .iter()
                            .enumerate()
                            .filter(|(_, value)| value.target_track == track)
                            .map(|(idx, value)| (idx, value.clone()))
                            .collect::<Vec<_>>();
                        if !entries.is_empty() {
                            neural_overrides.push(NeuralInstrumentOverrideState {
                                scene: scene_idx,
                                network: network_idx,
                                neuron: neuron_idx,
                                entries,
                            });
                        }
                    }
                }
            }
            (live, patterns, neural_overrides)
        };
        live.instrument_slot = EffectSlotSnapshot::capture(&self.pattern.instrument_slots[track]);
        live.instrument_base_note_offset = f32::from_bits(
            self.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        );
        live.instrument_run_mode = CustomInstrumentRunMode::from_runtime_flag(
            self.pattern.instrument_run_modes[track].load(Ordering::Relaxed),
        );
        live.track_sound_state = self.pattern.track_sound_state.lock().unwrap()[track].clone();
        live.rack_track = self.pattern.rack_tracks.lock().unwrap()[track].clone();
        live.process_chain = self.pattern.process_chains.lock().unwrap()[track].clone();
        live.project_process_lane_overrides =
            self.pattern.project_process_lane_overrides.lock().unwrap()[track].clone();
        live.plock_variant_registry =
            self.pattern.plock_variant_registries.lock().unwrap()[track].clone();
        live.key_lock_variant_registry = self.pattern.key_lock_variant_registries.lock().unwrap()
            [track]
            .clone();

        Ok(TrackInstrumentPatternStateSnapshot {
            live,
            patterns,
            neural_overrides,
        })
    }

    pub fn restore_track_instrument_pattern_state(
        &self,
        track: usize,
        snapshot: &TrackInstrumentPatternStateSnapshot,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) -> Result<(), String> {
        self.validate_track_instrument_pattern_state(track, snapshot, descriptor)?;
        let mut live = snapshot.live.clone();
        live.instrument_slot.sync_to_descriptor_with_modulator(
            descriptor,
            node_id,
            modulator_node_id,
        );
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot
                .patterns
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before instrument history replay",
                track + 1
            ));
        }
        for (id, state) in &snapshot.patterns {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .restore_instrument_state(state, descriptor, node_id, modulator_node_id);
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron
                        .output_overrides
                        .instrument
                        .retain(|value| value.target_track != track);
                }
            }
        }
        for saved in &snapshot.neural_overrides {
            let neuron = scenes
                .scenes
                .get_mut(saved.scene)
                .and_then(|scene| scene.neural_networks.get_mut(saved.network))
                .and_then(|network| network.neurons.get_mut(saved.neuron))
                .ok_or_else(|| {
                    format!(
                        "Track {} neural topology changed before instrument history replay",
                        track + 1
                    )
                })?;
            for (index, value) in &saved.entries {
                let index = (*index).min(neuron.output_overrides.instrument.len());
                let mut value = value.clone();
                let raw_idx = live
                    .instrument_slot
                    .param_node_indices
                    .get(value.param_index)
                    .copied()
                    .unwrap_or(value.param_index as u32);
                value.param_id = crate::neural::ParamNodeId::from_slot_param(
                    live.instrument_slot.node_id,
                    live.instrument_slot.modulator_node_id,
                    raw_idx,
                )
                .ok_or_else(|| {
                    format!(
                        "Track {} instrument parameter {} has no live identity",
                        track + 1,
                        value.param_index
                    )
                })?;
                neuron
                    .output_overrides
                    .instrument
                    .insert(index, value);
            }
        }
        drop(scenes);

        crate::process::refresh_track_process_chain_binding_param_ids(
            &mut live.process_chain,
            Some(descriptor),
            Some(&live.instrument_slot),
            &[],
            &[],
        );
        live.instrument_slot.restore(&self.pattern.instrument_slots[track]);
        self.pattern.instrument_base_note_offsets[track]
            .store(live.instrument_base_note_offset.to_bits(), Ordering::Relaxed);
        self.pattern.instrument_run_modes[track]
            .store(live.instrument_run_mode.runtime_flag(), Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track]
            .store(live.instrument_run_mode.runtime_flag(), Ordering::Release);
        self.pattern.track_sound_state.lock().unwrap()[track] = live.track_sound_state;
        self.pattern.rack_tracks.lock().unwrap()[track] = live.rack_track;
        self.pattern.process_chains.lock().unwrap()[track] = live.process_chain;
        self.pattern.project_process_lane_overrides.lock().unwrap()[track] =
            live.project_process_lane_overrides;
        self.pattern.plock_variant_registries.lock().unwrap()[track] =
            live.plock_variant_registry;
        self.pattern.key_lock_variant_registries.lock().unwrap()[track] =
            live.key_lock_variant_registry;
        Ok(())
    }

    pub fn validate_track_instrument_pattern_state(
        &self,
        track: usize,
        snapshot: &TrackInstrumentPatternStateSnapshot,
        descriptor: &EffectDescriptor,
    ) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot
                .patterns
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before instrument history replay",
                track + 1
            ));
        }
        for saved in &snapshot.neural_overrides {
            if scenes
                .scenes
                .get(saved.scene)
                .and_then(|scene| scene.neural_networks.get(saved.network))
                .and_then(|network| network.neurons.get(saved.neuron))
                .is_none()
            {
                return Err(format!(
                    "Track {} neural topology changed before instrument history replay",
                    track + 1
                ));
            }
            if saved.entries.iter().any(|(_, value)| {
                value.param_index >= descriptor.params.len()
                    || value.target_track != track
            }) {
                return Err(format!(
                    "Track {} neural instrument override no longer matches its descriptor",
                    track + 1
                ));
            }
        }
        Ok(())
    }

    pub fn copy_current_rack_slot_instrument_values_to_all_track_patterns(
        &self,
        track: usize,
        rack_slot_idx: usize,
    ) -> usize {
        let source = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(rack_slot_idx))
            .cloned();
        let Some(source) = source else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            let Some(slot) = pattern
                .rack_track
                .as_mut()
                .and_then(|rack| rack.slots.get_mut(rack_slot_idx))
            else {
                continue;
            };
            slot.instrument_slot
                .copy_base_values_from(&source.instrument_slot);
            slot.instrument_base_note_offset = source.instrument_base_note_offset;
            updated += 1;
        }
        updated
    }

    pub fn sync_effect_slot_with_modulator_in_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.sync_effect_slot_with_modulator(slot_idx, descriptor, node_id, modulator_node_id);
            data.refresh_process_effect_binding_param_ids_for_slot(slot_idx, descriptor);
        });
    }

    pub fn normalize_current_pattern_instrument_run_mode(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<(), String> {
        self.extend_all_pattern_snapshots_to_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            None,
        )?;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(id) = scenes.effective_pattern_id(track) else {
            return Ok(());
        };
        if let Some(data) = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
        {
            data.instrument_run_mode = run_mode;
        }
        Ok(())
    }
}
