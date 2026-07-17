use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::atomic::Ordering;

use crate::effects::reverb;
use crate::effects::{
    BUILTIN_SLOT_COUNT, EffectDescriptor, EffectSlotState, ParamKind, SyncDivision,
};

use super::command::{AppCommand, apply_command};
use super::draw::rect_contains;
use super::{App, EffectPaneEntry, EffectTab, InputMode, ParamMouseDragTarget};

const SCENE_MACRO_DIFF_EPSILON: f32 = 1.0e-5;

fn scene_macro_launch_quantize(
    quantize: crate::macro_engine::StealQuantize,
) -> crate::quantized_launch::LaunchQuantize {
    match quantize {
        crate::macro_engine::StealQuantize::Off => crate::quantized_launch::LaunchQuantize::Off,
        crate::macro_engine::StealQuantize::Sixteenth => {
            crate::quantized_launch::LaunchQuantize::Sixteenth
        }
        crate::macro_engine::StealQuantize::Bar => crate::quantized_launch::LaunchQuantize::Bar,
    }
}

fn scene_macro_launch_target(
    config: &crate::macro_engine::SceneMacroConfig,
) -> crate::quantized_launch::PatternLaunchTarget {
    scene_macro_launch_target_for_scene(config, config.target_scene)
}

fn scene_macro_launch_target_for_scene(
    config: &crate::macro_engine::SceneMacroConfig,
    scene: usize,
) -> crate::quantized_launch::PatternLaunchTarget {
    match &config.track_mask {
        None => crate::quantized_launch::PatternLaunchTarget::Scene { scene },
        Some(mask) => crate::quantized_launch::PatternLaunchTarget::SceneTracks {
            scene,
            tracks: mask
                .iter()
                .enumerate()
                .filter_map(|(track, enabled)| enabled.then_some(track))
                .collect(),
        },
    }
}

fn scene_macro_curve(param: &crate::effects::ParamDescriptor) -> crate::macro_engine::MacroCurve {
    match param.scaling {
        crate::effects::ParamScaling::Linear => crate::macro_engine::MacroCurve::Linear,
        crate::effects::ParamScaling::Exponential => crate::macro_engine::MacroCurve::LogDomain,
    }
}

fn scene_values_differ(param: &crate::effects::ParamDescriptor, base: f32, target: f32) -> bool {
    (param.normalize(base) - param.normalize(target)).abs() > SCENE_MACRO_DIFF_EPSILON
}

fn append_scene_instrument_mappings(
    app: &App,
    track: usize,
    target: &crate::sequencer::TrackPatternData,
    mappings: &mut Vec<crate::macro_engine::MacroMapping>,
) {
    let (Some(descriptor), Some(live_slot)) = (
        app.graph.instrument_descriptors.get(track),
        app.state.pattern.instrument_slots.get(track),
    ) else {
        return;
    };
    if live_slot.node_id.load(Ordering::Relaxed) != target.instrument_slot.node_id {
        return;
    }
    for (param_idx, param) in descriptor.params.iter().enumerate() {
        let Some(&target_value) = target.instrument_slot.defaults.get(param_idx) else {
            continue;
        };
        let base = live_slot.defaults.get(param_idx);
        if !scene_values_differ(param, base, target_value) {
            continue;
        }
        let target = crate::process::ParamTarget::InstrumentParam {
            param: param.name.clone(),
            param_id: live_slot.param_node_id(param_idx),
        };
        if let Ok(mapping) = crate::macro_engine::MacroMapping::new_resolved(
            track,
            target,
            Some(param_idx),
            base,
            target_value,
            scene_macro_curve(param),
        ) {
            mappings.push(mapping);
        }
    }
}

fn append_scene_effect_mappings(
    app: &App,
    track: usize,
    target: &crate::sequencer::TrackPatternData,
    mappings: &mut Vec<crate::macro_engine::MacroMapping>,
) {
    for (slot_idx, descriptor) in app
        .graph
        .effect_descriptors
        .get(track)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let (Some(live_slot), Some(target_slot)) = (
            app.state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx)),
            target.effect_slots.get(slot_idx),
        ) else {
            continue;
        };
        if live_slot.node_id.load(Ordering::Relaxed) != target_slot.node_id {
            continue;
        }
        for (param_idx, param) in descriptor.params.iter().enumerate() {
            let Some(&target_value) = target_slot.defaults.get(param_idx) else {
                continue;
            };
            let base = live_slot.defaults.get(param_idx);
            if !scene_values_differ(param, base, target_value) {
                continue;
            }
            let target = crate::process::ParamTarget::EffectParam {
                slot: slot_idx,
                effect: descriptor.name.clone(),
                param: param.name.clone(),
                param_id: live_slot.param_node_id(param_idx),
            };
            if let Ok(mapping) = crate::macro_engine::MacroMapping::new_resolved(
                track,
                target,
                Some(param_idx),
                base,
                target_value,
                scene_macro_curve(param),
            ) {
                mappings.push(mapping);
            }
        }
    }
}

