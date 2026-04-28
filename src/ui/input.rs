use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::effects::BUILTIN_SLOT_COUNT;
use crate::lisp_effect;
use crate::sequencer::{KeyboardTrigger, StepParam, STEPS_PER_PAGE};

use super::browser::BrowserNode;
use super::cirklon::track_list_row_layout;
use super::command::{apply_command, AppCommand};
use super::draw::rect_contains;
use super::{
    App, BrowserState, CompileTarget, EffectPaneEntry, EffectTab, InputMode, ParamMouseDrag,
    ParamMouseDragTarget, PendingEditor, Region, SidebarMode, SidebarTab, BAR_HEIGHT, COL_WIDTH,
};

// ── App impl: input handling ──

impl App {
    pub fn handle_input(&mut self) -> std::io::Result<()> {
        self.tick_control_hooks();
        self.poll_agent_request();

        if self.editor.pending_project_load.is_some() {
            let result = self.advance_project_load();
            if let Err(error) = result {
                self.editor.pending_project_load = None;
                self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
            }
            if self.editor.pending_project_load.is_some() && event::poll(Duration::from_millis(33))?
            {
                let _ = event::read()?;
            }
            return Ok(());
        }

        // Poll for async compilation result
        if let Some(ref pending) = self.editor.pending_compile {
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
                        }
                        CompileTarget::Instrument { name } => {
                            self.apply_compiled_instrument(compile_result, &name);
                        }
                    }
                }
                Ok(Err(e)) => {
                    self.editor.status_message =
                        Some((format!("Compile error: {}", e), Instant::now()));
                    self.editor.pending_compile = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Still compiling — increment tick for spinner animation
                    self.editor.pending_compile.as_mut().unwrap().tick += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.editor.status_message =
                        Some(("Compile thread crashed".to_string(), Instant::now()));
                    self.editor.pending_compile = None;
                }
            }
        }

        // Block normal input while compiling — just consume events
        if self.editor.pending_compile.is_some() {
            if event::poll(Duration::from_millis(33))? {
                let _ = event::read()?;
            }
            return Ok(());
        }

        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) => {
                    // Handle key release for note-off (armed keyboard playing)
                    if key.kind == KeyEventKind::Release {
                        if self.any_track_armed() {
                            if let KeyCode::Char(c) = key.code {
                                self.handle_note_release(c);
                            }
                        }
                        return Ok(());
                    }
                    if key.kind != KeyEventKind::Press {
                        return Ok(());
                    }
                    // Tab/BackTab: always exit current mode and cycle region
                    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
                        && self.ui.input_mode != InputMode::Normal
                    {
                        self.ui.input_mode = InputMode::Normal;
                    }
                    // Ctrl+A: always pass through to handle_normal regardless of mode
                    if matches!(key.code, KeyCode::Char('a'))
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && self.ui.input_mode != InputMode::Normal
                    {
                        self.handle_normal(key.code, key.modifiers);
                        return Ok(());
                    }
                    // Backspace/Delete with visual selection: clear all selected steps from any mode
                    if matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
                        && !self.ui.visual_steps.is_empty()
                        && !self.tracks.is_empty()
                    {
                        let track = self.ui.cursor_track;
                        let steps: Vec<usize> = self.ui.visual_steps.drain().collect();
                        apply_command(self, AppCommand::ClearSteps { track, steps });
                        return Ok(());
                    }
                    match self.ui.input_mode {
                        InputMode::Normal => self.handle_normal(key.code, key.modifiers),
                        InputMode::ValueEntry => self.handle_value_entry(key.code),
                        InputMode::Dropdown => self.handle_dropdown(key.code),
                        InputMode::PatternSelect => self.handle_pattern_select(key.code),
                        InputMode::PresetNameEntry => self.handle_preset_name_entry(key.code),
                        InputMode::ProjectNameEntry => self.handle_project_name_entry(key.code),
                        InputMode::WavExportNameEntry => {
                            self.handle_wav_export_name_entry(key.code)
                        }
                        InputMode::EffectPicker => self.handle_effect_picker(key.code),
                        InputMode::InstrumentPicker => {
                            self.handle_instrument_picker_overlay(key.code)
                        }
                        InputMode::ProjectPicker => self.handle_project_picker(key.code),
                        InputMode::StepInsert => self.handle_step_insert(key.code, key.modifiers),
                        InputMode::StepSelect => self.handle_step_select(key.code, key.modifiers),
                        InputMode::StepArm => self.handle_step_arm(key.code, key.modifiers),
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.handle_mouse_click(mouse.column, mouse.row, mouse.modifiers);
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        self.handle_mouse_drag(mouse.column, mouse.row);
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        self.ui.param_mouse_drag = None;
                        self.ui.track_drag_anchor = None;
                    }
                    MouseEventKind::ScrollUp => {
                        self.handle_mouse_scroll(mouse.column, mouse.row, -3);
                    }
                    MouseEventKind::ScrollDown => {
                        self.handle_mouse_scroll(mouse.column, mouse.row, 3);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_normal(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.sidebar_text_input_active()
            && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            match code {
                KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Enter
                | KeyCode::Esc => {
                    BrowserState::handle_sidebar_input(self, code);
                    return;
                }
                _ => {}
            }
        }

        // Global keys first
        match code {
            KeyCode::Char('q') => {
                self.ui.should_quit = true;
                return;
            }
            KeyCode::Char('s')
                if modifiers.contains(KeyModifiers::CONTROL)
                    && self.ui.focused_region != Region::Sidebar =>
            {
                if self.current_project_name.is_some() {
                    if let Some(name) = self.current_project_name.clone() {
                        match self.save_project_named(&name) {
                            Ok(()) => {
                                self.editor.status_message =
                                    Some((format!("Saved project '{}'", name), Instant::now()));
                            }
                            Err(error) => {
                                self.editor.status_message =
                                    Some((format!("Error: {error}"), Instant::now()));
                            }
                        }
                    }
                } else {
                    self.open_project_name_prompt();
                }
                return;
            }
            KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !(self.ui.focused_region == Region::Sidebar
                    && self.effective_sidebar_mode() == SidebarMode::Presets)
                {
                    self.open_project_picker();
                    return;
                }
            }
            KeyCode::Char(' ') if self.ui.focused_region != Region::Sidebar => {
                let was_playing = self.state.is_playing();
                apply_command(self, AppCommand::TogglePlay);
                if was_playing {
                    self.state.transport.playhead.store(0, Ordering::Relaxed);
                    for tph in &self.state.transport.track_playheads {
                        tph.store(0, Ordering::Relaxed);
                    }
                } else {
                    self.state
                        .transport
                        .mod_reset_counter
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.tracks.is_empty() {
                    // If focused on a loaded custom slot, edit it directly
                    let Some(slot_idx) = self.selected_effect_slot() else {
                        return;
                    };
                    if slot_idx >= BUILTIN_SLOT_COUNT
                        && self.ui.focused_region == Region::Params
                        && self.ui.params_column == 1
                    {
                        let chain = &self.state.pattern.effect_chains[self.ui.cursor_track];
                        if slot_idx < chain.len()
                            && chain[slot_idx].node_id.load(Ordering::Relaxed) != 0
                        {
                            // Edit existing effect
                            let name = self.graph.effect_descriptors[self.ui.cursor_track]
                                [slot_idx]
                                .name
                                .clone();
                            self.editor.pending_editor = Some(PendingEditor::Effect {
                                slot_idx,
                                name: Some(name),
                            });
                            return;
                        }
                    }
                    // Otherwise, open the effect picker
                    self.editor.picker_items = lisp_effect::list_saved_effects();
                    self.editor.picker_cursor = 0;
                    self.editor.picker_filter.clear();
                    self.ui.input_mode = InputMode::EffectPicker;
                }
                return;
            }
            KeyCode::Char('i') if modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+I: edit instrument on current custom track
                if !self.tracks.is_empty() && !self.is_sampler_track(self.ui.cursor_track) {
                    let name = self.tracks[self.ui.cursor_track].clone();
                    self.editor.pending_editor =
                        Some(PendingEditor::Instrument { name: Some(name) });
                }
                return;
            }
            KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.tracks.is_empty() {
                    self.editor.pending_editor = Some(PendingEditor::Scratch);
                }
                return;
            }
            KeyCode::Tab => {
                self.ui.focused_region = self.ui.focused_region.next();
                if self.ui.focused_region == Region::Sidebar {
                    self.ui.sidebar_tab = if self.tracks.is_empty() {
                        SidebarTab::Sounds
                    } else {
                        self.ui.sidebar_tab
                    };
                } else if self.ui.focused_region == Region::Params {
                    self.ui.params_column = 1;
                }
                return;
            }
            KeyCode::BackTab => {
                self.ui.focused_region = self.ui.focused_region.prev();
                if self.ui.focused_region == Region::Sidebar {
                    self.ui.sidebar_tab = if self.tracks.is_empty() {
                        SidebarTab::Sounds
                    } else {
                        self.ui.sidebar_tab
                    };
                } else if self.ui.focused_region == Region::Params {
                    self.ui.params_column = 1;
                }
                return;
            }
            KeyCode::Esc => {
                if self.has_selection() {
                    self.ui.selection_anchor = None;
                    self.ui.visual_steps.clear();
                    if self.ui.focused_region != Region::Sidebar {
                        return;
                    }
                }
                if self.ui.focused_region == Region::Sidebar {
                    BrowserState::handle_sidebar_input(self, KeyCode::Esc);
                }
                return;
            }
            // Ctrl+A: select all steps (Cirklon) or switch to Audition (Sidebar)
            KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                if self.ui.focused_region == Region::Sidebar {
                    if !self.tracks.is_empty() {
                        self.ui.sidebar_mode = self.effective_sidebar_mode();
                    }
                } else if !self.tracks.is_empty() {
                    self.ui.visual_steps.clear();
                    self.ui.selection_anchor = None;
                    for step in 0..self.num_steps() {
                        self.ui.visual_steps.insert(step);
                    }
                }
                return;
            }
            // Ctrl+C: copy selected steps to clipboard
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.tracks.is_empty() && self.ui.focused_region != Region::Sidebar {
                    let steps = self.selected_steps();
                    if !steps.is_empty() {
                        let track = self.ui.cursor_track;
                        let anchor = steps[0];
                        let clipboard: Vec<(usize, _)> = steps
                            .iter()
                            .map(|&s| (s - anchor, self.state.capture_step_snapshot(track, s)))
                            .collect();
                        let count = clipboard.len();
                        self.ui.step_clipboard = Some(clipboard);
                        self.editor.status_message = Some((
                            format!("Copied {} step{}", count, if count == 1 { "" } else { "s" }),
                            Instant::now(),
                        ));
                    }
                }
                return;
            }
            // Ctrl+V: paste clipboard at cursor
            KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.tracks.is_empty() && self.ui.focused_region != Region::Sidebar {
                    if let Some(clipboard) = self.ui.step_clipboard.take() {
                        let track = self.ui.cursor_track;
                        let dest_start = self.ui.cursor_step;
                        let num_steps = self.num_steps();
                        apply_command(
                            self,
                            AppCommand::PasteSteps {
                                track,
                                clipboard: clipboard.clone(),
                                dest_start,
                                num_steps,
                            },
                        );
                        // Put clipboard back so it can be pasted again
                        self.ui.step_clipboard = Some(clipboard);
                    }
                }
                return;
            }
            // Ctrl+N: focus sidebar in InstrumentPicker mode
            KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui.instrument_picker_cursor = 0;
                self.ui.sidebar_mode = SidebarMode::InstrumentPicker;
                self.focus_sidebar_sounds();
                return;
            }
            KeyCode::Char('s')
                if modifiers.contains(KeyModifiers::CONTROL)
                    && self.ui.focused_region == Region::Sidebar
                    && self.effective_sidebar_mode() == SidebarMode::Presets =>
            {
                self.ui.value_buffer.clear();
                self.ui.preset_prompt_kind = super::PresetPromptKind::SaveNew;
                self.ui.input_mode = InputMode::PresetNameEntry;
                return;
            }
            KeyCode::Char('o')
                if modifiers.contains(KeyModifiers::CONTROL)
                    && self.ui.focused_region == Region::Sidebar
                    && self.effective_sidebar_mode() == SidebarMode::Presets =>
            {
                self.overwrite_loaded_preset();
                return;
            }
            KeyCode::Char('r')
                if modifiers.contains(KeyModifiers::CONTROL)
                    && self.ui.focused_region == Region::Sidebar
                    && self.effective_sidebar_mode() == SidebarMode::Presets =>
            {
                self.revert_loaded_preset();
                return;
            }
            // , → toggle recording (when any track armed)
            KeyCode::Char(',') => {
                if self.any_track_armed() {
                    self.ui.recording = !self.ui.recording;
                }
                return;
            }
            // / → disarm all tracks and focus sidebar search
            KeyCode::Char('/') => {
                if self.ui.focused_region == Region::Sidebar
                    && self.ui.sidebar_tab == SidebarTab::Agent
                {
                    BrowserState::handle_sidebar_input(self, code);
                    return;
                }
                for armed in self.graph.record_armed.iter_mut() {
                    *armed = false;
                }
                let all_tracks: Vec<usize> = (0..self.graph.record_armed.len()).collect();
                self.release_held_notes_for_tracks(&all_tracks);
                self.ui.recording = false;
                self.focus_sidebar_sounds();
                return;
            }
            _ => {}
        }

        // Keyboard playing interception when any track is armed
        if self.any_track_armed() {
            if let KeyCode::Char(c) = code {
                match c {
                    'z' => {
                        self.ui.keyboard_octave = (self.ui.keyboard_octave - 12).max(-48);
                        return;
                    }
                    'x' => {
                        self.ui.keyboard_octave = (self.ui.keyboard_octave + 12).min(48);
                        return;
                    }
                    '[' => {
                        // Shift record quantize threshold earlier (compensate for more output latency)
                        apply_command(
                            self,
                            AppCommand::AdjustRecordQuantizeThresh { delta: -0.05 },
                        );
                        return;
                    }
                    ']' => {
                        // Shift record quantize threshold later
                        apply_command(self, AppCommand::AdjustRecordQuantizeThresh { delta: 0.05 });
                        return;
                    }
                    _ => {
                        if let Some(semitone) = Self::note_from_key(c) {
                            // Ignore key repeat — only trigger on first press
                            if self.ui.held_notes.iter().any(|note| note.key == c) {
                                return;
                            }
                            let transpose = semitone as f32 + self.ui.keyboard_octave as f32;
                            let mut pressed_tracks = Vec::new();
                            // Send note-on to audio thread for all armed tracks
                            for (track, armed) in self.graph.record_armed.iter().enumerate() {
                                if *armed {
                                    pressed_tracks.push(track);
                                    let _ = self.graph.keyboard_tx.send(KeyboardTrigger {
                                        track,
                                        transpose,
                                        velocity: 1.0,
                                        note_off: false,
                                    });
                                }
                            }
                            // Round-to-nearest-step using fractional phase from audio thread.
                            // If we're past the quantize threshold within the current step,
                            // snap forward to the next step (the user is anticipating it).
                            let step = self.state.transport.playhead.load(Ordering::Relaxed);
                            let phase = f32::from_bits(
                                self.state.transport.playhead_phase.load(Ordering::Relaxed),
                            );
                            let thresh = f32::from_bits(
                                self.state
                                    .transport
                                    .record_quantize_thresh
                                    .load(Ordering::Relaxed),
                            );
                            let step_now = if phase >= thresh {
                                step.wrapping_add(1) as usize
                            } else {
                                step as usize
                            };
                            self.ui.held_notes.push(crate::ui::HeldKeyboardNote {
                                key: c,
                                transpose,
                                step_at_press: step_now,
                                press_time: Instant::now(),
                                tracks: pressed_tracks,
                            });
                            return;
                        }
                    }
                }
            }
        }

        // Step modes: global, work from any region except sidebar (where keys are for filter typing)
        if !self.tracks.is_empty() && self.ui.focused_region != Region::Sidebar {
            match code {
                KeyCode::Char('i') => {
                    self.ui.input_mode = InputMode::StepInsert;
                    return;
                }
                KeyCode::Char('s') => {
                    self.ui.visual_steps.clear();
                    self.ui.input_mode = InputMode::StepSelect;
                    return;
                }
                KeyCode::Char('r') => {
                    self.ui.input_mode = InputMode::StepArm;
                    return;
                }
                _ => {}
            }
        }

        // Region-specific dispatch
        match self.ui.focused_region {
            Region::Cirklon => self.handle_cirklon_input(code, modifiers),
            Region::Sidebar => BrowserState::handle_sidebar_input(self, code),
            Region::Params => self.handle_params_input(code, modifiers),
        }
    }

    fn sidebar_text_input_active(&self) -> bool {
        if self.ui.focused_region != Region::Sidebar {
            return false;
        }

        match self.ui.sidebar_tab {
            SidebarTab::Agent => true,
            SidebarTab::Sounds => {
                self.ui.sidebar_search_focused
                    && self.effective_sidebar_mode() != SidebarMode::InstrumentPicker
            }
            SidebarTab::Tools => false,
        }
    }

    fn handle_mouse_click(&mut self, col: u16, row: u16, modifiers: KeyModifiers) {
        self.ui.param_mouse_drag = None;
        match self.ui.input_mode {
            // Allow mouse through in Normal and step modes
            InputMode::Normal | InputMode::StepInsert | InputMode::StepSelect => {}
            // Close picker overlay on click outside
            InputMode::EffectPicker => {
                self.ui.input_mode = InputMode::Normal;
                return;
            }
            // Block mouse in other overlay modes
            _ => return,
        }

        let l = &self.ui.layout;

        // Any click outside sidebar: exit AddTrack mode
        if !rect_contains(l.sidebar_inner, col, row) && !self.tracks.is_empty() {
            self.ui.sidebar_mode = SidebarMode::Audition;
        }

        // Sidebar: click selects item and focuses sidebar
        if rect_contains(l.sidebar_tabs, col, row) {
            self.ui.focused_region = Region::Sidebar;
            let x_rel = col.saturating_sub(l.sidebar_tabs.x);
            let tools_start = 0;
            let tools_end = 7; // " Tools "
            let agent_start = 8; // after " Tools " + " "
            let agent_end = agent_start + 7; // " Agent "
            let sounds_start = 16; // after tools + gap + agent + gap
            let sounds_end = sounds_start + 8; // " Sounds "
            if x_rel >= tools_start && x_rel < tools_end && !self.tracks.is_empty() {
                self.ui.sidebar_tab = SidebarTab::Tools;
            } else if x_rel >= agent_start && x_rel < agent_end {
                self.ui.sidebar_tab = SidebarTab::Agent;
            } else if x_rel >= sounds_start && x_rel < sounds_end {
                self.ui.sidebar_tab = SidebarTab::Sounds;
            }
            return;
        }

        if rect_contains(l.sidebar_inner, col, row) {
            self.ui.focused_region = Region::Sidebar;
            if self.ui.sidebar_tab == SidebarTab::Tools {
                if rect_contains(l.track_params_inner, col, row) {
                    let row_idx =
                        self.ui.tools_scroll_offset + (row - l.track_params_inner.y) as usize;
                    self.ui.tools_cursor = row_idx;
                    self.sync_tools_cursor();
                    self.ui.params_column = 0;
                    match row_idx {
                        idx if idx <= super::TP_LAST => {
                            let start_display_value = match idx {
                                super::TP_ATTACK => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_attack_ms(),
                                ),
                                super::TP_RELEASE => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_release_ms(),
                                ),
                                super::TP_SWING => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_swing(),
                                ),
                                super::TP_STEPS => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_num_steps() as f32,
                                ),
                                super::TP_VOLUME => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_volume(),
                                ),
                                super::TP_PAN => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track].get_pan(),
                                ),
                                super::TP_SEND => Some(
                                    self.state.pattern.track_params[self.ui.cursor_track]
                                        .get_send(),
                                ),
                                super::TP_MASTER => Some(f32::from_bits(
                                    self.state.transport.master_volume.load(Ordering::Relaxed),
                                )),
                                _ => None,
                            };
                            if let Some(start_display_value) = start_display_value {
                                self.ui.param_mouse_drag = Some(ParamMouseDrag {
                                    track: self.ui.cursor_track,
                                    target: ParamMouseDragTarget::TrackParam { row_idx: idx },
                                    start_col: col,
                                    start_display_value,
                                });
                            }
                        }
                        idx => {
                            let accum_idx = idx.saturating_sub(super::TP_LAST + 1);
                            if accum_idx == super::AC_LIMIT {
                                let start_display_value = self.state.pattern.track_params
                                    [self.ui.cursor_track]
                                    .get_accum_limit();
                                self.ui.param_mouse_drag = Some(ParamMouseDrag {
                                    track: self.ui.cursor_track,
                                    target: ParamMouseDragTarget::AccumParam { row_idx: accum_idx },
                                    start_col: col,
                                    start_display_value,
                                });
                            }
                        }
                    }
                    return;
                }
            } else if self.ui.sidebar_tab == SidebarTab::Agent {
                let model_row_y = l.sidebar_inner.y;
                let dropdown_x = l.sidebar_inner.x + 7;
                let dropdown_width = l.sidebar_inner.width.saturating_sub(7).min(28).max(16);
                let options = self.agent_model_options();
                let visible_count = options
                    .len()
                    .min(l.sidebar_inner.height.saturating_sub(1) as usize);
                let scroll = self
                    .agent_panel
                    .model_dropdown_cursor
                    .saturating_sub(visible_count.saturating_sub(1));

                if self.agent_panel.model_dropdown_open
                    && col >= dropdown_x
                    && col < dropdown_x + dropdown_width
                    && row > model_row_y
                    && row <= model_row_y + visible_count as u16
                {
                    let idx = scroll + (row - model_row_y - 1) as usize;
                    if idx < options.len() {
                        self.select_agent_model_index(idx);
                    }
                    self.agent_panel.model_dropdown_open = false;
                    return;
                }

                if row == model_row_y {
                    self.agent_panel.model_dropdown_open = !self.agent_panel.model_dropdown_open;
                    if self.agent_panel.model_dropdown_open {
                        self.agent_panel.model_dropdown_cursor =
                            self.selected_agent_model_index().unwrap_or(0);
                    }
                    return;
                }

                self.agent_panel.model_dropdown_open = false;
                return;
            } else if self.effective_sidebar_mode() == SidebarMode::Presets {
                let list_start_y = l.sidebar_inner.y + 3;
                if row >= list_start_y {
                    let vi = (row - list_start_y) as usize;
                    let idx = self.preset_browser.scroll_offset + vi;
                    let items = self.visible_preset_items();
                    if idx < items.len() {
                        self.preset_browser.cursor = idx;
                        self.load_selected_preset_into_track();
                        self.ui.sidebar_search_focused = false;
                    }
                }
            } else {
                // First row is the filter/search bar; rows below are list items
                let list_start_y = l.sidebar_inner.y + 1;
                if row < list_start_y {
                    // Clicked the search bar row — focus it
                    self.ui.sidebar_search_focused = true;
                } else {
                    let vi = (row - list_start_y) as usize;
                    let idx = self.browser.scroll_offset + vi;
                    let items = self.browser.visible_items();
                    if idx < items.len() {
                        self.browser.cursor = idx;
                        let item = &items[idx];
                        let path = item.path.clone();
                        if item.is_dir {
                            BrowserNode::toggle_expanded(&mut self.browser.tree, &path);
                        } else {
                            self.sidebar_select_file(&path);
                        }
                        // Blur search so computer keyboard is free for armed tracks
                        self.ui.sidebar_search_focused = false;
                    }
                }
            }
            return;
        }

        // Play button: click toggles playback
        if rect_contains(l.info_bar, col, row) {
            let was_playing = self.state.is_playing();
            apply_command(self, AppCommand::TogglePlay);
            if !self.state.is_playing() {
                self.state.transport.playhead.store(0, Ordering::Relaxed);
            } else if !was_playing {
                self.state
                    .transport
                    .mod_reset_counter
                    .fetch_add(1, Ordering::Relaxed);
            }
            return;
        }

        // REC button: click toggles recording
        if rect_contains(l.rec_button, col, row) {
            self.ui.recording = !self.ui.recording;
            return;
        }

        if rect_contains(l.master_rec_button, col, row) {
            self.toggle_master_recording();
            return;
        }

        // Pattern buttons (row 1 of info bar)
        if rect_contains(l.pattern_buttons_area, col, row) {
            use super::PatternBtn;
            for (x_start, x_end, btn) in &self.ui.pattern_btn_layout {
                if col >= *x_start && col < *x_end {
                    match btn {
                        PatternBtn::PrevPage => {
                            if self.ui.pattern_page > 0 {
                                self.ui.pattern_page -= 1;
                            }
                        }
                        PatternBtn::NextPage => {
                            self.ui.pattern_page += 1;
                        }
                        PatternBtn::Pattern(idx) => {
                            let num_tracks = self.tracks.len();
                            if let Some(sample_ids) = self.state.switch_pattern(
                                *idx,
                                num_tracks,
                                &self.graph.track_buffer_ids,
                                &self.tracks,
                                &self.graph.track_instrument_types,
                            ) {
                                self.graph_controller().apply_sample_ids(&sample_ids);
                                self.push_all_restored_defaults();
                            }
                            self.clamp_cursor_to_steps();
                        }
                        PatternBtn::Clone => {
                            let num_tracks = self.tracks.len();
                            let new_idx = self.state.clone_pattern(
                                num_tracks,
                                &self.graph.track_buffer_ids,
                                &self.tracks,
                                &self.graph.track_instrument_types,
                            );
                            // Show the page containing the new pattern
                            self.ui.pattern_page = new_idx / 10;
                        }
                        PatternBtn::Delete => {
                            let num_tracks = self.tracks.len();
                            if let Some(sample_ids) = self.state.delete_pattern(
                                num_tracks,
                                &self.graph.track_buffer_ids,
                                &self.tracks,
                                &self.graph.track_instrument_types,
                            ) {
                                self.graph_controller().apply_sample_ids(&sample_ids);
                                self.push_all_restored_defaults();
                            }
                            self.clamp_cursor_to_steps();
                            // Adjust page if current page is now past the end
                            let num_pats =
                                self.state.pattern.num_patterns.load(Ordering::Relaxed) as usize;
                            let max_page = num_pats.saturating_sub(1) / 10;
                            if self.ui.pattern_page > max_page {
                                self.ui.pattern_page = max_page;
                            }
                        }
                    }
                    break;
                }
            }
            return;
        }

        // Track list: click selects track, click on arm dot toggles arm
        if rect_contains(l.track_list, col, row) {
            let idx = (row - l.track_list.y) as usize;
            if idx < self.tracks.len() {
                let layout = track_list_row_layout(l.track_list);
                let shift_select = modifiers.contains(KeyModifiers::SHIFT);
                let previous_track = self.ui.cursor_track;
                self.ui.cursor_track = idx;
                let arm_hit_left = layout.arm_x.saturating_sub(1);
                let arm_hit_right = layout.arm_x + 2;
                if col >= arm_hit_left && col < arm_hit_right {
                    let was_armed = self.graph.record_armed[idx];
                    self.graph.record_armed[idx] = !self.graph.record_armed[idx];
                    if was_armed && !self.graph.record_armed[idx] {
                        self.release_held_notes_for_tracks(&[idx]);
                    }
                    self.ui.focused_region = Region::Cirklon;
                } else if col >= layout.volume_x && col < layout.volume_x + layout.volume_width {
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
                        idx >= lo && idx <= hi
                    };
                    if apply_bulk {
                        self.for_each_selected_track(|app, track| {
                            app.state.pattern.track_params[track].set_volume(volume);
                            app.push_track_volume(track);
                        });
                    } else {
                        self.ui.track_selection_anchor = None;
                        self.state.pattern.track_params[idx].set_volume(volume);
                        self.push_track_volume(idx);
                    }
                    self.ui.param_mouse_drag = Some(ParamMouseDrag {
                        track: idx,
                        target: ParamMouseDragTarget::TrackListVolume,
                        start_col: col,
                        start_display_value: volume,
                    });
                    self.ui.focused_region = Region::Cirklon;
                } else {
                    let drag_anchor = if shift_select {
                        self.ui.track_selection_anchor.unwrap_or(previous_track)
                    } else {
                        idx
                    };
                    self.ui.track_drag_anchor = Some(drag_anchor);
                    if shift_select {
                        if self.ui.track_selection_anchor.is_none() && !self.tracks.is_empty() {
                            self.ui.track_selection_anchor = Some(previous_track);
                        }
                    } else {
                        self.ui.track_selection_anchor = None;
                    }
                    self.ui.cursor_track = idx;
                    self.clamp_cursor_to_steps();
                    self.ui.focused_region = Region::Cirklon;
                }
                self.browser.sync_to_track(
                    &self.tracks,
                    self.ui.cursor_track,
                    self.is_sampler_track(self.ui.cursor_track),
                    &self.ui,
                );
            }
            return;
        }

        // Param tabs row: click selects active param
        if rect_contains(l.param_tabs, col, row) {
            let x_off = col.saturating_sub(l.param_tabs.x + 2);
            let tab_idx = (x_off / 6) as usize;
            if tab_idx < StepParam::VISIBLE.len() {
                self.ui.active_param = StepParam::VISIBLE[tab_idx];
                self.ui.focused_region = Region::Cirklon;
            }
            return;
        }

        // Bars area: click selects step, double-click toggles
        if rect_contains(l.bars, col, row) {
            if let Some(step) = self.step_from_click_x(col, l.bars.x) {
                self.touch_follow_timer();
                self.ui.cursor_step = step;
                self.ui.focused_region = Region::Cirklon;
                self.ui.param_mouse_drag = Some(ParamMouseDrag {
                    track: self.ui.cursor_track,
                    target: ParamMouseDragTarget::CirklonStepParam { step },
                    start_col: col,
                    start_display_value: self.state.pattern.step_data[self.ui.cursor_track]
                        .get(step, self.ui.active_param),
                });
                self.apply_param_mouse_drag(col, row);
            }
            return;
        }

        // Trigger row: click selects step, double-click toggles
        if rect_contains(l.trigger_row, col, row) {
            if let Some(step) = self.step_from_click_x(col, l.trigger_row.x) {
                self.handle_step_click(step, modifiers);
            }
            return;
        }

        if rect_contains(l.effects_tabs, col, row) {
            let idx = (row - l.effects_tabs.y) as usize;
            let entries = self.effect_pane_entries();
            if idx < entries.len() {
                self.ui.effect_tab_cursor = idx;
                match entries[idx] {
                    EffectPaneEntry::Tab(tab) => self.select_effect_tab(tab),
                    EffectPaneEntry::PlusButton => {
                        self.editor.picker_items = lisp_effect::list_saved_effects();
                        self.editor.picker_cursor = 0;
                        self.editor.picker_filter.clear();
                        self.ui.input_mode = InputMode::EffectPicker;
                    }
                }
                self.ui.focused_region = Region::Params;
                self.ui.params_column = 0;
            }
            return;
        }

        // Effects inner: click selects effect param row
        if rect_contains(l.effects_inner, col, row) {
            if self.ui.effect_tab == EffectTab::Synth {
                if let Some(row_idx) = self.synth_row_at_position(l.effects_inner, col, row) {
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.ui.instrument_param_cursor = row_idx;
                    self.ensure_synth_cursor_visible();
                    if let Some(start_display_value) =
                        self.synth_row_display_value(self.ui.cursor_track, row_idx)
                    {
                        self.ui.param_mouse_drag = Some(ParamMouseDrag {
                            track: self.ui.cursor_track,
                            target: ParamMouseDragTarget::SynthParam { row_idx },
                            start_col: col,
                            start_display_value,
                        });
                    }
                }
            } else if self.ui.effect_tab == EffectTab::Mod {
                if let Some(row_idx) = self.mod_row_at_position(l.effects_inner, col, row) {
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.ui.mod_param_cursor = row_idx;
                    self.ensure_mod_cursor_visible();
                    if let Some(desc) = self.current_mod_descriptor() {
                        if let Some(param) = desc.params.get(row_idx) {
                            if param.is_enum() {
                                self.ui.dropdown_open = true;
                                self.ui.dropdown_cursor = 0;
                                self.ui.input_mode = InputMode::Dropdown;
                                let actual_idx =
                                    self.mod_param_indices(self.ui.cursor_track)[row_idx];
                                let slot =
                                    &self.state.pattern.instrument_slots[self.ui.cursor_track];
                                let val = slot.defaults.get(actual_idx);
                                self.ui.dropdown_cursor = val.round() as usize;
                                self.ui.param_mouse_drag = None;
                                return;
                            }
                        }
                    }
                    if let Some(start_display_value) =
                        self.mod_row_display_value(self.ui.cursor_track, row_idx)
                    {
                        self.ui.param_mouse_drag = Some(ParamMouseDrag {
                            track: self.ui.cursor_track,
                            target: ParamMouseDragTarget::ModParam { row_idx },
                            start_col: col,
                            start_display_value,
                        });
                    }
                }
            } else if self.ui.effect_tab == EffectTab::Sources {
                if let Some(row_idx) = self.source_row_at_position(l.effects_inner, col, row) {
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.ui.source_param_cursor = row_idx;
                    self.ensure_source_cursor_visible();
                    if let Some(desc) = self.current_source_descriptor() {
                        if let Some(param) = desc.params.get(row_idx) {
                            if param.is_enum() {
                                self.ui.dropdown_open = true;
                                self.ui.dropdown_cursor = 0;
                                self.ui.input_mode = InputMode::Dropdown;
                                let actual_idx =
                                    self.source_param_actual_indices(self.ui.cursor_track)[row_idx];
                                let slot =
                                    &self.state.pattern.instrument_slots[self.ui.cursor_track];
                                let val = slot.defaults.get(actual_idx);
                                self.ui.dropdown_cursor = val.round() as usize;
                                self.ui.param_mouse_drag = None;
                                return;
                            }
                        }
                    }
                    if let Some(start_display_value) =
                        self.source_row_display_value(self.ui.cursor_track, row_idx)
                    {
                        self.ui.param_mouse_drag = Some(ParamMouseDrag {
                            track: self.ui.cursor_track,
                            target: ParamMouseDragTarget::SourceParam { row_idx },
                            start_col: col,
                            start_display_value,
                        });
                    }
                }
            } else if self.ui.effect_tab == EffectTab::Reverb {
                let row_idx = (row - l.effects_inner.y) as usize;
                if row_idx < 3 {
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.ui.reverb_param_cursor = row_idx;
                    let start_display_value = match row_idx {
                        0 => Some(self.ui.reverb_size),
                        1 => Some(self.ui.reverb_brightness),
                        2 => Some(self.ui.reverb_replace),
                        _ => None,
                    };
                    if let Some(start_display_value) = start_display_value {
                        self.ui.param_mouse_drag = Some(ParamMouseDrag {
                            track: self.ui.cursor_track,
                            target: ParamMouseDragTarget::ReverbParam { param_idx: row_idx },
                            start_col: col,
                            start_display_value,
                        });
                    }
                }
            } else if let Some(desc) = self.current_slot_descriptor() {
                let Some(row_idx) = self.effect_row_at_position(l.effects_inner, col, row) else {
                    return;
                };
                if row_idx < desc.params.len() {
                    self.ui.focused_region = Region::Params;
                    self.ui.params_column = 1;
                    self.ui.effect_param_cursor = row_idx;
                    self.ensure_effect_cursor_visible();
                    if let Some(slot_idx) = self.selected_effect_slot() {
                        if let Some(start_display_value) =
                            self.effect_row_display_value(self.ui.cursor_track, slot_idx, row_idx)
                        {
                            self.ui.param_mouse_drag = Some(ParamMouseDrag {
                                track: self.ui.cursor_track,
                                target: ParamMouseDragTarget::EffectParam {
                                    slot_idx,
                                    param_idx: row_idx,
                                },
                                start_col: col,
                                start_display_value,
                            });
                        }
                    }
                }
            }
            return;
        }

        // Page blocks: click navigates to that page
        if rect_contains(l.page_blocks_area, col, row) {
            self.touch_follow_timer();
            for &(x_start, x_end, page_idx) in &self.ui.page_btn_layout {
                if col >= x_start && col < x_end {
                    self.ui.cursor_step = page_idx * crate::sequencer::STEPS_PER_PAGE;
                    self.ui.focused_region = Region::Cirklon;
                    break;
                }
            }
            return;
        }

        // Piano roll click: toggle chord notes on cursor step
        if rect_contains(l.piano_area, col, row)
            && row < l.piano_area.y + 2  // only key rows, not label/info rows
            && !self.tracks.is_empty()
        {
            let semitone = self.ui.piano_lo + (col as i32 - l.piano_area.x as i32);
            self.handle_piano_note_click(semitone);
            self.ui.focused_region = Region::Cirklon;
            return;
        }

        // Catch-all: click anywhere in cirklon area focuses cirklon
        if rect_contains(l.cirklon_area, col, row) {
            self.ui.focused_region = Region::Cirklon;
        }
    }

    fn handle_piano_note_click(&mut self, semitone: i32) {
        use crate::sequencer::StepParam;
        let track = self.ui.cursor_track;
        let step = self.ui.cursor_step;
        let is_active = self.state.pattern.patterns[track].is_active(step);
        let chord_count = self.state.pattern.chord_data[track].count(step);

        if !is_active {
            // Activate the step and set this semitone as the Transpose value
            apply_command(
                self,
                AppCommand::SetStepActive {
                    track,
                    step,
                    active: true,
                },
            );
            apply_command(
                self,
                AppCommand::SetStepParam {
                    track,
                    step,
                    param: StepParam::Transpose,
                    value: semitone as f32,
                },
            );
            return;
        }

        if chord_count == 0 {
            // Single-note step using Transpose
            let current = self.state.pattern.step_data[track]
                .get(step, StepParam::Transpose)
                .round() as i32;
            if semitone == current {
                // Clicking the same note deactivates the step
                apply_command(
                    self,
                    AppCommand::SetStepActive {
                        track,
                        step,
                        active: false,
                    },
                );
            } else {
                // Add a second note: migrate Transpose into chord_data, then add new note
                // These chord mutations go directly through state (ChordData has no AppCommand yet)
                self.state.pattern.chord_data[track].add_note(step, current as f32);
                self.state.pattern.chord_data[track].add_note(step, semitone as f32);
                self.state.publish_scheduler_snapshot();
            }
        } else {
            // Step has chord data — toggle the clicked semitone
            let added = self.state.pattern.chord_data[track].toggle_note(step, semitone as f32);
            let new_count = self.state.pattern.chord_data[track].count(step);
            if !added {
                if new_count == 0 {
                    // Removed last note: deactivate step
                    apply_command(
                        self,
                        AppCommand::SetStepActive {
                            track,
                            step,
                            active: false,
                        },
                    );
                    return;
                } else if new_count == 1 {
                    // One note left: migrate back to Transpose, clear chord
                    let remaining = self.state.pattern.chord_data[track].get(step, 0);
                    apply_command(
                        self,
                        AppCommand::SetStepParam {
                            track,
                            step,
                            param: StepParam::Transpose,
                            value: remaining,
                        },
                    );
                    self.state.pattern.chord_data[track].clear_step(step);
                    self.state.publish_scheduler_snapshot();
                    return;
                }
            }
            self.state.publish_scheduler_snapshot();
        }

        self.state.publish_scheduler_snapshot();
    }

    fn handle_mouse_scroll(&mut self, col: u16, row: u16, delta: isize) {
        let l = &self.ui.layout;
        if rect_contains(l.sidebar_inner, col, row) {
            if self.ui.sidebar_tab == SidebarTab::Agent {
                self.scroll_agent_transcript(-delta);
            } else if self.effective_sidebar_mode() == SidebarMode::Presets {
                let items = self.visible_preset_items();
                let max_visible = self.preset_max_visible();
                let max_scroll = items.len().saturating_sub(max_visible);
                if delta < 0 {
                    self.preset_browser.scroll_offset = self
                        .preset_browser
                        .scroll_offset
                        .saturating_sub((-delta) as usize);
                } else {
                    self.preset_browser.scroll_offset =
                        (self.preset_browser.scroll_offset + delta as usize).min(max_scroll);
                }
            } else {
                self.browser.scroll(delta, &self.ui);
            }
            return;
        }

        if rect_contains(l.effects_inner, col, row) {
            if self.ui.effect_tab == EffectTab::Synth {
                self.ui.synth_scroll_offset = self.scroll_partition_offset(
                    l.effects_inner,
                    self.synth_row_count(),
                    self.ui.synth_scroll_offset,
                    delta,
                );
            } else if self.ui.effect_tab == EffectTab::Mod {
                self.ui.mod_scroll_offset = self.scroll_partition_offset(
                    l.effects_inner,
                    self.mod_row_count(),
                    self.ui.mod_scroll_offset,
                    delta,
                );
            } else if self.ui.effect_tab == EffectTab::Sources {
                self.ui.source_scroll_offset = self.scroll_partition_offset(
                    l.effects_inner,
                    self.source_row_count(),
                    self.ui.source_scroll_offset,
                    delta,
                );
            } else if matches!(self.ui.effect_tab, EffectTab::Slot(_)) {
                self.ui.effect_scroll_offset = self.scroll_partition_offset(
                    l.effects_inner,
                    self.effect_row_count(),
                    self.ui.effect_scroll_offset,
                    delta,
                );
            }
        }
    }

    fn handle_mouse_drag(&mut self, col: u16, row: u16) {
        if self.ui.param_mouse_drag.is_some() {
            self.apply_param_mouse_drag(col, row);
            return;
        }
        if let Some(anchor) = self.ui.track_drag_anchor {
            let track_list = self.ui.layout.track_list;
            if rect_contains(track_list, col, row) {
                let idx = (row - track_list.y) as usize;
                if idx < self.tracks.len() {
                    self.ui.cursor_track = idx;
                    if idx == anchor {
                        self.ui.track_selection_anchor = None;
                    } else {
                        self.ui.track_selection_anchor = Some(anchor);
                    }
                    self.clamp_cursor_to_steps();
                    self.ui.focused_region = Region::Cirklon;
                    self.browser.sync_to_track(
                        &self.tracks,
                        self.ui.cursor_track,
                        self.is_sampler_track(self.ui.cursor_track),
                        &self.ui,
                    );
                }
            }
        }
    }

    fn handle_step_click(&mut self, step: usize, modifiers: KeyModifiers) {
        self.touch_follow_timer();
        let now = Instant::now();
        let previous_step = self.ui.cursor_step;
        let is_double = self
            .ui
            .last_step_click
            .map(|(prev_step, prev_time)| {
                prev_step == step && now.duration_since(prev_time).as_millis() < 400
            })
            .unwrap_or(false);

        self.ui.cursor_step = step;
        self.ui.focused_region = Region::Cirklon;

        let range_select =
            modifiers.contains(KeyModifiers::SHIFT) || modifiers.contains(KeyModifiers::SUPER);
        if range_select {
            if self.ui.selection_anchor.is_none() {
                self.ui.selection_anchor = Some(previous_step);
            }
        } else {
            self.ui.selection_anchor = None;
        }

        if is_double && !self.tracks.is_empty() {
            self.state
                .toggle_step_and_clear_plocks(self.ui.cursor_track, step);
            self.ui.last_step_click = None;
        } else {
            self.ui.last_step_click = Some((step, now));
        }
    }

    pub(super) fn step_from_click_x(&self, col: u16, area_x: u16) -> Option<usize> {
        let x_offset = 2u16;
        if col < area_x + x_offset {
            return None;
        }
        let rel = col - area_x - x_offset;
        let step_in_page = (rel / COL_WIDTH) as usize;
        let (page_start, page_end) = self.page_range();
        let step = page_start + step_in_page;
        if step < page_end && step < self.num_steps() {
            Some(step)
        } else {
            None
        }
    }

    fn normalized_step_value_from_bar_row(&self, row: u16) -> Option<f32> {
        let bars = self.ui.layout.bars;
        if row < bars.y || row >= bars.y + BAR_HEIGHT as u16 {
            return None;
        }
        let rel_row = row.saturating_sub(bars.y) as usize;
        let clamped = rel_row.min(BAR_HEIGHT.saturating_sub(1));
        Some(1.0 - (clamped as f32 + 0.5) / BAR_HEIGHT as f32)
    }

    pub(super) fn step_param_value_from_bar_position(
        &self,
        param: StepParam,
        row: u16,
    ) -> Option<f32> {
        let normalized = self.normalized_step_value_from_bar_row(row)?;
        let value = param.slider_min() + normalized * (param.slider_max() - param.slider_min());
        Some(match param {
            StepParam::AuxA | StepParam::Chop | StepParam::Sync | StepParam::Transpose => {
                value.round()
            }
            _ => value,
        })
    }

    fn handle_value_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.ui.value_buffer.push(c);
            }
            KeyCode::Char('.') => {
                if !self.ui.value_buffer.contains('.') {
                    self.ui.value_buffer.push('.');
                }
            }
            KeyCode::Char('-') => {
                if self.ui.value_buffer.starts_with('-') {
                    self.ui.value_buffer.remove(0);
                } else {
                    self.ui.value_buffer.insert(0, '-');
                }
            }
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
                if self.ui.value_buffer.is_empty() {
                    self.ui.bpm_entry = false;
                    self.ui.input_mode = InputMode::Normal;
                }
            }
            KeyCode::Enter => {
                if let Ok(val) = self.ui.value_buffer.parse::<f32>() {
                    self.apply_value_entry(val);
                }
                self.ui.value_buffer.clear();
                self.ui.bpm_entry = false;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.bpm_entry = false;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn apply_value_entry(&mut self, val: f32) {
        if self.ui.bpm_entry {
            let bpm = (val as u32).clamp(20, 999);
            apply_command(self, AppCommand::SetBpm { bpm });
            self.ui.bpm_entry = false;
            return;
        }

        if self.tracks.is_empty() {
            return;
        }

        match self.ui.focused_region {
            Region::Cirklon => {
                let track = self.ui.cursor_track;
                let param = self.ui.active_param;
                for step in self.selected_steps() {
                    apply_command(
                        self,
                        AppCommand::SetStepParam {
                            track,
                            step,
                            param,
                            value: val,
                        },
                    );
                }
            }
            Region::Params | Region::Sidebar => {
                if self.ui.focused_region == Region::Sidebar || self.ui.params_column == 0 {
                    match self.active_tool_row() {
                        super::params::ToolRow::Accum(super::AC_LIMIT) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackAccumLimit {
                                        track,
                                        value: val.max(0.0),
                                    },
                                );
                            }
                        }
                        super::params::ToolRow::Track(super::TP_ATTACK) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(self, AppCommand::SetTrackAttack { track, ms: val });
                            }
                        }
                        super::params::ToolRow::Track(super::TP_RELEASE) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(self, AppCommand::SetTrackRelease { track, ms: val });
                            }
                        }
                        super::params::ToolRow::Track(super::TP_SWING) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                self.set_track_swing_or_plock(track, val);
                            }
                        }
                        super::params::ToolRow::Track(super::TP_STEPS) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackNumSteps {
                                        track,
                                        n: val as usize,
                                    },
                                );
                            }
                            self.clamp_cursor_to_steps();
                        }
                        super::params::ToolRow::Track(super::TP_VOLUME) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackVolume {
                                        track,
                                        value: val.clamp(0.0, 1.0),
                                    },
                                );
                            }
                        }
                        super::params::ToolRow::Track(super::TP_PAN) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackPan {
                                        track,
                                        value: val.clamp(-1.0, 1.0),
                                    },
                                );
                            }
                        }
                        super::params::ToolRow::Track(super::TP_SEND) => {
                            let tracks = self.selected_tracks();
                            for track in tracks {
                                apply_command(
                                    self,
                                    AppCommand::SetTrackSend {
                                        track,
                                        value: val.clamp(0.0, 1.0),
                                    },
                                );
                            }
                        }
                        super::params::ToolRow::Track(super::TP_MASTER) => {
                            apply_command(
                                self,
                                AppCommand::SetMasterVolume {
                                    value: val.clamp(0.0, 2.0),
                                },
                            );
                        }
                        _ => {}
                    }
                    self.state.publish_scheduler_snapshot();
                } else if self.ui.effect_tab == EffectTab::Reverb {
                    self.set_reverb_param(self.ui.reverb_param_cursor, val);
                } else if self.ui.effect_tab == EffectTab::Mod {
                    let track = self.ui.cursor_track;
                    let mod_indices = self.mod_param_indices(track);
                    let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) else {
                        return;
                    };
                    let desc = match self.graph.instrument_descriptors.get(track) {
                        Some(d) => d,
                        None => return,
                    };
                    let param_desc = &desc.params[param_idx];
                    let store_val = param_desc.clamp(param_desc.user_input_to_stored(val));
                    self.set_instrument_param_or_plock(track, param_idx, store_val);
                } else if self.ui.effect_tab == EffectTab::Sources {
                    let track = self.ui.cursor_track;
                    let source_indices = self.source_param_actual_indices(track);
                    let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
                        return;
                    };
                    let desc = match self.graph.instrument_descriptors.get(track) {
                        Some(d) => d,
                        None => return,
                    };
                    let param_desc = &desc.params[param_idx];
                    let store_val = param_desc.clamp(param_desc.user_input_to_stored(val));
                    self.set_instrument_param_or_plock(track, param_idx, store_val);
                } else if self.ui.effect_tab == EffectTab::Synth {
                    // Synth tab value entry
                    let track = self.ui.cursor_track;
                    if self.ui.instrument_param_cursor == 0 {
                        let store_val = val.clamp(-48.0, 48.0);
                        // Use the helper so publish_scheduler_snapshot is called
                        self.set_instrument_base_note_offset(track, store_val);
                    } else {
                        let synth_indices = self.synth_param_indices(track);
                        let Some(&param_idx) =
                            synth_indices.get(self.ui.instrument_param_cursor - 1)
                        else {
                            return;
                        };
                        let desc = match self.graph.instrument_descriptors.get(track) {
                            Some(d) => d,
                            None => return,
                        };
                        let param_desc = &desc.params[param_idx];
                        let store_val = param_desc.clamp(param_desc.user_input_to_stored(val));
                        let slot = &self.state.pattern.instrument_slots[track];

                        if self.has_selection() {
                            for step in self.selected_steps() {
                                slot.plocks.set(step, param_idx, store_val);
                            }
                            self.state.publish_scheduler_snapshot();
                        } else {
                            // publish is called inside set_instrument_param_or_plock
                            self.set_instrument_param_or_plock(track, param_idx, store_val);
                        }
                    }
                } else {
                    // Unified effect slot value entry
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
                    let store_val = param_desc.clamp(param_desc.user_input_to_stored(val));

                    let chain = &self.state.pattern.effect_chains[track];
                    if slot_idx >= chain.len() {
                        return;
                    }
                    let slot = &chain[slot_idx];

                    if matches!(
                        param_desc.host_control,
                        Some(crate::effects::HostControl::FxSidechain { .. })
                    ) {
                        let selection = store_val.round().max(0.0) as usize;
                        self.apply_effect_sidechain_selection(
                            track, slot_idx, param_idx, selection,
                        );
                        slot.defaults.set(param_idx, selection as f32);
                    } else if self.has_selection() {
                        for step in self.selected_steps() {
                            slot.plocks.set(step, param_idx, store_val);
                        }
                    } else {
                        slot.defaults.set(param_idx, store_val);
                        self.send_slot_param(track, slot_idx, param_idx, store_val);
                    }
                    self.state.publish_scheduler_snapshot();
                }
            }
        }
    }

    pub(super) fn adjust_selected(&mut self, delta: f32) {
        if self.tracks.is_empty() {
            return;
        }
        let track = self.ui.cursor_track;
        let param = self.ui.active_param;
        for step in self.selected_steps() {
            apply_command(
                self,
                AppCommand::AdjustStepParam {
                    track,
                    step,
                    param,
                    delta,
                },
            );
        }
    }

    pub(super) fn shift_selection(&mut self, direction: isize) {
        if self.tracks.is_empty() || !self.has_selection() {
            return;
        }
        let (lo, hi) = self.selected_range();
        let count = hi - lo + 1;
        let shift = direction;
        let ns = self.num_steps();
        let new_lo = (lo as isize + shift).clamp(0, (ns - count) as isize) as usize;

        if new_lo == lo {
            return;
        }
        let track = self.ui.cursor_track;
        apply_command(
            self,
            AppCommand::ShiftStepRange {
                track,
                lo,
                hi,
                new_lo,
            },
        );

        self.ui.cursor_step =
            (self.ui.cursor_step as isize + shift).clamp(0, (ns - 1) as isize) as usize;
        if let Some(ref mut anchor) = self.ui.selection_anchor {
            *anchor = (*anchor as isize + shift).clamp(0, (ns - 1) as isize) as usize;
        }
    }

    fn handle_pattern_select(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if !self.ui.pattern_clone_pending {
                    self.ui.value_buffer.push(c);
                }
            }
            KeyCode::Char('c') => {
                if self.ui.value_buffer.is_empty() && !self.ui.pattern_clone_pending {
                    self.ui.pattern_clone_pending = true;
                    self.ui.value_buffer = "clone".to_string();
                }
            }
            KeyCode::Char('x') => {
                let num_tracks = self.tracks.len();
                if let Some(sample_ids) = self.state.delete_pattern(
                    num_tracks,
                    &self.graph.track_buffer_ids,
                    &self.tracks,
                    &self.graph.track_instrument_types,
                ) {
                    self.graph_controller().apply_sample_ids(&sample_ids);
                    self.push_all_restored_defaults();
                }
                self.clamp_cursor_to_steps();
                self.ui.value_buffer.clear();
                self.ui.pattern_clone_pending = false;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if self.ui.pattern_clone_pending {
                    let num_tracks = self.tracks.len();
                    self.state.clone_pattern(
                        num_tracks,
                        &self.graph.track_buffer_ids,
                        &self.tracks,
                        &self.graph.track_instrument_types,
                    );
                } else if let Ok(n) = self.ui.value_buffer.parse::<usize>() {
                    if n >= 1 {
                        let num_tracks = self.tracks.len();
                        let num_patterns =
                            self.state.pattern.num_patterns.load(Ordering::Relaxed) as usize;
                        let idx = n - 1;
                        if idx < num_patterns {
                            if let Some(sample_ids) = self.state.switch_pattern(
                                idx,
                                num_tracks,
                                &self.graph.track_buffer_ids,
                                &self.tracks,
                                &self.graph.track_instrument_types,
                            ) {
                                self.graph_controller().apply_sample_ids(&sample_ids);
                                self.push_all_restored_defaults();
                            }
                            self.clamp_cursor_to_steps();
                        }
                    }
                }
                self.ui.value_buffer.clear();
                self.ui.pattern_clone_pending = false;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Backspace => {
                if self.ui.pattern_clone_pending {
                    self.ui.pattern_clone_pending = false;
                    self.ui.value_buffer.clear();
                } else {
                    self.ui.value_buffer.pop();
                }
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.pattern_clone_pending = false;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    // ── Step modes ──

    /// Map a key character to a step offset (0..15) within the current page.
    /// Number keys 1-8 map to steps 0-7, QWERTY row q-i maps to steps 8-15.
    /// Also maps shifted variants (!, @, #, etc. and uppercase Q-I).
    fn step_from_mode_key(c: char) -> Option<usize> {
        match c {
            '1' | '!' => Some(0),
            '2' | '@' => Some(1),
            '3' | '#' => Some(2),
            '4' | '$' => Some(3),
            '5' | '%' => Some(4),
            '6' | '^' => Some(5),
            '7' | '&' => Some(6),
            '8' | '*' => Some(7),
            'q' | 'Q' => Some(8),
            'w' | 'W' => Some(9),
            'e' | 'E' => Some(10),
            'r' | 'R' => Some(11),
            't' | 'T' => Some(12),
            'y' | 'Y' => Some(13),
            'u' | 'U' => Some(14),
            'i' | 'I' => Some(15),
            _ => None,
        }
    }

    /// Resolve a mode key to an absolute step index on the current page.
    fn resolve_mode_step(&self, c: char) -> Option<usize> {
        let offset = Self::step_from_mode_key(c)?;
        let (page_start, page_end) = self.page_range();
        let step = page_start + offset;
        if step < page_end && step < self.num_steps() {
            Some(step)
        } else {
            None
        }
    }

    /// Navigate to previous page (used by step modes).
    fn mode_prev_page(&mut self) {
        let ns = self.num_steps();
        let total_pages = ns.div_ceil(STEPS_PER_PAGE);
        if total_pages > 1 {
            let current_page = self.current_page();
            self.ui.cursor_step = ((current_page + total_pages - 1) % total_pages) * STEPS_PER_PAGE;
        }
    }

    /// Navigate to next page (used by step modes).
    fn mode_next_page(&mut self) {
        let ns = self.num_steps();
        let total_pages = ns.div_ceil(STEPS_PER_PAGE);
        if total_pages > 1 {
            let current_page = self.current_page();
            self.ui.cursor_step = ((current_page + 1) % total_pages) * STEPS_PER_PAGE;
        }
    }

    /// Returns true if a mode key character is a shifted variant (accent).
    fn is_accent_key(c: char) -> bool {
        matches!(
            c,
            '!' | '@'
                | '#'
                | '$'
                | '%'
                | '^'
                | '&'
                | '*'
                | 'Q'
                | 'W'
                | 'E'
                | 'R'
                | 'T'
                | 'Y'
                | 'U'
                | 'I'
        )
    }

    /// Arrow key navigation shared by step modes (insert/select).
    fn handle_mode_arrows(&mut self, code: KeyCode) -> bool {
        let ns = self.num_steps();
        match code {
            KeyCode::Up => {
                if self.ui.cursor_track > 0 {
                    self.ui.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.ui.cursor_track = self.tracks.len() - 1;
                }
                self.clamp_cursor_to_steps();
                true
            }
            KeyCode::Down => {
                if !self.tracks.is_empty() {
                    self.ui.cursor_track = (self.ui.cursor_track + 1) % self.tracks.len();
                }
                self.clamp_cursor_to_steps();
                true
            }
            KeyCode::Left => {
                if self.ui.cursor_step > 0 {
                    self.ui.cursor_step -= 1;
                } else {
                    self.ui.cursor_step = ns - 1;
                }
                true
            }
            KeyCode::Right => {
                self.ui.cursor_step = (self.ui.cursor_step + 1) % ns;
                true
            }
            _ => false,
        }
    }

    fn handle_step_insert(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.handle_mode_arrows(code) {
            return;
        }
        let has_shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Char(',') => {
                if self.any_track_armed() {
                    self.ui.recording = !self.ui.recording;
                }
            }
            KeyCode::Char('/') if !has_shift => {
                for armed in self.graph.record_armed.iter_mut() {
                    *armed = false;
                }
                self.ui.recording = false;
                self.ui.focused_region = Region::Sidebar;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Char(' ') => {
                let was_playing = self.state.is_playing();
                apply_command(self, AppCommand::TogglePlay);
                if was_playing {
                    self.state.transport.playhead.store(0, Ordering::Relaxed);
                }
            }
            KeyCode::Esc | KeyCode::Enter => {
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Char('[') => {
                self.mode_prev_page();
            }
            KeyCode::Char(']') => {
                self.mode_next_page();
            }
            KeyCode::Char(c) => {
                // Page navigation: a/s (also A/S with shift held)
                if c == 'a' || c == 'A' {
                    self.mode_prev_page();
                    return;
                }
                if c == 's' || c == 'S' {
                    self.mode_next_page();
                    return;
                }
                // xx: clear all steps in current track pattern
                if c == 'x' || c == 'X' {
                    let now = Instant::now();
                    let is_double = self
                        .ui
                        .last_x_press
                        .map(|t| now.duration_since(t).as_millis() < 400)
                        .unwrap_or(false);
                    if is_double {
                        let track = self.ui.cursor_track;
                        let ns = self.num_steps();
                        let steps: Vec<usize> = (0..ns)
                            .filter(|&s| self.state.pattern.patterns[track].is_active(s))
                            .collect();
                        apply_command(self, AppCommand::ClearSteps { track, steps });
                        self.ui.last_x_press = None;
                        self.editor.status_message =
                            Some(("Pattern cleared".to_string(), Instant::now()));
                    } else {
                        self.ui.last_x_press = Some(now);
                    }
                    return;
                }
                if let Some(step) = self.resolve_mode_step(c) {
                    let track = self.ui.cursor_track;
                    let is_accent = has_shift || Self::is_accent_key(c);
                    let is_active = self.state.pattern.patterns[track].is_active(step);

                    if is_active && is_accent {
                        // Already active + accent: lift velocity to 1.0 instead of toggling off
                        apply_command(
                            self,
                            AppCommand::SetStepParam {
                                track,
                                step,
                                param: StepParam::Velocity,
                                value: 1.0,
                            },
                        );
                    } else if is_active {
                        // Already active + no accent: toggle off
                        apply_command(self, AppCommand::ToggleStep { track, step });
                    } else {
                        // Inactive: toggle on with appropriate velocity
                        let vel = if is_accent { 1.0 } else { 0.5 };
                        apply_command(
                            self,
                            AppCommand::SetStepActive {
                                track,
                                step,
                                active: true,
                            },
                        );
                        apply_command(
                            self,
                            AppCommand::SetStepParam {
                                track,
                                step,
                                param: StepParam::Velocity,
                                value: vel,
                            },
                        );
                    }
                    self.ui.cursor_step = step;
                }
            }
            _ => {}
        }
    }

    fn handle_step_select(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.handle_mode_arrows(code) {
            return;
        }
        match code {
            KeyCode::Char(',') => {
                if self.any_track_armed() {
                    self.ui.recording = !self.ui.recording;
                }
            }
            KeyCode::Char('/') => {
                for armed in self.graph.record_armed.iter_mut() {
                    *armed = false;
                }
                self.ui.recording = false;
                self.ui.focused_region = Region::Sidebar;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Char(' ') => {
                let was_playing = self.state.is_playing();
                apply_command(self, AppCommand::TogglePlay);
                if was_playing {
                    self.state.transport.playhead.store(0, Ordering::Relaxed);
                }
            }
            KeyCode::Esc | KeyCode::Enter => {
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Char('[') | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.mode_prev_page();
            }
            KeyCode::Char(']') | KeyCode::Char('s') | KeyCode::Char('S') => {
                self.mode_next_page();
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                // Delete: untoggle all selected active steps
                let track = self.ui.cursor_track;
                let steps: Vec<usize> = self
                    .ui
                    .visual_steps
                    .iter()
                    .copied()
                    .filter(|&s| self.state.pattern.patterns[track].is_active(s))
                    .collect();
                for step in steps {
                    apply_command(self, AppCommand::ToggleStep { track, step });
                }
                self.ui.visual_steps.clear();
            }
            KeyCode::Char(c) => {
                if let Some(step) = self.resolve_mode_step(c) {
                    // Toggle step in/out of visual selection
                    if self.ui.visual_steps.contains(&step) {
                        self.ui.visual_steps.remove(&step);
                    } else {
                        self.ui.visual_steps.insert(step);
                    }
                    self.ui.cursor_step = step;
                }
            }
            _ => {}
        }
    }

    fn handle_step_arm(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        match code {
            KeyCode::Char(c) => {
                // , → toggle recording from arm mode too
                if c == ',' {
                    if self.any_track_armed() {
                        self.ui.recording = !self.ui.recording;
                    }
                    return;
                }
                // r → exit arm mode (toggle behavior)
                if c == 'r' {
                    self.ui.input_mode = InputMode::Normal;
                    return;
                }
                let track = match c {
                    '1' => Some(0),
                    '2' => Some(1),
                    '3' => Some(2),
                    '4' => Some(3),
                    '5' => Some(4),
                    '6' => Some(5),
                    '7' => Some(6),
                    '8' => Some(7),
                    'q' => Some(8),
                    'w' => Some(9),
                    'e' => Some(10),
                    'r' => Some(11), // unreachable due to early return above
                    't' => Some(12),
                    'y' => Some(13),
                    'u' => Some(14),
                    _ => None,
                };
                if let Some(t) = track {
                    if t < self.tracks.len() && t < self.graph.record_armed.len() {
                        let was_armed = self.graph.record_armed[t];
                        self.graph.record_armed[t] = !self.graph.record_armed[t];
                        if was_armed && !self.graph.record_armed[t] {
                            self.release_held_notes_for_tracks(&[t]);
                        }
                        if !self.any_track_armed() {
                            self.ui.recording = false;
                        }
                    }
                }
            }
            KeyCode::Esc | KeyCode::Enter => {
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn any_track_armed(&self) -> bool {
        self.graph.record_armed.iter().any(|a| *a)
    }

    /// Map QWERTY key to semitone offset (standard DAW layout).
    fn note_from_key(c: char) -> Option<i32> {
        match c {
            'a' => Some(0),  // C
            'w' => Some(1),  // C#
            's' => Some(2),  // D
            'e' => Some(3),  // D#
            'd' => Some(4),  // E
            'f' => Some(5),  // F
            't' => Some(6),  // F#
            'g' => Some(7),  // G
            'y' => Some(8),  // G#
            'h' => Some(9),  // A
            'u' => Some(10), // A#
            'j' => Some(11), // B
            'k' => Some(12), // C+1
            'o' => Some(13), // C#+1
            'l' => Some(14), // D+1
            _ => None,
        }
    }

    /// Handle key release: send note-off to audio and optionally record into pattern.
    fn handle_note_release(&mut self, c: char) {
        // Find and remove the held note for this key
        let held = self.ui.held_notes.iter().position(|note| note.key == c);
        let held = match held {
            Some(idx) => self.ui.held_notes.remove(idx),
            None => return,
        };
        let transpose = held.transpose;
        let step_at_press = held.step_at_press;
        let press_time = held.press_time;

        // Send note-off to the exact tracks that received the original note-on.
        for track in held.tracks {
            let _ = self.graph.keyboard_tx.send(KeyboardTrigger {
                track,
                transpose,
                velocity: 0.0,
                note_off: true,
            });
        }

        // Record into pattern if recording + playing
        if !self.ui.recording || !self.state.is_playing() {
            return;
        }

        // Compute duration in 1/16th note units from hold time
        let bpm = self.state.transport.bpm.load(Ordering::Relaxed) as f64;
        let secs_per_step = 60.0 / bpm / 4.0; // duration of one 1/16th note
        let hold_secs = press_time.elapsed().as_secs_f64();
        let duration_steps = (hold_secs / secs_per_step).max(0.15).min(64.0);

        for (track, armed) in self.graph.record_armed.iter().enumerate() {
            if !*armed {
                continue;
            }
            let num_steps = self.state.pattern.track_params[track].get_num_steps();
            let local_step = step_at_press % num_steps;
            // Enable step trigger
            if !self.state.pattern.patterns[track].is_active(local_step) {
                self.state.pattern.patterns[track].toggle_step(local_step);
            }
            // Add note to chord data (supports multiple notes per step)
            self.state.pattern.chord_data[track].add_note(local_step, transpose);
            // Keep StepData::Transpose in sync with first chord note for bar display
            let first_note = self.state.pattern.chord_data[track].get(local_step, 0);
            self.state.pattern.step_data[track].set(local_step, StepParam::Transpose, first_note);
            // Set velocity and duration p-locks
            self.state.pattern.step_data[track].set(local_step, StepParam::Velocity, 1.0);
            self.state.pattern.step_data[track].set(
                local_step,
                StepParam::Duration,
                duration_steps as f32,
            );
        }

        self.state.publish_scheduler_snapshot();
    }

    pub(super) fn release_held_notes_for_tracks(&mut self, tracks: &[usize]) {
        if tracks.is_empty() {
            return;
        }
        let mut retained = Vec::with_capacity(self.ui.held_notes.len());
        for mut held in self.ui.held_notes.drain(..) {
            for &track in tracks {
                if held.tracks.contains(&track) {
                    let _ = self.graph.keyboard_tx.send(KeyboardTrigger {
                        track,
                        transpose: held.transpose,
                        velocity: 0.0,
                        note_off: true,
                    });
                }
            }
            held.tracks.retain(|track| !tracks.contains(track));
            if !held.tracks.is_empty() {
                retained.push(held);
            }
        }
        self.ui.held_notes = retained;
    }
}
