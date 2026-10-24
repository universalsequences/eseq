use super::*;

impl GraphController<'_> {
    pub fn add_sampler_slot_to_rack(
        &mut self,
        track_idx: usize,
        wav_path: &Path,
    ) -> Result<usize, String> {
        let sample = self.app.load_rack_sample(wav_path)?;
        let slot_idx = self.add_sampler_slot_to_rack_buffer(
            track_idx,
            sample.buffer_id,
            sample.sample_rate,
            &sample.name,
        )?;
        self.app.register_rack_sample(&sample);
        Ok(slot_idx)
    }

    pub fn add_sampler_slot_to_rack_buffer(
        &mut self,
        track_idx: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<usize, String> {
        let (rack, slot_idx) = self.rack_slot_append_target(track_idx)?;
        let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
            return Err(format!(
                "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
            ));
        };
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .ok_or_else(|| format!("Track {} has no graph nodes", track_idx + 1))?
            .clone();
        let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
        let mixer = self.create_rack_slot_mixer(
            &slot_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            rack.slots.iter().any(|slot| slot.solo),
        )?;
        let max_polyphony = appended_rack_slot_max_polyphony(&rack.slots);
        let voices = self.build_sampler_voices(
            pool_id,
            &slot_name,
            buffer_id,
            sample_rate,
            mixer.slot_sum_l_id,
            mixer.slot_sum_r_id,
            track_nodes.mod_in_clip_ids,
            max_polyphony,
        )?;
        self.publish_sampler_voice_runtime(
            pool_id,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );

        let mut instrument_slot = EffectSlotSnapshot::new_default_with_modulator(
            &EffectDescriptor::builtin_sampler(),
            first_graph_node_identity(&voices.sampler_ids),
            first_graph_node_identity(&voices.modulator_ids),
        );
        instrument_slot.sync_to_descriptor_with_modulator(
            &EffectDescriptor::builtin_sampler(),
            first_graph_node_identity(&voices.sampler_ids),
            first_graph_node_identity(&voices.modulator_ids),
        );
        let rack_slot = RackSlotSnapshot {
            id: 0,
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot,
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
        };
        let rack_slot_nodes = RackSlotNodeIds {
            sampler_pool_id: Some(pool_id),
            engine_id: None,
            sampler_voice_lids: voices.voice_lids,
            sampler_ids: voices.sampler_ids,
            sampler_gatepitch_ids: voices.gatepitch_ids,
            sampler_modulator_ids: voices.modulator_ids,
            slot_sum_l_id: mixer.slot_sum_l_id,
            slot_sum_r_id: mixer.slot_sum_r_id,
            slot_pan_id: mixer.slot_pan_id,
        };
        self.app.graph.track_node_ids[track_idx]
            .rack_slots
            .push(rack_slot_nodes);
        self.publish_rack_slot_panner_runtime(track_idx);
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app
            .state
            .append_rack_slot_for_all_pattern_snapshots(track_idx, rack_slot);
        self.refresh_rack_signature_from_live_state(track_idx);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_event_compatible_topology();
        Ok(slot_idx)
    }

    pub fn delete_rack_slot(&mut self, track_idx: usize, slot_idx: usize) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let mut rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if slot_idx >= rack.slots.len() {
            return Err("Invalid rack layer".to_string());
        }
        let active_effect_slots = rack.slots[slot_idx]
            .effect_slots
            .iter()
            .enumerate()
            .filter_map(|(idx, effect)| (effect.node_id != 0).then_some(idx))
            .collect::<Vec<_>>();
        for effect_slot in active_effect_slots {
            self.app
                .delete_rack_slot_effect_slot(track_idx, slot_idx, effect_slot)?;
        }
        self.app.editor.effect_chain_leases.retire_host(
            FxChainLocator::RackSlot {
                track: track_idx,
                slot: slot_idx,
            },
            0,
        )?;
        rack.slots.remove(slot_idx);
        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut rack)?;
        if !self
            .app
            .state
            .remove_rack_slot_from_all_pattern_snapshots(track_idx, slot_idx)
        {
            return Err("Failed to remove rack layer from all patterns".to_string());
        }
        self.app
            .editor
            .effect_chain_leases
            .reindex_rack_slots_after_delete(track_idx, slot_idx);
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync rack layer bindings to all patterns".to_string());
        }
        let next_selection = if rack.slots.is_empty() {
            0
        } else {
            slot_idx.min(rack.slots.len() - 1)
        };
        self.app.set_rack_selected_slot(track_idx, next_selection);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_event_compatible_topology();
        self.app.push_all_restored_defaults();
        Ok(())
    }

    pub fn add_custom_slot_to_rack(
        &mut self,
        track_idx: usize,
        instrument_name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<usize, String> {
        let (rack, slot_idx) = self.rack_slot_append_target(track_idx)?;
        let route_idx = rack_slot_pool_index(track_idx, slot_idx)
            .ok_or_else(|| format!("Rack slot {} has no route-consumer identity", slot_idx + 1))?;

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(engine_id, instrument_name, manifest, lib)?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .ok_or_else(|| format!("Track {} has no graph nodes", track_idx + 1))?
            .clone();
        let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
        let mixer = self.create_rack_slot_mixer(
            &slot_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            rack.slots.iter().any(|slot| slot.solo),
        )?;
        self.connect_engine_to_track(
            engine_id,
            route_idx,
            track_idx,
            &slot_name,
            mixer.slot_sum_l_id,
            mixer.slot_sum_r_id,
            track_nodes.mod_out_ids,
            track_nodes.mod_in_clip_ids,
        )?;

        let engine = self.app.graph.engine_node_ids[engine_id]
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "Rack custom slot '{}' failed to initialize engine {}",
                    instrument_name, engine_id
                )
            })?;
        let desc = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let node_id = first_graph_node_identity(&engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
        let mut instrument_slot =
            EffectSlotSnapshot::new_default_with_modulator(&desc, node_id, modulator_node_id);
        instrument_slot.sync_to_descriptor_with_modulator(&desc, node_id, modulator_node_id);

        let rack_slot = RackSlotSnapshot {
            id: 0,
            instrument_type: InstrumentType::Custom,
            instrument_run_mode: run_mode,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony: crate::audio::DEFAULT_RACK_SLOT_MAX_POLYPHONY,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot,
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id: Some(engine_id),
                loaded_preset: Some(instrument_name.to_string()),
                dirty: false,
            },
            sample_id: None,
        };
        let rack_slot_nodes = RackSlotNodeIds {
            sampler_pool_id: None,
            engine_id: Some(engine_id),
            sampler_voice_lids: Vec::new(),
            sampler_ids: Vec::new(),
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            slot_sum_l_id: mixer.slot_sum_l_id,
            slot_sum_r_id: mixer.slot_sum_r_id,
            slot_pan_id: mixer.slot_pan_id,
        };
        self.app.graph.track_node_ids[track_idx]
            .rack_slots
            .push(rack_slot_nodes);
        self.publish_rack_slot_panner_runtime(track_idx);
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app
            .state
            .append_rack_slot_for_all_pattern_snapshots(track_idx, rack_slot);
        self.refresh_rack_signature_from_live_state(track_idx);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_event_compatible_topology();
        Ok(slot_idx)
    }

    pub(crate) fn replace_layer_rack_slot_source(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
        scope: RackSlotReplaceScope,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let mut rack = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        let existing = rack
            .slots
            .get(slot_idx)
            .cloned()
            .ok_or_else(|| "Invalid rack layer".to_string())?;
        rack.slots[slot_idx] = preserve_rack_slot_configuration(replacement, &existing);

        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut rack)?;
        let replacement = rack.slots[slot_idx].clone();
        let replaced = match scope {
            // A sample onto a sampler slot is content (rig spec §4): that
            // pattern's (a kit pad's: that clip's) sample only.
            RackSlotReplaceScope::CurrentPattern => self
                .app
                .state
                .replace_rack_slot_source_in_current_pattern(track_idx, slot_idx, replacement),
            // A new device: a slot id is one device in every pattern (§3.2).
            RackSlotReplaceScope::AllPatterns => self
                .app
                .state
                .replace_rack_slot_source_for_all_pattern_snapshots(track_idx, slot_idx, replacement),
        };
        if !replaced {
            return Err("Failed to replace rack layer source".to_string());
        }
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync rack layer bindings to all patterns".to_string());
        }
        self.app.set_rack_selected_slot(track_idx, slot_idx);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_event_compatible_topology();
        self.app.push_all_restored_defaults();
        Ok(())
    }

    pub fn replace_rack_slot_with_sampler(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        wav_path: &Path,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        if self
            .app
            .state
            .live_rack_track_snapshot(track_idx)
            .is_none_or(|rack| rack.slots.get(slot_idx).is_none())
        {
            return Err("Invalid instrument rack layer".to_string());
        }
        let sample = self.app.load_rack_sample(wav_path)?;
        self.replace_layer_rack_slot_source(
            track_idx,
            slot_idx,
            sampler_rack_slot_snapshot(sample.buffer_id, sample.sample_rate, &sample.name),
            RackSlotReplaceScope::CurrentPattern,
        )?;
        self.app.register_rack_sample(&sample);
        Ok(())
    }

    pub fn replace_rack_slot_with_sampler_buffer(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        self.replace_layer_rack_slot_source(
            track_idx,
            slot_idx,
            sampler_rack_slot_snapshot(buffer_id, sample_rate, sample_name),
            RackSlotReplaceScope::CurrentPattern,
        )
    }

    pub fn replace_rack_slot_with_custom(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
        instrument_name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<(), String> {
        self.replace_layer_rack_slot_source(
            track_idx,
            slot_idx,
            custom_rack_slot_snapshot(instrument_name, engine_id, manifest, run_mode),
            RackSlotReplaceScope::CurrentPattern,
        )
    }

    /// Insert `slot` (a fresh device, `sampler_rack_slot_snapshot` /
    /// `custom_rack_slot_snapshot`) at `position` of rack track `track_idx`,
    /// in every pattern under one new id, stored disabled (eseq-ka5t.9, rig
    /// spec §4 "replace for this bank"). `controls_from` names the slot whose
    /// strip controls it starts with (a position before the insert).
    ///
    /// Everything that addresses a slot by position moves with its device:
    /// rack macro targets (`insert_rack_slot_for_all_pattern_snapshots`),
    /// process bindings and fan-outs, the global macros' slot mappings, the
    /// slot-FX lease hosts, the device registry's slot identities (whose
    /// effect identities key by slot identity, so slot-FX instance records
    /// follow), the slot graph nodes (rebuilt, effects reconnected in place)
    /// and the selection. Returns the new slot's position.
    pub fn insert_rack_slot(
        &mut self,
        track_idx: usize,
        position: usize,
        slot: RackSlotSnapshot,
        controls_from: Option<usize>,
    ) -> Result<usize, String> {
        let (rack, append_at) = self.rack_slot_append_target(track_idx)?;
        if position > append_at {
            return Err(format!("Rack slot position {} is past the rack", position + 1));
        }
        let mut planned = rack.clone();
        planned.slots.insert(position, slot.clone());
        self.validate_rack_slot_graph_rebuild(track_idx, &planned)?;
        let track_id = self
            .app
            .track_registry
            .id_at(track_idx)
            .ok_or_else(|| format!("Track {} has no stable identity", track_idx + 1))?;
        self.app
            .state
            .insert_rack_slot_for_all_pattern_snapshots(track_idx, position, slot, controls_from)
            .ok_or_else(|| "Failed to insert the rack layer into every pattern".to_string())?;
        let shift = crate::process::RackSlotShift::Inserted(position);
        self.app.state.shift_rack_slot_process_targets(track_idx, shift);
        self.app.macro_engine.remap_rack_slot_mappings(track_idx, shift);
        self.app
            .editor
            .effect_chain_leases
            .reindex_rack_slots_after_insert(track_idx, position);
        self.app.device_registry.insert_rack_slot(track_id, position);
        let mut live = self
            .app
            .state
            .live_rack_track_snapshot(track_idx)
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        let bindings = self.rebuild_rack_slot_graph(track_idx, &mut live)?;
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
        {
            return Err("Failed to sync rack layer bindings to all patterns".to_string());
        }
        self.app.set_rack_selected_slot(track_idx, position);
        self.app.revalidate_live_macro_targets();
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_event_compatible_topology();
        self.app.push_all_restored_defaults();
        Ok(position)
    }

    pub fn hot_reload_instrument(
        &mut self,
        track: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        if track >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if self.app.graph.track_instrument_types[track] != InstrumentType::Custom {
            return Err("Not a custom instrument track".to_string());
        }

        let Some(engine_id) = self.app.graph.track_engine_ids[track] else {
            return Err("Custom track has no engine binding".to_string());
        };

        self.rebuild_custom_engine_runtime(engine_id, manifest, lib)?;

        for bound_track in 0..self.app.tracks.len() {
            if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id)) {
                let track_name = self.app.tracks[bound_track].clone();
                self.sync_instrument_slot(bound_track, &track_name, manifest);
            }
        }

        for bound_track in 0..self.app.tracks.len() {
            if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id))
                && self
                    .app
                    .graph
                    .track_instrument_run_modes
                    .get(bound_track)
                    .copied()
                    == Some(CustomInstrumentRunMode::FreePatch)
            {
                self.apply_free_patch_idle_voice(bound_track)?;
            }
        }

        Ok(())
    }

}

