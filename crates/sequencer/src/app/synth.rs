use std::sync::atomic::Ordering;

use super::command::{apply_command, AppCommand};

use crate::effects::EffectDescriptor;
use crate::sequencer::{InstrumentType, RackSlotParam, RackSlotSnapshot};

use super::App;

pub(super) const SYNTH_MIN_COLUMN_WIDTH: u16 = 42;
pub(super) const SYNTH_COLUMN_GAP: u16 = 2;

#[derive(Clone, Copy)]
struct RackSlotInstrumentParamRoute {
    instrument_type: InstrumentType,
    node_param_idx: u64,
    node_param_span: u32,
    sample_rate: Option<f32>,
}

#[derive(Clone, Copy)]
enum TransientRackMacroTarget {
    SlotGain {
        slot: usize,
    },
    SlotPan {
        slot: usize,
    },
    SlotMute {
        slot: usize,
    },
    SlotInstrumentParam {
        slot: usize,
        param_index: usize,
    },
    SlotEffectParam {
        slot: usize,
        effect_slot: usize,
        param_index: usize,
    },
}

#[derive(Clone, Copy)]
struct TransientRackMacroMapping {
    target: TransientRackMacroTarget,
    range_min: f32,
    range_max: f32,
    curve: crate::sequencer::RackMacroCurve,
}

