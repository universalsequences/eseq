use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crossterm::event::KeyCode;

use crate::effects::BUILTIN_SLOT_COUNT;
use crate::project::{
    self, chord_snapshot_from_steps, project_file_version, ProjectFile, ProjectPattern,
    ProjectReverbState, ProjectScratchState, ProjectTrack,
};
use crate::sequencer::{InstrumentType, PatternSnapshot, MAX_STEPS};

use super::{App, InputMode, Region, SidebarMode, SidebarTab};

impl App {
    pub fn save_project_with_name(
        &mut self,
        requested_name: Option<&str>,
    ) -> Result<String, String> {
        let requested_name = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| self.current_project_name.clone())
            .ok_or_else(|| "Project name must contain letters or numbers".to_string())?;

        let save_name = project::sanitize_project_name(&requested_name);
        if save_name.is_empty() {
            return Err("Project name must contain letters or numbers".to_string());
        }

        self.save_project_named(&save_name)?;
        self.current_project_name = Some(save_name.clone());
        Ok(save_name)
    }

    pub fn queue_project_load_named(&mut self, name: &str) -> Result<(), String> {
        eprintln!("project-load: queue requested name={name}");
        let project = project::load_project(name).map_err(|error| error.to_string())?;
        eprintln!(
            "project-load: file loaded name={} version={} tracks={} patterns={} custom_effect_tracks={}",
            name,
            project.version,
            project.tracks.len(),
            project.patterns.len(),
            project.custom_effects.len()
        );
        if project.version != project::project_file_version() {
            return Err(format!("Unsupported project version {}", project.version));
        }

        self.editor.pending_project_load = Some(super::PendingProjectLoad {
            name: name.to_string(),
            tick: 0,
            project,
            built_patterns: Vec::new(),
            fallback_samples: 0,
            phase: super::PendingProjectLoadPhase::ClearExisting,
        });
        Ok(())
    }

    pub fn has_pending_project_load(&self) -> bool {
        self.editor.pending_project_load.is_some()
    }

    pub fn advance_pending_project_load(&mut self) -> Result<(), String> {
        let result = self.advance_project_load();
        if result.is_err() {
            self.editor.pending_project_load = None;
        }
        result
    }

    pub(super) fn open_project_name_prompt(&mut self) {
        self.ui.value_buffer = self.current_project_name.clone().unwrap_or_default();
        self.ui.input_mode = InputMode::ProjectNameEntry;
    }

    pub(super) fn open_project_picker(&mut self) {
        match project::list_project_names() {
            Ok(items) => {
                self.editor.picker_items = items;
                self.editor.picker_cursor = 0;
                self.editor.picker_filter.clear();
                self.ui.input_mode = InputMode::ProjectPicker;
            }
            Err(error) => {
                self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
            }
        }
    }

    pub(super) fn handle_project_name_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.ui.value_buffer.push(c),
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
            }
            KeyCode::Enter => {
                let requested_name = self.ui.value_buffer.trim().to_string();
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
                if requested_name.is_empty() {
                    return;
                }
                match self.save_project_with_name(Some(&requested_name)) {
                    Ok(save_name) => {
                        self.editor.status_message =
                            Some((format!("Saved project '{}'", save_name), Instant::now()));
                    }
                    Err(error) => {
                        self.editor.status_message =
                            Some((format!("Error: {error}"), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn handle_project_picker(&mut self, code: KeyCode) {
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
                if self.editor.picker_cursor + 1 < self.filtered_project_items().len() {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let Some(name) = self
                    .filtered_project_items()
                    .get(self.editor.picker_cursor)
                    .cloned()
                else {
                    return;
                };
                self.ui.input_mode = InputMode::Normal;
                match self.queue_project_load_named(&name) {
                    Ok(()) => {}
                    Err(error) => {
                        self.editor.status_message =
                            Some((format!("Error: {error}"), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                self.editor.picker_filter.clear();
                self.editor.picker_cursor = 0;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn filtered_project_items(&self) -> Vec<String> {
        if self.editor.picker_filter.is_empty() {
            return self.editor.picker_items.clone();
        }
        let filter = self.editor.picker_filter.to_lowercase();
        self.editor
            .picker_items
            .iter()
            .filter(|item| item.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    }

    pub(super) fn save_project_named(&mut self, project_name: &str) -> Result<(), String> {
        let project = self.capture_project(project_name)?;
        project::save_project(project_name, &project).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn capture_project(&mut self, project_name: &str) -> Result<ProjectFile, String> {
        let num_tracks = self.tracks.len();
        let current_pattern = self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;

        {
            let mut bank = self.state.pattern.pattern_bank.lock().unwrap();
            if current_pattern < bank.len() {
                bank[current_pattern] = PatternSnapshot::capture(
                    &self.state,
                    num_tracks,
                    &self.graph.track_buffer_ids,
                    &self.tracks,
                    &self.graph.track_instrument_types,
                );
            }
        }

        let bank = self.state.pattern.pattern_bank.lock().unwrap().clone();
        let tracks = self.capture_project_tracks()?;
        let custom_effects = self.capture_custom_effects();
        let patterns = bank
            .iter()
            .enumerate()
            .map(|(pattern_idx, snapshot)| {
                let mut sample_paths = Vec::with_capacity(num_tracks);
                let mut sample_names = Vec::with_capacity(num_tracks);
                for track_idx in 0..num_tracks {
                    let sample_name = snapshot
                        .sample_ids
                        .get(track_idx)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_default();
                    let sample_path = if snapshot
                        .instrument_types
                        .get(track_idx)
                        .copied()
                        .unwrap_or(InstrumentType::Sampler)
                        == InstrumentType::Sampler
                        && !sample_name.is_empty()
                    {
                        self.resolve_sample_path_for_snapshot(pattern_idx, track_idx, &sample_name)?
                            .map(|path| path.to_string_lossy().to_string())
                    } else {
                        None
                    };
                    sample_paths.push(sample_path);
                    sample_names.push(sample_name);
                }
                Ok(ProjectPattern::from_snapshot(
                    snapshot,
                    sample_paths,
                    sample_names,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(ProjectFile {
            version: project_file_version(),
            name: project_name.to_string(),
            bpm: self.state.transport.bpm.load(Ordering::Relaxed),
            master_volume: f32::from_bits(
                self.state.transport.master_volume.load(Ordering::Relaxed),
            ),
            current_pattern,
            reverb: ProjectReverbState {
                size: self.ui.reverb_size,
                brightness: self.ui.reverb_brightness,
                replace: self.ui.reverb_replace,
            },
            tracks,
            custom_effects,
            scratch: ProjectScratchState {
                buffer: self.editor.scratch_buffer.clone(),
                cursor_row: self.editor.scratch_cursor.0,
                cursor_col: self.editor.scratch_cursor.1,
            },
            patterns,
        })
    }

    fn capture_project_tracks(&self) -> Result<Vec<ProjectTrack>, String> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(track_idx, name)| {
                if self.is_sampler_track(track_idx) {
                    let path = self
                        .sampler_paths
                        .get(track_idx)
                        .and_then(|path| path.clone())
                        .or_else(|| self.sample_path_registry.get(name).cloned())
                        .or_else(|| self.resolve_sample_path_by_name(name));
                    let Some(path) = path else {
                        return Err(format!("Couldn't resolve sample path for '{}'", name));
                    };
                    Ok(ProjectTrack::Sampler {
                        sample_path: path.to_string_lossy().to_string(),
                    })
                } else {
                    Ok(ProjectTrack::Custom {
                        instrument_name: name.clone(),
                    })
                }
            })
            .collect()
    }

    fn capture_custom_effects(&self) -> Vec<Vec<Option<String>>> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(track_idx, _)| {
                (BUILTIN_SLOT_COUNT..self.graph.effect_descriptors[track_idx].len())
                    .map(|slot_idx| {
                        let slot = &self.state.pattern.effect_chains[track_idx][slot_idx];
                        if slot.node_id.load(Ordering::Relaxed) == 0 {
                            None
                        } else {
                            let name = self.graph.effect_descriptors[track_idx][slot_idx]
                                .name
                                .trim()
                                .to_string();
                            if name.is_empty() {
                                None
                            } else {
                                Some(name)
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn resolve_sample_path_for_snapshot(
        &self,
        pattern_idx: usize,
        track_idx: usize,
        sample_name: &str,
    ) -> Result<Option<PathBuf>, String> {
        if self.state.pattern.current_pattern.load(Ordering::Relaxed) as usize == pattern_idx {
            if let Some(path) = self
                .sampler_paths
                .get(track_idx)
                .and_then(|path| path.clone())
            {
                return Ok(Some(path));
            }
        }
        if let Some(path) = self.sample_path_registry.get(sample_name) {
            return Ok(Some(path.clone()));
        }
        let resolved = self.resolve_sample_path_by_name(sample_name);
        if resolved.is_none() {
            return Err(format!(
                "Couldn't resolve sample path for '{}'",
                sample_name
            ));
        }
        Ok(resolved)
    }

    fn resolve_sample_path_by_name(&self, sample_name: &str) -> Option<PathBuf> {
        fn walk(dir: &Path, sample_name: &str) -> Option<PathBuf> {
            let entries = std::fs::read_dir(dir).ok()?;
            for entry in entries {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_dir() {
                    if let Some(found) = walk(&path, sample_name) {
                        return Some(found);
                    }
                    continue;
                }
                let stem = path.file_stem().and_then(|stem| stem.to_str())?;
                if stem == sample_name {
                    return Some(path);
                }
            }
            None
        }

        walk(Path::new("samples"), sample_name)
    }

    pub(super) fn advance_project_load(&mut self) -> Result<(), String> {
        let Some(mut pending) = self.editor.pending_project_load.take() else {
            return Ok(());
        };
        pending.tick += 1;

        match pending.phase {
            super::PendingProjectLoadPhase::ClearExisting => {
                eprintln!(
                    "project-load: tick={} phase=ClearExisting existing_tracks={}",
                    pending.tick,
                    self.tracks.len()
                );
                self.graph_controller().clear_all_tracks();
                pending.phase = super::PendingProjectLoadPhase::AddTrack(0);
            }
            super::PendingProjectLoadPhase::AddTrack(track_idx) => {
                eprintln!(
                    "project-load: tick={} phase=AddTrack index={} total={}",
                    pending.tick,
                    track_idx,
                    pending.project.tracks.len()
                );
                if track_idx >= pending.project.tracks.len() {
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx: 0,
                        offset: 0,
                    };
                } else {
                    match &pending.project.tracks[track_idx] {
                        ProjectTrack::Sampler { sample_path } => {
                            eprintln!(
                                "project-load: add sampler track index={} path={}",
                                track_idx,
                                sample_path
                            );
                            self.graph_controller()
                                .add_track(Path::new(sample_path))
                                .map_err(|error| {
                                    format!("Failed to load sample '{}': {error}", sample_path)
                                })?;
                        }
                        ProjectTrack::Custom { instrument_name } => {
                            eprintln!(
                                "project-load: add custom track index={} instrument={}",
                                track_idx,
                                instrument_name
                            );
                            self.add_saved_instrument_track_sync(instrument_name)?;
                        }
                    }
                    pending.phase = super::PendingProjectLoadPhase::AddTrack(track_idx + 1);
                }
            }
            super::PendingProjectLoadPhase::AddEffect { track_idx, offset } => {
                eprintln!(
                    "project-load: tick={} phase=AddEffect track={} offset={}",
                    pending.tick,
                    track_idx,
                    offset
                );
                if track_idx >= pending.project.custom_effects.len() {
                    pending.phase = super::PendingProjectLoadPhase::BuildPattern(0);
                } else if offset >= pending.project.custom_effects[track_idx].len() {
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx: track_idx + 1,
                        offset: 0,
                    };
                } else {
                    if let Some(effect_name) = pending.project.custom_effects[track_idx][offset]
                        .as_ref()
                        .map(|name| name.trim())
                        .filter(|name| !name.is_empty())
                    {
                        eprintln!(
                            "project-load: load effect track={} slot={} name={}",
                            track_idx,
                            BUILTIN_SLOT_COUNT + offset,
                            effect_name
                        );
                        self.load_saved_effect_to_slot_sync(
                            track_idx,
                            BUILTIN_SLOT_COUNT + offset,
                            effect_name,
                        )?;
                    }
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx,
                        offset: offset + 1,
                    };
                }
            }
            super::PendingProjectLoadPhase::BuildPattern(pattern_idx) => {
                eprintln!(
                    "project-load: tick={} phase=BuildPattern index={} total={}",
                    pending.tick,
                    pattern_idx,
                    pending.project.patterns.len()
                );
                if pattern_idx >= pending.project.patterns.len() {
                    pending.phase = super::PendingProjectLoadPhase::Finalize;
                } else {
                    let (snapshot, fallback_count) = self.project_pattern_into_snapshot(
                        pending.project.patterns[pattern_idx].clone(),
                    )?;
                    pending.built_patterns.push(snapshot);
                    pending.fallback_samples += fallback_count;
                    pending.phase = super::PendingProjectLoadPhase::BuildPattern(pattern_idx + 1);
                }
            }
            super::PendingProjectLoadPhase::Finalize => {
                eprintln!(
                    "project-load: tick={} phase=Finalize built_patterns={} fallback_samples={}",
                    pending.tick,
                    pending.built_patterns.len(),
                    pending.fallback_samples
                );
                self.finish_project_load(pending)?;
                return Ok(());
            }
        }

        self.editor.pending_project_load = Some(pending);
        Ok(())
    }

    fn finish_project_load(&mut self, pending: super::PendingProjectLoad) -> Result<(), String> {
        eprintln!(
            "project-load: finish start name={} tracks={} built_patterns={} fallback_samples={}",
            pending.name,
            self.tracks.len(),
            pending.built_patterns.len(),
            pending.fallback_samples
        );
        let ProjectFile {
            version: _,
            name: _,
            bpm,
            master_volume,
            current_pattern: saved_current_pattern,
            reverb,
            scratch,
            tracks: _,
            custom_effects: _,
            patterns: _,
        } = pending.project;
        let bank = pending.built_patterns;
        let current_pattern = saved_current_pattern.min(bank.len().saturating_sub(1));

        {
            let mut pattern_bank = self.state.pattern.pattern_bank.lock().unwrap();
            *pattern_bank = if bank.is_empty() {
                vec![PatternSnapshot::new_default(
                    self.tracks.len(),
                    &self.graph.effect_descriptors,
                )]
            } else {
                bank
            };
        }

        self.state.pattern.num_patterns.store(
            self.state.pattern.pattern_bank.lock().unwrap().len() as u32,
            Ordering::Relaxed,
        );
        self.state
            .pattern
            .current_pattern
            .store(current_pattern as u32, Ordering::Relaxed);
        self.state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.state.transport.bpm.store(bpm, Ordering::Relaxed);
        self.state
            .transport
            .master_volume
            .store(master_volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);

        self.ui.cursor_track = 0;
        self.ui.cursor_step = 0;
        self.ui.pattern_page = current_pattern / 10;
        self.ui.focused_region = if self.tracks.is_empty() {
            Region::Sidebar
        } else {
            Region::Cirklon
        };
        self.ui.sidebar_tab = if self.tracks.is_empty() {
            SidebarTab::Sounds
        } else {
            SidebarTab::Tools
        };
        self.ui.sidebar_mode = if self.tracks.is_empty() {
            SidebarMode::InstrumentPicker
        } else {
            self.effective_sidebar_mode()
        };

        let current_sample_ids = {
            let bank = self.state.pattern.pattern_bank.lock().unwrap();
            bank[current_pattern].restore(&self.state);
            bank[current_pattern].sample_ids.clone()
        };
        eprintln!(
            "project-load: restored current_pattern={} sample_ids={} graph_tracks={}",
            current_pattern,
            current_sample_ids.len(),
            self.graph.track_node_ids.len()
        );
        self.state.publish_scheduler_snapshot();
        self.graph_controller()
            .apply_sample_ids(&current_sample_ids);
        self.set_reverb_param(0, reverb.size);
        self.set_reverb_param(1, reverb.brightness);
        self.set_reverb_param(2, reverb.replace);
        self.push_all_restored_defaults();

        if !self.tracks.is_empty() {
            self.clamp_cursor_to_steps();
            self.browser.sync_to_track(
                &self.tracks,
                self.ui.cursor_track,
                self.is_sampler_track(self.ui.cursor_track),
                &self.ui,
            );
        }

        self.current_project_name = Some(pending.name.clone());
        self.editor.scratch_buffer = scratch.buffer;
        self.editor.scratch_cursor = (scratch.cursor_row, scratch.cursor_col);
        self.editor.scratch_runtime = None;
        self.state
            .set_scratch_source(self.editor.scratch_buffer.clone());
        self.clear_control_hooks();
        if let Err(error) = self.rebuild_scratch_runtime_from_buffer() {
            self.editor.status_message =
                Some((format!("Scratch eval error: {error}"), Instant::now()));
        }
        let status = if pending.fallback_samples > 0 {
            format!(
                "Opened project '{}' with {} fallback sample{}",
                pending.name,
                pending.fallback_samples,
                if pending.fallback_samples == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!("Opened project '{}'", pending.name)
        };
        eprintln!("project-load: finish complete status={status}");
        self.editor.status_message = Some((status, Instant::now()));
        self.editor.pending_project_load = None;
        Ok(())
    }

    fn project_pattern_into_snapshot(
        &mut self,
        pattern: ProjectPattern,
    ) -> Result<(PatternSnapshot, usize), String> {
        let num_tracks = self.tracks.len();
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut fallback_count = 0;
        for track_idx in 0..num_tracks {
            if self.is_sampler_track(track_idx) {
                let saved_path = pattern
                    .sample_paths
                    .get(track_idx)
                    .and_then(|path| path.as_ref())
                    .map(PathBuf::from);
                let saved_name = pattern
                    .sample_names
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_default();

                let resolved_path = saved_path
                    .as_ref()
                    .filter(|path| path.exists())
                    .cloned()
                    .or_else(|| {
                        if saved_name.is_empty() {
                            None
                        } else {
                            self.sample_path_registry
                                .get(&saved_name)
                                .cloned()
                                .or_else(|| self.resolve_sample_path_by_name(&saved_name))
                        }
                    })
                    .or_else(|| self.first_available_sample_path());

                let Some(path_buf) = resolved_path else {
                    return Err(format!(
                        "Couldn't recover sample for track {} and no fallback samples exist",
                        track_idx + 1
                    ));
                };
                let (buffer_id, sample_name) =
                    crate::sampler::load_wav_buffer(self.graph.lg.0, &path_buf).map_err(
                        |error| {
                            format!(
                                "Failed to load sample '{}' for track {}: {}",
                                path_buf.display(),
                                track_idx + 1,
                                error
                            )
                        },
                    )?;
                if saved_path.as_ref() != Some(&path_buf) {
                    fallback_count += 1;
                }
                self.register_sample_path(&sample_name, path_buf);
                sample_ids.push((buffer_id, sample_name));
            } else {
                sample_ids.push((-1, String::new()));
            }
        }

        let crate::project::ProjectPattern {
            track_bits,
            step_data,
            track_params,
            effect_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_sound_states,
            chord_snapshots,
            timebase_plock_snapshots,
            instrument_types: _,
            sample_paths: _,
            sample_names: _,
        } = pattern;

        Ok((
            PatternSnapshot {
                track_bits,
                step_data,
                track_params: track_params.into_iter().map(Into::into).collect(),
                effect_slots: effect_slots
                    .into_iter()
                    .enumerate()
                    .map(|(track_idx, slots)| {
                        slots
                            .into_iter()
                            .enumerate()
                            .map(|(slot_idx, slot)| {
                                let node_id = self.state.pattern.effect_chains[track_idx][slot_idx]
                                    .node_id
                                    .load(Ordering::Relaxed);
                                slot.into_snapshot_with_node_id(node_id)
                            })
                            .collect()
                    })
                    .collect(),
                instrument_slots: (0..num_tracks)
                    .map(|track_idx| {
                        let node_id = self.state.pattern.instrument_slots[track_idx]
                            .node_id
                            .load(Ordering::Relaxed);
                        if self.is_sampler_track(track_idx) {
                            crate::effects::EffectSlotSnapshot::new_empty()
                        } else {
                            let slot =
                                instrument_slots.get(track_idx).cloned().unwrap_or_else(|| {
                                    crate::project::ProjectEffectSlot {
                                        num_params: 0,
                                        defaults: Vec::new(),
                                        plocks: vec![Vec::new(); MAX_STEPS],
                                        param_node_indices: Vec::new(),
                                    }
                                });
                            if slot.num_params == 0 {
                                crate::effects::EffectSlotSnapshot::capture(
                                    &self.state.pattern.instrument_slots[track_idx],
                                )
                            } else {
                                slot.into_snapshot_with_node_id(node_id)
                            }
                        }
                    })
                    .collect(),
                instrument_base_note_offsets,
                track_sound_states: track_sound_states
                    .into_iter()
                    .enumerate()
                    .map(|(track_idx, sound)| {
                        let engine_id = self
                            .graph
                            .track_engine_ids
                            .get(track_idx)
                            .and_then(|id| *id);
                        sound.into_track_sound_state(engine_id)
                    })
                    .collect(),
                sample_ids,
                chord_snapshots: chord_snapshots
                    .into_iter()
                    .map(chord_snapshot_from_steps)
                    .collect(),
                timebase_plock_snapshots: timebase_plock_snapshots
                    .into_iter()
                    .map(|steps| {
                        let mut snapshot = [None; MAX_STEPS];
                        for (idx, value) in steps.into_iter().take(MAX_STEPS).enumerate() {
                            snapshot[idx] = value;
                        }
                        snapshot
                    })
                    .collect(),
                instrument_types: self.graph.track_instrument_types.clone(),
            },
            fallback_count,
        ))
    }

    fn first_available_sample_path(&self) -> Option<PathBuf> {
        fn walk(dir: &Path) -> Option<PathBuf> {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(found) = walk(&path) {
                        return Some(found);
                    }
                } else if path
                    .extension()
                    .map(|ext| ext.to_ascii_lowercase() == "wav")
                    .unwrap_or(false)
                {
                    return Some(path);
                }
            }
            None
        }

        walk(Path::new("samples"))
    }

    pub fn push_all_restored_defaults(&self) {
        self.push_master_volume();
        for track_idx in 0..self.tracks.len() {
            self.push_track_volume(track_idx);
            self.push_track_pan(track_idx);
            self.push_send_gain(track_idx);
            for slot_idx in 0..self.state.pattern.effect_chains[track_idx].len() {
                let slot = &self.state.pattern.effect_chains[track_idx][slot_idx];
                let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
                for param_idx in 0..num_params {
                    let value = slot.defaults.get(param_idx);
                    let host_control = self
                        .graph
                        .effect_descriptors
                        .get(track_idx)
                        .and_then(|slots| slots.get(slot_idx))
                        .and_then(|desc| desc.params.get(param_idx))
                        .and_then(|param| param.host_control.as_ref());
                    if matches!(
                        host_control,
                        Some(crate::effects::HostControl::FxSidechain { .. })
                    ) {
                        self.apply_effect_sidechain_selection(
                            track_idx,
                            slot_idx,
                            param_idx,
                            value.round().max(0.0) as usize,
                        );
                    } else {
                        self.send_slot_param(track_idx, slot_idx, param_idx, value);
                    }
                }
            }
        }
        self.push_all_restored_instrument_defaults();
    }
}
