use std::time::Instant;
use super::App;
use crate::lisp_host;
use crate::sequencer::InstrumentType;

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
            lisp_host::load_instrument_preset_names(name).unwrap_or_default()
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

        // The name check runs against the merged list (factory ∪ user), but the
        // mutation runs against the writable user bank only: for a factory
        // instrument the user bank is an overlay, and saving the merged list
        // would copy every factory preset into it.
        let name_taken = lisp_host::load_instrument_preset_names(&instrument_name)
            .unwrap_or_default()
            .iter()
            .any(|name| name == preset_name);
        if name_taken && !overwrite {
            self.editor.status_message = Some((
                format!("Preset '{}' already exists", preset_name),
                Instant::now(),
            ));
            return Err(format!("Preset '{preset_name}' already exists"));
        }
        let mut presets = match lisp_host::load_user_instrument_presets(&instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return Err(e.to_string());
            }
        };
        if let Some(existing_idx) = presets.iter().position(|p| p.name == preset_name) {
            presets[existing_idx] = preset;
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
