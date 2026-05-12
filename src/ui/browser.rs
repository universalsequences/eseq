use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::draw::region_border_style;
use super::params::draw_track_params_column;
use super::{
    App, BrowserState, InputMode, PresetPromptKind, Region, SidebarMode, SidebarTab, UiState,
};
use crate::lisp_effect;

// ── Sample Browser tree ──

pub struct BrowserEntry {
    pub depth: usize,
    pub is_dir: bool,
    pub name: String,
    pub path: std::path::PathBuf,
    pub expanded: bool,
}

pub struct BrowserNode {
    pub name: String,
    pub path: std::path::PathBuf,
    pub is_dir: bool,
    pub children: Vec<BrowserNode>,
    pub expanded: bool,
}

impl BrowserNode {
    /// Recursively scan a directory, including only dirs that contain .wav descendants and .wav files.
    pub fn scan_root(root: &str) -> Vec<BrowserNode> {
        let root_path = std::path::Path::new(root);
        if !root_path.is_dir() {
            return Vec::new();
        }
        Self::scan_dir(root_path)
    }

    fn scan_dir(dir: &std::path::Path) -> Vec<BrowserNode> {
        let mut nodes = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return nodes,
        };

        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                let children = Self::scan_dir(&path);
                if !children.is_empty() {
                    nodes.push(BrowserNode {
                        name,
                        path,
                        is_dir: true,
                        children,
                        expanded: false,
                    });
                }
            } else if path
                .extension()
                .map(|ext| ext.to_ascii_lowercase() == "wav")
                .unwrap_or(false)
            {
                nodes.push(BrowserNode {
                    name,
                    path,
                    is_dir: false,
                    children: Vec::new(),
                    expanded: false,
                });
            }
        }
        nodes
    }

    /// Flatten the tree respecting expanded/collapsed state.
    pub fn flatten_visible(nodes: &[BrowserNode], depth: usize) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            result.push(BrowserEntry {
                depth,
                is_dir: node.is_dir,
                name: node.name.clone(),
                path: node.path.clone(),
                expanded: node.expanded,
            });
            if node.is_dir && node.expanded {
                result.extend(Self::flatten_visible(&node.children, depth + 1));
            }
        }
        result
    }

    /// Flatten with search filter — show matching .wav files with their ancestor context (auto-expanded).
    /// Matches against both file names and folder names. When a folder name matches,
    /// all its descendants are included.
    pub fn flatten_filtered(
        nodes: &[BrowserNode],
        filter_lower: &str,
        depth: usize,
    ) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            if node.is_dir {
                let dir_matches = node.name.to_lowercase().contains(filter_lower);
                let child_results = if dir_matches {
                    // Folder name matches — include all children
                    Self::flatten_all(&node.children, depth + 1)
                } else {
                    Self::flatten_filtered(&node.children, filter_lower, depth + 1)
                };
                if !child_results.is_empty() {
                    result.push(BrowserEntry {
                        depth,
                        is_dir: true,
                        name: node.name.clone(),
                        path: node.path.clone(),
                        expanded: true,
                    });
                    result.extend(child_results);
                }
            } else if node.name.to_lowercase().contains(filter_lower) {
                result.push(BrowserEntry {
                    depth,
                    is_dir: false,
                    name: node.name.clone(),
                    path: node.path.clone(),
                    expanded: false,
                });
            }
        }
        result
    }

    /// Flatten all descendants (used when a parent folder matches the filter).
    fn flatten_all(nodes: &[BrowserNode], depth: usize) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            result.push(BrowserEntry {
                depth,
                is_dir: node.is_dir,
                name: node.name.clone(),
                path: node.path.clone(),
                expanded: node.is_dir,
            });
            if node.is_dir {
                result.extend(Self::flatten_all(&node.children, depth + 1));
            }
        }
        result
    }

    /// Toggle expanded state for a node at a given path in the tree.
    pub fn toggle_expanded(nodes: &mut [BrowserNode], target_path: &std::path::Path) {
        for node in nodes.iter_mut() {
            if node.path == target_path && node.is_dir {
                node.expanded = !node.expanded;
                return;
            }
            if node.is_dir && node.expanded {
                Self::toggle_expanded(&mut node.children, target_path);
            }
        }
    }

    /// Set expanded state for a node.
    pub fn set_expanded(nodes: &mut [BrowserNode], target_path: &std::path::Path, expanded: bool) {
        for node in nodes.iter_mut() {
            if node.path == target_path && node.is_dir {
                node.expanded = expanded;
                return;
            }
            if node.is_dir {
                Self::set_expanded(&mut node.children, target_path, expanded);
            }
        }
    }

    /// Expand all ancestor directories of a target file path. Returns true if found.
    pub fn expand_to_file(nodes: &mut [BrowserNode], target_stem: &str) -> bool {
        for node in nodes.iter_mut() {
            if node.is_dir {
                if Self::expand_to_file(&mut node.children, target_stem) {
                    node.expanded = true;
                    return true;
                }
            } else {
                let stem = node.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem == target_stem {
                    return true;
                }
            }
        }
        false
    }
}

