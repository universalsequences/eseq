use std::sync::atomic::Ordering;

use crate::sequencer::{BusId, SwingResolution, Timebase};

use super::command::{apply_command, AppCommand};
use crate::accumulator::{AccumMode, ACCUMULATOR_REGISTRY};

use super::{App, EffectTab, AC_FN, AC_LAST, AC_MODE, TP_FTS, TP_LAST, TP_SWING_RESOLUTION};

/// Static labels for the timebase dropdown (derived from Timebase::LABELS).
const TIMEBASE_LABELS: [&str; Timebase::COUNT] = Timebase::LABELS;
const SWING_RESOLUTION_LABELS: [&str; SwingResolution::COUNT] = SwingResolution::LABELS;
const TOOLS_ROW_COUNT: usize = TP_LAST + AC_LAST + 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolRow {
    Track(usize),
    Accum(usize),
}

// ── App impl: params input ──

impl App {
    fn displayed_track_swing(&self, track: usize) -> (f32, bool) {
        let tp = &self.state.pattern.track_params[track];
        if self.has_selection() {
            let step = self.selected_steps()[0];
            if let Some(value) = self.state.pattern.swing_plocks[track].get(step) {
                return (value, true);
            }
        }
        (tp.get_swing(), false)
    }

    fn displayed_track_swing_resolution(&self, track: usize) -> (SwingResolution, bool) {
        let tp = &self.state.pattern.track_params[track];
        if self.has_selection() {
            let step = self.selected_steps()[0];
            if let Some(value) = self.state.pattern.swing_resolution_plocks[track].get(step) {
                return (value, true);
            }
        }
        (tp.get_swing_resolution(), false)
    }

    pub(super) fn set_track_swing_or_plock(&mut self, track: usize, value: f32) {
        if self.has_selection() {
            apply_command(
                self,
                AppCommand::SetTrackSwingPlockMulti {
                    track,
                    steps: self.selected_steps(),
                    value,
                },
            );
        } else {
            apply_command(self, AppCommand::SetTrackSwing { track, value });
        }
    }

    fn adjust_track_swing_or_plock(&mut self, track: usize, delta: f32) {
        if self.has_selection() {
            let default = self.state.pattern.track_params[track].get_swing();
            for step in self.selected_steps() {
                let current = self.state.pattern.swing_plocks[track]
                    .get(step)
                    .unwrap_or(default);
                apply_command(
                    self,
                    AppCommand::SetTrackSwingPlock {
                        track,
                        step,
                        value: Some(current + delta),
                    },
                );
            }
        } else {
            apply_command(self, AppCommand::AdjustTrackSwing { track, delta });
        }
    }

    fn cycle_track_swing_resolution_or_plock(&mut self, track: usize, next: bool) {
        if self.has_selection() {
            let default = self.state.pattern.track_params[track].get_swing_resolution();
            for step in self.selected_steps() {
                let current = self.state.pattern.swing_resolution_plocks[track]
                    .get(step)
                    .unwrap_or(default);
                let resolution = if next { current.next() } else { current.prev() };
                apply_command(
                    self,
                    AppCommand::SetTrackSwingResolutionPlock {
                        track,
                        step,
                        resolution: Some(resolution),
                    },
                );
            }
        } else if next {
            apply_command(self, AppCommand::NextTrackSwingResolution { track });
        } else {
            apply_command(self, AppCommand::PrevTrackSwingResolution { track });
        }
    }

    fn accumulator_names(&self) -> Vec<String> {
        let mut names = ACCUMULATOR_REGISTRY
            .iter()
            .map(|def| def.name.to_string())
            .collect::<Vec<_>>();
        if let Some(runtime) = self.editor.scratch_runtime.as_ref() {
            names.extend(runtime.accumulator_names());
        }
        names
    }

