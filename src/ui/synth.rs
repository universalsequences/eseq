use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use std::sync::atomic::Ordering;

use super::command::{apply_command, AppCommand};

use crate::effects::EffectDescriptor;

use super::{App, InputMode};

pub(super) const SYNTH_MIN_COLUMN_WIDTH: u16 = 42;
pub(super) const SYNTH_COLUMN_GAP: u16 = 2;

impl App {
    pub(super) fn source_param_actual_indices(&self, track: usize) -> Vec<usize> {
        let Some(desc) = self.graph.instrument_descriptors.get(track) else {
            return Vec::new();
        };
        let slot = &self.state.pattern.instrument_slots[track];
        let source_indices = self.source_param_indices(track);

        let find_idx_by_node = |node_param_idx: u32| {
            source_indices
                .iter()
                .copied()
                .find(|&idx| desc.params.get(idx).map(|p| p.node_param_idx) == Some(node_param_idx))
        };

        let mut out = Vec::new();

        let lfo_sync = |sync_idx: u32| -> bool {
            find_idx_by_node(sync_idx)
                .map(|idx| slot.defaults.get(idx) > 0.5)
                .unwrap_or(false)
        };
        let lfo_shape_is_pulse = |shape_idx: u32| -> bool {
            find_idx_by_node(shape_idx)
                .map(|idx| slot.defaults.get(idx).round() as i32 == 2)
                .unwrap_or(false)
        };

        let push_lfo = |out: &mut Vec<usize>,
                        rate_idx: usize,
                        sync_idx: usize,
                        div_idx: usize,
                        shape_idx: usize,
                        pw_idx: usize,
                        retrig_idx: usize| {
            let rate_node = crate::voice_modulator::MOD_PARAM_BASE + rate_idx as u32;
            let sync_node = crate::voice_modulator::MOD_PARAM_BASE + sync_idx as u32;
            let div_node = crate::voice_modulator::MOD_PARAM_BASE + div_idx as u32;
            let shape_node = crate::voice_modulator::MOD_PARAM_BASE + shape_idx as u32;
            let pw_node = crate::voice_modulator::MOD_PARAM_BASE + pw_idx as u32;
            let retrig_node = crate::voice_modulator::MOD_PARAM_BASE + retrig_idx as u32;

            if let Some(idx) = if lfo_sync(sync_node) {
                find_idx_by_node(div_node)
            } else {
                find_idx_by_node(rate_node)
            } {
                out.push(idx);
            }
            if let Some(idx) = find_idx_by_node(sync_node) {
                out.push(idx);
            }
            if let Some(idx) = find_idx_by_node(shape_node) {
                out.push(idx);
            }
            if let Some(idx) = find_idx_by_node(retrig_node) {
                out.push(idx);
            }
            if lfo_shape_is_pulse(shape_node) {
                if let Some(idx) = find_idx_by_node(pw_node) {
                    out.push(idx);
                }
            }
        };

        push_lfo(
            &mut out,
            crate::voice_modulator::PARAM_LFO1_RATE_HZ,
            crate::voice_modulator::PARAM_LFO1_SYNC,
            crate::voice_modulator::PARAM_LFO1_DIV,
            crate::voice_modulator::PARAM_LFO1_SHAPE,
            crate::voice_modulator::PARAM_LFO1_PW,
            crate::voice_modulator::PARAM_LFO1_RETRIGGER,
        );

        for idx_const in [
            crate::voice_modulator::PARAM_ENV_ATTACK_MS,
            crate::voice_modulator::PARAM_ENV_DECAY_MS,
            crate::voice_modulator::PARAM_ENV_SUSTAIN,
            crate::voice_modulator::PARAM_ENV_RELEASE_MS,
        ] {
            if let Some(idx) =
                find_idx_by_node(crate::voice_modulator::MOD_PARAM_BASE + idx_const as u32)
            {
                out.push(idx);
            }
        }

        if let Some(idx) = if lfo_sync(
            crate::voice_modulator::MOD_PARAM_BASE + crate::voice_modulator::PARAM_RAND_SYNC as u32,
        ) {
            find_idx_by_node(
                crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_RAND_DIV as u32,
            )
        } else {
            find_idx_by_node(
                crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_RAND_RATE_HZ as u32,
            )
        } {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(
            crate::voice_modulator::MOD_PARAM_BASE + crate::voice_modulator::PARAM_RAND_SYNC as u32,
        ) {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(
            crate::voice_modulator::MOD_PARAM_BASE + crate::voice_modulator::PARAM_RAND_SLEW as u32,
        ) {
            out.push(idx);
        }

        if let Some(idx) = if lfo_sync(
            crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_DRIFT_SYNC as u32,
        ) {
            find_idx_by_node(
                crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_DRIFT_DIV as u32,
            )
        } else {
            find_idx_by_node(
                crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_DRIFT_RATE as u32,
            )
        } {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(
            crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_DRIFT_SYNC as u32,
        ) {
            out.push(idx);
        }

        push_lfo(
            &mut out,
            crate::voice_modulator::PARAM_LFO2_RATE_HZ,
            crate::voice_modulator::PARAM_LFO2_SYNC,
            crate::voice_modulator::PARAM_LFO2_DIV,
            crate::voice_modulator::PARAM_LFO2_SHAPE,
            crate::voice_modulator::PARAM_LFO2_PW,
            crate::voice_modulator::PARAM_LFO2_RETRIGGER,
        );
        push_lfo(
            &mut out,
            crate::voice_modulator::PARAM_LFO3_RATE_HZ,
            crate::voice_modulator::PARAM_LFO3_SYNC,
            crate::voice_modulator::PARAM_LFO3_DIV,
            crate::voice_modulator::PARAM_LFO3_SHAPE,
            crate::voice_modulator::PARAM_LFO3_PW,
            crate::voice_modulator::PARAM_LFO3_RETRIGGER,
        );

        out
    }

    pub(super) fn source_display_rows(
        &self,
        track: usize,
    ) -> Vec<(Option<&'static str>, Option<usize>)> {
        let actual = self.source_param_actual_indices(track);
        let Some(desc) = self.graph.instrument_descriptors.get(track) else {
            return Vec::new();
        };

        let mut rows = Vec::new();
        let mut cursor = 0usize;
        let sections = [
            ("LFO 1", 0usize),
            ("ENV 1", 0usize),
            ("RAND", 0usize),
            ("DRIFT", 0usize),
            ("LFO 2", 0usize),
            ("LFO 3", 0usize),
        ];

        let section_name = |idx: usize, desc: &EffectDescriptor| -> &'static str {
            let node_idx = desc.params[idx].node_param_idx;
            if (crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_LFO1_RATE_HZ as u32
                ..=crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_LFO1_RETRIGGER as u32)
                .contains(&node_idx)
            {
                "LFO 1"
            } else if (crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_ENV_ATTACK_MS as u32
                ..=crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_ENV_RELEASE_MS as u32)
                .contains(&node_idx)
            {
                "ENV 1"
            } else if (crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_RAND_RATE_HZ as u32
                ..=crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_RAND_SLEW as u32)
                .contains(&node_idx)
            {
                "RAND"
            } else if (crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_DRIFT_RATE as u32
                ..=crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_DRIFT_DIV as u32)
                .contains(&node_idx)
            {
                "DRIFT"
            } else if (crate::voice_modulator::MOD_PARAM_BASE
                + crate::voice_modulator::PARAM_LFO2_RATE_HZ as u32
                ..=crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_LFO2_RETRIGGER as u32)
                .contains(&node_idx)
            {
                "LFO 2"
            } else {
                "LFO 3"
            }
        };

        for (section, _) in sections {
            let section_params: Vec<usize> = actual
                .iter()
                .enumerate()
                .filter_map(|(row_idx, &actual_idx)| {
                    (section_name(actual_idx, desc) == section).then_some(row_idx)
                })
                .collect();
            if section_params.is_empty() {
                continue;
            }
            rows.push((Some(section), None));
            for row_idx in section_params {
                rows.push((None, Some(row_idx)));
            }
            cursor += 1;
            let _ = cursor;
        }

        rows
    }

    fn is_modulation_param_name(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_mod_source_param(node_param_idx: u32) -> bool {
        node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE
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

    pub(super) fn instrument_column_count(&self, area: Rect, total_rows: usize) -> usize {
        if area.height == 0 {
            return 1;
        }
        let rows = total_rows.max(1);
        let needed_columns = rows.div_ceil(area.height as usize).max(1);
        let max_columns = ((area.width + SYNTH_COLUMN_GAP)
            / (SYNTH_MIN_COLUMN_WIDTH + SYNTH_COLUMN_GAP))
            .max(1) as usize;
        needed_columns.min(max_columns).max(1)
    }

    pub(super) fn synth_rows_per_column(&self, area: Rect) -> usize {
        area.height as usize
    }

    pub(super) fn partition_scroll_offset(
        &self,
        area: Rect,
        total_rows: usize,
        scroll_offset: usize,
    ) -> usize {
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            return 0;
        }
        let rows_per_column = self.instrument_partition_rows_per_column(area, total_rows);
        let max_scroll = rows_per_column.saturating_sub(visible_rows);
        scroll_offset.min(max_scroll)
    }

    pub(super) fn scroll_partition_offset(
        &self,
        area: Rect,
        total_rows: usize,
        scroll_offset: usize,
        delta: isize,
    ) -> usize {
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            return 0;
        }
        let rows_per_column = self.instrument_partition_rows_per_column(area, total_rows);
        let max_scroll = rows_per_column.saturating_sub(visible_rows);
        if delta < 0 {
            scroll_offset.saturating_sub((-delta) as usize)
        } else {
            (scroll_offset + delta as usize).min(max_scroll)
        }
    }

    pub(super) fn ensure_partition_cursor_visible(
        &self,
        area: Rect,
        total_rows: usize,
        cursor: usize,
        scroll_offset: usize,
    ) -> (usize, usize) {
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            return (cursor.min(total_rows.saturating_sub(1)), 0);
        }

        let cursor = cursor.min(total_rows.saturating_sub(1));
        let mut scroll_offset = self.partition_scroll_offset(area, total_rows, scroll_offset);
        let rows_per_column = self.instrument_partition_rows_per_column(area, total_rows);
        let row_in_column = cursor % rows_per_column;
        if row_in_column < scroll_offset {
            scroll_offset = row_in_column;
        } else if row_in_column >= scroll_offset + visible_rows {
            scroll_offset = row_in_column + 1 - visible_rows;
        }

        (
            cursor,
            self.partition_scroll_offset(area, total_rows, scroll_offset),
        )
    }

    pub(super) fn partition_row_at_position(
        &self,
        area: Rect,
        col: u16,
        row: u16,
        total_rows: usize,
        scroll_offset: usize,
    ) -> Option<usize> {
        if area.height == 0
            || col < area.x
            || col >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }

        let columns = self.instrument_column_count(area, total_rows);
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            return None;
        }
        let rows_per_column = self.instrument_partition_rows_per_column(area, total_rows);
        let column_width = self.instrument_column_width(area, total_rows);
        if column_width == 0 {
            return None;
        }