impl BrowserState {
    pub(super) fn visible_items(&self) -> Vec<BrowserEntry> {
        if self.filter.is_empty() {
            BrowserNode::flatten_visible(&self.tree, 0)
        } else {
            let filter_lower = self.filter.to_lowercase();
            BrowserNode::flatten_filtered(&self.tree, &filter_lower, 0)
        }
    }

    fn max_visible(&self, ui: &UiState) -> usize {
        let h = ui.layout.sidebar_inner.height as usize;
        if h > 1 {
            h - 1
        } else {
            1
        }
    }

    pub(super) fn sync_to_track(
        &mut self,
        tracks: &[String],
        cursor_track: usize,
        is_sampler_track: bool,
        ui: &UiState,
    ) {
        if tracks.is_empty() || !is_sampler_track {
            return;
        }
        let sample_name = &tracks[cursor_track];
        if sample_name.is_empty() {
            return;
        }

        self.filter.clear();
        BrowserNode::expand_to_file(&mut self.tree, sample_name);

        let items = self.visible_items();
        for (i, entry) in items.iter().enumerate() {
            if !entry.is_dir {
                let stem = entry
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if stem == sample_name {
                    self.cursor = i;
                    let max_visible = self.max_visible(ui);
                    self.scroll_offset = i.saturating_sub(max_visible / 2);
                    return;
                }
            }
        }
    }

