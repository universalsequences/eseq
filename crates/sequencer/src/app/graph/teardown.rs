use super::*;

impl GraphController<'_> {
    pub fn delete_track(&mut self, track_idx: usize) -> Result<usize, String> {
        if delete_without_shift_enabled() {
            return self.clear_track_in_place(track_idx);
        }
        let old_count = self.app.tracks.len();
        if old_count <= 1 {
            return Err("Cannot delete the last remaining track".to_string());
        }
        if track_idx >= old_count {
            return Err("Invalid track index".to_string());
        }
        self.force_reap_all_rack_teardowns();

        let names = self.app.tracks.clone();
        let buffer_ids = self.app.graph.track_buffer_ids.clone();
        let sample_rates = self.app.graph.track_sample_rates.clone();
        let instrument_types = self.app.graph.track_instrument_types.clone();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];
        let deleted_rack_engine_ids = self.rack_engine_ids_for_track(track_idx);

        let retire_after;
        {
            let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            retire_after = batch.serial;
            self.delete_custom_effect_chain(track_idx)?;
            self.delete_rack_effect_chains(track_idx, retire_after)?;
            self.delete_track_engine_routes(track_idx);

            let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
            self.delete_track_shell(&track_nodes);

            if let Some(engine_id) = deleted_engine_id {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
            for engine_id in deleted_rack_engine_ids {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }

            self.shift_engine_route_tables_left(track_idx, old_count);
        }

        if !self.app.state.remove_track(
            track_idx,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
            &self.app.graph.effect_descriptors,
        ) {
            return Err("Failed to compact sequencer state for deleted track".to_string());
        }

        self.compact_app_track_vectors(track_idx, retire_after)?;
        self.rebind_live_track_runtime_after_delete();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();

        let new_selected = track_idx.min(self.app.tracks.len().saturating_sub(1));
        self.app.ui.cursor_track = new_selected;
        self.app.ui.cursor_step = self.app.ui.cursor_step.min(
            self.app.state.pattern.track_params[new_selected]
                .get_num_steps()
                .saturating_sub(1),
        );

        Ok(new_selected)
    }

    pub fn clear_track_in_place(&mut self, track_idx: usize) -> Result<usize, String> {
        if track_idx >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        self.force_reap_all_rack_teardowns();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];
        let deleted_rack_engine_ids = self.rack_engine_ids_for_track(track_idx);

        if self.app.is_sampler_track(track_idx) {
            self.send_sample_to_all_voices(track_idx, -1, self.app.graph.sample_rate);
        }

        for engine_id in 0..self.app.state.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                let lid_l = self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                    .load(Ordering::Relaxed);
                let lid_r = self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                    .load(Ordering::Relaxed);
                unsafe {
                    if lid_l != 0 {
                        crate::audiograph::params_push_wrapper(
                            self.app.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                idx: 0,
                                logical_id: lid_l,
                                fvalue: 0.0,
                            },
                        );
                    }
                    if lid_r != 0 {
                        crate::audiograph::params_push_wrapper(
                            self.app.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                idx: 0,
                                logical_id: lid_r,
                                fvalue: 0.0,
                            },
                        );
                    }
                }
            }
        }

        {
            let batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.delete_custom_effect_chain(track_idx)?;
            self.delete_rack_effect_chains(track_idx, batch.serial)?;
            self.delete_track_engine_routes(track_idx);
            if let Some(track_nodes) = self.app.graph.track_node_ids.get(track_idx).cloned() {
                for (slot_idx, rack_slot) in track_nodes.rack_slots.iter().enumerate() {
                    if let (Some(engine_id), Some(route_idx)) = (
                        rack_slot.engine_id,
                        rack_slot_pool_index(track_idx, slot_idx),
                    ) {
                        self.delete_engine_route_for_track(engine_id, route_idx, track_idx);
                    }
                    self.delete_rack_slot_nodes(rack_slot);
                }
            }
            if let Some(engine_id) = deleted_engine_id {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
            for engine_id in deleted_rack_engine_ids {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }
        }

        if !self
            .app
            .state
            .clear_track_in_place(track_idx, &self.app.graph.effect_descriptors)
        {
            return Err("Failed to clear track in place".to_string());
        }

        self.app.tracks[track_idx] = format!("Empty {}", track_idx + 1);
        self.app.sampler_paths[track_idx] = None;
        self.app.graph.track_buffer_ids[track_idx] = -1;
        self.app.graph.track_sample_rates[track_idx] = self.app.graph.sample_rate;
        self.app.graph.track_instrument_types[track_idx] = InstrumentType::Sampler;
        self.set_track_instrument_run_mode(track_idx, CustomInstrumentRunMode::Instrument)?;
        self.app.graph.track_engine_ids[track_idx] = None;
        if let Some(nodes) = self.app.graph.track_node_ids.get_mut(track_idx) {
            nodes.rack_slots.clear();
            nodes.rack_signature = None;
        }
        self.app.set_rack_selected_slot(track_idx, 0);
        self.app.graph.track_synth_node_ids[track_idx].clear();
        self.app.graph.track_gatepitch_node_ids[track_idx].clear();
        self.app.graph.effect_descriptors[track_idx] = EffectDescriptor::default_full_chain();
        self.app.graph.instrument_descriptors[track_idx] = EffectDescriptor::empty_custom_slot();
        self.app.graph.record_armed[track_idx] = false;
        self.rebind_live_track_runtime_after_delete();
        self.app.push_all_restored_defaults();
        self.app.ui.cursor_track = track_idx;
        self.app.ui.cursor_step = 0;

        Ok(track_idx)
    }

    pub fn send_sample_to_all_voices(&self, track: usize, buffer_id: i32, sample_rate: u32) {
        if track < self.app.graph.track_voice_lids.len() {
            for &lid in &self.app.graph.track_voice_lids[track] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::sampler::PARAM_BUFFER_ID,
                            logical_id: lid,
                            fvalue: buffer_id as f32,
                        },
                    );
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::sampler::PARAM_SOURCE_SAMPLE_RATE,
                            logical_id: lid,
                            fvalue: sample_rate.max(1) as f32,
                        },
                    );
                }
            }
        }
    }

    pub fn delete_custom_effect_slot(
        &mut self,
        track_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        if track_idx >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if slot_idx < crate::effects::BUILTIN_SLOT_COUNT {
            return Err("This effect slot cannot be deleted".to_string());
        }
        let chain_len = self.app.state.pattern.effect_chains[track_idx].len();
        if slot_idx >= chain_len {
            return Err("Invalid effect slot".to_string());
        }

        let node_id = self.app.state.pattern.effect_chains[track_idx][slot_idx]
            .node_id
            .load(Ordering::Relaxed);
        if node_id == 0 {
            return Err("Effect slot is empty".to_string());
        }
        self.app.remove_fx_slot_node(
            FxChainLocator::Track(track_idx),
            slot_idx,
            FxLeaseSlotRemoval::Shift,
        )?;

        for idx in slot_idx..chain_len.saturating_sub(1) {
            let next_idx = idx + 1;
            let next_desc = self.app.graph.effect_descriptors[track_idx][next_idx].clone();
            self.app.graph.effect_descriptors[track_idx][idx] = next_desc;
            let next_slot = &self.app.state.pattern.effect_chains[track_idx][next_idx];
            self.app.state.pattern.effect_chains[track_idx][idx].copy_from(next_slot);
        }

        if let Some(last_desc) = self.app.graph.effect_descriptors[track_idx].last_mut() {
            *last_desc = EffectDescriptor::empty_custom_slot();
        }
        if let Some(last_slot) = self.app.state.pattern.effect_chains[track_idx].last() {
            last_slot.clear();
        }
        self.app
            .state
            .remove_effect_slot_from_track_patterns(track_idx, slot_idx);

        self.app.state.publish_scheduler_snapshot();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();
        Ok(())
    }

    pub(super) fn delete_custom_effect_chain(&mut self, track_idx: usize) -> Result<(), String> {
        let retire_after =
            unsafe { crate::audiograph::graph_edit_current_batch_serial(self.app.graph.lg.0) };
        if retire_after == 0 {
            return Err("Track FX chain deletion requires an edit batch".to_string());
        }
        let host = self
            .app
            .fx_chain_host(FxChainLocator::Track(track_idx))
            .ok();
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
                crate::effects::conv_reverb::clear_instance(node_id as i32);
                if modulator_node_id != 0 {
                    crate::audiograph::delete_node(self.app.graph.lg.0, modulator_node_id as i32);
                }
            }
            self.app
                .set_track_effect_lease(track_idx, slot_idx, None, retire_after)?;
        }
        if let Some(host) = host {
            connect_fx_chain_gap(self.app.graph.lg.0, host.predecessor, host.successor);
        }
        Ok(())
    }

    pub(super) fn delete_rack_effect_chains(
        &mut self,
        track_idx: usize,
        retire_after: u64,
    ) -> Result<(), String> {
        if retire_after == 0 {
            return Err("Rack FX chain deletion requires an edit batch".to_string());
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
            .flatten();
        let Some(rack) = rack else {
            return Ok(());
        };
        for (rack_slot, slot) in rack.slots.iter().enumerate() {
            for effect in &slot.effect_slots {
                if effect.node_id == 0 {
                    continue;
                }
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, effect.node_id as i32);
                    if effect.modulator_node_id != 0 {
                        crate::audiograph::delete_node(
                            self.app.graph.lg.0,
                            effect.modulator_node_id as i32,
                        );
                    }
                }
                crate::effects::conv_reverb::clear_instance(effect.node_id as i32);
            }
            self.app.editor.effect_chain_leases.retire_host(
                FxChainLocator::RackSlot {
                    track: track_idx,
                    slot: rack_slot,
                },
                retire_after,
            )?;
        }
        Ok(())
    }

    pub(super) fn delete_track_engine_routes(&mut self, track_idx: usize) {
        let engine_ids = self
            .app
            .graph
            .engine_node_ids
            .iter()
            .enumerate()
            .filter_map(|(engine_id, engine)| {
                let engine = engine.as_ref()?;
                let has_audio_routes = engine
                    .route_gain_ids
                    .get(track_idx)
                    .is_some_and(|routes| !routes.is_empty());
                let has_ext_routes = engine
                    .ext_route_gain_ids
                    .get(track_idx)
                    .is_some_and(|routes| !routes.is_empty());
                (has_audio_routes || has_ext_routes).then_some(engine_id)
            })
            .collect::<Vec<_>>();
        for engine_id in engine_ids {
            self.delete_engine_route_for_track(engine_id, track_idx, track_idx);
        }
    }

    pub(super) fn delete_track_shell(&mut self, track: &TrackNodeIds) {
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

    pub(super) fn delete_rack_slot_nodes(&self, slot: &RackSlotNodeIds) {
        for &sampler_id in &slot.sampler_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
            }
        }
        for &modulator_id in &slot.sampler_modulator_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, modulator_id);
            }
        }
        for &gatepitch_id in &slot.sampler_gatepitch_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, gatepitch_id);
            }
        }
        unsafe {
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, slot.slot_pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_sum_r_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, slot.slot_sum_l_id);
        }
    }

    /// Stops an outgoing custom-instrument generation from carrying future
    /// audio produced by a shared engine. The slot panner smooths mute changes,
    /// so this closes the generation without a discontinuity while downstream
    /// rack FX remain available to render their own tails until reap.
    pub(super) fn retire_custom_rack_slot_output(&self, slot: &RackSlotNodeIds) {
        push_graph_param(
            self.app.graph.lg.0,
            slot.slot_pan_id as u64,
            crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            1.0,
        );
    }

    pub(super) fn rack_engine_ids_for_track(&self, track_idx: usize) -> Vec<usize> {
        let Some(nodes) = self.app.graph.track_node_ids.get(track_idx) else {
            return Vec::new();
        };
        let mut engine_ids = Vec::new();
        for engine_id in nodes.rack_slots.iter().filter_map(|slot| slot.engine_id) {
            if !engine_ids.contains(&engine_id) {
                engine_ids.push(engine_id);
            }
        }
        engine_ids
    }

    pub(super) fn engine_is_still_referenced(&self, engine_id: usize) -> bool {
        self.engine_is_still_referenced_excluding(engine_id, usize::MAX)
    }

    pub(super) fn engine_is_still_referenced_excluding(&self, engine_id: usize, removed_track: usize) -> bool {
        self.app
            .graph
            .track_engine_ids
            .iter()
            .enumerate()
            .any(|(track_idx, binding)| track_idx != removed_track && *binding == Some(engine_id))
            || self
                .app
                .graph
                .track_node_ids
                .iter()
                .enumerate()
                .any(|(track_idx, nodes)| {
                    track_idx != removed_track
                        && nodes
                            .rack_slots
                            .iter()
                            .any(|slot| slot.engine_id == Some(engine_id))
                })
    }

    pub(super) fn delete_engine_route_for_track(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
    ) {
        let track_mod_out_id = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .map(|nodes| nodes.mod_out_id);
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(|engine| engine.as_mut())
        else {
            return;
        };
        let has_sibling_route = engine
            .route_gain_ids
            .iter()
            .enumerate()
            .any(|(idx, routes)| {
                idx != route_idx
                    && !routes.is_empty()
                    && custom_route_parent_track(idx) == Some(track_idx)
            });
        if !has_sibling_route {
            if let (Some(track_mod_out_id), Some(mod_output_channel)) = (
                track_mod_out_id,
                engine.mod_output_channels.first().copied(),
            ) {
                for &synth_id in &engine.synth_ids {
                    unsafe {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            synth_id,
                            mod_output_channel as i32,
                            track_mod_out_id,
                            0,
                        );
                    }
                }
            }
        }
        if route_idx < engine.route_gain_ids.len() {
            for route_pair in &engine.route_gain_ids[route_idx] {
                for &route_id in route_pair {
                    if route_id > 0 {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            engine.route_gain_ids[route_idx].clear();
        }
        if route_idx < engine.ext_route_gain_ids.len() {
            for route_ids in &engine.ext_route_gain_ids[route_idx] {
                for &route_id in route_ids {
                    if route_id > 0 {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                        }
                    }
                }
            }
            engine.ext_route_gain_ids[route_idx].clear();
        }
        for voice in 0..MAX_VOICES {
            if route_idx < MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][route_idx]
                    .store(0, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][route_idx]
                    .store(0, Ordering::Release);
            } else {
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(0, Ordering::Release);
            }
            for input in 0..EXT_MOD_INPUT_COUNT {
                if route_idx < MAX_TRACKS {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][route_idx]
                        [input]
                        .store(0, Ordering::Release);
                } else {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(0, Ordering::Release);
                }
            }
        }
        if route_idx >= MAX_TRACKS {
            self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .store(u32::MAX, Ordering::Release);
        }
    }

    /// Removes one engine route generation from the live route tables without
    /// deleting its graph nodes. The returned concrete node ids remain valid
    /// until the deferred rack teardown reaps that generation.
    pub(super) fn detach_engine_route_generation(
        &mut self,
        engine_id: usize,
        route_idx: usize,
        track_idx: usize,
    ) -> Option<DeferredEngineRouteGeneration> {
        let track_mod_out_id = self
            .app
            .graph
            .track_node_ids
            .get(track_idx)
            .map(|nodes| nodes.mod_out_id)?;
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::as_mut)?;

        let has_sibling_route = engine
            .route_gain_ids
            .iter()
            .enumerate()
            .any(|(idx, routes)| {
                idx != route_idx
                    && !routes.is_empty()
                    && custom_route_parent_track(idx) == Some(track_idx)
            });
        if !has_sibling_route {
            if let Some(mod_output_channel) = engine.mod_output_channels.first().copied() {
                for &synth_id in &engine.synth_ids {
                    unsafe {
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            synth_id,
                            mod_output_channel as i32,
                            track_mod_out_id,
                            0,
                        );
                    }
                }
            }
        }

        let route_ids = engine
            .route_gain_ids
            .get_mut(route_idx)
            .map(std::mem::take)
            .unwrap_or_default();
        let ext_route_ids = engine
            .ext_route_gain_ids
            .get_mut(route_idx)
            .map(std::mem::take)
            .unwrap_or_default();
        let mut node_ids = route_ids
            .into_iter()
            .flatten()
            .filter(|node_id| *node_id > 0)
            .collect::<Vec<_>>();
        node_ids.extend(
            ext_route_ids
                .into_iter()
                .flatten()
                .filter(|node_id| *node_id > 0),
        );

        for voice in 0..MAX_VOICES {
            self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                .store(0, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                .store(0, Ordering::Release);
            for input in 0..EXT_MOD_INPUT_COUNT {
                self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                    .store(0, Ordering::Release);
            }
        }
        self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
            .store(u32::MAX, Ordering::Release);

        (!node_ids.is_empty()).then_some(DeferredEngineRouteGeneration {
            engine_id,
            node_ids,
        })
    }

    pub(super) fn enqueue_deferred_rack_teardown(&mut self, teardown: DeferredRackTeardown) {
        self.app.graph.deferred_rack_teardowns.push(teardown);
    }

    pub(super) fn engine_has_deferred_route_generation(&self, engine_id: usize) -> bool {
        self.app
            .graph
            .deferred_rack_teardowns
            .iter()
            .any(|teardown| {
                teardown
                    .engine_routes
                    .iter()
                    .any(|route| route.engine_id == engine_id)
            })
    }

    pub(super) fn reap_rack_teardowns(&mut self, teardowns: Vec<DeferredRackTeardown>) {
        if teardowns.is_empty() {
            return;
        }
        let mut reaped_engine_ids = Vec::new();
        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            for teardown in &teardowns {
                for route in &teardown.engine_routes {
                    for &node_id in &route.node_ids {
                        unsafe {
                            crate::audiograph::delete_node(self.app.graph.lg.0, node_id);
                        }
                    }
                    if !reaped_engine_ids.contains(&route.engine_id) {
                        reaped_engine_ids.push(route.engine_id);
                    }
                }
                for slot in &teardown.slots {
                    self.delete_rack_slot_nodes(slot);
                }
                if std::env::var_os("TINYSEQ_LOG_RACK_SYNC").is_some() {
                    eprintln!("rack sync track {}: reaped", teardown.track_idx);
                }
            }
        }
        for engine_id in reaped_engine_ids {
            if !self.engine_is_still_referenced(engine_id)
                && !self.engine_has_deferred_route_generation(engine_id)
            {
                self.delete_engine_runtime(engine_id);
            }
        }
    }

    pub(super) fn reap_excess_rack_teardowns(&mut self) {
        let excess = self
            .app
            .graph
            .deferred_rack_teardowns
            .len()
            .saturating_sub(MAX_DEFERRED_RACK_TEARDOWNS);
        if excess == 0 {
            return;
        }
        let oldest = self
            .app
            .graph
            .deferred_rack_teardowns
            .drain(..excess)
            .collect();
        self.reap_rack_teardowns(oldest);
    }

    pub fn reap_due_rack_teardowns(&mut self) {
        if self.app.graph.deferred_rack_teardowns.is_empty() {
            return;
        }
        let now = Instant::now();
        let pending = std::mem::take(&mut self.app.graph.deferred_rack_teardowns);
        let (due, waiting): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|teardown| teardown.due_at <= now);
        self.app.graph.deferred_rack_teardowns = waiting;
        self.reap_rack_teardowns(due);
    }

    pub fn force_reap_all_rack_teardowns(&mut self) {
        let teardowns = std::mem::take(&mut self.app.graph.deferred_rack_teardowns);
        self.reap_rack_teardowns(teardowns);
    }

    pub(super) fn rewire_engine_route_output_for_track(
        &self,
        engine_id: usize,
        track_idx: usize,
        old_sum_l_id: i32,
        old_sum_r_id: i32,
        new_sum_l_id: i32,
        new_sum_r_id: i32,
    ) -> Result<(), String> {
        let routes = self.validated_engine_route_ids_for_track(engine_id, track_idx)?;
        for [route_l_id, route_r_id] in routes {
            unsafe {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    route_l_id,
                    0,
                    old_sum_l_id,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    route_r_id,
                    0,
                    old_sum_r_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    route_l_id,
                    0,
                    new_sum_l_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    route_r_id,
                    0,
                    new_sum_r_id,
                    0,
                );
            }
        }
        Ok(())
    }

    pub(super) fn move_engine_route_to_rack_consumer(
        &mut self,
        engine_id: usize,
        track_idx: usize,
        route_idx: usize,
    ) -> Result<(), String> {
        let engine = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("Missing custom engine runtime {engine_id}"))?;
        if route_idx >= engine.route_gain_ids.len() {
            return Err(format!("Rack route consumer {route_idx} is unavailable"));
        }
        if !engine.route_gain_ids[route_idx].is_empty()
            || !engine.ext_route_gain_ids[route_idx].is_empty()
        {
            return Err(format!("Rack route consumer {route_idx} is already in use"));
        }
        engine.route_gain_ids[route_idx] = std::mem::take(&mut engine.route_gain_ids[track_idx]);
        engine.ext_route_gain_ids[route_idx] =
            std::mem::take(&mut engine.ext_route_gain_ids[track_idx]);
        self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
            .store(engine_id as u32, Ordering::Release);
        for voice in 0..MAX_VOICES {
            let [left, right] = engine.route_gain_ids[route_idx][voice];
            self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                .store(0, Ordering::Release);
            self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                .store(0, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                .store(left as u64, Ordering::Release);
            self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                .store(right as u64, Ordering::Release);
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext = engine.ext_route_gain_ids[route_idx][voice][input];
                self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track_idx][input]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                    .store(ext as u64, Ordering::Release);
            }
        }
        Ok(())
    }

    pub(super) fn validated_engine_route_ids_for_track(
        &self,
        engine_id: usize,
        track_idx: usize,
    ) -> Result<Vec<[i32; 2]>, String> {
        let routes = self
            .app
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
            .and_then(|engine| engine.route_gain_ids.get(track_idx))
            .ok_or_else(|| {
                format!(
                    "Custom engine {engine_id} has no route metadata for track {}",
                    track_idx + 1
                )
            })?;
        if routes.len() != MAX_VOICES
            || routes
                .iter()
                .any(|route_pair| route_pair[0] <= 0 || route_pair[1] <= 0)
        {
            return Err(format!(
                "Custom engine {engine_id} has an incomplete route for track {}",
                track_idx + 1
            ));
        }
        Ok(routes.clone())
    }

    pub(super) fn delete_engine_runtime(&mut self, engine_id: usize) {
        let Some(engine) = self
            .app
            .graph
            .engine_node_ids
            .get_mut(engine_id)
            .and_then(Option::take)
        else {
            return;
        };

        for route_pairs in &engine.route_gain_ids {
            for route_pair in route_pairs {
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
        for ext_routes in &engine.ext_route_gain_ids {
            for route_ids in ext_routes {
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

        self.app.state.runtime.engine_voice_counts[engine_id].store(0, Ordering::Release);
        lisp_host::set_dgen_engine_enabled_voices(engine_id, 0);
        for voice in 0..MAX_VOICES {
            self.app.state.runtime.engine_voice_lids[engine_id][voice].store(0, Ordering::Release);
            self.app.state.runtime.engine_synth_node_ids[engine_id][voice]
                .store(0, Ordering::Release);
            self.app.state.runtime.engine_modulator_node_ids[engine_id][voice]
                .store(0, Ordering::Release);
            for track_idx in 0..MAX_TRACKS {
                self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.engine_ext_route_lids[engine_id][voice][track_idx]
                        [input]
                        .store(0, Ordering::Release);
                }
            }
        }
        for route_idx in MAX_TRACKS..crate::sequencer::MAX_SAMPLER_POOLS {
            if self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .load(Ordering::Acquire)
                != engine_id as u32
            {
                continue;
            }
            self.app.state.runtime.rack_engine_route_engine_ids[route_idx]
                .store(u32::MAX, Ordering::Release);
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.rack_engine_route_lids[route_idx][voice]
                    .store(0, Ordering::Release);
                self.app.state.runtime.rack_engine_route_lids_r[route_idx][voice]
                    .store(0, Ordering::Release);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.app.state.runtime.rack_engine_ext_route_lids[route_idx][voice][input]
                        .store(0, Ordering::Release);
                }
            }
        }
    }

    pub(super) fn shift_engine_route_tables_left(&mut self, track_idx: usize, old_count: usize) {
        for engine in self.app.graph.engine_node_ids.iter_mut() {
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            for idx in track_idx..old_count.saturating_sub(1) {
                engine.route_gain_ids[idx] = std::mem::take(&mut engine.route_gain_ids[idx + 1]);
                engine.ext_route_gain_ids[idx] =
                    std::mem::take(&mut engine.ext_route_gain_ids[idx + 1]);
            }
            if old_count > 0 {
                engine.route_gain_ids[old_count - 1].clear();
                engine.ext_route_gain_ids[old_count - 1].clear();
            }
            for track in track_idx..old_count.saturating_sub(1) {
                for slot in 0..MAX_RACK_SLOTS {
                    let dst = rack_slot_pool_index(track, slot).expect("valid rack route index");
                    let src = rack_slot_pool_index(track + 1, slot)
                        .expect("valid shifted rack route index");
                    engine.route_gain_ids[dst] = std::mem::take(&mut engine.route_gain_ids[src]);
                    engine.ext_route_gain_ids[dst] =
                        std::mem::take(&mut engine.ext_route_gain_ids[src]);
                }
            }
            if old_count > 0 {
                for slot in 0..MAX_RACK_SLOTS {
                    let tail = rack_slot_pool_index(old_count - 1, slot)
                        .expect("valid trailing rack route index");
                    engine.route_gain_ids[tail].clear();
                    engine.ext_route_gain_ids[tail].clear();
                }
            }
        }
        for track in track_idx..old_count.saturating_sub(1) {
            for slot in 0..MAX_RACK_SLOTS {
                let dst = rack_slot_pool_index(track, slot).expect("valid rack route index");
                let src =
                    rack_slot_pool_index(track + 1, slot).expect("valid shifted rack route index");
                let engine_id = self.app.state.runtime.rack_engine_route_engine_ids[src]
                    .load(Ordering::Acquire);
                self.app.state.runtime.rack_engine_route_engine_ids[dst]
                    .store(engine_id, Ordering::Release);
                for voice in 0..MAX_VOICES {
                    let left = self.app.state.runtime.rack_engine_route_lids[src][voice]
                        .load(Ordering::Acquire);
                    let right = self.app.state.runtime.rack_engine_route_lids_r[src][voice]
                        .load(Ordering::Acquire);
                    self.app.state.runtime.rack_engine_route_lids[dst][voice]
                        .store(left, Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[dst][voice]
                        .store(right, Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        let ext = self.app.state.runtime.rack_engine_ext_route_lids[src][voice]
                            [input]
                            .load(Ordering::Acquire);
                        self.app.state.runtime.rack_engine_ext_route_lids[dst][voice][input]
                            .store(ext, Ordering::Release);
                    }
                }
            }
        }
        if old_count > 0 {
            for slot in 0..MAX_RACK_SLOTS {
                let tail = rack_slot_pool_index(old_count - 1, slot)
                    .expect("valid trailing rack route index");
                self.app.state.runtime.rack_engine_route_engine_ids[tail]
                    .store(u32::MAX, Ordering::Release);
                for voice in 0..MAX_VOICES {
                    self.app.state.runtime.rack_engine_route_lids[tail][voice]
                        .store(0, Ordering::Release);
                    self.app.state.runtime.rack_engine_route_lids_r[tail][voice]
                        .store(0, Ordering::Release);
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.app.state.runtime.rack_engine_ext_route_lids[tail][voice][input]
                            .store(0, Ordering::Release);
                    }
                }
            }
        }
    }

    pub(super) fn compact_app_track_vectors(
        &mut self,
        track_idx: usize,
        retire_after: u64,
    ) -> Result<(), String> {
        let track_id = self
            .app
            .track_registry
            .id_at(track_idx)
            .ok_or_else(|| format!("Missing stable id for track {}", track_idx + 1))?;
        self.app.track_registry.remove(track_id);
        self.app.tracks.remove(track_idx);
        if track_idx < self.app.track_colors.len() {
            self.app.track_colors.remove(track_idx);
        }
        if track_idx < self.app.track_collapsed.len() {
            self.app.track_collapsed.remove(track_idx);
        }
        if track_idx < self.app.rack_selected_slots.len() {
            self.app.rack_selected_slots.remove(track_idx);
        }
        if track_idx < self.app.rack_pad_bank_starts.len() {
            self.app.rack_pad_bank_starts.remove(track_idx);
        }
        self.app.sampler_paths.remove(track_idx);
        self.app.graph.track_node_ids.remove(track_idx);
        self.app.graph.track_buffer_ids.remove(track_idx);
        self.app.graph.track_sample_rates.remove(track_idx);
        self.app.graph.track_voice_lids.remove(track_idx);
        self.app.graph.track_instrument_types.remove(track_idx);
        self.app.graph.track_instrument_run_modes.remove(track_idx);
        self.app.graph.track_engine_ids.remove(track_idx);
        self.app.graph.track_synth_node_ids.remove(track_idx);
        self.app.graph.track_gatepitch_node_ids.remove(track_idx);
        self.app.graph.effect_descriptors.remove(track_idx);
        self.app.graph.instrument_descriptors.remove(track_idx);
        self.app.graph.record_armed.remove(track_idx);
        self.app
            .editor
            .effect_chain_leases
            .retire_host(FxChainLocator::Track(track_idx), retire_after)?;
        self.app
            .editor
            .effect_chain_leases
            .reindex_tracks_after_delete(track_idx);
        Ok(())
    }

}
