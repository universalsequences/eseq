use std::sync::Arc;

use crossterm::event::KeyCode;
use std::ffi::CString;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, HostControl, ParamDescriptor, ParamKind, ParamScaling, BUILTIN_SLOT_COUNT,
};
use crate::lisp_effect::{self, MAX_CUSTOM_FX, MAX_MIDI_FX_SLOTS};
use crate::sequencer::InstrumentType;
use eseqlisp::vm::{format_lisp_source, Value as LispValue};
use eseqlisp::Editor as LispEditor;

use super::{
    App, CompileTarget, EffectTab, HookCallback, HookUnit, InputMode, PendingCompile,
    PendingEditor, Region,
};

#[derive(Clone, Copy)]
pub(super) enum OverlayPickerKind {
    Effect,
    Instrument,
}

fn instrument_display_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}

impl App {
    fn sync_scratch_runtime_descriptors(&self) {
        self.state.set_scratch_runtime_descriptors(
            self.graph.effect_descriptors.clone(),
            self.graph.instrument_descriptors.clone(),
        );
    }

    pub(crate) fn rebuild_scratch_runtime_from_buffer(&mut self) -> Result<(), String> {
        let track = self
            .ui
            .cursor_track
            .min(self.state.active_track_count().saturating_sub(1));
        let cursor_step = self.ui.cursor_step;
        self.sync_scratch_runtime_descriptors();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&self.state),
            self.graph.effect_descriptors.clone(),
            self.graph.instrument_descriptors.clone(),
            track,
            cursor_step,
        );
        let scratch_source =
            lisp_effect::midi_fx_library_source_with_user_source(&self.editor.scratch_buffer);
        if !scratch_source.trim().is_empty() {
            runtime.eval(&scratch_source)?;
        }
        self.editor.scratch_runtime = Some(runtime);
        Ok(())
    }

    fn register_hook_from_payload(
        &mut self,
        editor: &mut LispEditor,
        track: usize,
        payload: &LispValue,
    ) -> Option<String> {
        let LispValue::Map(map) = payload else {
            return Some("register-hook expects a payload map".to_string());
        };

        let unit = match map.get("unit").map(|v| v.borrow().clone()) {
            Some(LispValue::Keyword(name)) if name == "step" => HookUnit::Step,
            Some(LispValue::Keyword(name)) if name == "beat" => HookUnit::Beat,
            Some(LispValue::Keyword(name)) if name == "bar" => HookUnit::Bar,
            _ => return Some("hook unit must be :step, :beat, or :bar".to_string()),
        };

        let interval = match map.get("interval").map(|v| v.borrow().clone()) {
            Some(LispValue::Number(n)) if n >= 1.0 => n as u64,
            _ => return Some("hook interval must be >= 1".to_string()),
        };

        let callback = match map.get("callback").map(|v| v.borrow().clone()) {
            Some(LispValue::Closure(_, _)) => {
                let callback_name = format!("__scratch_hook_{}", self.editor.next_hook_callback_id);
                self.editor.next_hook_callback_id += 1;
                editor
                    .runtime_mut()
                    .set_global_value(&callback_name, map["callback"].borrow().clone());
                HookCallback::Global(callback_name)
            }
            Some(value) => HookCallback::Source(format_lisp_source(&value)),
            None => match map.get("code").map(|v| v.borrow().clone()) {
                Some(LispValue::String(code)) if !code.trim().is_empty() => {
                    HookCallback::Source(code)
                }
                _ => return Some("hook callback must be a quoted form or lambda".to_string()),
            },
        };

        Some(self.register_control_hook(unit, interval, track, callback))
    }

    pub fn add_saved_instrument_track_sync(&mut self, name: &str) -> Result<usize, String> {
        let source = lisp_effect::load_instrument_source(name).map_err(|e| e.to_string())?;
        let asset_base = lisp_effect::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));

        if let Some(cache_idx) = self.cached_instrument_engine_idx(name, &source) {
            let manifest = self.editor.engine_registry.engines[cache_idx]
                .manifest
                .clone();
            let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
            let lib_ptr: *const lisp_effect::LoadedDGenLib =
                &self.editor.instrument_libs[lib_index];
            return unsafe {
                self.graph_controller()
                    .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
            };
        }

        let result = lisp_effect::compile_and_load_instrument_with_asset_base(
            &source,
            self.graph.sample_rate,
            asset_base.as_deref(),
        )?;
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        }
    }

    pub fn replace_current_custom_instrument_sync(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(), String> {
        if self.tracks.is_empty() {
            return Err("No current track is available.".to_string());
        }
        let track = self.ui.cursor_track;
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err("The current track is not a custom instrument track.".to_string());
        }
        let runtime_engine_id = self
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .ok_or_else(|| {
                "The current custom instrument track has no engine binding.".to_string()
            })?;

        let asset_base = lisp_effect::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let result = lisp_effect::compile_and_load_instrument_with_asset_base(
            source,
            self.graph.sample_rate,
            asset_base.as_deref(),
        )?;
        let cache_idx = self.cache_instrument_engine(name, source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .hot_reload_instrument(track, &manifest, &*lib_ptr)
        }
        .map_err(|e| e.to_string())?;
        self.editor.engine_registry.replace_at(
            runtime_engine_id,
            super::EngineDescriptor {
                name: name.to_string(),
                source: source.to_string(),
                manifest: manifest.clone(),
                lib_index,
            },
        );

        self.tracks[track] = instrument_display_name(name);
        if let Some(sound) = self
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            sound.engine_id = Some(runtime_engine_id);
        }
        Ok(())
    }

    fn cached_instrument_engine_idx(&self, name: &str, source: &str) -> Option<usize> {
        self.editor
            .engine_registry
            .find_by_name_and_source(name, source)
    }

    fn cache_instrument_engine(
        &mut self,
        name: &str,
        source: &str,
        manifest: &lisp_effect::DGenManifest,
        lib: lisp_effect::LoadedDGenLib,
    ) -> usize {
        let lib_index = self.editor.instrument_libs.len();
        self.editor.instrument_libs.push(lib);
        let entry = super::EngineDescriptor {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest.clone(),
            lib_index,
        };
        self.editor.engine_registry.upsert(entry)
    }

    fn try_add_cached_instrument_track(&mut self, name: &str, source: &str) -> bool {
        let Some(cache_idx) = self.cached_instrument_engine_idx(name, source) else {
            return false;
        };
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        } {
            Ok(idx) => {
                self.ui.cursor_track = idx;
                self.ui.sidebar_mode = super::SidebarMode::Presets;
                self.ui.focused_region = super::Region::Cirklon;
                self.editor.status_message = Some((
                    format!("Added synth track '{}' (cached)", name),
                    Instant::now(),
                ));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
        true
    }

    pub fn next_free_custom_slot(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let chain = &self.state.pattern.effect_chains[self.ui.cursor_track];
        for offset in 0..MAX_CUSTOM_FX {
            let idx = BUILTIN_SLOT_COUNT + offset;
            if idx < chain.len() && chain[idx].node_id.load(Ordering::Relaxed) == 0 {
                return Some(idx);
            }
        }
        None
    }

    pub fn next_free_midi_fx_slot(&self, track: usize) -> Option<usize> {
        if track >= self.tracks.len() {
            return None;
        }
        let chain = self.state.pattern.track_params[track].midi_fx_chain();
        if chain.len() < MAX_MIDI_FX_SLOTS {
            Some(chain.len())
        } else {
            None
        }
    }

    pub fn add_midi_fx_to_track_sync(&mut self, track: usize, name: &str) -> Result<usize, String> {
        let slot_idx = self
            .next_free_midi_fx_slot(track)
            .ok_or_else(|| "No free MIDI FX slots available".to_string())?;
        let desc = lisp_effect::load_midi_fx_descriptor(name)
            .ok_or_else(|| format!("Unknown MIDI FX '{name}'"))?;

        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        chain.push(desc.name.clone());
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);
        self.state.pattern.midi_fx_slots[track][slot_idx].apply_descriptor(&desc, 0);

        let current_pattern = self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        let current_snapshot = crate::sequencer::PatternSnapshot::capture(
            &self.state,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        let mut bank = self.state.pattern.pattern_bank.lock().unwrap();
        for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
            if pattern_idx == current_pattern {
                *snapshot = current_snapshot.clone();
            }
        }

        self.state.publish_scheduler_snapshot();
        Ok(slot_idx)
    }

    pub fn delete_midi_fx_slot(&mut self, track: usize, slot_idx: usize) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        if slot_idx >= chain.len() {
            return Err("Invalid MIDI FX slot".to_string());
        }
        chain.remove(slot_idx);
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);

        let slots = &self.state.pattern.midi_fx_slots[track];
        for idx in slot_idx..slots.len().saturating_sub(1) {
            let next_idx = idx + 1;
            slots[idx].copy_from(&slots[next_idx]);
        }
        if let Some(last_slot) = slots.last() {
            last_slot.clear();
        }

        let current_pattern = self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        let current_snapshot = crate::sequencer::PatternSnapshot::capture(
            &self.state,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        {
            let mut bank = self.state.pattern.pattern_bank.lock().unwrap();
            for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
                if pattern_idx == current_pattern {
                    *snapshot = current_snapshot.clone();
                } else {
                    snapshot.remove_midi_fx_slot(track, slot_idx);
                }
            }
        }

        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn find_custom_slot_predecessor(&self, track: usize, offset: usize) -> (i32, usize) {
        let chain = &self.state.pattern.effect_chains[track];
        for i in (0..offset).rev() {
            let idx = BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let nid = chain[idx].node_id.load(Ordering::Relaxed);
                if nid != 0 {
                    let channels = self.graph.effect_descriptors[track][idx]
                        .output_channels
                        .max(1);
                    return (nid as i32, channels);
                }
            }
        }
        (self.graph.track_node_ids[track].pan_id, 2)
    }

    fn find_custom_slot_successor(&self, track: usize, offset: usize) -> (i32, usize) {
        let chain = &self.state.pattern.effect_chains[track];
        for i in (offset + 1)..MAX_CUSTOM_FX {
            let idx = BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let nid = chain[idx].node_id.load(Ordering::Relaxed);
                if nid != 0 {
                    let channels = self.graph.effect_descriptors[track][idx]
                        .input_channels
                        .max(1);
                    return (nid as i32, channels);
                }
            }
        }
        (self.graph.track_node_ids[track].filter_id, 2)
    }

    fn resolve_custom_slot_wiring(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> (usize, i32, usize, i32, usize, Option<i32>) {
        let offset = slot_idx - BUILTIN_SLOT_COUNT;
        let slot_id = track * MAX_CUSTOM_FX + offset;
        let (predecessor_id, predecessor_outputs) =
            self.find_custom_slot_predecessor(track, offset);
        let (successor_id, successor_inputs) = self.find_custom_slot_successor(track, offset);
        let existing_node = self.state.pattern.effect_chains[track]
            .get(slot_idx)
            .map(|slot| slot.node_id.load(Ordering::Relaxed))
            .unwrap_or(0);
        let existing = if existing_node != 0 {
            Some(existing_node as i32)
        } else {
            None
        };
        (
            slot_id,
            predecessor_id,
            predecessor_outputs,
            successor_id,
            successor_inputs,
            existing,
        )
    }

    unsafe fn connect_builtin_effect_chain(
        &self,
        predecessor_id: i32,
        predecessor_outputs: usize,
        effect_id: i32,
        effect_inputs: usize,
        effect_outputs: usize,
        successor_id: i32,
        successor_inputs: usize,
    ) {
        for src_port in 0..2 {
            for dst_port in 0..2 {
                crate::audiograph::graph_disconnect(
                    self.graph.lg.0,
                    predecessor_id,
                    src_port,
                    successor_id,
                    dst_port,
                );
            }
        }

        if effect_inputs <= 1 {
            let pred_channels = predecessor_outputs.max(1).min(2);
            for src_port in 0..pred_channels {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    predecessor_id,
                    src_port as i32,
                    effect_id,
                    0,
                );
            }
        } else {
            let pred_channels = predecessor_outputs.max(1).min(2);
            for ch in 0..pred_channels.min(effect_inputs).min(2) {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    predecessor_id,
                    ch as i32,
                    effect_id,
                    ch as i32,
                );
            }
        }

        if effect_outputs <= 1 {
            let succ_channels = successor_inputs.max(1).min(2);
            for dst_port in 0..succ_channels {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    effect_id,
                    0,
                    successor_id,
                    dst_port as i32,
                );
            }
        } else {
            let succ_channels = successor_inputs.max(1).min(2);
            for ch in 0..succ_channels.min(effect_outputs).min(2) {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    effect_id,
                    ch as i32,
                    successor_id,
                    ch as i32,
                );
            }
        }
    }

    fn create_builtin_effect_node(
        &self,
        slot_id: usize,
        desc: &EffectDescriptor,
    ) -> Result<i32, String> {
        let (vtable, state_size) = match desc.name.as_str() {
            "Filter" => (
                crate::filter::filter_vtable(),
                crate::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Delay" => (
                crate::delay::delay_vtable(),
                crate::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Reverb" => (
                crate::reverb::reverb_vtable(),
                crate::reverb::REVERB_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "444 Compressor" | "Glue Compressor" => (
                crate::dynamics::dynamics_vtable(),
                crate::dynamics::DYNAMICS_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Compressor" => (
                crate::compressor::compressor_vtable(),
                crate::compressor::COMPRESSOR_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Limiter" => (
                crate::limiter::limiter_vtable(),
                crate::limiter::LIMITER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            other => return Err(format!("Unknown built-in effect '{other}'")),
        };
        let name = CString::new(format!(
            "builtin_fx_{}_{}",
            slot_id,
            desc.name.to_lowercase()
        ))
        .unwrap();
        let node_id = unsafe {
            crate::audiograph::add_node(
                self.graph.lg.0,
                vtable,
                state_size,
                name.as_ptr(),
                desc.input_channels as i32,
                desc.output_channels as i32,
                std::ptr::null(),
                0,
            )
        };
        if node_id < 0 {
            Err(format!("Failed to create built-in effect '{}'", desc.name))
        } else {
            Ok(node_id)
        }
    }

    fn push_track_effect_slot_defaults(&self, track: usize, slot_idx: usize) {
        let Some(desc) = self
            .graph
            .effect_descriptors
            .get(track)
            .and_then(|slots| slots.get(slot_idx))
        else {
            return;
        };
        for (param_idx, _) in desc.params.iter().enumerate() {
            let value = self.state.pattern.effect_chains[track][slot_idx]
                .defaults
                .get(param_idx);
            self.send_slot_param(track, slot_idx, param_idx, value);
        }
    }

    pub fn push_all_delay_bpm(&self) {
        let bpm = self.state.transport.bpm.load(Ordering::Relaxed) as f32;
        for (track_idx, descs) in self.graph.effect_descriptors.iter().enumerate() {
            for (slot_idx, desc) in descs.iter().enumerate() {
                let Some(slot) = self
                    .state
                    .pattern
                    .effect_chains
                    .get(track_idx)
                    .and_then(|chain| chain.get(slot_idx))
                else {
                    continue;
                };
                let node_id = slot.node_id.load(Ordering::Relaxed);
                if node_id != 0 {
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::delay::DELAY_PARAM_BPM,
                        "Filter" => crate::filter::FILTER_PARAM_BPM,
                        _ => continue,
                    };
                    unsafe {
                        crate::audiograph::params_push_wrapper(
                            self.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                logical_id: node_id as u64,
                                idx,
                                fvalue: bpm,
                            },
                        );
                    }
                }
            }
        }
        for bus in &self.buses {
            for (slot_idx, desc) in bus.effect_descriptors.iter().enumerate() {
                let Some(slot) = bus.effect_slots.get(slot_idx) else {
                    continue;
                };
                if slot.node_id != 0 {
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::delay::DELAY_PARAM_BPM,
                        "Filter" => crate::filter::FILTER_PARAM_BPM,
                        _ => continue,
                    };
                    unsafe {
                        crate::audiograph::params_push_wrapper(
                            self.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                logical_id: slot.node_id as u64,
                                idx,
                                fvalue: bpm,
                            },
                        );
                    }
                }
            }
        }
    }

    fn apply_builtin_effect_to_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        node_id: i32,
        desc: EffectDescriptor,
    ) {
        self.graph.effect_descriptors[track][slot_idx] = desc.clone();
        self.state.pattern.effect_chains[track][slot_idx].apply_descriptor(&desc, node_id as u32);

        let current_pattern = self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        let current_snapshot = crate::sequencer::PatternSnapshot::capture(
            &self.state,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        let mut bank = self.state.pattern.pattern_bank.lock().unwrap();
        for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
            if pattern_idx == current_pattern {
                *snapshot = current_snapshot.clone();
            } else {
                snapshot.sync_effect_slot(track, slot_idx, &desc, node_id as u32);
            }
        }
    }

    pub(super) fn load_builtin_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);
        let node_id = self.create_builtin_effect_node(slot_id, &desc)?;
        unsafe {
            if let Some(old_id) = existing {
                lisp_effect::remove_effect_from_chain(self.graph.lg.0, old_id, pred, succ);
            }
            self.connect_builtin_effect_chain(
                pred,
                pred_outputs,
                node_id,
                desc.input_channels,
                desc.output_channels,
                succ,
                succ_inputs,
            );
        }
        self.apply_builtin_effect_to_slot(track, slot_idx, node_id, desc);
        self.push_track_effect_slot_defaults(track, slot_idx);
        self.push_all_delay_bpm();
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        self.ui.effect_param_cursor = 0;
        self.ui.effect_scroll_offset = 0;
        Ok(())
    }

    pub fn add_builtin_effect_sync(&mut self, track: usize, name: &str) -> Result<usize, String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let chain = &self.state.pattern.effect_chains[track];
        let slot_idx = (0..MAX_CUSTOM_FX)
            .map(|offset| BUILTIN_SLOT_COUNT + offset)
            .find(|idx| *idx < chain.len() && chain[*idx].node_id.load(Ordering::Relaxed) == 0)
            .ok_or_else(|| "No free effect slots available".to_string())?;
        self.load_builtin_effect_to_slot_sync(track, slot_idx, name)?;
        Ok(slot_idx)
    }

    pub(super) fn effect_sidechain_labels(&self, track: usize) -> Vec<String> {
        let mut labels = vec!["off".to_string()];
        for (source_track, name) in self.tracks.iter().enumerate() {
            if source_track != track {
                labels.push(name.clone());
            }
        }
        labels
    }

    pub(super) fn effect_sidechain_source_track(
        &self,
        track: usize,
        selection_idx: usize,
    ) -> Option<usize> {
        if selection_idx == 0 {
            return None;
        }
        let mut current_idx = 0usize;
        for source_track in 0..self.tracks.len() {
            if source_track == track {
                continue;
            }
            current_idx += 1;
            if current_idx == selection_idx {
                return Some(source_track);
            }
        }
        None
    }

    fn build_effect_descriptor(
        &self,
        track: usize,
        name: &str,
        manifest: &lisp_effect::DGenManifest,
    ) -> EffectDescriptor {
        let mut desc = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );

        let sidechain_labels = self.effect_sidechain_labels(track);
        let mut modulators = manifest.modulators.clone();
        modulators.sort_by_key(|m| m.slot);
        desc.params
            .extend(modulators.into_iter().map(|modulator| ParamDescriptor {
                name: format!("sidechain {}", modulator.name),
                min: 0.0,
                max: sidechain_labels.len().saturating_sub(1) as f32,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: sidechain_labels.clone(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: u32::MAX,
                host_control: Some(HostControl::FxSidechain {
                    input_channel: modulator.input_channel,
                }),
            }));
        desc
    }

    fn build_bus_effect_descriptor(
        &self,
        name: &str,
        manifest: &lisp_effect::DGenManifest,
    ) -> EffectDescriptor {
        let mut desc = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );

        let sidechain_labels = self.bus_effect_sidechain_labels();
        let mut modulators = manifest.modulators.clone();
        modulators.sort_by_key(|m| m.slot);
        desc.params
            .extend(modulators.into_iter().map(|modulator| ParamDescriptor {
                name: format!("sidechain {}", modulator.name),
                min: 0.0,
                max: sidechain_labels.len().saturating_sub(1) as f32,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: sidechain_labels.clone(),
                },
                scaling: ParamScaling::Linear,
                node_param_idx: u32::MAX,
                host_control: Some(HostControl::FxSidechain {
                    input_channel: modulator.input_channel,
                }),
            }));
        desc
    }

    fn bus_effect_sidechain_labels(&self) -> Vec<String> {
        let mut labels = vec!["off".to_string()];
        labels.extend(self.tracks.iter().cloned());
        labels
    }

    fn bus_effect_sidechain_source_track(&self, selection_idx: usize) -> Option<usize> {
        if selection_idx == 0 {
            None
        } else {
            Some(selection_idx - 1).filter(|idx| *idx < self.tracks.len())
        }
    }

    pub(super) fn refresh_effect_sidechain_labels(&mut self) {
        for track in 0..self.graph.effect_descriptors.len() {
            let labels = self.effect_sidechain_labels(track);
            for desc in &mut self.graph.effect_descriptors[track] {
                for param in &mut desc.params {
                    if matches!(param.host_control, Some(HostControl::FxSidechain { .. })) {
                        param.max = labels.len().saturating_sub(1) as f32;
                        param.kind = ParamKind::Enum {
                            labels: labels.clone(),
                        };
                    }
                }
            }
        }

        let bus_labels = self.bus_effect_sidechain_labels();
        for bus in &mut self.buses {
            for desc in &mut bus.effect_descriptors {
                for param in &mut desc.params {
                    if matches!(param.host_control, Some(HostControl::FxSidechain { .. })) {
                        param.max = bus_labels.len().saturating_sub(1) as f32;
                        param.kind = ParamKind::Enum {
                            labels: bus_labels.clone(),
                        };
                    }
                }
            }
        }
    }

    pub fn apply_effect_sidechain_selection(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        selection: usize,
    ) {
        let Some(desc) = self
            .graph
            .effect_descriptors
            .get(track)
            .and_then(|d| d.get(slot_idx))
        else {
            return;
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        let Some(HostControl::FxSidechain { input_channel }) = param_desc.host_control.as_ref()
        else {
            return;
        };
        let Some(slot) = self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
        else {
            return;
        };
        let node_id = slot.node_id.load(Ordering::Relaxed) as i32;
        if node_id == 0 {
            return;
        }

        let old_selection = slot.defaults.get(param_idx).round().max(0.0) as usize;
        if let Some(old_track) = self.effect_sidechain_source_track(track, old_selection) {
            let source_port = (*input_channel).min(1) as i32;
            let disconnected = unsafe {
                crate::audiograph::graph_disconnect(
                    self.graph.lg.0,
                    self.graph.track_node_ids[old_track].delay_id,
                    source_port,
                    node_id,
                    *input_channel as i32,
                )
            };
            if !disconnected {
                eprintln!(
                    "sidechain: disconnect failed effect_node={} track={} slot={} old_track={} src_port={} dst_port={}",
                    node_id,
                    track,
                    slot_idx,
                    old_track,
                    source_port,
                    *input_channel as i32,
                );
            }
        }

        if let Some(new_track) = self.effect_sidechain_source_track(track, selection) {
            let source_port = (*input_channel).min(1) as i32;
            let connected = unsafe {
                crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    self.graph.track_node_ids[new_track].delay_id,
                    source_port,
                    node_id,
                    *input_channel as i32,
                )
            };
            if !connected {
                eprintln!(
                    "sidechain: connect failed effect_node={} track={} slot={} new_track={} src_port={} dst_port={}",
                    node_id,
                    track,
                    slot_idx,
                    new_track,
                    source_port,
                    *input_channel as i32,
                );
            }
        }
    }

    pub fn apply_bus_effect_sidechain_selection(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
        selection: usize,
    ) {
        let Some(bus) = self.buses.get(bus_idx) else {
            return;
        };
        let Some(desc) = bus.effect_descriptors.get(slot_idx) else {
            return;
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        let Some(HostControl::FxSidechain { input_channel }) = param_desc.host_control.as_ref()
        else {
            return;
        };
        let Some(slot) = bus.effect_slots.get(slot_idx) else {
            return;
        };
        let node_id = slot.node_id as i32;
        if node_id == 0 {
            return;
        }

        let old_selection = slot
            .defaults
            .get(param_idx)
            .copied()
            .unwrap_or_default()
            .round()
            .max(0.0) as usize;
        if let Some(old_track) = self.bus_effect_sidechain_source_track(old_selection) {
            if let Some(nodes) = self.graph.track_node_ids.get(old_track) {
                let source_port = (*input_channel).min(1) as i32;
                unsafe {
                    crate::audiograph::graph_disconnect(
                        self.graph.lg.0,
                        nodes.delay_id,
                        source_port,
                        node_id,
                        *input_channel as i32,
                    );
                }
            }
        }

        if let Some(new_track) = self.bus_effect_sidechain_source_track(selection) {
            if let Some(nodes) = self.graph.track_node_ids.get(new_track) {
                let source_port = (*input_channel).min(1) as i32;
                unsafe {
                    crate::audiograph::graph_connect(
                        self.graph.lg.0,
                        nodes.delay_id,
                        source_port,
                        node_id,
                        *input_channel as i32,
                    );
                }
            }
        }
    }

    fn apply_effect_to_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        node_id: i32,
        name: &str,
        manifest: &lisp_effect::DGenManifest,
    ) {
        let desc = self.build_effect_descriptor(track, name, manifest);
        self.graph.effect_descriptors[track][slot_idx] = desc;

        let slot = &self.state.pattern.effect_chains[track][slot_idx];
        slot.node_id.store(node_id as u32, Ordering::Relaxed);
        let params = &self.graph.effect_descriptors[track][slot_idx].params;
        slot.num_params
            .store(params.len() as u32, Ordering::Relaxed);
        for (i, p) in params.iter().enumerate() {
            slot.defaults.set(i, p.default);
            if i < slot.param_node_indices.len() {
                slot.param_node_indices[i].store(p.node_param_idx, Ordering::Relaxed);
            }
        }

        let current_pattern = self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        let current_snapshot = crate::sequencer::PatternSnapshot::capture(
            &self.state,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        let desc = self.graph.effect_descriptors[track][slot_idx].clone();
        let mut bank = self.state.pattern.pattern_bank.lock().unwrap();
        for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
            if pattern_idx == current_pattern {
                *snapshot = current_snapshot.clone();
            } else {
                snapshot.sync_effect_slot(track, slot_idx, &desc, node_id as u32);
            }
        }
    }

    fn run_effect_editor(&mut self, slot_idx: usize, existing_name: Option<String>) {
        if self.tracks.is_empty() {
            return;
        }
        let track = self.ui.cursor_track;
        let (
            slot_id,
            predecessor_id,
            predecessor_outputs,
            successor_id,
            successor_inputs,
            existing,
        ) = self.resolve_custom_slot_wiring(track, slot_idx);

        let result = lisp_effect::run_embedded_effect_editor_flow(
            self.graph.sample_rate,
            Arc::clone(&self.state),
            track,
            existing_name.as_deref(),
            |_, result, name, _source| {
                self.apply_compiled_effect(result, name, slot_idx, track);
                Ok(())
            },
        );

        if let Some(r) = result {
            match unsafe {
                lisp_effect::add_effect_to_chain_at(
                    self.graph.lg.0,
                    slot_id,
                    &r.manifest,
                    &r.lib,
                    predecessor_id,
                    predecessor_outputs,
                    successor_id,
                    successor_inputs,
                    existing,
                )
            } {
                Ok(node_id) => {
                    self.apply_effect_to_slot(track, slot_idx, node_id, &r.name, &r.manifest);
                    self.ui.effect_tab = EffectTab::Slot(slot_idx);
                    self.ui.effect_param_cursor = 0;
                    self.ui.effect_scroll_offset = 0;
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.editor.lisp_libs.push(r.lib);
                }
                Err(error) => {
                    self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
                }
            }
        }
    }

    pub fn start_effect_compile(&mut self, name: &str, slot_idx: usize) {
        let source = match lisp_effect::load_effect_source(name) {
            Ok(s) => s,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.graph.sample_rate;
        std::thread::spawn(move || {
            let result = lisp_effect::compile_and_load(&source, sample_rate);
            let _ = tx.send(result);
        });
        self.editor.pending_compile = Some(PendingCompile {
            receiver: rx,
            target: CompileTarget::Effect {
                name: name.to_string(),
                slot_idx,
                track: self.ui.cursor_track,
            },
            tick: 0,
        });
    }

    /// Poll for async compile completion. Returns a status message if something finished.
    pub fn poll_pending_compile(&mut self) -> Option<String> {
        let pending = self.editor.pending_compile.as_ref()?;
        match pending.receiver.try_recv() {
            Ok(Ok(compile_result)) => {
                let target = match &pending.target {
                    CompileTarget::Effect {
                        name,
                        slot_idx,
                        track,
                    } => CompileTarget::Effect {
                        name: name.clone(),
                        slot_idx: *slot_idx,
                        track: *track,
                    },
                    CompileTarget::Instrument { name } => {
                        CompileTarget::Instrument { name: name.clone() }
                    }
                };
                self.editor.pending_compile = None;
                match target {
                    CompileTarget::Effect {
                        name,
                        slot_idx,
                        track,
                    } => {
                        self.apply_compiled_effect(compile_result, &name, slot_idx, track);
                        Some(format!("Loaded effect: {name}"))
                    }
                    CompileTarget::Instrument { name } => {
                        self.apply_compiled_instrument(compile_result, &name);
                        Some(format!("Loaded instrument: {name}"))
                    }
                }
            }
            Ok(Err(e)) => {
                self.editor.pending_compile = None;
                Some(format!("Compile error: {e}"))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.editor.pending_compile.as_mut().unwrap().tick += 1;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.editor.pending_compile = None;
                Some("Compile thread crashed".to_string())
            }
        }
    }

    pub fn apply_compiled_effect(
        &mut self,
        result: lisp_effect::CompileResult,
        name: &str,
        slot_idx: usize,
        track: usize,
    ) {
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);

        match unsafe {
            lisp_effect::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &result.manifest,
                &result.lib,
                pred,
                pred_outputs,
                succ,
                succ_inputs,
                existing,
            )
        } {
            Ok(node_id) => {
                self.apply_effect_to_slot(track, slot_idx, node_id, name, &result.manifest);
                self.editor.lisp_libs.push(result.lib);
                self.ui.effect_tab = EffectTab::Slot(slot_idx);
                self.ui.effect_param_cursor = 0;
                self.ui.effect_scroll_offset = 0;
                self.ui.focused_region = Region::Params;
                self.ui.params_column = 1;
                self.editor.status_message = Some((format!("Loaded '{}'", name), Instant::now()));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    pub(super) fn load_saved_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let source = lisp_effect::load_effect_source(name).map_err(|e| e.to_string())?;
        let result = lisp_effect::compile_and_load(&source, self.graph.sample_rate)?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);
        let node_id = unsafe {
            lisp_effect::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &result.manifest,
                &result.lib,
                pred,
                pred_outputs,
                succ,
                succ_inputs,
                existing,
            )
        }?;
        self.apply_effect_to_slot(track, slot_idx, node_id, name, &result.manifest);
        self.editor.lisp_libs.push(result.lib);
        Ok(())
    }

    pub fn next_free_bus_effect_slot(&self, bus_idx: usize) -> Option<usize> {
        self.buses.get(bus_idx).and_then(|bus| {
            bus.effect_descriptors
                .iter()
                .position(|desc| desc.params.is_empty())
        })
    }

    pub fn add_bus_effect_sync(&mut self, bus_idx: usize, name: &str) -> Result<usize, String> {
        let slot_idx = self
            .next_free_bus_effect_slot(bus_idx)
            .ok_or_else(|| "No free bus effect slots available".to_string())?;
        self.load_bus_effect_to_slot_sync(bus_idx, slot_idx, name)?;
        Ok(slot_idx)
    }

    pub fn add_builtin_bus_effect_sync(
        &mut self,
        bus_idx: usize,
        name: &str,
    ) -> Result<usize, String> {
        let slot_idx = self
            .next_free_bus_effect_slot(bus_idx)
            .ok_or_else(|| "No free bus effect slots available".to_string())?;
        self.load_builtin_bus_effect_to_slot_sync(bus_idx, slot_idx, name)?;
        Ok(slot_idx)
    }

    pub fn load_builtin_bus_effect_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
        let node_id = self.create_builtin_effect_node(slot_id, &desc)?;
        unsafe {
            if let Some(old_id) = existing {
                lisp_effect::remove_effect_from_chain(self.graph.lg.0, old_id, pred, succ);
            }
            self.connect_builtin_effect_chain(
                pred,
                pred_outputs,
                node_id,
                desc.input_channels,
                desc.output_channels,
                succ,
                succ_inputs,
            );
        }
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        bus.effect_descriptors[slot_idx] = desc.clone();
        bus.effect_slots[slot_idx] =
            crate::effects::EffectSlotSnapshot::new_default(&desc, node_id as u32);
        if slot_idx < bus.custom_effect_names.len() {
            bus.custom_effect_names[slot_idx] =
                EffectDescriptor::builtin_insert_project_name(&desc.name);
        }
        self.push_bus_effect_slot_defaults(bus_idx, slot_idx);
        self.push_all_delay_bpm();
        self.ui.effect_param_cursor = 0;
        self.ui.effect_scroll_offset = 0;
        Ok(())
    }

    pub fn load_bus_effect_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        if bus_idx >= lisp_effect::MAX_BUS_FX_CHAINS {
            return Err(format!(
                "Bus {} is outside the current bus FX registry limit",
                bus_idx + 1
            ));
        }
        let source = lisp_effect::load_effect_source(name).map_err(|e| e.to_string())?;
        let result = lisp_effect::compile_and_load(&source, self.graph.sample_rate)?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
        let node_id = unsafe {
            lisp_effect::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &result.manifest,
                &result.lib,
                pred,
                pred_outputs,
                succ,
                succ_inputs,
                existing,
            )
        }?;
        let desc = self.build_bus_effect_descriptor(name, &result.manifest);
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        bus.effect_descriptors[slot_idx] = desc;
        let slot = &mut bus.effect_slots[slot_idx];
        *slot = crate::effects::EffectSlotSnapshot::new_default(
            &bus.effect_descriptors[slot_idx],
            node_id as u32,
        );
        if slot_idx < bus.custom_effect_names.len() {
            bus.custom_effect_names[slot_idx] = Some(name.to_string());
        }
        self.editor.lisp_libs.push(result.lib);
        Ok(())
    }

    pub fn delete_bus_effect_slot(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        let (node_id, pred, pred_outputs, succ, succ_inputs) = {
            let bus = self
                .buses
                .get(bus_idx)
                .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
            let node_id = bus
                .effect_slots
                .get(slot_idx)
                .map(|slot| slot.node_id)
                .unwrap_or(0);
            if node_id == 0 {
                (0, 0, 0, 0, 0)
            } else {
                let (_, pred, pred_outputs, succ, succ_inputs, _) =
                    self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
                (node_id, pred, pred_outputs, succ, succ_inputs)
            }
        };
        if node_id != 0 {
            unsafe {
                lisp_effect::remove_effect_from_chain(self.graph.lg.0, node_id as i32, pred, succ);
            }
            self.connect_bus_effect_gap(pred, pred_outputs, succ, succ_inputs);
        }
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        if slot_idx >= bus.effect_descriptors.len() || slot_idx >= bus.effect_slots.len() {
            return Err(format!("Bus effect slot {} out of range", slot_idx + 1));
        }
        bus.effect_descriptors[slot_idx] = EffectDescriptor::empty_custom_slot();
        bus.effect_slots[slot_idx] = crate::effects::EffectSlotSnapshot::new_empty();
        if slot_idx < bus.custom_effect_names.len() {
            bus.custom_effect_names[slot_idx] = None;
        }
        Ok(())
    }

    fn connect_bus_effect_gap(
        &self,
        predecessor_id: i32,
        predecessor_outputs: usize,
        successor_id: i32,
        successor_inputs: usize,
    ) {
        let channels = predecessor_outputs.min(successor_inputs).max(1).min(2);
        for ch in 0..channels {
            unsafe {
                crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    predecessor_id,
                    ch as i32,
                    successor_id,
                    ch as i32,
                );
            }
        }
    }

    pub fn set_bus_effect_param(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let desc = bus
            .effect_descriptors
            .get(slot_idx)
            .ok_or_else(|| format!("Bus effect slot {} out of range", slot_idx + 1))?;
        let param = desc
            .params
            .get(param_idx)
            .ok_or_else(|| format!("Bus effect param {} out of range", param_idx + 1))?;
        let slot = bus
            .effect_slots
            .get_mut(slot_idx)
            .ok_or_else(|| format!("Bus effect slot {} out of range", slot_idx + 1))?;
        if param_idx < slot.defaults.len() {
            slot.defaults[param_idx] = value.clamp(param.min, param.max);
        }
        let node_id = slot.node_id;
        let node_param_idx = param.node_param_idx;
        if node_id != 0 && node_param_idx != u32::MAX {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        logical_id: node_id as u64,
                        idx: node_param_idx as u64,
                        fvalue: value.clamp(param.min, param.max),
                    },
                );
            }
        }
        Ok(())
    }

    fn resolve_bus_effect_slot_wiring(
        &self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(usize, i32, usize, i32, usize, Option<i32>), String> {
        let bus_nodes = self
            .graph
            .bus_node_ids
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} graph nodes not found", bus_idx + 1))?;
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        if slot_idx >= bus.effect_descriptors.len() || slot_idx >= bus.effect_slots.len() {
            return Err(format!("Bus effect slot {} out of range", slot_idx + 1));
        }

        let slot_id = (crate::sequencer::MAX_TRACKS + bus_idx) * MAX_CUSTOM_FX + slot_idx;
        let mut predecessor_id = bus_nodes.gate_id;
        let mut predecessor_outputs = 2;
        for idx in (0..slot_idx).rev() {
            let node_id = bus.effect_slots[idx].node_id;
            if node_id != 0 {
                predecessor_id = node_id as i32;
                predecessor_outputs = bus.effect_descriptors[idx].output_channels.max(1);
                break;
            }
        }

        let mut successor_id = bus_nodes.volume_id;
        let mut successor_inputs = 2;
        for idx in (slot_idx + 1)..bus.effect_slots.len() {
            let node_id = bus.effect_slots[idx].node_id;
            if node_id != 0 {
                successor_id = node_id as i32;
                successor_inputs = bus.effect_descriptors[idx].input_channels.max(1);
                break;
            }
        }

        let existing_node = bus.effect_slots[slot_idx].node_id;
        let existing = (existing_node != 0).then_some(existing_node as i32);
        Ok((
            slot_id,
            predecessor_id,
            predecessor_outputs,
            successor_id,
            successor_inputs,
            existing,
        ))
    }

    pub fn bus_effect_param_option_index(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
        label: &str,
    ) -> Option<usize> {
        self.buses
            .get(bus_idx)?
            .effect_descriptors
            .get(slot_idx)?
            .params
            .get(param_idx)
            .and_then(|p| match &p.kind {
                ParamKind::Enum { labels } => labels.iter().position(|item| item == label),
                _ => None,
            })
    }

    pub fn push_bus_effect_slot_defaults(&self, bus_idx: usize, slot_idx: usize) {
        let Some(bus) = self.buses.get(bus_idx) else {
            return;
        };
        let Some(slot) = bus.effect_slots.get(slot_idx) else {
            return;
        };
        let Some(desc) = bus.effect_descriptors.get(slot_idx) else {
            return;
        };
        if slot.node_id == 0 {
            return;
        }
        for (param_idx, param) in desc.params.iter().enumerate() {
            if param.node_param_idx == u32::MAX || param_idx >= slot.defaults.len() {
                if matches!(param.host_control, Some(HostControl::FxSidechain { .. }))
                    && param_idx < slot.defaults.len()
                {
                    self.apply_bus_effect_sidechain_selection(
                        bus_idx,
                        slot_idx,
                        param_idx,
                        slot.defaults[param_idx].round().max(0.0) as usize,
                    );
                }
                continue;
            }
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        logical_id: slot.node_id as u64,
                        idx: param.node_param_idx as u64,
                        fvalue: slot.defaults[param_idx],
                    },
                );
            }
        }
    }

    pub(super) fn replace_current_effect_sync(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(), String> {
        if self.tracks.is_empty() {
            return Err("No current track is available.".to_string());
        }
        let track = self.ui.cursor_track;
        let slot_idx = self
            .selected_effect_slot()
            .ok_or_else(|| "No current custom effect slot is selected.".to_string())?;
        if slot_idx < BUILTIN_SLOT_COUNT {
            return Err("The selected effect slot is not a custom effect slot.".to_string());
        }
        crate::lisp_effect::save_effect(name, source).map_err(|e| e.to_string())?;
        self.load_saved_effect_to_slot_sync(track, slot_idx, name)?;
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        Ok(())
    }

    pub(super) fn apply_compiled_instrument(
        &mut self,
        result: lisp_effect::CompileResult,
        name: &str,
    ) {
        let source = lisp_effect::load_instrument_source(name).unwrap_or_default();
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        } {
            Ok(idx) => {
                self.ui.cursor_track = idx;
                self.ui.sidebar_mode = super::SidebarMode::Presets;
                self.ui.focused_region = super::Region::Cirklon;
                self.editor.status_message =
                    Some((format!("Added synth track '{}'", name), Instant::now()));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    fn run_instrument_editor(&mut self, existing_name: Option<String>) {
        let result = lisp_effect::run_embedded_instrument_editor_flow(
            self.graph.sample_rate,
            Arc::clone(&self.state),
            Some(self.ui.cursor_track),
            existing_name.as_deref(),
            |_, result, name, source| {
                let is_existing_custom = self.ui.cursor_track
                    < self.graph.track_instrument_types.len()
                    && self.graph.track_instrument_types[self.ui.cursor_track]
                        == InstrumentType::Custom;

                if is_existing_custom {
                    let track = self.ui.cursor_track;
                    let runtime_engine_id =
                        self.graph.track_engine_ids.get(track).and_then(|id| *id);
                    let cache_idx =
                        self.cache_instrument_engine(name, source, &result.manifest, result.lib);
                    let manifest = self.editor.engine_registry.engines[cache_idx]
                        .manifest
                        .clone();
                    let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
                    let lib_ptr: *const lisp_effect::LoadedDGenLib =
                        &self.editor.instrument_libs[lib_index];
                    unsafe {
                        self.graph_controller()
                            .hot_reload_instrument(track, &manifest, &*lib_ptr)
                    }
                    .map_err(|e| e.to_string())?;
                    if let Some(runtime_engine_id) = runtime_engine_id {
                        self.editor.engine_registry.replace_at(
                            runtime_engine_id,
                            super::EngineDescriptor {
                                name: name.to_string(),
                                source: source.to_string(),
                                manifest: manifest.clone(),
                                lib_index,
                            },
                        );
                    }
                    self.tracks[self.ui.cursor_track] = instrument_display_name(name);
                    if let Some(sound) = self
                        .state
                        .pattern
                        .track_sound_state
                        .lock()
                        .unwrap()
                        .get_mut(track)
                    {
                        sound.engine_id = runtime_engine_id;
                    }
                    self.editor.status_message =
                        Some((format!("Reloaded instrument '{}'", name), Instant::now()));
                } else {
                    self.apply_compiled_instrument(result, name);
                }
                Ok(())
            },
        );

        if let Some(r) = result {
            let is_existing_custom = self.ui.cursor_track < self.graph.track_instrument_types.len()
                && self.graph.track_instrument_types[self.ui.cursor_track]
                    == InstrumentType::Custom;

            if is_existing_custom {
                let track = self.ui.cursor_track;
                let runtime_engine_id = self.graph.track_engine_ids.get(track).and_then(|id| *id);
                let cache_idx =
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib);
                let manifest = self.editor.engine_registry.engines[cache_idx]
                    .manifest
                    .clone();
                let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
                let lib_ptr: *const lisp_effect::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller()
                        .hot_reload_instrument(track, &manifest, &*lib_ptr)
                } {
                    Ok(()) => {
                        if let Some(runtime_engine_id) = runtime_engine_id {
                            self.editor.engine_registry.replace_at(
                                runtime_engine_id,
                                super::EngineDescriptor {
                                    name: r.name.clone(),
                                    source: r.source.clone(),
                                    manifest: manifest.clone(),
                                    lib_index,
                                },
                            );
                        }
                        self.tracks[self.ui.cursor_track] = instrument_display_name(&r.name);
                        if let Some(sound) = self
                            .state
                            .pattern
                            .track_sound_state
                            .lock()
                            .unwrap()
                            .get_mut(track)
                        {
                            sound.engine_id = runtime_engine_id;
                        }
                        self.editor.status_message =
                            Some((format!("Reloaded instrument '{}'", r.name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            } else {
                let cache_idx =
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib);
                let manifest = self.editor.engine_registry.engines[cache_idx]
                    .manifest
                    .clone();
                let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
                let lib_ptr: *const lisp_effect::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller()
                        .add_custom_track(&r.name, cache_idx, &manifest, &*lib_ptr)
                } {
                    Ok(idx) => {
                        self.ui.cursor_track = idx;
                        self.ui.sidebar_mode = super::SidebarMode::Presets;
                        self.ui.focused_region = super::Region::Cirklon;
                        self.editor.status_message =
                            Some((format!("Added synth track '{}'", r.name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
        }
    }

    fn run_scratch_editor(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let scratch_buffer = self.editor.scratch_buffer.clone();
        let scratch_cursor = self.editor.scratch_cursor;
        let track = self.ui.cursor_track;
        let cursor_step = self.ui.cursor_step;
        self.sync_scratch_runtime_descriptors();
        let mut runtime = self.editor.scratch_runtime.take().unwrap_or_else(|| {
            lisp_effect::ScratchControlRuntime::new(
                Arc::clone(&self.state),
                self.graph.effect_descriptors.clone(),
                self.graph.instrument_descriptors.clone(),
                track,
                cursor_step,
            )
        });
        runtime.sync_descriptors(
            self.graph.effect_descriptors.clone(),
            self.graph.instrument_descriptors.clone(),
        );
        let midi_fx_library = lisp_effect::load_midi_fx_library_source();
        if !midi_fx_library.trim().is_empty() {
            let _ = runtime.eval(&midi_fx_library);
        }
        if let Some((text, cursor, runtime)) = lisp_effect::run_embedded_scratch_flow(
            track,
            cursor_step,
            &scratch_buffer,
            scratch_cursor,
            runtime,
            |editor, event| match event {
                Some((name, payload)) => match name {
                    "register-hook" => self.register_hook_from_payload(editor, track, payload),
                    "clear-hooks" => Some(self.clear_control_hooks()),
                    "sync-current-buffer" => {
                        self.editor.scratch_buffer = editor.active_buffer().text();
                        self.editor.scratch_cursor = editor.active_buffer().cursor;
                        self.sync_scratch_runtime_descriptors();
                        self.state
                            .set_scratch_source(self.editor.scratch_buffer.clone());
                        None
                    }
                    _ => None,
                },
                None => {
                    self.tick_control_hooks_with_editor(editor);
                    None
                }
            },
        ) {
            self.editor.scratch_buffer = text;
            self.editor.scratch_cursor = cursor;
            self.state
                .set_scratch_source(self.editor.scratch_buffer.clone());
            self.editor.scratch_runtime = Some(runtime);
        }
    }

    pub fn has_pending_editor(&self) -> bool {
        self.editor.pending_editor.is_some()
    }

    pub fn run_pending_editor(&mut self) {
        let Some(action) = self.editor.pending_editor.take() else {
            return;
        };

        match action {
            PendingEditor::Effect { slot_idx, name } => self.run_effect_editor(slot_idx, name),
            PendingEditor::Instrument { name } => self.run_instrument_editor(name),
            PendingEditor::Scratch => self.run_scratch_editor(),
        }
    }

    pub(super) fn overlay_new_label(kind: OverlayPickerKind) -> &'static str {
        match kind {
            OverlayPickerKind::Effect => "+ New effect",
            OverlayPickerKind::Instrument => "+ New instrument",
        }
    }

    pub(super) fn filtered_overlay_items(&self, kind: OverlayPickerKind) -> Vec<String> {
        let mut items = vec![Self::overlay_new_label(kind).to_string()];
        let filter_lower = self.editor.picker_filter.to_lowercase();
        for name in &self.editor.picker_items {
            if filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower) {
                items.push(name.clone());
            }
        }
        items
    }

    fn handle_overlay_picker_input(&mut self, kind: OverlayPickerKind, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.editor.picker_filter.push(c);
                self.editor.picker_cursor = 0;
            }
            KeyCode::Backspace => {
                self.editor.picker_filter.pop();
                self.editor.picker_cursor = 0;
            }
            KeyCode::Up => {
                if self.editor.picker_cursor > 0 {
                    self.editor.picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.filtered_overlay_items(kind).len();
                if self.editor.picker_cursor + 1 < max {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_overlay_items(kind);
                if self.editor.picker_cursor < items.len() {
                    let selected = &items[self.editor.picker_cursor];
                    if selected == Self::overlay_new_label(kind) {
                        match kind {
                            OverlayPickerKind::Effect => {
                                if let Some(slot_idx) = self.next_free_custom_slot() {
                                    self.editor.pending_editor = Some(PendingEditor::Effect {
                                        slot_idx,
                                        name: None,
                                    });
                                }
                            }
                            OverlayPickerKind::Instrument => {
                                self.editor.pending_editor =
                                    Some(PendingEditor::Instrument { name: None });
                            }
                        }
                    } else {
                        let name = selected.clone();
                        match kind {
                            OverlayPickerKind::Effect => {
                                if let Some(slot_idx) = self.next_free_custom_slot() {
                                    self.start_effect_compile(&name, slot_idx);
                                }
                            }
                            OverlayPickerKind::Instrument => {
                                self.start_instrument_compile(&name);
                            }
                        }
                    }
                }
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.input_mode = InputMode::Normal;
                if matches!(kind, OverlayPickerKind::Instrument) && !self.tracks.is_empty() {
                    self.ui.sidebar_mode = super::SidebarMode::Audition;
                    self.ui.focused_region = super::Region::Cirklon;
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_effect_picker(&mut self, code: KeyCode) {
        self.handle_overlay_picker_input(OverlayPickerKind::Effect, code);
    }

    pub(super) fn handle_instrument_picker_overlay(&mut self, code: KeyCode) {
        self.handle_overlay_picker_input(OverlayPickerKind::Instrument, code);
    }

    pub(super) fn instrument_usage_count(&self, instrument_name: &str) -> usize {
        self.graph
            .track_engine_ids
            .iter()
            .filter_map(|engine_id| {
                engine_id.and_then(|id| self.editor.engine_registry.engines.get(id))
            })
            .filter(|engine| engine.name == instrument_name)
            .count()
    }

    pub(super) fn instrument_picker_label(&self, instrument_name: &str) -> String {
        let usage_count = self.instrument_usage_count(instrument_name);
        if usage_count == 0 {
            instrument_name.to_string()
        } else {
            format!("{instrument_name}  [in use x{usage_count}]")
        }
    }

    fn start_instrument_compile(&mut self, name: &str) {
        let source = match lisp_effect::load_instrument_source(name) {
            Ok(s) => s,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        if self.try_add_cached_instrument_track(name, &source) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.graph.sample_rate;
        let asset_base = lisp_effect::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        std::thread::spawn(move || {
            let result = lisp_effect::compile_and_load_instrument_with_asset_base(
                &source,
                sample_rate,
                asset_base.as_deref(),
            );
            let _ = tx.send(result);
        });
        self.editor.pending_compile = Some(PendingCompile {
            receiver: rx,
            target: CompileTarget::Instrument {
                name: name.to_string(),
            },
            tick: 0,
        });
    }
}
