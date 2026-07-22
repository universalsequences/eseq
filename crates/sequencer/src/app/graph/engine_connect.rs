use super::*;

impl GraphController<'_> {
    pub(super) fn connect_engine_to_track(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
        track_name: &str,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_out_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        if engine_id >= self.app.state.runtime.engine_route_lids.len() {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has no audio-thread route table"
            ));
        }
        if route_idx >= crate::sequencer::MAX_SAMPLER_POOLS {
            return Err(format!(
                "connect_engine_to_track: route consumer {route_idx} exceeds the custom route limit"
            ));
        }
        let Some(existing_engine) = self.app.graph.engine_node_ids[engine_id].as_ref() else {
            return Err(format!(
                "connect_engine_to_track: missing engine runtime for engine {}",
                engine_id
            ));
        };
        if lisp_host::get_dgen_engine_enabled_voices(engine_id) == 0 {
            lisp_host::reset_dgen_engine_enabled_voices(engine_id);
        }
        let (Some(existing_routes), Some(existing_ext_routes)) = (
            existing_engine.route_gain_ids.get(route_idx),
            existing_engine.ext_route_gain_ids.get(route_idx),
        ) else {
            return Err(format!(
                "connect_engine_to_track: track {track_idx} is outside engine {engine_id}'s route table"
            ));
        };
        if existing_routes.len() == MAX_VOICES && existing_ext_routes.len() == MAX_VOICES {
            return Ok(());
        }
        if !existing_routes.is_empty() || !existing_ext_routes.is_empty() {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} track {track_idx} has incomplete route metadata"
            ));
        }
        let synth_ids = existing_engine.synth_ids.clone();
        let has_sibling_route =
            existing_engine
                .route_gain_ids
                .iter()
                .enumerate()
                .any(|(idx, routes)| {
                    idx != route_idx
                        && !routes.is_empty()
                        && custom_route_parent_track(idx) == Some(track_idx)
                });
        let audio_output_channels = existing_engine.audio_output_channels.clone();
        let primary_mod_output_channel = existing_engine.mod_output_channels.first().copied();
        let modulator_ids = existing_engine.modulator_ids.clone();
        if synth_ids.len() != MAX_VOICES {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has {} synth voices, expected {MAX_VOICES}",
                synth_ids.len()
            ));
        }
        if !modulator_ids.is_empty() && modulator_ids.len() != MAX_VOICES {
            return Err(format!(
                "connect_engine_to_track: engine {engine_id} has {} modulator voices, expected 0 or {MAX_VOICES}",
                modulator_ids.len()
            ));
        }

        let (route_node_capacity, route_connection_capacity) =
            engine_route_build_capacities(existing_engine);
        let mut transaction = GraphNodeBuildTransaction::new(
            self.app.graph.lg.0,
            route_node_capacity,
            route_connection_capacity,
        )?;

        let mut route_ids = Vec::with_capacity(MAX_VOICES);
        let mut ext_route_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let route_l_id = transaction.own(add_engine_route_gain_node_checked(
                self.app.graph.lg.0,
                0.0,
                &format!("{}_eng{}_route_{}_l", track_name, engine_id, v),
                &format!(
                    "connect_engine_to_track left route engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?)?;
            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 0) {
                transaction.connect(
                    synth_ids[v],
                    src_channel as i32,
                    route_l_id,
                    0,
                    &format!(
                        "connect_engine_to_track left engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
            }
            transaction.connect(
                route_l_id,
                0,
                voice_sum_id,
                0,
                &format!(
                    "connect_engine_to_track left engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?;

            let mut route_pair = [route_l_id, 0];

            if let Some(src_channel) = stereo_route_source_channel(&audio_output_channels, 1) {
                let route_r_id = transaction.own(add_engine_route_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?)?;
                transaction.connect(
                    synth_ids[v],
                    src_channel as i32,
                    route_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                transaction.connect(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                route_pair[1] = route_r_id;
            } else {
                let route_r_id = transaction.own(add_engine_route_gain_node_checked(
                    self.app.graph.lg.0,
                    0.0,
                    &format!("{}_eng{}_route_{}_r", track_name, engine_id, v),
                    &format!(
                        "connect_engine_to_track mirrored right route engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?)?;
                transaction.connect(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track mirrored-right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                route_pair[1] = route_r_id;
            }

            route_ids.push(route_pair);

            if !has_sibling_route {
                if let Some(src_channel) = primary_mod_output_channel {
                    transaction.connect(
                        synth_ids[v],
                        src_channel as i32,
                        track_mod_out_id,
                        0,
                        &format!(
                            "connect_engine_to_track mod output engine {} track {} voice {}",
                            engine_id, track_idx, v
                        ),
                    )?;
                }
            }

            let mut voice_ext_route_ids = [0; EXT_MOD_INPUT_COUNT];
            for input in 0..EXT_MOD_INPUT_COUNT {
                if !modulator_ids.is_empty() {
                    let ext_route_id = transaction.own(add_engine_route_gain_node_checked(
                        self.app.graph.lg.0,
                        0.0,
                        &format!(
                            "{}_eng{}_ext{}_route_{}",
                            track_name,
                            engine_id,
                            input + 1,
                            v
                        ),
                        &format!(
                            "connect_engine_to_track ext{} route engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?)?;
                    transaction.connect(
                        track_mod_in_clip_ids[input],
                        0,
                        ext_route_id,
                        0,
                        &format!(
                            "connect_engine_to_track ext{} input engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?;
                    transaction.connect(
                        ext_route_id,
                        0,
                        modulator_ids[v],
                        4 + input as i32,
                        &format!(
                            "connect_engine_to_track ext{} modulator engine {} track {} voice {}",
                            input + 1,
                            engine_id,
                            track_idx,
                            v
                        ),
                    )?;
                    voice_ext_route_ids[input] = ext_route_id;
                }
            }
            ext_route_ids.push(voice_ext_route_ids);
        }

        if self.app.graph.engine_node_ids[engine_id].is_none() {
            return Err(format!(
                "connect_engine_to_track: engine runtime disappeared for engine {}",
                engine_id
            ));
        }
        transaction.commit();
        let engine = self.app.graph.engine_node_ids[engine_id]
            .as_mut()
            .expect("engine runtime was validated immediately before transaction commit");
        engine.route_gain_ids[route_idx] = route_ids;
        engine.ext_route_gain_ids[route_idx] = ext_route_ids;
        for voice in 0..MAX_VOICES {
            let [route_l_id, route_r_id] = engine.route_gain_ids[route_idx][voice];
            if route_idx < MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][route_idx]
                    .store(route_l_id as u64, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][route_idx]
                    .store(route_r_id as u64, Ordering::Release);
            } else {
                self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                    .store(engine_id as u32, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(route_l_id as u64, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(route_r_id as u64, Ordering::Release);
            }
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext_route_id = engine.ext_route_gain_ids[route_idx][voice][input];
                if route_idx < MAX_TRACKS {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][route_idx]
                        [input]
                        .store(ext_route_id as u64, Ordering::Release);
                } else {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(ext_route_id as u64, Ordering::Release);
                }
            }
        }
        Ok(())
    }

    pub(super) fn silence_engine_routes(&self, engine_id: usize, engine: &EngineNodeIds) {
        for voice_idx in 0..MAX_VOICES {
            let lid = self.app.state.runtime.engine_voice_lids[engine_id][voice_idx]
                .load(Ordering::Acquire);
            if lid != 0 {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::effects::gatepitch::PARAM_GATE,
                            logical_id: lid,
                            fvalue: 0.0,
                        },
                    );
                }
            }
        }

        for route_pair in engine
            .route_gain_ids
            .iter()
            .flat_map(|routes| routes.iter())
        {
            for &route_id in route_pair {
                if route_id <= 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: 0,
                            logical_id: route_id as u64,
                            fvalue: 0.0,
                        },
                    );
                }
            }
        }
    }

    pub(super) fn rebuild_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        let host_routes = custom_host_input_routes(
            manifest,
            &format!("rebuild_custom_engine_runtime engine {engine_id}"),
        )?;
        let Some(mut engine) = self.app.graph.engine_node_ids[engine_id].take() else {
            return Err("Missing engine runtime".to_string());
        };
        self.silence_engine_routes(engine_id, &engine);
        lisp_host::reset_dgen_engine_enabled_voices(engine_id);

        let audio_output_channels = manifest_audio_output_channels(manifest);
        let mod_output_channels = manifest_mod_output_channels(manifest);
        let primary_mod_output_channel = mod_output_channels.first().copied();

        let mut new_synth_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let old_synth = engine.synth_ids[v];
            let gp_id = engine.gatepitch_ids[v];
            let mod_id = engine.modulator_ids[v];

            unsafe {
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 0, old_synth, 0);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 1, old_synth, 1);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 2, old_synth, 2);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 3, old_synth, 3);
                for mod_out in 0..crate::voice_modulator::NUM_OUTPUTS {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        mod_id,
                        mod_out as i32,
                        old_synth,
                        4 + mod_out as i32,
                    );
                }
                for route_pair in engine
                    .route_gain_ids
                    .iter()
                    .filter_map(|routes| routes.get(v))
                {
                    for (route_idx, &route_id) in route_pair.iter().enumerate() {
                        if route_id <= 0 {
                            continue;
                        }
                        if let Some(src_channel) =
                            stereo_route_source_channel(&engine.audio_output_channels, route_idx)
                        {
                            crate::audiograph::graph_disconnect(
                                self.app.graph.lg.0,
                                old_synth,
                                src_channel as i32,
                                route_id,
                                0,
                            );
                        }
                    }
                }
                for (input, ext_route_id) in engine
                    .ext_route_gain_ids
                    .iter()
                    .filter_map(|routes| routes.get(v))
                    .flat_map(|route_ids| route_ids.iter().enumerate())
                {
                    if *ext_route_id > 0 {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            *ext_route_id,
                            0,
                            mod_id,
                            4 + input as i32,
                        );
                    }
                }
                crate::audiograph::delete_node(self.app.graph.lg.0, old_synth);
            }

            let slot_id = engine_id * MAX_VOICES + v;
            lisp_host::set_dgen_instrument_fn(slot_id, lib.process_fn);
            lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
            let init_msg = lisp_host::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_host::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();
            let synth_name = CString::new(format!("engine_{}_synth_{}", engine_id, v)).unwrap();
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
                    "rebuild_custom_engine_runtime: failed to add synth node for engine {} voice {} (manifest.n_inputs={})",
                    engine_id, v, manifest.n_inputs
                ));
            }
            self.connect_custom_host_inputs(
                gp_id,
                mod_id,
                synth_id,
                &host_routes,
                &format!("rebuild_custom_engine_runtime engine {engine_id} voice {v}"),
            )?;
            for route_pair in engine
                .route_gain_ids
                .iter()
                .filter_map(|routes| routes.get(v))
            {
                for (route_idx, &route_id) in route_pair.iter().enumerate() {
                    if route_id <= 0 {
                        continue;
                    }
                    if let Some(src_channel) =
                        stereo_route_source_channel(&audio_output_channels, route_idx)
                    {
                        self.graph_connect_checked(
                            synth_id,
                            src_channel as i32,
                            route_id,
                            0,
                            &format!(
                                "rebuild_custom_engine_runtime engine {} voice {} route {}:{}",
                                engine_id, v, route_id, src_channel
                            ),
                        )?;
                    }
                }
            }
            if let Some(src_channel) = primary_mod_output_channel {
                for bound_track in 0..self.app.graph.track_engine_ids.len() {
                    if self.app.graph.track_engine_ids.get(bound_track) == Some(&Some(engine_id)) {
                        let Some(track_nodes) = self.app.graph.track_node_ids.get(bound_track)
                        else {
                            continue;
                        };
                        self.graph_connect_checked(
                            synth_id,
                            src_channel as i32,
                            track_nodes.mod_out_id,
                            0,
                            &format!(
                                "rebuild_custom_engine_runtime engine {} voice {} track {} mod output {}",
                                engine_id, v, bound_track, src_channel
                            ),
                        )?;
                    }
                }
            }
            for (input, ext_route_id) in engine
                .ext_route_gain_ids
                .iter()
                .filter_map(|routes| routes.get(v))
                .flat_map(|route_ids| route_ids.iter().enumerate())
            {
                if *ext_route_id <= 0 {
                    continue;
                }
                self.graph_connect_checked(
                    *ext_route_id,
                    0,
                    mod_id,
                    4 + input as i32,
                    &format!(
                        "rebuild_custom_engine_runtime engine {} voice {} ext{} route {}",
                        engine_id,
                        v,
                        input + 1,
                        ext_route_id
                    ),
                )?;
            }

            new_synth_ids.push(synth_id);
            self.app.state.runtime.engine_synth_node_ids[engine_id][v]
                .store(synth_id as u32, Ordering::Release);
        }

        engine.synth_ids = new_synth_ids;
        engine.synth_inputs = manifest.n_inputs;
        engine.synth_outputs = audio_output_channels.len();
        engine.audio_output_channels = audio_output_channels;
        engine.mod_output_channels = mod_output_channels;
        for (v, &mid) in engine.modulator_ids.iter().enumerate() {
            self.app.state.runtime.engine_modulator_node_ids[engine_id][v]
                .store(mid as u32, Ordering::Release);
        }
        for bound_track in 0..self.app.graph.track_engine_ids.len() {
            if self.app.graph.track_engine_ids[bound_track] == Some(engine_id) {
                self.app.graph.track_synth_node_ids[bound_track] = engine.synth_ids.clone();
                self.app.graph.track_gatepitch_node_ids[bound_track] = engine.gatepitch_ids.clone();
            }
        }
        self.silence_engine_routes(engine_id, &engine);
        self.app.graph.engine_node_ids[engine_id] = Some(engine);
        Ok(())
    }

}