    pub(super) fn handle_sidebar_input(app: &mut App, code: KeyCode) {
        if app.ui.sidebar_tab == SidebarTab::Tools {
            app.ui.params_column = 0;
            app.handle_track_params_column(code);
            return;
        }
        if app.ui.sidebar_tab == SidebarTab::Agent {
            if app.agent_panel.model_dropdown_open {
                match code {
                    KeyCode::Up => {
                        if app.agent_panel.model_dropdown_cursor > 0 {
                            app.agent_panel.model_dropdown_cursor -= 1;
                        }
                    }
                    KeyCode::Down => {
                        let max = app.agent_model_options().len();
                        if app.agent_panel.model_dropdown_cursor + 1 < max {
                            app.agent_panel.model_dropdown_cursor += 1;
                        }
                    }
                    KeyCode::Enter => {
                        app.select_agent_model_index(app.agent_panel.model_dropdown_cursor);
                        app.agent_panel.model_dropdown_open = false;
                    }
                    KeyCode::Esc => {
                        app.agent_panel.model_dropdown_open = false;
                    }
                    _ => {}
                }
                return;
            }
            match code {
                KeyCode::Char(c) => {
                    if app.agent_panel.pending_request.is_none() {
                        let cursor = app
                            .agent_panel
                            .input_cursor
                            .min(app.agent_panel.input_buffer.len());
                        app.agent_panel.input_buffer.insert(cursor, c);
                        app.agent_panel.input_cursor = cursor + 1;
                    }
                }
                KeyCode::Backspace => {
                    if app.agent_panel.pending_request.is_none() {
                        let cursor = app
                            .agent_panel
                            .input_cursor
                            .min(app.agent_panel.input_buffer.len());
                        if cursor > 0 {
                            app.agent_panel.input_buffer.remove(cursor - 1);
                            app.agent_panel.input_cursor = cursor - 1;
                        }
                    }
                }
                KeyCode::Left => {
                    if app.agent_panel.pending_request.is_none() {
                        app.agent_panel.input_cursor =
                            app.agent_panel.input_cursor.saturating_sub(1);
                    }
                }
                KeyCode::Right => {
                    if app.agent_panel.pending_request.is_none() {
                        app.agent_panel.input_cursor = (app.agent_panel.input_cursor + 1)
                            .min(app.agent_panel.input_buffer.len());
                    }
                }
                KeyCode::Home => app.agent_panel.input_cursor = 0,
                KeyCode::End => app.agent_panel.input_cursor = app.agent_panel.input_buffer.len(),
                KeyCode::Delete => {
                    if app.agent_panel.pending_request.is_none() {
                        let cursor = app
                            .agent_panel
                            .input_cursor
                            .min(app.agent_panel.input_buffer.len());
                        if cursor < app.agent_panel.input_buffer.len() {
                            app.agent_panel.input_buffer.remove(cursor);
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Err(error) = app.submit_agent_prompt() {
                        app.editor.status_message = Some((error, Instant::now()));
                    }
                }
                KeyCode::Esc => {
                    if app.agent_panel.pending_request.is_some() {
                        app.cancel_agent_request();
                    } else {
                        if !app.tracks.is_empty() {
                            app.ui.sidebar_tab = SidebarTab::Tools;
                        } else {
                            app.ui.sidebar_tab = SidebarTab::Sounds;
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Instrument picker mode: separate input handling
        if app.effective_sidebar_mode() == SidebarMode::InstrumentPicker {
            Self::handle_instrument_picker_input(app, code);
            return;
        }

        if app.effective_sidebar_mode() == SidebarMode::Presets {
            app.clamp_preset_browser();
            match code {
                KeyCode::Char(c) if app.ui.sidebar_search_focused => {
                    app.preset_browser.filter.push(c);
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                }
                KeyCode::Backspace if app.ui.sidebar_search_focused => {
                    app.preset_browser.filter.pop();
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                }
                KeyCode::Up => {
                    if app.preset_browser.cursor > 0 {
                        app.preset_browser.cursor -= 1;
                    }
                    app.clamp_preset_browser();
                }
                KeyCode::Down => {
                    let items = app.visible_preset_items();
                    if app.preset_browser.cursor + 1 < items.len() {
                        app.preset_browser.cursor += 1;
                    }
                    app.clamp_preset_browser();
                }
                KeyCode::Enter => {
                    app.load_selected_preset_into_track();
                    app.ui.sidebar_search_focused = false;
                }
                KeyCode::Esc => {
                    app.preset_browser.filter.clear();
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                    app.ui.sidebar_search_focused = false;
                    if !app.tracks.is_empty() {
                        app.ui.sidebar_tab = SidebarTab::Tools;
                    } else {
                        app.ui.focused_region = Region::Cirklon;
                    }
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char(c) if app.ui.sidebar_search_focused => {
                app.browser.filter.push(c);
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
            }
            KeyCode::Backspace if app.ui.sidebar_search_focused => {
                app.browser.filter.pop();
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
            }
            KeyCode::Up => {
                if app.browser.cursor > 0 {
                    app.browser.cursor -= 1;
                    if app.browser.cursor < app.browser.scroll_offset {
                        app.browser.scroll_offset = app.browser.cursor;
                    }
                }
            }
            KeyCode::Down => {
                let items = app.browser.visible_items();
                if app.browser.cursor + 1 < items.len() {
                    app.browser.cursor += 1;
                    let max_visible = app.browser.max_visible(&app.ui);
                    if app.browser.cursor >= app.browser.scroll_offset + max_visible {
                        app.browser.scroll_offset = app.browser.cursor + 1 - max_visible;
                    }
                }
            }
            KeyCode::Right => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    if item.is_dir && !item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut app.browser.tree, &path, true);
                    }
                }
            }
            KeyCode::Left => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    if item.is_dir && item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut app.browser.tree, &path, false);
                    }
                }
            }
            KeyCode::Enter => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    let path = item.path.clone();
                    if item.is_dir {
                        BrowserNode::toggle_expanded(&mut app.browser.tree, &path);
                    } else {
                        app.sidebar_select_file(&path);
                        app.ui.sidebar_search_focused = false;
                    }
                }
            }
            KeyCode::Esc => {
                app.browser.filter.clear();
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
                app.ui.sidebar_search_focused = false;
                if !app.tracks.is_empty() {
                    app.ui.sidebar_tab = SidebarTab::Tools;
                    app.ui.sidebar_mode = SidebarMode::Audition;
                } else {
                    app.ui.focused_region = Region::Cirklon;
                }
            }
            _ => {}
        }
    }

    fn handle_instrument_picker_input(app: &mut App, code: KeyCode) {
        use crate::sequencer::InstrumentType;
        match code {
            KeyCode::Up => {
                if app.ui.instrument_picker_cursor > 0 {
                    app.ui.instrument_picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if app.ui.instrument_picker_cursor + 1 < InstrumentType::COUNT {
                    app.ui.instrument_picker_cursor += 1;
                }
            }
            KeyCode::Enter => match InstrumentType::ALL[app.ui.instrument_picker_cursor] {
                InstrumentType::Sampler => {
                    app.browser.cursor = 0;
                    app.browser.filter.clear();
                    app.browser.scroll_offset = 0;
                    app.ui.sidebar_mode = SidebarMode::AddTrack;
                    app.ui.sidebar_search_focused = true;
                }
                InstrumentType::Custom => {
                    app.editor.picker_cursor = 0;
                    app.editor.picker_filter.clear();
                    app.editor.picker_items = crate::lisp_effect::list_saved_instruments();
                    app.ui.input_mode = super::InputMode::InstrumentPicker;
                }
            },
            KeyCode::Esc => {
                if !app.tracks.is_empty() {
                    app.ui.sidebar_tab = SidebarTab::Tools;
                    app.ui.sidebar_mode = SidebarMode::Audition;
                } else {
                    app.ui.focused_region = Region::Cirklon;
                }
            }
            _ => {}
        }
    }

    pub(super) fn scroll(&mut self, delta: isize, ui: &UiState) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let max_visible = self.max_visible(ui);
        let max_scroll = items.len().saturating_sub(max_visible);
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
        } else {
            self.scroll_offset = (self.scroll_offset + delta as usize).min(max_scroll);
        }
    }
}