        let rel_x = col - area.x;
        let stride = column_width + SYNTH_COLUMN_GAP;
        let column = (rel_x / stride) as usize;
        if column >= columns {
            return None;
        }
        let local_x = rel_x.saturating_sub(column as u16 * stride);
        if local_x >= column_width {
            return None;
        }

        let rel_y = (row - area.y) as usize;
        if rel_y >= visible_rows {
            return None;
        }

        let absolute = self.partition_scroll_offset(area, total_rows, scroll_offset)
            + column * rows_per_column
            + rel_y;
        (absolute < total_rows).then_some(absolute)
    }

    pub(super) fn partition_cursor_anchor_row(
        &self,
        area: Rect,
        total_rows: usize,
        cursor: usize,
        scroll_offset: usize,
    ) -> u16 {
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            return 0;
        }
        let cursor = cursor.min(total_rows.saturating_sub(1));
        let scroll_offset = self.partition_scroll_offset(area, total_rows, scroll_offset);
        let rows_per_column = self.instrument_partition_rows_per_column(area, total_rows);
        let row_in_column = cursor % rows_per_column;
        row_in_column.saturating_sub(scroll_offset) as u16
    }

    pub(super) fn instrument_partition_rows_per_column(
        &self,
        area: Rect,
        total_rows: usize,
    ) -> usize {
        let columns = self.instrument_column_count(area, total_rows);
        total_rows.div_ceil(columns).max(1)
    }

    pub(super) fn instrument_column_width(&self, area: Rect, total_rows: usize) -> u16 {
        let columns = self.instrument_column_count(area, total_rows) as u16;
        if columns <= 1 {
            area.width
        } else {
            let total_gap = SYNTH_COLUMN_GAP.saturating_mul(columns.saturating_sub(1));
            area.width.saturating_sub(total_gap) / columns
        }
    }

    pub(super) fn clamp_synth_scroll(&mut self, area: Rect) {
        self.ui.synth_scroll_offset =
            self.partition_scroll_offset(area, self.synth_row_count(), self.ui.synth_scroll_offset);
    }

    pub(super) fn clamp_mod_scroll(&mut self, area: Rect) {
        self.ui.mod_scroll_offset =
            self.partition_scroll_offset(area, self.mod_row_count(), self.ui.mod_scroll_offset);
    }

    pub(super) fn clamp_source_scroll(&mut self, area: Rect) {
        self.ui.source_scroll_offset = self.partition_scroll_offset(
            area,
            self.source_row_count(),
            self.ui.source_scroll_offset,
        );
    }

    pub(super) fn ensure_synth_cursor_visible(&mut self) {
        let area = self.ui.layout.effects_inner;
        (self.ui.instrument_param_cursor, self.ui.synth_scroll_offset) = self
            .ensure_partition_cursor_visible(
                area,
                self.synth_row_count(),
                self.ui.instrument_param_cursor,
                self.ui.synth_scroll_offset,
            );
    }

    pub(super) fn ensure_mod_cursor_visible(&mut self) {
        let area = self.ui.layout.effects_inner;
        (self.ui.mod_param_cursor, self.ui.mod_scroll_offset) = self
            .ensure_partition_cursor_visible(
                area,
                self.mod_row_count(),
                self.ui.mod_param_cursor,
                self.ui.mod_scroll_offset,
            );
    }

    pub(super) fn ensure_source_cursor_visible(&mut self) {
        let area = self.ui.layout.effects_inner;
        let visible_rows = self.synth_rows_per_column(area);
        if visible_rows == 0 {
            self.ui.source_scroll_offset = 0;
            return;
        }

        let max_cursor = self.source_param_count().saturating_sub(1);
        self.ui.source_param_cursor = self.ui.source_param_cursor.min(max_cursor);
        self.clamp_source_scroll(area);

        let display_row = self.source_display_row_for_param_row(self.ui.source_param_cursor);
        let rows_per_column =
            self.instrument_partition_rows_per_column(area, self.source_row_count());
        let row_in_column = display_row % rows_per_column;
        if row_in_column < self.ui.source_scroll_offset {
            self.ui.source_scroll_offset = row_in_column;
        } else if row_in_column >= self.ui.source_scroll_offset + visible_rows {
            self.ui.source_scroll_offset = row_in_column + 1 - visible_rows;
        }

        self.clamp_source_scroll(area);
    }

    pub(super) fn synth_row_at_position(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        self.partition_row_at_position(
            area,
            col,
            row,
            self.synth_row_count(),
            self.ui.synth_scroll_offset,
        )
    }

    pub(super) fn mod_row_at_position(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        self.partition_row_at_position(
            area,
            col,
            row,
            self.mod_row_count(),
            self.ui.mod_scroll_offset,
        )
    }

    pub(super) fn source_row_at_position(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        let display_row = self.partition_row_at_position(
            area,
            col,
            row,
            self.source_row_count(),
            self.ui.source_scroll_offset,
        )?;
        self.source_param_row_for_display(display_row)
    }

    pub(super) fn handle_synth_tab_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Left => {
                self.ui.params_column = 0;
                self.sync_effect_tab_cursor();
            }
            KeyCode::Right => {}
            KeyCode::Up => {
                if shift {
                    if self.ui.instrument_param_cursor == 0 {
                        let next = (self.instrument_base_note_offset(self.ui.cursor_track) + 1.0)
                            .clamp(-48.0, 48.0);
                        self.set_instrument_base_note_offset(self.ui.cursor_track, next);
                    } else {
                        self.adjust_instrument_param(1.0);
                    }
                } else if self.ui.instrument_param_cursor > 0 {
                    self.ui.instrument_param_cursor -= 1;
                    self.ensure_synth_cursor_visible();
                }
            }
            KeyCode::Down => {
                if shift {
                    if self.ui.instrument_param_cursor == 0 {
                        let next = (self.instrument_base_note_offset(self.ui.cursor_track) - 1.0)
                            .clamp(-48.0, 48.0);
                        self.set_instrument_base_note_offset(self.ui.cursor_track, next);
                    } else {
                        self.adjust_instrument_param(-1.0);
                    }
                } else {
                    let max = self.synth_row_count().saturating_sub(1);
                    if self.ui.instrument_param_cursor < max {
                        self.ui.instrument_param_cursor += 1;
                        self.ensure_synth_cursor_visible();
                    }
                }
            }
            KeyCode::Enter => {
                if self.ui.instrument_param_cursor == 0 {
                    self.ui.value_buffer.clear();
                    self.ui.input_mode = InputMode::ValueEntry;
                } else if let Some(desc) = self.current_instrument_descriptor() {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        let param = &desc.params[param_idx];
                        if param.is_boolean() {
                            self.toggle_instrument_boolean();
                        } else if param.is_enum() {
                            self.ui.dropdown_open = true;
                            self.ui.dropdown_cursor = 0;
                            self.ui.input_mode = InputMode::Dropdown;
                            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
                            let val = slot.defaults.get(param_idx);
                            self.ui.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if self.ui.instrument_param_cursor == 0 {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                } else if let Some(desc) = self.current_instrument_descriptor() {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        let param = &desc.params[param_idx];
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

    pub(super) fn handle_mod_tab_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Left => {
                self.ui.params_column = 0;
                self.sync_effect_tab_cursor();
            }
            KeyCode::Right => {}
            KeyCode::Up => {
                if shift {
                    self.adjust_mod_param(1.0);
                } else if self.ui.mod_param_cursor > 0 {
                    self.ui.mod_param_cursor -= 1;
                    self.ensure_mod_cursor_visible();
                }
            }
            KeyCode::Down => {
                if shift {
                    self.adjust_mod_param(-1.0);
                } else {
                    let max = self.mod_row_count().saturating_sub(1);
                    if self.ui.mod_param_cursor < max {
                        self.ui.mod_param_cursor += 1;
                        self.ensure_mod_cursor_visible();
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(desc) = self.current_mod_descriptor() {
                    let row_idx = self.ui.mod_param_cursor;
                    if row_idx < desc.params.len() {
                        let param = &desc.params[row_idx];
                        if param.is_boolean() {
                            self.toggle_mod_boolean();
                        } else if param.is_enum() {
                            self.ui.dropdown_open = true;
                            self.ui.dropdown_cursor = 0;
                            self.ui.input_mode = InputMode::Dropdown;
                            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
                            let actual_idx = self.mod_param_indices(self.ui.cursor_track)[row_idx];
                            let val = slot.defaults.get(actual_idx);
                            self.ui.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if let Some(desc) = self.current_mod_descriptor() {
                    let row_idx = self.ui.mod_param_cursor;
                    if row_idx < desc.params.len() {
                        let param = &desc.params[row_idx];
                        if !param.is_boolean() {
                            self.ui.value_buffer.clear();
                            self.ui.value_buffer.push(c);
                            self.ui.input_mode = InputMode::ValueEntry;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_sources_tab_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Left => {
                self.ui.params_column = 0;
                self.sync_effect_tab_cursor();
            }
            KeyCode::Right => {}
            KeyCode::Up => {
                if shift {
                    self.adjust_source_param(1.0);
                } else if self.ui.source_param_cursor > 0 {
                    self.ui.source_param_cursor -= 1;
                    self.ensure_source_cursor_visible();
                }
            }
            KeyCode::Down => {
                if shift {
                    self.adjust_source_param(-1.0);
                } else {
                    let max = self.source_row_count().saturating_sub(1);
                    if self.ui.source_param_cursor < max {
                        self.ui.source_param_cursor += 1;
                        self.ensure_source_cursor_visible();
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(desc) = self.current_source_descriptor() {
                    let row_idx = self.ui.source_param_cursor;
                    if row_idx < desc.params.len() {
                        let param = &desc.params[row_idx];
                        if param.is_boolean() {
                            self.toggle_source_boolean();
                        } else if param.is_enum() {
                            self.ui.dropdown_open = true;
                            self.ui.dropdown_cursor = 0;
                            self.ui.input_mode = InputMode::Dropdown;
                            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
                            let actual_idx =
                                self.source_param_actual_indices(self.ui.cursor_track)[row_idx];
                            let val = slot.defaults.get(actual_idx);
                            self.ui.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if let Some(desc) = self.current_source_descriptor() {
                    let row_idx = self.ui.source_param_cursor;
                    if row_idx < desc.params.len() {
                        let param = &desc.params[row_idx];
                        if !param.is_boolean() {
                            self.ui.value_buffer.clear();
                            self.ui.value_buffer.push(c);
                            self.ui.input_mode = InputMode::ValueEntry;
                        }
                    }
                }
            }
            _ => {}
        }
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
        })
    }

    pub(super) fn current_source_descriptor(&self) -> Option<EffectDescriptor> {
        let desc = self.current_instrument_descriptor()?;
        let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
        let params = self
            .source_param_actual_indices(self.ui.cursor_track)
            .into_iter()
            .filter_map(|i| desc.params.get(i).cloned())
            .map(|mut p| {
                if p.name.ends_with("_div") {
                    p.name = "rate".to_string();
                } else if p.name.ends_with("_rate") {
                    p.name = "rate".to_string();
                } else if p.name.ends_with("_sync") {
                    p.name = "sync".to_string();
                } else if p.name.ends_with("_shape") {
                    p.name = "shape".to_string();
                } else if p.name.ends_with("_pw") {
                    p.name = "pulse width".to_string();
                } else if p.name.ends_with("_retrigger") {
                    p.name = "retrigger".to_string();
                } else if p.name == "mod_rand_slew" {
                    p.name = "slew".to_string();
                } else if p.name == "mod_env_attack" {
                    p.name = "attack".to_string();
                } else if p.name == "mod_env_decay" {
                    p.name = "decay".to_string();
                } else if p.name == "mod_env_sustain" {
                    p.name = "sustain".to_string();
                } else if p.name == "mod_env_release" {
                    p.name = "release".to_string();
                }

                if p.name == "rate" && matches!(p.kind, crate::effects::ParamKind::Enum { .. }) {
                    p.scaling = crate::effects::ParamScaling::Linear;
                }

                if p.name == "sustain" {
                    let _ = slot;
                }
                p
            })
            .collect::<Vec<_>>();
        Some(EffectDescriptor {
            name: "Sources".to_string(),
            params,
            input_channels: 0,
            output_channels: 0,
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
            for step in self.selected_steps() {
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
        let slot = &self.state.pattern.instrument_slots[track];
        let idx = slot.resolve_node_idx(param_idx);
        if crate::voice_modulator::is_bar_resync_param(idx as u32) {
            self.state.schedule_mod_resync();
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
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
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
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: resolved_idx,
                        logical_id: node_id as u64,
                        fvalue: value,
                    },
                );
            }
        }
    }

    pub(super) fn push_instrument_defaults_for_track(&self, track: usize) {
        let slot = &self.state.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..num_params {
            self.send_instrument_param(track, param_idx, slot.defaults.get(param_idx));
        }
    }

    pub(super) fn push_all_restored_instrument_defaults(&self) {
        for track in 0..self.tracks.len() {
            if self.is_sampler_track(track) {
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
