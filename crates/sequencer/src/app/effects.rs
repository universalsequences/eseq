use std::sync::Arc;

use std::ffi::CString;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, HostControl, ParamDescriptor, ParamKind, ParamScaling,
    BUILTIN_SLOT_COUNT,
};
use crate::lisp_host::{self, MAX_CUSTOM_FX, MAX_MIDI_FX_SLOTS};
use crate::sequencer::{BusPatternSnapshot, CustomInstrumentRunMode, InstrumentType, RackRouting};
use eseqlisp::vm::{format_lisp_source, Value as LispValue};
use eseqlisp::Editor as LispEditor;

use super::fx_chain::{
    push_fx_param, rewire_fx_chain, FxChainLocator, FxGraphEditBatch, FxLeaseSlotRemoval,
    RetainedEffectSource,
};
use super::{
    App, CompileTarget, EffectTab, HookCallback, HookUnit, PendingCompile, Region,
};

pub(crate) struct PreparedRackInstrument {
    pub name: String,
    pub engine_id: usize,
    pub manifest: lisp_host::DGenManifest,
    pub lib_index: usize,
    pub run_mode: CustomInstrumentRunMode,
}

#[derive(Clone)]
struct CustomEffectEntry {
    desc: EffectDescriptor,
    snapshot: EffectSlotSnapshot,
}

#[derive(Clone)]
struct BusEffectEntry {
    desc: EffectDescriptor,
    snapshot: EffectSlotSnapshot,
    custom_name: Option<String>,
}

/// Patch any host-routed sidechain params on a descriptor with the current
/// source-track labels. Builtin descriptors (e.g. Compressor) ship with a
/// placeholder "off"-only enum; lisp effects get theirs from
/// `build_effect_descriptor`.
fn patch_sidechain_labels(desc: &mut EffectDescriptor, labels: &[String]) {
    for param in &mut desc.params {
        if matches!(param.host_control, Some(HostControl::FxSidechain { .. })) {
            param.max = labels.len().saturating_sub(1) as f32;
            param.kind = ParamKind::Enum {
                labels: labels.to_vec(),
            };
        }
    }
}

/// Some builtin effects normal their sidechain input to an internal source
/// and need a node-state flag flipped when the host actually routes a track
/// into the port (buffer content alone can't distinguish "no selection" from
/// silence). Returns the node param index to write 1.0/0.0 into on sidechain
/// selection changes.
fn sidechain_active_state_param(effect_name: &str, input_channel: usize) -> Option<u64> {
    if effect_name != "Filterbank" {
        return None;
    }
    match input_channel {
        crate::effects::filterbank::FILTERBANK_FM_INPUT_CHANNEL => {
            Some(crate::effects::filterbank::FILTERBANK_PARAM_FM_EXT_ACTIVE)
        }
        crate::effects::filterbank::FILTERBANK_AM_INPUT_CHANNEL => {
            Some(crate::effects::filterbank::FILTERBANK_PARAM_AM_EXT_ACTIVE)
        }
        _ => None,
    }
}

fn instrument_display_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}