fn append_scene_bus_effect_mappings(
    app: &App,
    bus_idx: usize,
    target: &crate::sequencer::BusPatternSnapshot,
    mappings: &mut Vec<crate::macro_engine::MacroMapping>,
) {
    let Some(bus) = app.buses.get(bus_idx) else {
        return;
    };
    for (slot_idx, descriptor) in bus.effect_descriptors.iter().enumerate() {
        let (Some(live_slot), Some(target_defaults)) = (
            bus.effect_slots.get(slot_idx),
            target.effect_defaults.get(slot_idx),
        ) else {
            continue;
        };
        for (param_idx, param) in descriptor.params.iter().enumerate() {
            let (Some(&base), Some(&target_value)) = (
                live_slot.defaults.get(param_idx),
                target_defaults.get(param_idx),
            ) else {
                continue;
            };
            if !scene_values_differ(param, base, target_value) {
                continue;
            }
            let raw_idx = live_slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            let target = crate::process::ParamTarget::EffectParam {
                slot: slot_idx,
                effect: descriptor.name.clone(),
                param: param.name.clone(),
                param_id: crate::neural::ParamNodeId::from_slot_param(
                    live_slot.node_id,
                    live_slot.modulator_node_id,
                    raw_idx,
                ),
            };
            if let Ok(mapping) = crate::macro_engine::MacroMapping::new_resolved(
                bus.id,
                target,
                Some(param_idx),
                base,
                target_value,
                scene_macro_curve(param),
            ) {
                mappings.push(mapping);
            }
        }
    }
}

impl App {
    pub(super) fn effect_pane_entries(&self) -> Vec<EffectPaneEntry> {
        let mut entries = Vec::new();
        if self.is_current_custom_track() {
            entries.push(EffectPaneEntry::Tab(EffectTab::Synth));
            entries.push(EffectPaneEntry::Tab(EffectTab::Mod));
            entries.push(EffectPaneEntry::Tab(EffectTab::Sources));
        }
        for slot_idx in self.visible_effect_indices() {
            entries.push(EffectPaneEntry::Tab(EffectTab::Slot(slot_idx)));
        }
        entries.push(EffectPaneEntry::Tab(EffectTab::Reverb));
        if self.can_add_custom_effect() {
            entries.push(EffectPaneEntry::PlusButton);
        }
        entries
    }

    pub(super) fn sync_effect_tab_cursor(&mut self) {
        let entries = self.effect_pane_entries();
        if let Some(idx) = entries.iter().position(
            |entry| matches!(entry, EffectPaneEntry::Tab(tab) if *tab == self.ui.effect_tab),
        ) {
            self.ui.effect_tab_cursor = idx;
        } else {
            self.ui.effect_tab_cursor = self
                .ui
                .effect_tab_cursor
                .min(entries.len().saturating_sub(1));
            if let Some(EffectPaneEntry::Tab(tab)) = entries.get(self.ui.effect_tab_cursor) {
                self.ui.effect_tab = *tab;
            }
        }
    }

    pub(super) fn select_effect_tab(&mut self, tab: EffectTab) {
        self.ui.effect_tab = tab;
        if tab == EffectTab::Synth {
            self.ui.instrument_param_cursor = 0;
            self.ui.synth_scroll_offset = 0;
        } else if tab == EffectTab::Mod {
            self.ui.mod_param_cursor = 0;
            self.ui.mod_scroll_offset = 0;
        } else if tab == EffectTab::Sources {
            self.ui.source_param_cursor = 0;
            self.ui.source_scroll_offset = 0;
        } else if tab == EffectTab::Reverb {
            self.ui.reverb_param_cursor = 0;
        } else {
            self.ui.effect_param_cursor = 0;
            self.ui.effect_scroll_offset = 0;
        }
        self.sync_effect_tab_cursor();
    }

    pub(super) fn effect_row_count(&self) -> usize {
        self.current_slot_descriptor()
            .map(|desc| desc.params.len())
            .unwrap_or(0)
    }

    pub(super) fn clamp_effect_scroll(&mut self, area: ratatui::prelude::Rect) {
        self.ui.effect_scroll_offset = self.partition_scroll_offset(
            area,
            self.effect_row_count(),
            self.ui.effect_scroll_offset,
        );
    }

    pub(super) fn ensure_effect_cursor_visible(&mut self) {
        let area = self.ui.layout.effects_inner;
        (self.ui.effect_param_cursor, self.ui.effect_scroll_offset) = self
            .ensure_partition_cursor_visible(
                area,
                self.effect_row_count(),
                self.ui.effect_param_cursor,
                self.ui.effect_scroll_offset,
            );
    }

    pub(super) fn effect_row_at_position(
        &self,
        area: ratatui::prelude::Rect,
        col: u16,
        row: u16,
    ) -> Option<usize> {
        self.partition_row_at_position(
            area,
            col,
            row,
            self.effect_row_count(),
            self.ui.effect_scroll_offset,
        )
    }

    fn activate_effect_pane_cursor_entry(&mut self) {
        let entries = self.effect_pane_entries();
        let Some(entry) = entries.get(self.ui.effect_tab_cursor).copied() else {
            return;
        };
        match entry {
            EffectPaneEntry::Tab(tab) => self.select_effect_tab(tab),
            EffectPaneEntry::PlusButton => {
                self.editor.picker_items = crate::lisp_host::list_saved_effects();
                self.editor.picker_cursor = 0;
                self.editor.picker_filter.clear();
                self.ui.input_mode = InputMode::EffectPicker;
            }
        }
    }

    fn preview_effect_pane_cursor_entry(&mut self) {
        let entries = self.effect_pane_entries();
        let Some(entry) = entries.get(self.ui.effect_tab_cursor).copied() else {
            return;
        };
        if let EffectPaneEntry::Tab(tab) = entry {
            self.select_effect_tab(tab);
        }
    }

    pub(super) fn current_slot_descriptor(&self) -> Option<&EffectDescriptor> {
        if self.tracks.is_empty() {
            return None;
        }
        let slot_idx = self.selected_effect_slot()?;
        self.graph
            .effect_descriptors
            .get(self.ui.cursor_track)
            .and_then(|descs| descs.get(slot_idx))
    }