/// Which patterns a slot source replacement writes (rig spec §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RackSlotReplaceScope {
    /// The current pattern only: a sample onto a sampler slot (content).
    CurrentPattern,
    /// Every pattern of the track: a new device in the slot.
    AllPatterns,
}

/// A fresh sampler slot holding `sample_name` (its instrument params bind
/// when the slot graph is built).
pub(crate) fn sampler_rack_slot_snapshot(
    buffer_id: i32,
    sample_rate: u32,
    sample_name: &str,
) -> RackSlotSnapshot {
    RackSlotSnapshot {
        id: 0,
        instrument_type: InstrumentType::Sampler,
        instrument_run_mode: CustomInstrumentRunMode::Instrument,
        instrument_base_note_offset: 0.0,
        choke_group: None,
        gain: 1.0,
        pan: 0.0,
        mute: false,
        solo: false,
        enabled: true,
        max_polyphony: crate::audio::DEFAULT_RACK_SLOT_MAX_POLYPHONY,
        param_plocks: RackSlotParamPlocks::new(),
        instrument_slot: EffectSlotSnapshot::new_empty(),
        effect_slots: RackSlotSnapshot::empty_effect_slots(),
        effect_descriptors: EffectDescriptor::default_full_chain(),
        custom_effect_names: RackSlotSnapshot::empty_effect_names(),
        track_sound_state: TrackSoundState::default(),
        sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
    }
}

