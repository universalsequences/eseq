use std::sync::Arc;

use crossterm::event::KeyCode;
use std::ffi::CString;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, HostControl, ParamDescriptor, ParamKind, ParamScaling,
    BUILTIN_SLOT_COUNT,
};
use crate::lisp_host::{self, MAX_CUSTOM_FX, MAX_MIDI_FX_SLOTS};
use crate::sequencer::{CustomInstrumentRunMode, InstrumentType};
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

#[derive(Clone, Copy)]
struct CustomEffectEdge {
    source_id: i32,
    source_channels: usize,
    dest_id: i32,
    dest_channels: usize,
}

fn adapted_audio_port_connections(
    source_channels: usize,
    destination_channels: usize,
) -> Vec<(i32, i32)> {
    let source_channels = source_channels.max(1).min(2);
    let destination_channels = destination_channels.max(1).min(2);
    match (source_channels, destination_channels) {
        (1, 2) => vec![(0, 0), (0, 1)],
        (2, 1) => vec![(0, 0), (1, 0)],
        _ => (0..source_channels.min(destination_channels))
            .map(|channel| (channel as i32, channel as i32))
            .collect(),
    }
}

fn instrument_display_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}

fn empty_track_effect_lease_slots() -> Vec<Option<lisp_host::DylibLease>> {
    std::iter::repeat_with(|| None)
        .take(BUILTIN_SLOT_COUNT + MAX_CUSTOM_FX)
        .collect()
}

fn empty_bus_effect_lease_slots() -> Vec<Option<lisp_host::DylibLease>> {
    std::iter::repeat_with(|| None)
        .take(MAX_CUSTOM_FX)
        .collect()
}

fn insert_empty_lease_slot<T>(row: &mut [Option<T>], slot_idx: usize) {
    if slot_idx >= row.len() {
        return;
    }
    let last = row.len().saturating_sub(1);
    for idx in (slot_idx + 1..=last).rev() {
        row[idx] = row[idx - 1].take();
    }
    row[slot_idx] = None;
}

fn remove_lease_slot<T>(row: &mut [Option<T>], slot_idx: usize) {
    if slot_idx >= row.len() {
        return;
    }
    row[slot_idx] = None;
    for idx in slot_idx..row.len().saturating_sub(1) {
        row[idx] = row[idx + 1].take();
    }
    if let Some(last) = row.last_mut() {
        *last = None;
    }
}

fn move_lease_slot<T>(row: &mut [Option<T>], source_slot: usize, target_slot: usize) {
    if source_slot >= row.len() || target_slot >= row.len() || source_slot == target_slot {
        return;
    }
    let lease = row[source_slot].take();
    if source_slot < target_slot {
        for idx in source_slot..target_slot {
            row[idx] = row[idx + 1].take();
        }
    } else {
        for idx in (target_slot + 1..=source_slot).rev() {
            row[idx] = row[idx - 1].take();
        }
    }
    row[target_slot] = lease;
}

impl App {
    fn ensure_track_effect_lease_capacity(&mut self, track: usize) {
        while self.editor.track_effect_leases.len() <= track {
            self.editor
                .track_effect_leases
                .push(empty_track_effect_lease_slots());
        }
        if self.editor.track_effect_leases[track].len() < BUILTIN_SLOT_COUNT + MAX_CUSTOM_FX {
            self.editor.track_effect_leases[track]
                .resize_with(BUILTIN_SLOT_COUNT + MAX_CUSTOM_FX, || None);
        }
    }

    fn ensure_bus_effect_lease_capacity(&mut self, bus_idx: usize) {
        while self.editor.bus_effect_leases.len() <= bus_idx {
            self.editor
                .bus_effect_leases
                .push(empty_bus_effect_lease_slots());
        }
        if self.editor.bus_effect_leases[bus_idx].len() < MAX_CUSTOM_FX {
            self.editor.bus_effect_leases[bus_idx].resize_with(MAX_CUSTOM_FX, || None);
        }
    }

    pub(super) fn set_track_effect_lease(
        &mut self,
        track: usize,
        slot_idx: usize,
        lease: Option<lisp_host::DylibLease>,
    ) {
        self.ensure_track_effect_lease_capacity(track);
        if let Some(slot) = self
            .editor
            .track_effect_leases
            .get_mut(track)
            .and_then(|row| row.get_mut(slot_idx))
        {
            *slot = lease;
        }
    }