impl App {
    pub(super) fn source_param_actual_indices(&self, track: usize) -> Vec<usize> {
        let Some(desc) = self.graph.instrument_descriptors.get(track) else {
            return Vec::new();
        };
        let slot = &self.state.pattern.instrument_slots[track];
        crate::instruments::voice_modulator::selected_source_param_indices(&desc.params, |idx, param| {
            if idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(idx)
            } else {
                param.default
            }
        })
    }

    pub(super) fn source_display_rows(
        &self,
        track: usize,
    ) -> Vec<(Option<&'static str>, Option<usize>)> {
        let actual = self.source_param_actual_indices(track);
        let mut rows = Vec::new();
        for source_slot in 1..=crate::instruments::voice_modulator::SLOT_COUNT {
            let Some(section) = crate::instruments::voice_modulator::modulator_slot_label_static(source_slot)
            else {
                continue;
            };
            let section_params: Vec<usize> = actual
                .iter()
                .enumerate()
                .filter_map(|(row_idx, &actual_idx)| {
                    self.graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|desc| desc.params.get(actual_idx))
                        .and_then(|param| crate::instruments::voice_modulator::slot_from_param_name(&param.name))
                        .is_some_and(|slot| slot == source_slot)
                        .then_some(row_idx)
                })
                .collect();
            if section_params.is_empty() {
                continue;
            }
            rows.push((Some(section), None));
            for row_idx in section_params {
                rows.push((None, Some(row_idx)));
            }
        }

        rows
    }

    fn is_modulation_param_name(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_hidden_modulation_param_name(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_mod_source_param(node_param_idx: u32) -> bool {
        node_param_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE
    }

    pub(super) fn synth_param_indices(&self, track: usize) -> Vec<usize> {
        self.graph
            .instrument_descriptors
            .get(track)
            .map(|d| {
                d.params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| {
                        (!Self::is_modulation_param_name(&p.name)
                            && !Self::is_hidden_modulation_param_name(&p.name)
                            && !Self::is_mod_source_param(p.node_param_idx))
                        .then_some(i)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn mod_param_indices(&self, track: usize) -> Vec<usize> {
        self.graph
            .instrument_descriptors
            .get(track)
            .map(|d| {
                d.params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| {
                        (Self::is_modulation_param_name(&p.name)
                            && !Self::is_hidden_modulation_param_name(&p.name)
                            && !Self::is_mod_source_param(p.node_param_idx))
                        .then_some(i)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn source_param_indices(&self, track: usize) -> Vec<usize> {
        self.graph
            .instrument_descriptors
            .get(track)
            .map(|d| {
                d.params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| Self::is_mod_source_param(p.node_param_idx).then_some(i))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn instrument_base_note_offset(&self, track: usize) -> f32 {
        f32::from_bits(
            self.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        )
    }

    pub(super) fn set_instrument_base_note_offset(&mut self, track: usize, value: f32) {
        apply_command(
            self,
            AppCommand::SetInstrumentBaseNoteOffset { track, value },
        );
    }

    pub(super) fn synth_row_count(&self) -> usize {
        self.synth_param_indices(self.ui.cursor_track).len() + 1
    }

    pub(super) fn mod_row_count(&self) -> usize {
        self.mod_param_indices(self.ui.cursor_track).len()
    }

    pub(super) fn source_row_count(&self) -> usize {
        self.source_display_row_count()
    }


    pub(super) fn is_current_custom_track(&self) -> bool {
        !self.is_sampler_track(self.ui.cursor_track)
    }

    pub(super) fn current_instrument_descriptor(&self) -> Option<&EffectDescriptor> {
        if !self.is_current_custom_track() {
            return None;
        }
        self.graph.instrument_descriptors.get(self.ui.cursor_track)
    }

    pub(super) fn current_mod_descriptor(&self) -> Option<EffectDescriptor> {
        let desc = self.current_instrument_descriptor()?;
        let params = self
            .mod_param_indices(self.ui.cursor_track)
            .into_iter()
            .filter_map(|i| desc.params.get(i).cloned())
            .collect::<Vec<_>>();
        Some(EffectDescriptor {
            name: "Mod".to_string(),
            params,
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        })
    }

    pub(super) fn current_source_descriptor(&self) -> Option<EffectDescriptor> {
        let desc = self.current_instrument_descriptor()?;
        let params = self
            .source_param_actual_indices(self.ui.cursor_track)
            .into_iter()
            .filter_map(|i| desc.params.get(i).cloned())
            .map(|mut p| {
                p.name = crate::instruments::voice_modulator::source_param_display_name(&p.name);
                if p.name == "rate" && matches!(p.kind, crate::effects::ParamKind::Enum { .. }) {
                    p.scaling = crate::effects::ParamScaling::Linear;
                }
                p
            })
            .collect::<Vec<_>>();
        Some(EffectDescriptor {
            name: "Sources".to_string(),
            params,
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        })
    }

    pub(super) fn synth_row_display_value(&self, track: usize, row_idx: usize) -> Option<f32> {
        if row_idx == 0 {
            return Some(self.instrument_base_note_offset(track));
        }

        let synth_indices = self.synth_param_indices(track);
        let param_idx = *synth_indices.get(row_idx.checked_sub(1)?)?;
        let desc = self.graph.instrument_descriptors.get(track)?;
        let param_desc = desc.params.get(param_idx)?;
        let stored = self.state.pattern.instrument_slots[track]
            .defaults
            .get(param_idx);
        Some(param_desc.stored_to_user(stored))
    }

    pub(super) fn mod_row_display_value(&self, track: usize, row_idx: usize) -> Option<f32> {
        let mod_indices = self.mod_param_indices(track);
        let param_idx = *mod_indices.get(row_idx)?;
        let desc = self.graph.instrument_descriptors.get(track)?;
        let param_desc = desc.params.get(param_idx)?;
        let stored = self.state.pattern.instrument_slots[track]
            .defaults
            .get(param_idx);
        Some(param_desc.stored_to_user(stored))
    }

    pub(super) fn source_row_display_value(&self, track: usize, row_idx: usize) -> Option<f32> {
        let source_indices = self.source_param_actual_indices(track);
        let param_idx = *source_indices.get(row_idx)?;
        let desc = self.graph.instrument_descriptors.get(track)?;
        let param_desc = desc.params.get(param_idx)?;
        let stored = self.state.pattern.instrument_slots[track]
            .defaults
            .get(param_idx);
        Some(param_desc.stored_to_user(stored))
    }

    pub(super) fn source_param_count(&self) -> usize {
        self.source_param_actual_indices(self.ui.cursor_track).len()
    }

    fn source_display_row_count(&self) -> usize {
        self.source_display_rows(self.ui.cursor_track).len()
    }

    pub(super) fn source_display_row_for_param_row(&self, param_row: usize) -> usize {
        self.source_display_rows(self.ui.cursor_track)
            .iter()
            .enumerate()
            .find_map(|(display_idx, (_, maybe_row))| {
                (*maybe_row == Some(param_row)).then_some(display_idx)
            })
            .unwrap_or(0)
    }

    pub(super) fn source_param_row_for_display(&self, display_row: usize) -> Option<usize> {
        self.source_display_rows(self.ui.cursor_track)
            .get(display_row)
            .and_then(|(_, maybe_row)| *maybe_row)
    }

    pub(super) fn set_instrument_param_or_plock(
        &mut self,
        track: usize,
        param_idx: usize,
        value: f32,
    ) {
        if self.has_selection() {
            apply_command(
                self,
                AppCommand::SetInstrumentPlockMulti {
                    track,
                    steps: self.selected_steps(),
                    param_idx,
                    value,
                },
            );
        } else {
            apply_command(
                self,
                AppCommand::SetInstrumentParam {
                    track,
                    param_idx,
                    value,
                },
            );
        }
    }

    fn adjust_instrument_param(&mut self, direction: f32) {
        let track = self.ui.cursor_track;
        if self.ui.instrument_param_cursor == 0 {
            return;
        }
        let synth_indices = self.synth_param_indices(track);
        let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) else {
            return;
        };
        let desc = match self.graph.instrument_descriptors.get(track) {
            Some(d) => d,
            None => return,
        };
        if param_idx >= desc.params.len() {
            return;
        }
        let param_desc = &desc.params[param_idx];
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    let inc = param_desc.increment(current);
                    (step, param_desc.clamp(current + direction * inc))
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let old = slot.defaults.get(param_idx);
            let inc = param_desc.increment(old);
            let new_val = param_desc.clamp(old + direction * inc);
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }

    fn adjust_mod_param(&mut self, direction: f32) {
        let track = self.ui.cursor_track;
        let mod_indices = self.mod_param_indices(track);
        let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) else {
            return;
        };
        let desc = match self.graph.instrument_descriptors.get(track) {
            Some(d) => d,
            None => return,
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    let inc = param_desc.increment(current);
                    (step, param_desc.clamp(current + direction * inc))
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let old = slot.defaults.get(param_idx);
            let inc = param_desc.increment(old);
            let new_val = param_desc.clamp(old + direction * inc);
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }

    fn adjust_source_param(&mut self, direction: f32) {
        let track = self.ui.cursor_track;
        let source_indices = self.source_param_actual_indices(track);
        let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
            return;
        };
        let desc = match self.graph.instrument_descriptors.get(track) {
            Some(d) => d,
            None => return,
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    let inc = param_desc.increment(current);
                    (step, param_desc.clamp(current + direction * inc))
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let old = slot.defaults.get(param_idx);
            let inc = param_desc.increment(old);
            let new_val = param_desc.clamp(old + direction * inc);
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }

    pub fn send_instrument_param(&self, track: usize, param_idx: usize, value: f32) {
        if self.graph.lg.0.is_null() {
            return;
        }
        let slot = &self.state.pattern.instrument_slots[track];
        let idx = slot.resolve_node_idx(param_idx);
        let span = slot.resolve_node_span(param_idx);
        if crate::instruments::voice_modulator::is_bar_resync_param(idx as u32) {
            self.state.schedule_mod_resync();
        }
        if self.is_sampler_track(track) {
            // Slice controls are host-resolved at trigger time and deliberately
            // have no sampler/modulator DSP state cell to push live.
            if idx == u32::MAX as u64 {
                return;
            }
            let sample_rate = self.graph.sample_rate as f32;
            let (idx, fvalue) = match param_idx {
                0 => (
                    crate::instruments::sampler::PARAM_ATTACK_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                1 => (
                    crate::instruments::sampler::PARAM_RELEASE_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                2 => (crate::instruments::sampler::PARAM_START_POINT, value),
                3 => (crate::instruments::sampler::PARAM_END_POINT, value),
                4 => (crate::instruments::sampler::PARAM_ENABLED, value),
                5 => (crate::instruments::sampler::PARAM_REVERSE, value),
                6 => (crate::instruments::sampler::PARAM_LOOP_MODE, value),
                7 => (
                    crate::instruments::sampler::PARAM_LOOP_XFADE_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                8 => (crate::instruments::sampler::PARAM_SR_HZ, value),
                9 => (crate::instruments::sampler::PARAM_WARP_ENABLED, value),
                10 => (crate::instruments::sampler::PARAM_WARP_MODE, value),
                11 => (crate::instruments::sampler::PARAM_WARP_SAMPLE_BPM, value),
                _ => (idx, value),
            };
            if crate::instruments::sampler::srange_debug_enabled() && (param_idx == 2 || param_idx == 3) {
                eprintln!(
                    "[srange] live push track={} param={} value={} voice_lids={}",
                    track,
                    param_idx,
                    fvalue,
                    self.graph
                        .track_voice_lids
                        .get(track)
                        .map_or(0, |lids| lids.len()),
                );
            }
            let is_mod_param = idx as u32 >= crate::instruments::voice_modulator::MOD_PARAM_BASE;
            let resolved_idx = if is_mod_param {
                idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64
            } else {
                idx
            };
            if is_mod_param {
                if let Some(nodes) = self.graph.track_node_ids.get(track) {
                    for &logical_id in &nodes.sampler_modulator_ids {
                        unsafe {
                            for lane in 0..span as u64 {
                                crate::audiograph::params_push_wrapper(
                                    self.graph.lg.0,
                                    crate::audiograph::ParamMsg {
                                        idx: resolved_idx + lane,
                                        logical_id: logical_id as u64,
                                        fvalue,
                                    },
                                );
                            }
                        }
                    }
                }
            } else if let Some(voice_lids) = self.graph.track_voice_lids.get(track) {
                for &logical_id in voice_lids {
                    unsafe {
                        for lane in 0..span as u64 {
                            crate::audiograph::params_push_wrapper(
                                self.graph.lg.0,
                                crate::audiograph::ParamMsg {
                                    idx: resolved_idx + lane,
                                    logical_id,
                                    fvalue,
                                },
                            );
                        }
                    }
                }
            }
            return;
        }
        let Some(engine_id) = self.graph.track_engine_ids.get(track).and_then(|id| *id) else {
            return;
        };
        let engine_track_uses = self
            .graph
            .track_engine_ids
            .iter()
            .filter(|bound| **bound == Some(engine_id))
            .count();
        if engine_track_uses > 1 {
            return;
        }
        let Some(engine) = self
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
        else {
            return;
        };
        let is_mod_param = idx as u32 >= crate::instruments::voice_modulator::MOD_PARAM_BASE;
        let resolved_idx = if is_mod_param {
            idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        let target_ids = if is_mod_param {
            &engine.modulator_ids
        } else {
            &engine.synth_ids
        };
        for &node_id in target_ids {
            unsafe {
                for lane in 0..span as u64 {
                    crate::audiograph::params_push_wrapper(
                        self.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: resolved_idx + lane,
                            logical_id: node_id as u64,
                            fvalue: value,
                        },
                    );
                }
            }
        }
    }

    pub fn effective_instrument_param_value(&self, track: usize, param_idx: usize) -> Option<f32> {
        let slot = self.state.pattern.instrument_slots.get(track)?;
        if param_idx >= slot.num_params.load(Ordering::Relaxed) as usize {
            return None;
        }
        let raw_idx = slot.resolve_node_idx(param_idx) as u32;
        let param_id = crate::neural::ParamNodeId::from_slot_param(
            slot.node_id.load(Ordering::Relaxed),
            slot.modulator_node_id.load(Ordering::Relaxed),
            raw_idx,
        );
        let key = crate::macro_engine::MacroParamKey::for_instrument(track, param_idx, param_id);
        Some(
            self.macro_engine
                .effective_value(&key, slot.defaults.get(param_idx)),
        )
    }

    /// Sends the current base value unless an engaged macro owns this param.
    /// Silent while the mirror holds a bound-but-not-audible clip (takes
    /// spec 16.7): tuning a past/future clip is display + edit only.
    pub(super) fn send_effective_instrument_param(&self, track: usize, param_idx: usize) {
        // A base edit under an engaged override is legal (release pops back
        // to the new base): keep the leak shadow tracking the fresh base.
        #[cfg(debug_assertions)]
        self.refresh_macro_base_shadow_for_instrument(track, param_idx);
        if self.sound_binding_is_silent(track) {
            return;
        }
        let Some(value) = self.effective_instrument_param_value(track, param_idx) else {
            return;
        };
        self.send_instrument_param(track, param_idx, value);
    }

    pub fn send_instrument_tensor_param(&self, track: usize, tensor_idx: usize, values: &[f32]) {
        if self.is_sampler_track(track) {
            return;
        }
        let Some(cell_offset) = self.state.pattern.instrument_slots[track]
            .tensor_params
            .tensor_cell_offset(tensor_idx)
        else {
            return;
        };
        let Some(engine_id) = self.graph.track_engine_ids.get(track).and_then(|id| *id) else {
            return;
        };
        let engine_track_uses = self
            .graph
            .track_engine_ids
            .iter()
            .filter(|bound| **bound == Some(engine_id))
            .count();
        if engine_track_uses > 1 {
            return;
        }
        let Some(engine) = self
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
        else {
            return;
        };
        for &node_id in &engine.synth_ids {
            unsafe {
                crate::lisp_host::queue_tensor_write(self.graph.lg.0, node_id, cell_offset, values);
            }
        }
    }

    pub fn rack_slot_instrument_descriptor(
        &self,
        slot: &RackSlotSnapshot,
    ) -> Option<EffectDescriptor> {
        match slot.instrument_type {
            InstrumentType::Sampler => Some(EffectDescriptor::builtin_sampler()),
            InstrumentType::Custom | InstrumentType::Modulator => {
                let engine_id = slot.track_sound_state.engine_id?;
                self.editor
                    .engine_registry
                    .get_instrument_descriptor(engine_id)
                    .cloned()
            }
            InstrumentType::Rack => None,
        }
    }

    pub fn rack_slot_cached_instrument_descriptor(
        &self,
        slot: &RackSlotSnapshot,
    ) -> Option<&EffectDescriptor> {
        match slot.instrument_type {
            InstrumentType::Custom | InstrumentType::Modulator => {
                slot.track_sound_state.engine_id.and_then(|engine_id| {
                    self.editor
                        .engine_registry
                        .get_instrument_descriptor(engine_id)
                })
            }
            InstrumentType::Sampler | InstrumentType::Rack => None,
        }
    }

    fn rack_slot_snapshot(&self, track: usize, slot_idx: usize) -> Option<RackSlotSnapshot> {
        self.state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .cloned()
    }

    /// Copies only the immutable routing metadata needed to send one live
    /// parameter value. A rack slot owns large effect descriptors and p-lock
    /// grids, so cloning the complete authoring snapshot on a pointer-rate
    /// control path is both unnecessary and disproportionately expensive.
    fn rack_slot_instrument_param_route(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
    ) -> Option<RackSlotInstrumentParamRoute> {
        let racks = self.state.pattern.rack_tracks.lock().unwrap();
        let slot = racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(slot_idx))?;
        Some(RackSlotInstrumentParamRoute {
            instrument_type: slot.instrument_type,
            node_param_idx: slot
                .instrument_slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(0) as u64,
            node_param_span: slot
                .instrument_slot
                .param_node_spans
                .get(param_idx)
                .copied()
                .unwrap_or(1)
                .max(1),
            sample_rate: slot
                .sample_id
                .as_ref()
                .map(|(_, _, rate)| (*rate).max(1) as f32),
        })
    }

    fn push_rack_slot_panner_param(&self, track: usize, slot_idx: usize, idx: u64, value: f32) {
        let Some(nodes) = self
            .graph
            .track_node_ids
            .get(track)
            .and_then(|track_nodes| track_nodes.rack_slots.get(slot_idx))
        else {
            return;
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx,
                    logical_id: nodes.slot_pan_id as u64,
                    fvalue: value,
                },
            );
        }
    }

    pub fn push_rack_slot_solo_mutes(&self, track: usize) {
        let Some(rack) = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .cloned()
        else {
            return;
        };
        let has_solo = rack.slots.iter().any(|slot| slot.solo);
        let Some(track_nodes) = self.graph.track_node_ids.get(track) else {
            return;
        };
        for (slot_idx, nodes) in track_nodes.rack_slots.iter().enumerate() {
            let muted_by_solo = has_solo && !rack.slots.get(slot_idx).is_some_and(|slot| slot.solo);
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                        logical_id: nodes.slot_pan_id as u64,
                        fvalue: if muted_by_solo { 1.0 } else { 0.0 },
                    },
                );
            }
        }
    }

    pub fn set_rack_slot_gain(&mut self, track: usize, slot_idx: usize, value: f32) -> bool {
        let value = value.clamp(0.0, 2.0);
        let updated = self
            .state
            .update_live_rack_slot(track, slot_idx, |slot| slot.gain = value);
        if updated {
            self.push_rack_slot_panner_param(
                track,
                slot_idx,
                crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                value,
            );
        }
        updated
    }

    pub fn set_rack_slot_pan(&mut self, track: usize, slot_idx: usize, value: f32) -> bool {
        let value = value.clamp(-1.0, 1.0);
        let updated = self
            .state
            .update_live_rack_slot(track, slot_idx, |slot| slot.pan = value);
        if updated {
            self.push_rack_slot_panner_param(
                track,
                slot_idx,
                crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                value,
            );
        }
        updated
    }

    pub fn set_rack_slot_mute(&mut self, track: usize, slot_idx: usize, value: bool) -> bool {
        let updated = self
            .state
            .update_live_rack_slot(track, slot_idx, |slot| slot.mute = value);
        if updated {
            self.push_rack_slot_panner_param(
                track,
                slot_idx,
                crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                if value { 1.0 } else { 0.0 },
            );
        }
        updated
    }

    pub fn set_rack_slot_solo(&mut self, track: usize, slot_idx: usize, value: bool) -> bool {
        let updated = self
            .state
            .update_live_rack_slot(track, slot_idx, |slot| slot.solo = value);
        if updated {
            self.push_rack_slot_solo_mutes(track);
        }
        updated
    }

    pub fn set_rack_slot_max_polyphony(
        &mut self,
        track: usize,
        slot_idx: usize,
        value: usize,
    ) -> bool {
        // Note: this can exceed the voice nodes actually built for this slot
        // (build_sampler_voices sizes the fan at slot-creation time). That's
        // fine — VoicePool::allocate_voice_retriggering_same_note_with_limit
        // already self-clamps to the pool's real voice count at allocation
        // time, so a higher value here just means "as polyphonic as the
        // built voices allow" rather than silently snapping the UI control
        // back down.
        self.state.update_live_rack_slot(track, slot_idx, |slot| {
            slot.max_polyphony = value.clamp(1, crate::audio::MAX_VOICES);
        })
    }

    pub fn set_rack_slot_choke_group(&mut self, track: usize, slot_idx: usize, value: u8) -> bool {
        self.state.update_live_rack_slot(track, slot_idx, |slot| {
            slot.choke_group = if value == 0 { None } else { Some(value) };
        })
    }

    pub fn set_rack_slot_base_note_offset(
        &mut self,
        track: usize,
        slot_idx: usize,
        value: f32,
    ) -> bool {
        let value = value.clamp(-48.0, 48.0);
        self.state.update_live_rack_slot(track, slot_idx, |slot| {
            slot.instrument_base_note_offset = value;
            slot.track_sound_state.dirty = true;
        })
    }

    pub fn set_rack_slot_param_plock(
        &mut self,
        track: usize,
        slot_idx: usize,
        step: usize,
        param: RackSlotParam,
        value: f32,
    ) -> bool {
        let value = param.clamp(value);
        self.state.update_live_rack_slot(track, slot_idx, |slot| {
            if slot.set_param_plock(step, param, value) && param == RackSlotParam::BaseNote {
                slot.track_sound_state.dirty = true;
            }
        })
    }

    pub fn send_rack_slot_instrument_param(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) {
        let Some(route) = self.rack_slot_instrument_param_route(track, slot_idx, param_idx) else {
            return;
        };
        let Some(nodes) = self
            .graph
            .track_node_ids
            .get(track)
            .and_then(|track_nodes| track_nodes.rack_slots.get(slot_idx))
        else {
            return;
        };
        let idx = route.node_param_idx;
        let span = route.node_param_span;
        if crate::instruments::voice_modulator::is_bar_resync_param(idx as u32) {
            self.state.schedule_mod_resync();
        }
        if route.instrument_type == InstrumentType::Sampler {
            // Slice controls are host-resolved at trigger time and deliberately
            // have no sampler/modulator DSP state cell to push live.
            if idx == u32::MAX as u64 {
                return;
            }
            let sample_rate = route
                .sample_rate
                .unwrap_or(self.graph.sample_rate.max(1) as f32);
            let (idx, fvalue) = match param_idx {
                0 => (
                    crate::instruments::sampler::PARAM_ATTACK_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                1 => (
                    crate::instruments::sampler::PARAM_RELEASE_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                2 => (crate::instruments::sampler::PARAM_START_POINT, value),
                3 => (crate::instruments::sampler::PARAM_END_POINT, value),
                4 => (crate::instruments::sampler::PARAM_ENABLED, value),
                5 => (crate::instruments::sampler::PARAM_REVERSE, value),
                6 => (crate::instruments::sampler::PARAM_LOOP_MODE, value),
                7 => (
                    crate::instruments::sampler::PARAM_LOOP_XFADE_SAMPLES,
                    value * sample_rate / 1000.0,
                ),
                8 => (crate::instruments::sampler::PARAM_SR_HZ, value),
                9 => (crate::instruments::sampler::PARAM_WARP_ENABLED, value),
                10 => (crate::instruments::sampler::PARAM_WARP_MODE, value),
                11 => (crate::instruments::sampler::PARAM_WARP_SAMPLE_BPM, value),
                _ => (idx, value),
            };
            if crate::instruments::sampler::srange_debug_enabled() && (param_idx == 2 || param_idx == 3) {
                eprintln!(
                    "[srange] rack live push track={} slot={} param={} value={} sampler_ids={}",
                    track,
                    slot_idx,
                    param_idx,
                    fvalue,
                    nodes.sampler_ids.len(),
                );
            }
            let is_mod_param = idx as u32 >= crate::instruments::voice_modulator::MOD_PARAM_BASE;
            let resolved_idx = if is_mod_param {
                idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64
            } else {
                idx
            };
            let target_ids: &[i32] = if is_mod_param {
                &nodes.sampler_modulator_ids
            } else {
                &nodes.sampler_ids
            };
            for &node_id in target_ids {
                unsafe {
                    for lane in 0..span as u64 {
                        crate::audiograph::params_push_wrapper(
                            self.graph.lg.0,
                            crate::audiograph::ParamMsg {
                                idx: resolved_idx + lane,
                                logical_id: node_id as u64,
                                fvalue,
                            },
                        );
                    }
                }
            }
            return;
        }

        let Some(engine_id) = nodes.engine_id else {
            return;
        };
        let Some(engine) = self
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref())
        else {
            return;
        };
        let is_mod_param = idx as u32 >= crate::instruments::voice_modulator::MOD_PARAM_BASE;
        let resolved_idx = if is_mod_param {
            idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        let target_ids = if is_mod_param {
            &engine.modulator_ids
        } else {
            &engine.synth_ids
        };
        for &node_id in target_ids {
            unsafe {
                for lane in 0..span as u64 {
                    crate::audiograph::params_push_wrapper(
                        self.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: resolved_idx + lane,
                            logical_id: node_id as u64,
                            fvalue: value,
                        },
                    );
                }
            }
        }
    }

    fn set_rack_slot_instrument_default_only(
        &mut self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) -> bool {
        let mut wrote = false;
        let slot_exists = self.state.update_live_rack_slot(track, slot_idx, |slot| {
            let param_count = slot.instrument_slot.num_params as usize;
            if param_idx < param_count {
                if slot.instrument_slot.defaults.len() < param_count {
                    slot.instrument_slot.defaults.resize(param_count, 0.0);
                }
                slot.instrument_slot.defaults[param_idx] = value;
                slot.track_sound_state.dirty = true;
                wrote = true;
            }
        });
        slot_exists && wrote
    }

    pub fn set_rack_slot_instrument_param(
        &mut self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) -> bool {
        let updated = self.set_rack_slot_instrument_default_only(track, slot_idx, param_idx, value);
        if updated {
            self.send_rack_slot_instrument_param(track, slot_idx, param_idx, value);
            self.sync_rack_slot_mod_active_default(track, slot_idx, param_idx);
        }
        updated
    }

    pub fn set_rack_slot_instrument_plock(
        &mut self,
        track: usize,
        slot_idx: usize,
        step: usize,
        param_idx: usize,
        value: f32,
    ) -> bool {
        let mut wrote = false;
        let slot_exists = self.state.update_live_rack_slot(track, slot_idx, |slot| {
            if slot.instrument_slot.set_plock(step, param_idx, value) {
                slot.track_sound_state.dirty = true;
                wrote = true;
            }
        });
        if slot_exists && wrote {
            self.sync_rack_slot_mod_active_plock(track, slot_idx, step, param_idx);
        }
        slot_exists && wrote
    }

    pub fn rename_rack_macro(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        name: String,
    ) -> bool {
        let name = name.trim().to_string();
        if name.is_empty() {
            return false;
        }
        self.state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.name = name.clone()
            })
    }

    pub fn set_rack_macro_plock(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        step: usize,
        value: f32,
    ) -> bool {
        if step >= crate::sequencer::MAX_STEPS {
            return false;
        }
        self.state
            .set_rack_macro_plocks_in_current_pattern(track, id, &[step], value)
    }

    pub fn set_rack_macro_plocks(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        steps: &[usize],
        value: f32,
    ) -> bool {
        self.state
            .set_rack_macro_plocks_in_current_pattern(track, id, steps, value)
    }

    pub fn clear_rack_macro_plock(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        step: usize,
    ) -> bool {
        if step >= crate::sequencer::MAX_STEPS {
            return false;
        }
        self.state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.plocks[step] = None;
            })
    }

    pub fn map_rack_macro(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        mapping: crate::sequencer::RackMacroMapping,
    ) -> Result<(), String> {
        if !mapping.range_min.is_finite() || !mapping.range_max.is_finite() {
            return Err("Rack macro mapping range must be finite".to_string());
        }
        let rack = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Clone::clone)
            .ok_or_else(|| "Track does not contain a rack".to_string())?;
        if rack
            .macros
            .iter()
            .flat_map(|rack_macro| rack_macro.mappings.iter())
            .any(|existing| existing.target == mapping.target)
        {
            return Err("Rack parameter is already owned by a rack macro".to_string());
        }
        if !self
            .state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.mappings.push(mapping.clone())
            })
        {
            return Err("Rack macro does not exist".to_string());
        }
        Ok(())
    }

    pub fn set_rack_macro_mapping_range(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        mapping_idx: usize,
        range_min: f32,
        range_max: f32,
    ) -> bool {
        if !range_min.is_finite() || !range_max.is_finite() {
            return false;
        }
        let mapping_exists = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))
            .is_some_and(|rack_macro| mapping_idx < rack_macro.mappings.len());
        if !mapping_exists {
            return false;
        }
        let updated = self
            .state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                let mapping = &mut rack_macro.mappings[mapping_idx];
                mapping.range_min = range_min;
                mapping.range_max = range_max;
            });
        if updated {
            self.state.publish_scheduler_snapshot();
        }
        updated
    }

    pub fn set_rack_macro_mapping_curve(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        mapping_idx: usize,
        curve: crate::sequencer::RackMacroCurve,
    ) -> bool {
        let mapping_exists = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))
            .is_some_and(|rack_macro| mapping_idx < rack_macro.mappings.len());
        if !mapping_exists {
            return false;
        }
        let updated = self
            .state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.mappings[mapping_idx].curve = curve;
            });
        if updated {
            self.state.publish_scheduler_snapshot();
        }
        updated
    }

    pub fn set_rack_macro_value(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        value: f32,
    ) -> bool {
        let value = value.clamp(0.0, 1.0);
        if !self
            .state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| rack_macro.value = value)
        {
            return false;
        }
        self.state.set_live_rack_macro_default(track, id, value);
        self.send_transient_rack_macro_value(track, id, value);
        true
    }

    pub(super) fn send_transient_rack_macro_value(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        value: f32,
    ) -> bool {
        let value = value.clamp(0.0, 1.0);
        let mappings = {
            let racks = self.state.pattern.rack_tracks.lock().unwrap();
            let Some(rack_macro) = racks
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.macros.get(id.index()))
            else {
                return false;
            };
            rack_macro
                .mappings
                .iter()
                .filter_map(|mapping| {
                    let target = match &mapping.target {
                        crate::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                            match param.replace('_', "-").as_str() {
                                "gain" => TransientRackMacroTarget::SlotGain { slot: *slot },
                                "pan" => TransientRackMacroTarget::SlotPan { slot: *slot },
                                "mute" => TransientRackMacroTarget::SlotMute { slot: *slot },
                                _ => return None,
                            }
                        }
                        crate::sequencer::RackMacroTarget::SlotInstrumentParam {
                            slot,
                            param_index,
                            ..
                        } => TransientRackMacroTarget::SlotInstrumentParam {
                            slot: *slot,
                            param_index: *param_index,
                        },
                        crate::sequencer::RackMacroTarget::SlotEffectParam {
                            slot,
                            effect_slot,
                            param_index,
                            ..
                        } => TransientRackMacroTarget::SlotEffectParam {
                            slot: *slot,
                            effect_slot: *effect_slot,
                            param_index: *param_index,
                        },
                    };
                    Some(TransientRackMacroMapping {
                        target,
                        range_min: mapping.range_min,
                        range_max: mapping.range_max,
                        curve: mapping.curve,
                    })
                })
                .collect::<Vec<_>>()
        };
        for mapping in mappings {
            let normalized = match mapping.curve {
                crate::sequencer::RackMacroCurve::Linear => value,
                crate::sequencer::RackMacroCurve::Exp => value * value,
                crate::sequencer::RackMacroCurve::Log => value.sqrt(),
            };
            let mapped = mapping.range_min + (mapping.range_max - mapping.range_min) * normalized;
            match mapping.target {
                TransientRackMacroTarget::SlotGain { slot } => self.push_rack_slot_panner_param(
                    track,
                    slot,
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    mapped.clamp(0.0, 2.0),
                ),
                TransientRackMacroTarget::SlotPan { slot } => self.push_rack_slot_panner_param(
                    track,
                    slot,
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    mapped.clamp(-1.0, 1.0),
                ),
                TransientRackMacroTarget::SlotMute { slot } => self.push_rack_slot_panner_param(
                    track,
                    slot,
                    crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                    if mapped >= 0.5 { 1.0 } else { 0.0 },
                ),
                TransientRackMacroTarget::SlotInstrumentParam { slot, param_index } => {
                    self.send_rack_slot_instrument_param(track, slot, param_index, mapped);
                }
                TransientRackMacroTarget::SlotEffectParam {
                    slot,
                    effect_slot,
                    param_index,
                } => {
                    let _ = self.send_rack_slot_effect_param(
                        track,
                        slot,
                        effect_slot,
                        param_index,
                        mapped,
                    );
                }
            }
        }
        true
    }

    pub fn effective_rack_macro_value(
        &self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        step: Option<usize>,
    ) -> Option<f32> {
        let racks = self.state.pattern.rack_tracks.lock().unwrap();
        let rack_macro = racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))?;
        if let Some(value) = step
            .and_then(|step| rack_macro.plocks.get(step))
            .and_then(|value| *value)
        {
            return Some(value.clamp(0.0, 1.0));
        }
        let key = crate::macro_engine::MacroParamKey::for_rack_macro(track, id.index() as u8);
        Some(
            self.macro_engine
                .effective_value(&key, rack_macro.value)
                .clamp(0.0, 1.0),
        )
    }

    pub fn unmap_rack_macro(
        &mut self,
        track: usize,
        id: crate::sequencer::RackMacroId,
        mapping_idx: usize,
    ) -> bool {
        let exists = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))
            .is_some_and(|rack_macro| mapping_idx < rack_macro.mappings.len());
        if !exists {
            return false;
        }
        let updated = self
            .state
            .update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.mappings.remove(mapping_idx);
            });
        if updated {
            self.state.publish_scheduler_snapshot();
        }
        updated
    }

    fn sync_rack_slot_mod_active_default(
        &mut self,
        track: usize,
        slot_idx: usize,
        changed_param_idx: usize,
    ) {
        let Some((active_param_idx, value)) =
            self.rack_slot_mod_active_value(track, slot_idx, None, changed_param_idx)
        else {
            return;
        };
        if self.set_rack_slot_instrument_default_only(track, slot_idx, active_param_idx, value) {
            self.send_rack_slot_instrument_param(track, slot_idx, active_param_idx, value);
        }
    }

    fn sync_rack_slot_mod_active_plock(
        &mut self,
        track: usize,
        slot_idx: usize,
        step: usize,
        changed_param_idx: usize,
    ) {
        let Some((active_param_idx, value)) =
            self.rack_slot_mod_active_value(track, slot_idx, Some(step), changed_param_idx)
        else {
            return;
        };
        self.state.update_live_rack_slot(track, slot_idx, |slot| {
            if slot
                .instrument_slot
                .set_plock(step, active_param_idx, value)
            {
                slot.track_sound_state.dirty = true;
            }
        });
    }

    /// Computes the derived modulation-active parameter while borrowing the
    /// live rack slot in place. Rack slots contain large p-lock grids and
    /// effect descriptors; cloning one for every pointer movement made even a
    /// parameter unrelated to modulation pay for copying the entire slot.
    fn rack_slot_mod_active_value(
        &self,
        track: usize,
        slot_idx: usize,
        step: Option<usize>,
        changed_param_idx: usize,
    ) -> Option<(usize, f32)> {
        let racks = self.state.pattern.rack_tracks.lock().unwrap();
        let slot = racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(slot_idx))?;

        let sampler_descriptor;
        let descriptor = match slot.instrument_type {
            InstrumentType::Sampler => {
                sampler_descriptor = EffectDescriptor::builtin_sampler();
                &sampler_descriptor
            }
            InstrumentType::Custom | InstrumentType::Modulator => {
                self.rack_slot_cached_instrument_descriptor(slot)?
            }
            InstrumentType::Rack => return None,
        };
        let active_param_idx = descriptor
            .instrument_modulation_targets
            .iter()
            .find(|target| target.depth_param_idx == changed_param_idx)
            .and_then(|target| target.active_param_idx)?;
        let active = descriptor
            .instrument_modulation_targets
            .iter()
            .filter(|target| target.active_param_idx == Some(active_param_idx))
            .any(|target| {
                step.and_then(|step| {
                    slot.instrument_slot
                        .plocks
                        .get(step)
                        .and_then(|step_plocks| step_plocks.get(target.depth_param_idx))
                        .copied()
                        .flatten()
                })
                .or_else(|| {
                    slot.instrument_slot
                        .defaults
                        .get(target.depth_param_idx)
                        .copied()
                })
                .unwrap_or_else(|| {
                    descriptor
                        .params
                        .get(target.depth_param_idx)
                        .map(|param| param.default)
                        .unwrap_or_default()
                })
                .abs()
                    > f32::EPSILON
            });
        Some((active_param_idx, if active { 1.0 } else { 0.0 }))
    }

    pub(super) fn push_instrument_defaults_for_track(&self, track: usize) {
        let slot = &self.state.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..num_params {
            self.send_effective_instrument_param(track, param_idx);
        }
    }

    pub(super) fn push_rack_slot_instrument_defaults_for_track(&self, track: usize) {
        let Some(rack) = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .cloned()
        else {
            return;
        };
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            for param_idx in 0..slot.instrument_slot.num_params as usize {
                let value = slot
                    .instrument_slot
                    .defaults
                    .get(param_idx)
                    .copied()
                    .unwrap_or_default();
                self.send_rack_slot_instrument_param(track, slot_idx, param_idx, value);
            }
        }
        self.push_rack_slot_solo_mutes(track);
    }

    pub fn force_instrument_enabled(&self, track: usize) -> bool {
        let Some(desc) = self.graph.instrument_descriptors.get(track) else {
            return false;
        };
        let Some(slot) = self.state.pattern.instrument_slots.get(track) else {
            return false;
        };
        let Some(enabled_idx) = slot.force_enabled_default(desc) else {
            return false;
        };

        self.send_instrument_param(track, enabled_idx, 1.0);
        true
    }

    /// Custom-instrument voices in Instrument mode are fully stamped per
    /// note-on (defaults + p-locks, fingerprint-gated in the audio thread).
    /// An out-of-band defaults push can only stomp p-locked values sitting
    /// on the voice nodes — and because a looping pattern keeps producing
    /// the same fingerprints, the audio thread then skips re-dispatch
    /// forever, so every later note plays base params. Same bug class the
    /// sampler exemption covers. Free-patch tracks keep the push: the idle
    /// drone has no note-ons to stamp it.
    pub(super) fn instrument_defaults_push_would_stomp(&self, track: usize) -> bool {
        self.graph.track_instrument_types.get(track) == Some(&InstrumentType::Custom)
            && self.graph.track_instrument_run_modes.get(track)
                != Some(&crate::sequencer::CustomInstrumentRunMode::FreePatch)
    }

    pub(super) fn push_all_restored_instrument_defaults(&self) {
        self.push_all_restored_instrument_defaults_except(0);
    }

    /// `push_all_restored_instrument_defaults` sparing the lanes in
    /// `hold_mask` — see `push_all_restored_defaults_except`.
    pub(super) fn push_all_restored_instrument_defaults_except(&self, hold_mask: u64) {
        for track in 0..self.tracks.len() {
            if track < 64 && hold_mask >> track & 1 == 1 {
                continue;
            }
            if self.graph.track_instrument_types.get(track) == Some(&InstrumentType::Rack) {
                self.push_rack_slot_instrument_defaults_for_track(track);
                continue;
            }
            if self.is_sampler_track(track) || self.instrument_defaults_push_would_stomp(track) {
                continue;
            }
            self.push_instrument_defaults_for_track(track);
        }
    }

    fn toggle_instrument_boolean(&mut self) {
        let track = self.ui.cursor_track;
        if self.ui.instrument_param_cursor == 0 {
            return;
        }
        let synth_indices = self.synth_param_indices(track);
        let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    (step, if current > 0.5 { 0.0 } else { 1.0 })
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let current = slot.defaults.get(param_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }

    fn toggle_mod_boolean(&mut self) {
        let track = self.ui.cursor_track;
        let mod_indices = self.mod_param_indices(track);
        let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    (step, if current > 0.5 { 0.0 } else { 1.0 })
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let current = slot.defaults.get(param_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }

    fn toggle_source_boolean(&mut self) {
        let track = self.ui.cursor_track;
        let source_indices = self.source_param_actual_indices(track);
        let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        if self.has_selection() {
            let new_vals: Vec<(usize, f32)> = self
                .selected_steps()
                .into_iter()
                .map(|step| {
                    let current = slot
                        .plocks
                        .get(step, param_idx)
                        .unwrap_or_else(|| slot.defaults.get(param_idx));
                    (step, if current > 0.5 { 0.0 } else { 1.0 })
                })
                .collect();
            for (step, value) in new_vals {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlock {
                        track,
                        step,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let current = slot.defaults.get(param_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            self.set_instrument_param_or_plock(track, param_idx, new_val);
        }
    }
}
