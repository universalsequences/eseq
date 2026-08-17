use super::*;

impl GraphController<'_> {
    pub fn apply_sample_ids(&mut self, sample_ids: &[(i32, String, u32)]) {
        for (track, (buffer_id, name, sample_rate)) in sample_ids.iter().enumerate() {
            if *buffer_id < 0 {
                continue;
            }
            if track >= self.app.tracks.len() {
                break;
            }
            if !self.app.is_sampler_track(track) {
                continue;
            }
            self.send_sample_to_all_voices(track, *buffer_id, *sample_rate);
            self.app.graph.track_buffer_ids[track] = *buffer_id;
            if let Some(track_sample_rate) = self.app.graph.track_sample_rates.get_mut(track) {
                *track_sample_rate = *sample_rate;
            }
            self.app.tracks[track] = name.clone();
            self.app
                .sync_sampler_path_from_sample(track, *buffer_id, name);
            self.app.publish_sampler_analysis_runtime(track);
        }
        if let Err(error) = self.sync_live_rack_tracks_from_pattern_state() {
            self.app.editor.status_message = Some((
                format!("Pattern rack sync failed: {error}"),
                std::time::Instant::now(),
            ));
        }
    }

    pub fn sync_live_rack_tracks_from_pattern_state(&mut self) -> Result<(), String> {
        let rack_tracks = self.app.state.pattern.rack_tracks.lock().unwrap().clone();
        let mut rebuilt_any = false;
        let mut applied_in_place_any = false;
        for (track_idx, rack) in rack_tracks.into_iter().enumerate() {
            if track_idx >= self.app.tracks.len() {
                break;
            }
            if self.app.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
                continue;
            }
            let Some(mut rack) = rack else {
                continue;
            };
            let incoming_signature = rack_topology_signature(&rack);
            let live_signature = self.app.graph.track_node_ids[track_idx]
                .rack_signature
                .clone();
            let in_place = live_signature.as_ref() == Some(&incoming_signature)
                && self
                    .validate_rack_slot_graph_rebuild(track_idx, &rack)
                    .is_ok();
            let bindings = if in_place {
                applied_in_place_any = true;
                self.apply_rack_scene_state_in_place(track_idx, &mut rack)?
            } else {
                rebuilt_any = true;
                self.rebuild_rack_slot_graph(track_idx, &mut rack)?
            };
            if std::env::var_os("TINYSEQ_LOG_RACK_SYNC").is_some() {
                eprintln!(
                    "rack sync track {track_idx}: {}",
                    if in_place { "in-place" } else { "rebuild" }
                );
            }
            if let Some(live_rack_track) = self
                .app
                .state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get_mut(track_idx)
            {
                *live_rack_track = Some(rack);
            }
            if !self
                .app
                .state
                .sync_rack_slot_instrument_bindings_for_all_patterns(track_idx, &bindings)
            {
                return Err(format!(
                    "Failed to sync rack bindings for track {}",
                    track_idx + 1
                ));
            }
        }
        if rebuilt_any {
            self.app.state.schedule_mod_resync();
            self.app.state.request_all_accumulator_resets();
            self.app.state.publish_scheduler_snapshot();
            self.app
                .state
                .transport
                .topology_epoch
                .fetch_add(1, Ordering::Relaxed);
        } else if applied_in_place_any {
            self.app.state.publish_scheduler_snapshot();
        }
        Ok(())
    }

    pub fn clear_all_tracks(&mut self) {
        self.force_reap_all_rack_teardowns();
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let old_track_count = self.app.tracks.len();

        for track_idx in 0..old_track_count {
            if let Err(error) = self.delete_rack_effect_chains(track_idx, batch.serial) {
                self.app.editor.status_message = Some((error, std::time::Instant::now()));
            }
            for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
                ..self.app.state.pattern.effect_chains[track_idx].len()
            {
                let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
                let node_id = slot.node_id.load(Ordering::Relaxed);
                let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
                if node_id == 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
                    if modulator_node_id != 0 {
                        crate::audiograph::delete_node(
                            self.app.graph.lg.0,
                            modulator_node_id as i32,
                        );
                    }
                }
            }
        }

        for engine in self.app.graph.engine_node_ids.iter_mut().flatten() {
            for routes in &engine.route_gain_ids {
                for route_pair in routes {
                    for &route_id in route_pair {
                        if route_id <= 0 {
                            continue;
                        }
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            for routes in &engine.ext_route_gain_ids {
                for route_ids in routes {
                    for &route_id in route_ids {
                        if route_id > 0 {
                            unsafe {
                                crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                            }
                        }
                    }
                }
            }
            for &node_id in &engine.synth_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
            for &node_id in &engine.modulator_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
            for &node_id in &engine.gatepitch_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                }
            }
        }

        for track in self.app.graph.track_node_ids.iter().rev() {
            for rack_slot in &track.rack_slots {
                self.delete_rack_slot_nodes(rack_slot);
            }
            for &sampler_id in &track.sampler_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
                }
            }
            for &modulator_id in &track.sampler_modulator_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
                }
            }
            for &gatepitch_id in &track.sampler_gatepitch_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, gatepitch_id);
                }
            }
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, track.send_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_env_id);
                for &mod_in_clip_id in &track.mod_in_clip_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, mod_in_clip_id);
                }
                crate::audiograph::delete_node(self.app.graph.lg.0, track.mod_out_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.delay_id);
                if track.filter_id != 0 {
                    crate::audiograph::delete_node(self.app.graph.lg.0, track.filter_id);
                }
                crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_r_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_id);
            }
        }

        self.app.tracks.clear();
        self.app.track_registry = crate::sequencer::TrackRegistry::default();
        self.app.track_colors.clear();
        self.app.track_collapsed.clear();
        self.app.sampler_paths.clear();
        self.app.rack_selected_slots.clear();
        self.app.rack_pad_bank_starts.clear();
        self.app.graph.track_node_ids.clear();
        self.app.graph.applied_mod_routes.clear();
        self.app.graph.track_buffer_ids.clear();
        self.app.graph.track_sample_rates.clear();
        self.app.graph.track_voice_lids.clear();
        self.app.graph.track_instrument_types.clear();
        self.app.graph.track_instrument_run_modes.clear();
        self.app.graph.track_engine_ids.clear();
        self.app.graph.track_synth_node_ids.clear();
        self.app.graph.track_gatepitch_node_ids.clear();
        self.app.graph.engine_node_ids.clear();
        self.app.graph.effect_descriptors.clear();
        self.app.graph.instrument_descriptors.clear();
        self.app.graph.record_armed.clear();
        self.clear_all_rack_sampler_runtime_pools();
        if let Err(error) = self
            .app
            .editor
            .effect_chain_leases
            .retire_tracks(batch.serial)
        {
            self.app.editor.status_message = Some((error, std::time::Instant::now()));
        }

        self.app.ui.cursor_track = 0;
        self.app.ui.cursor_step = 0;
        self.app.ui.pattern_page = 0;
        self.app.ui.focused_region = super::super::Region::Sidebar;
        self.app.ui.sidebar_tab = super::super::SidebarTab::Sounds;
        self.app.ui.sidebar_mode = super::super::SidebarMode::InstrumentPicker;
        self.app.ui.sidebar_search_focused = false;

        self.app.state.clear_live_track_state(old_track_count);
        self.app
            .state
            .transport
            .num_tracks
            .store(0, Ordering::Release);
        self.app.state.replace_pattern_repository(
            vec![crate::sequencer::PatternSnapshot::new_default(0, &[])],
            0,
        );
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
        self.app.state.publish_scheduler_snapshot();
    }

    pub fn clear_all_bus_effect_chains(&mut self) {
        let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let bus_nodes = self.app.graph.bus_node_ids.clone();

        for bus in &mut self.app.buses {
            let Some(nodes) = bus_nodes.iter().find(|nodes| nodes.id == bus.id) else {
                continue;
            };

            let active_effect_ids = bus
                .effect_slots
                .iter()
                .filter_map(|slot| (slot.node_id != 0).then_some(slot.node_id as i32))
                .collect::<Vec<_>>();
            let active_effect_modulator_ids = bus
                .effect_slots
                .iter()
                .filter_map(|slot| {
                    (slot.modulator_node_id != 0).then_some(slot.modulator_node_id as i32)
                })
                .collect::<Vec<_>>();

            unsafe {
                let mut predecessor_id = nodes.gate_id;
                for effect_id in &active_effect_ids {
                    disconnect_all_ports(self.app.graph.lg.0, predecessor_id, *effect_id);
                    predecessor_id = *effect_id;
                }
                disconnect_all_ports(self.app.graph.lg.0, predecessor_id, nodes.volume_id);
                disconnect_all_ports(self.app.graph.lg.0, nodes.gate_id, nodes.volume_id);

                for effect_id in active_effect_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, effect_id);
                    crate::effects::dgen_builtin::clear_instance(effect_id);
                }
                for modulator_id in active_effect_modulator_ids {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
                }

                connect_stereo_pair(self.app.graph.lg.0, nodes.gate_id, nodes.volume_id);
            }

            bus.effect_descriptors = super::super::BusChannelState::default_effect_descriptors();
            bus.effect_slots = super::super::BusChannelState::default_effect_slots();
            bus.custom_effect_names = vec![None; crate::lisp_host::MAX_CUSTOM_FX];
        }
        if let Err(error) = self
            .app
            .editor
            .effect_chain_leases
            .retire_buses(batch.serial)
        {
            self.app.editor.status_message = Some((error, std::time::Instant::now()));
        }

        self.app.publish_bus_gate_runtime();
    }

}
