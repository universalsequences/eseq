use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::effects::EffectDescriptor;
use crate::lisp_effect::{self, DGenManifest, LoadedDGenLib};
use crate::sequencer::{InstrumentType, MAX_TRACKS};
use crate::voice::MAX_VOICES;

use super::{App, EngineNodeIds, TrackNodeIds};

const DELETE_WITHOUT_SHIFT_ENV: &str = "TINYSEQ_DELETE_TRACK_WITHOUT_SHIFT";

struct TrackShell {
    voice_sum_id: i32,
    voice_sum_r_id: i32,
    pan_id: i32,
    filter_id: i32,
    delay_id: i32,
    send_id: i32,
}

struct SamplerVoiceSetup {
    sampler_ids: Vec<i32>,
    voice_lids: Vec<u64>,
}

enum InstrumentRegistration<'a> {
    Sampler {
        buffer_id: i32,
        sampler_ids: Vec<i32>,
    },
    Custom {
        engine_id: usize,
        manifest: &'a DGenManifest,
    },
}

struct TrackRegistration<'a> {
    idx: usize,
    track_name: String,
    shell: TrackShell,
    voice_lids: Vec<u64>,
    instrument: InstrumentRegistration<'a>,
}

pub struct GraphController<'a> {
    app: &'a mut App,
}

struct GraphEditBatchGuard {
    lg: *mut crate::audiograph::LiveGraph,
}

impl GraphEditBatchGuard {
    fn new(lg: *mut crate::audiograph::LiveGraph) -> Self {
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        Self { lg }
    }
}

impl Drop for GraphEditBatchGuard {
    fn drop(&mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
    }
}

impl App {
    pub fn graph_controller(&mut self) -> GraphController<'_> {
        GraphController { app: self }
    }
}

