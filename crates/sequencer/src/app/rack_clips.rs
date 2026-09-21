//! Rack clips: the recorded app-level edits (`docs/rack-clips-and-break-kits-spec.md`
//! §4.2–4.4).
//!
//! The bank itself lives in `ProjectScenes` (see
//! `sequencer/state/rack_clips.rs`), which the recorded group-structure
//! mutation captures whole — so every clip create/delete/convert here undoes
//! for free through `BusGroupStructureState::scenes`.

use super::*;
use crate::sequencer::RackClipId;

impl App {
    /// A rack's clip bank as the UI needs it: `(id, name, active-in-current-scene)`.
    pub fn rack_clip_bank(&self, group_id: u64) -> Vec<(RackClipId, String, bool)> {
        self.state.with_scenes(|scenes| {
            let active = scenes.current_rack_clip(group_id);
            scenes
                .rack_bank(group_id)
                .map(|bank| {
                    bank.clips
                        .iter()
                        .map(|clip| (clip.id, clip.name.clone(), Some(clip.id) == active))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    pub fn rack_is_legacy(&self, group_id: u64) -> bool {
        self.state
            .with_scenes(|scenes| scenes.rack_is_legacy(group_id))
    }

    fn rack_members(&self, group_id: u64) -> Result<Vec<usize>, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if group.rack.is_none() {
            return Err(format!("Track group {group_id} is not a drum rack"));
        }
        Ok(group.members.clone())
    }

    /// "Convert to clips" (§4.3). Runs once per legacy rack: every project
    /// scene's member slice and rack-owned overrides move into a clip named
    /// after the scene, and the scene points at it. All-empty scenes share the
    /// `None` pointer. One undoable group-structure patch.
    pub fn convert_rack_to_clips_recorded(&mut self, group_id: u64) -> Result<usize, String> {
        let members = self.rack_members(group_id)?;
        if !self.rack_is_legacy(group_id) {
            return Err("This rack already uses clips".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Convert rack to clips", move |app| {
            app.state
                .with_scenes_mut(|scenes| scenes.convert_rack_to_clips(group_id, &members))
                .ok_or_else(|| "This rack already uses clips".to_string())
        })
    }

    /// Point the current scene at `clip` (§4.4: the audible launch is the
    /// caller's relaunch of the current scene, which rides quantized launch).
    pub fn set_current_rack_clip_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> Result<(), String> {
        self.rack_members(group_id)?;
        super::edit::finish_active_gesture(self);
        let scene = self.state.current_scene_id()
            .ok_or_else(|| "The current scene no longer exists".to_string())?;
        let (index, before) = self.validate_rack_clip_assignment(scene, group_id, clip)?;
        if before == clip {
            return Ok(());
        }
        self.save_live_patterns_before_clip_assignment()?;
        self.state.with_scenes_mut(|scenes| {
            assert!(scenes.set_scene_rack_clip(index, group_id, clip));
        });
        let patch = super::history::RackClipAssignmentPatch { scene, group_id, before, after: clip };
        self.history.commit("Launch rack clip", None,
            super::history::EditPatch::RackClipAssignment(patch),
            std::mem::size_of::<super::history::RackClipAssignmentPatch>());
        Ok(())
    }

    fn validate_rack_clip_assignment(
        &self,
        scene: crate::sequencer::SceneId,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> Result<(usize, Option<RackClipId>), String> {
        self.rack_members(group_id)?;
        self.state.with_scenes(|scenes| {
            let index = scenes.scene_index(scene)
                .ok_or_else(|| "That scene no longer exists".to_string())?;
            let bank = scenes.rack_bank(group_id)
                .ok_or_else(|| "That rack has no clip bank".to_string())?;
            if clip.is_some_and(|id| bank.clip(id).is_none()) {
                return Err("That rack clip no longer exists".to_string());
            }
            Ok((index, scenes.scene_rack_clip(index, group_id)))
        })
    }

    fn save_live_patterns_before_clip_assignment(&mut self) -> Result<(), String> {
        // Save while the old pointer still identifies the audible clip. Once
        // it changes, launch save-back deliberately ignores those stale lanes.
        if self.state.save_current_pattern_snapshot(self.tracks.len(),
            &self.graph.track_buffer_ids, &self.graph.track_sample_rates,
            &self.tracks, &self.graph.track_instrument_types)
        {
            Ok(())
        } else {
            Err("Could not save the playing patterns before assigning a rack clip".to_string())
        }
    }

    pub(super) fn restore_rack_clip_assignment(
        &mut self,
        scene: crate::sequencer::SceneId,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> Result<(), String> {
        let (index, _) = self.validate_rack_clip_assignment(scene, group_id, clip)?;
        let is_current = self.state.current_scene_id() == Some(scene);
        if is_current {
            self.save_live_patterns_before_clip_assignment()?;
        }
        self.state.with_scenes_mut(|scenes| {
            assert!(scenes.set_scene_rack_clip(index, group_id, clip));
        });
        if is_current {
            let _ = self.state.quantized_launches().cancel_all();
            self.apply_pattern_launch(&crate::quantized_launch::PatternLaunchTarget::Scene { scene: index })
                .map_err(|error| format!("Could not restore rack clip playback: {error:?}"))?;
        } else {
            self.state.publish_scheduler_snapshot();
        }
        Ok(())
    }

    /// "Save clip as…": a new clip holding an independent copy of what the rack
    /// is playing right now, with the current scene pointed at it. A legacy
    /// rack is converted first (§4.3) in the same undo entry, so every other
    /// scene keeps its member slices as clips instead of falling silent the
    /// moment the rack becomes clip-bearing; the new clip is then forked from
    /// the current scene's clip.
    /// "Clip N" with the smallest N no clip of this rack already uses, so a
    /// bank that converted from scenes reads "Scene 1, Scene 2, Clip 3" rather
    /// than a run of identical "Clip" entries.
    fn next_rack_clip_name(&self, group_id: u64) -> String {
        self.state.with_scenes(|scenes| {
            let names: Vec<&str> = scenes
                .rack_bank(group_id)
                .map(|bank| bank.clips.iter().map(|clip| clip.name.as_str()).collect())
                .unwrap_or_default();
            let mut n = names.len() + 1;
            loop {
                let candidate = format!("Clip {n}");
                if !names.iter().any(|name| *name == candidate) {
                    return candidate;
                }
                n += 1;
            }
        })
    }

    pub fn save_rack_clip_as_recorded(
        &mut self,
        group_id: u64,
        name: &str,
    ) -> Result<RackClipId, String> {
        let members = self.rack_members(group_id)?;
        let name = if name.trim().is_empty() {
            self.next_rack_clip_name(group_id)
        } else {
            name.trim().to_string()
        };
        self.apply_recorded_bus_group_structure_mutation("Save rack clip", move |app| {
            Ok(app.state.with_scenes_mut(|scenes| {
                let scene = scenes.current_scene;
                if scenes.rack_is_legacy(group_id) {
                    scenes.convert_rack_to_clips(group_id, &members);
                }
                // Fork the lanes so editing the new clip never edits the old.
                let forked: Vec<Option<crate::sequencer::PatternId>> = members
                    .iter()
                    .map(|track| {
                        let data = scenes
                            .composed_scene_cell(scene, *track)
                            .and_then(|id| scenes.track_pools.get(*track)?.get(id))?;
                        Some(scenes.track_pools.get_mut(*track)?.insert(data))
                    })
                    .collect();
                let overrides = scenes
                    .composed_graph_overrides(scene)
                    .into_iter()
                    .filter(|graph| graph.owner_rack == Some(group_id))
                    .collect();
                let id = scenes.create_rack_clip_with_members(group_id, &members, &name);
                if let Some(clip) = scenes
                    .rack_bank_mut(group_id)
                    .and_then(|bank| bank.clip_mut(id))
                {
                    clip.cells = forked;
                    clip.graph_overrides = overrides;
                }
                scenes.set_scene_rack_clip(scene, group_id, Some(id));
                // The rack now owns its slices; the scene's own cells for those
                // members are dead weight and must not shadow the clip.
                if let Some(scene) = scenes.scenes.get_mut(scene) {
                    for track in &members {
                        if let Some(cell) = scene.cells.get_mut(*track) {
                            *cell = None;
                        }
                    }
                }
                // The new clip was forked FROM the live lanes, so they are not
                // stale against it.
                scenes.adopt_live_rack_clips();
                id
            }))
        })
    }

    pub fn delete_rack_clip_recorded(
        &mut self,
        group_id: u64,
        clip: RackClipId,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Delete rack clip", move |app| {
            if app
                .state
                .with_scenes_mut(|scenes| scenes.delete_rack_clip(group_id, clip))
            {
                Ok(())
            } else {
                Err("That rack clip no longer exists".to_string())
            }
        })
    }

    pub fn rename_rack_clip_recorded(
        &mut self,
        group_id: u64,
        clip: RackClipId,
        name: &str,
    ) -> Result<(), String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("A clip name cannot be empty".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Rename rack clip", move |app| {
            if app
                .state
                .with_scenes_mut(|scenes| scenes.rename_rack_clip(group_id, clip, &name))
            {
                Ok(())
            } else {
                Err("That rack clip no longer exists".to_string())
            }
        })
    }
}
