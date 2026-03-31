use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::sync::atomic::Ordering;

use crate::sequencer::{SwingResolution, Timebase, MAX_STEPS};

use super::command::{apply_command, AppCommand};
use super::effects_draw::draw_effects_column;
use crate::accumulator::{AccumMode, ACCUMULATOR_REGISTRY};

use super::{
    App, EffectTab, InputMode, Region, AC_FN, AC_LAST, AC_LIMIT, AC_MODE, TP_ATTACK, TP_FTS,
    TP_GATE, TP_LAST, TP_MASTER, TP_PAN, TP_POLY, TP_RELEASE, TP_SEND, TP_STEPS, TP_SWING,
    TP_SWING_RESOLUTION, TP_TIMEBASE, TP_VOLUME,
};

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

    pub(super) fn handle_params_input(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.tracks.is_empty() {
            return;
        }
        self.handle_effects_column(code, _modifiers);
    }

    pub(super) fn handle_track_params_column(&mut self, code: KeyCode) {
        let tp = &self.state.pattern.track_params[self.ui.cursor_track];

        match code {
            KeyCode::Up => {
                if self.ui.tools_cursor > 0 {
                    self.ui.tools_cursor -= 1;
                    self.sync_tools_cursor();
                }
            }
            KeyCode::Down => {
                if self.ui.tools_cursor + 1 < TOOLS_ROW_COUNT {
                    self.ui.tools_cursor += 1;
                    self.sync_tools_cursor();
                }
            }
            KeyCode::Right => {}
            KeyCode::Left => {} // Already at leftmost column
            KeyCode::Enter => match self.active_tool_row() {
                ToolRow::Track(TP_GATE) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::ToggleTrackGate { track });
                    }
                }
                ToolRow::Track(TP_POLY) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::ToggleTrackPolyphonic { track });
                    }
                }
                ToolRow::Track(TP_TIMEBASE) => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    let current_tb = if self.has_selection() {
                        let step = self.selected_steps()[0];
                        self.state.pattern.timebase_plocks[self.ui.cursor_track]
                            .get(step)
                            .unwrap_or(tp.get_timebase())
                    } else {
                        tp.get_timebase()
                    };
                    self.ui.dropdown_cursor = current_tb as u8 as usize;
                    self.ui.input_mode = InputMode::Dropdown;
                }
                ToolRow::Track(TP_SWING_RESOLUTION) => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = tp.get_swing_resolution() as usize;
                    self.ui.input_mode = InputMode::Dropdown;
                }
                ToolRow::Track(TP_FTS) => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = tp.get_fts_scale();
                    self.ui.input_mode = InputMode::Dropdown;
                }
                ToolRow::Accum(AC_FN) => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor =
                        self.selected_accumulator_dropdown_index(self.ui.cursor_track);
                    self.ui.input_mode = InputMode::Dropdown;
                }
                ToolRow::Accum(AC_MODE) => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = tp.get_accum_mode() as usize;
                    self.ui.input_mode = InputMode::Dropdown;
                }
                _ => {}
            },
            KeyCode::Char('+') | KeyCode::Char('=') => match self.active_tool_row() {
                ToolRow::Track(TP_ATTACK) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackAttack { track, delta: 5.0 });
                }
                ToolRow::Track(TP_RELEASE) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackRelease { track, delta: 10.0 });
                }
                ToolRow::Track(TP_SWING) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackSwing { track, delta: 1.0 });
                }
                ToolRow::Track(TP_SWING_RESOLUTION) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::NextTrackSwingResolution { track });
                }
                ToolRow::Track(TP_STEPS) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackNumSteps { track, delta: 1 });
                    self.clamp_cursor_to_steps();
                }
                ToolRow::Track(TP_VOLUME) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::AdjustTrackVolume { track, delta: 0.05 });
                    }
                }
                ToolRow::Track(TP_PAN) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::AdjustTrackPan { track, delta: 0.05 });
                    }
                }
                ToolRow::Track(TP_TIMEBASE) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::NextTrackTimebase { track });
                }
                ToolRow::Track(TP_SEND) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(self, AppCommand::AdjustTrackSend { track, delta: 0.05 });
                    }
                }
                ToolRow::Track(TP_MASTER) => {
                    apply_command(self, AppCommand::AdjustMasterVolume { delta: 0.05 });
                }
                ToolRow::Track(TP_FTS) => {
                    let track = self.ui.cursor_track;
                    let new_scale = (tp.get_fts_scale() + 1).min(crate::scale::SCALES.len() - 1);
                    apply_command(
                        self,
                        AppCommand::SetTrackFtsScale {
                            track,
                            scale_idx: new_scale,
                        },
                    );
                }
                ToolRow::Accum(AC_LIMIT) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::AdjustTrackAccumLimit { track, delta: 1.0 },
                        );
                    }
                }
                _ => {}
            },
            KeyCode::Char('-') => match self.active_tool_row() {
                ToolRow::Track(TP_ATTACK) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackAttack { track, delta: -5.0 });
                }
                ToolRow::Track(TP_RELEASE) => {
                    let track = self.ui.cursor_track;
                    apply_command(
                        self,
                        AppCommand::AdjustTrackRelease {
                            track,
                            delta: -10.0,
                        },
                    );
                }
                ToolRow::Track(TP_SWING) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackSwing { track, delta: -1.0 });
                }
                ToolRow::Track(TP_SWING_RESOLUTION) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::PrevTrackSwingResolution { track });
                }
                ToolRow::Track(TP_STEPS) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::AdjustTrackNumSteps { track, delta: -1 });
                    self.clamp_cursor_to_steps();
                }
                ToolRow::Track(TP_VOLUME) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::AdjustTrackVolume {
                                track,
                                delta: -0.05,
                            },
                        );
                    }
                }
                ToolRow::Track(TP_PAN) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::AdjustTrackPan {
                                track,
                                delta: -0.05,
                            },
                        );
                    }
                }
                ToolRow::Track(TP_TIMEBASE) => {
                    let track = self.ui.cursor_track;
                    apply_command(self, AppCommand::PrevTrackTimebase { track });
                }
                ToolRow::Track(TP_SEND) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::AdjustTrackSend {
                                track,
                                delta: -0.05,
                            },
                        );
                    }
                }
                ToolRow::Track(TP_MASTER) => {
                    apply_command(self, AppCommand::AdjustMasterVolume { delta: -0.05 });
                }
                ToolRow::Track(TP_FTS) => {
                    let track = self.ui.cursor_track;
                    let new_scale = tp.get_fts_scale().saturating_sub(1);
                    apply_command(
                        self,
                        AppCommand::SetTrackFtsScale {
                            track,
                            scale_idx: new_scale,
                        },
                    );
                }
                ToolRow::Accum(AC_LIMIT) => {
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::AdjustTrackAccumLimit { track, delta: -1.0 },
                        );
                    }
                }
                _ => {}
            },
            KeyCode::Backspace | KeyCode::Delete => {
                if self.active_tool_row() == ToolRow::Track(TP_TIMEBASE) && self.has_selection() {
                    let track = self.ui.cursor_track;
                    let steps = self.selected_steps();
                    apply_command(self, AppCommand::ClearTimebasePlockMulti { track, steps });
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => match self.active_tool_row() {
                ToolRow::Track(idx)
                    if idx > TP_GATE
                        && idx != TP_POLY
                        && idx != TP_TIMEBASE
                        && idx != TP_SWING_RESOLUTION =>
                {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                }
                ToolRow::Accum(AC_LIMIT) => {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                }
                _ => {}
            },
            _ => {}
        }

        match code {
            KeyCode::Char('+') | KeyCode::Char('=')
                if matches!(
                    self.active_tool_row(),
                    ToolRow::Track(TP_ATTACK)
                        | ToolRow::Track(TP_RELEASE)
                        | ToolRow::Track(TP_SWING)
                        | ToolRow::Track(TP_SWING_RESOLUTION)
                        | ToolRow::Track(TP_STEPS)
                        | ToolRow::Track(TP_TIMEBASE)
                        | ToolRow::Track(TP_FTS)
                        | ToolRow::Accum(AC_LIMIT)
                ) =>
            {
                self.state.publish_scheduler_snapshot();
            }
            KeyCode::Char('-')
                if matches!(
                    self.active_tool_row(),
                    ToolRow::Track(TP_ATTACK)
                        | ToolRow::Track(TP_RELEASE)
                        | ToolRow::Track(TP_SWING)
                        | ToolRow::Track(TP_SWING_RESOLUTION)
                        | ToolRow::Track(TP_STEPS)
                        | ToolRow::Track(TP_TIMEBASE)
                        | ToolRow::Track(TP_FTS)
                        | ToolRow::Accum(AC_LIMIT)
                ) =>
            {
                self.state.publish_scheduler_snapshot();
            }
            _ => {}
        }
    }

    pub(super) fn active_tool_row(&self) -> ToolRow {
        if self.ui.tools_cursor <= TP_LAST {
            ToolRow::Track(self.ui.tools_cursor)
        } else {
            ToolRow::Accum(self.ui.tools_cursor - (TP_LAST + 1))
        }
    }

    pub(super) fn sync_tools_cursor(&mut self) {
        self.ui.tools_cursor = self.ui.tools_cursor.min(TOOLS_ROW_COUNT.saturating_sub(1));
        let visible = self.ui.layout.track_params_inner.height as usize;
        if visible > 0 {
            if self.ui.tools_cursor < self.ui.tools_scroll_offset {
                self.ui.tools_scroll_offset = self.ui.tools_cursor;
            } else if self.ui.tools_cursor >= self.ui.tools_scroll_offset + visible {
                self.ui.tools_scroll_offset = self.ui.tools_cursor + 1 - visible;
            }
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
                    idx: crate::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: node.pan_id as u64,
                    fvalue: tp.get_volume(),
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
                    idx: crate::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: node.pan_id as u64,
                    fvalue: tp.get_pan(),
                },
            );
        }
    }

    pub(super) fn push_master_volume(&self) {
        let volume = f32::from_bits(self.state.transport.master_volume.load(Ordering::Relaxed));
        for bus_id in [self.graph.bus_l_id, self.graph.bus_r_id] {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: 0,
                        logical_id: bus_id as u64,
                        fvalue: volume,
                    },
                );
            }
        }
    }

    pub(super) fn handle_dropdown(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up => {
                if self.ui.dropdown_cursor > 0 {
                    self.ui.dropdown_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.dropdown_max_items();
                if self.ui.dropdown_cursor < max.saturating_sub(1) {
                    self.ui.dropdown_cursor += 1;
                }
            }
            KeyCode::Enter => {
                self.apply_dropdown_selection();
                self.ui.dropdown_open = false;
                self.ui.track_param_dropdown = false;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.dropdown_open = false;
                self.ui.track_param_dropdown = false;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
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
                    let tracks = self.selected_tracks();
                    for track in tracks {
                        apply_command(
                            self,
                            AppCommand::SetTrackSwingResolution { track, resolution },
                        );
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
            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
            if self.has_selection() {
                for step in self.selected_steps() {
                    slot.plocks.set(step, param_idx, val);
                }
            } else {
                slot.defaults.set(param_idx, val);
                self.send_instrument_param(self.ui.cursor_track, param_idx, val);
                self.mark_track_sound_dirty(self.ui.cursor_track);
            }
            self.state.publish_scheduler_snapshot();
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

        let slot = match self.current_slot() {
            Some(s) => s,
            None => return,
        };
        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let Some(desc) = self.current_slot_descriptor() else {
            return;
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        if matches!(
            param_desc.host_control,
            Some(crate::effects::HostControl::FxSidechain { .. })
        ) {
            self.apply_effect_sidechain_selection(
                self.ui.cursor_track,
                slot_idx,
                param_idx,
                self.ui.dropdown_cursor,
            );
            slot.defaults.set(param_idx, val);
            self.state.publish_scheduler_snapshot();
            return;
        }

        if self.has_selection() {
            for step in self.selected_steps() {
                slot.plocks.set(step, param_idx, val);
            }
        } else {
            slot.defaults.set(param_idx, val);
        }
        self.state.publish_scheduler_snapshot();
    }
}

// ── Drawing ──

pub(super) fn draw_params_region(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.ui.focused_region == Region::Params;
    draw_effects_column(frame, app, area, is_focused);
}

pub(super) fn draw_track_params_column(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    region_focused: bool,
) {
    let col_focused = region_focused && app.ui.params_column == 0;
    app.ui.layout.track_params_inner = area;

    if app.tracks.is_empty() || area.height < 1 {
        return;
    }

    app.sync_tools_cursor();
    draw_tools_list(frame, app, area, col_focused);

    if app.ui.dropdown_open && app.ui.track_param_dropdown && col_focused {
        draw_track_param_dropdown(frame, app, area);
    }
}
fn draw_tools_list(frame: &mut Frame, app: &App, area: Rect, col_focused: bool) {
    let tp = &app.state.pattern.track_params[app.ui.cursor_track];
    let attack = tp.get_attack_ms();
    let release = tp.get_release_ms();
    let swing = tp.get_swing();
    let swing_resolution = tp.get_swing_resolution();
    let steps = tp.get_num_steps();
    let volume = tp.get_volume();
    let pan = tp.get_pan();
    let default_tb = tp.get_timebase();
    let timebase_display = if app.has_selection() {
        let step = app.selected_steps()[0];
        match app.state.pattern.timebase_plocks[app.ui.cursor_track].get(step) {
            Some(tb) => format!("{} [P]", tb.label()),
            None => default_tb.label().to_string(),
        }
    } else {
        default_tb.label().to_string()
    };
    let send = tp.get_send();
    let master = f32::from_bits(app.state.transport.master_volume.load(Ordering::Relaxed));

    let mut params: Vec<(&str, String, Option<f32>)> = vec![
        (
            "gate",
            if tp.is_gate_on() {
                "ON".into()
            } else {
                "OFF".into()
            },
            None,
        ),
        ("attack", format!("{:.0} ms", attack), Some(attack / 500.0)),
        (
            "release",
            format!("{:.0} ms", release),
            Some(release / 2000.0),
        ),
        (
            "swing",
            format!("{:.0}%", swing),
            Some((swing - 50.0) / 25.0),
        ),
        ("swing res", swing_resolution.label().to_string(), None),
        (
            "steps",
            format!("{}", steps),
            Some(steps as f32 / MAX_STEPS as f32),
        ),
        ("vol", format!("{:.2}", volume), Some(volume)),
        ("pan", format!("{:+.2}", pan), Some((pan + 1.0) * 0.5)),
        ("timebase", timebase_display, None),
        ("send", format!("{:.2}", send), Some(send)),
        ("master", format!("{:.2}", master), Some(master / 2.0)),
        (
            "poly",
            if tp.is_polyphonic() {
                "ON".into()
            } else {
                "OFF".into()
            },
            None,
        ),
        (
            "fts",
            crate::scale::SCALES
                .get(tp.get_fts_scale())
                .map(|s| s.name)
                .unwrap_or("Off")
                .to_string(),
            None,
        ),
    ];
    let accum_idx = app.selected_accumulator_dropdown_index(app.ui.cursor_track);
    let accum_name = app
        .accumulator_names()
        .get(accum_idx)
        .cloned()
        .unwrap_or_else(|| "Off".to_string());
    let limit = tp.get_accum_limit();
    let mode = AccumMode::from_u32(tp.get_accum_mode());

    params.extend_from_slice(&[
        ("acc fn", accum_name, None),
        ("acc lim", format!("{:.0}", limit), Some(limit / 127.0)),
        ("acc mode", mode.label().to_string(), None),
    ]);

    let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;
    draw_param_rows(
        frame,
        app,
        area,
        col_focused,
        &params,
        app.ui.tools_cursor,
        app.ui.tools_scroll_offset,
        is_entering_value,
    );
}

/// Shared row-rendering logic for both tabs.
fn draw_param_rows(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    col_focused: bool,
    params: &[(&str, String, Option<f32>)],
    cursor: usize,
    scroll: usize,
    is_entering_value: bool,
) {
    let visible = area.height as usize;

    for (i, (name, value, slider)) in params.iter().enumerate().skip(scroll) {
        let row = i - scroll;
        if row >= visible {
            break;
        }
        let y = area.y + row as u16;
        let is_cursor_row = col_focused && cursor == i;
        let cur_str = if is_cursor_row { "> " } else { "  " };
        let cursor_style = if is_cursor_row {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let label_width = 10;
        let value_width = 12;

        if is_cursor_row && is_entering_value {
            let spans = vec![
                Span::styled(cur_str, cursor_style),
                Span::styled(
                    format!("{:<width$}", name, width = label_width),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{}\u{2588}", app.ui.value_buffer),
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::Rgb(60, 60, 20))
                        .bold(),
                ),
                Span::styled(
                    "  Enter: set  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, y, area.width, 1),
            );
            continue;
        }

        let mut spans = vec![
            Span::styled(cur_str, cursor_style),
            Span::styled(
                format!("{:<width$}", name, width = label_width),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{:<width$}", value, width = value_width),
                cursor_style,
            ),
        ];

        if let Some(norm) = slider {
            let slider_width = (area.width as usize).saturating_sub(label_width + value_width + 4);
            if slider_width > 2 {
                let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                let bar = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                spans.push(Span::styled(
                    format!("[{}]", bar),
                    Style::default().fg(Color::Cyan),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

/// Draw a generic dropdown overlay given a list of item labels.
fn draw_dropdown_items(
    frame: &mut Frame,
    items: &[&str],
    cursor: usize,
    area: Rect,
    anchor_row: u16,
) {
    if items.is_empty() {
        return;
    }

    let dropdown_x = area.x + 14; // after label
    let dropdown_width = 16u16;

    // How many rows can we show?
    let max_rows = (area.y + area.height).saturating_sub(area.y) as usize;
    let visible_count = items.len().min(max_rows);
    if visible_count == 0 {
        return;
    }

    // Scroll so the cursor is always visible
    let scroll = if cursor >= visible_count {
        cursor + 1 - visible_count
    } else {
        0
    };

    // Position: try to start at the anchor row, but shift up if it would overflow
    let ideal_y = area.y + anchor_row;
    let dropdown_y = ideal_y.min((area.y + area.height).saturating_sub(visible_count as u16));

    for vi in 0..visible_count {
        let i = scroll + vi;
        if i >= items.len() {
            break;
        }
        let y = dropdown_y + vi as u16;
        let is_cursor = i == cursor;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
        };
        let text = format!(
            " {:<width$}",
            items[i],
            width = (dropdown_width - 2) as usize
        );
        let cell = Rect::new(dropdown_x, y, dropdown_width, 1);
        frame.render_widget(Paragraph::new(text).style(style), cell);
    }
}

pub(super) fn draw_dropdown(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<&str> = app.dropdown_labels().iter().map(|s| s.as_str()).collect();
    let anchor = match app.ui.effect_tab {
        super::EffectTab::Synth => app.partition_cursor_anchor_row(
            area,
            app.synth_row_count(),
            app.ui.instrument_param_cursor,
            app.ui.synth_scroll_offset,
        ),
        super::EffectTab::Mod => app.partition_cursor_anchor_row(
            area,
            app.mod_row_count(),
            app.ui.mod_param_cursor,
            app.ui.mod_scroll_offset,
        ),
        super::EffectTab::Sources => app.partition_cursor_anchor_row(
            area,
            app.source_row_count(),
            app.source_display_row_for_param_row(app.ui.source_param_cursor),
            app.ui.source_scroll_offset,
        ),
        super::EffectTab::Slot(_) => app.partition_cursor_anchor_row(
            area,
            app.effect_row_count(),
            app.ui.effect_param_cursor,
            app.ui.effect_scroll_offset,
        ),
        super::EffectTab::Reverb => app.ui.reverb_param_cursor as u16,
    };
    draw_dropdown_items(frame, &items, app.ui.dropdown_cursor, area, anchor);
}

pub(super) fn draw_track_param_dropdown(frame: &mut Frame, app: &App, area: Rect) {
    let anchor = app
        .ui
        .tools_cursor
        .saturating_sub(app.ui.tools_scroll_offset) as u16;
    match app.active_tool_row() {
        ToolRow::Accum(AC_FN) => {
            let owned_names = app.accumulator_names();
            let names: Vec<&str> = owned_names.iter().map(|name| name.as_str()).collect();
            draw_dropdown_items(frame, &names, app.ui.dropdown_cursor, area, anchor);
            return;
        }
        ToolRow::Accum(AC_MODE) => {
            draw_dropdown_items(
                frame,
                &AccumMode::LABELS,
                app.ui.dropdown_cursor,
                area,
                anchor,
            );
            return;
        }
        ToolRow::Track(TP_FTS) => {
            let names: Vec<&str> = crate::scale::SCALES.iter().map(|s| s.name).collect();
            draw_dropdown_items(frame, &names, app.ui.dropdown_cursor, area, anchor);
            return;
        }
        ToolRow::Track(TP_SWING_RESOLUTION) => {
            draw_dropdown_items(
                frame,
                &SWING_RESOLUTION_LABELS,
                app.ui.dropdown_cursor,
                area,
                anchor,
            );
            return;
        }
        ToolRow::Accum(_) => return,
        ToolRow::Track(_) => {}
    }
    draw_dropdown_items(
        frame,
        &TIMEBASE_LABELS,
        app.ui.dropdown_cursor,
        area,
        anchor,
    );
}
