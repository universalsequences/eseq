use super::*;

impl GraphController<'_> {
    pub(super) fn create_dedicated_engine_descriptor_from(
        &mut self,
        engine_id: usize,
    ) -> Result<usize, String> {
        if self.app.editor.engine_registry.engines.len()
            >= self.app.state.runtime.engine_voice_lids.len()
        {
            return Err(format!(
                "Instrument engine runtime slots are exhausted; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }
        let descriptor = self
            .app
            .editor
            .engine_registry
            .get(engine_id)
            .cloned()
            .ok_or_else(|| format!("Missing instrument engine descriptor {engine_id}"))?;
        let dedicated_id = self.app.editor.engine_registry.upsert(EngineDescriptor {
            name: descriptor.name,
            source: descriptor.source,
            manifest: descriptor.manifest,
            lib_index: descriptor.lib_index,
            shared_runtime: false,
        });
        if dedicated_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slots are exhausted; cannot create dedicated free-patch engine {dedicated_id}"
            ));
        }
        Ok(dedicated_id)
    }

    pub(super) fn ensure_track_uses_dedicated_engine(&mut self, track: usize) -> Result<(), String> {
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Ok(());
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let descriptor = self
            .app
            .editor
            .engine_registry
            .get(old_engine_id)
            .cloned()
            .ok_or_else(|| format!("Missing instrument engine descriptor {old_engine_id}"))?;
        if !descriptor.shared_runtime {
            return Ok(());
        }
        if descriptor.lib_index >= self.app.editor.instrument_libs.len() {
            return Err(format!(
                "Instrument engine {old_engine_id} references missing library {}",
                descriptor.lib_index
            ));
        }

        let dedicated_engine_id = self.create_dedicated_engine_descriptor_from(old_engine_id)?;
        let manifest = descriptor.manifest;
        let name = descriptor.name;
        let lib_ptr: *const LoadedDGenLib = &self.app.editor.instrument_libs[descriptor.lib_index];
        let track_name = self.app.tracks[track].clone();
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Missing graph nodes for track {}", track + 1))?;

        self.delete_track_engine_routes(track);
        if !self.engine_is_still_referenced_excluding(old_engine_id, track) {
            self.delete_engine_runtime(old_engine_id);
        }

        self.ensure_custom_engine_runtime(dedicated_engine_id, &name, &manifest, unsafe {
            &*lib_ptr
        })?;
        self.connect_engine_to_track(
            dedicated_engine_id,
            track,
            track,
            &track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;

        self.app.graph.track_engine_ids[track] = Some(dedicated_engine_id);
        self.app.state.runtime.track_engine_ids[track]
            .store(dedicated_engine_id as u32, Ordering::Release);
        if let Some(engine) = self.app.graph.engine_node_ids[dedicated_engine_id].as_ref() {
            self.app.graph.track_synth_node_ids[track] = engine.synth_ids.clone();
            self.app.graph.track_gatepitch_node_ids[track] = engine.gatepitch_ids.clone();
        }
        if let Some(sound) = self
            .app
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            sound.engine_id = Some(dedicated_engine_id);
        }
        Ok(())
    }

    pub(super) fn set_engine_voice_route_to_track(
        &self,
        engine_id: usize,
        voice_idx: usize,
        track_idx: usize,
        value: f32,
    ) {
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
        else {
            return;
        };
        if let Some(route_pair) = engine
            .route_gain_ids
            .get(track_idx)
            .and_then(|routes| routes.get(voice_idx))
        {
            for &route_id in route_pair {
                if route_id > 0 {
                    push_graph_param(self.app.graph.lg.0, route_id as u64, 0, value);
                }
            }
        }
        if let Some(ext_routes) = engine
            .ext_route_gain_ids
            .get(track_idx)
            .and_then(|routes| routes.get(voice_idx))
        {
            for &route_id in ext_routes {
                if route_id > 0 {
                    push_graph_param(self.app.graph.lg.0, route_id as u64, 0, value);
                }
            }
        }
    }

    pub(super) fn route_free_patch_idle_voice_to_track(
        &self,
        engine_id: usize,
        track: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        if engine.synth_ids.is_empty() || engine.gatepitch_ids.is_empty() {
            return Err(format!(
                "Instrument engine {engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let transport_playing = self.app.state.transport.playing.load(Ordering::Acquire);
        for track_idx in 0..self.app.tracks.len() {
            let value = free_patch_idle_route_value(track_idx, track, transport_playing);
            self.set_engine_voice_route_to_track(engine_id, 0, track_idx, value);
        }
        Ok(())
    }

    pub(super) fn close_free_patch_idle_route(&self, track: usize) {
        let Some(engine_id) = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
        else {
            return;
        };
        self.set_engine_voice_route_to_track(engine_id, 0, track, 0.0);
    }

    pub(super) fn dispatch_instrument_defaults_to_engine_voice(
        &self,
        track: usize,
        engine_id: usize,
        voice_idx: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        let synth_id =
            engine.synth_ids.get(voice_idx).copied().ok_or_else(|| {
                format!("Missing synth node for engine {engine_id} voice {voice_idx}")
            })? as u64;
        let modulator_id = engine.modulator_ids.get(voice_idx).copied().unwrap_or(0) as u64;
        let slot = &self.app.state.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        let mut param_indices = (0..num_params).collect::<Vec<_>>();
        param_indices.sort_by_key(|param_idx| slot.resolve_node_idx(*param_idx));
        for param_idx in param_indices {
            let idx = slot.resolve_node_idx(param_idx);
            let is_mod_param = idx as u32 >= crate::instruments::voice_modulator::MOD_PARAM_BASE;
            let logical_id = if is_mod_param { modulator_id } else { synth_id };
            let resolved_idx = if is_mod_param {
                idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64
            } else {
                idx
            };
            push_graph_param_span(
                self.app.graph.lg.0,
                logical_id,
                resolved_idx,
                slot.resolve_node_span(param_idx),
                slot.defaults.get(param_idx),
            );
        }
        Ok(())
    }

    pub(super) fn push_free_patch_idle_gatepitch(&self, engine_id: usize) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {engine_id}"))?;
        let gatepitch_id = engine
            .gatepitch_ids
            .first()
            .copied()
            .ok_or_else(|| format!("Missing gatepitch node for engine {engine_id} voice 0"))?
            as u64;
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_TRIGGER,
            0.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_PITCH,
            440.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_VELOCITY,
            1.0,
        );
        push_graph_param(
            self.app.graph.lg.0,
            gatepitch_id,
            crate::effects::gatepitch::PARAM_GATE,
            0.0,
        );
        Ok(())
    }

    pub(super) fn apply_free_patch_idle_voice(&self, track: usize) -> Result<(), String> {
        let engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        self.route_free_patch_idle_voice_to_track(engine_id, track)?;
        self.dispatch_instrument_defaults_to_engine_voice(track, engine_id, 0)?;
        self.push_free_patch_idle_gatepitch(engine_id)?;
        lisp_host::set_dgen_engine_enabled_voices(engine_id, 1);
        Ok(())
    }

    pub fn set_track_instrument_run_mode(
        &mut self,
        track: usize,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<(), String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        let normalized_mode =
            if self.app.graph.track_instrument_types.get(track) == Some(&InstrumentType::Custom) {
                run_mode
            } else {
                CustomInstrumentRunMode::Instrument
            };

        if normalized_mode == CustomInstrumentRunMode::FreePatch {
            self.ensure_track_uses_dedicated_engine(track)?;
        }

        let previous_mode = self
            .app
            .graph
            .track_instrument_run_modes
            .get(track)
            .copied()
            .unwrap_or(CustomInstrumentRunMode::Instrument);
        if let Some(mode) = self.app.graph.track_instrument_run_modes.get_mut(track) {
            *mode = normalized_mode;
        }
        self.app.state.pattern.instrument_run_modes[track]
            .store(normalized_mode.runtime_flag(), Ordering::Relaxed);
        self.app.state.runtime.instrument_run_mode_flags[track]
            .store(normalized_mode.runtime_flag(), Ordering::Release);

        self.app
            .state
            .normalize_current_pattern_instrument_run_mode(
                self.app.tracks.len(),
                &self.app.graph.effect_descriptors,
                track,
                normalized_mode,
            )?;
        if normalized_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)?;
        } else if previous_mode == CustomInstrumentRunMode::FreePatch {
            // Only close the idle route when actually leaving free-patch mode;
            // closing it unconditionally mutes voice 0 mid-note on live tracks.
            self.close_free_patch_idle_route(track);
        }
        self.app.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn sync_track_instrument_run_modes_from_live_state(&mut self) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        self.app
            .graph
            .track_instrument_run_modes
            .resize(track_count, CustomInstrumentRunMode::Instrument);
        for track in 0..track_count {
            let mode = CustomInstrumentRunMode::from_runtime_flag(
                self.app.state.pattern.instrument_run_modes[track].load(Ordering::Relaxed),
            );
            let mode = if self.app.graph.track_instrument_types.get(track)
                == Some(&InstrumentType::Custom)
            {
                mode
            } else {
                CustomInstrumentRunMode::Instrument
            };
            if mode == CustomInstrumentRunMode::FreePatch {
                self.ensure_track_uses_dedicated_engine(track)?;
            }
            let previous_mode = self.app.graph.track_instrument_run_modes[track];
            self.app.graph.track_instrument_run_modes[track] = mode;
            self.app.state.runtime.instrument_run_mode_flags[track]
                .store(mode.runtime_flag(), Ordering::Release);
            if mode == CustomInstrumentRunMode::FreePatch {
                self.apply_free_patch_idle_voice(track)?;
            } else if previous_mode == CustomInstrumentRunMode::FreePatch {
                self.close_free_patch_idle_route(track);
            }
        }
        Ok(())
    }

    pub(super) fn rebind_live_track_runtime_after_delete(&mut self) {
        // Topology changed (delete/reorder): index-keyed device loans are
        // meaningless across the shift — drop them; the reactive tick's
        // binding sync re-borrows against the new lane order.
        self.app.state.release_bound_device_state();
        let mut track_sound_state = self.app.state.pattern.track_sound_state.lock().unwrap();

        for track_idx in 0..self.app.tracks.len() {
            if let (Some(nodes), Some(descs), Some(chain)) = (
                self.app.graph.track_node_ids.get(track_idx),
                self.app.graph.effect_descriptors.get(track_idx),
                self.app.state.pattern.effect_chains.get(track_idx),
            ) {
                for (slot_idx, slot) in chain.iter().enumerate() {
                    let Some(desc) = descs.get(slot_idx) else {
                        continue;
                    };
                    let node_id = slot.node_id.load(Ordering::Relaxed);
                    slot.sync_descriptor(desc, node_id);
                }
            }

            let engine_id = self
                .app
                .graph
                .track_engine_ids
                .get(track_idx)
                .and_then(|id| *id);
            self.app.state.runtime.track_engine_ids[track_idx].store(
                engine_id.map(|id| id as u32).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            let run_mode = self
                .app
                .graph
                .track_instrument_run_modes
                .get(track_idx)
                .copied()
                .unwrap_or(CustomInstrumentRunMode::Instrument);
            self.app.state.runtime.instrument_run_mode_flags[track_idx]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            if let Some(meta) = track_sound_state.get_mut(track_idx) {
                meta.engine_id = engine_id;
            }

            if self.app.graph.track_instrument_types.get(track_idx)
                == Some(&crate::sequencer::InstrumentType::Custom)
            {
                if let Some(desc) = self.app.graph.instrument_descriptors.get(track_idx) {
                    let node_id = self.app.state.pattern.instrument_slots[track_idx]
                        .node_id
                        .load(Ordering::Relaxed);
                    self.app.state.pattern.instrument_slots[track_idx]
                        .sync_descriptor(desc, node_id);
                }
            }
        }
        drop(track_sound_state);
        self.rebind_rack_sampler_runtime_pools();
    }

    pub(super) fn rebind_rack_sampler_runtime_pools(&mut self) {
        self.clear_all_rack_sampler_runtime_pools();
        let scheduler_snapshot = self.app.state.latest_scheduler_snapshot();
        for track_idx in 0..self.app.graph.track_node_ids.len() {
            self.publish_rack_slot_panner_runtime(track_idx);
            let slot_count = self.app.graph.track_node_ids[track_idx].rack_slots.len();
            for slot_idx in 0..slot_count {
                let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                    continue;
                };
                let (sampler_ids, gatepitch_ids, modulator_ids, voice_lids, has_sampler_slot) = {
                    let slot = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
                    (
                        slot.sampler_ids.clone(),
                        slot.sampler_gatepitch_ids.clone(),
                        slot.sampler_modulator_ids.clone(),
                        slot.sampler_voice_lids.clone(),
                        slot.sampler_pool_id.is_some(),
                    )
                };
                if !has_sampler_slot {
                    continue;
                }
                self.publish_sampler_voice_runtime(
                    pool_id,
                    &voice_lids,
                    &sampler_ids,
                    &gatepitch_ids,
                    &modulator_ids,
                );
                if let Some(buffer_id) = scheduler_snapshot
                    .tracks
                    .get(track_idx)
                    .and_then(|track| track.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(slot_idx))
                    .and_then(|slot| slot.sample_id.as_ref())
                    .map(|sample| sample.0)
                {
                    self.app.publish_sampler_analysis_pool_runtime(pool_id, buffer_id);
                }
                self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx].sampler_pool_id =
                    Some(pool_id);
            }
        }
    }

    pub(super) fn publish_rack_slot_panner_runtime(&self, track_idx: usize) {
        let Some(track_nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return;
        };
        for slot_idx in 0..MAX_RACK_SLOTS {
            let lid = track_nodes
                .rack_slots
                .get(slot_idx)
                .map(|slot| slot.slot_pan_id as u64)
                .unwrap_or(0);
            self.app.state.runtime.rack_slot_pan_lids[track_idx][slot_idx]
                .store(lid, Ordering::Release);
        }
    }

    pub(super) fn clear_rack_sampler_runtime_pools_for_track(&self, track_idx: usize) {
        for slot_idx in 0..MAX_RACK_SLOTS {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                continue;
            };
            self.clear_sampler_runtime_pool(pool_id);
        }
    }

}