    pub(super) fn current_slot(&self) -> Option<&EffectSlotState> {
        if self.tracks.is_empty() {
            return None;
        }
        let slot_idx = self.selected_effect_slot()?;
        self.state
            .pattern
            .effect_chains
            .get(self.ui.cursor_track)
            .and_then(|chain| chain.get(slot_idx))
    }

    pub(super) fn visible_effect_indices(&self) -> Vec<usize> {
        if self.tracks.is_empty() {
            return Vec::new();
        }
        let track = self.ui.cursor_track;
        let descs = &self.graph.effect_descriptors[track];
        let mut visible = Vec::new();
        for i in 0..descs.len() {
            if i < BUILTIN_SLOT_COUNT || !descs[i].name.is_empty() {
                visible.push(i);
            }
        }
        visible
    }

    pub(super) fn can_add_custom_effect(&self) -> bool {
        self.next_free_custom_slot().is_some()
    }

    pub(super) fn handle_effects_column(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.sync_effect_tab_cursor();

        if self.ui.params_column == 0 {
            let entries = self.effect_pane_entries();
            match code {
                KeyCode::Left => {}
                KeyCode::Right | KeyCode::Enter => {
                    self.activate_effect_pane_cursor_entry();
                    if self.ui.input_mode == InputMode::Normal {
                        self.ui.params_column = 1;
                    }
                }
                KeyCode::Up => {
                    if self.ui.effect_tab_cursor > 0 {
                        self.ui.effect_tab_cursor -= 1;
                        self.preview_effect_pane_cursor_entry();
                        self.ui.params_column = 0;
                    }
                }
                KeyCode::Down => {
                    if self.ui.effect_tab_cursor + 1 < entries.len() {
                        self.ui.effect_tab_cursor += 1;
                        self.preview_effect_pane_cursor_entry();
                        self.ui.params_column = 0;
                    }
                }
                _ => {}
            }
            return;
        }

        if self.ui.effect_tab == EffectTab::Synth {
            self.handle_synth_tab_input(code, modifiers);
            return;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            self.handle_mod_tab_input(code, modifiers);
            return;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            self.handle_sources_tab_input(code, modifiers);
            return;
        }

        if self.ui.effect_tab == EffectTab::Reverb {
            self.handle_reverb_tab_input(code, modifiers);
            return;
        }

        self.handle_effect_slot_input(code, modifiers);
    }

