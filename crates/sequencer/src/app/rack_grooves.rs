//! Rack grooves: the recorded app-level edits (docs/rack-groove-spec.md,
//! "Extraction" and §Groove pool, library and pad roles). The groove model
//! and the pure extraction live in `crate::groove`; this file resolves a
//! rack's source patterns and commits the result.
//!
//! Grooves live in the project pool (`App::grooves`); a rack points into it
//! (`ProjectRackConfig::groove.active`). The recorded group-structure
//! mutation captures the rack configs, the pool AND the project scenes, so
//! "extract, quantize the source and activate" is a single
//! `BusGroupStructure` patch: one undo step restores the patterns, the swing,
//! the pool and the selection together.
//!
//! Grooves enter the pool by extraction, copy-on-apply from the library
//! (`apply_library_groove_recorded`) or from a kit (`install_kit_groove`);
//! the last two reuse a pool groove with the same feel
//! (`crate::groove::import_groove`). Nothing is remapped per pad on the way:
//! pad rows key on `pad_note`, so a rack's pads play the rows with their own
//! notes and every other pad plays the shared row.

use super::*;
use crate::groove::{
    extract_groove, heard_hits_through, import_groove, next_pool_groove_id, pool_groove,
    quantize_groove_source, GrooveExtractOptions, GrooveId, GroovePadSource, KitGroove,
    ProjectGroove, RackGrooveSettings,
};
use crate::sequencer::{PatternId, ProjectScenes, RackClipId};

/// Which patterns a groove is extracted from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrooveExtractSource {
    /// Each member's current effective pattern (its active rack clip's cell
    /// on a clip rack, the scene cell on a legacy one).
    #[default]
    CurrentPatterns,
    /// One clip of the rack's bank.
    Clip(RackClipId),
}

/// Everything the Extract Groove modal collects.
#[derive(Clone, Debug, PartialEq)]
pub struct RackGrooveExtractRequest {
    pub options: GrooveExtractOptions,
    pub source: GrooveExtractSource,
    /// Zero the source's delays and swing and activate the new groove at
    /// neutral settings (on, 1×, full timing, no accent or jitter, every pad
    /// included), so the pattern sounds the same through it (spec: default
    /// on).
    pub quantize_source: bool,
}

impl Default for RackGrooveExtractRequest {
    fn default() -> Self {
        Self {
            options: GrooveExtractOptions::default(),
            source: GrooveExtractSource::default(),
            quantize_source: true,
        }
    }
}

/// One pad's source lane: its note, member track and the pool pattern read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GrooveSourceLane {
    pad_note: i32,
    role: Option<crate::project::PadRole>,
    track: usize,
    pattern: PatternId,
}

/// Resolves each pad of a rack to the pattern the groove reads. Pads whose
/// member has no pattern there (a silent clip cell, a bare lane) are skipped.
fn groove_source_lanes(
    scenes: &ProjectScenes,
    group: &crate::project::ProjectTrackGroup,
    source: GrooveExtractSource,
) -> Result<Vec<GrooveSourceLane>, String> {
    let rack = group
        .rack
        .as_ref()
        .ok_or_else(|| format!("Track group {} is not a drum rack", group.id))?;
    let clip = match source {
        GrooveExtractSource::CurrentPatterns => None,
        GrooveExtractSource::Clip(clip_id) => Some(
            scenes
                .rack_bank(group.id)
                .and_then(|bank| bank.clip(clip_id))
                .ok_or_else(|| format!("Drum rack has no clip {clip_id}"))?,
        ),
    };
    let mut lanes = Vec::with_capacity(rack.pads.len());
    for pad in &rack.pads {
        let Some(&track) = group.members.get(pad.member) else {
            continue;
        };
        let pattern = match clip {
            None => scenes.effective_pattern_id(track),
            Some(clip) => clip.cells.get(pad.member).copied().flatten(),
        };
        if let Some(pattern) = pattern {
            lanes.push(GrooveSourceLane {
                pad_note: pad.pad_note,
                role: pad.effective_role(),
                track,
                pattern,
            });
        }
    }
    Ok(lanes)
}

