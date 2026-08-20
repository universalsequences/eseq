use super::*;

impl GraphController<'_> {
    pub fn move_appended_track_to(&mut self, target: usize) -> Result<(), String> {
        let track_count = self.app.tracks.len();
        let last = track_count.checked_sub(1)
            .ok_or_else(|| "Cannot insert into an empty track topology".to_string())?;
        if target > last {
            return Err(format!("Track insertion index {} is out of range", target + 1));
        }
        if target == last {
            return Ok(());
        }
        let track_id = self.app.track_registry.id_at(last)
            .ok_or_else(|| "Appended track has no stable identity".to_string())?;
        self.move_engine_route_tables_from_end_to(target, track_count);

        fn move_last_to<T>(values: &mut Vec<T>, target: usize) {
            let value = values.pop().expect("aligned track vector must be non-empty");
            values.insert(target, value);
        }
        move_last_to(&mut self.app.tracks, target);
        move_last_to(&mut self.app.track_name_user_authored, target);
        move_last_to(&mut self.app.track_colors, target);
        move_last_to(&mut self.app.track_collapsed, target);
        move_last_to(&mut self.app.rack_selected_slots, target);
        move_last_to(&mut self.app.sampler_paths, target);
        move_last_to(&mut self.app.graph.track_node_ids, target);
        move_last_to(&mut self.app.graph.track_buffer_ids, target);
        move_last_to(&mut self.app.graph.track_sample_rates, target);
        move_last_to(&mut self.app.graph.track_voice_lids, target);
        move_last_to(&mut self.app.graph.track_instrument_types, target);
        move_last_to(&mut self.app.graph.track_instrument_run_modes, target);
        move_last_to(&mut self.app.graph.track_engine_ids, target);
        move_last_to(&mut self.app.graph.track_synth_node_ids, target);
        move_last_to(&mut self.app.graph.track_gatepitch_node_ids, target);
        move_last_to(&mut self.app.graph.effect_descriptors, target);
        move_last_to(&mut self.app.graph.instrument_descriptors, target);
        move_last_to(&mut self.app.graph.record_armed, target);
        self.app.track_registry.move_to(track_id, target)
            .map_err(|error| format!("Failed to insert stable track identity: {error:?}"))?;
        self.app.editor.effect_chain_leases.reindex_tracks_move_last_to(last, target);
        self.rebind_live_track_runtime_after_delete();
        self.rebind_all_track_graph_runtime();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();
        self.debug_assert_track_vectors_aligned();
        Ok(())
    }

    pub(super) fn move_engine_route_tables_from_end_to(&mut self, target: usize, track_count: usize) {
        let last = track_count - 1;
        for engine in self.app.graph.engine_node_ids.iter_mut().filter_map(Option::as_mut) {
            let route = std::mem::take(&mut engine.route_gain_ids[last]);
            let ext = std::mem::take(&mut engine.ext_route_gain_ids[last]);
            for track in (target + 1..=last).rev() {
                engine.route_gain_ids[track] = std::mem::take(&mut engine.route_gain_ids[track - 1]);
                engine.ext_route_gain_ids[track] =
                    std::mem::take(&mut engine.ext_route_gain_ids[track - 1]);
            }
            engine.route_gain_ids[target] = route;
            engine.ext_route_gain_ids[target] = ext;
            for slot in 0..MAX_RACK_SLOTS {
                let last_pool = rack_slot_pool_index(last, slot).expect("valid rack pool");
                let route = std::mem::take(&mut engine.route_gain_ids[last_pool]);
                let ext = std::mem::take(&mut engine.ext_route_gain_ids[last_pool]);
                for track in (target + 1..=last).rev() {
                    let dst = rack_slot_pool_index(track, slot).expect("valid rack pool");
                    let src = rack_slot_pool_index(track - 1, slot).expect("valid rack pool");
                    engine.route_gain_ids[dst] = std::mem::take(&mut engine.route_gain_ids[src]);
                    engine.ext_route_gain_ids[dst] =
                        std::mem::take(&mut engine.ext_route_gain_ids[src]);
                }
                let target_pool = rack_slot_pool_index(target, slot).expect("valid rack pool");
                engine.route_gain_ids[target_pool] = route;
                engine.ext_route_gain_ids[target_pool] = ext;
            }
        }
        for engine_id in 0..self.app.state.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                let left = self.app.state.runtime.engine_route_lids[engine_id][voice][last]
                    .load(Ordering::Acquire);
                let right = self.app.state.runtime.engine_route_lids_r[engine_id][voice][last]
                    .load(Ordering::Acquire);
                let ext: [u64; EXT_MOD_INPUT_COUNT] = std::array::from_fn(|input| {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][last][input]
                        .load(Ordering::Acquire)
                });
                for track in (target + 1..=last).rev() {
                    let source = track - 1;
                    self.app.state.runtime.engine_route_lids[engine_id][voice][track].store(
                        self.app.state.runtime.engine_route_lids[engine_id][voice][source]
                            .load(Ordering::Acquire), Ordering::Release);
                    self.app.state.runtime.engine_route_lids_r[engine_id][voice][track].store(
                        self.app.state.runtime.engine_route_lids_r[engine_id][voice][source]
                            .load(Ordering::Acquire), Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track][input]
                            .store(self.app.state.runtime.engine_ext_route_lids[engine_id][voice]
                                [source][input].load(Ordering::Acquire), Ordering::Release);
                    }
                }
                self.app.state.runtime.engine_route_lids[engine_id][voice][target]
                    .store(left, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][target]
                    .store(right, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][target][input]
                        .store(ext[input], Ordering::Release);
                }
            }
        }
        for slot in 0..MAX_RACK_SLOTS {
            let last_pool = rack_slot_pool_index(last, slot).expect("valid rack pool");
            let engine_id = self.app.state.runtime.rack_engine_route_engine_ids[last_pool]
                .load(Ordering::Acquire);
            let left: [u64; MAX_VOICES] = std::array::from_fn(|voice| {
                self.app.state.runtime.rack_engine_route_lids[last_pool][voice]
                    .load(Ordering::Acquire)
            });
            let right: [u64; MAX_VOICES] = std::array::from_fn(|voice| {
                self.app.state.runtime.rack_engine_route_lids_r[last_pool][voice]
                    .load(Ordering::Acquire)
            });
            let ext: [[u64; EXT_MOD_INPUT_COUNT]; MAX_VOICES] =
                std::array::from_fn(|voice| std::array::from_fn(|input| {
                    self.app.state.runtime.rack_engine_ext_route_lids[last_pool][voice][input]
                        .load(Ordering::Acquire)
                }));
            for track in (target + 1..=last).rev() {
                let dst = rack_slot_pool_index(track, slot).expect("valid rack pool");
                let src = rack_slot_pool_index(track - 1, slot).expect("valid rack pool");
                self.app.state.runtime.rack_engine_route_engine_ids[dst].store(
                    self.app.state.runtime.rack_engine_route_engine_ids[src]
                        .load(Ordering::Acquire), Ordering::Release);
                for voice in 0..MAX_VOICES {
                    self.app.state.runtime.rack_engine_route_lids[dst][voice].store(
                        self.app.state.runtime.rack_engine_route_lids[src][voice]
                            .load(Ordering::Acquire), Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[dst][voice].store(
                        self.app.state.runtime.rack_engine_route_lids_r[src][voice]
                            .load(Ordering::Acquire), Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.rack_engine_ext_route_lids[dst][voice][input].store(
                            self.app.state.runtime.rack_engine_ext_route_lids[src][voice][input]
                                .load(Ordering::Acquire), Ordering::Release);
                    }
                }
            }
            let target_pool = rack_slot_pool_index(target, slot).expect("valid rack pool");
            self.app.state.runtime.rack_engine_route_engine_ids[target_pool]
                .store(engine_id, Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.rack_engine_route_lids[target_pool][voice]
                    .store(left[voice], Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[target_pool][voice]
                    .store(right[voice], Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.rack_engine_ext_route_lids[target_pool][voice][input]
                        .store(ext[voice][input], Ordering::Release);
                }
            }
        }
        self.rebind_rack_sampler_runtime_pools();
    }

    pub(super) fn rebind_all_track_graph_runtime(&mut self) {
        for track in 0..self.app.tracks.len() {
            let nodes = &self.app.graph.track_node_ids[track];
            let voices = &self.app.graph.track_voice_lids[track];
            self.app.state.runtime.voice_counts[track]
                .store(voices.len() as u32, Ordering::Release);
            self.app.state.runtime.sampler_lids[track]
                .store(voices.first().copied().unwrap_or(0), Ordering::Release);
            self.app.state.runtime.pan_lids[track].store(nodes.pan_id as u64, Ordering::Release);
            self.app.state.runtime.delay_lids[track].store(nodes.delay_id as u64, Ordering::Release);
            self.app.state.runtime.send_lids[track].store(nodes.send_id as u64, Ordering::Release);
            self.app.state.runtime.modulator_lids[track].store(
                if self.app.graph.track_instrument_types[track] == InstrumentType::Modulator {
                    nodes.mod_env_id as u64
                } else { 0 },
                Ordering::Release,
            );
            self.app.state.runtime.instrument_type_flags[track].store(
                self.app.graph.track_instrument_types[track].runtime_flag(), Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.voice_lids[track][voice]
                    .store(voices.get(voice).copied().unwrap_or(0), Ordering::Release);
                self.app.state.runtime.synth_node_ids[track][voice].store(
                    nodes.sampler_ids.get(voice).copied().and_then(|id| u32::try_from(id).ok())
                        .unwrap_or(0), Ordering::Release);
                self.app.state.runtime.sampler_gatepitch_node_ids[track][voice].store(
                    nodes.sampler_gatepitch_ids.get(voice).copied()
                        .and_then(|id| u32::try_from(id).ok()).unwrap_or(0), Ordering::Release);
                self.app.state.runtime.sampler_modulator_node_ids[track][voice].store(
                    nodes.sampler_modulator_ids.get(voice).copied()
                        .and_then(|id| u32::try_from(id).ok()).unwrap_or(0), Ordering::Release);
            }
            self.app.publish_sampler_analysis_runtime(track);
        }
        self.rebind_rack_sampler_runtime_pools();
    }

}
