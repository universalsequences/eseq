use super::*;

impl GraphController<'_> {
    pub(super) fn finish_track_registration(
        &mut self,
        registration: TrackRegistration<'_>,
    ) -> Result<(), String> {
        self.app
            .track_registry
            .allocate()
            .map_err(|error| format!("Failed to allocate stable track id: {error:?}"))?;
        let TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids,
            instrument,
        } = registration;
        let instrument_type = match &instrument {
            InstrumentRegistration::Sampler { .. } => InstrumentType::Sampler,
            InstrumentRegistration::Custom { .. } => InstrumentType::Custom,
            InstrumentRegistration::Modulator => InstrumentType::Modulator,
        };
        let run_mode = match &instrument {
            InstrumentRegistration::Custom { run_mode, .. } => *run_mode,
            InstrumentRegistration::Sampler { .. } | InstrumentRegistration::Modulator => {
                CustomInstrumentRunMode::Instrument
            }
        };

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.voice_counts[idx].store(voice_lids.len() as u32, Ordering::Release);
        self.app.state.runtime.sampler_lids[idx]
            .store(voice_lids.first().copied().unwrap_or(0), Ordering::Release);
        self.app.state.runtime.modulator_lids[idx].store(
            if instrument_type == InstrumentType::Modulator {
                shell.mod_env_id as u64
            } else {
                0
            },
            Ordering::Release,
        );
        self.app.state.runtime.pan_lids[idx].store(shell.pan_id as u64, Ordering::Release);
        self.app.state.runtime.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.runtime.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.runtime.instrument_type_flags[idx]
            .store(instrument_type.runtime_flag(), Ordering::Release);
        self.app.state.pattern.instrument_run_modes[idx]
            .store(run_mode.runtime_flag(), Ordering::Release);
        self.app.state.runtime.instrument_run_mode_flags[idx]
            .store(run_mode.runtime_flag(), Ordering::Release);

        self.app.tracks.push(track_name.clone());
        self.app.track_name_user_authored.push(false);
        self.app.push_next_track_color();
        self.app.push_default_track_collapsed();
        self.app.rack_selected_slots.push(0);
        self.app
            .graph
            .effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.app.graph.record_armed.push(false);
        self.app.graph.track_voice_lids.push(voice_lids);
        self.app.graph.track_instrument_types.push(instrument_type);
        self.app.graph.track_instrument_run_modes.push(run_mode);

        match instrument {
            InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids,
                gatepitch_ids,
                modulator_ids,
            } => {
                let instrument_node_id = first_graph_node_identity(&sampler_ids);
                let instrument_modulator_node_id = first_graph_node_identity(&modulator_ids);
                for (v, &sampler_id) in sampler_ids.iter().enumerate() {
                    self.app.state.runtime.synth_node_ids[idx][v]
                        .store(sampler_id as u32, Ordering::Release);
                }
                for (v, &gatepitch_id) in gatepitch_ids.iter().enumerate() {
                    self.app.state.runtime.sampler_gatepitch_node_ids[idx][v]
                        .store(gatepitch_id as u32, Ordering::Release);
                }
                for (v, &modulator_id) in modulator_ids.iter().enumerate() {
                    self.app.state.runtime.sampler_modulator_node_ids[idx][v]
                        .store(modulator_id as u32, Ordering::Release);
                }
                self.app.state.runtime.track_engine_ids[idx].store(u32::MAX, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = None;
                }
                self.app.graph.track_buffer_ids.push(buffer_id);
                self.app.graph.track_sample_rates.push(sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids,
                    sampler_gatepitch_ids: gatepitch_ids.clone(),
                    sampler_modulator_ids: modulator_ids.clone(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    pdc_id: shell.pdc_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                let sampler_desc = EffectDescriptor::builtin_sampler();
                self.app.state.pattern.instrument_slots[idx].apply_descriptor_with_modulator(
                    &sampler_desc,
                    instrument_node_id,
                    instrument_modulator_node_id,
                );
                self.app.graph.instrument_descriptors.push(sampler_desc);
            }
            InstrumentRegistration::Custom {
                engine_id,
                manifest,
                run_mode: _,
            } => {
                self.app.state.runtime.track_engine_ids[idx]
                    .store(engine_id as u32, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = Some(engine_id);
                }
                self.app.graph.track_buffer_ids.push(-1);
                self.app
                    .graph
                    .track_sample_rates
                    .push(self.app.graph.sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    pdc_id: shell.pdc_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                let engine = self.app.graph.engine_node_ids[engine_id]
                    .as_ref()
                    .expect("engine runtime initialized");
                self.app
                    .graph
                    .track_synth_node_ids
                    .push(engine.synth_ids.clone());
                self.app
                    .graph
                    .track_gatepitch_node_ids
                    .push(engine.gatepitch_ids.clone());
                self.app.graph.track_engine_ids.push(Some(engine_id));
                self.initialize_instrument_slot(idx, &track_name, manifest);
            }
            InstrumentRegistration::Modulator => {
                unsafe {
                    crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, shell.mod_env_id);
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        shell.mod_env_id,
                        0,
                        shell.mod_out_id,
                        0,
                    );
                }
                self.app.state.runtime.track_engine_ids[idx].store(u32::MAX, Ordering::Release);
                if let Some(sound) = self
                    .app
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(idx)
                {
                    sound.engine_id = None;
                }
                self.app.graph.track_buffer_ids.push(-1);
                self.app
                    .graph
                    .track_sample_rates
                    .push(self.app.graph.sample_rate);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    pdc_id: shell.pdc_id,
                    send_id: shell.send_id,
                    mod_out_id: shell.mod_out_id,
                    mod_in_clip_ids: shell.mod_in_clip_ids,
                    mod_env_id: shell.mod_env_id,
                    bus_send_ids: Vec::new(),
                    rack_slots: Vec::new(),
                    rack_signature: None,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                let desc = crate::instruments::track_modulator::descriptor();
                self.app.state.pattern.instrument_slots[idx]
                    .apply_descriptor(&desc, shell.mod_env_id as u32);
                self.app.graph.instrument_descriptors.push(desc);
            }
        }

        let instrument_descriptor = self.app.graph.instrument_descriptors[idx].clone();
        let instrument_slot = &self.app.state.pattern.instrument_slots[idx];
        let instrument_node_id = instrument_slot.node_id.load(Ordering::Relaxed);
        let instrument_modulator_node_id =
            instrument_slot.modulator_node_id.load(Ordering::Relaxed);
        self.app.state.extend_all_pattern_snapshots_to_track(
            idx + 1,
            &self.app.graph.effect_descriptors,
            idx,
            run_mode,
            Some((
                &instrument_descriptor,
                instrument_node_id,
                instrument_modulator_node_id,
                instrument_type,
            )),
        )?;
        if let (Some(&buffer_id), Some(&sample_rate)) = (
            self.app.graph.track_buffer_ids.get(idx),
            self.app.graph.track_sample_rates.get(idx),
        ) {
            if buffer_id >= 0 {
                self.app.state.seed_unset_pattern_sample_ids(
                    idx,
                    (buffer_id, self.app.tracks[idx].clone(), sample_rate),
                );
            }
        }
        self.app.refresh_effect_sidechain_labels();

        self.app
            .state
            .transport
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);
        self.app.ui.cursor_track = idx;
        self.app.ui.cursor_step = 0;
        self.app.ui.focused_region = super::super::Region::Cirklon;
        self.app.ui.sidebar_tab = super::super::SidebarTab::Tools;
        self.app.ui.sidebar_mode = match instrument_type {
            InstrumentType::Custom => super::super::SidebarMode::Presets,
            InstrumentType::Sampler | InstrumentType::Modulator | InstrumentType::Rack => {
                super::super::SidebarMode::Audition
            }
        };
        self.app.ui.sidebar_search_focused = false;
        self.app
            .state
            .transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app
            .state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.latch_track_created_during_song_playback(idx);
        self.app.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub(super) fn debug_assert_track_vectors_aligned(&self) {
        debug_assert_eq!(self.app.track_registry.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.track_name_user_authored.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.graph.track_node_ids.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.graph.track_buffer_ids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_sample_rates.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.track_voice_lids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_instrument_types.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.track_instrument_run_modes.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.track_engine_ids.len(), self.app.tracks.len());
        debug_assert_eq!(
            self.app.graph.track_synth_node_ids.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.track_gatepitch_node_ids.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.effect_descriptors.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(
            self.app.graph.instrument_descriptors.len(),
            self.app.tracks.len()
        );
        debug_assert_eq!(self.app.graph.record_armed.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.sampler_paths.len(), self.app.tracks.len());
        debug_assert_eq!(self.app.rack_selected_slots.len(), self.app.tracks.len());
    }

    pub(super) fn initialize_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        self.apply_instrument_slot_descriptor(track, name, manifest, false);
    }

    pub(super) fn sync_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        self.apply_instrument_slot_descriptor(track, name, manifest, true);
    }

    pub(super) fn apply_instrument_slot_descriptor(
        &mut self,
        track: usize,
        name: &str,
        manifest: &DGenManifest,
        preserve_runtime_values: bool,
    ) {
        let inst_desc = lisp_host::instrument_descriptor_from_manifest(name, manifest);
        let inst_slot = &self.app.state.pattern.instrument_slots[track];
        let (node_id, modulator_node_id) = self.instrument_slot_identity(track);
        if preserve_runtime_values {
            if let Some(old_desc) = self.app.graph.instrument_descriptors.get(track) {
                inst_slot.sync_descriptor_by_param_name_with_modulator(
                    old_desc,
                    &inst_desc,
                    node_id,
                    modulator_node_id,
                );
            } else {
                inst_slot.sync_descriptor_with_modulator(&inst_desc, node_id, modulator_node_id);
            }
        } else {
            inst_slot.apply_descriptor_with_modulator(&inst_desc, node_id, modulator_node_id);
        }

        if track < self.app.graph.instrument_descriptors.len() {
            self.app.graph.instrument_descriptors[track] = inst_desc;
        } else {
            self.app.graph.instrument_descriptors.push(inst_desc);
        }
    }

    pub(super) fn instrument_slot_identity(&self, track: usize) -> (u32, u32) {
        let Some(Some(engine_id)) = self.app.graph.track_engine_ids.get(track).copied() else {
            let slot = &self.app.state.pattern.instrument_slots[track];
            return (
                slot.node_id.load(Ordering::Relaxed),
                slot.modulator_node_id.load(Ordering::Relaxed),
            );
        };
        let Some(Some(engine)) = self.app.graph.engine_node_ids.get(engine_id) else {
            return (0, 0);
        };
        (
            first_graph_node_identity(&engine.synth_ids),
            first_graph_node_identity(&engine.modulator_ids),
        )
    }
}