/// The rack settings a kit's groove installs: its copy enters `pool`
/// (reusing an identical pool groove) and becomes the rack's active groove
/// with the kit's amounts. A kit whose rack played no groove (or whose copy
/// is malformed) installs the default settings: groove off.
pub(super) fn kit_groove_settings(
    pool: &mut Vec<ProjectGroove>,
    kit_groove: Option<&KitGroove>,
) -> RackGrooveSettings {
    match kit_groove {
        Some(kit) => match import_groove(pool, &kit.groove) {
            Some(id) => kit.settings(Some(id)),
            None => RackGrooveSettings::default(),
        },
        None => RackGrooveSettings::default(),
    }
}

/// "<name> copy", then "<name> copy 2", ... — the first not already a pool
/// groove's name.
fn unique_copy_name(pool: &[ProjectGroove], name: &str) -> String {
    let base = format!("{name} copy");
    let taken = |candidate: &str| pool.iter().any(|groove| groove.name == candidate);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always finds a free name")
}

impl App {
    /// The scheduler's per-track groove table: every rack member of a rack
    /// with an active pool groove gets its pad row (or the shared row).
    pub fn track_groove_snapshots(&self) -> Vec<Option<crate::groove::TrackGrooveSnapshot>> {
        crate::groove::track_groove_snapshots(
            self.groups.iter().filter_map(|group| {
                group
                    .rack
                    .as_ref()
                    .map(|rack| (group.members.as_slice(), rack))
            }),
            &self.grooves,
            self.tracks.len(),
        )
    }

    /// Every groove table scenes pick among: each rack on its own settings,
    /// plus one variant per clip that plays its own groove.
    pub fn rack_groove_variants(&self) -> crate::groove::RackGrooveVariants {
        crate::groove::RackGrooveVariants::build(
            self.groups.iter().filter_map(|group| {
                group
                    .rack
                    .as_ref()
                    .map(|rack| (group.id, group.members.as_slice(), rack))
            }),
            &self.grooves,
            self.tracks.len(),
        )
    }

    /// The racks (group ids, in group order) whose active groove — the rack
    /// default or one of its clips' own — is pool groove `groove_id`.
    pub fn racks_using_groove(&self, groove_id: GrooveId) -> Vec<u64> {
        self.groups
            .iter()
            .filter(|group| {
                group
                    .rack
                    .as_ref()
                    .is_some_and(|rack| {
                        rack.groove.active == Some(groove_id)
                            || rack
                                .clip_grooves
                                .iter()
                                .any(|own| own.settings.active == Some(groove_id))
                    })
            })
            .map(|group| group.id)
            .collect()
    }

    /// The clip rack `group_id` plays in the current scene (`None` for a
    /// rack without clips): the source clip of an extraction.
    pub fn current_rack_clip(&self, group_id: u64) -> Option<RackClipId> {
        self.state.with_scenes(|scenes| scenes.current_rack_clip(group_id))
    }

