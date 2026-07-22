use std::time::Instant;
use super::{App, BrowserState};
use crate::lisp_host;
use crate::sequencer::InstrumentType;

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
        let mut items = if self.graph.track_instrument_types.get(self.ui.cursor_track)
            == Some(&InstrumentType::Rack)
        {
            crate::project::list_rack_presets().unwrap_or_default()
        } else {
            let Some(name) = self.current_custom_instrument_name() else {
                return Vec::new();
            };
            lisp_host::load_instrument_presets(name)
                .unwrap_or_default()
                .into_iter()
                .map(|preset| preset.name)
                .collect::<Vec<_>>()
        };
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



    fn selected_preset_name(&self) -> Option<String> {
        let items = self.visible_preset_items();
        items.get(self.preset_browser.cursor).cloned()
    }


    pub fn save_current_track_as_preset(
        &mut self,
        preset_name: &str,
        overwrite: bool,
    ) -> Result<(), String> {
        let track = self.ui.cursor_track;
        if self.graph.track_instrument_types.get(track) == Some(&InstrumentType::Rack) {
            return match self.save_rack_preset(track, preset_name, overwrite) {
                Ok(_) => {
                    self.editor.status_message =
                        Some((format!("Saved preset '{}'", preset_name), Instant::now()));
                    Ok(())
                }
                Err(error) => {
                    self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
                    Err(error)
                }
            };
        }
        let Some(instrument_name) = self.current_custom_instrument_name() else {
            return Err("Current track does not support instrument presets".to_string());
        };
        let instrument_name = instrument_name.to_string();
        let Some(desc) = self.current_instrument_descriptor() else {
            return Err("Instrument descriptor unavailable".to_string());
        };
        let desc = desc.clone();
        let slot = &self.state.pattern.instrument_slots[track];
        let mut params = std::collections::BTreeMap::new();
        for (idx, param) in desc.params.iter().enumerate() {
            params.insert(param.name.clone(), slot.defaults.get(idx));
        }
        let preset = lisp_host::InstrumentPreset {
            id: preset_name.to_string(),
            name: preset_name.to_string(),
            base_note_offset: f32::from_bits(
                self.state.pattern.instrument_base_note_offsets[track]
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            params,
            key_locks: crate::effects::capture_key_locks_by_param_name(slot, &desc),
        };

        let mut presets = match lisp_host::load_instrument_presets(&instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return Err(e.to_string());
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
                return Err(format!("Preset '{preset_name}' already exists"));
            }
        } else {
            presets.push(preset);
            presets.sort_by(|a, b| a.name.cmp(&b.name));
        }

        match lisp_host::save_instrument_presets(&instrument_name, &presets) {
            Ok(()) => {
                let engine_id = self.graph.track_engine_ids.get(track).and_then(|id| *id);
                self.set_track_sound_state(track, engine_id, Some(preset_name.to_string()), false);
                self.editor.status_message =
                    Some((format!("Saved preset '{}'", preset_name), Instant::now()));
                Ok(())
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                Err(e.to_string())
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
        let _ = self.save_current_track_as_preset(&name, true);
    }



}