impl App {
    pub(super) fn current_custom_instrument_name(&self) -> Option<&str> {
        if self.tracks.is_empty() || self.is_sampler_track(self.ui.cursor_track) {
            None
        } else if let Some(Some(engine_id)) = self.graph.track_engine_ids.get(self.ui.cursor_track)
        {
            self.editor
                .engine_registry
                .get(*engine_id)
                .map(|engine| engine.name.as_str())
        } else {
            self.tracks.get(self.ui.cursor_track).map(String::as_str)
        }
    }

    pub(super) fn visible_preset_items(&self) -> Vec<String> {
        let Some(name) = self.current_custom_instrument_name() else {
            return Vec::new();
        };
        let mut items: Vec<String> = lisp_effect::load_instrument_presets(name)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.name)
            .collect();
        items.sort();
        if self.preset_browser.filter.is_empty() {
            return items;
        }
        let filter = self.preset_browser.filter.to_lowercase();
        items.retain(|item| item.to_lowercase().contains(&filter));
        items
    }

    pub(super) fn current_preset_engine_name(&self) -> Option<&str> {
        self.current_custom_instrument_name()
    }

    fn current_track_sound_state(&self) -> crate::sequencer::TrackSoundState {
        self.state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get(self.ui.cursor_track)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn set_track_sound_state(
        &self,
        track: usize,
        engine_id: Option<usize>,
        loaded_preset: Option<String>,
        dirty: bool,
    ) {
        if let Some(meta) = self
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            meta.engine_id = engine_id;
            meta.loaded_preset = loaded_preset;
            meta.dirty = dirty;
        }
    }

    pub(super) fn mark_track_sound_dirty(&self, track: usize) {
        if let Some(meta) = self
            .state
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            meta.dirty = true;
        }
    }

    pub(super) fn clamp_preset_browser(&mut self) {
        let items = self.visible_preset_items();
        if items.is_empty() {
            self.preset_browser.cursor = 0;
            self.preset_browser.scroll_offset = 0;
            return;
        }
        self.preset_browser.cursor = self.preset_browser.cursor.min(items.len() - 1);
        let max_visible = self.preset_max_visible();
        let max_scroll = items.len().saturating_sub(max_visible);
        self.preset_browser.scroll_offset = self.preset_browser.scroll_offset.min(max_scroll);
        if self.preset_browser.cursor < self.preset_browser.scroll_offset {
            self.preset_browser.scroll_offset = self.preset_browser.cursor;
        } else if self.preset_browser.cursor >= self.preset_browser.scroll_offset + max_visible {
            self.preset_browser.scroll_offset = self.preset_browser.cursor + 1 - max_visible;
        }
    }

    pub(super) fn preset_max_visible(&self) -> usize {
        let h = self.ui.layout.sidebar_inner.height as usize;
        if h > 3 {
            h - 3
        } else {
            1
        }
    }

    fn selected_preset_name(&self) -> Option<String> {
        let items = self.visible_preset_items();
        items.get(self.preset_browser.cursor).cloned()
    }

    pub(super) fn load_selected_preset_into_track(&mut self) {
        let Some(instrument_name) = self.current_custom_instrument_name() else {
            return;
        };
        let Some(selected_name) = self.selected_preset_name() else {
            return;
        };
        let presets = match lisp_effect::load_instrument_presets(instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let Some(preset) = presets.into_iter().find(|p| p.name == selected_name) else {
            return;
        };
        let track = self.ui.cursor_track;
        let Some(desc) = self.current_instrument_descriptor() else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        for (idx, param) in desc.params.iter().enumerate() {
            let value = preset
                .params
                .get(&param.name)
                .copied()
                .unwrap_or(param.default);
            let clamped = param.clamp(value);
            slot.defaults.set(idx, clamped);
            self.send_instrument_param(track, idx, clamped);
        }
        self.state.pattern.instrument_base_note_offsets[track].store(
            preset.base_note_offset.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.state.schedule_mod_resync();
        self.state.publish_scheduler_snapshot();
        let engine_id = self.graph.track_engine_ids.get(track).and_then(|id| *id);
        self.set_track_sound_state(track, engine_id, Some(preset.name.clone()), false);
        self.editor.status_message =
            Some((format!("Loaded preset '{}'", preset.name), Instant::now()));
    }

    pub fn save_current_track_as_preset(&mut self, preset_name: &str, overwrite: bool) {
        let Some(instrument_name) = self.current_custom_instrument_name() else {
            return;
        };
        let track = self.ui.cursor_track;
        let Some(desc) = self.current_instrument_descriptor() else {
            return;
        };
        let slot = &self.state.pattern.instrument_slots[track];
        let mut params = std::collections::BTreeMap::new();
        for (idx, param) in desc.params.iter().enumerate() {
            params.insert(param.name.clone(), slot.defaults.get(idx));
        }
        let preset = lisp_effect::InstrumentPreset {
            id: preset_name.to_string(),
            name: preset_name.to_string(),
            base_note_offset: f32::from_bits(
                self.state.pattern.instrument_base_note_offsets[track]
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            params,
        };

        let mut presets = match lisp_effect::load_instrument_presets(instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };

        if let Some(existing_idx) = presets.iter().position(|p| p.name == preset_name) {
            if overwrite {
                presets[existing_idx] = preset;
            } else {
                self.editor.status_message = Some((
                    format!("Preset '{}' already exists", preset_name),
                    Instant::now(),
                ));
                return;
            }
        } else {
            presets.push(preset);
            presets.sort_by(|a, b| a.name.cmp(&b.name));
        }

        match lisp_effect::save_instrument_presets(instrument_name, &presets) {
            Ok(()) => {
                let engine_id = self.graph.track_engine_ids.get(track).and_then(|id| *id);
                self.set_track_sound_state(track, engine_id, Some(preset_name.to_string()), false);
                self.editor.status_message =
                    Some((format!("Saved preset '{}'", preset_name), Instant::now()));
                self.clamp_preset_browser();
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
            }
        }
    }

    pub fn overwrite_loaded_preset(&mut self) {
        let meta = self.current_track_sound_state();
        let Some(name) = meta.loaded_preset else {
            self.editor.status_message =
                Some(("No loaded preset to overwrite".to_string(), Instant::now()));
            return;
        };
        self.save_current_track_as_preset(&name, true);
    }

    pub(super) fn revert_loaded_preset(&mut self) {
        let meta = self.current_track_sound_state();
        if let Some(name) = meta.loaded_preset {
            let items = self.visible_preset_items();
            if let Some(idx) = items.iter().position(|item| item == &name) {
                self.preset_browser.cursor = idx;
            }
            self.load_selected_preset_into_track();
        } else {
            self.editor.status_message =
                Some(("No loaded preset to revert".to_string(), Instant::now()));
        }
    }

    pub(super) fn handle_preset_name_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.ui.value_buffer.push(c),
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
            }
            KeyCode::Enter => {
                let name = self.ui.value_buffer.trim().to_string();
                if !name.is_empty() {
                    match self.ui.preset_prompt_kind {
                        PresetPromptKind::SaveNew => {
                            self.save_current_track_as_preset(&name, false)
                        }
                    }
                }
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    /// Execute the sidebar action for a file selection (Enter or click).
    pub(super) fn sidebar_select_file(&mut self, path: &std::path::Path) {
        match self.effective_sidebar_mode() {
            SidebarMode::InstrumentPicker => return, // no file selection in picker
            SidebarMode::AddTrack => match self.graph_controller().add_track(path) {
                Ok(idx) => {
                    self.ui.cursor_track = idx;
                    self.editor.status_message =
                        Some((format!("Added track {}", idx + 1), Instant::now()));
                }
                Err(e) => {
                    self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
                }
            },
            SidebarMode::Audition => {
                if self.tracks.is_empty() || self.ui.cursor_track >= self.tracks.len() {
                    return;
                }
                match crate::sampler::load_wav_buffer(self.graph.lg.0, path) {
                    Ok(loaded) => {
                        self.submit_sample_analysis(&loaded);
                        let new_buffer_id = loaded.buffer_id;
                        let sample_rate = loaded.sample_rate;
                        let new_name = loaded.name;
                        let track = self.ui.cursor_track;
                        self.graph_controller().send_sample_to_all_voices(
                            track,
                            new_buffer_id,
                            sample_rate,
                        );
                        self.graph.track_buffer_ids[track] = new_buffer_id;
                        self.graph.track_sample_rates[track] = sample_rate;
                        self.tracks[track] = new_name.clone();
                        self.register_loaded_sample_path(
                            &new_name,
                            new_buffer_id,
                            path.to_path_buf(),
                        );
                        if track < self.sampler_paths.len() {
                            self.sampler_paths[track] = Some(path.to_path_buf());
                        }
                        self.reset_sampler_bpm_for_analysis(track);
                        self.publish_sampler_analysis_runtime(track);
                        self.editor.status_message =
                            Some((format!("Swapped: {}", new_name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
            SidebarMode::Presets => {}
        }
    }
}

// ── Drawing ──

pub(super) fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.ui.focused_region == Region::Sidebar;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(region_border_style(app, Region::Sidebar));
    let block_inner = block.inner(area);
    frame.render_widget(block, area);

    if block_inner.height < 2 || block_inner.width < 4 {
        app.ui.layout.sidebar_tabs = block_inner;
        app.ui.layout.sidebar_inner = Rect::new(block_inner.x, block_inner.y, block_inner.width, 0);
        return;
    }

    let tabs_area = Rect::new(block_inner.x, block_inner.y, block_inner.width, 1);
    let inner = Rect::new(
        block_inner.x,
        block_inner.y + 1,
        block_inner.width,
        block_inner.height.saturating_sub(1),
    );

    app.ui.layout.sidebar_tabs = tabs_area;
    app.ui.layout.sidebar_inner = inner;

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let tools_selected = app.ui.sidebar_tab == SidebarTab::Tools;
    let agent_selected = app.ui.sidebar_tab == SidebarTab::Agent;
    let sounds_selected = app.ui.sidebar_tab == SidebarTab::Sounds;
    let tools_style = if tools_selected && focused {
        Style::default().fg(Color::Black).bg(Color::White).bold()
    } else if tools_selected {
        Style::default().fg(Color::Black).bg(Color::Rgb(90, 90, 90))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let agent_style = if agent_selected && focused {
        Style::default().fg(Color::Black).bg(Color::White).bold()
    } else if agent_selected {
        Style::default().fg(Color::Black).bg(Color::Rgb(90, 90, 90))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let sounds_style = if sounds_selected && focused {
        Style::default().fg(Color::Black).bg(Color::White).bold()
    } else if sounds_selected {
        Style::default().fg(Color::Black).bg(Color::Rgb(90, 90, 90))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Tools ", tools_style),
            Span::raw(" "),
            Span::styled(" Agent ", agent_style),
            Span::raw(" "),
            Span::styled(" Sounds ", sounds_style),
        ])),
        tabs_area,
    );

    if app.ui.sidebar_tab == SidebarTab::Tools {
        draw_track_params_column(frame, app, inner, focused);
        return;
    }

    if app.ui.sidebar_tab == SidebarTab::Agent {
        draw_agent_sidebar(frame, app, inner, focused);
        return;
    }

    // Clear the entire inner area first to prevent stale content
    let buf = frame.buffer_mut();
    for y in inner.y..(inner.y + inner.height) {
        for x in inner.x..(inner.x + inner.width) {
            buf[(x, y)].reset();
        }
    }

    // Instrument picker mode: draw simple list instead of browser
    if app.effective_sidebar_mode() == SidebarMode::InstrumentPicker && focused {
        for (i, inst) in crate::sequencer::InstrumentType::ALL.iter().enumerate() {
            let label = inst.label();
            if i as u16 >= inner.height {
                break;
            }
            let is_cursor = i == app.ui.instrument_picker_cursor;
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            let text = format!("  {} ", label);
            let buf = frame.buffer_mut();
            buf.set_string(inner.x, inner.y + i as u16, &text, style);
            let text_width = UnicodeWidthStr::width(text.as_str());
            let remaining = (inner.width as usize).saturating_sub(text_width);
            if remaining > 0 {
                buf.set_string(
                    inner.x + text_width as u16,
                    inner.y + i as u16,
                    &" ".repeat(remaining),
                    style,
                );
            }
        }
        return;
    }

    if app.effective_sidebar_mode() == SidebarMode::Presets {
        app.clamp_preset_browser();
        let items = app.visible_preset_items();
        let max_visible = (inner.height as usize).saturating_sub(3);
        let meta = app.current_track_sound_state();
        let engine_name = app.current_preset_engine_name().unwrap_or("None");
        let engine_header = format!(" engine: {}", engine_name);
        let loaded = meta
            .loaded_preset
            .clone()
            .unwrap_or_else(|| "None".to_string());
        let header = format!(" preset: {}{}", loaded, if meta.dirty { " *" } else { "" });
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                engine_header,
                Style::default().fg(Color::Cyan),
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                header,
                Style::default().fg(Color::White),
            ))),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        let filter_text = if focused {
            format!("> {}\u{2588}", app.preset_browser.filter)
        } else {
            format!("> {}", app.preset_browser.filter)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                filter_text,
                Style::default().fg(Color::White),
            ))),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
        let list_start_y = inner.y + 3;
        let scroll = app.preset_browser.scroll_offset;
        for (vi, i) in (scroll..items.len()).enumerate() {
            if vi >= max_visible {
                break;
            }
            let row_y = list_start_y + vi as u16;
            if row_y >= inner.y + inner.height {
                break;
            }
            let item = &items[i];
            let is_cursor = focused && i == app.preset_browser.cursor;
            let is_loaded = meta
                .loaded_preset
                .as_ref()
                .map(|p| p == item)
                .unwrap_or(false);
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else if is_loaded {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            let text = format!("  {}", item);
            let buf = frame.buffer_mut();
            buf.set_string(inner.x, row_y, &text, style);
            let text_width = UnicodeWidthStr::width(text.as_str());
            let remaining = (inner.width as usize).saturating_sub(text_width);
            if remaining > 0 {
                buf.set_string(
                    inner.x + text_width as u16,
                    row_y,
                    &" ".repeat(remaining),
                    style,
                );
            }
        }
        return;
    }

    let items = app.browser.visible_items();
    let max_visible = (inner.height as usize).saturating_sub(1); // 1 row for filter

    let filter_text = if app.ui.sidebar_search_focused {
        format!("> {}\u{2588}", app.browser.filter)
    } else {
        format!("> {}", app.browser.filter)
    };
    let filter_line = Line::from(Span::styled(filter_text, Style::default().fg(Color::White)));
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    let list_start_y = inner.y + 1;
    let list_max = max_visible;
    let scroll = app.browser.scroll_offset;

    for (vi, i) in (scroll..items.len()).enumerate() {
        if vi >= list_max {
            break;
        }
        let row_y = list_start_y + vi as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let entry = &items[i];
        let is_cursor = focused && i == app.browser.cursor;
        let is_current_sample = !entry.is_dir
            && !app.tracks.is_empty()
            && entry
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == app.tracks[app.ui.cursor_track])
                .unwrap_or(false);

        let indent = "  ".repeat(entry.depth);
        let icon = if entry.is_dir {
            if entry.expanded {
                "\u{25bc} "
            } else {
                "\u{25b6} "
            }
        } else {
            "  "
        };

        let prefix_width = UnicodeWidthStr::width(indent.as_str()) + UnicodeWidthStr::width(icon);
        let max_name_width = (inner.width as usize).saturating_sub(prefix_width);
        // Truncate name by display width
        let mut truncated = String::new();
        let mut w = 0;
        for ch in entry.name.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w + cw > max_name_width {
                break;
            }
            truncated.push(ch);
            w += cw;
        }
        let text = format!("{}{}{}", indent, icon, truncated);

        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if is_current_sample {
            Style::default().fg(Color::Yellow)
        } else if entry.is_dir {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        // Write directly to the buffer for guaranteed cell coverage
        let buf = frame.buffer_mut();
        buf.set_string(inner.x, row_y, &text, style);
        // Fill remaining cells with spaces in the same style
        let text_width = UnicodeWidthStr::width(text.as_str());
        let remaining = (inner.width as usize).saturating_sub(text_width);
        if remaining > 0 {
            buf.set_string(
                inner.x + text_width as u16,
                row_y,
                &" ".repeat(remaining),
                style,
            );
        }
    }
}

fn draw_agent_sidebar(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let provider_state = &app.agent_panel.provider_state;
    let selected_provider = provider_state
        .providers
        .iter()
        .find(|entry| entry.provider == provider_state.selected_provider);

    let model_label = selected_provider
        .map(|provider| provider.selected_model.as_str())
        .unwrap_or("unavailable");
    let focus_mark = if focused { " <" } else { "" };
    let pending = app.agent_panel.pending_request.is_some();
    let elapsed = app
        .agent_panel
        .pending_request
        .as_ref()
        .map(|pending| pending.started_at.elapsed())
        .unwrap_or_default();
    let input_lines = build_agent_input_lines(app, focused, pending);
    let input_height = input_lines.len().max(1) as u16;
    let header_height = if selected_provider
        .map(|provider| !provider.api_key_present)
        .unwrap_or(app.agent_panel.load_error.is_some())
    {
        2
    } else {
        1
    };
    let status_height = if pending { 1 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(input_height),
        ])
        .split(area);

    let mut header_lines = vec![Line::from(Span::styled(
        format!("model: {model_label}{focus_mark}"),
        Style::default().fg(Color::Cyan),
    ))];
    if let Some(provider) = selected_provider {
        if !provider.api_key_present {
            header_lines.push(Line::from(Span::styled(
                format!("error: missing {}", provider.provider.api_key_env()),
                Style::default().fg(Color::LightRed),
            )));
        }
    } else if let Some(error) = app.agent_panel.load_error.as_ref() {
        header_lines.push(Line::from(Span::styled(
            format!("error: {error}"),
            Style::default().fg(Color::LightRed),
        )));
    }
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    draw_agent_model_dropdown(frame, app, chunks[0]);

    let transcript_lines = build_agent_transcript_lines(app);
    let visible_height = chunks[1].height as usize;
    let total_lines = transcript_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = app.agent_panel.scroll_offset.min(max_scroll);
    let start = total_lines.saturating_sub(visible_height + scroll);
    let end = total_lines.saturating_sub(scroll);
    let visible = if start < end {
        transcript_lines[start..end].to_vec()
    } else {
        Vec::new()
    };
    frame.render_widget(
        Paragraph::new(visible).wrap(Wrap { trim: false }),
        chunks[1],
    );

    if pending {
        frame.render_widget(Paragraph::new(build_agent_status_line(elapsed)), chunks[2]);
    }

    frame.render_widget(Paragraph::new(input_lines), chunks[3]);
}