    /// The settings an edit aimed at `clip` changes (see
    /// `ProjectRackConfig::groove_target_mut`), read-only.
    fn rack_groove(
        &self,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> Result<&crate::groove::RackGrooveSettings, String> {
        Ok(self.rack_config(group_id)?.groove_for_clip(clip))
    }

    fn rack_config(&self, group_id: u64) -> Result<&crate::project::ProjectRackConfig, String> {
        self.groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| group.rack.as_ref())
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))
    }

    /// "Extract Groove…": reads the rack's source patterns and extracts a
    /// groove into the project pool; with `quantize_source` it also
    /// straightens the source (`crate::groove::quantize_groove_source` on
    /// every source lane) and activates the new groove on the source rack —
    /// all as ONE undo step. Without it the groove is only added to the pool:
    /// activating it over its own unquantized source would double the feel.
    pub fn extract_rack_groove_recorded(
        &mut self,
        group_id: u64,
        request: &RackGrooveExtractRequest,
    ) -> Result<GrooveId, String> {
        let scenes = self.capture_synchronized_scene_structure_state()?;
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let lanes = groove_source_lanes(&scenes, group, request.source)?;
        // The clip the source patterns belong to: quantizing straightens
        // THAT clip, so it (not every clip of the rack) plays the new groove.
        let source_clip = match request.source {
            GrooveExtractSource::Clip(clip) => Some(clip),
            GrooveExtractSource::CurrentPatterns => self.current_rack_clip(group_id),
        };
        // What the source clip plays through now: a grooved member heard the
        // groove's pocket, not its pattern's swing.
        let rack = self.rack_config(group_id)?;
        let playing = crate::groove::rack_member_grooves(
            &group.members,
            rack,
            rack.groove_for_clip(source_clip),
            &self.grooves,
        );
        let groove_id = next_pool_groove_id(&self.grooves);
        let sources = lanes
            .iter()
            .map(|lane| {
                let groove = playing
                    .iter()
                    .find(|(track, _)| *track == lane.track)
                    .and_then(|(_, groove)| groove.as_ref());
                let data = scenes
                    .track_pools
                    .get(lane.track)
                    .and_then(|pool| pool.get(lane.pattern))
                    .ok_or_else(|| format!("Track {} lost its pattern", lane.track + 1))?;
                Ok(GroovePadSource {
                    pad_note: lane.pad_note,
                    role: lane.role,
                    hits: heard_hits_through(&data, groove),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let groove = extract_groove(groove_id, &request.options, &sources)
            .map_err(|error| error.to_string())?;
        let quantize = request.quantize_source;
        let label = if quantize {
            "Extract groove and quantize source"
        } else {
            "Extract groove"
        };
        self.apply_recorded_bus_group_structure_mutation(label, move |app| {
            if quantize {
                let mut scenes = app.capture_synchronized_scene_structure_state()?;
                let mut changed = false;
                for lane in &lanes {
                    let pool = scenes
                        .track_pools
                        .get_mut(lane.track)
                        .ok_or_else(|| format!("Track {} has no pattern pool", lane.track + 1))?;
                    let mut data = pool
                        .get(lane.pattern)
                        .ok_or_else(|| format!("Track {} lost its pattern", lane.track + 1))?;
                    if quantize_groove_source(&mut data) {
                        if !pool.store(lane.pattern, data) {
                            return Err(format!(
                                "Could not quantize the pattern on track {}",
                                lane.track + 1
                            ));
                        }
                        changed = true;
                    }
                }
                if changed {
                    app.restore_scene_structure_state(&scenes)?;
                }
            }
            let rack = app.rack_config_mut(group_id)?;
            if quantize {
                // The straightened source sounds the same only through the
                // groove exactly as extracted: on, 1× scale, full timing, no
                // accent or jitter, every pad at its full share.
                let settings = RackGrooveSettings {
                    active: Some(groove_id),
                    ..RackGrooveSettings::default()
                };
                match source_clip {
                    Some(clip) => {
                        if rack.clip_groove(clip).is_none() {
                            rack.clip_grooves.push(crate::project::RackClipGroove {
                                clip,
                                settings: settings.clone(),
                            });
                        }
                        *rack.groove_target_mut(Some(clip)) = settings;
                    }
                    None => rack.groove = settings,
                }
            }
            app.grooves.push(groove);
            Ok(groove_id)
        })
    }

    /// Gives clip `clip` its own groove (a copy of what it plays now, the
    /// rack's), or, with `own == false`, returns it to the rack's. One undo
    /// step.
    pub fn set_rack_clip_own_groove_recorded(
        &mut self,
        group_id: u64,
        clip: RackClipId,
        own: bool,
    ) -> Result<(), String> {
        if self.rack_config(group_id)?.clip_groove(clip).is_some() == own {
            return Err("Clip groove is unchanged".to_string());
        }
        let label = if own { "Give clip its own groove" } else { "Use rack groove for clip" };
        self.apply_recorded_bus_group_structure_mutation(label, move |app| {
            let rack = app.rack_config_mut(group_id)?;
            if own {
                let settings = rack.groove.clone();
                rack.clip_grooves.push(crate::project::RackClipGroove { clip, settings });
            } else {
                rack.clip_grooves.retain(|entry| entry.clip != clip);
            }
            Ok(())
        })
    }

    /// "Apply to All Clips in This Rack": the groove `clip` plays (its own,
    /// else the rack's) becomes the rack's, and every clip follows it again.
    /// One undo step.
    pub fn apply_rack_groove_to_all_clips_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
    ) -> Result<(), String> {
        let settings = self.rack_groove(group_id, clip)?.clone();
        let rack = self.rack_config(group_id)?;
        if rack.clip_grooves.is_empty() && rack.groove == settings {
            // Every clip already plays it: nothing to record.
            return Ok(());
        }
        self.apply_recorded_bus_group_structure_mutation("Apply groove to all clips", move |app| {
            let rack = app.rack_config_mut(group_id)?;
            rack.groove = settings;
            rack.clip_grooves.clear();
            Ok(())
        })
    }

    /// Picks the pool groove a rack plays through (`None` = off). The groove
    /// must be in the pool. One undo step.
    pub fn set_rack_active_groove_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
        active: Option<GrooveId>,
    ) -> Result<(), String> {
        if let Some(id) = active {
            if pool_groove(&self.grooves, id).is_none() {
                return Err(format!("The project has no groove {id}"));
            }
        }
        if self.rack_groove(group_id, clip)?.active == active {
            return Err("Rack groove is unchanged".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Set rack groove", move |app| {
            app.rack_config_mut(group_id)?.groove_target_mut(clip).active = active;
            Ok(())
        })
    }

    /// The rack groove buffer's on/off switch: bypasses the rack's groove
    /// (every member plays straight) without forgetting the selection, the
    /// amounts or the pad shares. One undo step.
    pub fn set_rack_groove_enabled_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
        enabled: bool,
    ) -> Result<(), String> {
        if self.rack_groove(group_id, clip)?.enabled == enabled {
            return Err("Rack groove is unchanged".to_string());
        }
        let label = if enabled {
            "Turn rack groove on"
        } else {
            "Turn rack groove off"
        };
        self.apply_recorded_bus_group_structure_mutation(label, move |app| {
            app.rack_config_mut(group_id)?.groove_target_mut(clip).enabled = enabled;
            Ok(())
        })
    }

    /// The rack groove's time scale (one of [`crate::groove::GROOVE_SCALES`]):
    /// its grid stretches by this much when it plays. One undo step.
    pub fn set_rack_groove_scale_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
        scale: f32,
    ) -> Result<(), String> {
        if !crate::groove::GROOVE_SCALES.contains(&scale) {
            return Err(format!("A groove scale is ½×, 1× or 2×, not {scale}"));
        }
        if self.rack_groove(group_id, clip)?.scale == scale {
            return Err("Rack groove is unchanged".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Set groove scale", move |app| {
            app.rack_config_mut(group_id)?.groove_target_mut(clip).scale = scale;
            Ok(())
        })
    }

    /// Includes pad `pad_note` in the rack's groove or leaves it straight
    /// (its amount is kept for when it comes back). One undo step.
    pub fn set_rack_groove_pad_enabled_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
        pad_note: i32,
        enabled: bool,
    ) -> Result<(), String> {
        let rack = self.rack_config(group_id)?;
        if !rack.pads.iter().any(|pad| pad.pad_note == pad_note) {
            return Err(format!("The rack has no pad at note {pad_note}"));
        }
        if rack.groove_for_clip(clip).pad(pad_note).enabled == enabled {
            return Err("Pad groove is unchanged".to_string());
        }
        let label = if enabled {
            "Include pad in groove"
        } else {
            "Exclude pad from groove"
        };
        self.apply_recorded_bus_group_structure_mutation(label, move |app| {
            let settings = app.rack_config_mut(group_id)?.groove_target_mut(clip);
            settings.pad_mut(pad_note).enabled = enabled;
            settings.sanitize();
            Ok(())
        })
    }

    /// Copy-on-apply: imports `groove` (a library file's groove) into the
    /// project pool — reusing a pool groove with the same feel — and makes it
    /// the rack's active groove, as ONE undo step. The rack's amounts stay.
    /// Returns the pool id. Re-applying a groove the rack already plays
    /// records nothing.
    pub fn apply_library_groove_recorded(
        &mut self,
        group_id: u64,
        clip: Option<RackClipId>,
        groove: &ProjectGroove,
    ) -> Result<GrooveId, String> {
        if !groove.is_well_formed() {
            return Err(format!("Groove '{}' is malformed", groove.name));
        }
        let rack = self.rack_config(group_id)?;
        if let Some(existing) = self.grooves.iter().find(|own| own.same_feel(groove)) {
            if rack.groove_for_clip(clip).active == Some(existing.id) {
                return Ok(existing.id);
            }
        }
        let groove = groove.clone();
        self.apply_recorded_bus_group_structure_mutation("Apply groove", move |app| {
            let id = import_groove(&mut app.grooves, &groove)
                .ok_or_else(|| format!("Groove '{}' is malformed", groove.name))?;
            app.rack_config_mut(group_id)?.groove_target_mut(clip).active = Some(id);
            Ok(id)
        })
    }

    /// Renames one pool groove. One undo step; an empty or unchanged name is
    /// refused.
    pub fn rename_pool_groove_recorded(
        &mut self,
        groove_id: GrooveId,
        name: &str,
    ) -> Result<(), String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("Groove name cannot be empty".to_string());
        }
        let current = pool_groove(&self.grooves, groove_id)
            .ok_or_else(|| format!("The project has no groove {groove_id}"))?;
        if current.name == name {
            return Err("Groove name is unchanged".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Rename groove", move |app| {
            let groove = app
                .grooves
                .iter_mut()
                .find(|groove| groove.id == groove_id)
                .ok_or_else(|| format!("The project has no groove {groove_id}"))?;
            groove.name = name;
            Ok(())
        })
    }

    /// "Duplicate" (Grooves tab): adds a copy of pool groove `groove_id` to
    /// the pool under a fresh id and a unique "<name> copy" name, so it can
    /// be renamed and applied independently. One undo step; returns the new
    /// id. No rack changes.
    pub fn duplicate_pool_groove_recorded(
        &mut self,
        groove_id: GrooveId,
    ) -> Result<GrooveId, String> {
        let source = pool_groove(&self.grooves, groove_id)
            .ok_or_else(|| format!("The project has no groove {groove_id}"))?;
        let mut copy = source.clone();
        copy.id = next_pool_groove_id(&self.grooves);
        copy.name = unique_copy_name(&self.grooves, &source.name);
        let id = copy.id;
        self.apply_recorded_bus_group_structure_mutation("Duplicate groove", move |app| {
            app.grooves.push(copy);
            Ok(id)
        })
    }

    /// Deletes one pool groove. Every rack that plays it turns its groove
    /// off in the SAME undo step (the UI confirms first, listing
    /// [`App::racks_using_groove`]). Returns the racks that were turned off.
    pub fn delete_pool_groove_recorded(&mut self, groove_id: GrooveId) -> Result<Vec<u64>, String> {
        if pool_groove(&self.grooves, groove_id).is_none() {
            return Err(format!("The project has no groove {groove_id}"));
        }
        let using = self.racks_using_groove(groove_id);
        self.apply_recorded_bus_group_structure_mutation("Delete groove", move |app| {
            app.grooves.retain(|groove| groove.id != groove_id);
            for group in &mut app.groups {
                if let Some(rack) = group.rack.as_mut() {
                    for settings in rack.all_groove_settings_mut() {
                        if settings.active == Some(groove_id) {
                            settings.active = None;
                        }
                    }
                }
            }
            Ok(using)
        })
    }

    /// "Save to Library": writes a copy of pool groove `groove_id` into the
    /// user library under `name` (the groove's own name when `None`). Not an
    /// undoable edit; the project is unchanged and the file is not linked.
    pub fn save_pool_groove_to_library(
        &self,
        groove_id: GrooveId,
        name: Option<&str>,
    ) -> Result<std::path::PathBuf, String> {
        let groove = pool_groove(&self.grooves, groove_id)
            .ok_or_else(|| format!("The project has no groove {groove_id}"))?;
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&groove.name);
        crate::groove::library::save_groove_to_library(name, groove)
            .map_err(|error| format!("Could not save groove '{name}': {error}"))
    }

    /// Installs a loaded kit's groove on a rack (kit version 5+), as one
    /// recorded edit: the kit's copy enters the pool (an identical pool
    /// groove is reused) and the rack plays it with the kit's amounts; a kit
    /// whose rack played no groove turns the rack's groove off. A rack
    /// already in that state records nothing.
    pub(super) fn install_kit_groove(
        &mut self,
        group_id: u64,
        kit_groove: Option<&KitGroove>,
    ) -> Result<(), String> {
        let mut preview_pool = self.grooves.clone();
        let settings = kit_groove_settings(&mut preview_pool, kit_groove);
        if preview_pool.len() == self.grooves.len()
            && self.rack_config(group_id)?.groove == settings
        {
            return Ok(());
        }
        let kit_groove = kit_groove.cloned();
        self.apply_recorded_bus_group_structure_mutation("Load kit groove", move |app| {
            let settings = kit_groove_settings(&mut app.grooves, kit_groove.as_ref());
            app.rack_config_mut(group_id)?.groove = settings;
            Ok(())
        })
    }
}
