use super::*;

impl GraphController<'_> {
    pub(super) fn validate_rack_slot_graph_rebuild(
        &self,
        track_idx: usize,
        rack: &RackTrackSnapshot,
    ) -> Result<(), String> {
        if self.app.graph.track_node_ids.get(track_idx).is_none() {
            return Err(format!("Track {} has no graph nodes", track_idx + 1));
        }
        if rack.slots.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            match slot.instrument_type {
                InstrumentType::Sampler => {
                    if slot.sample_id.is_none() {
                        return Err(format!(
                            "Rack sampler layer {} is missing sample metadata",
                            slot_idx + 1
                        ));
                    }
                    if rack_slot_pool_index(track_idx, slot_idx).is_none() {
                        return Err(format!(
                            "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
                        ));
                    }
                }
                InstrumentType::Custom | InstrumentType::Modulator => {
                    let engine_id = slot.track_sound_state.engine_id.ok_or_else(|| {
                        format!(
                            "Rack instrument layer {} is missing engine metadata",
                            slot_idx + 1
                        )
                    })?;
                    let descriptor =
                        self.app
                            .editor
                            .engine_registry
                            .get(engine_id)
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer {} references missing engine {}",
                                    slot_idx + 1,
                                    engine_id
                                )
                            })?;
                    if descriptor.lib_index >= self.app.editor.instrument_libs.len() {
                        return Err(format!(
                            "Rack instrument layer {} references missing engine library {}",
                            slot_idx + 1,
                            descriptor.lib_index
                        ));
                    }
                }
                InstrumentType::Rack => {
                    return Err("Nested rack layers are not supported".to_string());
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply_rack_scene_state_in_place(
        &mut self,
        track_idx: usize,
        rack: &mut RackTrackSnapshot,
    ) -> Result<Vec<(EffectDescriptor, u32, u32)>, String> {
        let mut bindings = Vec::with_capacity(rack.slots.len());

        for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
            let nodes = self
                .app
                .graph
                .track_node_ids
                .get(track_idx)
                .and_then(|track| track.rack_slots.get(slot_idx))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Rack track {} is missing live nodes for slot {}",
                        track_idx + 1,
                        slot_idx + 1
                    )
                })?;

            let (descriptor, node_id, modulator_node_id) = match slot.instrument_type {
                InstrumentType::Sampler => {
                    let (buffer_id, _sample_name, sample_rate) =
                        slot.sample_id.as_ref().ok_or_else(|| {
                            format!(
                                "Rack sampler layer {} is missing sample metadata",
                                slot_idx + 1
                            )
                        })?;
                    for &logical_id in &nodes.sampler_voice_lids {
                        unsafe {
                            crate::audiograph::params_push_wrapper(
                                self.app.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    idx: crate::instruments::sampler::PARAM_BUFFER_ID,
                                    logical_id,
                                    fvalue: *buffer_id as f32,
                                },
                            );
                            crate::audiograph::params_push_wrapper(
                                self.app.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    idx: crate::instruments::sampler::PARAM_SOURCE_SAMPLE_RATE,
                                    logical_id,
                                    fvalue: (*sample_rate).max(1) as f32,
                                },
                            );
                        }
                    }
                    (
                        EffectDescriptor::builtin_sampler(),
                        first_graph_node_identity(&nodes.sampler_ids),
                        first_graph_node_identity(&nodes.sampler_modulator_ids),
                    )
                }
                InstrumentType::Custom | InstrumentType::Modulator => {
                    let engine_id = nodes.engine_id.ok_or_else(|| {
                        format!(
                            "Rack instrument layer {} has no live engine binding",
                            slot_idx + 1
                        )
                    })?;
                    let engine_descriptor = self
                        .app
                        .editor
                        .engine_registry
                        .get(engine_id)
                        .ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} references missing engine {}",
                                slot_idx + 1,
                                engine_id
                            )
                        })?;
                    let engine = self
                        .app
                        .graph
                        .engine_node_ids
                        .get(engine_id)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} is missing live engine runtime {}",
                                slot_idx + 1,
                                engine_id
                            )
                        })?;
                    (
                        lisp_host::instrument_descriptor_from_manifest(
                            &engine_descriptor.name,
                            &engine_descriptor.manifest,
                        ),
                        first_graph_node_identity(&engine.synth_ids),
                        first_graph_node_identity(&engine.modulator_ids),
                    )
                }
                InstrumentType::Rack => {
                    return Err("Nested rack layers are not supported".to_string());
                }
            };

            slot.instrument_slot.sync_to_descriptor_with_modulator(
                &descriptor,
                node_id,
                modulator_node_id,
            );
            bindings.push((descriptor, node_id, modulator_node_id));

            for (param, value) in [
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    slot.gain,
                ),
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    slot.pan,
                ),
                (
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                    if slot.mute { 1.0 } else { 0.0 },
                ),
            ] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: param,
                            logical_id: nodes.slot_pan_id as u64,
                            fvalue: value,
                        },
                    );
                }
            }
        }

        let has_solo = rack.slots.iter().any(|slot| slot.solo);
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            let nodes = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                        logical_id: nodes.slot_pan_id as u64,
                        fvalue: if has_solo && !slot.solo { 1.0 } else { 0.0 },
                    },
                );
            }
        }
        self.publish_rack_slot_panner_runtime(track_idx);

        Ok(bindings)
    }

    pub(super) fn rebuild_rack_slot_graph(
        &mut self,
        track_idx: usize,
        rack: &mut RackTrackSnapshot,
    ) -> Result<Vec<(EffectDescriptor, u32, u32)>, String> {
        self.validate_rack_slot_graph_rebuild(track_idx, rack)?;
        let old_engine_ids = self.rack_engine_ids_for_track(track_idx);
        let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
        let has_solo = rack.slots.iter().any(|slot| slot.solo);
        let mut rebuilt_nodes = Vec::with_capacity(rack.slots.len());
        let mut bindings = Vec::with_capacity(rack.slots.len());

        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.retire_rack_slot_graph_generation(track_idx);

            for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
                let slot_name = format!("{}_rack{}", self.app.tracks[track_idx], slot_idx + 1);
                let mixer = self.create_rack_slot_mixer(
                    &slot_name,
                    track_nodes.voice_sum_id,
                    track_nodes.voice_sum_r_id,
                    slot.gain,
                    slot.pan,
                    slot.mute,
                    has_solo && !slot.solo,
                )?;

                match slot.instrument_type {
                    InstrumentType::Sampler => {
                        let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                            return Err(format!(
                                "Rack sampler pool unavailable for track {track_idx} slot {slot_idx}"
                            ));
                        };
                        let (buffer_id, _sample_name, sample_rate) =
                            slot.sample_id.clone().ok_or_else(|| {
                                format!(
                                    "Rack sampler layer {} is missing sample metadata",
                                    slot_idx + 1
                                )
                            })?;
                        let voices = self.build_sampler_voices(
                            pool_id,
                            &slot_name,
                            buffer_id,
                            sample_rate,
                            mixer.slot_sum_l_id,
                            mixer.slot_sum_r_id,
                            track_nodes.mod_in_clip_ids,
                            slot.max_polyphony,
                        )?;
                        self.publish_sampler_voice_runtime(
                            pool_id,
                            &voices.voice_lids,
                            &voices.sampler_ids,
                            &voices.gatepitch_ids,
                            &voices.modulator_ids,
                        );
                        let descriptor = EffectDescriptor::builtin_sampler();
                        let node_id = first_graph_node_identity(&voices.sampler_ids);
                        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
                        slot.instrument_slot.sync_to_descriptor_with_modulator(
                            &descriptor,
                            node_id,
                            modulator_node_id,
                        );
                        bindings.push((descriptor, node_id, modulator_node_id));
                        rebuilt_nodes.push(RackSlotNodeIds {
                            sampler_pool_id: Some(pool_id),
                            engine_id: None,
                            sampler_voice_lids: voices.voice_lids,
                            sampler_ids: voices.sampler_ids,
                            sampler_gatepitch_ids: voices.gatepitch_ids,
                            sampler_modulator_ids: voices.modulator_ids,
                            slot_sum_l_id: mixer.slot_sum_l_id,
                            slot_sum_r_id: mixer.slot_sum_r_id,
                            slot_pan_id: mixer.slot_pan_id,
                        });
                    }
                    InstrumentType::Custom | InstrumentType::Modulator => {
                        let engine_id = slot.track_sound_state.engine_id.ok_or_else(|| {
                            format!(
                                "Rack instrument layer {} is missing engine metadata",
                                slot_idx + 1
                            )
                        })?;
                        let engine_descriptor = self
                            .app
                            .editor
                            .engine_registry
                            .get(engine_id)
                            .cloned()
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer {} references missing engine {}",
                                    slot_idx + 1,
                                    engine_id
                                )
                            })?;
                        if self
                            .app
                            .graph
                            .engine_node_ids
                            .get(engine_id)
                            .and_then(|engine| engine.as_ref())
                            .is_none()
                        {
                            let lib_index = engine_descriptor.lib_index;
                            let lib_ptr: *const LoadedDGenLib =
                                &self.app.editor.instrument_libs[lib_index];
                            unsafe {
                                self.ensure_custom_engine_runtime(
                                    engine_id,
                                    &engine_descriptor.name,
                                    &engine_descriptor.manifest,
                                    &*lib_ptr,
                                )?;
                            }
                        }
                        self.connect_engine_to_track(
                            engine_id,
                            rack_slot_pool_index(track_idx, slot_idx).ok_or_else(|| {
                                format!("Rack slot {} has no route-consumer identity", slot_idx + 1)
                            })?,
                            track_idx,
                            &slot_name,
                            mixer.slot_sum_l_id,
                            mixer.slot_sum_r_id,
                            track_nodes.mod_out_id,
                            track_nodes.mod_in_clip_ids,
                        )?;
                        let engine = self.app.graph.engine_node_ids[engine_id]
                            .as_ref()
                            .ok_or_else(|| {
                                format!(
                                    "Rack instrument layer '{}' failed to initialize engine {}",
                                    engine_descriptor.name, engine_id
                                )
                            })?;
                        let descriptor = lisp_host::instrument_descriptor_from_manifest(
                            &engine_descriptor.name,
                            &engine_descriptor.manifest,
                        );
                        let node_id = first_graph_node_identity(&engine.synth_ids);
                        let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
                        slot.instrument_slot.sync_to_descriptor_with_modulator(
                            &descriptor,
                            node_id,
                            modulator_node_id,
                        );
                        bindings.push((descriptor, node_id, modulator_node_id));
                        rebuilt_nodes.push(RackSlotNodeIds {
                            sampler_pool_id: None,
                            engine_id: Some(engine_id),
                            sampler_voice_lids: Vec::new(),
                            sampler_ids: Vec::new(),
                            sampler_gatepitch_ids: Vec::new(),
                            sampler_modulator_ids: Vec::new(),
                            slot_sum_l_id: mixer.slot_sum_l_id,
                            slot_sum_r_id: mixer.slot_sum_r_id,
                            slot_pan_id: mixer.slot_pan_id,
                        });
                    }
                    InstrumentType::Rack => {
                        return Err("Nested rack layers are not supported".to_string());
                    }
                }
            }
            self.app.graph.track_node_ids[track_idx].rack_slots = rebuilt_nodes;
            for (slot_idx, slot) in rack.slots.iter().enumerate() {
                let nodes = &self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx];
                let host = FxChainHost {
                    locator: FxChainLocator::RackSlot {
                        track: track_idx,
                        slot: slot_idx,
                    },
                    label: format!("Track {} Rack Slot {}", track_idx + 1, slot_idx + 1),
                    predecessor: StereoEndpoint {
                        node_id: nodes.slot_pan_id,
                        channels: 2,
                    },
                    successor: ChainSuccessor::MonoPair {
                        left: track_nodes.voice_sum_id,
                        right: track_nodes.voice_sum_r_id,
                    },
                    slots: slot
                        .effect_slots
                        .iter()
                        .zip(&slot.effect_descriptors)
                        .map(|(effect, descriptor)| FxChainSlotView {
                            node_id: effect.node_id as i32,
                            modulator_node_id: effect.modulator_node_id as i32,
                            input_channels: descriptor.input_channels,
                            output_channels: descriptor.output_channels,
                        })
                        .collect(),
                };
                connect_fx_chain_host(self.app.graph.lg.0, &host);
            }
            self.publish_rack_slot_panner_runtime(track_idx);
        }

        self.app.graph.track_node_ids[track_idx].rack_signature =
            Some(rack_topology_signature(rack));

        for engine_id in old_engine_ids.iter().copied() {
            if !self.engine_is_still_referenced(engine_id) {
                // A deferred custom generation has no scheduler-visible work
                // left. Keep its graph nodes allocated for safe reap, but stop
                // running an otherwise-idle instrument voice in the meantime.
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if !self.engine_is_still_referenced(engine_id)
                && !self.engine_has_deferred_route_generation(engine_id)
            {
                self.delete_engine_runtime(engine_id);
            }
        }

        Ok(bindings)
    }

    pub(super) fn retire_rack_slot_graph_generation(&mut self, track_idx: usize) {
        let old_rack_slots = self.app.graph.track_node_ids[track_idx].rack_slots.clone();
        let old_engine_routes = old_rack_slots
            .iter()
            .enumerate()
            .filter_map(|(slot_idx, slot)| {
                let engine_id = slot.engine_id?;
                self.retire_custom_rack_slot_output(slot);
                let route_idx = rack_slot_pool_index(track_idx, slot_idx)?;
                self.detach_engine_route_generation(engine_id, route_idx, track_idx)
            })
            .collect::<Vec<_>>();
        if !old_rack_slots.is_empty() || !old_engine_routes.is_empty() {
            self.enqueue_deferred_rack_teardown(DeferredRackTeardown {
                slots: old_rack_slots,
                engine_routes: old_engine_routes,
                track_idx,
                due_at: Instant::now() + RACK_TEARDOWN_TAIL,
            });
        }
        self.clear_rack_sampler_runtime_pools_for_track(track_idx);
    }

}
