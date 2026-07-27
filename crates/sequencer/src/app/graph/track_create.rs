use super::*;

impl GraphController<'_> {
    pub fn add_track(&mut self, wav_path: &Path) -> Result<usize, String> {
        let loaded = crate::instruments::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let track_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        self.add_track_from_sample(
            wav_path,
            loaded.buffer_id,
            loaded.sample_rate,
            track_name,
        )
    }

    /// Build a sampler track around an existing immutable graph buffer.
    ///
    /// The caller owns sample loading and analysis submission. Project loading
    /// uses this path so every reference to one WAV can share one buffer.
    pub(in crate::app) fn add_track_from_sample(
        &mut self,
        wav_path: &Path,
        buffer_id: i32,
        sample_rate: u32,
        track_name: String,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let shell = self.create_track_shell(idx, &track_name)?;
        let voices = self.build_sampler_voices(
            idx,
            &track_name,
            buffer_id,
            sample_rate,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids: voices.sampler_ids,
                gatepitch_ids: voices.gatepitch_ids,
                modulator_ids: voices.modulator_ids,
            },
        })?;
        let sample_path = wav_path.to_path_buf();
        let sample_name = self.app.tracks[idx].clone();
        self.app.sampler_paths.push(Some(sample_path.clone()));
        self.app
            .register_loaded_sample_path(&sample_name, buffer_id, sample_path);
        self.app.reset_sampler_bpm_for_analysis(idx);
        self.app.publish_sampler_analysis_runtime(idx);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_blank_sampler_track(&mut self) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let buffer_id = crate::instruments::sampler::create_silent_buffer(self.app.graph.lg.0)?;
        let sample_rate = self.app.graph.sample_rate;
        let track_name = format!("Sampler {}", idx + 1);
        let shell = self.create_track_shell(idx, &track_name)?;
        let voices = self.build_sampler_voices(
            idx,
            &track_name,
            buffer_id,
            sample_rate,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sample_rate,
                sampler_ids: voices.sampler_ids,
                gatepitch_ids: voices.gatepitch_ids,
                modulator_ids: voices.modulator_ids,
            },
        })?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_modulator_track(&mut self) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = format!("Modulator {}", idx + 1);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Modulator,
        })?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_custom_track(
        &mut self,
        name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = instrument_display_name(name);
        let shell = self.create_track_shell(idx, &track_name)?;
        self.ensure_custom_engine_runtime(engine_id, name, manifest, lib)?;
        self.connect_engine_to_track(
            engine_id,
            idx,
            idx,
            &track_name,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
            shell.mod_out_id,
            shell.mod_in_clip_ids,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Custom {
                engine_id,
                manifest,
                run_mode,
            },
        })?;
        self.app.sampler_paths.push(None);
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(idx)?;
        }
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn swap_custom_track_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(format!(
                "Track {} is not a custom instrument track",
                track + 1
            ));
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        for (len, collection) in [
            (
                self.app.graph.track_instrument_run_modes.len(),
                "instrument run modes",
            ),
            (
                self.app.graph.track_synth_node_ids.len(),
                "track synth node ids",
            ),
            (
                self.app.graph.track_gatepitch_node_ids.len(),
                "track gatepitch node ids",
            ),
            (
                self.app.graph.instrument_descriptors.len(),
                "instrument descriptors",
            ),
        ] {
            if track >= len {
                return Err(format!(
                    "Track {} is missing from {collection} (length {len})",
                    track + 1
                ));
            }
        }
        self.app
            .state
            .validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let track_name = self.app.tracks[track].clone();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let (new_synth_ids, new_gatepitch_ids, node_id, modulator_node_id) = {
            self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;

            let new_engine = self.app.graph.engine_node_ids[new_engine_id]
                .as_ref()
                .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
            let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
                format!(
                    "Track {} is outside engine {new_engine_id}'s route table",
                    track + 1
                )
            })?;
            let existing_ext_routes =
                new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
                    format!(
                        "Track {} is outside engine {new_engine_id}'s external route table",
                        track + 1
                    )
                })?;
            if new_engine_id != old_engine_id
                && (!existing_routes.is_empty() || !existing_ext_routes.is_empty())
            {
                return Err(format!(
                    "Instrument engine {new_engine_id} already has a route for track {}",
                    track + 1
                ));
            }

            let old_engine = self
                .app
                .graph
                .engine_node_ids
                .get(old_engine_id)
                .and_then(|engine| engine.as_ref())
                .ok_or_else(|| format!("Missing runtime for instrument engine {old_engine_id}"))?;
            if old_engine
                .route_gain_ids
                .get(track)
                .is_none_or(|routes| routes.len() != MAX_VOICES)
            {
                return Err(format!(
                    "Instrument engine {old_engine_id} does not have a complete route for track {}",
                    track + 1
                ));
            }

            let should_delete_old_runtime = new_engine_id != old_engine_id
                && !self.engine_is_still_referenced_excluding(old_engine_id, track);
            if new_engine_id != old_engine_id {
                let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
                let route_transaction_commands =
                    (route_nodes + route_connections)
                        .checked_mul(2)
                        .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
                let old_route_delete_commands =
                    engine_route_delete_command_count(old_engine, track);
                let old_runtime_delete_commands = if should_delete_old_runtime {
                    engine_runtime_delete_command_count_excluding_track(old_engine, track)
                } else {
                    0
                };
                let required_commands = route_transaction_commands
                    .checked_add(old_route_delete_commands)
                    .and_then(|count| count.checked_add(old_runtime_delete_commands))
                    .ok_or_else(|| "Instrument swap graph capacity overflow".to_string())?;
                require_graph_edit_queue_capacity(
                    self.app.graph.lg.0,
                    required_commands,
                    "Instrument swap",
                )?;
            }

            if run_mode == CustomInstrumentRunMode::FreePatch
                && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
            {
                return Err(format!(
                    "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
                ));
            }
            let new_synth_ids = new_engine.synth_ids.clone();
            let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
            let node_id = first_graph_node_identity(&new_engine.synth_ids);
            let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);

            self.connect_engine_to_track(
                new_engine_id,
                track,
                track,
                &track_name,
                track_nodes.voice_sum_id,
                track_nodes.voice_sum_r_id,
                track_nodes.mod_out_id,
                track_nodes.mod_in_clip_ids,
            )?;

            let new_engine = self.app.graph.engine_node_ids[new_engine_id]
                .as_ref()
                .expect("new engine runtime was validated above");
            debug_assert_eq!(
                new_engine.route_gain_ids[track].len(),
                MAX_VOICES,
                "successful route construction must publish every voice"
            );

            if new_engine_id != old_engine_id {
                self.delete_engine_route_for_track(old_engine_id, track, track);
                if should_delete_old_runtime {
                    self.delete_engine_runtime(old_engine_id);
                }
            }
            (new_synth_ids, new_gatepitch_ids, node_id, modulator_node_id)
        };

        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_instrument_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                new_engine_id,
                run_mode,
            )
            .expect("instrument reset target was validated before graph mutation");

        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn replace_track_with_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        match self.app.graph.track_instrument_types.get(track).copied() {
            Some(InstrumentType::Custom) => self.swap_custom_track_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(InstrumentType::Sampler) => self.convert_sampler_track_to_custom_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(InstrumentType::Rack) => self.convert_rack_track_to_custom_instrument(
                track,
                instrument_name,
                new_engine_id,
                manifest,
                lib,
                run_mode,
            ),
            Some(other) => Err(format!(
                "Track {} has instrument type {other:?}, which cannot be replaced with a custom instrument",
                track + 1
            )),
            None => Err(format!("Invalid track index {}", track + 1)),
        }
    }

    pub(super) fn convert_sampler_track_to_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Sampler) {
            return Err(format!("Track {} is not a sampler track", track + 1));
        }
        if self.app.graph.track_engine_ids.get(track) != Some(&None) {
            return Err(format!(
                "Sampler track {} has an unexpected custom engine binding",
                track + 1
            ));
        }
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        if track_nodes.sampler_ids.is_empty()
            || track_nodes.sampler_gatepitch_ids.len() != track_nodes.sampler_ids.len()
            || track_nodes.sampler_modulator_ids.len() != track_nodes.sampler_ids.len()
        {
            return Err(format!(
                "Sampler track {} does not have a complete voice pool",
                track + 1
            ));
        }
        self.app
            .state
            .validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let old_track_name = self.app.tracks[track].clone();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;
        let new_engine = self.app.graph.engine_node_ids[new_engine_id]
            .as_ref()
            .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
        let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s route table",
                track + 1
            )
        })?;
        let existing_ext_routes = new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s external route table",
                track + 1
            )
        })?;
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "Instrument engine {new_engine_id} already has a route for track {}",
                track + 1
            ));
        }
        if run_mode == CustomInstrumentRunMode::FreePatch
            && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
        {
            return Err(format!(
                "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
        let route_transaction_commands = (route_nodes + route_connections)
            .checked_mul(2)
            .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
        let required_commands = route_transaction_commands
            .checked_add(sampler_voice_delete_command_count(&track_nodes))
            .ok_or_else(|| "Sampler conversion graph capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Sampler-to-instrument conversion",
        )?;

        let new_synth_ids = new_engine.synth_ids.clone();
        let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
        let node_id = first_graph_node_identity(&new_engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);
        self.connect_engine_to_track(
            new_engine_id,
            track,
            track,
            &old_track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;
        self.delete_sampler_voice_nodes(&track_nodes);
        self.clear_sampler_runtime_pool(track);

        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.sampler_ids.clear();
        live_nodes.sampler_gatepitch_ids.clear();
        live_nodes.sampler_modulator_ids.clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Custom;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_instrument_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                new_engine_id,
                run_mode,
            )
            .expect("instrument reset target was validated before graph mutation");
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.app.publish_sampler_analysis_runtime(track);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub(super) fn convert_rack_track_to_custom_instrument(
        &mut self,
        track: usize,
        instrument_name: &str,
        new_engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        run_mode: CustomInstrumentRunMode,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err(format!("Track {} is not an instrument rack", track + 1));
        }
        if self.app.graph.track_engine_ids.get(track) != Some(&None) {
            return Err(format!(
                "Rack track {} has an unexpected flat engine binding",
                track + 1
            ));
        }
        let track_id = self.app.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let track_nodes = self.app.graph.track_node_ids.get(track).cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        self.app.state.validate_instrument_slot_reset_target(track, new_engine_id)?;
        if new_engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {new_engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }

        let descriptor = lisp_host::instrument_descriptor_from_manifest(instrument_name, manifest);
        let old_track_name = self.app.tracks[track].clone();
        let old_engine_ids = self.rack_engine_ids_for_track(track);
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        self.ensure_custom_engine_runtime(new_engine_id, instrument_name, manifest, lib)?;
        let new_engine = self.app.graph.engine_node_ids[new_engine_id]
            .as_ref()
            .ok_or_else(|| format!("Missing runtime for instrument engine {new_engine_id}"))?;
        let existing_routes = new_engine.route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s route table",
                track + 1
            )
        })?;
        let existing_ext_routes = new_engine.ext_route_gain_ids.get(track).ok_or_else(|| {
            format!(
                "Track {} is outside engine {new_engine_id}'s external route table",
                track + 1
            )
        })?;
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "Instrument engine {new_engine_id} already has a route for track {}",
                track + 1
            ));
        }
        if run_mode == CustomInstrumentRunMode::FreePatch
            && (new_engine.synth_ids.is_empty() || new_engine.gatepitch_ids.is_empty())
        {
            return Err(format!(
                "Instrument engine {new_engine_id} has no voice 0 runtime for free-patch mode"
            ));
        }
        let (route_nodes, route_connections) = engine_route_build_capacities(new_engine);
        let required_commands = (route_nodes + route_connections)
            .checked_mul(2)
            .ok_or_else(|| "Instrument route capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Rack-to-instrument conversion",
        )?;

        let new_synth_ids = new_engine.synth_ids.clone();
        let new_gatepitch_ids = new_engine.gatepitch_ids.clone();
        let node_id = first_graph_node_identity(&new_engine.synth_ids);
        let modulator_node_id = first_graph_node_identity(&new_engine.modulator_ids);
        self.connect_engine_to_track(
            new_engine_id,
            track,
            track,
            &old_track_name,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_out_id,
            track_nodes.mod_in_clip_ids,
        )?;
        self.delete_rack_effect_chains(track, batch.serial)?;
        self.retire_rack_slot_graph_generation(track);

        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.rack_slots.clear();
        live_nodes.rack_signature = None;
        self.app.graph.track_instrument_types[track] = InstrumentType::Custom;
        self.app.graph.track_instrument_run_modes[track] = run_mode;
        self.app.graph.track_engine_ids[track] = Some(new_engine_id);
        self.app.graph.track_synth_node_ids[track] = new_synth_ids;
        self.app.graph.track_gatepitch_node_ids[track] = new_gatepitch_ids;
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self.app.state.reset_instrument_slot_all_patterns(
            track,
            &descriptor,
            node_id,
            modulator_node_id,
            new_engine_id,
            run_mode,
        ).expect("instrument reset target was validated before graph mutation");
        if run_mode == CustomInstrumentRunMode::FreePatch {
            self.apply_free_patch_idle_voice(track)
                .expect("free-patch engine runtime was validated before graph mutation");
        }
        drop(batch);

        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if engine_id != new_engine_id && !self.engine_is_still_referenced(engine_id) {
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.app.tracks[track] = instrument_display_name(instrument_name);
        self.app.device_registry.clear_rack_track(track_id);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn convert_custom_track_to_sampler(
        &mut self,
        track: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if track >= self.app.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(format!(
                "Track {} is not a custom instrument track",
                track + 1
            ));
        }
        let old_engine_id = self
            .app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
        let track_nodes = self
            .app
            .graph
            .track_node_ids
            .get(track)
            .cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        self.app.state.validate_sampler_slot_reset_target(track)?;
        let old_engine = self
            .app
            .graph
            .engine_node_ids
            .get(old_engine_id)
            .and_then(|engine| engine.as_ref())
            .ok_or_else(|| format!("Missing runtime for instrument engine {old_engine_id}"))?;
        if old_engine
            .route_gain_ids
            .get(track)
            .is_none_or(|routes| routes.len() != MAX_VOICES)
        {
            return Err(format!(
                "Instrument engine {old_engine_id} does not have a complete route for track {}",
                track + 1
            ));
        }
        let should_delete_old_runtime =
            !self.engine_is_still_referenced_excluding(old_engine_id, track);
        let (sampler_nodes, sampler_connections) = sampler_voice_build_capacities(MAX_VOICES);
        let sampler_transaction_commands = (sampler_nodes + sampler_connections)
            .checked_mul(2)
            .ok_or_else(|| "Sampler voice capacity overflow".to_string())?;
        let old_route_delete_commands = engine_route_delete_command_count(old_engine, track);
        let old_runtime_delete_commands = if should_delete_old_runtime {
            engine_runtime_delete_command_count_excluding_track(old_engine, track)
        } else {
            0
        };
        let required_commands = sampler_transaction_commands
            .checked_add(old_route_delete_commands)
            .and_then(|count| count.checked_add(old_runtime_delete_commands))
            .ok_or_else(|| "Instrument-to-sampler graph capacity overflow".to_string())?;

        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Instrument-to-sampler conversion",
        )?;
        let voices = self.build_sampler_voices(
            track,
            sample_name,
            buffer_id,
            sample_rate,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.delete_engine_route_for_track(old_engine_id, track, track);
        if should_delete_old_runtime {
            self.delete_engine_runtime(old_engine_id);
        }

        let descriptor = EffectDescriptor::builtin_sampler();
        let node_id = first_graph_node_identity(&voices.sampler_ids);
        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
        self.publish_sampler_voice_runtime(
            track,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );
        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.sampler_ids = voices.sampler_ids;
        live_nodes.sampler_gatepitch_ids = voices.gatepitch_ids;
        live_nodes.sampler_modulator_ids = voices.modulator_ids;
        self.app.graph.track_voice_lids[track] = voices.voice_lids;
        self.app.graph.track_buffer_ids[track] = buffer_id;
        self.app.graph.track_sample_rates[track] = sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Sampler;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self
            .app
            .state
            .reset_sampler_slot_all_patterns(
                track,
                &descriptor,
                node_id,
                modulator_node_id,
                (buffer_id, sample_name.to_string(), sample_rate),
            )
            .expect("sampler reset target was validated before graph mutation");
        self.app.tracks[track] = sample_name.to_string();
        self.app.reset_sampler_bpm_for_analysis(track);
        self.app.publish_sampler_analysis_runtime(track);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub(super) fn delete_sampler_voice_nodes(&self, track: &TrackNodeIds) {
        for node_id in track
            .sampler_ids
            .iter()
            .chain(&track.sampler_gatepitch_ids)
            .chain(&track.sampler_modulator_ids)
        {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, *node_id);
            }
        }
    }

    pub(super) fn finish_track_instrument_source_change(&mut self, track: usize) {
        self.app.sync_scratch_runtime_descriptors();
        self.app
            .macro_engine
            .remove_instrument_mappings_for_track(track);
        self.app.push_instrument_defaults_for_track(track);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
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
        // Publish only after both epochs advance. The scheduler stamps queued
        // events with snapshot.pattern_epoch while the audio callback rejects
        // events against the live atomic epoch; publishing the old epoch here
        // silences the swapped track until another transport action republishes.
        self.app
            .state
            .publish_macro_overrides(self.app.macro_engine.override_snapshot());
    }

    pub fn add_rack_track(
        &mut self,
        name: &str,
        routing: RackRouting,
        slots: Vec<RackSlotBuildSpec<'_>>,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }
        if slots.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        validate_rack_build_slot_pad_map(routing, &slots)?;
        self.force_reap_all_rack_teardowns();
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        let track_name = instrument_display_name(name);
        let shell = self.create_track_shell(idx, &track_name)?;
        let has_solo = slots.iter().any(|slot| slot.solo);
        let mut rack_slot_nodes = Vec::with_capacity(slots.len());
        let mut rack_slot_snapshots = Vec::with_capacity(slots.len());

        for (slot_idx, slot) in slots.into_iter().enumerate() {
            let slot_name = format!("{}_rack{}", track_name, slot_idx + 1);
            let mixer = self.create_rack_slot_mixer(
                &slot_name,
                shell.voice_sum_id,
                shell.voice_sum_r_id,
                slot.gain,
                slot.pan,
                slot.mute,
                has_solo && !slot.solo,
            )?;
            let max_polyphony = slot.max_polyphony.clamp(1, MAX_VOICES);
            match slot.instrument {
                RackSlotInstrumentBuildSpec::Sampler(sampler) => {
                    let Some(pool_id) = rack_slot_pool_index(idx, slot_idx) else {
                        return Err(format!(
                            "Rack sampler pool unavailable for track {idx} slot {slot_idx}"
                        ));
                    };
                    let voices = self.build_sampler_voices(
                        pool_id,
                        &slot_name,
                        sampler.buffer_id,
                        sampler.sample_rate,
                        mixer.slot_sum_l_id,
                        mixer.slot_sum_r_id,
                        shell.mod_in_clip_ids,
                        max_polyphony,
                    )?;
                    self.publish_sampler_voice_runtime(
                        pool_id,
                        &voices.voice_lids,
                        &voices.sampler_ids,
                        &voices.gatepitch_ids,
                        &voices.modulator_ids,
                    );
                    let mut instrument_slot = slot.instrument_slot.unwrap_or_else(|| {
                        EffectSlotSnapshot::new_default_with_modulator(
                            &EffectDescriptor::builtin_sampler(),
                            first_graph_node_identity(&voices.sampler_ids),
                            first_graph_node_identity(&voices.modulator_ids),
                        )
                    });
                    instrument_slot.sync_to_descriptor_with_modulator(
                        &EffectDescriptor::builtin_sampler(),
                        first_graph_node_identity(&voices.sampler_ids),
                        first_graph_node_identity(&voices.modulator_ids),
                    );
                    let sample_id = Some((
                        sampler.buffer_id,
                        sampler.sample_name.clone(),
                        sampler.sample_rate,
                    ));
                    rack_slot_snapshots.push(RackSlotSnapshot {
                        instrument_type: InstrumentType::Sampler,
                        instrument_run_mode: CustomInstrumentRunMode::Instrument,
                        instrument_base_note_offset: slot.instrument_base_note_offset,
                        pad_note: slot.pad_note,
                        choke_group: slot.choke_group,
                        gain: slot.gain,
                        pan: slot.pan.clamp(-1.0, 1.0),
                        mute: slot.mute,
                        solo: slot.solo,
                        max_polyphony,
                        param_plocks: slot.param_plocks.unwrap_or_default(),
                        instrument_slot,
                        effect_slots: slot
                            .effect_slots
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_slots),
                        effect_descriptors: slot
                            .effect_descriptors
                            .unwrap_or_else(EffectDescriptor::default_full_chain),
                        custom_effect_names: slot
                            .custom_effect_names
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_names),
                        track_sound_state: slot.track_sound_state.unwrap_or_default(),
                        sample_id,
                    });
                    rack_slot_nodes.push(RackSlotNodeIds {
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
                RackSlotInstrumentBuildSpec::Custom(custom) => {
                    let route_idx = rack_slot_pool_index(idx, slot_idx).ok_or_else(|| {
                        format!("Rack slot {} has no route-consumer identity", slot_idx + 1)
                    })?;
                    self.ensure_custom_engine_runtime(
                        custom.engine_id,
                        custom.instrument_name,
                        custom.manifest,
                        custom.lib,
                    )?;
                    self.connect_engine_to_track(
                        custom.engine_id,
                        route_idx,
                        idx,
                        &slot_name,
                        mixer.slot_sum_l_id,
                        mixer.slot_sum_r_id,
                        shell.mod_out_id,
                        shell.mod_in_clip_ids,
                    )?;
                    let engine = self.app.graph.engine_node_ids[custom.engine_id]
                        .as_ref()
                        .ok_or_else(|| {
                            format!(
                                "Rack custom slot '{}' failed to initialize engine {}",
                                custom.instrument_name, custom.engine_id
                            )
                        })?;
                    let desc = lisp_host::instrument_descriptor_from_manifest(
                        custom.instrument_name,
                        custom.manifest,
                    );
                    let node_id = first_graph_node_identity(&engine.synth_ids);
                    let modulator_node_id = first_graph_node_identity(&engine.modulator_ids);
                    let mut instrument_slot = slot.instrument_slot.unwrap_or_else(|| {
                        EffectSlotSnapshot::new_default_with_modulator(
                            &desc,
                            node_id,
                            modulator_node_id,
                        )
                    });
                    instrument_slot.sync_to_descriptor_with_modulator(
                        &desc,
                        node_id,
                        modulator_node_id,
                    );
                    let mut sound_state = slot.track_sound_state.unwrap_or_default();
                    sound_state.engine_id = Some(custom.engine_id);
                    rack_slot_snapshots.push(RackSlotSnapshot {
                        instrument_type: InstrumentType::Custom,
                        instrument_run_mode: custom.run_mode,
                        instrument_base_note_offset: slot.instrument_base_note_offset,
                        pad_note: slot.pad_note,
                        choke_group: slot.choke_group,
                        gain: slot.gain,
                        pan: slot.pan.clamp(-1.0, 1.0),
                        mute: slot.mute,
                        solo: slot.solo,
                        max_polyphony,
                        param_plocks: slot.param_plocks.unwrap_or_default(),
                        instrument_slot,
                        effect_slots: slot
                            .effect_slots
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_slots),
                        effect_descriptors: slot
                            .effect_descriptors
                            .unwrap_or_else(EffectDescriptor::default_full_chain),
                        custom_effect_names: slot
                            .custom_effect_names
                            .unwrap_or_else(RackSlotSnapshot::empty_effect_names),
                        track_sound_state: sound_state,
                        sample_id: None,
                    });
                    rack_slot_nodes.push(RackSlotNodeIds {
                        sampler_pool_id: None,
                        engine_id: Some(custom.engine_id),
                        sampler_voice_lids: Vec::new(),
                        sampler_ids: Vec::new(),
                        sampler_gatepitch_ids: Vec::new(),
                        sampler_modulator_ids: Vec::new(),
                        slot_sum_l_id: mixer.slot_sum_l_id,
                        slot_sum_r_id: mixer.slot_sum_r_id,
                        slot_pan_id: mixer.slot_pan_id,
                    });
                }
            }
        }

        let rack_track = RackTrackSnapshot {
            routing,
            slots: rack_slot_snapshots,
            macros: crate::sequencer::default_rack_macros(),
            runtime_macro_values: None,
            runtime_macro_track: 0,
        };
        self.finish_rack_track_registration(idx, track_name, shell, rack_slot_nodes, rack_track)?;
        self.app.sampler_paths.push(None);
        self.debug_assert_track_vectors_aligned();
        Ok(idx)
    }

    pub fn add_empty_rack_track(&mut self) -> Result<usize, String> {
        self.add_rack_track(
            "Drum Rack",
            RackRouting::ByPitch,
            Vec::<RackSlotBuildSpec<'_>>::new(),
        )
    }

    pub fn add_empty_layer_rack_track(&mut self) -> Result<usize, String> {
        self.add_rack_track(
            "Layer Rack",
            RackRouting::Broadcast,
            Vec::<RackSlotBuildSpec<'_>>::new(),
        )
    }

    pub fn group_track_to_instrument_rack(&mut self, track: usize) -> Result<(), String> {
        let instrument_type = self
            .app
            .graph
            .track_instrument_types
            .get(track)
            .copied()
            .ok_or_else(|| format!("Invalid track index {}", track + 1))?;
        if !matches!(
            instrument_type,
            InstrumentType::Sampler | InstrumentType::Custom
        ) {
            return Err("Only sampler and custom-instrument tracks can be grouped".to_string());
        }
        self.app.state.validate_group_flat_track_to_rack(track)?;
        let rack_locator = FxChainLocator::RackSlot { track, slot: 0 };
        if self
            .app
            .editor
            .effect_chain_leases
            .contains_host(rack_locator)
        {
            return Err("Rack slot effect-chain host is already in use".to_string());
        }
        let old_nodes = self.app.graph.track_node_ids[track].clone();
        let old_host = self.app.fx_chain_host(FxChainLocator::Track(track))?;
        let descriptors = self.app.graph.effect_descriptors[track].clone();
        let custom_effect_names = descriptors
            .iter()
            .enumerate()
            .map(|(slot_idx, descriptor)| {
                let active = self.app.state.pattern.effect_chains[track][slot_idx]
                    .node_id
                    .load(Ordering::Relaxed)
                    != 0;
                active.then(|| {
                    EffectDescriptor::builtin_insert_project_name(&descriptor.name)
                        .unwrap_or_else(|| descriptor.name.clone())
                })
            })
            .collect::<Vec<_>>();
        let track_name = self.app.tracks[track].clone();
        let instrument_run_mode = self.app.graph.track_instrument_run_modes[track];
        let engine_id = self.app.graph.track_engine_ids[track];
        if instrument_type == InstrumentType::Custom {
            let engine_id =
                engine_id.ok_or_else(|| "Custom track has no engine binding".to_string())?;
            self.validated_engine_route_ids_for_track(engine_id, track)?;
        }
        if !self.app.state.save_current_pattern_snapshot(
            self.app.tracks.len(),
            &self.app.graph.track_buffer_ids,
            &self.app.graph.track_sample_rates,
            &self.app.tracks,
            &self.app.graph.track_instrument_types,
        ) {
            return Err("Failed to save the active track pattern before grouping".to_string());
        }
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let mixer = self.create_rack_slot_mixer(
            &format!("{}_rack1", track_name),
            old_nodes.voice_sum_id,
            old_nodes.voice_sum_r_id,
            1.0,
            0.0,
            false,
            false,
        )?;

        let rack_nodes = match instrument_type {
            InstrumentType::Sampler => {
                let pool_id = rack_slot_pool_index(track, 0)
                    .ok_or_else(|| "Rack sampler pool unavailable".to_string())?;
                let buffer_id = self.app.graph.track_buffer_ids[track];
                let sample_rate = self.app.graph.track_sample_rates[track];
                let voices = self.build_sampler_voices(
                    pool_id,
                    &format!("{}_rack1", track_name),
                    buffer_id,
                    sample_rate,
                    mixer.slot_sum_l_id,
                    mixer.slot_sum_r_id,
                    old_nodes.mod_in_clip_ids,
                    old_nodes.sampler_ids.len().max(1),
                )?;
                self.publish_sampler_voice_runtime(
                    pool_id,
                    &voices.voice_lids,
                    &voices.sampler_ids,
                    &voices.gatepitch_ids,
                    &voices.modulator_ids,
                );
                self.delete_sampler_voice_nodes(&old_nodes);
                self.clear_sampler_runtime_pool(track);
                RackSlotNodeIds {
                    sampler_pool_id: Some(pool_id),
                    engine_id: None,
                    sampler_voice_lids: voices.voice_lids,
                    sampler_ids: voices.sampler_ids,
                    sampler_gatepitch_ids: voices.gatepitch_ids,
                    sampler_modulator_ids: voices.modulator_ids,
                    slot_sum_l_id: mixer.slot_sum_l_id,
                    slot_sum_r_id: mixer.slot_sum_r_id,
                    slot_pan_id: mixer.slot_pan_id,
                }
            }
            InstrumentType::Custom => {
                let engine_id = self.app.graph.track_engine_ids[track]
                    .ok_or_else(|| "Custom track has no engine binding".to_string())?;
                self.rewire_engine_route_output_for_track(
                    engine_id,
                    track,
                    old_nodes.voice_sum_id,
                    old_nodes.voice_sum_r_id,
                    mixer.slot_sum_l_id,
                    mixer.slot_sum_r_id,
                )?;
                let route_idx = rack_slot_pool_index(track, 0)
                    .ok_or_else(|| "Rack custom route unavailable".to_string())?;
                self.move_engine_route_to_rack_consumer(engine_id, track, route_idx)?;
                RackSlotNodeIds {
                    sampler_pool_id: None,
                    engine_id: Some(engine_id),
                    sampler_voice_lids: Vec::new(),
                    sampler_ids: Vec::new(),
                    sampler_gatepitch_ids: Vec::new(),
                    sampler_modulator_ids: Vec::new(),
                    slot_sum_l_id: mixer.slot_sum_l_id,
                    slot_sum_r_id: mixer.slot_sum_r_id,
                    slot_pan_id: mixer.slot_pan_id,
                }
            }
            _ => unreachable!(),
        };

        let rack = self
            .app
            .state
            .group_flat_track_to_rack(
                track,
                instrument_type,
                instrument_run_mode,
                engine_id,
                &descriptors,
                &custom_effect_names,
            )
            .ok_or_else(|| "Failed to move flat-track state into rack".to_string())?;
        self.app.graph.track_node_ids[track].rack_slots = vec![rack_nodes];
        self.app.graph.track_node_ids[track].rack_signature = Some(rack_topology_signature(&rack));
        self.app.graph.track_node_ids[track].sampler_ids.clear();
        self.app.graph.track_node_ids[track]
            .sampler_gatepitch_ids
            .clear();
        self.app.graph.track_node_ids[track]
            .sampler_modulator_ids
            .clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track] = InstrumentType::Rack;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = EffectDescriptor::empty_custom_slot();
        self.app.graph.effect_descriptors[track] = EffectDescriptor::default_full_chain();

        let new_host = self.app.fx_chain_host(rack_locator)?;
        rewire_fx_chain(self.app.graph.lg.0, &old_host, &new_host);
        connect_fx_chain_gap(
            self.app.graph.lg.0,
            StereoEndpoint {
                node_id: old_nodes.pan_id,
                channels: 2,
            },
            ChainSuccessor::StereoNode(StereoEndpoint {
                node_id: old_nodes.delay_id,
                channels: 2,
            }),
        );
        self.app
            .editor
            .effect_chain_leases
            .move_host(FxChainLocator::Track(track), rack_locator)?;
        self.app.tracks[track] = format!("Rack {track_name}");
        self.app.set_rack_selected_slot(track, 0);
        self.publish_rack_slot_panner_runtime(track);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
        self.app.push_all_restored_defaults();
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
        Ok(())
    }

    pub fn replace_track_instrument_container_with_rack(
        &mut self,
        track: usize,
        mut rack: RackTrackSnapshot,
        display_name: &str,
    ) -> Result<(), String> {
        let previous_type = self
            .app
            .graph
            .track_instrument_types
            .get(track)
            .copied()
            .ok_or_else(|| format!("Invalid track index {}", track + 1))?;
        let old_nodes = self.app.graph.track_node_ids[track].clone();
        let old_flat_engine = (previous_type == InstrumentType::Custom)
            .then(|| self.app.graph.track_engine_ids[track])
            .flatten();
        self.validate_rack_slot_graph_rebuild(track, &rack)?;

        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        if previous_type == InstrumentType::Rack {
            self.delete_rack_effect_chains(track, batch.serial)?;
        } else {
            match previous_type {
                InstrumentType::Sampler => {
                    self.delete_sampler_voice_nodes(&old_nodes);
                    self.clear_sampler_runtime_pool(track);
                }
                InstrumentType::Custom => {
                    if let Some(engine_id) = old_flat_engine {
                        self.delete_engine_route_for_track(engine_id, track, track);
                    }
                }
                _ => {
                    return Err("Only sampler, custom, or rack tracks can load a Sound".to_string());
                }
            }
        }

        if !self
            .app
            .state
            .replace_instrument_container_with_rack(track, rack.clone())
        {
            return Err("Failed to replace track rack state".to_string());
        }
        self.app.graph.track_instrument_types[track] = InstrumentType::Rack;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.track_node_ids[track].sampler_ids.clear();
        self.app.graph.track_node_ids[track]
            .sampler_gatepitch_ids
            .clear();
        self.app.graph.track_node_ids[track]
            .sampler_modulator_ids
            .clear();
        self.app.graph.track_voice_lids[track].clear();
        self.app.graph.track_buffer_ids[track] = -1;
        self.app.graph.track_sample_rates[track] = self.app.graph.sample_rate;
        self.app.graph.instrument_descriptors[track] = EffectDescriptor::empty_custom_slot();

        let bindings = self.rebuild_rack_slot_graph(track, &mut rack)?;
        if !self
            .app
            .state
            .sync_rack_slot_instrument_bindings_for_all_patterns(track, &bindings)
        {
            return Err("Failed to bind loaded Sound instruments".to_string());
        }
        if let Some(engine_id) = old_flat_engine {
            if !self.engine_is_still_referenced(engine_id) {
                self.delete_engine_runtime(engine_id);
            }
        }
        self.app.tracks[track] = display_name.to_string();
        self.app.set_rack_selected_slot(track, 0);
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.state.publish_scheduler_snapshot();
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
        Ok(())
    }

    pub fn replace_rack_track_with_sampler(
        &mut self,
        track: usize,
        buffer_id: i32,
        sample_rate: u32,
        sample_name: &str,
    ) -> Result<InstrumentSlotResetSummary, String> {
        if self.app.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err(format!("Track {} is not an instrument rack", track + 1));
        }
        if buffer_id < 0 {
            return Err("Retained sampler buffer is invalid".to_string());
        }
        let track_id = self.app.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.app.state.validate_sampler_slot_reset_target(track)?;
        let track_nodes = self.app.graph.track_node_ids.get(track).cloned()
            .ok_or_else(|| format!("Track {} has no graph nodes", track + 1))?;
        let (sampler_nodes, sampler_connections) = sampler_voice_build_capacities(MAX_VOICES);
        let required_commands = (sampler_nodes + sampler_connections)
            .checked_mul(2)
            .ok_or_else(|| "Rack-to-sampler graph capacity overflow".to_string())?;
        require_graph_edit_queue_capacity(
            self.app.graph.lg.0,
            required_commands,
            "Rack-to-sampler conversion",
        )?;

        let descriptor = EffectDescriptor::builtin_sampler();
        let old_engine_ids = self.rack_engine_ids_for_track(track);
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let voices = self.build_sampler_voices(
            track,
            sample_name,
            buffer_id,
            sample_rate,
            track_nodes.voice_sum_id,
            track_nodes.voice_sum_r_id,
            track_nodes.mod_in_clip_ids,
            MAX_VOICES,
        )?;
        self.delete_rack_effect_chains(track, batch.serial)?;
        self.retire_rack_slot_graph_generation(track);
        self.publish_sampler_voice_runtime(
            track,
            &voices.voice_lids,
            &voices.sampler_ids,
            &voices.gatepitch_ids,
            &voices.modulator_ids,
        );

        let node_id = first_graph_node_identity(&voices.sampler_ids);
        let modulator_node_id = first_graph_node_identity(&voices.modulator_ids);
        let live_nodes = &mut self.app.graph.track_node_ids[track];
        live_nodes.rack_slots.clear();
        live_nodes.rack_signature = None;
        live_nodes.sampler_ids = voices.sampler_ids;
        live_nodes.sampler_gatepitch_ids = voices.gatepitch_ids;
        live_nodes.sampler_modulator_ids = voices.modulator_ids;
        self.app.graph.track_voice_lids[track] = voices.voice_lids;
        self.app.graph.track_buffer_ids[track] = buffer_id;
        self.app.graph.track_sample_rates[track] = sample_rate.max(1);
        self.app.graph.track_instrument_types[track] = InstrumentType::Sampler;
        self.app.graph.track_instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.app.graph.track_engine_ids[track] = None;
        self.app.graph.track_synth_node_ids[track].clear();
        self.app.graph.track_gatepitch_node_ids[track].clear();
        self.app.graph.instrument_descriptors[track] = descriptor.clone();
        let reset_summary = self.app.state.reset_sampler_slot_all_patterns(
            track,
            &descriptor,
            node_id,
            modulator_node_id,
            (buffer_id, sample_name.to_string(), sample_rate.max(1)),
        ).expect("sampler reset target was validated before graph mutation");
        drop(batch);

        self.reap_excess_rack_teardowns();
        for engine_id in old_engine_ids {
            if !self.engine_is_still_referenced(engine_id) {
                lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
            }
        }
        self.app.tracks[track] = sample_name.to_string();
        self.app.reset_sampler_bpm_for_analysis(track);
        self.app.publish_sampler_analysis_runtime(track);
        self.app.device_registry.clear_rack_track(track_id);
        self.finish_track_instrument_source_change(track);
        Ok(reset_summary)
    }

    pub fn add_sampler_rack_track(
        &mut self,
        sample_paths: &[std::path::PathBuf],
    ) -> Result<usize, String> {
        if sample_paths.is_empty() {
            return Err("Rack track creation requires at least one sample".to_string());
        }
        if sample_paths.len() > MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }

        let mut loaded_slots = Vec::with_capacity(sample_paths.len());
        for path in sample_paths {
            let loaded = crate::instruments::sampler::load_wav_buffer(self.app.graph.lg.0, path)?;
            self.app.submit_sample_analysis(&loaded);
            let sample_name =
                crate::sample_db::display_title_for_sample_path(path).unwrap_or(loaded.name);
            loaded_slots.push((
                path.clone(),
                loaded.buffer_id,
                loaded.sample_rate,
                sample_name,
            ));
        }
        let track_name = if loaded_slots.len() == 1 {
            format!("Rack {}", loaded_slots[0].3)
        } else {
            "Layer Rack".to_string()
        };
        let per_slot_max_polyphony = appended_rack_slot_max_polyphony(&[]);
        let specs: Vec<RackSlotBuildSpec<'_>> = loaded_slots
            .iter()
            .map(
                |(_, buffer_id, sample_rate, sample_name)| RackSlotBuildSpec {
                    instrument: RackSlotInstrumentBuildSpec::Sampler(RackSamplerBuildSpec {
                        buffer_id: *buffer_id,
                        sample_rate: *sample_rate,
                        sample_name: sample_name.clone(),
                    }),
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: per_slot_max_polyphony,
                    param_plocks: None,
                    instrument_slot: None,
                    effect_slots: None,
                    effect_descriptors: None,
                    custom_effect_names: None,
                    track_sound_state: None,
                },
            )
            .collect();

        let idx = self.add_rack_track(&track_name, RackRouting::Broadcast, specs)?;
        for (path, buffer_id, _, sample_name) in loaded_slots {
            self.app
                .register_loaded_sample_path(&sample_name, buffer_id, path);
        }
        Ok(idx)
    }

    pub fn add_sampler_drum_rack_track(
        &mut self,
        wav_path: &Path,
        pad_note: i32,
    ) -> Result<usize, String> {
        if !validate_drum_rack_pad_note(pad_note) {
            return Err(format!("Unsupported drum rack pad note {pad_note}"));
        }
        let loaded = crate::instruments::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        self.app.submit_sample_analysis(&loaded);
        let sample_name =
            crate::sample_db::display_title_for_sample_path(wav_path).unwrap_or(loaded.name);
        let specs = vec![RackSlotBuildSpec {
            instrument: RackSlotInstrumentBuildSpec::Sampler(RackSamplerBuildSpec {
                buffer_id: loaded.buffer_id,
                sample_rate: loaded.sample_rate,
                sample_name: sample_name.clone(),
            }),
            instrument_base_note_offset: 0.0,
            pad_note: Some(pad_note),
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: DEFAULT_DRUM_SLOT_MAX_POLYPHONY,
            param_plocks: None,
            instrument_slot: None,
            effect_slots: None,
            effect_descriptors: None,
            custom_effect_names: None,
            track_sound_state: None,
        }];
        let idx = self.add_rack_track("Drum Rack", RackRouting::ByPitch, specs)?;
        self.app.register_loaded_sample_path(
            &sample_name,
            loaded.buffer_id,
            wav_path.to_path_buf(),
        );
        Ok(idx)
    }

}