impl App {
    pub(super) fn reclaim_applied_effect_leases(&mut self) {
        let applied =
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(self.graph.lg.0) };
        self.editor.effect_chain_leases.reclaim_applied(applied);
    }

    fn bus_fx_locator(&self, bus_idx: usize) -> Result<FxChainLocator, String> {
        self.buses
            .get(bus_idx)
            .map(|bus| FxChainLocator::Bus(bus.id))
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))
    }

    pub(super) fn set_track_effect_lease(
        &mut self,
        track: usize,
        slot_idx: usize,
        lease: Option<lisp_host::DylibLease>,
        retire_after: u64,
    ) -> Result<(), String> {
        self.reclaim_applied_effect_leases();
        self.editor.effect_chain_leases.set(
            FxChainLocator::Track(track),
            slot_idx,
            lease,
            retire_after,
        )
    }

    pub(super) fn insert_empty_track_effect_lease_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        self.editor
            .effect_chain_leases
            .insert_empty_slot(FxChainLocator::Track(track), slot_idx)
    }

    pub(super) fn move_track_effect_lease_slot(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) -> Result<(), String> {
        self.editor.effect_chain_leases.move_slot(
            FxChainLocator::Track(track),
            source_slot,
            target_slot,
        )
    }

    pub(super) fn insert_empty_bus_effect_lease_slot(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        let locator = self.bus_fx_locator(bus_idx)?;
        self.editor
            .effect_chain_leases
            .insert_empty_slot(locator, slot_idx)
    }

    pub(super) fn move_bus_effect_lease_slot(
        &mut self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
    ) -> Result<(), String> {
        let locator = self.bus_fx_locator(bus_idx)?;
        self.editor
            .effect_chain_leases
            .move_slot(locator, source_slot, target_slot)
    }

    fn compile_saved_effect(&self, name: &str) -> Result<lisp_host::CompileResult, String> {
        let source_path = lisp_host::effect_source_path(name);
        let source = std::fs::read_to_string(&source_path).map_err(|e| e.to_string())?;
        self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            lisp_host::DGenSourceOrigin::Custom,
            &source,
            self.graph.sample_rate,
            source_path.parent(),
        )
    }

    pub(super) fn retained_effect_source_for_name(
        &self,
        name: &str,
    ) -> Result<RetainedEffectSource, String> {
        if EffectDescriptor::builtin_insert(name).is_some() {
            return Ok(RetainedEffectSource::NativeBuiltin {
                name: name.to_string(),
            });
        }
        if let Some(builtin) = crate::effects::dgen_builtin::find(name) {
            return Ok(RetainedEffectSource::Compiled {
                name: name.to_string(),
                source: builtin.source.to_string(),
                asset_base: None,
                origin: builtin.origin,
            });
        }
        let source_path = lisp_host::effect_source_path(name);
        Ok(RetainedEffectSource::Compiled {
            name: name.to_string(),
            source: std::fs::read_to_string(&source_path).map_err(|error| error.to_string())?,
            asset_base: source_path.parent().map(std::path::Path::to_path_buf),
            origin: lisp_host::DGenSourceOrigin::Custom,
        })
    }

    pub(super) fn retain_effect_source(
        &mut self,
        locator: FxChainLocator,
        slot: usize,
        source: RetainedEffectSource,
    ) -> Result<(), String> {
        self.editor.effect_chain_leases.set_source(locator, slot, Some(source))
    }

    pub(super) fn sync_scratch_runtime_descriptors(&self) {
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&self.state),
            self.graph.effect_descriptors.clone(),
            self.graph.instrument_descriptors.clone(),
            track,
            cursor_step,
        );
        let scratch_source = lisp_host::midi_fx_library_source_with_user_source(
            &lisp_host::process_library_source_with_user_source(&self.editor.scratch_buffer),
        );
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
        let source = lisp_host::load_instrument_source(name).map_err(|e| e.to_string())?;
        let run_mode = lisp_host::load_instrument_run_mode(name).map_err(|e| e.to_string())?;
        let asset_base = lisp_host::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));

        if let Some(cache_idx) = self.cached_instrument_engine_idx(name, &source) {
            let manifest = self.editor.engine_registry.engines[cache_idx]
                .manifest
                .clone();
            let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
            let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
            let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
                self.register_dedicated_instrument_engine(name, &source, &manifest, lib_index)?
            } else {
                cache_idx
            };
            return unsafe {
                self.graph_controller()
                    .add_custom_track(name, engine_id, &manifest, &*lib_ptr, run_mode)
            };
        }

        let result = lisp_host::compile_and_load_instrument_with_asset_base(
            &source,
            self.graph.sample_rate,
            asset_base.as_deref(),
        )?;
        let cache_idx =
            self.cache_instrument_engine(name, &source, &result.manifest, result.lib, result.lease);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
            self.register_dedicated_instrument_engine(name, &source, &manifest, lib_index)?
        } else {
            cache_idx
        };
        unsafe {
            self.graph_controller()
                .add_custom_track(name, engine_id, &manifest, &*lib_ptr, run_mode)
        }
    }

    pub(crate) fn prepare_saved_instrument_for_rack_slot_sync(
        &mut self,
        name: &str,
    ) -> Result<PreparedRackInstrument, String> {
        let source = lisp_host::load_instrument_source(name).map_err(|e| e.to_string())?;
        let run_mode = lisp_host::load_instrument_run_mode(name).map_err(|e| e.to_string())?;
        let asset_base = lisp_host::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));

        let cache_idx = if let Some(cache_idx) = self.cached_instrument_engine_idx(name, &source) {
            cache_idx
        } else {
            let result = lisp_host::compile_and_load_instrument_with_asset_base(
                &source,
                self.graph.sample_rate,
                asset_base.as_deref(),
            )?;
            self.cache_instrument_engine(name, &source, &result.manifest, result.lib, result.lease)
        };

        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        Ok(PreparedRackInstrument {
            name: name.to_string(),
            engine_id: cache_idx,
            manifest,
            lib_index,
            run_mode,
        })
    }

    pub fn add_saved_instrument_slot_to_rack_sync(
        &mut self,
        track: usize,
        name: &str,
    ) -> Result<usize, String> {
        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        let lib_ptr: *const lisp_host::LoadedDGenLib =
            &self.editor.instrument_libs[prepared.lib_index];
        self.apply_recorded_rack_slot_add(track, "Add rack instrument", |app| unsafe {
            app.graph_controller().add_custom_slot_to_rack(
                track,
                &prepared.name,
                prepared.engine_id,
                &prepared.manifest,
                &*lib_ptr,
                prepared.run_mode,
            )
        })
    }

    pub fn replace_rack_slot_with_saved_instrument_sync(
        &mut self,
        track: usize,
        slot: usize,
        name: &str,
    ) -> Result<(), String> {
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err("Current track is not a rack".to_string());
        }
        let rack = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .flatten()
            .ok_or_else(|| "Rack track has no rack metadata".to_string())?;
        if rack.routing != RackRouting::Broadcast || rack.slots.get(slot).is_none() {
            return Err("Invalid instrument rack layer".to_string());
        }

        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        self.apply_recorded_rack_slot_source_replacement(
            track,
            slot,
            "Replace rack instrument",
            |app| {
                app.graph_controller().replace_rack_slot_with_custom(
                    track,
                    slot,
                    &prepared.name,
                    prepared.engine_id,
                    &prepared.manifest,
                    prepared.run_mode,
                )
            },
        )
    }

    pub fn add_saved_instrument_slot_to_drum_rack_pad_sync(
        &mut self,
        track: usize,
        pad_note: i32,
        name: &str,
    ) -> Result<usize, String> {
        let prepared = self.prepare_saved_instrument_for_rack_slot_sync(name)?;
        let lib_ptr: *const lisp_host::LoadedDGenLib =
            &self.editor.instrument_libs[prepared.lib_index];
        unsafe {
            self.graph_controller().add_custom_slot_to_drum_rack_pad(
                track,
                pad_note,
                &prepared.name,
                prepared.engine_id,
                &prepared.manifest,
                &*lib_ptr,
                prepared.run_mode,
            )
        }
    }

    pub fn try_add_cached_saved_instrument_track_sync(
        &mut self,
        name: &str,
        source: &str,
        run_mode: CustomInstrumentRunMode,
    ) -> Option<Result<usize, String>> {
        let cache_idx = self.cached_instrument_engine_idx(name, source)?;
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
            match self.register_dedicated_instrument_engine(name, source, &manifest, lib_index) {
                Ok(engine_id) => engine_id,
                Err(error) => return Some(Err(error)),
            }
        } else {
            cache_idx
        };
        Some(unsafe {
            self.graph_controller()
                .add_custom_track(name, engine_id, &manifest, &*lib_ptr, run_mode)
        })
    }

    pub fn try_swap_track_to_cached_saved_instrument_sync(
        &mut self,
        track: usize,
        name: &str,
        source: &str,
        run_mode: CustomInstrumentRunMode,
    ) -> Option<Result<crate::sequencer::InstrumentSlotResetSummary, String>> {
        let cache_idx = self.cached_instrument_engine_idx(name, source)?;
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib =
            match self.editor.instrument_libs.get(lib_index) {
                Some(lib) => lib,
                None => {
                    return Some(Err(format!(
                        "Instrument engine {cache_idx} references missing library {lib_index}"
                    )));
                }
            };
        let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
            match self.register_dedicated_instrument_engine(name, source, &manifest, lib_index) {
                Ok(engine_id) => engine_id,
                Err(error) => return Some(Err(error)),
            }
        } else {
            cache_idx
        };
        Some(self.apply_recorded_instrument_binding_mutation(
            track,
            "Replace instrument",
            |app| unsafe {
                app.graph_controller().replace_track_with_custom_instrument(
                    track, name, engine_id, &manifest, &*lib_ptr, run_mode,
                )
            },
        ))
    }

    pub fn add_compiled_saved_instrument_track_sync(
        &mut self,
        name: &str,
        source: &str,
        run_mode: CustomInstrumentRunMode,
        result: lisp_host::CompileResult,
    ) -> Result<usize, String> {
        let cache_idx =
            self.cache_instrument_engine(name, source, &result.manifest, result.lib, result.lease);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
            self.register_dedicated_instrument_engine(name, source, &manifest, lib_index)?
        } else {
            cache_idx
        };
        unsafe {
            self.graph_controller()
                .add_custom_track(name, engine_id, &manifest, &*lib_ptr, run_mode)
        }
    }

    pub fn swap_track_to_compiled_saved_instrument_sync(
        &mut self,
        track: usize,
        name: &str,
        source: &str,
        run_mode: CustomInstrumentRunMode,
        result: lisp_host::CompileResult,
    ) -> Result<crate::sequencer::InstrumentSlotResetSummary, String> {
        let cache_idx =
            self.cache_instrument_engine(name, source, &result.manifest, result.lib, result.lease);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib =
            self.editor.instrument_libs.get(lib_index).ok_or_else(|| {
                format!("Instrument engine {cache_idx} references missing library {lib_index}")
            })?;
        let engine_id = if run_mode == CustomInstrumentRunMode::FreePatch {
            self.register_dedicated_instrument_engine(name, source, &manifest, lib_index)?
        } else {
            cache_idx
        };
        self.apply_recorded_instrument_binding_mutation(
            track,
            "Replace instrument",
            |app| unsafe {
                app.graph_controller().replace_track_with_custom_instrument(
                    track, name, engine_id, &manifest, &*lib_ptr, run_mode,
                )
            },
        )
    }

    pub fn add_transient_instrument_track_sync(
        &mut self,
        name: &str,
        source: &str,
        asset_base: Option<&std::path::Path>,
    ) -> Result<usize, String> {
        let result = lisp_host::compile_and_load_instrument_with_origin(
            source,
            self.graph.sample_rate,
            asset_base,
            lisp_host::DGenSourceOrigin::Draft,
        )?;
        let cache_idx =
            self.cache_instrument_engine(name, source, &result.manifest, result.lib, result.lease);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller().add_custom_track(
                name,
                cache_idx,
                &manifest,
                &*lib_ptr,
                crate::sequencer::CustomInstrumentRunMode::Instrument,
            )
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
        self.replace_custom_instrument_track_sync(track, name, source)
    }

    pub fn replace_custom_instrument_track_sync(
        &mut self,
        track: usize,
        name: &str,
        source: &str,
    ) -> Result<(), String> {
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

        let asset_base = lisp_host::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let result = lisp_host::compile_and_load_instrument_with_asset_base(
            source,
            self.graph.sample_rate,
            asset_base.as_deref(),
        )?;
        let manifest = result.manifest.clone();
        let lib_index = self.push_instrument_lib(result.lib, result.lease);
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .hot_reload_instrument(track, &manifest, &*lib_ptr)
        }
        .map_err(|e| e.to_string())?;
        self.push_instrument_defaults_for_track(track);
        self.editor.engine_registry.replace_at(
            runtime_engine_id,
            super::EngineDescriptor {
                name: name.to_string(),
                source: source.to_string(),
                manifest: manifest.clone(),
                lib_index,
                shared_runtime: self
                    .editor
                    .engine_registry
                    .get(runtime_engine_id)
                    .map(|engine| engine.shared_runtime)
                    .unwrap_or(true),
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

    pub fn replace_custom_instrument_engine_sync(
        &mut self,
        engine_id: usize,
        name: &str,
        source: &str,
    ) -> Result<(), String> {
        let track = self
            .graph
            .track_engine_ids
            .iter()
            .position(|track_engine_id| *track_engine_id == Some(engine_id))
            .ok_or_else(|| format!("No live track is using instrument engine id {engine_id}."))?;
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(
                "The target engine is not attached to a custom instrument track.".to_string(),
            );
        }

        let asset_base = lisp_host::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let result = lisp_host::compile_and_load_instrument_with_asset_base(
            source,
            self.graph.sample_rate,
            asset_base.as_deref(),
        )?;
        self.apply_compiled_instrument_engine(engine_id, name, source, result)
    }

    pub fn apply_compiled_instrument_engine(
        &mut self,
        engine_id: usize,
        name: &str,
        source: &str,
        result: lisp_host::CompileResult,
    ) -> Result<(), String> {
        let track = self
            .graph
            .track_engine_ids
            .iter()
            .position(|track_engine_id| *track_engine_id == Some(engine_id))
            .ok_or_else(|| format!("No live track is using instrument engine id {engine_id}."))?;
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
            return Err(
                "The target engine is not attached to a custom instrument track.".to_string(),
            );
        }

        let manifest = result.manifest.clone();
        let lib_index = self.push_instrument_lib(result.lib, result.lease);
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .hot_reload_instrument(track, &manifest, &*lib_ptr)
        }
        .map_err(|e| e.to_string())?;

        for bound_track in 0..self.graph.track_engine_ids.len() {
            if self.graph.track_engine_ids[bound_track] == Some(engine_id) {
                self.push_instrument_defaults_for_track(bound_track);
                self.tracks[bound_track] = instrument_display_name(name);
                if let Some(sound) = self
                    .state
                    .pattern
                    .track_sound_state
                    .lock()
                    .unwrap()
                    .get_mut(bound_track)
                {
                    sound.engine_id = Some(engine_id);
                }
            }
        }

        self.editor.engine_registry.replace_at(
            engine_id,
            super::EngineDescriptor {
                name: name.to_string(),
                source: source.to_string(),
                manifest,
                lib_index,
                shared_runtime: self
                    .editor
                    .engine_registry
                    .get(engine_id)
                    .map(|engine| engine.shared_runtime)
                    .unwrap_or(true),
            },
        );
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
        manifest: &lisp_host::DGenManifest,
        lib: lisp_host::LoadedDGenLib,
        lease: Option<lisp_host::DylibLease>,
    ) -> usize {
        let lib_index = self.push_instrument_lib(lib, lease);
        let entry = super::EngineDescriptor {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest.clone(),
            lib_index,
            shared_runtime: true,
        };
        self.editor.engine_registry.upsert(entry)
    }

    fn register_dedicated_instrument_engine(
        &mut self,
        name: &str,
        source: &str,
        manifest: &lisp_host::DGenManifest,
        lib_index: usize,
    ) -> Result<usize, String> {
        if self.editor.engine_registry.engines.len() >= self.state.runtime.engine_voice_lids.len() {
            return Err(format!(
                "Instrument engine runtime slots are exhausted; maximum runtime engines is {}",
                self.state.runtime.engine_voice_lids.len()
            ));
        }
        let entry = super::EngineDescriptor {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest.clone(),
            lib_index,
            shared_runtime: false,
        };
        Ok(self.editor.engine_registry.upsert(entry))
    }

    fn push_instrument_lib(
        &mut self,
        lib: lisp_host::LoadedDGenLib,
        lease: Option<lisp_host::DylibLease>,
    ) -> usize {
        let lib_index = self.editor.instrument_libs.len();
        self.editor.instrument_libs.push(lib);
        self.editor.instrument_lib_leases.push(lease);
        lib_index
    }

    fn try_add_cached_instrument_track(&mut self, name: &str, source: &str) -> bool {
        let Some(cache_idx) = self.cached_instrument_engine_idx(name, source) else {
            return false;
        };
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller().add_custom_track(
                name,
                cache_idx,
                &manifest,
                &*lib_ptr,
                crate::sequencer::CustomInstrumentRunMode::Instrument,
            )
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
        let desc = lisp_host::load_midi_fx_descriptor(name)
            .ok_or_else(|| format!("Unknown MIDI FX '{name}'"))?;

        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        let old_len = chain.len();
        chain.push(desc.name.clone());
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);
        self.state.pattern.midi_fx_slots[track][slot_idx].apply_descriptor(&desc, 0);

        self.state.save_current_track_midi_fx_snapshot(track);
        // The track sound's chain layout must follow the append (track-sound
        // spec §2.3), or the carrier drifts from the live chain.
        self.state
            .insert_midi_fx_slot_in_track_sound(track, slot_idx, desc.name.clone(), &desc);
        self.device_registry
            .insert_midi_effect_identity(track_id, slot_idx, old_len)?;

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
        let old_len = chain.len();
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
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

        self.state
            .remove_midi_fx_slot_from_track_patterns(track, slot_idx);
        self.device_registry
            .remove_midi_effect_identity(track_id, slot_idx, old_len)?;

        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn replace_midi_fx_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let descriptor = lisp_host::load_midi_fx_descriptor(name)
            .ok_or_else(|| format!("Unknown MIDI FX '{name}'"))?;
        self.state.replace_midi_fx_slot_in_all_track_patterns(
            track,
            slot_idx,
            descriptor.name.clone(),
            &descriptor,
        )?;
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn custom_effect_entries(&self, track: usize) -> Vec<CustomEffectEntry> {
        let chain = &self.state.pattern.effect_chains[track];
        (0..MAX_CUSTOM_FX)
            .filter_map(|offset| {
                let slot_idx = BUILTIN_SLOT_COUNT + offset;
                let slot = chain.get(slot_idx)?;
                let node_id = slot.node_id.load(Ordering::Relaxed);
                if node_id == 0 {
                    return None;
                }
                Some(CustomEffectEntry {
                    desc: self.graph.effect_descriptors[track][slot_idx].clone(),
                    snapshot: EffectSlotSnapshot::capture(slot),
                })
            })
            .collect()
    }

    fn write_custom_effect_entries(&mut self, track: usize, entries: &[CustomEffectEntry]) {
        let chain = &self.state.pattern.effect_chains[track];
        for offset in 0..MAX_CUSTOM_FX {
            let slot_idx = BUILTIN_SLOT_COUNT + offset;
            if slot_idx >= chain.len() {
                break;
            }
            if let Some(entry) = entries.get(offset) {
                self.graph.effect_descriptors[track][slot_idx] = entry.desc.clone();
                entry.snapshot.restore(&chain[slot_idx]);
            } else {
                self.graph.effect_descriptors[track][slot_idx] =
                    EffectDescriptor::empty_custom_slot();
                chain[slot_idx].clear();
            }
        }
    }

    fn publish_effect_reorder(&mut self) {
        self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        self.refresh_effect_sidechain_labels();
        self.sync_scratch_runtime_descriptors();
        self.push_all_restored_defaults();
    }

    fn sync_other_pattern_effect_insert(&mut self, track: usize, slot_idx: usize) {
        self.state
            .insert_effect_slot_in_other_track_patterns(track, slot_idx);
    }

    fn sync_other_pattern_effect_move(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        self.state
            .move_effect_slot_in_other_track_patterns(track, source_slot, target_slot);
    }

    fn sync_other_pattern_midi_fx_insert(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: String,
        desc: &EffectDescriptor,
    ) {
        self.state
            .insert_midi_fx_slot_in_other_track_patterns(track, slot_idx, name, desc);
    }

    fn sync_other_pattern_midi_fx_move(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        self.state
            .move_midi_fx_slot_in_other_track_patterns(track, source_slot, target_slot);
    }

    fn remap_other_bus_pattern_effect_slots(
        &self,
        bus_idx: usize,
        new_to_old: &[Option<usize>],
        snapshot_before_remap: &[BusPatternSnapshot],
    ) {
        self.state.remap_bus_effect_slots_in_other_scene_patterns(
            bus_idx,
            new_to_old,
            snapshot_before_remap,
        );
    }

    fn initialize_other_bus_pattern_effect_slot(&self, bus_idx: usize, slot_idx: usize) {
        let live_snapshot = self.capture_bus_pattern_snapshot();
        self.state.replace_bus_effect_slot_in_other_scene_patterns(
            bus_idx,
            slot_idx,
            &live_snapshot,
        );
    }

    pub fn copy_bus_effect_values_to_all_scenes(&self, bus_idx: usize, slot_idx: usize) -> usize {
        let live_snapshot = self.capture_bus_pattern_snapshot();
        self.state
            .copy_bus_effect_values_to_all_scene_patterns(bus_idx, slot_idx, &live_snapshot)
    }

    fn prepare_custom_effect_insert_slot(
        &mut self,
        track: usize,
        target_slot: usize,
    ) -> Result<usize, String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if target_slot < BUILTIN_SLOT_COUNT {
            return Err("Cannot insert before a built-in effect slot".to_string());
        }
        let mut entries = self.custom_effect_entries(track);
        if entries.len() >= MAX_CUSTOM_FX {
            return Err("No free effect slots available".to_string());
        }
        let old_host = self.fx_chain_host(FxChainLocator::Track(track))?;
        let target_offset = target_slot.saturating_sub(BUILTIN_SLOT_COUNT);
        let insert_offset = target_offset.min(entries.len());
        entries.insert(
            insert_offset,
            CustomEffectEntry {
                desc: EffectDescriptor::empty_custom_slot(),
                snapshot: EffectSlotSnapshot::new_empty(),
            },
        );
        self.write_custom_effect_entries(track, &entries);
        let new_host = self.fx_chain_host(FxChainLocator::Track(track))?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        let slot_idx = BUILTIN_SLOT_COUNT + insert_offset;
        self.insert_empty_track_effect_lease_slot(track, slot_idx)?;
        self.sync_other_pattern_effect_insert(track, slot_idx);
        Ok(slot_idx)
    }

    pub fn insert_builtin_effect_before_slot_sync(
        &mut self,
        track: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let slot_idx = self.prepare_custom_effect_insert_slot(track, target_slot)?;
        self.load_builtin_effect_to_slot_sync(track, slot_idx, name)?;
        Ok(slot_idx)
    }

    pub fn insert_saved_effect_before_slot_sync(
        &mut self,
        track: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        let source = self.retained_effect_source_for_name(name)?;
        let result = self.compile_saved_effect(name)?;
        let slot_idx = self.prepare_custom_effect_insert_slot(track, target_slot)?;
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
        self.retain_effect_source(FxChainLocator::Track(track), slot_idx, source)?;
        Ok(slot_idx)
    }

    pub fn move_effect_slot_sync(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: Option<usize>,
    ) -> Result<usize, String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if source_slot < BUILTIN_SLOT_COUNT {
            return Err("Cannot move a built-in effect slot".to_string());
        }
        let source_offset = source_slot - BUILTIN_SLOT_COUNT;
        let mut entries = self.custom_effect_entries(track);
        if source_offset >= entries.len() {
            return Err("Invalid source effect slot".to_string());
        }
        let entry = entries.remove(source_offset);
        let mut target_offset = target_slot
            .map(|slot| slot.saturating_sub(BUILTIN_SLOT_COUNT))
            .unwrap_or(entries.len());
        if let Some(slot) = target_slot {
            if slot < BUILTIN_SLOT_COUNT {
                return Err("Cannot move before a built-in effect slot".to_string());
            }
            if source_offset < target_offset {
                target_offset = target_offset.saturating_sub(1);
            }
        }
        target_offset = target_offset.min(entries.len());
        if target_offset == source_offset {
            entries.insert(source_offset, entry);
            return Ok(source_slot);
        }
        let old_host = self.fx_chain_host(FxChainLocator::Track(track))?;
        entries.insert(target_offset, entry);
        self.write_custom_effect_entries(track, &entries);
        let new_host = self.fx_chain_host(FxChainLocator::Track(track))?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        let slot_idx = BUILTIN_SLOT_COUNT + target_offset;
        self.move_track_effect_lease_slot(track, source_slot, slot_idx)?;
        self.sync_other_pattern_effect_move(track, source_slot, slot_idx);
        self.publish_effect_reorder();
        Ok(slot_idx)
    }

    pub fn insert_midi_fx_before_slot_sync(
        &mut self,
        track: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let desc = lisp_host::load_midi_fx_descriptor(name)
            .ok_or_else(|| format!("Unknown MIDI FX '{name}'"))?;
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        let old_len = chain.len();
        if chain.len() >= MAX_MIDI_FX_SLOTS {
            return Err("No free MIDI FX slots available".to_string());
        }
        let slot_idx = target_slot.min(chain.len());
        chain.insert(slot_idx, desc.name.clone());
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);
        let slots = &self.state.pattern.midi_fx_slots[track];
        for idx in (slot_idx + 1..slots.len()).rev() {
            slots[idx].copy_from(&slots[idx - 1]);
        }
        slots[slot_idx].apply_descriptor(&desc, 0);
        self.sync_other_pattern_midi_fx_insert(track, slot_idx, desc.name.clone(), &desc);
        self.device_registry
            .insert_midi_effect_identity(track_id, slot_idx, old_len)?;
        self.publish_effect_reorder();
        Ok(slot_idx)
    }

    pub fn move_midi_fx_slot_sync(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: Option<usize>,
    ) -> Result<usize, String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        if source_slot >= chain.len() {
            return Err("Invalid source MIDI FX slot".to_string());
        }
        let chain_len = chain.len();
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let name = chain.remove(source_slot);
        let source_snapshot =
            EffectSlotSnapshot::capture(&self.state.pattern.midi_fx_slots[track][source_slot]);
        let slots = &self.state.pattern.midi_fx_slots[track];
        for idx in source_slot..chain.len() {
            slots[idx].copy_from(&slots[idx + 1]);
        }
        if let Some(last_slot) = slots.last() {
            last_slot.clear();
        }
        let mut target_idx = target_slot.unwrap_or(chain.len()).min(chain.len());
        if let Some(slot) = target_slot {
            if source_slot < slot {
                target_idx = target_idx.saturating_sub(1);
            }
        }
        chain.insert(target_idx, name);
        for idx in (target_idx + 1..=chain.len()).rev() {
            if idx < slots.len() {
                slots[idx].copy_from(&slots[idx - 1]);
            }
        }
        source_snapshot.restore(&slots[target_idx]);
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);
        self.sync_other_pattern_midi_fx_move(track, source_slot, target_idx);
        self.device_registry
            .move_midi_effect_identity(track_id, source_slot, target_idx, chain_len)?;
        self.publish_effect_reorder();
        Ok(target_idx)
    }

    fn resolve_custom_slot_wiring(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> Result<(usize, i32, usize, i32, usize, Option<i32>), String> {
        self.resolve_fx_slot(FxChainLocator::Track(track), slot_idx)
    }

    pub(super) fn create_builtin_effect_node(
        &self,
        slot_id: usize,
        desc: &EffectDescriptor,
    ) -> Result<i32, String> {
        let (vtable, state_size) = match desc.name.as_str() {
            "Filter" => (
                crate::effects::filter::filter_vtable(),
                crate::effects::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "EQ8" => (
                crate::effects::eq8::eq8_vtable(),
                crate::effects::eq8::EQ8_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Delay" => (
                crate::effects::delay::delay_vtable(),
                crate::effects::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Str8 Delay" => (
                crate::effects::str8_delay::str8_delay_vtable(),
                crate::effects::str8_delay::STR8_DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Space Echo" => (
                crate::effects::space_echo::space_echo_vtable(),
                crate::effects::space_echo::SPACE_ECHO_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Dimension" => (
                crate::effects::dimension::dimension_vtable(),
                crate::effects::dimension::DIMENSION_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Phaser-Flanger" => (
                crate::effects::phaser_flanger::phaser_flanger_vtable(),
                crate::effects::phaser_flanger::PHASER_FLANGER_STATE_SIZE
                    * std::mem::size_of::<f32>(),
            ),
            "Roar" => (
                crate::effects::roar::roar_vtable(),
                crate::effects::roar::ROAR_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "DJ Mixer" => (
                crate::effects::dj_mixer::dj_mixer_vtable(),
                crate::effects::dj_mixer::DJ_MIXER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Reverb" => (
                crate::effects::reverb::reverb_vtable(),
                crate::effects::reverb::REVERB_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Multiverb" => (
                crate::effects::multiverb::multiverb_vtable(),
                crate::effects::multiverb::MULTIVERB_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "444 Compressor" | "Glue Compressor" => (
                crate::effects::dynamics::dynamics_vtable(),
                crate::effects::dynamics::DYNAMICS_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Compressor" => (
                crate::effects::compressor::compressor_vtable(),
                crate::effects::compressor::COMPRESSOR_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "OTT" => (
                crate::effects::ott::ott_vtable(),
                crate::effects::ott::OTT_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Limiter" => (
                crate::effects::limiter::limiter_vtable(),
                crate::effects::limiter::LIMITER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Tape" => (
                crate::effects::tape::tape_vtable(),
                crate::effects::tape::TAPE_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Filterbank" => (
                crate::effects::filterbank::filterbank_vtable(),
                crate::effects::filterbank::FILTERBANK_STATE_SIZE * std::mem::size_of::<f32>(),
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

    pub(super) fn create_effect_modulator_node(
        &self,
        name: &str,
        slot_id: usize,
    ) -> Result<i32, String> {
        let mod_name = CString::new(format!("{}_{}_mod", name.to_lowercase(), slot_id)).unwrap();
        let mod_id = unsafe {
            crate::audiograph::add_node(
                self.graph.lg.0,
                crate::instruments::voice_modulator::effect_modulator_vtable(),
                crate::instruments::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                mod_name.as_ptr(),
                crate::instruments::voice_modulator::INPUT_COUNT as i32,
                crate::instruments::voice_modulator::NUM_OUTPUTS as i32,
                std::ptr::null(),
                0,
            )
        };
        if mod_id < 0 {
            Err(format!("Failed to create effect modulator for '{name}'"))
        } else {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::instruments::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: self.state.transport.bpm.load(Ordering::Relaxed) as f32,
                    },
                );
            }
            Ok(mod_id)
        }
    }

    pub(super) unsafe fn connect_effect_modulator_for_descriptor(
        &self,
        modulator_node_id: i32,
        effect_node_id: i32,
        desc: &EffectDescriptor,
        ext_mod_input_nodes: Option<&[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]>,
    ) -> Result<(), String> {
        if let Some(ext_nodes) = ext_mod_input_nodes {
            for (input, &ext_node) in ext_nodes.iter().enumerate() {
                if !crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    ext_node,
                    0,
                    modulator_node_id,
                    (4 + input) as i32,
                ) {
                    return Err(format!(
                        "Failed to connect Ext {} to effect modulator",
                        input + 1
                    ));
                }
            }
        }

        let mut slots = desc
            .instrument_modulation_targets
            .iter()
            .map(|target| target.modulator_slot)
            .collect::<Vec<_>>();
        slots.sort_unstable();
        slots.dedup();
        for slot in slots {
            if !(1..=crate::instruments::voice_modulator::SLOT_COUNT).contains(&slot) {
                continue;
            }
            if !crate::audiograph::graph_connect(
                self.graph.lg.0,
                modulator_node_id,
                (slot - 1) as i32,
                effect_node_id,
                (2 + slot - 1) as i32,
            ) {
                return Err(format!(
                    "Failed to connect effect Mod {} output to effect input",
                    slot
                ));
            }
        }
        Ok(())
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
                let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
                if modulator_node_id != 0 {
                    unsafe {
                        crate::audiograph::params_push_wrapper(
                            self.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                logical_id: modulator_node_id as u64,
                                idx: crate::instruments::voice_modulator::PARAM_BPM as u64,
                                fvalue: bpm,
                            },
                        );
                    }
                }
                if node_id != 0 {
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::effects::delay::DELAY_PARAM_BPM,
                        "Str8 Delay" => crate::effects::str8_delay::STR8_DELAY_PARAM_BPM,
                        "Space Echo" => crate::effects::space_echo::SPACE_ECHO_PARAM_BPM,
                        "Phaser-Flanger" => {
                            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_BPM
                        }
                        "Roar" => crate::effects::roar::ROAR_PARAM_BPM,
                        "Filter" => crate::effects::filter::FILTER_PARAM_BPM,
                        "DJ Mixer" => crate::effects::dj_mixer::DJ_MIXER_PARAM_BPM,
                        "Filterbank" => crate::effects::filterbank::FILTERBANK_PARAM_BPM,
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
                    if slot.modulator_node_id != 0 {
                        unsafe {
                            crate::audiograph::params_push_wrapper(
                                self.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    logical_id: slot.modulator_node_id as u64,
                                    idx: crate::instruments::voice_modulator::PARAM_BPM as u64,
                                    fvalue: bpm,
                                },
                            );
                        }
                    }
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::effects::delay::DELAY_PARAM_BPM,
                        "Str8 Delay" => crate::effects::str8_delay::STR8_DELAY_PARAM_BPM,
                        "Space Echo" => crate::effects::space_echo::SPACE_ECHO_PARAM_BPM,
                        "Phaser-Flanger" => {
                            crate::effects::phaser_flanger::PHASER_FLANGER_PARAM_BPM
                        }
                        "Roar" => crate::effects::roar::ROAR_PARAM_BPM,
                        "Filter" => crate::effects::filter::FILTER_PARAM_BPM,
                        "DJ Mixer" => crate::effects::dj_mixer::DJ_MIXER_PARAM_BPM,
                        "Filterbank" => crate::effects::filterbank::FILTERBANK_PARAM_BPM,
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
        self.apply_builtin_effect_to_slot_with_modulator(track, slot_idx, node_id, None, desc);
    }

    fn apply_builtin_effect_to_slot_with_modulator(
        &mut self,
        track: usize,
        slot_idx: usize,
        node_id: i32,
        modulator_node_id: Option<i32>,
        desc: EffectDescriptor,
    ) {
        self.graph.effect_descriptors[track][slot_idx] = desc.clone();
        self.state.pattern.effect_chains[track][slot_idx].apply_descriptor_with_modulator(
            &desc,
            node_id as u32,
            modulator_node_id.unwrap_or(0) as u32,
        );

        self.state
            .sync_effect_slot_with_modulator_in_track_patterns(
                track,
                slot_idx,
                &desc,
                node_id as u32,
                modulator_node_id.unwrap_or(0) as u32,
            );
        self.sync_scratch_runtime_descriptors();
    }

    pub(super) fn load_builtin_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        // Some builtins own host-managed assets but use DGenLisp for DSP.
        if crate::effects::dgen_builtin::contains(name) {
            return self.load_dgen_builtin_to_slot_sync(track, slot_idx, name);
        }
        let mut desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        patch_sidechain_labels(&mut desc, &self.effect_sidechain_labels(track));
        let (node_id, modulator_node_id) =
            self.install_builtin_fx_node(FxChainLocator::Track(track), slot_idx, &desc)?;
        self.apply_builtin_effect_to_slot_with_modulator(
            track,
            slot_idx,
            node_id,
            modulator_node_id,
            desc,
        );
        self.retain_effect_source(
            FxChainLocator::Track(track),
            slot_idx,
            RetainedEffectSource::NativeBuiltin { name: name.to_string() },
        )?;
        self.push_track_effect_slot_defaults(track, slot_idx);
        self.push_all_delay_bpm();
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        self.ui.effect_param_cursor = 0;
        self.ui.effect_scroll_offset = 0;
        Ok(())
    }

    /// Compile and install a host-integrated DGenLisp builtin on a track.
    pub(super) fn load_dgen_builtin_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let builtin = crate::effects::dgen_builtin::find(name)
            .ok_or_else(|| format!("Unknown dgenlisp builtin '{name}'"))?;
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            builtin.origin,
            builtin.source,
            self.graph.sample_rate,
            None,
        )?;
        let ir_slots = crate::effects::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
        let table_slot = crate::effects::filter_table::TableSlot::from_manifest(&result.manifest);
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
        self.retain_effect_source(
            FxChainLocator::Track(track),
            slot_idx,
            RetainedEffectSource::Compiled {
                name: name.to_string(),
                source: builtin.source.to_string(),
                asset_base: None,
                origin: builtin.origin,
            },
        )?;
        let node_id = self.state.pattern.effect_chains[track][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        self.initialize_dgen_builtin_node(name, node_id, ir_slots, table_slot)
    }

    fn initialize_dgen_builtin_node(
        &mut self,
        name: &str,
        node_id: i32,
        ir_slots: Option<crate::effects::conv_reverb::StereoIrSlots>,
        table_slot: Option<crate::effects::filter_table::TableSlot>,
    ) -> Result<(), String> {
        if name == crate::effects::conv_reverb::NAME {
            let slots = ir_slots
                .ok_or_else(|| format!("'{name}' compiled without the expected IR tensors"))?;
            crate::effects::conv_reverb::record_ir_slots(node_id, slots);
            if let Some(path) = crate::effects::conv_reverb::default_ir_path() {
                if let Err(error) = self.apply_conv_reverb_ir_to_node(
                    node_id,
                    &path,
                    crate::effects::conv_reverb::DEFAULT_IR_REF,
                ) {
                    self.editor.status_message = Some((
                        format!("Convolution Reverb: default IR not loaded ({error})"),
                        Instant::now(),
                    ));
                }
            }
        } else if name == crate::effects::filter_table::NAME {
            let slot = table_slot
                .ok_or_else(|| format!("'{name}' compiled without table_magnitudes"))?;
            crate::effects::filter_table::record_slot(node_id, slot);
            self.apply_prepared_filter_table_to_node(
                node_id,
                Arc::new(crate::effects::filter_table::default_table()),
                crate::effects::filter_table::DEFAULT_TABLE_REF,
                std::path::Path::new("Procedural Shapes"),
            )?;
        } else {
            return Err(format!("Unknown dgenlisp builtin '{name}'"));
        }
        Ok(())
    }

    /// Load an impulse response into a live Convolution Reverb instance on a
    /// track slot. `abs_path` is the WAV to load; `reference` is the persisted
    /// id (sample hash/stem). Runs the IR prep (decode/resample/partition-FFT)
    /// synchronously — fine for a user action, never call from the audio thread.
    /// Core IR load shared by track and bus: prep the WAV and bulk-write it into
    /// the live node. Runs the IR prep (decode/resample/partition-FFT)
    /// synchronously — fine for a user action, never call from the audio thread.
    fn apply_conv_reverb_ir_to_node(
        &self,
        node_id: i32,
        abs_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        if node_id == 0 {
            return Err("Convolution Reverb node not live".to_string());
        }
        let slots = crate::effects::conv_reverb::ir_slots_for(node_id)
            .ok_or_else(|| "slot is not a Convolution Reverb".to_string())?;
        let ir = Arc::new(crate::effects::conv_reverb::prepare_ir(
            abs_path,
            self.graph.sample_rate,
        )?);
        self.apply_prepared_conv_reverb_ir_to_node(node_id, ir, reference, abs_path)
    }

    pub(crate) fn apply_prepared_conv_reverb_ir_to_node(
        &self,
        node_id: i32,
        ir: Arc<crate::effects::conv_reverb::StereoIr>,
        reference: &str,
        source_path: &std::path::Path,
    ) -> Result<(), String> {
        if node_id == 0 {
            return Err("Convolution Reverb node not live".to_string());
        }
        let slots = crate::effects::conv_reverb::ir_slots_for(node_id)
            .ok_or_else(|| "slot is not a Convolution Reverb".to_string())?;
        unsafe {
            crate::effects::conv_reverb::apply_ir_to_node(
                self.graph.lg.0,
                node_id,
                &slots,
                ir.as_ref(),
            )?;
        }
        // Friendly label: the bundled default has a fixed title; user samples
        // resolve their display title from the DB, falling back to the stem.
        let display = if reference == crate::effects::conv_reverb::DEFAULT_IR_REF {
            "Lexicon 300 Rich Plate".to_string()
        } else {
            crate::sample_db::display_title_for_sample_path(source_path)
                .unwrap_or_else(|| reference.to_string())
        };
        crate::effects::conv_reverb::record_prepared_ir(node_id, reference, &display, ir);
        Ok(())
    }

    pub(crate) fn restore_prepared_track_effect_ir(
        &self,
        track: usize,
        slot_idx: usize,
        reference: &str,
        ir: Arc<crate::effects::conv_reverb::StereoIr>,
    ) -> Result<(), String> {
        let node_id = self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
            .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
            .ok_or_else(|| "Track effect slot not found".to_string())?;
        self.apply_prepared_conv_reverb_ir_to_node(
            node_id,
            ir,
            reference,
            std::path::Path::new(reference),
        )
    }

    pub(crate) fn restore_prepared_rack_effect_ir(
        &self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        reference: &str,
        ir: Arc<crate::effects::conv_reverb::StereoIr>,
    ) -> Result<(), String> {
        let node_id = self
            .rack_slot_effect_snapshot(track, rack_slot)?
            .effect_slots
            .get(effect_slot)
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Rack effect slot not found".to_string())?;
        self.apply_prepared_conv_reverb_ir_to_node(
            node_id,
            ir,
            reference,
            std::path::Path::new(reference),
        )
    }

    pub(crate) fn restore_prepared_bus_effect_ir(
        &self,
        bus_idx: usize,
        effect_slot: usize,
        reference: &str,
        ir: Arc<crate::effects::conv_reverb::StereoIr>,
    ) -> Result<(), String> {
        let node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(effect_slot))
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Bus effect slot not found".to_string())?;
        self.apply_prepared_conv_reverb_ir_to_node(
            node_id,
            ir,
            reference,
            std::path::Path::new(reference),
        )
    }

    /// Load an impulse response into a live Convolution Reverb on a track slot.
    pub fn set_conv_reverb_ir(
        &mut self,
        track: usize,
        slot_idx: usize,
        abs_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        let node_id = self.state.pattern.effect_chains[track][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        self.apply_conv_reverb_ir_to_node(node_id, abs_path, reference)
    }

    /// Load an impulse response into a live Convolution Reverb on a bus slot.
    pub fn set_conv_reverb_ir_bus(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        abs_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        let node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| format!("Bus {} effect slot {} not found", bus_idx + 1, slot_idx + 1))?;
        self.apply_conv_reverb_ir_to_node(node_id, abs_path, reference)
    }

    pub fn set_conv_reverb_ir_rack_slot(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        abs_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        let node_id = self
            .rack_slot_effect_snapshot(track, rack_slot)?
            .effect_slots
            .get(effect_slot)
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Rack-slot effect not found".to_string())?;
        self.apply_conv_reverb_ir_to_node(node_id, abs_path, reference)
    }

    /// Analyze and install a table source. `reference` may carry an explicit
    /// analysis mode (`encode_table_ref`); otherwise the recommended mode is
    /// used. Either way the stored reference records the mode actually used so
    /// save/reload reproduces the identical analysis.
    fn apply_filter_table_to_node(
        &self,
        node_id: i32,
        source_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        use crate::effects::{filter_table, filter_table_asset};
        // A baked .fltab asset skips analysis entirely: the file already
        // carries the validated magnitude bank, and the stored reference
        // (`fltab:<stem>`) resolves back to the asset on reload.
        if filter_table_asset::is_asset_path(source_path)
            || filter_table_asset::decode_asset_ref(reference).is_some()
        {
            let asset = filter_table_asset::read_asset(source_path)?;
            let stem = source_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| "asset path has no file stem".to_string())?;
            let stored = filter_table_asset::encode_asset_ref(stem);
            return self.apply_prepared_filter_table_to_node(
                node_id,
                Arc::new(asset.table),
                &stored,
                source_path,
            );
        }
        let (sample_ref, requested) = filter_table::decode_table_ref(reference);
        let (table, mode) = match requested {
            Some(mode) => (
                filter_table::prepare_table_with_mode(source_path, mode)?,
                mode,
            ),
            None => filter_table::prepare_table(source_path)?,
        };
        let stored = filter_table::encode_table_ref(sample_ref, mode);
        self.apply_prepared_filter_table_to_node(node_id, Arc::new(table), &stored, source_path)
    }

    pub(crate) fn apply_prepared_filter_table_to_node(
        &self,
        node_id: i32,
        table: Arc<crate::effects::filter_table::MagnitudeTable>,
        reference: &str,
        source_path: &std::path::Path,
    ) -> Result<(), String> {
        if node_id == 0 {
            return Err("Filter Table node not live".to_string());
        }
        unsafe {
            crate::effects::filter_table::apply_table_to_node(
                self.graph.lg.0,
                node_id,
                table.as_ref(),
            )?;
        }
        // Snapshot/persisted references may carry an `#ft-engine=` suffix;
        // registries always hold the bare form (engine reconciliation is the
        // chain/compile layer's job, not the table upload's).
        let (reference, _engine) = crate::effects::filter_table::split_engine_ref(reference);
        let (sample_ref, _mode) = crate::effects::filter_table::decode_table_ref(reference);
        let display = if sample_ref == crate::effects::filter_table::DEFAULT_TABLE_REF {
            "Procedural Shapes".to_string()
        } else if let Some(stem) = crate::effects::filter_table_asset::decode_asset_ref(sample_ref)
        {
            stem.to_string()
        } else {
            crate::sample_db::display_title_for_sample_path(source_path)
                .unwrap_or_else(|| sample_ref.to_string())
        };
        crate::effects::filter_table::record_prepared_table(
            node_id,
            reference,
            &display,
            table,
        );
        Ok(())
    }

    /// Apply a persisted table reference to a node, stripping the
    /// `#ft-engine=` tag first.
    ///
    /// The engine is a property of the compiled node, not of the table, so it
    /// must not reach the table registries. Left on, it also corrupts the
    /// decode: `decode_table_ref` looks for the analysis-mode suffix with
    /// `rfind`, so on `kick#ft-mode=wavetable#ft-engine=causal` it matches the
    /// engine tag, fails to parse it as a mode, and hands back the whole
    /// string as the source path.
    fn apply_restored_filter_table_to_node(
        &self,
        node_id: i32,
        reference: &str,
        table: Arc<crate::effects::filter_table::MagnitudeTable>,
    ) -> Result<(), String> {
        let (bare, _engine) = crate::effects::filter_table::split_engine_ref(reference);
        self.apply_prepared_filter_table_to_node(
            node_id,
            table,
            bare,
            std::path::Path::new(crate::effects::filter_table::decode_table_ref(bare).0),
        )
    }

    pub(crate) fn restore_prepared_track_filter_table(
        &self,
        track: usize,
        slot_idx: usize,
        reference: &str,
        table: Arc<crate::effects::filter_table::MagnitudeTable>,
    ) -> Result<(), String> {
        let node_id = self.state.pattern.effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
            .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
            .ok_or_else(|| "Track effect slot not found".to_string())?;
        self.apply_restored_filter_table_to_node(node_id, reference, table)
    }

    pub(crate) fn restore_prepared_bus_filter_table(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        reference: &str,
        table: Arc<crate::effects::filter_table::MagnitudeTable>,
    ) -> Result<(), String> {
        let node_id = self.buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Bus effect slot not found".to_string())?;
        self.apply_restored_filter_table_to_node(node_id, reference, table)
    }

    pub(crate) fn restore_prepared_rack_filter_table(
        &self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        reference: &str,
        table: Arc<crate::effects::filter_table::MagnitudeTable>,
    ) -> Result<(), String> {
        let node_id = self.rack_slot_effect_snapshot(track, rack_slot)?
            .effect_slots
            .get(effect_slot)
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Rack effect slot not found".to_string())?;
        self.apply_restored_filter_table_to_node(node_id, reference, table)
    }

    /// Recompile a live track Filter Table slot for a different engine,
    /// preserving authoring values (params, p-locks, key locks) and the loaded
    /// table. Returns `false` without touching the graph when the node already
    /// runs the requested engine. Callers wrap this in a recorded chain
    /// mutation so undo/redo rebuild from the retained per-engine source.
    pub fn set_track_filter_table_engine(
        &mut self,
        track: usize,
        slot_idx: usize,
        engine: crate::effects::filter_table::TableEngine,
    ) -> Result<bool, String> {
        use crate::effects::filter_table;
        let slot_state = self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
            .ok_or_else(|| "Track effect slot not found".to_string())?;
        let old_node_id = slot_state.node_id.load(Ordering::Relaxed) as i32;
        if old_node_id == 0 {
            return Err("Filter Table node not live".to_string());
        }
        if filter_table::engine_for(old_node_id) == engine {
            return Ok(false);
        }
        let values = EffectSlotSnapshot::capture_authoring_values(slot_state);
        let session = self.detach_filter_table_editor_session(old_node_id);
        let source = filter_table::dsp_source_for(engine);
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            lisp_host::DGenSourceOrigin::BuiltinFilterTable,
            source,
            self.graph.sample_rate,
            None,
        )?;
        let manifest = result.manifest.clone();
        self.apply_compiled_effect_to_slot_sync(result, filter_table::NAME, slot_idx, track)?;
        self.retain_effect_source(
            FxChainLocator::Track(track),
            slot_idx,
            RetainedEffectSource::Compiled {
                name: filter_table::NAME.to_string(),
                source: source.to_string(),
                asset_base: None,
                origin: lisp_host::DGenSourceOrigin::BuiltinFilterTable,
            },
        )?;
        let node_id = self.state.pattern.effect_chains[track][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        filter_table::record_compiled_instance(node_id, &manifest, source);
        let slot_state = &self.state.pattern.effect_chains[track][slot_idx];
        EffectSlotSnapshot::restore_authoring_values(slot_state, &values)?;
        self.push_track_effect_slot_defaults(track, slot_idx);
        self.reapply_filter_table_after_engine_change(node_id, &values)?;
        self.reattach_filter_table_editor_session(session, node_id)?;
        Ok(true)
    }

    /// Bus twin of [`Self::set_track_filter_table_engine`].
    pub fn set_bus_filter_table_engine(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        engine: crate::effects::filter_table::TableEngine,
    ) -> Result<bool, String> {
        use crate::effects::filter_table;
        let old_node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| "Bus effect slot not found".to_string())?;
        if old_node_id == 0 {
            return Err("Filter Table node not live".to_string());
        }
        if filter_table::engine_for(old_node_id) == engine {
            return Ok(false);
        }
        let values = self.buses[bus_idx].effect_slots[slot_idx].authoring_values();
        let session = self.detach_filter_table_editor_session(old_node_id);
        let locator = self.bus_fx_locator(bus_idx)?;
        let source = filter_table::dsp_source_for(engine);
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            lisp_host::DGenSourceOrigin::BuiltinFilterTable,
            source,
            self.graph.sample_rate,
            None,
        )?;
        let manifest = result.manifest.clone();
        self.apply_compiled_bus_effect_to_slot_sync(
            bus_idx,
            slot_idx,
            filter_table::NAME,
            result,
        )?;
        self.retain_effect_source(
            locator,
            slot_idx,
            RetainedEffectSource::Compiled {
                name: filter_table::NAME.to_string(),
                source: source.to_string(),
                asset_base: None,
                origin: lisp_host::DGenSourceOrigin::BuiltinFilterTable,
            },
        )?;
        let node_id = self.buses[bus_idx].effect_slots[slot_idx].node_id as i32;
        filter_table::record_compiled_instance(node_id, &manifest, source);
        self.buses[bus_idx].effect_slots[slot_idx]
            .apply_authoring_values(&values)
            .map_err(|error| format!("reapplying bus effect values: {error}"))?;
        self.push_bus_effect_slot_defaults(bus_idx, slot_idx);
        self.reapply_filter_table_after_engine_change(node_id, &values)?;
        self.reattach_filter_table_editor_session(session, node_id)?;
        Ok(true)
    }

    /// After an engine swap replaced the node, re-upload the table the slot
    /// was using (or re-seed the procedural default for untouched slots).
    fn reapply_filter_table_after_engine_change(
        &mut self,
        node_id: i32,
        values: &crate::effects::EffectSlotValuesSnapshot,
    ) -> Result<(), String> {
        if let (Some(reference), Some(table)) = (&values.table, &values.prepared_table) {
            self.apply_restored_filter_table_to_node(node_id, reference, table.clone())
        } else {
            self.apply_prepared_filter_table_to_node(
                node_id,
                Arc::new(crate::effects::filter_table::default_table()),
                crate::effects::filter_table::DEFAULT_TABLE_REF,
                std::path::Path::new("Procedural Shapes"),
            )
        }
    }

    /// Current Filter Table source for a track or bus effect slot: the decoded
    /// sample name and the analysis mode recorded at import. `None` when the
    /// slot has no user-loaded table (the procedural default included).
    pub fn filter_table_source_info(
        &self,
        track: Option<usize>,
        bus: Option<usize>,
        slot_idx: usize,
    ) -> Option<(String, Option<crate::effects::filter_table::AnalysisMode>)> {
        let node_id = if let Some(bus_idx) = bus {
            self.buses.get(bus_idx)?.effect_slots.get(slot_idx)?.node_id as i32
        } else {
            self.state
                .pattern
                .effect_chains
                .get(track?)?
                .get(slot_idx)?
                .node_id
                .load(Ordering::Relaxed) as i32
        };
        let reference = crate::effects::filter_table::table_ref_for(node_id)?;
        let (sample_ref, mode) = crate::effects::filter_table::decode_table_ref(&reference);
        if sample_ref == crate::effects::filter_table::DEFAULT_TABLE_REF {
            return None;
        }
        Some((sample_ref.to_string(), mode))
    }

    pub fn set_filter_table_source(
        &mut self,
        track: usize,
        slot_idx: usize,
        source_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        let node_id = self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
            .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
            .ok_or_else(|| "Track effect slot not found".to_string())?;
        self.apply_filter_table_to_node(node_id, source_path, reference)
    }

    pub fn set_filter_table_source_bus(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        source_path: &std::path::Path,
        reference: &str,
    ) -> Result<(), String> {
        let node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .ok_or_else(|| format!("Bus {} effect slot {} not found", bus_idx + 1, slot_idx + 1))?;
        self.apply_filter_table_to_node(node_id, source_path, reference)
    }

    // ---- Filter Table response editor sessions (eseq-dtx.8) ----------
    //
    // The document/command model lives in effects::filter_table_editor;
    // these methods bind the single active session to a live device node.
    // Previews write the baked table straight to the node tensor and the
    // published visualization bank WITHOUT touching the prepared-table
    // registries, so app-level undo/persistence keep seeing the table the
    // device actually owns until the user saves (recorded mutation) or the
    // session closes (original re-applied).

    fn filter_table_editor_target_node(
        &self,
        target: crate::effects::filter_table_editor::EditorTarget,
    ) -> Result<i32, String> {
        use crate::effects::filter_table_editor::EditorTarget;
        let node_id = match target {
            EditorTarget::Track { track, slot } => self
                .state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot))
                .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32),
            EditorTarget::Bus { bus, slot } => self
                .buses
                .get(bus)
                .and_then(|bus| bus.effect_slots.get(slot))
                .map(|slot| slot.node_id as i32),
        }
        .ok_or_else(|| "Filter Table effect slot not found".to_string())?;
        if node_id == 0 {
            return Err("Filter Table node not live".to_string());
        }
        Ok(node_id)
    }

    /// Open an editor session on a track/bus Filter Table slot. When the
    /// device's current table is a saved editor asset, its nondestructive
    /// document (base + ops) is restored; otherwise the loaded table
    /// becomes the base of a fresh document.
    pub fn open_filter_table_editor(
        &mut self,
        target: crate::effects::filter_table_editor::EditorTarget,
    ) -> Result<(), String> {
        use crate::effects::{filter_table, filter_table_asset, filter_table_editor};
        let node_id = self.filter_table_editor_target_node(target)?;
        let original_table = filter_table::prepared_table_for(node_id)
            .ok_or_else(|| "Filter Table has no prepared table yet".to_string())?;
        let original_ref = filter_table::table_ref_for(node_id)
            .unwrap_or_else(|| filter_table::DEFAULT_TABLE_REF.to_string());
        let original_name =
            filter_table::table_name_for(node_id).unwrap_or_else(|| "table".to_string());
        let doc = filter_table_asset::decode_asset_ref(&original_ref)
            .and_then(filter_table_asset::resolve_asset_path)
            .and_then(|path| filter_table_asset::read_asset(&path).ok())
            .and_then(|asset| asset.meta.recipe)
            .and_then(|recipe| filter_table_editor::EditorDoc::from_snapshot(&recipe).ok())
            .unwrap_or_else(|| filter_table_editor::EditorDoc::from_table(&original_table));
        let replaced = filter_table_editor::set_session(Some(filter_table_editor::EditorSession {
            target,
            node_id,
            doc,
            selected_frame: 0,
            original_table,
            original_ref,
            original_name,
            dirty: false,
        }));
        // Only one session can be live, so a session left open on another
        // device must be rolled back — otherwise that device keeps auditioning
        // edits forever while save/snapshot persist its original table.
        if let Some(previous) = replaced.filter(|previous| previous.dirty) {
            self.rollback_filter_table_editor_session(&previous)?;
        }
        Ok(())
    }

    /// Write a baked editor table to the session's node for live audition:
    /// tensor + published bank only, never the prepared-table registries.
    fn preview_filter_table_editor_table(
        &self,
        node_id: i32,
        table: &crate::effects::filter_table::MagnitudeTable,
    ) -> Result<(), String> {
        unsafe {
            crate::effects::filter_table::apply_table_to_node(self.graph.lg.0, node_id, table)?;
        }
        eseqlisp::widget_render::wavetable_viewer::publish_bank(
            crate::effects::filter_table::visualization_key(node_id),
            crate::effects::filter_table::NBINS,
            table.data.clone(),
        );
        Ok(())
    }

    /// Apply an op to the active session (optionally coalescing with the
    /// newest history entry, for drag gestures) and audition the result.
    pub fn filter_table_editor_apply_op(
        &mut self,
        op: crate::effects::filter_table_editor::EditOp,
        coalesce: bool,
    ) -> Result<(), String> {
        use crate::effects::filter_table_editor::with_session;
        let (node_id, baked) = with_session(|session| {
            let session = session.ok_or_else(|| "no Filter Table editor open".to_string())?;
            if coalesce {
                session.doc.replace_last(op)?;
            } else {
                session.doc.apply(op)?;
            }
            session.dirty = true;
            session.selected_frame = session.selected_frame.min(session.doc.frame_count() - 1);
            Ok::<_, String>((session.node_id, session.doc.bake()?))
        })?;
        self.preview_filter_table_editor_table(node_id, &baked)
    }

    /// Audition an uncommitted op (mid-drag preview): nothing enters the
    /// document history. `replacing_last` must match the `coalesce` flag the
    /// gesture will commit with, so preview and commit bake the same result.
    pub fn filter_table_editor_preview_op(
        &mut self,
        op: crate::effects::filter_table_editor::EditOp,
        replacing_last: bool,
    ) -> Result<(), String> {
        use crate::effects::filter_table_editor::with_session;
        let (node_id, baked) = with_session(|session| {
            let session = session.ok_or_else(|| "no Filter Table editor open".to_string())?;
            Ok::<_, String>((
                session.node_id,
                session.doc.bake_with_preview(&op, replacing_last)?,
            ))
        })?;
        self.preview_filter_table_editor_table(node_id, &baked)
    }

    /// Editor-document undo/redo (independent of app history). Returns
    /// whether anything changed.
    pub fn filter_table_editor_history(&mut self, redo: bool) -> Result<bool, String> {
        use crate::effects::filter_table_editor::with_session;
        let stepped = with_session(|session| {
            let session = session.ok_or_else(|| "no Filter Table editor open".to_string())?;
            let stepped = if redo {
                session.doc.redo()
            } else {
                session.doc.undo()
            };
            if stepped {
                session.dirty = true;
                session.selected_frame =
                    session.selected_frame.min(session.doc.frame_count() - 1);
                Ok(Some((session.node_id, session.doc.bake()?)))
            } else {
                Ok::<_, String>(None)
            }
        })?;
        match stepped {
            Some((node_id, baked)) => {
                self.preview_filter_table_editor_table(node_id, &baked)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn filter_table_editor_select_frame(&mut self, frame: usize) -> Result<(), String> {
        crate::effects::filter_table_editor::with_session(|session| {
            let session = session.ok_or_else(|| "no Filter Table editor open".to_string())?;
            if frame >= session.doc.frame_count() {
                return Err(format!(
                    "frame {frame} is out of range for a {}-frame document",
                    session.doc.frame_count()
                ));
            }
            session.selected_frame = frame;
            Ok(())
        })
    }

    /// Save the session document as a user `.fltab` asset (baked payload +
    /// full nondestructive document in the recipe) and load it into the
    /// device through the recorded-mutation path so app-level undo/redo
    /// and persistence treat it like any other table load. Returns the
    /// asset stem.
    pub fn filter_table_editor_save(&mut self, name: Option<&str>) -> Result<String, String> {
        self.filter_table_editor_save_in(
            name,
            &crate::effects::filter_table_asset::user_asset_dir(),
        )
    }

    /// [`filter_table_editor_save`] with an explicit destination directory
    /// (tests save into scratch space instead of the user library).
    pub fn filter_table_editor_save_in(
        &mut self,
        name: Option<&str>,
        dir: &std::path::Path,
    ) -> Result<String, String> {
        use crate::effects::filter_table_editor::{with_session, EditorTarget};
        let (target, doc, baked, fallback_name) = with_session(|session| {
            let session = session.ok_or_else(|| "no Filter Table editor open".to_string())?;
            Ok::<_, String>((
                session.target,
                session.doc.clone(),
                session.doc.bake()?,
                session.original_name.clone(),
            ))
        })?;
        let stem = sanitize_asset_stem(name.unwrap_or(&fallback_name));
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("failed to create '{}': {error}", dir.display()))?;
        let path = dir.join(format!(
            "{stem}.{}",
            crate::effects::filter_table_asset::EXTENSION
        ));
        let meta = crate::effects::filter_table_editor::save_meta(&stem, &doc);
        crate::effects::filter_table_asset::write_asset(&path, &meta, &baked)?;
        match target {
            EditorTarget::Track { track, slot } => {
                crate::app::edit::apply_recorded_track_filter_table_mutation(
                    self, track, slot, &path, &stem,
                )
                .map(|_| ())
                .map_err(|error| format!("{error:?}"))?;
            }
            EditorTarget::Bus { bus, slot } => {
                self.apply_recorded_bus_effect_value_mutation(
                    bus,
                    slot,
                    "Save Filter Table edit",
                    "filter-table-source",
                    |app| app.set_filter_table_source_bus(bus, slot, &path, &stem),
                )?;
            }
        }
        with_session(|session| {
            if let Some(session) = session {
                session.dirty = false;
                // The saved asset is now the device's table; closing must
                // not roll back to the pre-edit original.
                session.original_ref =
                    crate::effects::filter_table_asset::encode_asset_ref(&stem);
                session.original_name = stem.clone();
                if let Some(table) =
                    crate::effects::filter_table::prepared_table_for(session.node_id)
                {
                    session.original_table = table;
                }
            }
        });
        Ok(stem)
    }

    /// Re-apply the table a session's device had when the editor opened,
    /// discarding whatever the live preview left on the node.
    fn rollback_filter_table_editor_session(
        &self,
        session: &crate::effects::filter_table_editor::EditorSession,
    ) -> Result<(), String> {
        self.apply_restored_filter_table_to_node(
            session.node_id,
            &session.original_ref,
            session.original_table.clone(),
        )
    }

    /// Close the session. Unsaved edits are rolled back by re-applying the
    /// table the device had when the editor opened.
    pub fn close_filter_table_editor(&mut self) -> Result<(), String> {
        let Some(session) = crate::effects::filter_table_editor::set_session(None) else {
            return Ok(());
        };
        if session.dirty {
            self.rollback_filter_table_editor_session(&session)?;
        }
        Ok(())
    }

    /// Lift an open editor session off a node that is about to be rebuilt.
    /// Tearing the old node down runs `clear_instance`, which abandons any
    /// session still bound to it, so a swap that means to keep the session
    /// must detach it first and [`Self::reattach_filter_table_editor_session`]
    /// it onto the replacement.
    fn detach_filter_table_editor_session(
        &self,
        node_id: i32,
    ) -> Option<crate::effects::filter_table_editor::EditorSession> {
        crate::effects::filter_table_editor::take_session_for_node(node_id)
    }

    /// Re-bind a detached session to the rebuilt node and re-audition its
    /// document, so an engine swap under the editor keeps unsaved edits and
    /// keeps the panel's editor section pointed at a live node.
    fn reattach_filter_table_editor_session(
        &mut self,
        session: Option<crate::effects::filter_table_editor::EditorSession>,
        new_node_id: i32,
    ) -> Result<(), String> {
        let Some(mut session) = session else {
            return Ok(());
        };
        session.node_id = new_node_id;
        // The rebuild re-applied the committed table; unsaved edits have to be
        // re-auditioned on top of it.
        let baked = if session.dirty {
            Some(session.doc.bake()?)
        } else {
            None
        };
        crate::effects::filter_table_editor::set_session(Some(session));
        match baked {
            Some(table) => self.preview_filter_table_editor_table(new_node_id, &table),
            None => Ok(()),
        }
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
        manifest: &lisp_host::DGenManifest,
    ) -> EffectDescriptor {
        let mut desc = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        desc.tensor_params = crate::effects::tensor_param_descriptors_from_manifest(
            &manifest.tensors,
            &manifest.tensor_init_data,
        );

        lisp_host::append_effect_host_modulation_controls(&mut desc, manifest);

        let sidechain_labels = self.effect_sidechain_labels(track);
        desc.params.extend(
            lisp_host::effect_sidechain_inputs(manifest)
                .into_iter()
                .map(|input| ParamDescriptor {
                    name: input.name,
                    min: 0.0,
                    max: sidechain_labels.len().saturating_sub(1) as f32,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: sidechain_labels.clone(),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: u32::MAX,
                    node_param_span: 1,
                    host_control: Some(HostControl::FxSidechain {
                        input_channel: input.input_channel,
                    }),
                    ui_metadata: None,
                }),
        );
        desc
    }

    fn build_bus_effect_descriptor(
        &self,
        name: &str,
        manifest: &lisp_host::DGenManifest,
    ) -> EffectDescriptor {
        let mut desc = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        desc.tensor_params = crate::effects::tensor_param_descriptors_from_manifest(
            &manifest.tensors,
            &manifest.tensor_init_data,
        );
        lisp_host::append_effect_host_modulation_controls(&mut desc, manifest);

        let sidechain_labels = self.bus_effect_sidechain_labels();
        desc.params.extend(
            lisp_host::effect_sidechain_inputs(manifest)
                .into_iter()
                .map(|input| ParamDescriptor {
                    name: input.name,
                    min: 0.0,
                    max: sidechain_labels.len().saturating_sub(1) as f32,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: sidechain_labels.clone(),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: u32::MAX,
                    node_param_span: 1,
                    host_control: Some(HostControl::FxSidechain {
                        input_channel: input.input_channel,
                    }),
                    ui_metadata: None,
                }),
        );
        desc
    }

    fn track_effect_ext_mod_input_nodes(
        &self,
        track: usize,
    ) -> Option<[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]> {
        self.graph
            .track_node_ids
            .get(track)
            .map(|nodes| nodes.mod_in_clip_ids)
    }

    fn current_effect_modulator_node(&self, track: usize, slot_idx: usize) -> Option<i32> {
        self.state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
            .map(|slot| slot.modulator_node_id.load(Ordering::Relaxed) as i32)
            .filter(|node_id| *node_id > 0)
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
                    self.graph.track_node_ids[old_track].pdc_id,
                    source_port,
                    node_id,
                    *input_channel as i32,
                )
            };
            if !disconnected {
                eprintln!(
                    "sidechain: disconnect failed effect_node={} track={} slot={} old_track={} src_port={} dst_port={}",
                    node_id, track, slot_idx, old_track, source_port, *input_channel as i32,
                );
            }
        }

        let mut ext_connected = false;
        if let Some(new_track) = self.effect_sidechain_source_track(track, selection) {
            let source_port = (*input_channel).min(1) as i32;
            let connected = unsafe {
                crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    self.graph.track_node_ids[new_track].pdc_id,
                    source_port,
                    node_id,
                    *input_channel as i32,
                )
            };
            if !connected {
                eprintln!(
                    "sidechain: connect failed effect_node={} track={} slot={} new_track={} src_port={} dst_port={}",
                    node_id, track, slot_idx, new_track, source_port, *input_channel as i32,
                );
            }
            ext_connected = connected;
        }

        // Tell effects with a normalled internal source (e.g. Filterbank
        // FM/AM) whether an external sidechain is now routed in. Keyed to the
        // actual connect result: on a failed connect the DSP must keep its
        // normalled source rather than read a silent, never-connected port.
        if let Some(active_idx) = sidechain_active_state_param(desc.name.as_str(), *input_channel)
        {
            let active = ext_connected;
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        logical_id: node_id as u64,
                        idx: active_idx,
                        fvalue: if active { 1.0 } else { 0.0 },
                    },
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
                        nodes.pdc_id,
                        source_port,
                        node_id,
                        *input_channel as i32,
                    );
                }
            }
        }

        let mut ext_connected = false;
        if let Some(new_track) = self.bus_effect_sidechain_source_track(selection) {
            if let Some(nodes) = self.graph.track_node_ids.get(new_track) {
                let source_port = (*input_channel).min(1) as i32;
                ext_connected = unsafe {
                    crate::audiograph::graph_connect(
                        self.graph.lg.0,
                        nodes.pdc_id,
                        source_port,
                        node_id,
                        *input_channel as i32,
                    )
                };
            }
        }

        // Tell effects with a normalled internal source (e.g. Filterbank
        // FM/AM) whether an external sidechain is now routed in. Keyed to the
        // actual connect result so a failed connect keeps the normalled
        // source instead of a silent, never-connected port.
        if let Some(active_idx) = sidechain_active_state_param(desc.name.as_str(), *input_channel)
        {
            let active = ext_connected;
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        logical_id: node_id as u64,
                        idx: active_idx,
                        fvalue: if active { 1.0 } else { 0.0 },
                    },
                );
            }
        }
    }

    fn apply_effect_to_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        node_ids: lisp_host::EffectGraphNodeIds,
        name: &str,
        manifest: &lisp_host::DGenManifest,
    ) {
        let old_desc = self.graph.effect_descriptors[track][slot_idx].clone();
        let preserve_by_param_name = old_desc.name == name
            && self.state.pattern.effect_chains[track][slot_idx]
                .node_id
                .load(Ordering::Relaxed)
                != 0;
        let desc = self.build_effect_descriptor(track, name, manifest);
        self.graph.effect_descriptors[track][slot_idx] = desc.clone();

        let slot = &self.state.pattern.effect_chains[track][slot_idx];
        if preserve_by_param_name {
            slot.sync_descriptor_by_param_name(&old_desc, &desc, node_ids.effect_node_id as u32);
            slot.modulator_node_id.store(
                node_ids.modulator_node_id.unwrap_or(0) as u32,
                Ordering::Relaxed,
            );
        } else {
            slot.apply_descriptor_with_modulator(
                &desc,
                node_ids.effect_node_id as u32,
                node_ids.modulator_node_id.unwrap_or(0) as u32,
            );
        }

        let desc = self.graph.effect_descriptors[track][slot_idx].clone();
        self.state
            .sync_effect_slot_with_modulator_in_track_patterns(
                track,
                slot_idx,
                &desc,
                node_ids.effect_node_id as u32,
                node_ids.modulator_node_id.unwrap_or(0) as u32,
            );

        self.push_track_effect_slot_defaults(track, slot_idx);
        self.push_all_delay_bpm();
        self.sync_scratch_runtime_descriptors();
    }

    fn descriptor_has_sidechain_control(desc: &EffectDescriptor) -> bool {
        desc.params
            .iter()
            .any(|param| matches!(param.host_control, Some(HostControl::FxSidechain { .. })))
    }

    fn is_sidechain_effect_name(name: &str) -> bool {
        name.trim().trim_end_matches('/') == "sidechain"
    }

    pub fn repair_stale_sidechain_effect_slots(&mut self) -> Result<usize, String> {
        let mut targets = Vec::new();
        for (track, descs) in self.graph.effect_descriptors.iter().enumerate() {
            for slot_idx in BUILTIN_SLOT_COUNT..descs.len() {
                let desc = &descs[slot_idx];
                if !Self::is_sidechain_effect_name(&desc.name)
                    || Self::descriptor_has_sidechain_control(desc)
                {
                    continue;
                }
                let node_id = self
                    .state
                    .pattern
                    .effect_chains
                    .get(track)
                    .and_then(|chain| chain.get(slot_idx))
                    .map(|slot| slot.node_id.load(Ordering::Relaxed))
                    .unwrap_or(0);
                if node_id != 0 {
                    targets.push((track, slot_idx, desc.name.clone()));
                }
            }
        }

        let saved_effect_tab = self.ui.effect_tab;
        let saved_effect_tab_cursor = self.ui.effect_tab_cursor;
        let saved_effect_param_cursor = self.ui.effect_param_cursor;
        let saved_effect_scroll_offset = self.ui.effect_scroll_offset;
        let mut repaired = 0;
        for (track, slot_idx, name) in targets {
            let result = self.compile_saved_effect(&name)?;
            if lisp_host::effect_sidechain_inputs(&result.manifest).is_empty() {
                continue;
            }
            self.apply_compiled_effect_to_slot_sync(result, &name, slot_idx, track)?;
            repaired += 1;
        }
        if repaired > 0 {
            self.ui.effect_tab = saved_effect_tab;
            self.ui.effect_tab_cursor = saved_effect_tab_cursor;
            self.ui.effect_param_cursor = saved_effect_param_cursor;
            self.ui.effect_scroll_offset = saved_effect_scroll_offset;
        }
        Ok(repaired)
    }


    pub fn start_effect_compile(&mut self, name: &str, slot_idx: usize) {
        let track = match self.track_registry.id_at(self.ui.cursor_track) {
            Some(track) => track,
            None => {
                self.editor.status_message = Some(("Error: effect target track is missing".to_string(), Instant::now()));
                return;
            }
        };
        let expected_node_id = self
            .state
            .pattern
            .effect_chains
            .get(self.ui.cursor_track)
            .and_then(|chain| chain.get(slot_idx))
            .map(|slot| slot.node_id.load(Ordering::Relaxed))
            .unwrap_or(0);
        let source_path = lisp_host::effect_source_path(name);
        let source = match std::fs::read_to_string(&source_path) {
            Ok(source) => source,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.graph.sample_rate;
        let asset_base = source_path.parent().map(|path| path.to_path_buf());
        let cache = self.editor.dylib_cache.clone();
        std::thread::spawn(move || {
            let result = cache.acquire(
                lisp_host::DGenCompileKind::Effect,
                lisp_host::DGenSourceOrigin::Custom,
                &source,
                sample_rate,
                asset_base.as_deref(),
            );
            let _ = tx.send(result);
        });
        self.editor.pending_compile = Some(PendingCompile {
            receiver: rx,
            target: CompileTarget::Effect {
                name: name.to_string(),
                slot_idx,
                track,
                expected_node_id,
            },
            tick: 0,
        });
    }

    /// Poll for async compile completion. Returns a status message if something finished.
    pub fn poll_pending_compile(&mut self) -> Option<String> {
        self.reclaim_applied_effect_leases();
        let pending = self.editor.pending_compile.as_ref()?;
        match pending.receiver.try_recv() {
            Ok(Ok(compile_result)) => {
                let target = match &pending.target {
                    CompileTarget::Effect {
                        name,
                        slot_idx,
                        track,
                        expected_node_id,
                    } => CompileTarget::Effect {
                        name: name.clone(),
                        slot_idx: *slot_idx,
                        track: *track,
                        expected_node_id: *expected_node_id,
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
                        expected_node_id,
                    } => {
                        let Some(track) = self.track_registry.index_of(track) else {
                            return Some(format!("Effect load canceled: target track no longer exists"));
                        };
                        let current_node_id = self
                            .state
                            .pattern
                            .effect_chains
                            .get(track)
                            .and_then(|chain| chain.get(slot_idx))
                            .map(|slot| slot.node_id.load(Ordering::Relaxed));
                        if current_node_id != Some(expected_node_id) {
                            return Some("Effect load canceled: target slot changed while compiling".to_string());
                        }
                        match self.apply_compiled_effect_to_slot_recorded(
                            compile_result,
                            &name,
                            slot_idx,
                            track,
                        ) {
                            Ok(()) => Some(format!("Loaded effect: {name}")),
                            Err(error) => Some(format!("Effect load failed: {error}")),
                        }
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

    pub fn apply_compiled_effect_to_slot_sync(
        &mut self,
        result: lisp_host::CompileResult,
        name: &str,
        slot_idx: usize,
        track: usize,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if slot_idx >= self.graph.effect_descriptors[track].len() {
            return Err("Invalid effect slot".to_string());
        }
        let (manifest, node_ids) =
            self.install_compiled_fx_node(FxChainLocator::Track(track), slot_idx, result)?;
        self.apply_effect_to_slot(track, slot_idx, node_ids, name, &manifest);
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        self.ui.effect_param_cursor = 0;
        self.ui.effect_scroll_offset = 0;
        self.ui.focused_region = Region::Params;
        self.ui.params_column = 1;
        Ok(())
    }

    pub fn apply_compiled_effect(
        &mut self,
        result: lisp_host::CompileResult,
        name: &str,
        slot_idx: usize,
        track: usize,
    ) {
        match self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track) {
            Ok(()) => {
                self.editor.status_message = Some((format!("Loaded '{}'", name), Instant::now()));
            }
            Err(error) => {
                self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
            }
        }
    }

    pub fn load_saved_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let source = self.retained_effect_source_for_name(name)?;
        let result = self.compile_saved_effect(name)?;
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
        self.retain_effect_source(FxChainLocator::Track(track), slot_idx, source)?;
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
        self.initialize_other_bus_pattern_effect_slot(bus_idx, slot_idx);
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
        self.initialize_other_bus_pattern_effect_slot(bus_idx, slot_idx);
        Ok(slot_idx)
    }

    fn bus_effect_entries(&self, bus_idx: usize) -> Result<Vec<BusEffectEntry>, String> {
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        Ok((0..MAX_CUSTOM_FX)
            .filter_map(|slot_idx| {
                let slot = bus.effect_slots.get(slot_idx)?;
                if slot.node_id == 0 {
                    return None;
                }
                Some(BusEffectEntry {
                    desc: bus.effect_descriptors[slot_idx].clone(),
                    snapshot: slot.clone(),
                    custom_name: bus.custom_effect_names.get(slot_idx).cloned().flatten(),
                })
            })
            .collect())
    }

    fn active_bus_effect_slots(&self, bus_idx: usize) -> Result<Vec<usize>, String> {
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        Ok(bus
            .effect_slots
            .iter()
            .enumerate()
            .take(MAX_CUSTOM_FX)
            .filter_map(|(slot_idx, slot)| (slot.node_id != 0).then_some(slot_idx))
            .collect())
    }

    fn write_bus_effect_entries(
        &mut self,
        bus_idx: usize,
        entries: &[BusEffectEntry],
    ) -> Result<(), String> {
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        for slot_idx in 0..MAX_CUSTOM_FX {
            if slot_idx >= bus.effect_descriptors.len() || slot_idx >= bus.effect_slots.len() {
                break;
            }
            if let Some(entry) = entries.get(slot_idx) {
                bus.effect_descriptors[slot_idx] = entry.desc.clone();
                bus.effect_slots[slot_idx] = entry.snapshot.clone();
                if slot_idx < bus.custom_effect_names.len() {
                    bus.custom_effect_names[slot_idx] = entry.custom_name.clone();
                }
            } else {
                bus.effect_descriptors[slot_idx] = EffectDescriptor::empty_custom_slot();
                bus.effect_slots[slot_idx] = EffectSlotSnapshot::new_empty();
                if slot_idx < bus.custom_effect_names.len() {
                    bus.custom_effect_names[slot_idx] = None;
                }
            }
        }
        Ok(())
    }

    fn prepare_bus_effect_insert_slot(
        &mut self,
        bus_idx: usize,
        target_slot: usize,
    ) -> Result<usize, String> {
        let snapshot_before_remap = self.capture_bus_pattern_snapshot();
        let mut entries = self.bus_effect_entries(bus_idx)?;
        let mut new_to_old = self
            .active_bus_effect_slots(bus_idx)?
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        if entries.len() >= MAX_CUSTOM_FX {
            return Err("No free bus effect slots available".to_string());
        }
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let insert_offset = bus
            .effect_slots
            .iter()
            .enumerate()
            .take(target_slot.min(MAX_CUSTOM_FX))
            .filter(|(_, slot)| slot.node_id != 0)
            .count()
            .min(entries.len());
        entries.insert(
            insert_offset,
            BusEffectEntry {
                desc: EffectDescriptor::empty_custom_slot(),
                snapshot: EffectSlotSnapshot::new_empty(),
                custom_name: None,
            },
        );
        new_to_old.insert(insert_offset, None);
        new_to_old.resize(MAX_CUSTOM_FX, None);
        new_to_old.truncate(MAX_CUSTOM_FX);
        let locator = self.bus_fx_locator(bus_idx)?;
        let old_host = self.fx_chain_host(locator)?;
        self.write_bus_effect_entries(bus_idx, &entries)?;
        let new_host = self.fx_chain_host(locator)?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        self.insert_empty_bus_effect_lease_slot(bus_idx, insert_offset)?;
        self.remap_other_bus_pattern_effect_slots(bus_idx, &new_to_old, &snapshot_before_remap);
        Ok(insert_offset)
    }

    pub fn insert_builtin_bus_effect_before_slot_sync(
        &mut self,
        bus_idx: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let slot_idx = self.prepare_bus_effect_insert_slot(bus_idx, target_slot)?;
        self.load_builtin_bus_effect_to_slot_sync(bus_idx, slot_idx, name)?;
        self.initialize_other_bus_pattern_effect_slot(bus_idx, slot_idx);
        Ok(slot_idx)
    }

    pub fn insert_bus_effect_before_slot_sync(
        &mut self,
        bus_idx: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        let result = self.compile_saved_effect(name)?;
        let slot_idx = self.prepare_bus_effect_insert_slot(bus_idx, target_slot)?;
        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot_idx, name, result)?;
        self.initialize_other_bus_pattern_effect_slot(bus_idx, slot_idx);
        Ok(slot_idx)
    }

    pub fn move_bus_effect_slot_sync(
        &mut self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: Option<usize>,
    ) -> Result<usize, String> {
        let snapshot_before_remap = self.capture_bus_pattern_snapshot();
        let mut entries = self.bus_effect_entries(bus_idx)?;
        let active_slots = self.active_bus_effect_slots(bus_idx)?;
        let source_offset = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?
            .effect_slots
            .iter()
            .enumerate()
            .take(source_slot.min(MAX_CUSTOM_FX))
            .filter(|(_, slot)| slot.node_id != 0)
            .count();
        if source_offset >= entries.len() {
            return Err("Invalid source bus effect slot".to_string());
        }
        let entry = entries.remove(source_offset);
        let mut target_offset = match target_slot {
            Some(slot) => self
                .buses
                .get(bus_idx)
                .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?
                .effect_slots
                .iter()
                .enumerate()
                .take(slot.min(MAX_CUSTOM_FX))
                .filter(|(_, slot)| slot.node_id != 0)
                .count(),
            None => entries.len(),
        };
        if source_offset < target_offset {
            target_offset = target_offset.saturating_sub(1);
        }
        target_offset = target_offset.min(entries.len());
        if target_offset == source_offset {
            entries.insert(source_offset, entry);
            return Ok(source_slot);
        }
        let mut new_to_old = active_slots.into_iter().map(Some).collect::<Vec<_>>();
        let source_physical_slot = new_to_old.remove(source_offset);
        new_to_old.insert(target_offset, source_physical_slot);
        new_to_old.resize(MAX_CUSTOM_FX, None);
        new_to_old.truncate(MAX_CUSTOM_FX);
        let locator = self.bus_fx_locator(bus_idx)?;
        let old_host = self.fx_chain_host(locator)?;
        entries.insert(target_offset, entry);
        self.write_bus_effect_entries(bus_idx, &entries)?;
        let new_host = self.fx_chain_host(locator)?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        self.move_bus_effect_lease_slot(bus_idx, source_offset, target_offset)?;
        self.remap_other_bus_pattern_effect_slots(bus_idx, &new_to_old, &snapshot_before_remap);
        self.push_all_delay_bpm();
        Ok(target_offset)
    }

    /// Bus counterpart of `load_dgen_builtin_to_slot_sync`.
    pub(super) fn load_dgen_builtin_bus_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let builtin = crate::effects::dgen_builtin::find(name)
            .ok_or_else(|| format!("Unknown dgenlisp builtin '{name}'"))?;
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            builtin.origin,
            builtin.source,
            self.graph.sample_rate,
            None,
        )?;
        let ir_slots = crate::effects::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
        let table_slot = crate::effects::filter_table::TableSlot::from_manifest(&result.manifest);
        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot_idx, name, result)?;
        let node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .unwrap_or(0);
        self.initialize_dgen_builtin_node(name, node_id, ir_slots, table_slot)
    }

    pub fn load_builtin_bus_effect_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        if crate::effects::dgen_builtin::contains(name) {
            return self.load_dgen_builtin_bus_to_slot_sync(bus_idx, slot_idx, name);
        }
        let mut desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        patch_sidechain_labels(&mut desc, &self.bus_effect_sidechain_labels());
        let locator = self.bus_fx_locator(bus_idx)?;
        let (node_id, modulator_node_id) =
            self.install_builtin_fx_node(locator, slot_idx, &desc)?;
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        bus.effect_descriptors[slot_idx] = desc.clone();
        bus.effect_slots[slot_idx] = crate::effects::EffectSlotSnapshot::new_default_with_modulator(
            &desc,
            node_id as u32,
            modulator_node_id.unwrap_or(0) as u32,
        );
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
        if bus_idx >= lisp_host::MAX_BUS_FX_CHAINS {
            return Err(format!(
                "Bus {} is outside the current bus FX registry limit",
                bus_idx + 1
            ));
        }
        let result = self.compile_saved_effect(name)?;
        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot_idx, name, result)
    }

    pub fn apply_compiled_bus_effect_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
        result: lisp_host::CompileResult,
    ) -> Result<(), String> {
        if bus_idx >= lisp_host::MAX_BUS_FX_CHAINS {
            return Err(format!(
                "Bus {} is outside the current bus FX registry limit",
                bus_idx + 1
            ));
        }
        let locator = self.bus_fx_locator(bus_idx)?;
        let (manifest, node_ids) = self.install_compiled_fx_node(locator, slot_idx, result)?;
        let desc = self.build_bus_effect_descriptor(name, &manifest);
        let bus = self
            .buses
            .get_mut(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        bus.effect_descriptors[slot_idx] = desc;
        let slot = &mut bus.effect_slots[slot_idx];
        *slot = crate::effects::EffectSlotSnapshot::new_default_with_modulator(
            &bus.effect_descriptors[slot_idx],
            node_ids.effect_node_id as u32,
            node_ids.modulator_node_id.unwrap_or(0) as u32,
        );
        if slot_idx < bus.custom_effect_names.len() {
            bus.custom_effect_names[slot_idx] = Some(name.to_string());
        }
        self.push_bus_effect_slot_defaults(bus_idx, slot_idx);
        self.push_all_delay_bpm();
        Ok(())
    }

    pub fn delete_bus_effect_slot(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        let snapshot_before_remap = self.capture_bus_pattern_snapshot();
        let active_slots = self.active_bus_effect_slots(bus_idx)?;
        let source_offset = active_slots.iter().position(|slot| *slot == slot_idx)
            .ok_or_else(|| format!("Bus effect slot {} is empty", slot_idx + 1))?;
        let mut entries = self.bus_effect_entries(bus_idx)?;
        entries.remove(source_offset);
        let mut new_to_old = active_slots.into_iter().map(Some).collect::<Vec<_>>();
        new_to_old.remove(source_offset);
        new_to_old.resize(MAX_CUSTOM_FX, None);
        let locator = self.bus_fx_locator(bus_idx)?;
        self.remove_fx_slot_node(locator, slot_idx, FxLeaseSlotRemoval::Shift)?;
        self.write_bus_effect_entries(bus_idx, &entries)?;
        self.remap_other_bus_pattern_effect_slots(
            bus_idx,
            &new_to_old,
            &snapshot_before_remap,
        );
        Ok(())
    }

    pub fn set_bus_effect_param(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        let (node_id, modulator_node_id, node_param_idx, node_param_span, stored_value) = {
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
            if crate::instruments::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
                return Err(
                    "Group/bus effect modulation does not support envelope sources".to_string(),
                );
            }
            let slot = bus
                .effect_slots
                .get_mut(slot_idx)
                .ok_or_else(|| format!("Bus effect slot {} out of range", slot_idx + 1))?;
            let stored_value = value.clamp(param.min, param.max);
            if param_idx < slot.defaults.len() {
                slot.defaults[param_idx] = stored_value;
            }
            (
                slot.node_id,
                slot.modulator_node_id,
                param.node_param_idx,
                param.node_param_span.max(1),
                stored_value,
            )
        };
        push_fx_param(
            self.graph.lg.0,
            node_id,
            modulator_node_id,
            node_param_idx,
            node_param_span,
            stored_value,
        );
        self.sync_bus_effect_mod_active_defaults(bus_idx, slot_idx);
        Ok(())
    }

    pub fn set_bus_effect_plock(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        step: usize,
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
        if crate::instruments::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
            return Err(
                "Group/bus effect modulation does not support envelope sources".to_string(),
            );
        }
        let slot = bus
            .effect_slots
            .get_mut(slot_idx)
            .ok_or_else(|| format!("Bus effect slot {} out of range", slot_idx + 1))?;
        if step < slot.plocks.len() && param_idx < slot.plocks[step].len() {
            slot.plocks[step][param_idx] = Some(value.clamp(param.min, param.max));
        }
        self.sync_bus_effect_mod_active_plocks(bus_idx, step, slot_idx);
        Ok(())
    }

    fn sync_bus_effect_mod_active_defaults(&mut self, bus_idx: usize, slot_idx: usize) {
        let Some(updates) = self.buses.get(bus_idx).and_then(|bus| {
            let desc = bus.effect_descriptors.get(slot_idx)?;
            let slot = bus.effect_slots.get(slot_idx)?;
            let mut active_indices = desc
                .instrument_modulation_targets
                .iter()
                .filter_map(|target| target.active_param_idx)
                .collect::<Vec<_>>();
            active_indices.sort_unstable();
            active_indices.dedup();
            Some(
                active_indices
                    .into_iter()
                    .filter_map(|active_param_idx| {
                        let param = desc.params.get(active_param_idx)?;
                        let active = desc
                            .instrument_modulation_targets
                            .iter()
                            .filter(|target| target.active_param_idx == Some(active_param_idx))
                            .any(|target| {
                                slot.defaults
                                    .get(target.depth_param_idx)
                                    .copied()
                                    .unwrap_or(0.0)
                                    .abs()
                                    > f32::EPSILON
                            });
                        Some((
                            active_param_idx,
                            param.node_param_idx,
                            param.node_param_span.max(1),
                            active,
                        ))
                    })
                    .collect::<Vec<_>>(),
            )
        }) else {
            return;
        };
        let Some((node_id, modulator_node_id)) = self.buses.get_mut(bus_idx).and_then(|bus| {
            let slot = bus.effect_slots.get_mut(slot_idx)?;
            for (active_param_idx, _, _, active) in &updates {
                if *active_param_idx < slot.defaults.len() {
                    slot.defaults[*active_param_idx] = if *active { 1.0 } else { 0.0 };
                }
            }
            Some((slot.node_id, slot.modulator_node_id))
        }) else {
            return;
        };
        for (_, node_param_idx, node_param_span, active) in updates {
            push_fx_param(
                self.graph.lg.0,
                node_id,
                modulator_node_id,
                node_param_idx,
                node_param_span,
                if active { 1.0 } else { 0.0 },
            );
        }
    }

    fn sync_bus_effect_mod_active_plocks(&mut self, bus_idx: usize, step: usize, slot_idx: usize) {
        let Some(updates) = self.buses.get(bus_idx).and_then(|bus| {
            let desc = bus.effect_descriptors.get(slot_idx)?;
            let slot = bus.effect_slots.get(slot_idx)?;
            let mut active_indices = desc
                .instrument_modulation_targets
                .iter()
                .filter_map(|target| target.active_param_idx)
                .collect::<Vec<_>>();
            active_indices.sort_unstable();
            active_indices.dedup();
            Some(
                active_indices
                    .into_iter()
                    .map(|active_param_idx| {
                        let active = desc
                            .instrument_modulation_targets
                            .iter()
                            .filter(|target| target.active_param_idx == Some(active_param_idx))
                            .any(|target| {
                                slot.plocks
                                    .get(step)
                                    .and_then(|step_plocks| step_plocks.get(target.depth_param_idx))
                                    .copied()
                                    .flatten()
                                    .unwrap_or_else(|| {
                                        slot.defaults
                                            .get(target.depth_param_idx)
                                            .copied()
                                            .unwrap_or(0.0)
                                    })
                                    .abs()
                                    > f32::EPSILON
                            });
                        (active_param_idx, active)
                    })
                    .collect::<Vec<_>>(),
            )
        }) else {
            return;
        };
        let Some(slot) = self
            .buses
            .get_mut(bus_idx)
            .and_then(|bus| bus.effect_slots.get_mut(slot_idx))
        else {
            return;
        };
        if step >= slot.plocks.len() {
            return;
        }
        for (active_param_idx, active) in updates {
            if active_param_idx < slot.plocks[step].len() {
                slot.plocks[step][active_param_idx] = Some(if active { 1.0 } else { 0.0 });
            }
        }
    }

    fn resolve_bus_effect_slot_wiring(
        &self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(usize, i32, usize, i32, usize, Option<i32>), String> {
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        if slot_idx >= bus.effect_descriptors.len() || slot_idx >= bus.effect_slots.len() {
            return Err(format!("Bus effect slot {} out of range", slot_idx + 1));
        }

        // Resolve by BusId rather than vector position: bus ordering is project
        // state, while graph nodes may survive project reconstruction.
        self.resolve_fx_slot(FxChainLocator::Bus(bus.id), slot_idx)
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
                ParamKind::Enum { labels } => {
                    if crate::instruments::voice_modulator::is_source_param(p.node_param_idx) && label == "env" {
                        None
                    } else {
                        labels.iter().position(|item| item == label)
                    }
                }
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
            if crate::instruments::voice_modulator::is_envelope_source_param_value(
                param.node_param_idx,
                slot.defaults[param_idx],
            ) {
                continue;
            }
            push_fx_param(
                self.graph.lg.0,
                slot.node_id,
                slot.modulator_node_id,
                param.node_param_idx,
                param.node_param_span,
                slot.defaults[param_idx],
            );
        }
    }

    pub(super) fn rack_slot_fx_locator(track: usize, rack_slot: usize) -> FxChainLocator {
        FxChainLocator::RackSlot {
            track,
            slot: rack_slot,
        }
    }

    pub(super) fn rack_slot_effect_snapshot(
        &self,
        track: usize,
        rack_slot: usize,
    ) -> Result<crate::sequencer::RackSlotSnapshot, String> {
        self.state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(rack_slot))
            .cloned()
            .ok_or_else(|| format!("Track {} rack slot {} not found", track + 1, rack_slot + 1))
    }

    fn write_rack_slot_effect(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        descriptor: EffectDescriptor,
        snapshot: EffectSlotSnapshot,
        custom_name: Option<String>,
    ) -> Result<(), String> {
        if effect_slot >= MAX_CUSTOM_FX {
            return Err(format!(
                "Rack-slot effect slot {} is out of range",
                effect_slot + 1
            ));
        }
        let updated =
            self.state
                .update_rack_slot_in_all_pattern_snapshots(track, rack_slot, |slot| {
                    slot.normalize_effect_chain();
                    slot.effect_descriptors[effect_slot] = descriptor.clone();
                    slot.effect_slots[effect_slot] = snapshot.clone();
                    slot.custom_effect_names[effect_slot] = custom_name.clone();
                });
        if !updated {
            return Err("Failed to update rack-slot FX state".to_string());
        }
        self.graph_controller()
            .refresh_rack_signature_from_live_state(track);
        Ok(())
    }

    pub fn next_free_rack_slot_effect_slot(&self, track: usize, rack_slot: usize) -> Option<usize> {
        self.rack_slot_effect_snapshot(track, rack_slot)
            .ok()?
            .effect_slots
            .iter()
            .position(|slot| slot.node_id == 0)
    }

    pub fn add_builtin_rack_slot_effect_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        let effect_slot = self
            .next_free_rack_slot_effect_slot(track, rack_slot)
            .ok_or_else(|| "No free rack-slot effect slots available".to_string())?;
        self.load_builtin_rack_slot_effect_to_slot_sync(track, rack_slot, effect_slot, name)?;
        Ok(effect_slot)
    }

    pub fn add_rack_slot_effect_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        let effect_slot = self
            .next_free_rack_slot_effect_slot(track, rack_slot)
            .ok_or_else(|| "No free rack-slot effect slots available".to_string())?;
        self.load_rack_slot_effect_to_slot_sync(track, rack_slot, effect_slot, name)?;
        Ok(effect_slot)
    }

    fn prepare_rack_slot_effect_insert_slot(
        &mut self,
        track: usize,
        rack_slot: usize,
        target_slot: usize,
    ) -> Result<usize, String> {
        if target_slot >= MAX_CUSTOM_FX {
            return Err("Rack-slot FX insert target is out of range".to_string());
        }
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let old_host = self.fx_chain_host(locator)?;
        if old_host.slots[target_slot].node_id == 0 {
            return Err("Rack-slot FX insert target is empty".to_string());
        }
        if old_host.slots.iter().all(|slot| slot.node_id != 0) {
            return Err("No free rack-slot effect slots available".to_string());
        }
        let updated =
            self.state
                .update_rack_slot_in_all_pattern_snapshots(track, rack_slot, |slot| {
                    slot.normalize_effect_chain();
                    slot.effect_slots
                        .insert(target_slot, EffectSlotSnapshot::new_empty());
                    slot.effect_slots.truncate(MAX_CUSTOM_FX);
                    slot.effect_descriptors
                        .insert(target_slot, EffectDescriptor::empty_custom_slot());
                    slot.effect_descriptors.truncate(MAX_CUSTOM_FX);
                    slot.custom_effect_names.insert(target_slot, None);
                    slot.custom_effect_names.truncate(MAX_CUSTOM_FX);
                });
        if !updated {
            return Err("Failed to update rack-slot FX state".to_string());
        }
        self.graph_controller()
            .refresh_rack_signature_from_live_state(track);
        self.state
            .update_rack_macros_for_all_pattern_snapshots(track, |macros| {
                for mapping in macros
                    .iter_mut()
                    .flat_map(|rack_macro| &mut rack_macro.mappings)
                {
                    if let crate::sequencer::RackMacroTarget::SlotEffectParam {
                        slot,
                        effect_slot,
                        ..
                    } = &mut mapping.target
                    {
                        if *slot == rack_slot && *effect_slot >= target_slot {
                            *effect_slot += 1;
                        }
                    }
                }
            });
        let new_host = self.fx_chain_host(locator)?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        self.editor
            .effect_chain_leases
            .insert_empty_slot(locator, target_slot)?;
        Ok(target_slot)
    }

    pub fn insert_builtin_rack_slot_effect_before_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        if EffectDescriptor::builtin_insert(name).is_none()
            && !crate::effects::dgen_builtin::contains(name)
        {
            return Err(format!("Unknown built-in effect '{name}'"));
        }
        let effect_slot =
            self.prepare_rack_slot_effect_insert_slot(track, rack_slot, target_slot)?;
        self.load_builtin_rack_slot_effect_to_slot_sync(track, rack_slot, effect_slot, name)?;
        Ok(effect_slot)
    }

    pub fn insert_rack_slot_effect_before_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        target_slot: usize,
        name: &str,
    ) -> Result<usize, String> {
        let result = self.compile_saved_effect(name)?;
        let effect_slot =
            self.prepare_rack_slot_effect_insert_slot(track, rack_slot, target_slot)?;
        self.apply_compiled_rack_slot_effect_to_slot_sync(
            track,
            rack_slot,
            effect_slot,
            name,
            result,
        )?;
        Ok(effect_slot)
    }

    pub fn load_builtin_rack_slot_effect_to_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        name: &str,
    ) -> Result<(), String> {
        if let Some(builtin) = crate::effects::dgen_builtin::find(name) {
            let result = self.editor.dylib_cache.acquire(
                lisp_host::DGenCompileKind::Effect,
                builtin.origin,
                builtin.source,
                self.graph.sample_rate,
                None,
            )?;
            let ir_slots =
                crate::effects::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
            let table_slot =
                crate::effects::filter_table::TableSlot::from_manifest(&result.manifest);
            self.apply_compiled_rack_slot_effect_to_slot_sync(
                track,
                rack_slot,
                effect_slot,
                name,
                result,
            )?;
            let node_id = self
                .rack_slot_effect_snapshot(track, rack_slot)?
                .effect_slots[effect_slot]
                .node_id as i32;
            return self.initialize_dgen_builtin_node(name, node_id, ir_slots, table_slot);
        }
        let mut descriptor = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        patch_sidechain_labels(&mut descriptor, &self.effect_sidechain_labels(track));
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let (node_id, modulator_node_id) =
            self.install_builtin_fx_node(locator, effect_slot, &descriptor)?;
        let snapshot = EffectSlotSnapshot::new_default_with_modulator(
            &descriptor,
            node_id as u32,
            modulator_node_id.unwrap_or(0) as u32,
        );
        let project_name = EffectDescriptor::builtin_insert_project_name(&descriptor.name);
        self.write_rack_slot_effect(
            track,
            rack_slot,
            effect_slot,
            descriptor,
            snapshot,
            project_name,
        )?;
        self.push_rack_slot_effect_defaults(track, rack_slot, effect_slot);
        self.push_all_delay_bpm();
        Ok(())
    }

    pub fn load_rack_slot_effect_to_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        name: &str,
    ) -> Result<(), String> {
        let result = self.compile_saved_effect(name)?;
        self.apply_compiled_rack_slot_effect_to_slot_sync(
            track,
            rack_slot,
            effect_slot,
            name,
            result,
        )
    }

    pub(super) fn apply_compiled_rack_slot_effect_to_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        name: &str,
        result: lisp_host::CompileResult,
    ) -> Result<(), String> {
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let (manifest, node_ids) = self.install_compiled_fx_node(locator, effect_slot, result)?;
        let mut descriptor = EffectDescriptor::from_lisp_manifest(
            name,
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        descriptor.tensor_params = crate::effects::tensor_param_descriptors_from_manifest(
            &manifest.tensors,
            &manifest.tensor_init_data,
        );
        lisp_host::append_effect_host_modulation_controls(&mut descriptor, &manifest);
        let snapshot = EffectSlotSnapshot::new_default_with_modulator(
            &descriptor,
            node_ids.effect_node_id as u32,
            node_ids.modulator_node_id.unwrap_or(0) as u32,
        );
        self.write_rack_slot_effect(
            track,
            rack_slot,
            effect_slot,
            descriptor,
            snapshot,
            Some(name.to_string()),
        )?;
        self.push_rack_slot_effect_defaults(track, rack_slot, effect_slot);
        self.push_all_delay_bpm();
        Ok(())
    }

    pub fn delete_rack_slot_effect_slot(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
    ) -> Result<(), String> {
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        self.remove_fx_slot_node(locator, effect_slot, FxLeaseSlotRemoval::Shift)?;
        let updated =
            self.state
                .update_rack_slot_in_all_pattern_snapshots(track, rack_slot, |slot| {
                    slot.normalize_effect_chain();
                    slot.effect_slots.remove(effect_slot);
                    slot.effect_slots.push(EffectSlotSnapshot::new_empty());
                    slot.effect_descriptors.remove(effect_slot);
                    slot.effect_descriptors
                        .push(EffectDescriptor::empty_custom_slot());
                    slot.custom_effect_names.remove(effect_slot);
                    slot.custom_effect_names.push(None);
                });
        if updated {
            self.state.update_rack_macros_for_all_pattern_snapshots(track, |macros| {
                for rack_macro in macros {
                    rack_macro.mappings.retain(|mapping| !matches!(mapping.target,
                        crate::sequencer::RackMacroTarget::SlotEffectParam { slot, effect_slot: mapped, .. }
                        if slot == rack_slot && mapped == effect_slot));
                    for mapping in &mut rack_macro.mappings {
                        if let crate::sequencer::RackMacroTarget::SlotEffectParam { slot, effect_slot: mapped, .. } = &mut mapping.target {
                            if *slot == rack_slot && *mapped > effect_slot { *mapped -= 1; }
                        }
                    }
                }
            });
            self.graph_controller()
                .refresh_rack_signature_from_live_state(track);
        }
        updated
            .then_some(())
            .ok_or_else(|| "Failed to update rack-slot FX state".to_string())
    }

    pub fn move_rack_slot_effect_slot_sync(
        &mut self,
        track: usize,
        rack_slot: usize,
        source_slot: usize,
        target_slot: usize,
    ) -> Result<(), String> {
        if source_slot >= MAX_CUSTOM_FX || target_slot >= MAX_CUSTOM_FX {
            return Err("Rack-slot FX move is out of range".to_string());
        }
        if source_slot == target_slot {
            return Ok(());
        }
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let old_host = self.fx_chain_host(locator)?;
        if old_host.slots[source_slot].node_id == 0 {
            return Err("Source rack-slot effect is empty".to_string());
        }
        let updated =
            self.state
                .update_rack_slot_in_all_pattern_snapshots(track, rack_slot, |slot| {
                    slot.normalize_effect_chain();
                    let effect = slot.effect_slots.remove(source_slot);
                    slot.effect_slots.insert(target_slot, effect);
                    let descriptor = slot.effect_descriptors.remove(source_slot);
                    slot.effect_descriptors.insert(target_slot, descriptor);
                    let name = slot.custom_effect_names.remove(source_slot);
                    slot.custom_effect_names.insert(target_slot, name);
                });
        if !updated {
            return Err("Failed to move rack-slot effect state".to_string());
        }
        self.graph_controller()
            .refresh_rack_signature_from_live_state(track);
        self.state
            .update_rack_macros_for_all_pattern_snapshots(track, |macros| {
                for mapping in macros
                    .iter_mut()
                    .flat_map(|rack_macro| &mut rack_macro.mappings)
                {
                    if let crate::sequencer::RackMacroTarget::SlotEffectParam {
                        slot,
                        effect_slot,
                        ..
                    } = &mut mapping.target
                    {
                        if *slot != rack_slot {
                            continue;
                        }
                        *effect_slot = if *effect_slot == source_slot {
                            target_slot
                        } else if source_slot < target_slot
                            && *effect_slot > source_slot
                            && *effect_slot <= target_slot
                        {
                            *effect_slot - 1
                        } else if target_slot < source_slot
                            && *effect_slot >= target_slot
                            && *effect_slot < source_slot
                        {
                            *effect_slot + 1
                        } else {
                            *effect_slot
                        };
                    }
                }
            });
        let new_host = self.fx_chain_host(locator)?;
        {
            let _batch = FxGraphEditBatch::new(self.graph.lg.0);
            rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        }
        self.editor
            .effect_chain_leases
            .move_slot(locator, source_slot, target_slot)?;
        Ok(())
    }

    pub fn set_rack_slot_effect_param(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        let rack = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let descriptor = rack
            .effect_descriptors
            .get(effect_slot)
            .ok_or_else(|| "Rack-slot effect slot is out of range".to_string())?;
        let param = descriptor
            .params
            .get(param_idx)
            .ok_or_else(|| "Rack-slot effect parameter is out of range".to_string())?;
        if matches!(param.host_control, Some(HostControl::FxSidechain { .. })) {
            return Err("Sidechain routing into rack-slot effects is not supported".to_string());
        }
        if crate::instruments::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
            return Err(
                "Rack-slot effect modulation does not support envelope sources".to_string(),
            );
        }
        let stored_value = value.clamp(param.min, param.max);
        let node_id = rack.effect_slots[effect_slot].node_id;
        let modulator_node_id = rack.effect_slots[effect_slot].modulator_node_id;
        let node_param_idx = param.node_param_idx;
        let node_param_span = param.node_param_span.max(1);
        if !self
            .state
            .update_rack_slot_in_current_pattern(track, rack_slot, |slot| {
                if let Some(value) = slot
                    .effect_slots
                    .get_mut(effect_slot)
                    .and_then(|effect| effect.defaults.get_mut(param_idx))
                {
                    *value = stored_value;
                }
            })
        {
            return Err("Failed to update rack-slot effect parameter".to_string());
        }
        push_fx_param(
            self.graph.lg.0,
            node_id,
            modulator_node_id,
            node_param_idx,
            node_param_span,
            stored_value,
        );
        Ok(())
    }

    pub fn send_rack_slot_effect_param(
        &self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        let rack = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let descriptor = rack
            .effect_descriptors
            .get(effect_slot)
            .ok_or_else(|| "Rack-slot effect slot is out of range".to_string())?;
        let param = descriptor
            .params
            .get(param_idx)
            .ok_or_else(|| "Rack-slot effect parameter is out of range".to_string())?;
        let effect = rack
            .effect_slots
            .get(effect_slot)
            .ok_or_else(|| "Rack-slot effect state is out of range".to_string())?;
        push_fx_param(
            self.graph.lg.0,
            effect.node_id,
            effect.modulator_node_id,
            param.node_param_idx,
            param.node_param_span.max(1),
            value.clamp(param.min, param.max),
        );
        Ok(())
    }

    pub fn send_effect_tensor_param(
        &self,
        track: usize,
        slot_idx: usize,
        tensor_idx: usize,
        values: &[f32],
    ) {
        let Some(slot) = self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot_idx))
        else {
            return;
        };
        let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) else {
            return;
        };
        let node_id = slot.node_id.load(Ordering::Relaxed) as i32;
        if node_id == 0 {
            return;
        }
        unsafe {
            crate::lisp_host::queue_tensor_write(self.graph.lg.0, node_id, cell_offset, values);
        }
    }

    pub fn set_rack_slot_effect_plocks(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        steps: &[usize],
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        self.set_rack_slot_effect_plocks_no_publish(
            track,
            rack_slot,
            effect_slot,
            steps,
            param_idx,
            value,
        )?;
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub(crate) fn set_rack_slot_effect_plocks_no_publish(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        steps: &[usize],
        param_idx: usize,
        value: f32,
    ) -> Result<(), String> {
        if steps.is_empty() {
            return Err("No steps are selected for rack-slot effect parameter locks".to_string());
        }
        let rack = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let descriptor = rack
            .effect_descriptors
            .get(effect_slot)
            .ok_or_else(|| "Rack-slot effect slot is out of range".to_string())?;
        let param = descriptor
            .params
            .get(param_idx)
            .ok_or_else(|| "Rack-slot effect parameter is out of range".to_string())?;
        if matches!(param.host_control, Some(HostControl::FxSidechain { .. })) {
            return Err("Sidechain routing into rack-slot effects is not supported".to_string());
        }
        if crate::instruments::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
            return Err(
                "Rack-slot effect modulation does not support envelope sources".to_string(),
            );
        }
        let effect = rack
            .effect_slots
            .get(effect_slot)
            .ok_or_else(|| "Rack-slot effect slot is out of range".to_string())?;
        if param_idx >= effect.num_params as usize
            || steps
                .iter()
                .any(|step| *step >= crate::sequencer::MAX_STEPS)
        {
            return Err("Rack-slot effect parameter-lock target is out of range".to_string());
        }
        let stored_value = value.clamp(param.min, param.max);
        let updated = self.state.update_rack_slot_in_current_pattern(
            track,
            rack_slot,
            |rack_slot_snapshot| {
                let effect = &mut rack_slot_snapshot.effect_slots[effect_slot];
                for &step in steps {
                    let wrote = effect.set_plock(step, param_idx, stored_value);
                    debug_assert!(wrote, "validated rack-slot effect p-lock target");
                }
            },
        );
        if !updated {
            return Err("Failed to set rack-slot effect parameter locks".to_string());
        }
        Ok(())
    }

    pub fn rack_slot_effect_option_value(
        &self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        param_idx: usize,
        label: &str,
    ) -> Result<f32, String> {
        let rack = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let param = rack
            .effect_descriptors
            .get(effect_slot)
            .and_then(|descriptor| descriptor.params.get(param_idx))
            .ok_or_else(|| "Rack-slot effect parameter is out of range".to_string())?;
        match &param.kind {
            ParamKind::Enum { labels } => labels
                .iter()
                .position(|item| item.eq_ignore_ascii_case(label))
                .map(|idx| idx as f32)
                .ok_or_else(|| format!("Unknown rack-slot effect option '{label}'")),
            ParamKind::Boolean => match label.to_ascii_lowercase().as_str() {
                "on" => Ok(1.0),
                "off" => Ok(0.0),
                _ => Err(format!("Unknown rack-slot effect option '{label}'")),
            },
            ParamKind::Continuous { .. } => Err(format!(
                "Rack-slot effect parameter '{}' is not an option",
                param.name
            )),
        }
    }

    pub fn set_rack_slot_effect_param_option(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        param_idx: usize,
        label: &str,
    ) -> Result<(), String> {
        let value =
            self.rack_slot_effect_option_value(track, rack_slot, effect_slot, param_idx, label)?;
        self.set_rack_slot_effect_param(track, rack_slot, effect_slot, param_idx, value)
    }

    pub fn set_rack_slot_effect_plock_option(
        &mut self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
        steps: &[usize],
        param_idx: usize,
        label: &str,
    ) -> Result<(), String> {
        let value =
            self.rack_slot_effect_option_value(track, rack_slot, effect_slot, param_idx, label)?;
        self.set_rack_slot_effect_plocks(track, rack_slot, effect_slot, steps, param_idx, value)
    }

    pub fn push_rack_slot_effect_defaults(
        &self,
        track: usize,
        rack_slot: usize,
        effect_slot: usize,
    ) {
        let Ok(rack) = self.rack_slot_effect_snapshot(track, rack_slot) else {
            return;
        };
        let Some(slot) = rack.effect_slots.get(effect_slot) else {
            return;
        };
        let Some(descriptor) = rack.effect_descriptors.get(effect_slot) else {
            return;
        };
        for (param_idx, param) in descriptor.params.iter().enumerate() {
            if param.node_param_idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            push_fx_param(
                self.graph.lg.0,
                slot.node_id,
                slot.modulator_node_id,
                param.node_param_idx,
                param.node_param_span,
                slot.defaults[param_idx],
            );
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
        crate::lisp_host::save_effect(name, source).map_err(|e| e.to_string())?;
        self.load_saved_effect_to_slot_recorded(track, slot_idx, name)?;
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        Ok(())
    }

    pub(super) fn apply_compiled_instrument(
        &mut self,
        result: lisp_host::CompileResult,
        name: &str,
    ) {
        let source = lisp_host::load_instrument_source(name).unwrap_or_default();
        let lisp_host::CompileResult {
            manifest,
            lib,
            lease,
        } = result;
        let cache_idx = self.cache_instrument_engine(name, &source, &manifest, lib, lease);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_host::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller().add_custom_track(
                name,
                cache_idx,
                &manifest,
                &*lib_ptr,
                crate::sequencer::CustomInstrumentRunMode::Instrument,
            )
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
        let source = match lisp_host::load_instrument_source(name) {
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
        let asset_base = lisp_host::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        std::thread::spawn(move || {
            let result = lisp_host::compile_and_load_instrument_with_asset_base(
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

/// Filter Table editor saves name their asset after the edited table; keep
/// stems filesystem- and reference-safe (`fltab:<stem>` must round-trip).
fn sanitize_asset_stem(name: &str) -> String {
    let mut stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        "edited-table".to_string()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};
    use crate::app::AudioBuses;
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    fn test_app_with_track_count(track_count: usize) -> App {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        let mut app = App::new(
            state,
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = (0..track_count)
            .map(|idx| format!("Track {}", idx + 1))
            .collect();
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(track_count).unwrap();
        app
    }

    fn test_app_with_track() -> App {
        test_app_with_track_count(1)
    }

    struct TestLiveGraph {
        ptr: LiveGraphPtr,
        block_size: i32,
        channels: usize,
    }

    impl TestLiveGraph {
        fn new(label: &str, block_size: i32, sample_rate: i32, channels: usize) -> Self {
            crate::audiograph::initialize_engine_for_test(block_size, sample_rate);

            let label = CString::new(label).expect("test graph label should not contain nul");
            let ptr = unsafe {
                crate::audiograph::create_live_graph(
                    32,
                    block_size,
                    label.as_ptr(),
                    channels as i32,
                )
            };
            assert!(!ptr.is_null(), "test live graph should be created");
            Self {
                ptr: LiveGraphPtr(ptr),
                block_size,
                channels,
            }
        }

        fn add_gain(&self, gain: f32, name: &str) -> i32 {
            let name = CString::new(name).expect("test gain name should not contain nul");
            let node_id =
                unsafe { crate::audiograph::add_gain_node(self.ptr.0, gain, name.as_ptr()) };
            assert!(node_id > 0, "test gain node should be allocated");
            node_id
        }

        fn process_block(&self) {
            let mut output = vec![0.0_f32; self.block_size as usize * self.channels];
            unsafe {
                self.ptr
                    .process_next_block(output.as_mut_ptr(), self.block_size);
            }
        }
    }

    impl Drop for TestLiveGraph {
        fn drop(&mut self) {
            unsafe {
                crate::audiograph::destroy_live_graph(self.ptr.0);
            }
        }
    }

    fn test_app_for_live_graph(graph: &TestLiveGraph, track_count: usize) -> App {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        let bus_l_id = graph.add_gain(1.0, "test_bus_l");
        let bus_r_id = graph.add_gain(1.0, "test_bus_r");
        App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id,
                bus_r_id,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: bus_l_id,
                reverb_node_id: bus_r_id,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    fn test_instrument_manifest() -> lisp_host::DGenManifest {
        lisp_host::DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 1,
            process_abi: String::new(),
            total_memory_slots: 1,
            params: Vec::new(),
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: ["gate", "pitch", "velocity", "trigger"]
                .into_iter()
                .enumerate()
                .map(|(channel, name)| lisp_host::DGenInput {
                    channel,
                    name: name.to_string(),
                })
                .collect(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 4,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    #[test]
    fn saved_instrument_swap_supports_cached_and_compiled_results() {
        let graph = TestLiveGraph::new("saved-instrument-swap-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let manifest = test_instrument_manifest();
        let initial_track = app
            .add_compiled_saved_instrument_track_sync(
                "old",
                "old source",
                CustomInstrumentRunMode::Instrument,
                lisp_host::CompileResult {
                    manifest: manifest.clone(),
                    lib: lisp_host::test_loaded_dgen_lib(),
                    lease: None,
                },
            )
            .expect("initial saved instrument track should load");
        assert_eq!(initial_track, 0);
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        graph.process_block();

        let cached_engine_id = app.cache_instrument_engine(
            "bank/cached",
            "cached source",
            &manifest,
            lisp_host::test_loaded_dgen_lib(),
            None,
        );
        let cached_summary = app
            .try_swap_track_to_cached_saved_instrument_sync(
                0,
                "bank/cached",
                "cached source",
                CustomInstrumentRunMode::Instrument,
            )
            .expect("cached instrument should be found")
            .expect("cached swap should succeed");
        // Scene pattern + track-sound carrier.
        assert_eq!(cached_summary.patterns_reset, 2);
        assert_eq!(app.graph.track_engine_ids, vec![Some(cached_engine_id)]);
        assert_eq!(app.tracks, vec!["cached"]);
        assert!(app.graph.engine_node_ids[0].is_none());
        graph.process_block();

        let compiled_summary = app
            .swap_track_to_compiled_saved_instrument_sync(
                0,
                "bank/compiled",
                "compiled source",
                CustomInstrumentRunMode::Instrument,
                lisp_host::CompileResult {
                    manifest,
                    lib: lisp_host::test_loaded_dgen_lib(),
                    lease: None,
                },
            )
            .expect("compiled swap should succeed");
        assert_eq!(compiled_summary.patterns_reset, 2);
        assert_eq!(app.graph.track_engine_ids, vec![Some(2)]);
        assert!(app.graph.engine_node_ids[cached_engine_id].is_none());
        assert_eq!(app.tracks, vec!["compiled"]);
        graph.process_block();

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_engine_ids, vec![Some(cached_engine_id)]);
        assert_eq!(app.tracks, vec!["cached"]);
        graph.process_block();
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(app.tracks, vec!["old"]);
        graph.process_block();
        for _ in 0..3 {
            assert!(matches!(
                crate::app::edit::redo(&mut app),
                crate::app::history::HistoryReplay::Applied(_)
            ));
            graph.process_block();
            assert!(matches!(
                crate::app::edit::undo(&mut app),
                crate::app::history::HistoryReplay::Applied(_)
            ));
            graph.process_block();
        }
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        graph.process_block();
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_engine_ids, vec![Some(2)]);
        assert_eq!(app.tracks, vec!["compiled"]);
        graph.process_block();
    }

    #[test]
    fn saved_instrument_swap_while_playing_publishes_current_scheduler_epochs() {
        let graph = TestLiveGraph::new("playing-saved-instrument-swap-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let manifest = test_instrument_manifest();
        app.add_compiled_saved_instrument_track_sync(
            "old",
            "old source",
            CustomInstrumentRunMode::Instrument,
            lisp_host::CompileResult {
                manifest: manifest.clone(),
                lib: lisp_host::test_loaded_dgen_lib(),
                lease: None,
            },
        )
        .expect("initial saved instrument track should load");
        app.state.start_playback();

        let pattern_epoch_before = app.state.transport.pattern_epoch.load(Ordering::Acquire);
        let topology_epoch_before = app.state.transport.topology_epoch.load(Ordering::Acquire);
        let snapshot_version_before = app.state.scheduler_snapshot_version();
        let new_engine_id = app.cache_instrument_engine(
            "new",
            "new source",
            &manifest,
            lisp_host::test_loaded_dgen_lib(),
            None,
        );

        app.try_swap_track_to_cached_saved_instrument_sync(
            0,
            "new",
            "new source",
            CustomInstrumentRunMode::Instrument,
        )
        .expect("cached instrument should be found")
        .expect("playing track swap should succeed");

        let live_pattern_epoch = app.state.transport.pattern_epoch.load(Ordering::Acquire);
        let live_topology_epoch = app.state.transport.topology_epoch.load(Ordering::Acquire);
        let snapshot = app.state.latest_scheduler_snapshot();
        assert!(snapshot.transport.playing);
        assert_eq!(snapshot.tracks[0].engine_id, Some(new_engine_id));
        assert!(live_pattern_epoch > pattern_epoch_before);
        assert!(live_topology_epoch > topology_epoch_before);
        assert_eq!(
            snapshot.transport.pattern_epoch, live_pattern_epoch,
            "scheduled events must carry the live epoch accepted by the audio callback"
        );
        assert_eq!(snapshot.transport.topology_epoch, live_topology_epoch);
        assert!(app.state.scheduler_snapshot_version() > snapshot_version_before);
        graph.process_block();
    }

    #[test]
    fn instrument_history_replays_custom_sampler_conversion() {
        let graph = TestLiveGraph::new("instrument-history-conversion-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let manifest = test_instrument_manifest();
        app.add_compiled_saved_instrument_track_sync(
            "old",
            "old source",
            CustomInstrumentRunMode::Instrument,
            lisp_host::CompileResult {
                manifest,
                lib: lisp_host::test_loaded_dgen_lib(),
                lease: None,
            },
        )
        .expect("initial custom track should load");
        let buffer_id = crate::instruments::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should allocate");

        app.apply_recorded_instrument_binding_mutation(
            0,
            "Replace instrument",
            |app| {
                app.graph_controller().convert_custom_track_to_sampler(
                    0,
                    buffer_id,
                    44_100,
                    "silent",
                )
            },
        )
        .expect("custom track should convert to sampler");
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Sampler);
        assert_eq!(app.tracks[0], "silent");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(0));
        assert_eq!(app.tracks[0], "old");
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Sampler);
        assert_eq!(app.graph.track_buffer_ids[0], buffer_id);
        assert_eq!(app.tracks[0], "silent");
        graph.process_block();
    }

    #[test]
    fn instrument_history_retains_free_patch_dedicated_engine() {
        let graph = TestLiveGraph::new("instrument-history-free-patch-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let manifest = test_instrument_manifest();
        app.add_compiled_saved_instrument_track_sync(
            "old",
            "old source",
            CustomInstrumentRunMode::Instrument,
            lisp_host::CompileResult {
                manifest: manifest.clone(),
                lib: lisp_host::test_loaded_dgen_lib(),
                lease: None,
            },
        )
        .unwrap();
        app.cache_instrument_engine(
            "free",
            "free source",
            &manifest,
            lisp_host::test_loaded_dgen_lib(),
            None,
        );
        app.try_swap_track_to_cached_saved_instrument_sync(
            0,
            "free",
            "free source",
            CustomInstrumentRunMode::FreePatch,
        )
        .unwrap()
        .unwrap();
        let dedicated_engine = app.graph.track_engine_ids[0].unwrap();
        assert_eq!(
            app.graph.track_instrument_run_modes[0],
            CustomInstrumentRunMode::FreePatch
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_engine_ids[0], Some(0));
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_engine_ids[0], Some(dedicated_engine));
        assert_eq!(
            app.graph.track_instrument_run_modes[0],
            CustomInstrumentRunMode::FreePatch
        );
        graph.process_block();
    }

    #[test]
    fn bus_effect_wiring_resolves_graph_nodes_by_bus_id_after_reordering() {
        let mut app = test_app_with_track_count(0);
        let first_id = crate::sequencer::BusId(41);
        let target_id = crate::sequencer::BusId(73);
        app.buses = vec![
            crate::app::BusChannelState::new(target_id, "Loaded project group"),
            crate::app::BusChannelState::new(first_id, "Previous project group"),
        ];
        app.graph.bus_node_ids = vec![
            crate::app::BusNodeIds {
                id: first_id,
                pdc_id: 0,
                meter_id: 0,
                left_id: 101,
                right_id: 102,
                merge_id: 103,
                gate_id: 104,
                volume_id: 105,
                mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            },
            crate::app::BusNodeIds {
                id: target_id,
                pdc_id: 0,
                meter_id: 0,
                left_id: 201,
                right_id: 202,
                merge_id: 203,
                gate_id: 204,
                volume_id: 205,
                mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            },
        ];

        let (slot_id, predecessor, _, successor, _, existing) = app
            .resolve_bus_effect_slot_wiring(0, 0)
            .expect("loaded project's group bus should resolve by stable id");

        assert_eq!(predecessor, 204, "effect must follow the target bus gate");
        assert_eq!(successor, 205, "effect must precede the target bus volume");
        assert_eq!(existing, None);

        app.buses.swap(0, 1);
        app.graph.bus_node_ids.swap(0, 1);
        let (
            reordered_slot_id,
            reordered_predecessor,
            _,
            reordered_successor,
            _,
            reordered_existing,
        ) = app
            .resolve_bus_effect_slot_wiring(1, 0)
            .expect("the same stable bus id should resolve after both collections reorder");

        assert_eq!(
            reordered_slot_id, slot_id,
            "effect instance identity must be stable across bus reordering"
        );
        assert_eq!(reordered_predecessor, 204);
        assert_eq!(reordered_successor, 205);
        assert_eq!(reordered_existing, None);
    }

    #[test]
    fn project_bus_reconciliation_restores_indexed_mixer_invariant() {
        let mut app = test_app_with_track_count(0);
        let first_id = crate::sequencer::BusId(41);
        let target_id = crate::sequencer::BusId(73);
        app.buses = vec![
            crate::app::BusChannelState::new(target_id, "Loaded project group"),
            crate::app::BusChannelState::new(first_id, "Previous project group"),
        ];
        app.graph.bus_node_ids = vec![
            crate::app::BusNodeIds {
                id: first_id,
                pdc_id: 0,
                meter_id: 0,
                left_id: 101,
                right_id: 102,
                merge_id: 103,
                gate_id: 104,
                volume_id: 105,
                mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            },
            crate::app::BusNodeIds {
                id: target_id,
                pdc_id: 0,
                meter_id: 0,
                left_id: 201,
                right_id: 202,
                merge_id: 203,
                gate_id: 204,
                volume_id: 205,
                mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            },
        ];

        app.graph_controller()
            .reconcile_bus_graph_nodes()
            .expect("project bus graph should reconcile");

        assert_eq!(app.graph.bus_node_ids[0].id, target_id);
        assert_eq!(app.graph.bus_node_ids[0].volume_id, 205);
        assert_eq!(app.graph.bus_node_ids[1].id, first_id);
        assert_eq!(app.graph.bus_node_ids[1].volume_id, 105);
    }

    #[test]
    fn push_all_delay_bpm_updates_str8_delay_node_state() {
        let graph = TestLiveGraph::new("str8-delay-bpm-test", 64, 44_100, 2);
        let name = CString::new("str8_delay_bpm_probe").unwrap();
        let node_id = unsafe {
            crate::audiograph::add_node(
                graph.ptr.0,
                crate::effects::str8_delay::str8_delay_vtable(),
                crate::effects::str8_delay::STR8_DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                name.as_ptr(),
                2 + crate::instruments::voice_modulator::NUM_OUTPUTS as i32,
                2,
                std::ptr::null(),
                0,
            )
        };
        assert!(node_id > 0, "Str8 Delay node should be allocated");
        assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, node_id) });
        graph.process_block();

        let desc = EffectDescriptor::builtin_str8_delay();
        let state = Arc::new(SequencerState::new(
            1,
            vec![vec![crate::effects::EffectSlotState::new(
                &desc,
                node_id as u32,
            )]],
        ));
        state.transport.bpm.store(137, Ordering::Relaxed);

        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        let mut app = App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.effect_descriptors = vec![vec![desc]];

        app.push_all_delay_bpm();

        const MAX_WATCH_REFRESH_BLOCKS: usize = 16;
        let mut observed_bpm = None;
        for _ in 0..MAX_WATCH_REFRESH_BLOCKS {
            graph.process_block();

            let mut state_buf = vec![0.0_f32; crate::effects::str8_delay::STR8_DELAY_STATE_SIZE];
            let mut state_size = 0usize;
            let copied = unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state_buf.as_mut_ptr().cast(),
                    state_buf.len() * std::mem::size_of::<f32>(),
                    &mut state_size,
                )
            };
            assert!(copied, "watched Str8 Delay state should be copied");
            assert_eq!(
                state_size,
                crate::effects::str8_delay::STR8_DELAY_STATE_SIZE * std::mem::size_of::<f32>()
            );
            let bpm = state_buf[crate::effects::str8_delay::STR8_DELAY_PARAM_BPM as usize];
            if (bpm - 137.0).abs() <= f32::EPSILON {
                observed_bpm = Some(bpm);
                break;
            }
        }
        assert_eq!(
            observed_bpm,
            Some(137.0),
            "Str8 Delay BPM should reach watched node state within {MAX_WATCH_REFRESH_BLOCKS} blocks"
        );
    }

    #[test]
    fn add_builtin_effect_publishes_scheduler_descriptor_snapshot() {
        let graph = TestLiveGraph::new("builtin-effect-scheduler-descriptor-test", 64, 44_100, 2);
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        let mut app = App::new(
            Arc::clone(&state),
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id: 0,
            filter_id: 0,
            delay_id: 0,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];

        let slot_idx = app
            .add_builtin_effect_sync(0, "Filter")
            .expect("add built-in filter");
        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(
            snapshot.tracks[0].effect_descriptors[slot_idx].name, "Filter",
            "scheduler snapshot should publish the same descriptor name used by the live graph"
        );
        assert!(
            snapshot.tracks[0].effect_slots[slot_idx].node_id != 0,
            "scheduler snapshot should publish the loaded effect slot state"
        );
    }

    // End-to-end eseq-dtx.2: swapping a live Filter Table to the causal
    // min-phase engine recompiles the node, preserves the loaded table and
    // authoring values, flips the reported latency for PDC, and persists the
    // engine as an `#ft-engine=` suffix on the snapshot table reference.
    #[test]
    fn filter_table_engine_toggle_recompiles_and_preserves_state() {
        use crate::effects::filter_table::{self, TableEngine};

        let _render = filter_table::tests::render_lock();
        let _registry = filter_table::tests::registry_lock();
        let graph = TestLiveGraph::new("filter-table-engine-e2e", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 1);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id: 0,
            filter_id: 0,
            delay_id: 0,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];
        let slot_idx = app
            .add_builtin_effect_sync(0, filter_table::NAME)
            .expect("add Filter Table");
        let node_id = app.state.pattern.effect_chains[0][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert!(node_id > 0, "Filter Table node should be live");
        assert_eq!(filter_table::engine_for(node_id), TableEngine::Spectral);

        // Load a distinctive table and set a distinctive param default so the
        // toggle has real state to preserve.
        let data = (0..filter_table::TABLE_LEN)
            .map(|index| ((index % 11) as f32 / 11.0).max(0.05))
            .collect::<Vec<_>>();
        let table =
            std::sync::Arc::new(filter_table::MagnitudeTable::new(data.clone()).expect("valid"));
        app.apply_prepared_filter_table_to_node(
            node_id,
            table,
            "engine-e2e-table",
            std::path::Path::new("engine-e2e-table"),
        )
        .expect("table applies");
        let cutoff_idx = app.graph.effect_descriptors[0][slot_idx]
            .params
            .iter()
            .position(|param| param.name == filter_table::PARAM_CUTOFF)
            .expect("cutoff param");
        app.state.pattern.effect_chains[0][slot_idx]
            .defaults
            .set(cutoff_idx, 4321.0);

        let changed = app
            .set_track_filter_table_engine(0, slot_idx, TableEngine::Causal)
            .expect("engine swap");
        assert!(changed, "swap to a different engine must recompile");
        let causal_node = app.state.pattern.effect_chains[0][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert!(causal_node > 0, "recompiled node should be live");
        assert_eq!(filter_table::engine_for(causal_node), TableEngine::Causal);
        assert_eq!(
            app.graph.effect_descriptors[0][slot_idx].latency_samples(causal_node),
            0,
            "causal engine reports zero latency for PDC"
        );
        assert_eq!(
            filter_table::persisted_table_ref_for(causal_node).as_deref(),
            Some("engine-e2e-table#ft-engine=causal"),
            "persisted reference carries the engine suffix"
        );
        let preserved = filter_table::prepared_table_for(causal_node).expect("table survives");
        assert_eq!(preserved.data.as_slice(), data.as_slice());
        let restored_cutoff =
            app.state.pattern.effect_chains[0][slot_idx].defaults.get(cutoff_idx);
        assert_eq!(restored_cutoff, 4321.0, "authoring values survive the swap");

        // Same engine again is a no-op; toggling back restores the spectral
        // latency and drops the persisted suffix.
        assert!(!app
            .set_track_filter_table_engine(0, slot_idx, TableEngine::Causal)
            .expect("no-op swap"));
        assert!(app
            .set_track_filter_table_engine(0, slot_idx, TableEngine::Spectral)
            .expect("swap back"));
        let spectral_node = app.state.pattern.effect_chains[0][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert_eq!(filter_table::engine_for(spectral_node), TableEngine::Spectral);
        assert_eq!(
            app.graph.effect_descriptors[0][slot_idx].latency_samples(spectral_node),
            filter_table::N as u32
        );
        assert_eq!(
            filter_table::persisted_table_ref_for(spectral_node).as_deref(),
            Some("engine-e2e-table"),
            "default engine persists without a suffix"
        );

        // The per-node registries are global and keyed by graph node id;
        // TestLiveGraphs reuse ids across tests, so leaving entries behind
        // makes later tests' snapshot captures see this test's table.
        filter_table::clear_instance(spectral_node);
        filter_table::clear_instance(causal_node);
        filter_table::clear_instance(node_id);
    }

    // End-to-end eseq-dtx.6: a baked .fltab asset used as a Filter Table
    // source on a live graph — no analysis runs, the recorded reference is the
    // deterministic `fltab:<stem>` form, and the undo memento retains the
    // prepared bank for fileless redo.
    #[test]
    fn filter_table_asset_loads_as_source_with_deterministic_reference() {
        use crate::effects::{filter_table, filter_table_asset};

        // The live graph runs the same cached effect dylib as the offline
        // render tests; the compiled code is not reentrant (eseq-dtx.9), so
        // hold the shared render lock for the whole live-graph session.
        let _render = filter_table::tests::render_lock();
        let _registry = filter_table::tests::registry_lock();
        let graph = TestLiveGraph::new("filter-table-asset-e2e", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 1);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id: 0,
            filter_id: 0,
            delay_id: 0,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];
        let slot_idx = app
            .add_builtin_effect_sync(0, filter_table::NAME)
            .expect("add Filter Table");
        let node_id = app.state.pattern.effect_chains[0][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert!(node_id > 0, "Filter Table node should be live");

        let data = (0..filter_table::TABLE_LEN)
            .map(|index| (index % 7) as f32 / 7.0)
            .collect::<Vec<_>>();
        let table = filter_table::MagnitudeTable::new(data.clone()).expect("valid magnitudes");
        let dir = std::env::temp_dir().join(format!("fltab-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("asset dir");
        let asset_path = dir.join("morph-pack.fltab");
        let mut meta = filter_table_asset::FilterTableAssetMeta::new("Morph Pack");
        meta.source_name = Some("synthetic".to_string());
        filter_table_asset::write_asset(&asset_path, &meta, &table).expect("write asset");

        app.set_filter_table_source(0, slot_idx, &asset_path, "morph-pack")
            .expect("asset loads as table source");

        assert_eq!(
            filter_table::table_ref_for(node_id).as_deref(),
            Some("fltab:morph-pack"),
            "asset loads persist the deterministic fltab reference"
        );
        assert_eq!(
            filter_table::table_name_for(node_id).as_deref(),
            Some("morph-pack")
        );
        let prepared = filter_table::prepared_table_for(node_id).expect("prepared bank");
        assert_eq!(
            prepared.data.as_slice(),
            data.as_slice(),
            "the baked payload must reach the device without re-analysis"
        );
        assert_eq!(
            app.filter_table_source_info(Some(0), None, slot_idx),
            Some(("fltab:morph-pack".to_string(), None)),
            "asset sources report no analysis mode"
        );

        let snapshot = crate::effects::EffectSlotSnapshot::capture_authoring_values(
            &app.state.pattern.effect_chains[0][slot_idx],
        );
        assert_eq!(snapshot.table.as_deref(), Some("fltab:morph-pack"));
        assert!(Arc::ptr_eq(
            snapshot.prepared_table.as_ref().expect("prepared memento"),
            &prepared,
        ));

        filter_table::clear_instance(node_id);
    }

    // End-to-end eseq-dtx.8: a live editor session on a real graph node —
    // create, edit with live audition, undo/redo, save as an asset that
    // round-trips the nondestructive document, and close-with-rollback.
    #[test]
    fn filter_table_editor_session_edits_audition_save_and_restore() {
        use crate::effects::filter_table_editor::{EditOp, EditorTarget};
        use crate::effects::{filter_table, filter_table_asset, filter_table_editor};

        // Hold the shared render lock too: the live graph executes the same
        // non-reentrant cached effect dylib as the offline render tests
        // (eseq-dtx.9).
        let _render = filter_table::tests::render_lock();
        let _registry = filter_table::tests::registry_lock();
        let graph = TestLiveGraph::new("filter-table-editor-e2e", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 1);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id: 0,
            filter_id: 0,
            delay_id: 0,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];
        let slot_idx = app
            .add_builtin_effect_sync(0, filter_table::NAME)
            .expect("add Filter Table");
        let node_id = app.state.pattern.effect_chains[0][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert!(node_id > 0, "Filter Table node should be live");
        let original = filter_table::prepared_table_for(node_id).expect("default table");
        let original_ref = filter_table::table_ref_for(node_id).expect("default ref");

        let target = EditorTarget::Track {
            track: 0,
            slot: slot_idx,
        };
        app.open_filter_table_editor(target).expect("open editor");
        let ui = filter_table_editor::session_ui_state().expect("session state");
        assert_eq!(ui.frames, filter_table::FRAMES);
        assert!(!ui.can_undo && !ui.can_redo && !ui.dirty);

        // Edit + live audition: the published visualization bank must be
        // bit-identical to the document's baked runtime table (displayed
        // response == runtime response, zero tolerance).
        app.filter_table_editor_apply_op(
            EditOp::Tilt {
                frame_start: 0,
                frame_end: 63,
                db_per_octave: -6.0,
            },
            false,
        )
        .expect("apply tilt");
        let baked = filter_table_editor::with_session(|session| {
            session.expect("session").doc.bake().expect("bake")
        });
        let bank = eseqlisp::widget_render::wavetable_viewer::published_bank(
            &filter_table::visualization_key(node_id),
        )
        .expect("published editor bank");
        assert_eq!(
            bank.data.as_slice(),
            baked.data.as_slice(),
            "displayed bank must match the baked runtime table bit-exactly"
        );
        assert_ne!(
            bank.data.as_slice(),
            original.data.as_slice(),
            "the tilt must actually change the table"
        );
        // Previews must not touch the prepared-table registry (app undo and
        // persistence keep seeing the device's real table until save).
        assert!(Arc::ptr_eq(
            &filter_table::prepared_table_for(node_id).expect("prepared"),
            &original,
        ));

        // Editor-document undo restores the pre-edit audition; redo re-applies.
        assert!(app.filter_table_editor_history(false).expect("undo"));
        let bank = eseqlisp::widget_render::wavetable_viewer::published_bank(
            &filter_table::visualization_key(node_id),
        )
        .expect("bank after undo");
        let unedited_bake = filter_table_editor::with_session(|session| {
            session.expect("session").doc.bake().expect("bake")
        });
        assert_eq!(bank.data.as_slice(), unedited_bake.data.as_slice());
        assert!(app.filter_table_editor_history(true).expect("redo"));

        // Save: writes a .fltab whose recipe restores the document, and
        // loads it into the device through the recorded-mutation path.
        let dir = std::env::temp_dir().join(format!("fltab-editor-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("save dir");
        let stem = app
            .filter_table_editor_save_in(Some("My Edited Table"), &dir)
            .expect("save");
        assert_eq!(stem, "my-edited-table", "names sanitize to safe stems");
        assert_eq!(
            filter_table::table_ref_for(node_id).as_deref(),
            Some("fltab:my-edited-table"),
            "save rebinds the device to the asset reference"
        );
        let prepared = filter_table::prepared_table_for(node_id).expect("prepared after save");
        assert_eq!(
            prepared.data.as_slice(),
            baked.data.as_slice(),
            "the device now owns the baked edit"
        );
        let asset =
            filter_table_asset::read_asset(&dir.join("my-edited-table.fltab")).expect("read back");
        let restored_doc = crate::effects::filter_table_editor::EditorDoc::from_snapshot(
            asset.meta.recipe.as_ref().expect("editor recipe"),
        )
        .expect("document restores from the asset recipe");
        assert_eq!(restored_doc.op_count(), 1, "the tilt survives the save");
        assert_eq!(
            restored_doc.bake().expect("restored bake").data.as_slice(),
            baked.data.as_slice(),
            "restored document bakes bit-exactly"
        );

        // Closing after save keeps the saved table (session is clean).
        app.close_filter_table_editor().expect("close clean");
        assert!(filter_table_editor::session_ui_state().is_none());
        assert_eq!(
            filter_table::table_ref_for(node_id).as_deref(),
            Some("fltab:my-edited-table"),
        );

        // Reopen, dirty the session, close without saving: the device rolls
        // back to what it had when the editor opened.
        app.open_filter_table_editor(target).expect("reopen");
        app.filter_table_editor_apply_op(
            EditOp::Normalize {
                frame_start: 0,
                frame_end: 63,
            },
            false,
        )
        .expect("dirty edit");
        app.close_filter_table_editor().expect("close dirty");
        assert_eq!(
            filter_table::table_ref_for(node_id).as_deref(),
            Some("fltab:my-edited-table"),
            "rollback restores the table the session opened with"
        );
        assert_eq!(
            filter_table::prepared_table_for(node_id)
                .expect("prepared after rollback")
                .data
                .as_slice(),
            prepared.data.as_slice(),
        );

        // App-level undo of the save returns the pre-save table (the save
        // rode the recorded-mutation path).
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            filter_table::table_ref_for(node_id).as_deref(),
            Some(original_ref.as_str()),
            "app undo rolls the device back to the pre-save reference"
        );

        filter_table::clear_instance(node_id);
        filter_table_editor::set_session(None);
    }

    #[test]
    fn bus_builtin_effects_add_move_and_delete_through_shared_host() {
        let graph = TestLiveGraph::new("bus-fx-chain-host-lifecycle-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let bus_id = app.add_bus_channel("FX Test");
        let bus_idx = app
            .buses
            .iter()
            .position(|bus| bus.id == bus_id)
            .expect("new bus should exist");

        let filter_slot = app
            .add_builtin_bus_effect_sync(bus_idx, "Filter")
            .expect("filter should install on bus host");
        let ott_slot = app
            .add_builtin_bus_effect_sync(bus_idx, "OTT")
            .expect("OTT should install after filter");
        assert!(app.buses[bus_idx].effect_slots[filter_slot].node_id > 0);
        assert!(app.buses[bus_idx].effect_slots[ott_slot].node_id > 0);

        let moved_slot = app
            .move_bus_effect_slot_sync(bus_idx, ott_slot, Some(filter_slot))
            .expect("bus effect move should rewire through the shared host");
        assert_eq!(moved_slot, filter_slot);
        assert_eq!(
            app.buses[bus_idx].effect_descriptors[moved_slot].name,
            "OTT"
        );

        app.delete_bus_effect_slot(bus_idx, moved_slot)
            .expect("bus effect delete should rewire through the shared host");
        assert_eq!(app.buses[bus_idx].effect_descriptors[moved_slot].name, "Filter");
        assert!(app.buses[bus_idx].effect_slots[moved_slot].node_id > 0);
        assert_eq!(app.buses[bus_idx].effect_slots[moved_slot + 1].node_id, 0);
        graph.process_block();
    }

    #[test]
    fn recorded_bus_effect_insert_places_new_effect_before_target_without_duplication() {
        let graph = TestLiveGraph::new("bus-fx-recorded-insert-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let bus_id = app.add_bus_channel("Insert Test");
        let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id).unwrap();
        let filter_slot = app
            .add_builtin_bus_effect_sync(bus_idx, "Filter")
            .expect("filter should install first");
        let ott_slot = app
            .add_builtin_bus_effect_sync(bus_idx, "OTT")
            .expect("OTT should install second");
        let filter_node = app.buses[bus_idx].effect_slots[filter_slot].node_id;
        let ott_node = app.buses[bus_idx].effect_slots[ott_slot].node_id;

        let inserted = app
            .apply_recorded_bus_effect_chain_mutation(bus_idx, "Insert bus effect", |app| {
                app.insert_builtin_bus_effect_before_slot_sync(bus_idx, ott_slot, "Phaser-Flanger")
            })
            .expect("recorded insert should succeed");

        assert_eq!(inserted, 1);
        assert_eq!(app.buses[bus_idx].effect_descriptors[0].name, "Filter");
        assert_eq!(app.buses[bus_idx].effect_descriptors[1].name, "Phaser-Flanger");
        assert_eq!(app.buses[bus_idx].effect_descriptors[2].name, "OTT");
        assert_eq!(app.buses[bus_idx].effect_slots[0].node_id, filter_node);
        assert_eq!(app.buses[bus_idx].effect_slots[2].node_id, ott_node);
        assert_eq!(
            app.buses[bus_idx]
                .effect_slots
                .iter()
                .take(3)
                .map(|slot| slot.node_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "every effect in the inserted chain must retain a distinct live node",
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_descriptors[0].name, "Filter");
        assert_eq!(app.buses[bus_idx].effect_descriptors[1].name, "OTT");
        assert_eq!(app.buses[bus_idx].effect_slots[0].node_id, filter_node);
        assert_eq!(app.buses[bus_idx].effect_slots[1].node_id, ott_node);

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_descriptors[0].name, "Filter");
        assert_eq!(app.buses[bus_idx].effect_descriptors[1].name, "Phaser-Flanger");
        assert_eq!(app.buses[bus_idx].effect_descriptors[2].name, "OTT");
        graph.process_block();
    }

    #[test]
    fn recorded_track_effect_insert_places_new_effect_before_target() {
        let graph = TestLiveGraph::new("track-fx-recorded-insert-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("sampler track should be created");
        let filter_slot = app
            .add_builtin_effect_sync(0, "Filter")
            .expect("filter should install first");
        let ott_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("OTT should install second");
        let track_id = app.track_registry.id_at(0).unwrap();
        let filter_id = app.device_registry.audio_effect(track_id, filter_slot);
        let ott_id = app.device_registry.audio_effect(track_id, ott_slot);

        let inserted = app
            .apply_recorded_track_effect_chain_mutation(0, "Insert audio effect", |app| {
                app.insert_builtin_effect_before_slot_sync(0, ott_slot, "Phaser-Flanger")
            })
            .expect("recorded insert should succeed");

        assert_eq!(inserted, ott_slot);
        assert_eq!(app.graph.effect_descriptors[0][filter_slot].name, "Filter");
        assert_eq!(app.graph.effect_descriptors[0][inserted].name, "Phaser-Flanger");
        assert_eq!(app.graph.effect_descriptors[0][inserted + 1].name, "OTT");
        assert_eq!(app.device_registry.audio_effect_location(filter_id), Some((track_id, filter_slot)));
        assert_eq!(app.device_registry.audio_effect_location(ott_id), Some((track_id, inserted + 1)));
        graph.process_block();
    }

    #[test]
    fn recorded_rack_slot_effect_insert_places_new_effect_before_target() {
        let graph = TestLiveGraph::new("rack-fx-recorded-insert-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        app.graph_controller()
            .add_sampler_rack_track(&[std::path::Path::new(
                "assets/ir/lexicon-300-rich-plate.wav",
            )
            .to_path_buf()])
            .expect("sampler rack should be created");
        let filter_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "Filter")
            .expect("filter should install first");
        let ott_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("OTT should install second");
        let track_id = app.track_registry.id_at(0).unwrap();
        let rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        let filter_id = app.device_registry.rack_audio_effect(rack_slot_id, filter_slot);
        let ott_id = app.device_registry.rack_audio_effect(rack_slot_id, ott_slot);

        let inserted = app
            .apply_recorded_rack_effect_chain_mutation(0, 0, "Insert rack-slot effect", |app| {
                app.insert_builtin_rack_slot_effect_before_slot_sync(
                    0,
                    0,
                    ott_slot,
                    "Phaser-Flanger",
                )
            })
            .expect("recorded insert should succeed");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack should remain live");
        assert_eq!(inserted, ott_slot);
        assert_eq!(rack.slots[0].effect_descriptors[filter_slot].name, "Filter");
        assert_eq!(rack.slots[0].effect_descriptors[inserted].name, "Phaser-Flanger");
        assert_eq!(rack.slots[0].effect_descriptors[inserted + 1].name, "OTT");
        assert_eq!(
            app.device_registry.rack_audio_effect_location(filter_id),
            Some((rack_slot_id, filter_slot)),
        );
        assert_eq!(
            app.device_registry.rack_audio_effect_location(ott_id),
            Some((rack_slot_id, inserted + 1)),
        );
        graph.process_block();
    }

    #[test]
    fn recorded_group_delete_restores_backing_bus_fx_and_all_scene_routing() {
        let graph = TestLiveGraph::new("recorded-group-delete-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        app.graph_controller().add_blank_sampler_track()
            .expect("first sampler track");
        app.graph_controller().add_blank_sampler_track()
            .expect("second sampler track");
        let bus_id = app.group_tracks_recorded(vec![0, 1])
            .expect("tracks should group");
        let group_id = app.groups[0].id;
        let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id)
            .expect("group backing bus");
        app.add_builtin_bus_effect_sync(bus_idx, "OTT")
            .expect("group bus should accept an effect");
        let effect_id = app.device_registry.bus_audio_effect(bus_id, 0);

        app.delete_group_recorded(group_id)
            .expect("group deletion should record");
        assert!(app.groups.is_empty());
        assert!(app.buses.iter().all(|bus| bus.id != bus_id));

        let undo = crate::app::edit::undo(&mut app);
        assert!(
            matches!(undo, crate::app::history::HistoryReplay::Applied(_)),
            "group delete undo failed: {undo:?}",
        );
        let restored_idx = app.buses.iter().position(|bus| bus.id == bus_id)
            .expect("undo should restore the same bus id");
        assert_eq!(app.groups[0].id, group_id);
        assert_eq!(app.groups[0].members, vec![0, 1]);
        assert_eq!(app.buses[restored_idx].effect_descriptors[0].name, "OTT");
        assert_eq!(app.device_registry.bus_audio_effect(bus_id, 0), effect_id);
        for track in 0..2 {
            assert_eq!(
                app.state.with_scene_track_pattern(0, track, |pattern| {
                    pattern.track_params.output.clone()
                }),
                Some(crate::sequencer::TrackOutput::Bus(bus_id)),
            );
        }

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(app.groups.is_empty());
        assert!(app.buses.iter().all(|bus| bus.id != bus_id));
        graph.process_block();
    }

    #[test]
    fn recorded_bus_effect_chain_restores_stable_identity_and_scene_values() {
        let graph = TestLiveGraph::new("bus-fx-history-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let bus_id = app.add_bus_channel("History Bus");
        let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id).unwrap();

        let slot = app
            .apply_recorded_bus_effect_chain_mutation(bus_idx, "Add bus effect", |app| {
                app.add_builtin_bus_effect_sync(bus_idx, "Filter")
            })
            .expect("recorded bus filter add should succeed");
        let instance = app.device_registry.bus_audio_effect(bus_id, slot);
        app.buses[bus_idx].effect_slots[slot].defaults[0] = 0.37;
        app.save_current_bus_pattern();

        app.apply_recorded_bus_effect_chain_mutation(bus_idx, "Delete bus effect", |app| {
            app.delete_bus_effect_slot(bus_idx, slot)
        })
        .expect("recorded bus filter delete should succeed");
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_descriptors[slot].name, "Filter");
        assert_eq!(app.buses[bus_idx].effect_slots[slot].defaults[0].to_bits(), 0.37_f32.to_bits());
        assert_eq!(
            app.device_registry.bus_audio_effect_location(instance),
            Some((bus_id, slot))
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_slots[slot].node_id, 0);
        assert!(app.device_registry.bus_audio_effect_location(instance).is_none());
        graph.process_block();
    }

    #[test]
    fn recorded_bus_effect_parameter_drag_coalesces_and_replays() {
        let graph = TestLiveGraph::new("bus-fx-value-history-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let bus_id = app.add_bus_channel("Value Bus");
        let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id).unwrap();
        let slot = app.apply_recorded_bus_effect_chain_mutation(
            bus_idx,
            "Add bus effect",
            |app| app.add_builtin_bus_effect_sync(bus_idx, "Filter"),
        ).unwrap();
        let param = 2;
        let before = app.buses[bus_idx].effect_slots[slot].defaults[param];

        for value in [500.0, 1_000.0, 2_000.0] {
            app.apply_recorded_bus_effect_value_mutation(
                bus_idx,
                slot,
                "Set bus effect parameter",
                format!("param:{param}"),
                |app| app.set_bus_effect_param(bus_idx, slot, param, value),
            ).unwrap();
        }
        crate::app::edit::finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 2, "the drag should add one history entry");
        assert_eq!(app.buses[bus_idx].effect_slots[slot].defaults[param].to_bits(), 2_000.0_f32.to_bits());
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_slots[slot].defaults[param].to_bits(), before.to_bits());
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.buses[bus_idx].effect_slots[slot].defaults[param].to_bits(), 2_000.0_f32.to_bits());
        graph.process_block();
    }

    #[test]
    fn bus_effect_value_history_follows_stable_scene_identity_after_reorder() {
        let graph = TestLiveGraph::new("bus-fx-scene-identity-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 0);
        let bus_id = app.add_bus_channel("Scene Identity Bus");
        let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id).unwrap();
        let slot = app.apply_recorded_bus_effect_chain_mutation(
            bus_idx,
            "Add bus effect",
            |app| app.add_builtin_bus_effect_sync(bus_idx, "Filter"),
        ).unwrap();
        let original_scene = app.state.current_scene_id().unwrap();
        let param = 2;
        let before = app.buses[bus_idx].effect_slots[slot].defaults[param];
        app.apply_recorded_bus_effect_value_mutation(
            bus_idx,
            slot,
            "Set bus effect parameter",
            format!("param:{param}"),
            |app| app.set_bus_effect_param(bus_idx, slot, param, 4_000.0),
        ).unwrap();
        crate::app::edit::finish_active_gesture(&mut app);

        app.state.clone_pattern(0, &[], &[], &[], &[]);
        assert_eq!(app.state.reorder_scene(0, 1), Some(0));
        let original_scene_idx = app.state.scene_index(original_scene).unwrap();
        assert_eq!(original_scene_idx, 1);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let live = app.capture_bus_pattern_snapshot();
        let repository = app.state.export_bus_pattern_repository(&live);
        let original_bus = repository[original_scene_idx]
            .iter()
            .find(|bus| bus.id == bus_id)
            .unwrap();
        assert_eq!(
            original_bus.effect_defaults[slot][param].to_bits(),
            before.to_bits(),
            "undo must update the original scene after its dense index changes"
        );
        graph.process_block();
    }

    #[test]
    fn track_builtin_effects_add_move_and_delete_through_shared_host() {
        let graph = TestLiveGraph::new("track-fx-chain-host-lifecycle-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 1);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        let pan_id = graph.add_gain(1.0, "track_fx_pan");
        let delay_name = CString::new("track_fx_delay").unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                graph.ptr.0,
                crate::effects::delay::delay_vtable(),
                crate::effects::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        assert!(delay_id > 0);
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id,
            filter_id: 0,
            delay_id,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];

        let filter_slot = app
            .add_builtin_effect_sync(0, "Filter")
            .expect("filter should install on track host");
        let ott_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("OTT should install after filter");
        let filter_node_id = app.state.pattern.effect_chains[0][filter_slot]
            .node_id
            .load(Ordering::Relaxed);
        let moved_slot = app
            .move_effect_slot_sync(0, ott_slot, Some(filter_slot))
            .expect("track effect move should rewire through the shared host");
        assert_eq!(moved_slot, filter_slot);
        assert_eq!(app.graph.effect_descriptors[0][moved_slot].name, "OTT");

        app.graph_controller()
            .delete_custom_effect_slot(0, moved_slot)
            .expect("track effect delete should rewire through the shared host");
        assert_eq!(
            app.state.pattern.effect_chains[0][moved_slot]
                .node_id
                .load(Ordering::Relaxed),
            filter_node_id,
            "deleting the moved first slot should shift the remaining filter into it",
        );
        assert_eq!(app.graph.effect_descriptors[0][moved_slot].name, "Filter");
        graph.process_block();
    }

    #[test]
    fn recorded_track_effect_add_undo_redo_restores_stable_instance() {
        let graph = TestLiveGraph::new("track-fx-history-add-test", 64, 44_100, 2);
        let mut app = test_app_for_live_graph(&graph, 1);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        let pan_id = graph.add_gain(1.0, "track_fx_history_pan");
        let delay_name = CString::new("track_fx_history_delay").unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                graph.ptr.0,
                crate::effects::delay::delay_vtable(),
                crate::effects::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                2,
                2,
                std::ptr::null(),
                0,
            )
        };
        app.graph.track_node_ids = vec![crate::app::TrackNodeIds {
            sampler_ids: Vec::new(),
            pdc_id: 0,
            sampler_gatepitch_ids: Vec::new(),
            sampler_modulator_ids: Vec::new(),
            voice_sum_id: 0,
            voice_sum_r_id: 0,
            pan_id,
            filter_id: 0,
            delay_id,
            send_id: 0,
            mod_out_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
            mod_env_id: 0,
            bus_send_ids: Vec::new(),
            rack_slots: Vec::new(),
            rack_signature: None,
        }];
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.instrument_descriptors = vec![EffectDescriptor::builtin_sampler()];

        let slot = app
            .apply_recorded_track_effect_chain_mutation(0, "Add audio effect", |app| {
                app.add_builtin_effect_sync(0, "Filter")
            })
            .expect("recorded filter add should succeed");
        let track_id = app.track_registry.id_at(0).unwrap();
        let instance = app.device_registry.audio_effect(track_id, slot);
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.effect_chains[0][slot]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert!(app.device_registry.audio_effect_location(instance).is_none());

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.effect_descriptors[0][slot].name, "Filter");
        assert_eq!(
            app.device_registry.audio_effect_location(instance),
            Some((track_id, slot))
        );

        let ott_slot = app
            .apply_recorded_track_effect_chain_mutation(0, "Add audio effect", |app| {
                app.add_builtin_effect_sync(0, "OTT")
            })
            .expect("recorded OTT add should succeed");
        let ott_instance = app.device_registry.audio_effect(track_id, ott_slot);
        app.apply_recorded_track_effect_chain_mutation(0, "Move audio effect", |app| {
            app.move_effect_slot_sync(0, ott_slot, Some(slot))
        })
        .expect("recorded effect move should succeed");
        assert_eq!(
            app.device_registry.audio_effect_location(ott_instance),
            Some((track_id, slot)),
            "the moved logical effect keeps its identity"
        );
        assert_eq!(
            app.device_registry.audio_effect_location(instance),
            Some((track_id, ott_slot)),
            "the displaced effect identity follows its new slot"
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.device_registry.audio_effect_location(instance),
            Some((track_id, slot))
        );
        assert_eq!(
            app.device_registry.audio_effect_location(ott_instance),
            Some((track_id, ott_slot))
        );

        let retained_value = 0.37_f32;
        app.state.pattern.effect_chains[0][slot]
            .defaults
            .set(0, retained_value);
        app.delete_custom_effect_slot_recorded(0, slot)
            .expect("recorded effect delete should succeed");
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.effect_chains[0][slot].defaults.get(0),
            retained_value,
            "undoing delete restores the effect's authoring values"
        );
        assert_eq!(
            app.device_registry.audio_effect_location(instance),
            Some((track_id, slot))
        );
        app.apply_recorded_track_effect_chain_mutation(0, "Replace audio effect", |app| {
            app.load_builtin_effect_to_slot_sync(0, slot, "OTT")
        })
        .expect("recorded source replacement should succeed");
        assert_eq!(app.graph.effect_descriptors[0][slot].name, "OTT");
        assert_eq!(
            app.device_registry.audio_effect_location(instance),
            Some((track_id, slot)),
            "source replacement keeps the logical instance identity"
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.effect_descriptors[0][slot].name, "Filter");
        assert_eq!(app.state.pattern.effect_chains[0][slot].defaults.get(0), retained_value);
        graph.process_block();
    }

    #[test]
    fn sidechain_effect_descriptor_uses_other_tracks_as_options() {
        let app = test_app_with_track_count(2);
        let result = app
            .compile_saved_effect("sidechain")
            .expect("compile sidechain effect");
        let desc = app.build_effect_descriptor(0, "sidechain", &result.manifest);
        let sidechain = desc
            .params
            .iter()
            .find(|param| param.name == "sidechain")
            .expect("sidechain effect descriptor should expose sidechain selector");
        let ParamKind::Enum { labels } = &sidechain.kind else {
            panic!("sidechain selector should be an enum param");
        };
        assert_eq!(labels, &vec!["off".to_string(), "Track 2".to_string()]);
    }

    #[test]
    fn add_midi_fx_to_track_publishes_snapshot_without_deadlocking_pattern_bank() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut app = test_app_with_track();
            let _ = ready_tx.send(());
            let result = app.add_midi_fx_to_track_sync(0, "arp");
            let published_chain = app
                .state
                .latest_scheduler_snapshot()
                .tracks
                .first()
                .map(|track| track.params.midi_fx_chain.clone())
                .unwrap_or_default();
            let _ = done_tx.send((result, published_chain));
        });

        ready_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("test app should initialize");
        let (result, published_chain) = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("adding MIDI FX should not block on pattern_bank");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(published_chain, vec!["arp".to_string()]);
    }

    #[test]
    fn recorded_midi_fx_chain_restores_order_values_and_stable_ids() {
        let mut app = test_app_with_track();
        let track_id = app.track_registry.id_at(0).unwrap();
        let arp_slot = app
            .apply_recorded_track_midi_fx_chain_mutation(0, "Add MIDI FX", |app| {
                app.add_midi_fx_to_track_sync(0, "arp")
            })
            .unwrap();
        let arp_id = app.device_registry.midi_effect(track_id, arp_slot);
        let trigger_slot = app
            .apply_recorded_track_midi_fx_chain_mutation(0, "Add MIDI FX", |app| {
                app.add_midi_fx_to_track_sync(0, "trigger-to-track")
            })
            .unwrap();
        let trigger_id = app.device_registry.midi_effect(track_id, trigger_slot);
        app.state.pattern.midi_fx_slots[0][arp_slot]
            .defaults
            .set(0, 0.42);

        app.apply_recorded_track_midi_fx_chain_mutation(0, "Move MIDI FX", |app| {
            app.move_midi_fx_slot_sync(0, trigger_slot, Some(arp_slot))
        })
        .unwrap();
        assert_eq!(
            app.device_registry.midi_effect_location(trigger_id),
            Some((track_id, arp_slot))
        );
        assert_eq!(
            app.device_registry.midi_effect_location(arp_id),
            Some((track_id, trigger_slot))
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.device_registry.midi_effect_location(arp_id),
            Some((track_id, arp_slot))
        );
        assert_eq!(app.state.pattern.midi_fx_slots[0][arp_slot].defaults.get(0), 0.42);

        app.apply_recorded_track_midi_fx_chain_mutation(0, "Delete MIDI FX", |app| {
            app.delete_midi_fx_slot(0, arp_slot)
        })
        .unwrap();
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.device_registry.midi_effect_location(arp_id),
            Some((track_id, arp_slot))
        );
        assert_eq!(app.state.pattern.midi_fx_slots[0][arp_slot].defaults.get(0), 0.42);
        app.apply_recorded_track_midi_fx_chain_mutation(0, "Replace MIDI FX", |app| {
            app.replace_midi_fx_slot_sync(0, arp_slot, "trigger-to-track")
        })
        .unwrap();
        assert_eq!(
            app.device_registry.midi_effect_location(arp_id),
            Some((track_id, arp_slot)),
            "MIDI-FX source replacement preserves instance identity"
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.track_params[0].midi_fx_chain()[arp_slot],
            "arp"
        );
        assert_eq!(app.state.pattern.midi_fx_slots[0][arp_slot].defaults.get(0), 0.42);
    }
}