    fn selected_accumulator_dropdown_index(&self, track: usize) -> usize {
        let tp = &self.state.pattern.track_params[track];
        if let Some(name) = tp.script_accumulator_name() {
            let builtin_count = ACCUMULATOR_REGISTRY.len();
            if let Some(runtime) = self.editor.scratch_runtime.as_ref() {
                if let Some(script_idx) = runtime
                    .accumulator_names()
                    .iter()
                    .position(|entry| entry == &name)
                {
                    return builtin_count + script_idx;
                }
            }
        }
        tp.get_accumulator_idx()
    }

    pub(super) fn for_each_selected_track<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut Self, usize),
    {
        let tracks = self.selected_tracks();
        for track in tracks {
            f(self, track);
        }
    }



    pub(super) fn active_tool_row(&self) -> ToolRow {
        if self.ui.tools_cursor <= TP_LAST {
            ToolRow::Track(self.ui.tools_cursor)
        } else {
            ToolRow::Accum(self.ui.tools_cursor - (TP_LAST + 1))
        }
    }


    pub(super) fn push_send_gain(&self, track: usize) {
        let send_lid = self.state.runtime.send_lids[track].load(Ordering::Acquire);
        if send_lid != 0 {
            let tp = &self.state.pattern.track_params[track];
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: 0,
                        logical_id: send_lid,
                        fvalue: tp.get_send(),
                    },
                );
            }
        }
    }

    pub(super) fn push_track_volume(&self, track: usize) {
        let Some(node) = self.graph.track_node_ids.get(track) else {
            return;
        };
        let tp = &self.state.pattern.track_params[track];
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: node.delay_id as u64,
                    fvalue: crate::mixer_volume::fader_to_gain(tp.get_volume()),
                },
            );
        }
    }

    pub(super) fn push_track_pan(&self, track: usize) {
        let Some(node) = self.graph.track_node_ids.get(track) else {
            return;
        };
        let tp = &self.state.pattern.track_params[track];
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: node.pan_id as u64,
                    fvalue: tp.get_pan(),
                },
            );
        }
    }

    pub(super) fn push_track_mute(&self, track: usize) {
        let Some(node) = self.graph.track_node_ids.get(track) else {
            return;
        };
        let tp = &self.state.pattern.track_params[track];
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                    logical_id: node.delay_id as u64,
                    fvalue: if tp.is_muted() { 1.0 } else { 0.0 },
                },
            );
        }
    }

    pub(super) fn push_track_solo_mutes(&self) {
        let has_solo = (0..self.state.active_track_count())
            .any(|track| self.state.pattern.track_params[track].is_solo());
        for track in 0..self.state.active_track_count() {
            let Some(node) = self.graph.track_node_ids.get(track) else {
                continue;
            };
            let muted_by_solo = has_solo && !self.state.pattern.track_params[track].is_solo();
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                        logical_id: node.delay_id as u64,
                        fvalue: if muted_by_solo { 1.0 } else { 0.0 },
                    },
                );
            }
        }
    }

    pub(super) fn push_bus_volume(&self, bus: crate::sequencer::BusId) {
        let Some(channel) = self.buses.iter().find(|channel| channel.id == bus) else {
            return;
        };
        let Some(nodes) = self.graph.bus_node_ids.iter().find(|nodes| nodes.id == bus) else {
            return;
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: nodes.volume_id as u64,
                    fvalue: crate::mixer_volume::fader_to_gain(channel.volume),
                },
            );
        }
    }

    pub(super) fn push_bus_mute(&self, bus: crate::sequencer::BusId) {
        let Some(channel) = self.buses.iter().find(|channel| channel.id == bus) else {
            return;
        };
        let Some(nodes) = self.graph.bus_node_ids.iter().find(|nodes| nodes.id == bus) else {
            return;
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                    logical_id: nodes.volume_id as u64,
                    fvalue: if channel.mute { 1.0 } else { 0.0 },
                },
            );
        }
    }

    pub(super) fn push_bus_solo_mutes(&self) {
        let has_solo = self.buses.iter().any(|bus| bus.solo);
        for channel in &self.buses {
            let Some(nodes) = self
                .graph
                .bus_node_ids
                .iter()
                .find(|nodes| nodes.id == channel.id)
            else {
                continue;
            };
            let muted_by_solo = has_solo && !channel.solo;
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                        logical_id: nodes.volume_id as u64,
                        fvalue: if muted_by_solo { 1.0 } else { 0.0 },
                    },
                );
            }
        }
    }

    pub(super) fn push_master_volume(&self) {
        let volume = f32::from_bits(self.state.transport.master_volume.load(Ordering::Relaxed));
        if let Some(nodes) = self
            .graph
            .bus_node_ids
            .iter()
            .find(|nodes| nodes.id == BusId::MIX)
        {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: volume,
                    },
                );
            }
        }
    }


    fn dropdown_max_items(&self) -> usize {
        if self.ui.track_param_dropdown {
            match self.active_tool_row() {
                ToolRow::Accum(AC_FN) => return self.accumulator_names().len(),
                ToolRow::Accum(AC_MODE) => return AccumMode::COUNT,
                ToolRow::Track(TP_FTS) => {
                    return crate::scale::SCALES.len();
                }
                ToolRow::Track(TP_SWING_RESOLUTION) => return SwingResolution::COUNT,
                ToolRow::Track(_) => return Timebase::COUNT,
                _ => return 0,
            }
        }
        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            if let Some(desc) = self.current_instrument_descriptor() {
                if self.ui.instrument_param_cursor > 0 {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        if let crate::effects::ParamKind::Enum { ref labels } =
                            desc.params[param_idx].kind
                        {
                            return labels.len();
                        }
                    }
                }
            }
            return 0;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            if let Some(desc) = self.current_mod_descriptor() {
                if self.ui.mod_param_cursor < desc.params.len() {
                    if let crate::effects::ParamKind::Enum { ref labels } =
                        desc.params[self.ui.mod_param_cursor].kind
                    {
                        return labels.len();
                    }
                }
            }
            return 0;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            if let Some(desc) = self.current_source_descriptor() {
                if self.ui.source_param_cursor < desc.params.len() {
                    if let crate::effects::ParamKind::Enum { ref labels } =
                        desc.params[self.ui.source_param_cursor].kind
                    {
                        return labels.len();
                    }
                }
            }
            return 0;
        }
        if let Some(desc) = self.current_slot_descriptor() {
            if self.ui.effect_param_cursor < desc.params.len() {
                if let crate::effects::ParamKind::Enum { ref labels } =
                    desc.params[self.ui.effect_param_cursor].kind
                {
                    return labels.len();
                }
            }
        }
        0
    }

    pub(super) fn dropdown_labels(&self) -> &[String] {
        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            if let Some(desc) = self.current_instrument_descriptor() {
                if self.ui.instrument_param_cursor > 0 {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        if let crate::effects::ParamKind::Enum { ref labels } =
                            desc.params[param_idx].kind
                        {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if self.ui.effect_tab == EffectTab::Mod {
            if let Some(desc) = self.current_instrument_descriptor() {
                let mod_indices = self.mod_param_indices(self.ui.cursor_track);
                if let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) {
                    if let Some(param) = desc.params.get(param_idx) {
                        if let crate::effects::ParamKind::Enum { ref labels } = param.kind {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if self.ui.effect_tab == EffectTab::Sources {
            if let Some(desc) = self.current_instrument_descriptor() {
                let source_indices = self.source_param_actual_indices(self.ui.cursor_track);
                if let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) {
                    if let Some(param) = desc.params.get(param_idx) {
                        if let crate::effects::ParamKind::Enum { ref labels } = param.kind {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if let Some(desc) = self.current_slot_descriptor() {
            if self.ui.effect_param_cursor < desc.params.len() {
                if let crate::effects::ParamKind::Enum { ref labels } =
                    desc.params[self.ui.effect_param_cursor].kind
                {
                    return labels;
                }
            }
        }
        &[]
    }

    fn apply_dropdown_selection(&mut self) {
        if self.ui.track_param_dropdown {
            match self.active_tool_row() {
                ToolRow::Accum(row) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        match row {
                            AC_FN => {
                                let default_limit = ACCUMULATOR_REGISTRY
                                    .get(self.ui.dropdown_cursor)
                                    .map(|def| def.default_limit);
                                apply_command(
                                    self,
                                    AppCommand::SetTrackAccumIdx {
                                        track,
                                        idx: self.ui.dropdown_cursor,
                                        default_limit,
                                        script_name: None,
                                    },
                                );
                            }
                            AC_MODE => {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackAccumMode {
                                        track,
                                        mode: self.ui.dropdown_cursor as u32,
                                    },
                                );
                            }
                            _ => {}
                        }
                    }
                }
                ToolRow::Track(TP_FTS) => {
                    let scale_idx = self.ui.dropdown_cursor;
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::SetTrackFtsScale { track, scale_idx });
                    }
                }
                ToolRow::Track(TP_SWING_RESOLUTION) => {
                    let resolution = SwingResolution::from_index(self.ui.dropdown_cursor as u32);
                    if self.has_selection() {
                        apply_command(
                            self,
                            AppCommand::SetTrackSwingResolutionPlockMulti {
                                track: self.ui.cursor_track,
                                steps: self.selected_steps(),
                                resolution,
                            },
                        );
                    } else {
                        let tracks = self.selected_tracks();
                        for track in tracks {
                            apply_command(
                                self,
                                AppCommand::SetTrackSwingResolution { track, resolution },
                            );
                        }
                    }
                }
                ToolRow::Track(_) => {
                    let tb = Timebase::from_index(self.ui.dropdown_cursor as u32);
                    if self.has_selection() {
                        let steps = self.selected_steps();
                        let track = self.ui.cursor_track;
                        apply_command(
                            self,
                            AppCommand::SetTimebasePlockMulti {
                                track,
                                steps,
                                timebase: tb,
                            },
                        );
                    } else {
                        let tracks = self.selected_tracks();
                        for track in tracks {
                            apply_command(
                                self,
                                AppCommand::SetTrackTimebase {
                                    track,
                                    timebase: tb,
                                },
                            );
                        }
                    }
                }
            }
            self.state.publish_scheduler_snapshot();
            return;
        }

        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            let val = self.ui.dropdown_cursor as f32;
            if self.ui.instrument_param_cursor == 0 {
                return;
            }
            let synth_indices = self.synth_param_indices(self.ui.cursor_track);
            let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) else {
                return;
            };
            if self.has_selection() {
                apply_command(
                    self,
                    AppCommand::SetInstrumentPlockMulti {
                        track: self.ui.cursor_track,
                        steps: self.selected_steps(),
                        param_idx,
                        value: val,
                    },
                );
            } else {
                apply_command(
                    self,
                    AppCommand::SetInstrumentParam {
                        track: self.ui.cursor_track,
                        param_idx,
                        value: val,
                    },
                );
            }
            return;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            let val = self.ui.dropdown_cursor as f32;
            let mod_indices = self.mod_param_indices(self.ui.cursor_track);
            let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) else {
                return;
            };
            self.set_instrument_param_or_plock(self.ui.cursor_track, param_idx, val);
            return;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            let val = self.ui.dropdown_cursor as f32;
            let source_indices = self.source_param_actual_indices(self.ui.cursor_track);
            let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
                return;
            };
            self.set_instrument_param_or_plock(self.ui.cursor_track, param_idx, val);
            return;
        }

        let val = self.ui.dropdown_cursor as f32;
        let param_idx = self.ui.effect_param_cursor;

        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let Some(desc) = self.current_slot_descriptor() else {
            return;
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        if self.has_selection() {
            if matches!(
                param_desc.host_control,
                Some(crate::effects::HostControl::FxSidechain { .. })
            ) {
                return;
            }
            apply_command(
                self,
                AppCommand::SetEffectPlockMulti {
                    track: self.ui.cursor_track,
                    steps: self.selected_steps(),
                    slot_idx,
                    param_idx,
                    value: val,
                },
            );
        } else {
            apply_command(
                self,
                AppCommand::SetEffectParam {
                    track: self.ui.cursor_track,
                    slot_idx,
                    param_idx,
                    value: val,
                },
            );
        }
    }
}

// ── Drawing ──