    fn handle_reverb_tab_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Left => {
                self.ui.params_column = 0;
                self.sync_effect_tab_cursor();
            }
            KeyCode::Right => {}
            KeyCode::Up => {
                if shift {
                    self.adjust_reverb_param(0.05);
                } else if self.ui.reverb_param_cursor > 0 {
                    self.ui.reverb_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if shift {
                    self.adjust_reverb_param(-0.05);
                } else if self.ui.reverb_param_cursor < 2 {
                    self.ui.reverb_param_cursor += 1;
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                self.ui.value_buffer.clear();
                self.ui.value_buffer.push(c);
                self.ui.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step == 0 {
                    ns - 1
                } else {
                    self.ui.cursor_step - 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step + 1 >= ns {
                    0
                } else {
                    self.ui.cursor_step + 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            _ => {}
        }
    }

    fn handle_effect_slot_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);

        match code {
            KeyCode::Left => {
                self.ui.params_column = 0;
                self.sync_effect_tab_cursor();
            }
            KeyCode::Right => {}
            KeyCode::Up => {
                if shift {
                    self.adjust_slot_param(1.0);
                } else if self.ui.effect_param_cursor > 0 {
                    self.ui.effect_param_cursor -= 1;
                    self.ensure_effect_cursor_visible();
                }
            }
            KeyCode::Down => {
                if shift {
                    self.adjust_slot_param(-1.0);
                } else if let Some(desc) = self.current_slot_descriptor() {
                    let max = desc.params.len().saturating_sub(1);
                    if self.ui.effect_param_cursor < max {
                        self.ui.effect_param_cursor += 1;
                        self.ensure_effect_cursor_visible();
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.ui.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.ui.effect_param_cursor];
                        if param.is_boolean() {
                            self.toggle_slot_boolean();
                            self.update_delay_time_param_kind();
                        } else if param.is_enum() {
                            self.ui.dropdown_open = true;
                            self.ui.dropdown_cursor = 0;
                            self.ui.input_mode = InputMode::Dropdown;
                            let val = self.get_current_slot_value();
                            self.ui.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.ui.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.ui.effect_param_cursor];
                        if !param.is_boolean() {
                            self.ui.value_buffer.clear();
                            self.ui.value_buffer.push(c);
                            self.ui.input_mode = InputMode::ValueEntry;
                        }
                    }
                }
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step == 0 {
                    ns - 1
                } else {
                    self.ui.cursor_step - 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step + 1 >= ns {
                    0
                } else {
                    self.ui.cursor_step + 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            _ => {}
        }
    }

    pub(super) fn set_reverb_param(&mut self, cursor: usize, value: f32) {
        let clamped = value.clamp(0.0, 1.0);
        let param_idx = match cursor {
            0 => {
                self.ui.reverb_size = clamped;
                reverb::REVERB_PARAM_SIZE
            }
            1 => {
                self.ui.reverb_brightness = clamped;
                reverb::REVERB_PARAM_BRIGHT
            }
            2 => {
                self.ui.reverb_replace = clamped;
                reverb::REVERB_PARAM_REPLACE
            }
            _ => return,
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: param_idx,
                    logical_id: self.graph.reverb_node_id as u64,
                    fvalue: clamped,
                },
            );
        }
    }

    fn adjust_reverb_param(&mut self, delta: f32) {
        let current = match self.ui.reverb_param_cursor {
            0 => self.ui.reverb_size,
            1 => self.ui.reverb_brightness,
            2 => self.ui.reverb_replace,
            _ => return,
        };
        self.set_reverb_param(self.ui.reverb_param_cursor, current + delta);
    }

    fn adjust_slot_param(&mut self, direction: f32) {
        let track = self.ui.cursor_track;
        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let param_idx = self.ui.effect_param_cursor;

        let desc = match self
            .graph
            .effect_descriptors
            .get(track)
            .and_then(|d| d.get(slot_idx))
        {
            Some(d) => d,
            None => return,
        };
        if param_idx >= desc.params.len() {
            return;
        }
        let param_desc = &desc.params[param_idx];
        let is_host_sidechain = matches!(
            param_desc.host_control,
            Some(crate::effects::HostControl::FxSidechain { .. })
        );

        let chain = &self.state.pattern.effect_chains[track];
        if slot_idx >= chain.len() {
            return;
        }
        let slot = &chain[slot_idx];

        if is_host_sidechain {
            let old = slot.defaults.get(param_idx);
            let inc = param_desc.increment(old);
            let new_val = param_desc.clamp(old + direction * inc);
            self.apply_effect_sidechain_selection(track, slot_idx, param_idx, new_val as usize);
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track,
                    slot_idx,
                    param_idx,
                    value: new_val,
                },
            );
        } else if self.has_selection() {
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
                    AppCommand::SetEffectPlock {
                        track,
                        step,
                        slot_idx,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let slot = &self.state.pattern.effect_chains[track][slot_idx];
            let desc = &self.graph.effect_descriptors[track][slot_idx];
            let old = slot.defaults.get(param_idx);
            let inc = desc.params[param_idx].increment(old);
            let new_val = desc.params[param_idx].clamp(old + direction * inc);
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track,
                    slot_idx,
                    param_idx,
                    value: new_val,
                },
            );
        }
    }

    pub(super) fn send_slot_param(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) {
        if self.graph.lg.0.is_null() {
            return;
        }
        let chain = &self.state.pattern.effect_chains[track];
        if slot_idx >= chain.len() {
            return;
        }
        let slot = &chain[slot_idx];
        let Some(desc) = self
            .graph
            .effect_descriptors
            .get(track)
            .and_then(|d| d.get(slot_idx))
        else {
            return;
        };
        if desc
            .params
            .get(param_idx)
            .and_then(|p| p.host_control.as_ref())
            .is_some()
        {
            return;
        }
        let idx = slot.resolve_node_idx(param_idx);
        let (logical_id, idx) = if idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE {
            let modulator_node_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if modulator_node_id == 0 {
                return;
            }
            (
                modulator_node_id as u64,
                idx - crate::voice_modulator::MOD_PARAM_BASE as u64,
            )
        } else {
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                return;
            }
            (node_id as u64, idx)
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx,
                    logical_id,
                    fvalue: value,
                },
            );
        }
    }

    pub fn effective_slot_param_value(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
    ) -> Option<f32> {
        let slot = self.state.pattern.effect_chains.get(track)?.get(slot_idx)?;
        if param_idx >= slot.num_params.load(Ordering::Relaxed) as usize {
            return None;
        }
        let raw_idx = slot.resolve_node_idx(param_idx) as u32;
        let param_id = crate::neural::ParamNodeId::from_slot_param(
            slot.node_id.load(Ordering::Relaxed),
            slot.modulator_node_id.load(Ordering::Relaxed),
            raw_idx,
        );
        let key =
            crate::macro_engine::MacroParamKey::for_effect(track, slot_idx, param_idx, param_id);
        Some(
            self.macro_engine
                .effective_value(&key, slot.defaults.get(param_idx)),
        )
    }

    /// Sends the current base value unless an engaged macro owns this param.
    pub(super) fn send_effective_slot_param(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
    ) {
        let Some(value) = self.effective_slot_param_value(track, slot_idx, param_idx) else {
            return;
        };
        self.send_slot_param(track, slot_idx, param_idx, value);
    }

    pub fn effective_bus_slot_param_value(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
    ) -> Option<f32> {
        let bus = self.buses.get(bus_idx)?;
        let slot = bus.effect_slots.get(slot_idx)?;
        let base = *slot.defaults.get(param_idx)?;
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let param_id = crate::neural::ParamNodeId::from_slot_param(
            slot.node_id,
            slot.modulator_node_id,
            raw_idx,
        );
        let key = crate::macro_engine::MacroParamKey::for_bus_effect(
            bus.id, slot_idx, param_idx, param_id,
        );
        Some(self.macro_engine.effective_value(&key, base))
    }

    pub(super) fn send_effective_bus_slot_param(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        param_idx: usize,
    ) {
        let Some(value) = self.effective_bus_slot_param_value(bus_idx, slot_idx, param_idx) else {
            return;
        };
        let Some(bus) = self.buses.get(bus_idx) else {
            return;
        };
        let Some(slot) = bus.effect_slots.get(slot_idx) else {
            return;
        };
        let Some(param) = bus
            .effect_descriptors
            .get(slot_idx)
            .and_then(|descriptor| descriptor.params.get(param_idx))
        else {
            return;
        };
        self.push_bus_effect_param_to_graph(
            slot.node_id,
            slot.modulator_node_id,
            param.node_param_idx,
            param.node_param_span,
            value,
        );
    }

    /// Applies a live macro position and re-sends every affected target.
    /// Phase 2 commands call this entry point rather than mutating the engine
    /// directly so both engagement and release reach the DSP immediately.
    pub fn set_macro_value(&mut self, id: crate::macro_engine::MacroId, value: f32) {
        let scene_config = self.macro_engine.scene_config(id).cloned();
        let touched = if let Some(config) = scene_config {
            if value > 0.0 && !self.macro_engine.is_engaged(id) {
                self.engage_scene_macro(id, &config, value)
            } else if self.macro_engine.is_engaged(id) {
                self.macro_engine.set_value(id, value)
            } else {
                Vec::new()
            }
        } else {
            self.macro_engine.set_value(id, value)
        };
        self.send_macro_targets(touched);
    }

    pub fn release_macro(&mut self, id: crate::macro_engine::MacroId) {
        let defer_release =
            self.macro_engine.scene_config(id).is_some() && self.release_scene_macro_patterns(id);
        let touched = if defer_release {
            self.macro_engine.set_value(id, 0.0)
        } else {
            self.macro_engine.release(id)
        };
        self.send_macro_targets(touched);
    }

    /// Begins the transport's ephemeral Shift gesture. This deliberately
    /// morphs parameter snapshots only: pattern switching is discrete and
    /// therefore has no meaningful value between 0 and 1.
    pub fn begin_scene_push(&mut self, target_scene: usize, value: f32) {
        if target_scene >= self.state.scene_count() {
            return;
        }
        let config = crate::macro_engine::SceneMacroConfig {
            target_scene,
            morph_params: true,
            steal_patterns: false,
            quantize: crate::macro_engine::StealQuantize::Off,
            track_mask: None,
        };
        let mappings = self.scene_macro_mappings(&config);
        let touched = self.macro_engine.begin_scene_push(mappings, value);
        self.send_macro_targets(touched);
    }

    pub fn set_scene_push_value(&mut self, value: f32) {
        let touched = self.macro_engine.set_scene_push_value(value);
        self.send_macro_targets(touched);
    }

    pub fn end_scene_push(&mut self) {
        let touched = self.macro_engine.end_scene_push();
        self.send_macro_targets(touched);
    }

    fn engage_scene_macro(
        &mut self,
        id: crate::macro_engine::MacroId,
        config: &crate::macro_engine::SceneMacroConfig,
        value: f32,
    ) -> Vec<crate::macro_engine::ScopedParamTarget> {
        if config.target_scene >= self.state.scene_count() {
            return Vec::new();
        }
        let mappings = self.scene_macro_mappings(config);
        let touched = self
            .macro_engine
            .engage_scene(id, mappings, value)
            .unwrap_or_default();
        let mut runtime = super::SceneMacroRuntime {
            origin_scene: self.state.current_scene_index(),
            target_token: None,
            return_token: None,
            target_applied: config.target_scene == self.state.current_scene_index(),
        };
        if config.steal_patterns && config.target_scene != runtime.origin_scene {
            let target = scene_macro_launch_target(config);
            let quantize = scene_macro_launch_quantize(config.quantize);
            runtime.target_token = self
                .state
                .schedule_quantized_pattern_launch(
                    target,
                    quantize,
                    crate::quantized_launch::QuantizedLaunchOwner::SceneMacro(id),
                )
                .ok();
        }
        self.scene_macro_runtime.insert(id, runtime);
        touched
    }

    fn release_scene_macro_patterns(&mut self, id: crate::macro_engine::MacroId) -> bool {
        let owner = crate::quantized_launch::QuantizedLaunchOwner::SceneMacro(id);
        let _ = self.state.quantized_launches().cancel_owner(owner);
        let Some(mut runtime) = self.scene_macro_runtime.remove(&id) else {
            return false;
        };
        if !runtime.target_applied || runtime.origin_scene == self.state.current_scene_index() {
            return false;
        }
        let Some(config) = self.macro_engine.scene_config(id).cloned() else {
            return false;
        };
        runtime.return_token = self
            .state
            .schedule_quantized_pattern_launch(
                scene_macro_launch_target_for_scene(&config, runtime.origin_scene),
                scene_macro_launch_quantize(config.quantize),
                owner,
            )
            .ok();
        let scheduled = runtime.return_token.is_some();
        self.scene_macro_runtime.insert(id, runtime);
        scheduled
    }

    pub(super) fn cancel_scene_macro(&mut self, id: crate::macro_engine::MacroId) {
        let _ = self.state.quantized_launches().cancel_owner(
            crate::quantized_launch::QuantizedLaunchOwner::SceneMacro(id),
        );
        self.scene_macro_runtime.remove(&id);
    }

    fn scene_macro_mappings(
        &self,
        config: &crate::macro_engine::SceneMacroConfig,
    ) -> Vec<crate::macro_engine::MacroMapping> {
        if !config.morph_params {
            return Vec::new();
        }
        let mut mappings = Vec::new();
        for track in 0..self.tracks.len() {
            if config
                .track_mask
                .as_ref()
                .is_some_and(|mask| !mask.get(track).copied().unwrap_or(false))
            {
                continue;
            }
            self.state
                .with_scene_track_pattern(config.target_scene, track, |target| {
                    append_scene_instrument_mappings(self, track, target, &mut mappings);
                    append_scene_effect_mappings(self, track, target, &mut mappings);
                });
        }
        let current_bus_snapshot = self.capture_bus_pattern_snapshot();
        let target_buses = self
            .state
            .bus_pattern_snapshot_or_default(config.target_scene, &current_bus_snapshot);
        for (bus_idx, bus) in self.buses.iter().enumerate() {
            if let Some(target) = target_buses.iter().find(|target| target.id == bus.id) {
                append_scene_bus_effect_mappings(self, bus_idx, target, &mut mappings);
            }
        }
        mappings
    }

    pub fn scene_macro_diff_count(&self, config: &crate::macro_engine::SceneMacroConfig) -> usize {
        self.scene_macro_mappings(config).len()
    }

    pub(super) fn send_macro_targets(
        &mut self,
        touched: Vec<crate::macro_engine::ScopedParamTarget>,
    ) {
        self.state
            .publish_macro_overrides(self.macro_engine.override_snapshot());
        let mut bus_touched = false;
        for (scope, target) in touched {
            match (scope, target) {
                (
                    crate::macro_engine::ParamScope::Track(track),
                    crate::process::ParamTarget::EffectParam {
                        slot,
                        effect,
                        param,
                        ..
                    },
                ) => {
                    let Some(param_idx) = self
                        .graph
                        .effect_descriptors
                        .get(track)
                        .and_then(|descriptors| descriptors.get(slot))
                        .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(&effect))
                        .and_then(|descriptor| {
                            descriptor
                                .params
                                .iter()
                                .position(|descriptor| descriptor.has_tag_or_name(&param))
                        })
                    else {
                        continue;
                    };
                    self.send_effective_slot_param(track, slot, param_idx);
                }
                (
                    crate::macro_engine::ParamScope::Track(track),
                    crate::process::ParamTarget::InstrumentParam { param, .. },
                ) => {
                    let Some(param_idx) =
                        self.graph
                            .instrument_descriptors
                            .get(track)
                            .and_then(|descriptor| {
                                descriptor
                                    .params
                                    .iter()
                                    .position(|descriptor| descriptor.has_tag_or_name(&param))
                            })
                    else {
                        continue;
                    };
                    self.send_effective_instrument_param(track, param_idx);
                }
                (
                    crate::macro_engine::ParamScope::Bus(bus_id),
                    crate::process::ParamTarget::EffectParam {
                        slot,
                        effect,
                        param,
                        ..
                    },
                ) => {
                    bus_touched = true;
                    let Some(bus_idx) = self.buses.iter().position(|bus| bus.id == bus_id) else {
                        continue;
                    };
                    let Some(param_idx) = self
                        .buses
                        .get(bus_idx)
                        .and_then(|bus| bus.effect_descriptors.get(slot))
                        .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(&effect))
                        .and_then(|descriptor| {
                            descriptor
                                .params
                                .iter()
                                .position(|descriptor| descriptor.has_tag_or_name(&param))
                        })
                    else {
                        continue;
                    };
                    self.send_effective_bus_slot_param(bus_idx, slot, param_idx);
                }
                _ => {}
            }
        }
        if bus_touched {
            self.publish_bus_gate_runtime();
        }
    }

    /// Resolves a host target against the live descriptor and captures its
    /// absolute device range and stable node identity at mapping time.
    pub(super) fn map_macro_param(
        &mut self,
        id: crate::macro_engine::MacroId,
        track: usize,
        target: crate::process::ParamTarget,
    ) -> Result<(), crate::macro_engine::MacroEngineError> {
        let (target, param_idx, min, max) = match target {
            crate::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            } => {
                let Some(descriptor) = self
                    .graph
                    .effect_descriptors
                    .get(track)
                    .and_then(|descriptors| descriptors.get(slot))
                    .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(&effect))
                else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let Some(param_idx) = descriptor
                    .params
                    .iter()
                    .position(|descriptor| descriptor.has_tag_or_name(&param))
                else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let param_descriptor = &descriptor.params[param_idx];
                let Some(slot_state) = self
                    .state
                    .pattern
                    .effect_chains
                    .get(track)
                    .and_then(|chain| chain.get(slot))
                else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let target = crate::process::ParamTarget::EffectParam {
                    slot,
                    effect: descriptor.name.clone(),
                    param: param_descriptor.name.clone(),
                    param_id: slot_state.param_node_id(param_idx),
                };
                (
                    target,
                    param_idx,
                    param_descriptor.min,
                    param_descriptor.max,
                )
            }
            crate::process::ParamTarget::InstrumentParam { param, .. } => {
                let Some(descriptor) = self.graph.instrument_descriptors.get(track) else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let Some(param_idx) = descriptor
                    .params
                    .iter()
                    .position(|descriptor| descriptor.has_tag_or_name(&param))
                else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let param_descriptor = &descriptor.params[param_idx];
                let Some(slot_state) = self.state.pattern.instrument_slots.get(track) else {
                    return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget);
                };
                let target = crate::process::ParamTarget::InstrumentParam {
                    param: param_descriptor.name.clone(),
                    param_id: slot_state.param_node_id(param_idx),
                };
                (
                    target,
                    param_idx,
                    param_descriptor.min,
                    param_descriptor.max,
                )
            }
            _ => return Err(crate::macro_engine::MacroEngineError::UnsupportedTarget),
        };
        let resend_target = target.clone();
        let mapping = crate::macro_engine::MacroMapping::new_resolved(
            track,
            target,
            Some(param_idx),
            min,
            max,
            crate::macro_engine::MacroCurve::Linear,
        )?;
        self.macro_engine.add_mapping(id, mapping)?;
        let engaged = self.macro_engine.is_engaged(id);
        if engaged {
            self.send_macro_targets(vec![(
                crate::macro_engine::ParamScope::Track(track),
                resend_target,
            )]);
        }
        Ok(())
    }

    fn toggle_slot_boolean(&mut self) {
        let param_idx = self.ui.effect_param_cursor;
        let track = self.ui.cursor_track;
        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let Some(slot) = self.current_slot() else {
            return;
        };

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
                    AppCommand::SetEffectPlock {
                        track,
                        step,
                        slot_idx,
                        param_idx,
                        value,
                    },
                );
            }
        } else {
            let current = slot.defaults.get(param_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track,
                    slot_idx,
                    param_idx,
                    value: new_val,
                },
            );
        }
    }

    fn update_delay_time_param_kind(&mut self) {
        const SYNCED_PARAM: usize = 1;
        const TIME_PARAM: usize = 2;

        let EffectTab::Slot(slot_idx) = self.ui.effect_tab else {
            return;
        };
        if self.ui.effect_param_cursor != SYNCED_PARAM {
            return;
        }

        let track = self.ui.cursor_track;
        let slot = match self
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|c| c.get(slot_idx))
        {
            Some(s) => s,
            None => return,
        };
        let synced = slot.defaults.get(SYNCED_PARAM) > 0.5;

        let desc = match self
            .graph
            .effect_descriptors
            .get_mut(track)
            .and_then(|d| d.get_mut(slot_idx))
        {
            Some(d) => d,
            None => return,
        };
        if desc.name != "Delay" {
            return;
        }
        if TIME_PARAM >= desc.params.len() {
            return;
        }

        if synced {
            let labels: Vec<String> = SyncDivision::ALL
                .iter()
                .map(|d| d.label().to_string())
                .collect();
            desc.params[TIME_PARAM].kind = ParamKind::Enum { labels };
            desc.params[TIME_PARAM].min = 0.0;
            desc.params[TIME_PARAM].max = (SyncDivision::ALL.len() - 1) as f32;
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track,
                    slot_idx,
                    param_idx: TIME_PARAM,
                    value: 6.0,
                },
            );
        } else {
            desc.params[TIME_PARAM].kind = ParamKind::Continuous {
                unit: Some("ms".to_string()),
            };
            desc.params[TIME_PARAM].min = 1.0;
            desc.params[TIME_PARAM].max = 2000.0;
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track,
                    slot_idx,
                    param_idx: TIME_PARAM,
                    value: 250.0,
                },
            );
        }
    }

    fn get_current_slot_value(&self) -> f32 {
        match self.current_slot() {
            Some(slot) => slot.defaults.get(self.ui.effect_param_cursor),
            None => 0.0,
        }
    }

    fn scrub_param_display_value(
        &self,
        param_desc: &crate::effects::ParamDescriptor,
        start_display_value: f32,
        dx: i32,
        cells_for_full_range: f32,
    ) -> f32 {
        let display_min = param_desc.stored_to_user(param_desc.min);
        let display_max = param_desc.stored_to_user(param_desc.max);
        let display_range = (display_max - display_min).abs();
        let cells_for_full_range = cells_for_full_range.max(8.0);
        match &param_desc.kind {
            ParamKind::Boolean => {
                if dx >= 2 {
                    1.0
                } else if dx <= -2 {
                    0.0
                } else {
                    start_display_value
                }
            }
            ParamKind::Enum { .. } => {
                let step = (dx as f32 / 2.0).round();
                (start_display_value + step).clamp(display_min, display_max)
            }
            ParamKind::Continuous { .. } => {
                let sensitivity = if display_range > 0.0 {
                    display_range / cells_for_full_range
                } else {
                    0.0
                };
                (start_display_value + dx as f32 * sensitivity).clamp(display_min, display_max)
            }
        }
    }

    fn instrument_drag_cells_for_full_range(&self, total_rows: usize) -> f32 {
        self.instrument_column_width(self.ui.layout.effects_inner, total_rows)
            .max(8) as f32
    }

    pub(super) fn effect_row_display_value(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
    ) -> Option<f32> {
        let desc = self.graph.effect_descriptors.get(track)?.get(slot_idx)?;
        let param_desc = desc.params.get(param_idx)?;
        let slot = self.state.pattern.effect_chains.get(track)?.get(slot_idx)?;
        Some(param_desc.stored_to_user(slot.defaults.get(param_idx)))
    }

    pub(super) fn apply_param_mouse_drag(&mut self, col: u16, row: u16) {
        let Some(drag) = self.ui.param_mouse_drag else {
            return;
        };
        if drag.track >= self.tracks.len() || drag.track != self.ui.cursor_track {
            return;
        }

        let dx = col as i32 - drag.start_col as i32;
        match drag.target {
            ParamMouseDragTarget::CirklonStepParam { step } => {
                let step = if rect_contains(self.ui.layout.bars, col, row) {
                    self.step_from_click_x(col, self.ui.layout.bars.x)
                        .unwrap_or(step)
                } else {
                    step
                };
                let Some(value) =
                    self.step_param_value_from_bar_position(self.ui.active_param, row)
                else {
                    return;
                };
                self.ui.cursor_step = step;
                let param = self.ui.active_param;
                let track = drag.track;
                apply_command(
                    self,
                    AppCommand::SetStepParam {
                        track,
                        step,
                        param,
                        value,
                    },
                );
            }
            ParamMouseDragTarget::TrackListVolume => {
                let layout = super::cirklon::track_list_row_layout(self.ui.layout.track_list);
                let inner_width = layout.volume_inner_width.max(1);
                let clamped_col = col.clamp(
                    layout.volume_inner_x,
                    layout.volume_inner_x + inner_width - 1,
                );
                let rel = clamped_col - layout.volume_inner_x;
                let volume = if inner_width <= 1 {
                    0.0
                } else {
                    rel as f32 / (inner_width - 1) as f32
                };
                let apply_bulk = self.has_track_selection() && {
                    let (lo, hi) = self.track_selected_range();
                    drag.track >= lo && drag.track <= hi
                };
                let tracks = if apply_bulk {
                    self.selected_tracks()
                } else {
                    vec![drag.track]
                };
                for track in tracks {
                    apply_command(
                        self,
                        AppCommand::SetTrackVolume {
                            track,
                            value: volume,
                        },
                    );
                }
            }
            ParamMouseDragTarget::TrackParam { row_idx } => {
                let sdv = drag.start_display_value;
                let tracks = self.selected_tracks();
                match row_idx {
                    super::TP_ATTACK => {
                        let ms = (sdv + dx as f32 * 5.0).clamp(0.0, 500.0);
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackAttack { track, ms });
                        }
                    }
                    super::TP_RELEASE => {
                        let ms = (sdv + dx as f32 * 10.0).clamp(0.0, 2000.0);
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackRelease { track, ms });
                        }
                    }
                    super::TP_SWING => {
                        let value = (sdv + dx as f32 * 0.5).clamp(50.0, 75.0);
                        for track in tracks {
                            self.set_track_swing_or_plock(track, value);
                        }
                    }
                    super::TP_STEPS => {
                        let n = (sdv + (dx as f32 / 2.0).round())
                            .clamp(1.0, crate::sequencer::MAX_STEPS as f32)
                            as usize;
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackNumSteps { track, n });
                        }
                    }
                    super::TP_VOLUME => {
                        let value = (sdv + dx as f32 * 0.01).clamp(0.0, 1.0);
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackVolume { track, value });
                        }
                    }
                    super::TP_PAN => {
                        let value = (sdv + dx as f32 * 0.01).clamp(-1.0, 1.0);
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackPan { track, value });
                        }
                    }
                    super::TP_SEND => {
                        let value = (sdv + dx as f32 * 0.01).clamp(0.0, 1.0);
                        for track in tracks {
                            apply_command(self, AppCommand::SetTrackSend { track, value });
                        }
                    }
                    super::TP_MASTER => {
                        apply_command(
                            self,
                            AppCommand::SetMasterVolume {
                                value: (sdv + dx as f32 * 0.01).clamp(0.0, 2.0),
                            },
                        );
                    }
                    _ => {}
                }
            }
            ParamMouseDragTarget::AccumParam { row_idx } => {
                if row_idx == super::AC_LIMIT {
                    let value = (drag.start_display_value + dx as f32).max(0.0);
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::SetTrackAccumLimit { track, value });
                    }
                }
            }
            ParamMouseDragTarget::SynthParam { row_idx } => {
                let drag_scale = self.instrument_drag_cells_for_full_range(self.synth_row_count());
                if row_idx == 0 {
                    let sensitivity = 96.0 / drag_scale;
                    let new_val =
                        (drag.start_display_value + dx as f32 * sensitivity).clamp(-48.0, 48.0);
                    self.set_instrument_base_note_offset(drag.track, new_val);
                    return;
                }

                let synth_indices = self.synth_param_indices(drag.track);
                let Some(&param_idx) = synth_indices.get(row_idx - 1) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display = self.scrub_param_display_value(
                    param_desc,
                    drag.start_display_value,
                    dx,
                    drag_scale,
                );
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::ModParam { row_idx } => {
                let drag_scale = self.instrument_drag_cells_for_full_range(self.mod_row_count());
                let mod_indices = self.mod_param_indices(drag.track);
                let Some(&param_idx) = mod_indices.get(row_idx) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display = self.scrub_param_display_value(
                    param_desc,
                    drag.start_display_value,
                    dx,
                    drag_scale,
                );
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::SourceParam { row_idx } => {
                let drag_scale = self.instrument_drag_cells_for_full_range(self.source_row_count());
                let source_indices = self.source_param_actual_indices(drag.track);
                let Some(&param_idx) = source_indices.get(row_idx) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display = self.scrub_param_display_value(
                    param_desc,
                    drag.start_display_value,
                    dx,
                    drag_scale,
                );
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::EffectParam {
                slot_idx,
                param_idx,
            } => {
                let Some(desc) = self
                    .graph
                    .effect_descriptors
                    .get(drag.track)
                    .and_then(|d| d.get(slot_idx))
                else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx, 48.0);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                let Some(slot) = self
                    .state
                    .pattern
                    .effect_chains
                    .get(drag.track)
                    .and_then(|c| c.get(slot_idx))
                else {
                    return;
                };
                let track = drag.track;
                if matches!(
                    param_desc.host_control,
                    Some(crate::effects::HostControl::FxSidechain { .. })
                ) {
                    let selection = new_stored.round().max(0.0) as usize;
                    self.apply_effect_sidechain_selection(track, slot_idx, param_idx, selection);
                    apply_command(
                        self,
                        AppCommand::SetEffectParam {
                            track,
                            slot_idx,
                            param_idx,
                            value: selection as f32,
                        },
                    );
                } else {
                    apply_command(
                        self,
                        AppCommand::SetEffectParam {
                            track,
                            slot_idx,
                            param_idx,
                            value: new_stored,
                        },
                    );
                }
            }
            ParamMouseDragTarget::ReverbParam { param_idx } => {
                let sensitivity = 1.0 / 48.0;
                self.set_reverb_param(
                    param_idx,
                    drag.start_display_value + dx as f32 * sensitivity,
                );
            }
        }
    }
}