    pub(super) fn set_bus_effect_lease(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        lease: Option<lisp_host::DylibLease>,
    ) {
        self.ensure_bus_effect_lease_capacity(bus_idx);
        if let Some(slot) = self
            .editor
            .bus_effect_leases
            .get_mut(bus_idx)
            .and_then(|row| row.get_mut(slot_idx))
        {
            *slot = lease;
        }
    }

    pub(super) fn insert_empty_track_effect_lease_slot(&mut self, track: usize, slot_idx: usize) {
        self.ensure_track_effect_lease_capacity(track);
        let row = &mut self.editor.track_effect_leases[track];
        insert_empty_lease_slot(row, slot_idx);
    }

    pub(super) fn remove_track_effect_lease_slot(&mut self, track: usize, slot_idx: usize) {
        self.ensure_track_effect_lease_capacity(track);
        let row = &mut self.editor.track_effect_leases[track];
        remove_lease_slot(row, slot_idx);
    }

    pub(super) fn move_track_effect_lease_slot(
        &mut self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        self.ensure_track_effect_lease_capacity(track);
        let row = &mut self.editor.track_effect_leases[track];
        move_lease_slot(row, source_slot, target_slot);
    }

    pub(super) fn insert_empty_bus_effect_lease_slot(&mut self, bus_idx: usize, slot_idx: usize) {
        self.ensure_bus_effect_lease_capacity(bus_idx);
        let row = &mut self.editor.bus_effect_leases[bus_idx];
        insert_empty_lease_slot(row, slot_idx);
    }