/// A fresh custom-instrument slot running engine `engine_id`.
pub(crate) fn custom_rack_slot_snapshot(
    instrument_name: &str,
    engine_id: usize,
    manifest: &DGenManifest,
    run_mode: CustomInstrumentRunMode,
) -> RackSlotSnapshot {
    let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
    RackSlotSnapshot {
        id: 0,
        instrument_type: InstrumentType::Custom,
        instrument_run_mode: run_mode,
        instrument_base_note_offset: 0.0,
        choke_group: None,
        gain: 1.0,
        pan: 0.0,
        mute: false,
        solo: false,
        enabled: true,
        max_polyphony: crate::audio::DEFAULT_RACK_SLOT_MAX_POLYPHONY,
        param_plocks: RackSlotParamPlocks::new(),
        instrument_slot: EffectSlotSnapshot::new_default_with_modulator(&descriptor, 0, 0),
        effect_slots: RackSlotSnapshot::empty_effect_slots(),
        effect_descriptors: EffectDescriptor::default_full_chain(),
        custom_effect_names: RackSlotSnapshot::empty_effect_names(),
        track_sound_state: TrackSoundState {
            engine_id: Some(engine_id),
            loaded_preset: Some(instrument_name.to_string()),
            dirty: false,
        },
        sample_id: None,
    }
}