impl GraphController<'_> {
    pub fn add_track(&mut self, wav_path: &Path) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let (buffer_id, track_name) =
            crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        let shell = self.create_track_shell(&track_name);
        let voices = self.build_sampler_voices(
            &track_name,
            buffer_id,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sampler_ids: voices.sampler_ids,
            },
        });
        let sample_path = wav_path.to_path_buf();
        let sample_name = self.app.tracks[idx].clone();
        self.app.sampler_paths.push(Some(sample_path.clone()));
        self.app.register_sample_path(&sample_name, sample_path);
        Ok(idx)
    }

    pub fn add_custom_track(
        &mut self,
        name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<usize, String> {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let shell = self.create_track_shell(name);
        self.ensure_custom_engine_runtime(engine_id, name, manifest, lib)?;
        self.connect_engine_to_track(
            engine_id,
            idx,
            name,
            shell.voice_sum_id,
            shell.voice_sum_r_id,
        )?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name: name.to_string(),
            shell,
            voice_lids: Vec::new(),
            instrument: InstrumentRegistration::Custom {
                engine_id,
                manifest,
            },
        });
        self.app.sampler_paths.push(None);
        Ok(idx)
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
                self.initialize_instrument_slot(bound_track, &track_name, manifest);
            }
        }

        Ok(())
    }

    pub fn apply_sample_ids(&mut self, sample_ids: &[(i32, String)]) {
        for (track, (buffer_id, name)) in sample_ids.iter().enumerate() {
            if *buffer_id < 0 {
                continue;
            }
            if track >= self.app.tracks.len() {
                break;
            }
            if !self.app.is_sampler_track(track) {
                continue;
            }
            self.send_buffer_to_all_voices(track, *buffer_id);
            self.app.graph.track_buffer_ids[track] = *buffer_id;
            self.app.tracks[track] = name.clone();
            self.app.sync_sampler_path_from_name(track, name);
        }
    }

    pub fn clear_all_tracks(&mut self) {
        let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);

        for track_idx in 0..self.app.tracks.len() {
            for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
                ..self.app.state.pattern.effect_chains[track_idx].len()
            {
                let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
                let node_id = slot.node_id.load(Ordering::Relaxed);
                if node_id == 0 {
                    continue;
                }
                let offset = slot_idx - crate::effects::BUILTIN_SLOT_COUNT;
                let predecessor_id = self.find_custom_slot_predecessor(track_idx, offset);
                let successor_id = self.find_custom_slot_successor(track_idx, offset);
                unsafe {
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        0,
                        node_id as i32,
                        0,
                    );
                    crate::audiograph::graph_disconnect(
                        self.app.graph.lg.0,
                        node_id as i32,
                        0,
                        successor_id,
                        0,
                    );
                    crate::audiograph::graph_connect(
                        self.app.graph.lg.0,
                        predecessor_id,
                        0,
                        successor_id,
                        0,
                    );
                    crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
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
            for &sampler_id in &track.sampler_ids {
                unsafe {
                    crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
                }
            }
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, track.send_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.delay_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.filter_id);
                crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.pan_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_r_id);
                crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_id);
            }
        }

        self.app.tracks.clear();
        self.app.sampler_paths.clear();
        self.app.graph.track_node_ids.clear();
        self.app.graph.track_buffer_ids.clear();
        self.app.graph.track_voice_lids.clear();
        self.app.graph.track_instrument_types.clear();
        self.app.graph.track_engine_ids.clear();
        self.app.graph.track_synth_node_ids.clear();
        self.app.graph.track_gatepitch_node_ids.clear();
        self.app.graph.engine_node_ids.clear();
        self.app.graph.effect_descriptors.clear();
        self.app.graph.instrument_descriptors.clear();
        self.app.graph.record_armed.clear();

        self.app.ui.cursor_track = 0;
        self.app.ui.cursor_step = 0;
        self.app.ui.pattern_page = 0;
        self.app.ui.focused_region = super::Region::Sidebar;
        self.app.ui.sidebar_mode = super::SidebarMode::InstrumentPicker;

        self.app
            .state
            .transport
            .num_tracks
            .store(0, Ordering::Release);
        self.app
            .state
            .pattern
            .current_pattern
            .store(0, Ordering::Relaxed);
        self.app
            .state
            .pattern
            .num_patterns
            .store(1, Ordering::Relaxed);
        *self.app.state.pattern.pattern_bank.lock().unwrap() =
            vec![crate::sequencer::PatternSnapshot::new_default(0, &[])];
    }

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

        let names = self.app.tracks.clone();
        let buffer_ids = self.app.graph.track_buffer_ids.clone();
        let instrument_types = self.app.graph.track_instrument_types.clone();
        let deleted_engine_id = self.app.graph.track_engine_ids[track_idx];

        {
            let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
            self.delete_custom_effect_chain(track_idx);
            self.delete_track_engine_routes(track_idx);

            let track_nodes = self.app.graph.track_node_ids[track_idx].clone();
            self.delete_track_shell(&track_nodes);

            if let Some(engine_id) = deleted_engine_id {
                if !self.engine_is_still_referenced_excluding(engine_id, track_idx) {
                    self.delete_engine_runtime(engine_id);
                }
            }

            self.shift_engine_route_tables_left(track_idx, old_count);
        }

        if !self
            .app
            .state
            .remove_track(
                track_idx,
                &buffer_ids,
                &names,
                &instrument_types,
                &self.app.graph.effect_descriptors,
            )
        {
            return Err("Failed to compact sequencer state for deleted track".to_string());
        }

        self.compact_app_track_vectors(track_idx);
        self.rebind_live_track_runtime_after_delete();
        self.app.refresh_effect_sidechain_labels();
        self.app.push_all_restored_defaults();

        let new_selected = track_idx.min(self.app.tracks.len().saturating_sub(1));
        self.app.ui.cursor_track = new_selected;
        self.app.ui.cursor_step = self
            .app
            .ui
            .cursor_step
            .min(self.app.state.pattern.track_params[new_selected].get_num_steps().saturating_sub(1));

        Ok(new_selected)
    }

    fn clear_track_in_place(&mut self, track_idx: usize) -> Result<usize, String> {
        if track_idx >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }

        if self.app.is_sampler_track(track_idx) {
            self.send_buffer_to_all_voices(track_idx, -1);
        }

        for engine_id in 0..MAX_TRACKS {
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
        self.app.graph.track_instrument_types[track_idx] = InstrumentType::Sampler;
        self.app.graph.track_engine_ids[track_idx] = None;
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

    pub fn send_buffer_to_all_voices(&self, track: usize, buffer_id: i32) {
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
                }
            }
        }
    }

    fn find_custom_slot_predecessor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (0..offset).rev() {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    return node_id as i32;
                }
            }
        }
        self.app.graph.track_node_ids[track].pan_id
    }

    fn find_custom_slot_successor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.app.state.pattern.effect_chains[track];
        for i in (offset + 1)..crate::lisp_effect::MAX_CUSTOM_FX {
            let idx = crate::effects::BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let node_id = chain[idx].node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    return node_id as i32;
                }
            }
        }
        self.app.graph.track_node_ids[track].filter_id
    }

    fn delete_custom_effect_chain(&mut self, track_idx: usize) {
        for slot_idx in crate::effects::BUILTIN_SLOT_COUNT
            ..self.app.state.pattern.effect_chains[track_idx].len()
        {
            let slot = &self.app.state.pattern.effect_chains[track_idx][slot_idx];
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            let offset = slot_idx - crate::effects::BUILTIN_SLOT_COUNT;
            let predecessor_id = self.find_custom_slot_predecessor(track_idx, offset);
            let successor_id = self.find_custom_slot_successor(track_idx, offset);
            unsafe {
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    predecessor_id,
                    0,
                    node_id as i32,
                    0,
                );
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    node_id as i32,
                    0,
                    successor_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    predecessor_id,
                    0,
                    successor_id,
                    0,
                );
                crate::audiograph::delete_node(self.app.graph.lg.0, node_id as i32);
            }
        }
    }

    fn delete_track_engine_routes(&mut self, track_idx: usize) {
        for (engine_id, engine) in self.app.graph.engine_node_ids.iter_mut().enumerate() {
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            if track_idx >= engine.route_gain_ids.len() {
                continue;
            }
            for route_pair in &engine.route_gain_ids[track_idx] {
                for &route_id in route_pair {
                    if route_id <= 0 {
                        continue;
                    }
                    unsafe {
                        crate::audiograph::delete_node(self.app.graph.lg.0, route_id);
                    }
                }
            }
            engine.route_gain_ids[track_idx].clear();
            for voice in 0..MAX_VOICES {
                self.app.state.runtime.engine_route_lids[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
                self.app.state.runtime.engine_route_lids_r[engine_id][voice][track_idx]
                    .store(0, Ordering::Release);
            }
        }
    }

    fn delete_track_shell(&mut self, track: &TrackNodeIds) {
        for &sampler_id in &track.sampler_ids {
            unsafe {
                crate::audiograph::delete_node(self.app.graph.lg.0, sampler_id);
            }
        }
        unsafe {
            crate::audiograph::delete_node(self.app.graph.lg.0, track.send_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.delay_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.filter_id);
            crate::audiograph::remove_node_from_watchlist(self.app.graph.lg.0, track.pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.pan_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_r_id);
            crate::audiograph::delete_node(self.app.graph.lg.0, track.voice_sum_id);
        }
    }

    fn engine_is_still_referenced_excluding(&self, engine_id: usize, removed_track: usize) -> bool {
        self.app
            .graph
            .track_engine_ids
            .iter()
            .enumerate()
            .any(|(track_idx, binding)| track_idx != removed_track && *binding == Some(engine_id))
    }

    fn delete_engine_runtime(&mut self, engine_id: usize) {
        let Some(engine) = self.app.graph.engine_node_ids.get_mut(engine_id).and_then(Option::take)
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
            }
        }
    }

    fn shift_engine_route_tables_left(&mut self, track_idx: usize, old_count: usize) {
        for engine in self.app.graph.engine_node_ids.iter_mut() {
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            for idx in track_idx..old_count.saturating_sub(1) {
                engine.route_gain_ids[idx] = std::mem::take(&mut engine.route_gain_ids[idx + 1]);
            }
            if old_count > 0 {
                engine.route_gain_ids[old_count - 1].clear();
            }
        }
    }

    fn compact_app_track_vectors(&mut self, track_idx: usize) {
        self.app.tracks.remove(track_idx);
        self.app.sampler_paths.remove(track_idx);
        self.app.graph.track_node_ids.remove(track_idx);
        self.app.graph.track_buffer_ids.remove(track_idx);
        self.app.graph.track_voice_lids.remove(track_idx);
        self.app.graph.track_instrument_types.remove(track_idx);
        self.app.graph.track_engine_ids.remove(track_idx);
        self.app.graph.track_synth_node_ids.remove(track_idx);
        self.app.graph.track_gatepitch_node_ids.remove(track_idx);
        self.app.graph.effect_descriptors.remove(track_idx);
        self.app.graph.instrument_descriptors.remove(track_idx);
        self.app.graph.record_armed.remove(track_idx);
    }

    fn rebind_live_track_runtime_after_delete(&mut self) {
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
                    let node_id = match slot_idx {
                        0 => nodes.filter_id as u32,
                        1 => nodes.delay_id as u32,
                        _ => slot.node_id.load(Ordering::Relaxed),
                    };
                    slot.sync_descriptor(desc, node_id);
                }
            }

            let engine_id = self.app.graph.track_engine_ids.get(track_idx).and_then(|id| *id);
            self.app.state.runtime.track_engine_ids[track_idx].store(
                engine_id.map(|id| id as u32).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
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
                    self.app.state.pattern.instrument_slots[track_idx].sync_descriptor(desc, node_id);
                }
            }
        }
    }

    fn create_track_shell(&mut self, name: &str) -> TrackShell {
        let sum_name = CString::new(format!("{}_sum_l", name)).unwrap();
        let voice_sum_id = unsafe {
            crate::audiograph::live_add_gain(self.app.graph.lg.0, 1.0, sum_name.as_ptr())
        };
        let sum_r_name = CString::new(format!("{}_sum_r", name)).unwrap();
        let voice_sum_r_id = unsafe {
            crate::audiograph::live_add_gain(self.app.graph.lg.0, 1.0, sum_r_name.as_ptr())
        };

        let pan_name = CString::new(format!("{}_pan", name)).unwrap();
        let pan_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::stereo_panner::stereo_panner_vtable(),
                crate::stereo_panner::STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(),
                pan_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        unsafe {
            crate::audiograph::add_node_to_watchlist(self.app.graph.lg.0, pan_id);
        }

        let filter_name = CString::new(format!("{}_filter", name)).unwrap();
        let filter_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::filter::filter_vtable(),
                crate::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
                filter_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };

        let delay_name = CString::new(format!("{}_delay", name)).unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::delay::delay_vtable(),
                crate::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };

        let send_name = CString::new(format!("{}_send", name)).unwrap();
        let send_id = unsafe {
            crate::audiograph::live_add_gain(self.app.graph.lg.0, 0.0, send_name.as_ptr())
        };

        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_id, 0, pan_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_r_id, 0, pan_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 0, filter_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 1, filter_id, 1);
            crate::audiograph::graph_connect(self.app.graph.lg.0, filter_id, 0, delay_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, filter_id, 1, delay_id, 1);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                delay_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                delay_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 0, send_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, pan_id, 1, send_id, 0);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                send_id,
                0,
                self.app.graph.reverb_bus_id,
                0,
            );
        }

        TrackShell {
            voice_sum_id,
            voice_sum_r_id,
            pan_id,
            filter_id,
            delay_id,
            send_id,
        }
    }

    fn build_sampler_voices(
        &mut self,
        track_name: &str,
        buffer_id: i32,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
    ) -> Result<SamplerVoiceSetup, String> {
        let mut sampler_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let node_name = format!("{}_{}", track_name, v);
            let st =
                crate::sampler::create_sampler_node(self.app.graph.lg.0, buffer_id, &node_name)?;
            unsafe {
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    st.node_id,
                    0,
                    voice_sum_id,
                    0,
                );
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    st.node_id,
                    1,
                    voice_sum_r_id,
                    0,
                );
            }
            sampler_ids.push(st.node_id);
            voice_lids.push(st.logical_id);
        }

        Ok(SamplerVoiceSetup {
            sampler_ids,
            voice_lids,
        })
    }

    fn ensure_engine_slot(&mut self, engine_id: usize) {
        while self.app.graph.engine_node_ids.len() <= engine_id {
            self.app.graph.engine_node_ids.push(None);
        }
    }

    fn graph_connect_checked(
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

    fn ensure_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        name: &str,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        if self.app.graph.engine_node_ids[engine_id].is_some() {
            return Ok(());
        }

        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut modulator_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let gp_name = CString::new(format!("{}_gp_{}", name, v)).unwrap();
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::gatepitch::gatepitch_vtable(),
                    crate::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    4,
                    std::ptr::null(),
                    0,
                )
            };
            if gp_id < 0 {
                return Err(format!(
                    "ensure_custom_engine_runtime: failed to add gatepitch node for engine {} voice {}",
                    engine_id, v
                ));
            }

            let mod_name = CString::new(format!("{}_mod_{}", name, v)).unwrap();
            let mod_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::voice_modulator::voice_modulator_vtable(),
                    crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    mod_name.as_ptr(),
                    4,
                    crate::voice_modulator::NUM_OUTPUTS as i32,
                    std::ptr::null(),
                    0,
                )
            };
            if mod_id < 0 {
                return Err(format!(
                    "ensure_custom_engine_runtime: failed to add modulator node for engine {} voice {}",
                    engine_id, v
                ));
            }

            let slot_id = engine_id * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);
            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_effect::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name = CString::new(format!("{}_engine_synth_{}", name, v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
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
                    "ensure_custom_engine_runtime: failed to add synth node for engine {} voice {} (manifest.n_inputs={})",
                    engine_id, v, manifest.n_inputs
                ));
            }
            for input in &manifest.inputs {
                if input.channel < 4 {
                    self.graph_connect_checked(
                        gp_id,
                        input.channel as i32,
                        synth_id,
                        input.channel as i32,
                        &format!(
                            "ensure_custom_engine_runtime engine {} voice {} input {}",
                            engine_id, v, input.channel
                        ),
                    )?;
                }
            }
            self.graph_connect_checked(
                gp_id,
                0,
                mod_id,
                0,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                1,
                mod_id,
                1,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                2,
                mod_id,
                2,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            self.graph_connect_checked(
                gp_id,
                3,
                mod_id,
                3,
                &format!(
                    "ensure_custom_engine_runtime engine {} voice {}",
                    engine_id, v
                ),
            )?;
            for mod_out in 0..crate::voice_modulator::NUM_OUTPUTS {
                let synth_in = 4 + mod_out as i32;
                if manifest.n_inputs > synth_in as usize {
                    self.graph_connect_checked(
                        mod_id,
                        mod_out as i32,
                        synth_id,
                        synth_in,
                        &format!(
                            "ensure_custom_engine_runtime engine {} voice {} mod {}",
                            engine_id, v, mod_out
                        ),
                    )?;
                }
            }

            gatepitch_ids.push(gp_id);
            modulator_ids.push(mod_id);
            synth_ids.push(synth_id);
            voice_lids.push(gp_id as u64);
        }

        self.app.graph.engine_node_ids[engine_id] = Some(EngineNodeIds {
            synth_ids,
            synth_outputs: manifest.n_outputs.max(1),
            gatepitch_ids,
            modulator_ids,
            route_gain_ids: (0..MAX_TRACKS).map(|_| Vec::new()).collect(),
        });

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.engine_voice_lids[engine_id][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.engine_voice_counts[engine_id]
            .store(MAX_VOICES as u32, Ordering::Release);
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

    fn connect_engine_to_track(
        &mut self,
        engine_id: usize,
        track_idx: usize,
        track_name: &str,
        voice_sum_id: i32,
        voice_sum_r_id: i32,
    ) -> Result<(), String> {
        self.ensure_engine_slot(engine_id);
        let Some(existing_engine) = self.app.graph.engine_node_ids[engine_id].as_ref() else {
            return Err(format!(
                "connect_engine_to_track: missing engine runtime for engine {}",
                engine_id
            ));
        };
        if existing_engine.route_gain_ids[track_idx].len() == MAX_VOICES {
            return Ok(());
        }
        let synth_ids = existing_engine.synth_ids.clone();
        let synth_outputs = existing_engine.synth_outputs.max(1);

        let mut route_ids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let route_l_name =
                CString::new(format!("{}_eng{}_route_{}_l", track_name, engine_id, v)).unwrap();
            let route_l_id = unsafe {
                crate::audiograph::live_add_gain(self.app.graph.lg.0, 0.0, route_l_name.as_ptr())
            };
            if route_l_id < 0 {
                return Err(format!(
                    "connect_engine_to_track: failed to add left route gain for engine {} track {} voice {}",
                    engine_id, track_idx, v
                ));
            }
            self.graph_connect_checked(
                synth_ids[v],
                0,
                route_l_id,
                0,
                &format!(
                    "connect_engine_to_track left engine {} track {} voice {}",
                    engine_id, track_idx, v
                ),
            )?;
            self.graph_connect_checked(
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
            self.app.state.runtime.engine_route_lids[engine_id][v][track_idx]
                .store(route_l_id as u64, Ordering::Release);

            if synth_outputs > 1 {
                let route_r_name =
                    CString::new(format!("{}_eng{}_route_{}_r", track_name, engine_id, v)).unwrap();
                let route_r_id = unsafe {
                    crate::audiograph::live_add_gain(
                        self.app.graph.lg.0,
                        0.0,
                        route_r_name.as_ptr(),
                    )
                };
                if route_r_id < 0 {
                    return Err(format!(
                        "connect_engine_to_track: failed to add right route gain for engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ));
                }
                self.graph_connect_checked(
                    synth_ids[v],
                    1,
                    route_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.graph_connect_checked(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.app.state.runtime.engine_route_lids_r[engine_id][v][track_idx]
                    .store(route_r_id as u64, Ordering::Release);
                route_pair[1] = route_r_id;
            } else {
                let route_r_name =
                    CString::new(format!("{}_eng{}_route_{}_r", track_name, engine_id, v)).unwrap();
                let route_r_id = unsafe {
                    crate::audiograph::live_add_gain(
                        self.app.graph.lg.0,
                        0.0,
                        route_r_name.as_ptr(),
                    )
                };
                if route_r_id < 0 {
                    return Err(format!(
                        "connect_engine_to_track: failed to add mirrored right route gain for engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ));
                }
                self.graph_connect_checked(
                    synth_ids[v],
                    0,
                    route_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track mirrored-right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.graph_connect_checked(
                    route_r_id,
                    0,
                    voice_sum_r_id,
                    0,
                    &format!(
                        "connect_engine_to_track mirrored-right engine {} track {} voice {}",
                        engine_id, track_idx, v
                    ),
                )?;
                self.app.state.runtime.engine_route_lids_r[engine_id][v][track_idx]
                    .store(route_r_id as u64, Ordering::Release);
                route_pair[1] = route_r_id;
            }

            route_ids.push(route_pair);
        }

        let Some(engine) = self.app.graph.engine_node_ids[engine_id].as_mut() else {
            return Err(format!(
                "connect_engine_to_track: engine runtime disappeared for engine {}",
                engine_id
            ));
        };
        engine.route_gain_ids[track_idx] = route_ids;
        Ok(())
    }

    fn rebuild_custom_engine_runtime(
        &mut self,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        let Some(mut engine) = self.app.graph.engine_node_ids[engine_id].take() else {
            return Err("Missing engine runtime".to_string());
        };

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
                    .flat_map(|routes| routes.iter())
                {
                    for (route_idx, &route_id) in route_pair.iter().enumerate() {
                        if route_id <= 0 {
                            continue;
                        }
                        let src_port = if engine.synth_outputs > 1 {
                            route_idx as i32
                        } else {
                            0
                        };
                        crate::audiograph::graph_disconnect(
                            self.app.graph.lg.0,
                            old_synth,
                            src_port,
                            route_id,
                            0,
                        );
                    }
                }
                crate::audiograph::delete_node(self.app.graph.lg.0, old_synth);
            }

            let slot_id = engine_id * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);
            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = lisp_effect::dgen_total_state_slots(manifest.total_memory_slots)
                * std::mem::size_of::<f32>();
            let synth_name = CString::new(format!("engine_{}_synth_{}", engine_id, v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
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
            for input in &manifest.inputs {
                if input.channel < 4 {
                    self.graph_connect_checked(
                        gp_id,
                        input.channel as i32,
                        synth_id,
                        input.channel as i32,
                        &format!(
                            "rebuild_custom_engine_runtime engine {} voice {} input {}",
                            engine_id, v, input.channel
                        ),
                    )?;
                }
            }
            for mod_out in 0..crate::voice_modulator::NUM_OUTPUTS {
                let synth_in = 4 + mod_out as i32;
                if manifest.n_inputs > synth_in as usize {
                    self.graph_connect_checked(
                        mod_id,
                        mod_out as i32,
                        synth_id,
                        synth_in,
                        &format!(
                            "rebuild_custom_engine_runtime engine {} voice {} mod {}",
                            engine_id, v, mod_out
                        ),
                    )?;
                }
            }
            for route_pair in engine
                .route_gain_ids
                .iter()
                .flat_map(|routes| routes.iter())
            {
                for (route_idx, &route_id) in route_pair.iter().enumerate() {
                    if route_id <= 0 {
                        continue;
                    }
                    let src_port = if manifest.n_outputs > 1 {
                        route_idx as i32
                    } else {
                        0
                    };
                    self.graph_connect_checked(
                        synth_id,
                        src_port,
                        route_id,
                        0,
                        &format!(
                            "rebuild_custom_engine_runtime engine {} voice {} route {}:{}",
                            engine_id, v, route_id, src_port
                        ),
                    )?;
                }
            }

            new_synth_ids.push(synth_id);
            self.app.state.runtime.engine_synth_node_ids[engine_id][v]
                .store(synth_id as u32, Ordering::Release);
        }

        engine.synth_ids = new_synth_ids;
        for (v, &mid) in engine.modulator_ids.iter().enumerate() {
            self.app.state.runtime.engine_modulator_node_ids[engine_id][v]
                .store(mid as u32, Ordering::Release);
        }
        self.app.graph.engine_node_ids[engine_id] = Some(engine);
        Ok(())
    }

    fn finish_track_registration(&mut self, registration: TrackRegistration<'_>) {
        let TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids,
            instrument,
        } = registration;
        let instrument_type = match instrument {
            InstrumentRegistration::Sampler { .. } => InstrumentType::Sampler,
            InstrumentRegistration::Custom { .. } => InstrumentType::Custom,
        };

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.runtime.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.app.state.runtime.voice_counts[idx].store(voice_lids.len() as u32, Ordering::Release);
        self.app.state.runtime.sampler_lids[idx]
            .store(voice_lids.first().copied().unwrap_or(0), Ordering::Release);
        self.app.state.runtime.pan_lids[idx].store(shell.pan_id as u64, Ordering::Release);
        self.app.state.runtime.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.runtime.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.runtime.instrument_type_flags[idx].store(
            (instrument_type == InstrumentType::Custom) as u32,
            Ordering::Release,
        );

        let filter_desc = EffectDescriptor::builtin_filter();
        let delay_desc = EffectDescriptor::builtin_delay();
        let chain = &self.app.state.pattern.effect_chains[idx];
        chain[0].apply_descriptor(&filter_desc, shell.filter_id as u32);
        chain[1].apply_descriptor(&delay_desc, shell.delay_id as u32);

        self.app.tracks.push(track_name.clone());
        self.app
            .graph
            .effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.app.graph.record_armed.push(false);
        self.app.graph.track_voice_lids.push(voice_lids);
        self.app.graph.track_instrument_types.push(instrument_type);

        match instrument {
            InstrumentRegistration::Sampler {
                buffer_id,
                sampler_ids,
            } => {
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
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids,
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                self.app
                    .graph
                    .instrument_descriptors
                    .push(EffectDescriptor::empty_custom_slot());
            }
            InstrumentRegistration::Custom {
                engine_id,
                manifest,
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
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: Vec::new(),
                    voice_sum_id: shell.voice_sum_id,
                    voice_sum_r_id: shell.voice_sum_r_id,
                    pan_id: shell.pan_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
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
        }

        let mut bank = self.app.state.pattern.pattern_bank.lock().unwrap();
        for snap in bank.iter_mut() {
            snap.extend_to_tracks(idx + 1, &self.app.graph.effect_descriptors);
            if instrument_type == InstrumentType::Custom {
                let desc = self.app.graph.instrument_descriptors[idx].clone();
                let node_id = self.app.state.pattern.instrument_slots[idx]
                    .node_id
                    .load(Ordering::Relaxed);
                snap.sync_instrument_slot(idx, &desc, node_id, InstrumentType::Custom);
            }
        }
        drop(bank);
        self.app.refresh_effect_sidechain_labels();

        self.app
            .state
            .transport
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);
    }

    fn initialize_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        let mut inst_desc = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        for param in &mut inst_desc.params {
            if param.node_param_idx < crate::voice_modulator::MOD_PARAM_BASE {
                param.node_param_idx += lisp_effect::HEADER_SLOTS as u32;
            }
        }
        inst_desc
            .params
            .extend(crate::voice_modulator::ui_param_descriptors());
        let sorted_modulators = {
            let mut ms = manifest.modulators.clone();
            ms.sort_by_key(|m| m.slot);
            ms
        };
        let mod_source_labels: Vec<String> = std::iter::once("off".to_string())
            .chain(sorted_modulators.iter().map(|m| match m.slot {
                1 => "LFO 1".to_string(),
                2 => "ENV 1".to_string(),
                3 => "RAND".to_string(),
                4 => "DRIFT".to_string(),
                5 => "LFO 2".to_string(),
                6 => "LFO 3".to_string(),
                _ => m.name.clone(),
            }))
            .collect();
        let param_by_cell: std::collections::HashMap<usize, &crate::lisp_effect::DGenParam> =
            manifest.params.iter().map(|p| (p.cell_id, p)).collect();
        for dest in &manifest.mod_destinations {
            let source_default = param_by_cell
                .get(&dest.source_cell_id)
                .map(|p| p.default)
                .unwrap_or(0.0);
            let depth_default = param_by_cell
                .get(&dest.depth_cell_id)
                .map(|p| p.default)
                .unwrap_or(0.0);
            inst_desc.params.push(crate::effects::ParamDescriptor {
                name: format!("mod {} src", dest.name),
                min: 0.0,
                max: sorted_modulators.len() as f32,
                default: source_default,
                kind: crate::effects::ParamKind::Enum {
                    labels: mod_source_labels.clone(),
                },
                scaling: crate::effects::ParamScaling::Linear,
                node_param_idx: (lisp_effect::HEADER_SLOTS + dest.source_cell_id) as u32,
                host_control: None,
            });
            inst_desc.params.push(crate::effects::ParamDescriptor {
                name: format!("mod {} amt", dest.name),
                min: dest.depth_min.unwrap_or_else(|| {
                    param_by_cell
                        .get(&dest.depth_cell_id)
                        .map(|p| p.min)
                        .unwrap_or(-1.0)
                }),
                max: dest.depth_max.unwrap_or_else(|| {
                    param_by_cell
                        .get(&dest.depth_cell_id)
                        .map(|p| p.max)
                        .unwrap_or(1.0)
                }),
                default: depth_default,
                kind: crate::effects::ParamKind::Continuous {
                    unit: dest.unit.clone(),
                },
                scaling: crate::effects::ParamScaling::Linear,
                node_param_idx: (lisp_effect::HEADER_SLOTS + dest.depth_cell_id) as u32,
                host_control: None,
            });
        }
        let inst_slot = &self.app.state.pattern.instrument_slots[track];
        inst_slot
            .num_params
            .store(inst_desc.params.len() as u32, Ordering::Relaxed);
        for (i, p) in inst_desc.params.iter().enumerate() {
            inst_slot.defaults.set(i, p.default);
            if i < inst_slot.param_node_indices.len() {
                inst_slot.param_node_indices[i].store(p.node_param_idx, Ordering::Relaxed);
            }
        }

        if track < self.app.graph.instrument_descriptors.len() {
            self.app.graph.instrument_descriptors[track] = inst_desc;
        } else {
            self.app.graph.instrument_descriptors.push(inst_desc);
        }
    }
}

fn delete_without_shift_enabled() -> bool {
    match std::env::var(DELETE_WITHOUT_SHIFT_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}