fn build_agent_transcript_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let wrap_width = app.ui.layout.sidebar_inner.width.saturating_sub(2).max(8) as usize;
    for entry in &app.agent_panel.transcript {
        let (label_style, text_style) = match entry.role.as_str() {
            "user" => (
                Style::default().fg(Color::Cyan),
                Style::default().fg(Color::White),
            ),
            "assistant" => (
                Style::default().fg(Color::Yellow),
                Style::default().fg(Color::Gray),
            ),
            "tool" => (
                Style::default().fg(Color::LightBlue),
                Style::default().fg(Color::Gray),
            ),
            "error" => (
                Style::default().fg(Color::LightRed),
                Style::default().fg(Color::LightRed),
            ),
            _ => (
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::Gray),
            ),
        };
        lines.push(Line::from(Span::styled(
            format!("{}:", entry.role),
            label_style.bold(),
        )));
        for part in entry.text.lines() {
            for wrapped in wrap_agent_text(part, wrap_width) {
                lines.push(Line::from(Span::styled(wrapped, text_style)));
            }
        }
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Describe a synth or effect and press Enter.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn wrap_agent_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            if word.len() <= width {
                current.push_str(word);
            } else {
                for chunk in word.as_bytes().chunks(width.max(1)) {
                    out.push(String::from_utf8_lossy(chunk).to_string());
                }
            }
            continue;
        }

        if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(current);
            if word.len() <= width {
                current = word.to_string();
            } else {
                current = String::new();
                for chunk in word.as_bytes().chunks(width.max(1)) {
                    out.push(String::from_utf8_lossy(chunk).to_string());
                }
            }
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn build_agent_input_lines(app: &App, focused: bool, pending: bool) -> Vec<Line<'static>> {
    let wrap_width = app.ui.layout.sidebar_inner.width.saturating_sub(4).max(8) as usize;
    let text = app.agent_panel.input_buffer.clone();

    let wrapped = if text.is_empty() {
        vec![String::new()]
    } else {
        wrap_agent_text(&text, wrap_width)
    };

    let mut rendered = Vec::new();
    let mut absolute_offset = 0usize;
    let cursor_offset = app
        .agent_panel
        .input_cursor
        .min(app.agent_panel.input_buffer.len());

    for (idx, line) in wrapped.into_iter().enumerate() {
        let prefix = if idx == 0 { "> " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(Color::White))];
        let line_len = line.len();

        if focused
            && !pending
            && cursor_offset >= absolute_offset
            && cursor_offset <= absolute_offset + line_len
        {
            let local_cursor = cursor_offset.saturating_sub(absolute_offset);
            let before = &line[..local_cursor.min(line.len())];
            let cursor_char = line
                .get(local_cursor..)
                .and_then(|rest| rest.chars().next())
                .map(|ch| ch.to_string())
                .unwrap_or_else(|| " ".to_string());
            let after_start = local_cursor.saturating_add(cursor_char.len());
            let after = if after_start <= line.len() {
                &line[after_start..]
            } else {
                ""
            };

            if !before.is_empty() {
                spans.push(Span::styled(
                    before.to_string(),
                    Style::default().fg(Color::White),
                ));
            }
            spans.push(Span::styled(
                cursor_char,
                Style::default().fg(Color::Black).bg(Color::White),
            ));
            if !after.is_empty() {
                spans.push(Span::styled(
                    after.to_string(),
                    Style::default().fg(Color::White),
                ));
            }
        } else {
            spans.push(Span::styled(line, Style::default().fg(Color::White)));
        }

        rendered.push(Line::from(spans));
        absolute_offset += line_len;
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::styled(
            "> ",
            Style::default().fg(Color::White),
        )));
    }

    rendered
}