    pub(super) fn move_bus_effect_lease_slot(
        &mut self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        self.ensure_bus_effect_lease_capacity(bus_idx);
        let row = &mut self.editor.bus_effect_leases[bus_idx];
        move_lease_slot(row, source_slot, target_slot);
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&self.state),
            self.graph.effect_descriptors.clone(),
            self.graph.instrument_descriptors.clone(),
            track,
            cursor_step,
        );
        let scratch_source =
            lisp_host::midi_fx_library_source_with_user_source(&self.editor.scratch_buffer);
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
        let engine_id =
            self.register_dedicated_instrument_engine(name, &source, &manifest, lib_index)?;
        Ok(PreparedRackInstrument {
            name: name.to_string(),
            engine_id,
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
        unsafe {
            self.graph_controller().add_custom_slot_to_rack(
                track,
                &prepared.name,
                prepared.engine_id,
                &prepared.manifest,
                &*lib_ptr,
                prepared.run_mode,
            )
        }
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

        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
        chain.push(desc.name.clone());
        self.state.pattern.track_params[track].set_midi_fx_chain(chain);
        self.state.pattern.midi_fx_slots[track][slot_idx].apply_descriptor(&desc, 0);

        self.state.save_current_track_midi_fx_snapshot(track);

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

        self.state
            .remove_midi_fx_slot_from_track_patterns(track, slot_idx);

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

    fn custom_effect_edges(&self, track: usize) -> Vec<CustomEffectEdge> {
        let mut edges = Vec::new();
        let mut prev_id = self.graph.track_node_ids[track].pan_id;
        let mut prev_channels = 2usize;
        for offset in 0..MAX_CUSTOM_FX {
            let slot_idx = BUILTIN_SLOT_COUNT + offset;
            let Some(slot) = self.state.pattern.effect_chains[track].get(slot_idx) else {
                continue;
            };
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            let desc = &self.graph.effect_descriptors[track][slot_idx];
            edges.push(CustomEffectEdge {
                source_id: prev_id,
                source_channels: prev_channels,
                dest_id: node_id as i32,
                dest_channels: desc.input_channels.max(1),
            });
            prev_id = node_id as i32;
            prev_channels = desc.output_channels.max(1);
        }
        edges.push(CustomEffectEdge {
            source_id: prev_id,
            source_channels: prev_channels,
            dest_id: self.graph.track_node_ids[track].delay_id,
            dest_channels: 2,
        });
        edges
    }

    unsafe fn disconnect_custom_effect_edge(&self, edge: CustomEffectEdge) {
        for src_port in 0..2 {
            for dst_port in 0..2 {
                let _ = crate::audiograph::graph_disconnect(
                    self.graph.lg.0,
                    edge.source_id,
                    src_port,
                    edge.dest_id,
                    dst_port,
                );
            }
        }
    }

    unsafe fn connect_custom_effect_edge(&self, edge: CustomEffectEdge) {
        let source_channels = edge.source_channels.max(1).min(2);
        let dest_channels = edge.dest_channels.max(1).min(2);
        if source_channels <= 1 {
            for dst_port in 0..dest_channels {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    edge.source_id,
                    0,
                    edge.dest_id,
                    dst_port as i32,
                );
            }
        } else if dest_channels <= 1 {
            for src_port in 0..source_channels {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    edge.source_id,
                    src_port as i32,
                    edge.dest_id,
                    0,
                );
            }
        } else {
            for ch in 0..source_channels.min(dest_channels) {
                let _ = crate::audiograph::graph_connect(
                    self.graph.lg.0,
                    edge.source_id,
                    ch as i32,
                    edge.dest_id,
                    ch as i32,
                );
            }
        }
    }

    fn reconnect_custom_effect_chain(&self, old_edges: Vec<CustomEffectEdge>, track: usize) {
        unsafe {
            for edge in old_edges {
                self.disconnect_custom_effect_edge(edge);
            }
            for edge in self.custom_effect_edges(track) {
                self.disconnect_custom_effect_edge(edge);
                self.connect_custom_effect_edge(edge);
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
        self.state.publish_scheduler_snapshot();
        self.refresh_effect_sidechain_labels();
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

    fn sync_other_bus_pattern_effect_insert(&mut self, bus_idx: usize, slot_idx: usize) {
        let default_snapshot = self.capture_bus_pattern_snapshot();
        self.state.insert_bus_effect_slot_in_other_scene_patterns(
            bus_idx,
            slot_idx,
            &default_snapshot,
        );
    }

    fn sync_other_bus_pattern_effect_move(
        &mut self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let default_snapshot = self.capture_bus_pattern_snapshot();
        self.state.move_bus_effect_slot_in_other_scene_patterns(
            bus_idx,
            source_slot,
            target_slot,
            &default_snapshot,
        );
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
        let old_edges = self.custom_effect_edges(track);
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
        self.reconnect_custom_effect_chain(old_edges, track);
        let slot_idx = BUILTIN_SLOT_COUNT + insert_offset;
        self.insert_empty_track_effect_lease_slot(track, slot_idx);
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
        let result = self.compile_saved_effect(name)?;
        let slot_idx = self.prepare_custom_effect_insert_slot(track, target_slot)?;
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
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
        let old_edges = self.custom_effect_edges(track);
        entries.insert(target_offset, entry);
        self.write_custom_effect_entries(track, &entries);
        self.reconnect_custom_effect_chain(old_edges, track);
        let slot_idx = BUILTIN_SLOT_COUNT + target_offset;
        self.move_track_effect_lease_slot(track, source_slot, slot_idx);
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
        let mut chain = self.state.pattern.track_params[track].midi_fx_chain();
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
        self.publish_effect_reorder();
        Ok(target_idx)
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
        (self.graph.track_node_ids[track].delay_id, 2)
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

        for (src_port, dst_port) in
            adapted_audio_port_connections(predecessor_outputs, effect_inputs)
        {
            let _ = crate::audiograph::graph_connect(
                self.graph.lg.0,
                predecessor_id,
                src_port,
                effect_id,
                dst_port,
            );
        }

        for (src_port, dst_port) in adapted_audio_port_connections(effect_outputs, successor_inputs)
        {
            let _ = crate::audiograph::graph_connect(
                self.graph.lg.0,
                effect_id,
                src_port,
                successor_id,
                dst_port,
            );
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
            "Str8 Delay" => (
                crate::str8_delay::str8_delay_vtable(),
                crate::str8_delay::STR8_DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Space Echo" => (
                crate::space_echo::space_echo_vtable(),
                crate::space_echo::SPACE_ECHO_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Dimension" => (
                crate::dimension::dimension_vtable(),
                crate::dimension::DIMENSION_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "DJ Mixer" => (
                crate::dj_mixer::dj_mixer_vtable(),
                crate::dj_mixer::DJ_MIXER_STATE_SIZE * std::mem::size_of::<f32>(),
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
            "OTT" => (
                crate::ott::ott_vtable(),
                crate::ott::OTT_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Limiter" => (
                crate::limiter::limiter_vtable(),
                crate::limiter::LIMITER_STATE_SIZE * std::mem::size_of::<f32>(),
            ),
            "Tape" => (
                crate::tape::tape_vtable(),
                crate::tape::TAPE_STATE_SIZE * std::mem::size_of::<f32>(),
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

    fn create_effect_modulator_node(&self, name: &str, slot_id: usize) -> Result<i32, String> {
        let mod_name = CString::new(format!("{}_{}_mod", name.to_lowercase(), slot_id)).unwrap();
        let mod_id = unsafe {
            crate::audiograph::add_node(
                self.graph.lg.0,
                crate::voice_modulator::effect_modulator_vtable(),
                crate::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                mod_name.as_ptr(),
                crate::voice_modulator::INPUT_COUNT as i32,
                crate::voice_modulator::NUM_OUTPUTS as i32,
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
                        idx: crate::voice_modulator::PARAM_BPM as u64,
                        logical_id: mod_id as u64,
                        fvalue: self.state.transport.bpm.load(Ordering::Relaxed) as f32,
                    },
                );
            }
            Ok(mod_id)
        }
    }

    unsafe fn connect_effect_modulator_for_descriptor(
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
            if !(1..=crate::voice_modulator::SLOT_COUNT).contains(&slot) {
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
                                idx: crate::voice_modulator::PARAM_BPM as u64,
                                fvalue: bpm,
                            },
                        );
                    }
                }
                if node_id != 0 {
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::delay::DELAY_PARAM_BPM,
                        "Str8 Delay" => crate::str8_delay::STR8_DELAY_PARAM_BPM,
                        "Space Echo" => crate::space_echo::SPACE_ECHO_PARAM_BPM,
                        "Filter" => crate::filter::FILTER_PARAM_BPM,
                        "DJ Mixer" => crate::dj_mixer::DJ_MIXER_PARAM_BPM,
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
                                    idx: crate::voice_modulator::PARAM_BPM as u64,
                                    fvalue: bpm,
                                },
                            );
                        }
                    }
                    let idx = match desc.name.as_str() {
                        "Delay" => crate::delay::DELAY_PARAM_BPM,
                        "Str8 Delay" => crate::str8_delay::STR8_DELAY_PARAM_BPM,
                        "Space Echo" => crate::space_echo::SPACE_ECHO_PARAM_BPM,
                        "Filter" => crate::filter::FILTER_PARAM_BPM,
                        "DJ Mixer" => crate::dj_mixer::DJ_MIXER_PARAM_BPM,
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
    }

    pub(super) fn load_builtin_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        // Convolution Reverb is a builtin whose DSP body is dgenlisp: route it
        // through the compile/apply path instead of a native vtable.
        if crate::conv_reverb::is_dgen_builtin(name) {
            return self.load_dgen_builtin_to_slot_sync(track, slot_idx, name);
        }
        let desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);
        let node_id = self.create_builtin_effect_node(slot_id, &desc)?;
        let existing_modulator = self.current_effect_modulator_node(track, slot_idx);
        let modulator_node_id = if desc.instrument_modulation_targets.is_empty() {
            None
        } else {
            Some(self.create_effect_modulator_node(&desc.name, slot_id)?)
        };
        let ext_mod_inputs = self.track_effect_ext_mod_input_nodes(track);
        unsafe {
            if let Some(old_id) = existing {
                lisp_host::remove_effect_from_chain(self.graph.lg.0, old_id, pred, succ);
                crate::conv_reverb::clear_instance(old_id);
            }
            if let Some(old_mod_id) = existing_modulator {
                lisp_host::remove_effect_modulator(self.graph.lg.0, old_mod_id);
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
            if let Some(mod_id) = modulator_node_id {
                self.connect_effect_modulator_for_descriptor(
                    mod_id,
                    node_id,
                    &desc,
                    ext_mod_inputs.as_ref(),
                )?;
            }
        }
        self.set_track_effect_lease(track, slot_idx, None);
        self.apply_builtin_effect_to_slot_with_modulator(
            track,
            slot_idx,
            node_id,
            modulator_node_id,
            desc,
        );
        self.push_track_effect_slot_defaults(track, slot_idx);
        self.push_all_delay_bpm();
        self.ui.effect_tab = EffectTab::Slot(slot_idx);
        self.ui.effect_param_cursor = 0;
        self.ui.effect_scroll_offset = 0;
        Ok(())
    }

    /// Load a dgenlisp-backed builtin (e.g. Convolution Reverb) onto a track
    /// slot: compile the bundled source fresh, apply it through the dgenlisp
    /// path, and record the instance's IR tensor offsets for the IR loader.
    pub(super) fn load_dgen_builtin_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let source = if crate::conv_reverb::is_dgen_builtin(name) {
            crate::conv_reverb::dsp_source()
        } else {
            return Err(format!("Unknown dgenlisp builtin '{name}'"));
        };
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            lisp_host::DGenSourceOrigin::BuiltinConvolutionReverb,
            source,
            self.graph.sample_rate,
            None,
        )?;
        // Capture IR tensor offsets before `result` is consumed by apply.
        let slots = crate::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
        let node_id = self.state.pattern.effect_chains[track][slot_idx]
            .node_id
            .load(Ordering::Relaxed) as i32;
        match slots {
            Some(slots) => crate::conv_reverb::record_ir_slots(node_id, slots),
            None => return Err(format!("'{name}' compiled without the expected IR tensors")),
        }
        // Auto-load the bundled default IR so a fresh instance is audible.
        // Non-fatal: if the asset is missing the effect still works (silent).
        if let Some(path) = crate::conv_reverb::default_ir_path() {
            if let Err(e) =
                self.set_conv_reverb_ir(track, slot_idx, &path, crate::conv_reverb::DEFAULT_IR_REF)
            {
                self.editor.status_message = Some((
                    format!("Convolution Reverb: default IR not loaded ({e})"),
                    Instant::now(),
                ));
            }
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
        let slots = crate::conv_reverb::ir_slots_for(node_id)
            .ok_or_else(|| "slot is not a Convolution Reverb".to_string())?;
        let ir = crate::conv_reverb::prepare_ir(abs_path, self.graph.sample_rate)?;
        unsafe {
            crate::conv_reverb::apply_ir_to_node(self.graph.lg.0, node_id, &slots, &ir)?;
        }
        // Friendly label: the bundled default has a fixed title; user samples
        // resolve their display title from the DB, falling back to the stem.
        let display = if reference == crate::conv_reverb::DEFAULT_IR_REF {
            "Lexicon 300 Rich Plate".to_string()
        } else {
            crate::sample_db::display_title_for_sample_path(abs_path)
                .unwrap_or_else(|| reference.to_string())
        };
        crate::conv_reverb::record_ir(node_id, reference, &display);
        Ok(())
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

    fn bus_effect_ext_mod_input_nodes(
        &self,
        bus_idx: usize,
    ) -> Option<[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]> {
        let bus_id = self.buses.get(bus_idx)?.id;
        self.graph
            .bus_node_ids
            .iter()
            .find(|nodes| nodes.id == bus_id)
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
        self.state.publish_scheduler_snapshot();
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

        let result = lisp_host::run_embedded_effect_editor_flow(
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
            let lisp_host::EffectEditResult {
                manifest,
                lib,
                source: _,
                name,
                lease,
            } = r;
            let existing_modulator = self.current_effect_modulator_node(track, slot_idx);
            let ext_mod_inputs = self.track_effect_ext_mod_input_nodes(track);
            match unsafe {
                lisp_host::add_effect_to_chain_at(
                    self.graph.lg.0,
                    slot_id,
                    &manifest,
                    &lib,
                    predecessor_id,
                    predecessor_outputs,
                    successor_id,
                    successor_inputs,
                    existing,
                    existing_modulator,
                    ext_mod_inputs.as_ref(),
                )
            } {
                Ok(node_ids) => {
                    if let Some(old_node_id) = existing {
                        crate::conv_reverb::clear_instance(old_node_id);
                    }
                    self.apply_effect_to_slot(track, slot_idx, node_ids, &name, &manifest);
                    self.ui.effect_tab = EffectTab::Slot(slot_idx);
                    self.ui.effect_param_cursor = 0;
                    self.ui.effect_scroll_offset = 0;
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.editor.lisp_libs.push(lib);
                    self.set_track_effect_lease(track, slot_idx, lease);
                }
                Err(error) => {
                    self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
                }
            }
        }
    }

    pub fn start_effect_compile(&mut self, name: &str, slot_idx: usize) {
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
        let lisp_host::CompileResult {
            manifest,
            lib,
            lease,
        } = result;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);

        let existing_modulator = self.current_effect_modulator_node(track, slot_idx);
        let ext_mod_inputs = self.track_effect_ext_mod_input_nodes(track);
        let node_ids = unsafe {
            lisp_host::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &manifest,
                &lib,
                pred,
                pred_outputs,
                succ,
                succ_inputs,
                existing,
                existing_modulator,
                ext_mod_inputs.as_ref(),
            )
        }?;
        if let Some(old_node_id) = existing {
            crate::conv_reverb::clear_instance(old_node_id);
        }
        self.apply_effect_to_slot(track, slot_idx, node_ids, name, &manifest);
        self.editor.lisp_libs.push(lib);
        self.set_track_effect_lease(track, slot_idx, lease);
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
        let result = self.compile_saved_effect(name)?;
        self.apply_compiled_effect_to_slot_sync(result, name, slot_idx, track)?;
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

    fn bus_effect_edges(&self, bus_idx: usize) -> Result<Vec<CustomEffectEdge>, String> {
        let bus_nodes = self
            .graph
            .bus_node_ids
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} graph nodes not found", bus_idx + 1))?;
        let bus = self
            .buses
            .get(bus_idx)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let mut edges = Vec::new();
        let mut prev_id = bus_nodes.gate_id;
        let mut prev_channels = 2usize;
        for slot_idx in 0..MAX_CUSTOM_FX {
            let Some(slot) = bus.effect_slots.get(slot_idx) else {
                continue;
            };
            if slot.node_id == 0 {
                continue;
            }
            let desc = &bus.effect_descriptors[slot_idx];
            edges.push(CustomEffectEdge {
                source_id: prev_id,
                source_channels: prev_channels,
                dest_id: slot.node_id as i32,
                dest_channels: desc.input_channels.max(1),
            });
            prev_id = slot.node_id as i32;
            prev_channels = desc.output_channels.max(1);
        }
        edges.push(CustomEffectEdge {
            source_id: prev_id,
            source_channels: prev_channels,
            dest_id: bus_nodes.volume_id,
            dest_channels: 2,
        });
        Ok(edges)
    }

    fn reconnect_bus_effect_chain(
        &self,
        old_edges: Vec<CustomEffectEdge>,
        bus_idx: usize,
    ) -> Result<(), String> {
        unsafe {
            for edge in old_edges {
                self.disconnect_custom_effect_edge(edge);
            }
            for edge in self.bus_effect_edges(bus_idx)? {
                self.disconnect_custom_effect_edge(edge);
                self.connect_custom_effect_edge(edge);
            }
        }
        Ok(())
    }

    fn prepare_bus_effect_insert_slot(
        &mut self,
        bus_idx: usize,
        target_slot: usize,
    ) -> Result<usize, String> {
        let mut entries = self.bus_effect_entries(bus_idx)?;
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
        let old_edges = self.bus_effect_edges(bus_idx)?;
        self.write_bus_effect_entries(bus_idx, &entries)?;
        self.reconnect_bus_effect_chain(old_edges, bus_idx)?;
        self.insert_empty_bus_effect_lease_slot(bus_idx, insert_offset);
        self.sync_other_bus_pattern_effect_insert(bus_idx, insert_offset);
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
        Ok(slot_idx)
    }

    pub fn move_bus_effect_slot_sync(
        &mut self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: Option<usize>,
    ) -> Result<usize, String> {
        let mut entries = self.bus_effect_entries(bus_idx)?;
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
        let old_edges = self.bus_effect_edges(bus_idx)?;
        entries.insert(target_offset, entry);
        self.write_bus_effect_entries(bus_idx, &entries)?;
        self.reconnect_bus_effect_chain(old_edges, bus_idx)?;
        self.move_bus_effect_lease_slot(bus_idx, source_offset, target_offset);
        self.sync_other_bus_pattern_effect_move(bus_idx, source_offset, target_offset);
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
        let source = if crate::conv_reverb::is_dgen_builtin(name) {
            crate::conv_reverb::dsp_source()
        } else {
            return Err(format!("Unknown dgenlisp builtin '{name}'"));
        };
        let result = self.editor.dylib_cache.acquire(
            lisp_host::DGenCompileKind::Effect,
            lisp_host::DGenSourceOrigin::BuiltinConvolutionReverb,
            source,
            self.graph.sample_rate,
            None,
        )?;
        let slots = crate::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot_idx, name, result)?;
        let node_id = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.node_id as i32)
            .unwrap_or(0);
        match slots {
            Some(slots) => crate::conv_reverb::record_ir_slots(node_id, slots),
            None => return Err(format!("'{name}' compiled without the expected IR tensors")),
        }
        // Auto-load the bundled default IR so a fresh instance is audible.
        if let Some(path) = crate::conv_reverb::default_ir_path() {
            if let Err(e) = self.set_conv_reverb_ir_bus(
                bus_idx,
                slot_idx,
                &path,
                crate::conv_reverb::DEFAULT_IR_REF,
            ) {
                self.editor.status_message = Some((
                    format!("Convolution Reverb: default IR not loaded ({e})"),
                    Instant::now(),
                ));
            }
        }
        Ok(())
    }

    pub fn load_builtin_bus_effect_to_slot_sync(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        if crate::conv_reverb::is_dgen_builtin(name) {
            return self.load_dgen_builtin_bus_to_slot_sync(bus_idx, slot_idx, name);
        }
        let desc = EffectDescriptor::builtin_insert(name)
            .ok_or_else(|| format!("Unknown built-in effect '{name}'"))?;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
        let node_id = self.create_builtin_effect_node(slot_id, &desc)?;
        let existing_modulator = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.modulator_node_id as i32)
            .filter(|node_id| *node_id > 0);
        let modulator_node_id = if desc.instrument_modulation_targets.is_empty() {
            None
        } else {
            Some(self.create_effect_modulator_node(&desc.name, slot_id)?)
        };
        let ext_mod_inputs = self.bus_effect_ext_mod_input_nodes(bus_idx);
        unsafe {
            if let Some(old_id) = existing {
                lisp_host::remove_effect_from_chain(self.graph.lg.0, old_id, pred, succ);
                crate::conv_reverb::clear_instance(old_id);
            }
            if let Some(old_mod_id) = existing_modulator {
                lisp_host::remove_effect_modulator(self.graph.lg.0, old_mod_id);
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
            if let Some(mod_id) = modulator_node_id {
                self.connect_effect_modulator_for_descriptor(
                    mod_id,
                    node_id,
                    &desc,
                    ext_mod_inputs.as_ref(),
                )?;
            }
        }
        self.set_bus_effect_lease(bus_idx, slot_idx, None);
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
        let lisp_host::CompileResult {
            manifest,
            lib,
            lease,
        } = result;
        let (slot_id, pred, pred_outputs, succ, succ_inputs, existing) =
            self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
        let existing_modulator = self
            .buses
            .get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot_idx))
            .map(|slot| slot.modulator_node_id as i32)
            .filter(|node_id| *node_id > 0);
        let ext_mod_inputs = self.bus_effect_ext_mod_input_nodes(bus_idx);
        let node_ids = unsafe {
            lisp_host::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &manifest,
                &lib,
                pred,
                pred_outputs,
                succ,
                succ_inputs,
                existing,
                existing_modulator,
                ext_mod_inputs.as_ref(),
            )
        }?;
        if let Some(old_node_id) = existing {
            crate::conv_reverb::clear_instance(old_node_id);
        }
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
        self.editor.lisp_libs.push(lib);
        self.set_bus_effect_lease(bus_idx, slot_idx, lease);
        Ok(())
    }

    pub fn delete_bus_effect_slot(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
    ) -> Result<(), String> {
        let (node_id, modulator_node_id, pred, pred_outputs, succ, succ_inputs) = {
            let bus = self
                .buses
                .get(bus_idx)
                .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
            let (node_id, modulator_node_id) = bus
                .effect_slots
                .get(slot_idx)
                .map(|slot| (slot.node_id, slot.modulator_node_id))
                .unwrap_or((0, 0));
            if node_id == 0 {
                (0, 0, 0, 0, 0, 0)
            } else {
                let (_, pred, pred_outputs, succ, succ_inputs, _) =
                    self.resolve_bus_effect_slot_wiring(bus_idx, slot_idx)?;
                (
                    node_id,
                    modulator_node_id,
                    pred,
                    pred_outputs,
                    succ,
                    succ_inputs,
                )
            }
        };
        if node_id != 0 {
            unsafe {
                lisp_host::remove_effect_from_chain(self.graph.lg.0, node_id as i32, pred, succ);
                lisp_host::remove_effect_modulator(self.graph.lg.0, modulator_node_id as i32);
            }
            crate::conv_reverb::clear_instance(node_id as i32);
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
        self.set_bus_effect_lease(bus_idx, slot_idx, None);
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
            if crate::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
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
        self.push_bus_effect_param_to_graph(
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
        if crate::voice_modulator::is_envelope_source_param_value(param.node_param_idx, value) {
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

    fn push_bus_effect_param_to_graph(
        &self,
        node_id: u32,
        modulator_node_id: u32,
        node_param_idx: u32,
        node_param_span: u32,
        value: f32,
    ) {
        let Some((logical_id, node_param_idx)) =
            (if node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE {
                (modulator_node_id != 0).then_some((
                    modulator_node_id as u64,
                    node_param_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
                ))
            } else if node_id != 0 && node_param_idx != u32::MAX {
                Some((node_id as u64, node_param_idx as u64))
            } else {
                None
            })
        else {
            return;
        };
        unsafe {
            for lane in 0..node_param_span.max(1) as u64 {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        logical_id,
                        idx: node_param_idx + lane,
                        fvalue: value,
                    },
                );
            }
        }
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
            self.push_bus_effect_param_to_graph(
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
                ParamKind::Enum { labels } => {
                    if crate::voice_modulator::is_source_param(p.node_param_idx) && label == "env" {
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
            if crate::voice_modulator::is_envelope_source_param_value(
                param.node_param_idx,
                slot.defaults[param_idx],
            ) {
                continue;
            }
            let (logical_id, node_param_idx) =
                if param.node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE {
                    if slot.modulator_node_id == 0 {
                        continue;
                    }
                    (
                        slot.modulator_node_id as u64,
                        param.node_param_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
                    )
                } else {
                    (slot.node_id as u64, param.node_param_idx as u64)
                };
            unsafe {
                for lane in 0..param.node_param_span.max(1) as u64 {
                    crate::audiograph::params_push_wrapper(
                        self.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            logical_id,
                            idx: node_param_idx + lane,
                            fvalue: slot.defaults[param_idx],
                        },
                    );
                }
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
        crate::lisp_host::save_effect(name, source).map_err(|e| e.to_string())?;
        self.load_saved_effect_to_slot_sync(track, slot_idx, name)?;
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

    fn run_instrument_editor(&mut self, existing_name: Option<String>) {
        let result = lisp_host::run_embedded_instrument_editor_flow(
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
                    let manifest = result.manifest.clone();
                    let lib_index = self.push_instrument_lib(result.lib, result.lease);
                    let lib_ptr: *const lisp_host::LoadedDGenLib =
                        &self.editor.instrument_libs[lib_index];
                    unsafe {
                        self.graph_controller()
                            .hot_reload_instrument(track, &manifest, &*lib_ptr)
                    }
                    .map_err(|e| e.to_string())?;
                    self.push_instrument_defaults_for_track(track);
                    if let Some(runtime_engine_id) = runtime_engine_id {
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
                let manifest = r.manifest.clone();
                let lib_index = self.push_instrument_lib(r.lib, r.lease);
                let lib_ptr: *const lisp_host::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller()
                        .hot_reload_instrument(track, &manifest, &*lib_ptr)
                } {
                    Ok(()) => {
                        self.push_instrument_defaults_for_track(track);
                        if let Some(runtime_engine_id) = runtime_engine_id {
                            self.editor.engine_registry.replace_at(
                                runtime_engine_id,
                                super::EngineDescriptor {
                                    name: r.name.clone(),
                                    source: r.source.clone(),
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
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib, r.lease);
                let manifest = self.editor.engine_registry.engines[cache_idx]
                    .manifest
                    .clone();
                let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
                let lib_ptr: *const lisp_host::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller().add_custom_track(
                        &r.name,
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
            lisp_host::ScratchControlRuntime::new(
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
        let midi_fx_library = lisp_host::load_midi_fx_library_source();
        if !midi_fx_library.trim().is_empty() {
            let _ = runtime.eval(&midi_fx_library);
        }
        if let Some((text, cursor, runtime)) = lisp_host::run_embedded_scratch_flow(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};
    use crate::ui::AudioBuses;
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
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
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
        app
    }

    fn test_app_with_track() -> App {
        test_app_with_track_count(1)
    }

    #[test]
    fn audio_port_adapter_duplicates_mono_and_folds_stereo() {
        assert_eq!(adapted_audio_port_connections(1, 1), vec![(0, 0)]);
        assert_eq!(adapted_audio_port_connections(1, 2), vec![(0, 0), (0, 1)]);
        assert_eq!(adapted_audio_port_connections(2, 1), vec![(0, 0), (1, 0)]);
        assert_eq!(adapted_audio_port_connections(2, 2), vec![(0, 0), (1, 1)]);
        assert_eq!(adapted_audio_port_connections(4, 4), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn dylib_cache_lease_slot_insert_shifts_right_and_drops_tail() {
        let mut row = vec![Some(1), Some(2), None, Some(4)];

        insert_empty_lease_slot(&mut row, 1);

        assert_eq!(row, vec![Some(1), None, Some(2), None]);
    }

    #[test]
    fn dylib_cache_lease_slot_remove_shifts_left_and_clears_tail() {
        let mut row = vec![Some(1), Some(2), Some(3), None];

        remove_lease_slot(&mut row, 1);

        assert_eq!(row, vec![Some(1), Some(3), None, None]);
    }

    #[test]
    fn dylib_cache_lease_slot_move_preserves_relative_order() {
        let mut row = vec![Some(1), Some(2), Some(3), Some(4)];

        move_lease_slot(&mut row, 0, 2);
        assert_eq!(row, vec![Some(2), Some(3), Some(1), Some(4)]);

        move_lease_slot(&mut row, 3, 1);
        assert_eq!(row, vec![Some(2), Some(4), Some(3), Some(1)]);
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
            .recv_timeout(Duration::from_secs(10))
            .expect("test app should initialize");
        let (result, published_chain) = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("adding MIDI FX should not block on pattern_bank");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(published_chain, vec!["arp".to_string()]);
    }
}
