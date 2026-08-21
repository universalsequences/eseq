use super::*;

impl GraphController<'_> {
    pub(super) fn create_track_shell(&mut self, idx: usize, name: &str) -> Result<TrackShell, String> {
        if !self.app.track_registry.can_allocate() {
            return Err("Stable track id space is exhausted".to_string());
        }
        let voice_sum_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_sum_l", name),
            "create_track_shell left voice sum",
        )?;
        let voice_sum_r_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_sum_r", name),
            "create_track_shell right voice sum",
        )?;

        let pan_name = CString::new(format!("{}_pan", name)).unwrap();
        let pan_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                pan_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        let fx_out_name = CString::new(format!("{}_fx_out", name)).unwrap();
        let fx_out_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                fx_out_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };

        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, fx_out_id);
        }

        let send_id = add_gain_node_checked(
            self.app.graph.lg.0,
            0.0,
            &format!("{}_send", name),
            "create_track_shell send",
        )?;
        let mod_out_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{}_mod_out", name),
            "create_track_shell mod output",
        )?;
        let mod_in_clip_ids = std::array::from_fn(|input| {
            let mod_in_name = CString::new(format!("{}_mod_in{}_clip", name, input + 1)).unwrap();
            unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::instruments::track_modulator::mod_in_clip_vtable(),
                    crate::instruments::track_modulator::MOD_IN_CLIP_STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_in_name.as_ptr(),
                    1,
                    1,
                    std::ptr::null(),
                    0,
                )
            }
        });
        let mod_env_name = CString::new(format!("{}_mod_env", name)).unwrap();
        let mod_env_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::instruments::track_modulator::modulator_envelope_vtable(),
                crate::instruments::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE * std::mem::size_of::<f32>(),
                mod_env_name.as_ptr(),
                0,
                1,
                std::ptr::null(),
                0,
            )
        };

        let pdc_id = super::latency::add_pdc_node(self.app.graph.lg.0, &format!("{}_pdc", name));

        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_id, 0, pan_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_r_id, 0, pan_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 0, fx_out_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 1, fx_out_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, fx_out_id, 0, pdc_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, fx_out_id, 1, pdc_id, 1);
        }
        let output = self.app.state.pattern.track_params[idx].output();
        self.connect_delay_output_to(pdc_id, &output);

        Ok(TrackShell {
            voice_sum_id,
            voice_sum_r_id,
            pan_id,
            filter_id: 0,
            delay_id: fx_out_id,
            pdc_id,
            send_id,
            mod_out_id,
            mod_in_clip_ids,
            mod_env_id,
        })
    }

    pub(super) fn create_rack_slot_mixer(
        &mut self,
        slot_name: &str,
        track_voice_sum_id: i32,
        track_voice_sum_r_id: i32,
        gain: f32,
        pan: f32,
        mute: bool,
        muted_by_solo: bool,
    ) -> Result<RackSlotMixer, String> {
        let slot_sum_l_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{slot_name}_sum_l"),
            "create_rack_slot_mixer left sum",
        )?;
        let slot_sum_r_id = add_gain_node_checked(
            self.app.graph.lg.0,
            1.0,
            &format!("{slot_name}_sum_r"),
            "create_rack_slot_mixer right sum",
        )?;
        let pan_name = CString::new(format!("{slot_name}_pan")).unwrap();
        let slot_pan_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::effects::stereo_panner::stereo_panner_vtable(),
                crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
                pan_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        if slot_pan_id < 0 {
            return Err(format!(
                "create_rack_slot_mixer: failed to add panner for {slot_name}"
            ));
        }
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, slot_pan_id);
            crate::audiograph::graph_connect(self.app.graph.lg.0, slot_sum_l_id, 0, slot_pan_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, slot_sum_r_id, 0, slot_pan_id, 1);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                slot_pan_id,
                0,
                track_voice_sum_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                slot_pan_id,
                1,
                track_voice_sum_r_id,
                0,
            );
        }
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
            gain.max(0.0),
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
            pan.clamp(-1.0, 1.0),
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            if mute { 1.0 } else { 0.0 },
        );
        push_graph_param(
            self.app.graph.lg.0,
            slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            if muted_by_solo { 1.0 } else { 0.0 },
        );
        Ok(RackSlotMixer {
            slot_sum_l_id,
            slot_sum_r_id,
            slot_pan_id,
        })
    }

    pub(super) fn rack_slot_append_target(
        &self,
        track_idx: usize,
    ) -> Result<(RackTrackSnapshot, usize), String> {
        if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let rack = self
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
        let slot_idx = rack.slots.len();
        if slot_idx >= MAX_RACK_SLOTS {
            return Err(format!(
                "Rack tracks support at most {MAX_RACK_SLOTS} slots"
            ));
        }
        Ok((rack, slot_idx))
    }

    pub(in crate::app) fn refresh_rack_signature_from_live_state(&mut self, track_idx: usize) {
        let signature = self
            .app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track_idx)
            .and_then(Option::as_ref)
            .map(rack_topology_signature);
        if let Some(track_nodes) = self.app.graph.track_node_ids.get_mut(track_idx) {
            track_nodes.rack_signature = signature;
        }
    }

    pub(super) fn finish_rack_track_registration(
        &mut self,
        idx: usize,
        track_name: String,
        shell: TrackShell,
        rack_slots: Vec<RackSlotNodeIds>,
        rack_track: RackTrackSnapshot,
    ) -> Result<(), String> {
        self.app
            .track_registry
            .allocate()
            .map_err(|error| format!("Failed to allocate stable track id: {error:?}"))?;
        let rack_signature = rack_topology_signature(&rack_track);
        self.app.state.runtime.voice_counts[idx].store(0, Ordering::Release);
        self.app.state.runtime.sampler_lids[idx].store(0, Ordering::Release);
        self.app.state.runtime.modulator_lids[idx].store(0, Ordering::Release);
        self.app.state.runtime.pan_lids[idx].store(shell.pan_id as u64, Ordering::Release);
        self.app.state.runtime.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.runtime.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.runtime.instrument_type_flags[idx]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.app.state.pattern.instrument_run_modes[idx].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.app.state.runtime.instrument_run_mode_flags[idx].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
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
        self.app.graph.track_voice_lids.push(Vec::new());
        self.app
            .graph
            .track_instrument_types
            .push(InstrumentType::Rack);
        self.app
            .graph
            .track_instrument_run_modes
            .push(CustomInstrumentRunMode::Instrument);
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
            rack_slots,
            rack_signature: Some(rack_signature),
        });
        self.publish_rack_slot_panner_runtime(idx);
        self.app.graph.track_synth_node_ids.push(Vec::new());
        self.app.graph.track_gatepitch_node_ids.push(Vec::new());
        self.app.graph.track_engine_ids.push(None);
        self.app.state.pattern.instrument_slots[idx].clear();
        self.app
            .graph
            .instrument_descriptors
            .push(EffectDescriptor::empty_custom_slot());

        self.app.state.extend_all_pattern_snapshots_to_track(
            idx + 1,
            &self.app.graph.effect_descriptors,
            idx,
            CustomInstrumentRunMode::Instrument,
            None,
        )?;
        self.app
            .state
            .set_rack_track_for_all_pattern_snapshots(idx, rack_track);
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
        self.app.ui.sidebar_mode = super::super::SidebarMode::Audition;
        self.app.ui.sidebar_search_focused = false;
        self.app.state.schedule_mod_resync();
        self.app.state.request_all_accumulator_resets();
        self.app.latch_track_created_during_song_playback(idx);
        self.app.state.publish_event_compatible_topology();
        Ok(())
    }

    pub(super) fn build_sampler_voices(
        &mut self,
        sampler_pool_id: usize,
        track_name: &str,
        buffer_id: i32,
        sample_rate: u32,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
        voice_count: usize,
    ) -> Result<SamplerVoiceSetup, String> {
        if sampler_pool_id >= MAX_SAMPLER_POOLS {
            return Err(format!(
                "Sampler pool {sampler_pool_id} is unavailable; maximum sampler pools is {MAX_SAMPLER_POOLS}"
            ));
        }
        let voice_count = voice_count.clamp(1, MAX_VOICES);
        let mut sampler_ids = Vec::with_capacity(voice_count);
        let mut gatepitch_ids = Vec::with_capacity(voice_count);
        let mut modulator_ids = Vec::with_capacity(voice_count);
        let mut voice_lids = Vec::with_capacity(voice_count);
        let (node_capacity, connection_capacity) = sampler_voice_build_capacities(voice_count);
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            node_capacity,
            connection_capacity,
        )?;

        for v in 0..voice_count {
            let gp_name = CString::new(format!("{}_gp_{}", track_name, v)).unwrap();
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} gatepitch"
            ))?;
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::effects::gatepitch::gatepitch_vtable(),
                    crate::effects::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::effects::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!(
                    "build_sampler_voices: failed to add gatepitch node for voice {v}"
                ));
            }
            let gp_id = transaction.own(gp_id)?;

            let mod_name = CString::new(format!("{}_mod_{}", track_name, v)).unwrap();
            let mod_initial_state =
                crate::instruments::voice_modulator::sampler_voice_initial_state(sampler_pool_id, v);
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} modulator"
            ))?;
            let mod_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::instruments::voice_modulator::voice_modulator_vtable(),
                    crate::instruments::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_name.as_ptr(),
                    crate::instruments::voice_modulator::INPUT_COUNT as i32,
                    crate::instruments::voice_modulator::NUM_OUTPUTS as i32,
                    (&mod_initial_state
                        as *const crate::instruments::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::instruments::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            if mod_id < 0 {
                return Err(format!(
                    "build_sampler_voices: failed to add modulator node for voice {v}"
                ));
            }
            let mod_id = transaction.own(mod_id)?;
            let node_name = format!("{}_{}", track_name, v);
            check_test_graph_build_node_add(&format!(
                "build_sampler_voices pool {sampler_pool_id} voice {v} sampler"
            ))?;
            let st = crate::instruments::sampler::create_sampler_node(
                self.app.graph.lg.0,
                buffer_id,
                sample_rate,
                &node_name,
            )?;
            let sampler_id = transaction.own(st.node_id)?;
            for port in 0..4 {
                transaction.connect(
                    gp_id,
                    port,
                    mod_id,
                    port,
                    &format!("build_sampler_voices voice {v} gatepitch port {port}"),
                )?;
            }
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_PHASE as i32,
                mod_id,
                crate::instruments::voice_modulator::INPUT_TRANSPORT_BAR_PHASE as i32,
                &format!("build_sampler_voices voice {v} transport clock"),
            )?;
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_INC as i32,
                mod_id,
                crate::instruments::voice_modulator::INPUT_TRANSPORT_BAR_PHASE_INC as i32,
                &format!("build_sampler_voices voice {v} transport clock increment"),
            )?;
            for port in 0..crate::instruments::voice_modulator::NUM_OUTPUTS {
                transaction.connect(
                    mod_id,
                    port as i32,
                    sampler_id,
                    port as i32,
                    &format!("build_sampler_voices voice {v} modulator port {port}"),
                )?;
            }
            for (input, &clip_id) in track_mod_in_clip_ids.iter().enumerate() {
                transaction.connect(
                    clip_id,
                    0,
                    mod_id,
                    (4 + input) as i32,
                    &format!("build_sampler_voices voice {v} external mod input {input}"),
                )?;
            }
            transaction.connect(
                sampler_id,
                0,
                voice_sum_id,
                0,
                &format!("build_sampler_voices voice {v} left output"),
            )?;
            transaction.connect(
                sampler_id,
                1,
                voice_sum_r_id,
                0,
                &format!("build_sampler_voices voice {v} right output"),
            )?;
            sampler_ids.push(sampler_id);
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            voice_lids.push(st.logical_id);
        }

        transaction.commit();
        let bpm = self.app.state.transport.bpm.load(Ordering::Relaxed) as f32;
        for &mod_id in &modulator_ids {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.app.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::instruments::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: bpm,
                    },
                );
            }
        }

        Ok(SamplerVoiceSetup {
            sampler_ids,
            gatepitch_ids,
            modulator_ids,
            voice_lids,
        })
    }

    pub(super) fn publish_sampler_voice_runtime(
        &self,
        pool_id: usize,
        voice_lids: &[u64],
        sampler_ids: &[i32],
        gatepitch_ids: &[i32],
        modulator_ids: &[i32],
    ) {
        if pool_id >= self.app.state.runtime.voice_lids.len() {
            return;
        }
        self.clear_sampler_runtime_pool(pool_id);
        let voice_count = voice_lids
            .len()
            .min(sampler_ids.len())
            .min(gatepitch_ids.len())
            .min(modulator_ids.len())
            .min(MAX_VOICES);
        for v in 0..voice_count {
            self.app.state.runtime.voice_lids[pool_id][v].store(voice_lids[v], Ordering::Release);
            self.app.state.runtime.synth_node_ids[pool_id][v]
                .store(sampler_ids[v] as u32, Ordering::Release);
            self.app.state.runtime.sampler_gatepitch_node_ids[pool_id][v]
                .store(gatepitch_ids[v] as u32, Ordering::Release);
            self.app.state.runtime.sampler_modulator_node_ids[pool_id][v]
                .store(modulator_ids[v] as u32, Ordering::Release);
        }
        self.app.state.runtime.voice_counts[pool_id].store(voice_count as u32, Ordering::Release);
        self.app.state.runtime.sampler_lids[pool_id]
            .store(voice_lids.first().copied().unwrap_or(0), Ordering::Release);
    }

    pub(super) fn clear_sampler_runtime_pool(&self, pool_id: usize) {
        if pool_id >= self.app.state.runtime.voice_lids.len() {
            return;
        }
        self.app.state.runtime.voice_counts[pool_id].store(0, Ordering::Release);
        self.app.state.runtime.sampler_lids[pool_id].store(0, Ordering::Release);
        for v in 0..MAX_VOICES {
            self.app.state.runtime.voice_lids[pool_id][v].store(0, Ordering::Release);
            self.app.state.runtime.synth_node_ids[pool_id][v].store(0, Ordering::Release);
            self.app.state.runtime.sampler_gatepitch_node_ids[pool_id][v]
                .store(0, Ordering::Release);
            self.app.state.runtime.sampler_modulator_node_ids[pool_id][v]
                .store(0, Ordering::Release);
        }
    }

    pub(super) fn clear_all_rack_sampler_runtime_pools(&self) {
        for pool_id in MAX_TRACKS..self.app.state.runtime.voice_lids.len() {
            self.clear_sampler_runtime_pool(pool_id);
        }
    }

    pub(super) fn ensure_engine_slot(&mut self, engine_id: usize) {
        while self.app.graph.engine_node_ids.len() <= engine_id {
            self.app.graph.engine_node_ids.push(None);
        }
    }

    pub(super) fn graph_connect_checked(
        &self,
        src_node: i32,
        src_port: i32,
        dst_node: i32,
        dst_port: i32,
        context: &str,
    ) -> Result<(), String> {
        let ok = unsafe {
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                src_node,
                src_port,
                dst_node,
                dst_port,
            )
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{context}: graph_connect({}, {}, {}, {}) failed",
                src_node, src_port, dst_node, dst_port
            ))
        }
    }

    pub(super) fn connect_custom_host_inputs(
        &self,
        gp_id: i32,
        mod_id: i32,
        synth_id: i32,
        routes: &[CustomHostInputRoute],
        context: &str,
    ) -> Result<(), String> {
        for route in routes {
            let (source_node, source_port) = match route.source {
                CustomHostInputSource::GatePitch(port) => (gp_id, port),
                CustomHostInputSource::Modulator(port) => (mod_id, port),
            };
            self.graph_connect_checked(
                source_node,
                source_port,
                synth_id,
                route.input_channel,
                &format!("{context} host channel {}", route.input_channel),
            )?;
        }
        Ok(())
    }

    pub(super) fn ensure_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        name: &str,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        if engine_id >= self.app.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slot {engine_id} is unavailable; maximum runtime engines is {}",
                self.app.state.runtime.engine_voice_lids.len()
            ));
        }
        if self.app.graph.engine_node_ids[engine_id].is_some() {
            return Ok(());
        }

        let context = format!("ensure_custom_engine_runtime engine {engine_id}");
        let host_routes = custom_host_input_routes(manifest, &context)?;
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            MAX_VOICES * 3,
            MAX_VOICES * (host_routes.len() + 6),
        )?;
        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut modulator_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let voice_context = format!("{context} voice {v}");
            let gp_name = CString::new(format!("{}_gp_{}", name, v))
                .map_err(|_| format!("{voice_context}: gatepitch node name contains NUL"))?;
            check_test_graph_build_node_add(&format!("{voice_context} gatepitch"))?;
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::effects::gatepitch::gatepitch_vtable(),
                    crate::effects::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    crate::effects::gatepitch::OUTPUT_COUNT as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!("{voice_context}: failed to add gatepitch node"));
            }
            let gp_id = transaction.own(gp_id)?;

            let mod_name = CString::new(format!("{}_mod_{}", name, v))
                .map_err(|_| format!("{voice_context}: modulator node name contains NUL"))?;
            let mod_initial_state =
                crate::instruments::voice_modulator::custom_engine_initial_state(engine_id, v);
            check_test_graph_build_node_add(&format!("{voice_context} modulator"))?;
            let mod_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::instruments::voice_modulator::voice_modulator_vtable(),
                    crate::instruments::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_name.as_ptr(),
                    crate::instruments::voice_modulator::INPUT_COUNT as i32,
                    crate::instruments::voice_modulator::NUM_OUTPUTS as i32,
                    (&mod_initial_state
                        as *const crate::instruments::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::instruments::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            if mod_id < 0 {
                return Err(format!("{voice_context}: failed to add modulator node"));
            }
            let mod_id = transaction.own(mod_id)?;

            let slot_id = engine_id * MAX_VOICES + v;
            let init_msg = lisp_host::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_host::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name = CString::new(format!("{}_engine_synth_{}", name, v))
                .map_err(|_| format!("{voice_context}: synth node name contains NUL"))?;
            check_test_graph_build_node_add(&format!("{voice_context} synth"))?;
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_host::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    manifest.n_outputs.max(1) as i32,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };
            if synth_id < 0 {
                return Err(format!(
                    "{voice_context}: failed to add synth node (manifest.n_inputs={})",
                    manifest.n_inputs
                ));
            }
            let synth_id = transaction.own(synth_id)?;
            for route in &host_routes {
                let (source_node, source_port) = match route.source {
                    CustomHostInputSource::GatePitch(port) => (gp_id, port),
                    CustomHostInputSource::Modulator(port) => (mod_id, port),
                };
                transaction.connect(
                    source_node,
                    source_port,
                    synth_id,
                    route.input_channel,
                    &format!("{voice_context} host channel {}", route.input_channel),
                )?;
            }
            for port in 0..4 {
                transaction.connect(gp_id, port, mod_id, port, &voice_context)?;
            }
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_PHASE as i32,
                mod_id,
                crate::instruments::voice_modulator::INPUT_TRANSPORT_BAR_PHASE as i32,
                &format!("{voice_context} transport clock"),
            )?;
            transaction.connect(
                gp_id,
                crate::effects::gatepitch::PARAM_CLOCK_INC as i32,
                mod_id,
                crate::instruments::voice_modulator::INPUT_TRANSPORT_BAR_PHASE_INC as i32,
                &format!("{voice_context} transport clock increment"),
            )?;
            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            synth_ids.push(synth_id);
            voice_lids.push(gp_id as u64);
        }

        // The process registry must be ready before the batch can reach the
        // audio thread. No fallible operations remain after this publication.
        for v in 0..MAX_VOICES {
            let slot_id = engine_id * MAX_VOICES + v;
            lisp_host::set_dgen_instrument_fn(slot_id, lib.process_fn);
            lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
        }
        transaction.commit();

        let bpm = self.app.state.transport.bpm.load(Ordering::Relaxed) as f32;
        for &mod_id in &modulator_ids {
            push_graph_param(
                self.app.graph.lg.0,
                mod_id as u64,
                crate::instruments::voice_modulator::PARAM_BPM as u64,
                bpm,
            );
        }
        let audio_output_channels = manifest_audio_output_channels(manifest);
        let mod_output_channels = manifest_mod_output_channels(manifest);
        self.app.graph.engine_node_ids[engine_id] = Some(EngineNodeIds {
            synth_ids,
            synth_inputs: manifest.n_inputs,
            synth_outputs: audio_output_channels.len(),
            audio_output_channels,
            mod_output_channels,
            gatepitch_ids,
            modulator_ids,
            route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
            ext_route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
        });

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.engine_voice_lids[engine_id][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.engine_voice_counts[engine_id]
            .store(MAX_VOICES as u32, Ordering::Release);
        lisp_host::reset_dgen_engine_enabled_voices(engine_id);
        if let Some(engine) = &self.app.graph.engine_node_ids[engine_id] {
            for (v, &sid) in engine.synth_ids.iter().enumerate() {
                self.app.state.runtime.engine_synth_node_ids[engine_id][v]
                    .store(sid as u32, Ordering::Release);
            }
            for (v, &mid) in engine.modulator_ids.iter().enumerate() {
                self.app.state.runtime.engine_modulator_node_ids[engine_id][v]
                    .store(mid as u32, Ordering::Release);
            }
        }
        Ok(())
    }

}