fn build_agent_status_line(elapsed: std::time::Duration) -> Line<'static> {
    let label = "Working";
    let palette = [
        Color::Rgb(120, 208, 210),
        Color::Rgb(132, 220, 222),
        Color::Rgb(146, 232, 234),
        Color::Rgb(132, 220, 222),
    ];
    let shift = ((elapsed.as_millis() / 140) as usize) % palette.len();
    let mut spans = Vec::new();
    for (idx, ch) in label.chars().enumerate() {
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(palette[(idx + shift) % palette.len()]),
        ));
    }
    spans.push(Span::styled(
        format!(" ({:>2}s • esc to interrupt)", elapsed.as_secs()),
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn draw_agent_model_dropdown(frame: &mut Frame, app: &App, model_row: Rect) {
    if !app.agent_panel.model_dropdown_open {
        return;
    }

    let options = app.agent_model_options();
    if options.is_empty() {
        return;
    }

    let dropdown_x = model_row.x + 7;
    let dropdown_width = model_row.width.saturating_sub(7).min(28).max(16);
    let selected = app.agent_panel.model_dropdown_cursor;
    let visible_count = options
        .len()
        .min(app.ui.layout.sidebar_inner.height.saturating_sub(1) as usize);
    let scroll = selected.saturating_sub(visible_count.saturating_sub(1));
    if visible_count == 0 {
        return;
    }

    for vi in 0..visible_count {
        let idx = scroll + vi;
        if idx >= options.len() {
            break;
        }
        let y = model_row.y + 1 + vi as u16;
        let is_cursor = idx == selected;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
        };
        let text = format!(
            " {:<width$}",
            options[idx].1,
            width = dropdown_width.saturating_sub(2) as usize
        );
        frame.render_widget(Clear, Rect::new(dropdown_x, y, dropdown_width, 1));
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(dropdown_x, y, dropdown_width, 1),
        );
    }
}
