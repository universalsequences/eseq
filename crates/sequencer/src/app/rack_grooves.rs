//! Rack grooves: the recorded app-level edits (docs/rack-groove-spec.md,
//! "Extraction"). The groove model and the pure extraction live in
//! `crate::groove`; this file resolves a rack's source patterns and commits
//! the result.
//!
//! Grooves are rack config (`ProjectRackConfig::grooves` / `::groove`), and
//! the recorded group-structure mutation captures the rack config AND the
//! project scenes, so "extract, quantize the source and activate" is a single
//! `BusGroupStructure` patch: one undo step restores the patterns, the swing
//! and the rack's groove list together.
//!
//! Grooves also move between racks (eseq-groove.7): a kit carries its rack's
//! grooves, and a groove picked from another rack is copied into the target's
//! list (`crate::groove::import_grooves`). Nothing is remapped per pad on the
//! way: pad rows key on `pad_note`, so the target's pads play the rows with
//! their own notes and every other pad plays the shared row.

use super::*;
use crate::groove::{
    extract_groove, heard_hits, import_grooves, install_groove_settings, quantize_groove_source,
    GrooveExtractOptions, GrooveId, GroovePadSource, GrooveRef, ProjectGroove, RackGrooveSettings,
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
    /// Zero the source's delays and swing and activate the new groove, so the
    /// pattern sounds the same through it (spec: default on).
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
                track,
                pattern,
            });
        }
    }
    Ok(lanes)
}

impl App {
    /// The scheduler's per-track groove table: every rack member of a rack
    /// with an active groove gets its pad row (or the shared row).
    pub fn track_groove_snapshots(&self) -> Vec<Option<crate::groove::TrackGrooveSnapshot>> {
        crate::groove::track_groove_snapshots(
            self.groups.iter().filter_map(|group| {
                group
                    .rack
                    .as_ref()
                    .map(|rack| (group.members.as_slice(), rack))
            }),
            self.tracks.len(),
        )
    }

    /// "Extract Groove…": reads the rack's source patterns, extracts a groove
    /// into the rack's list and, with `quantize_source`, straightens the
    /// source (`crate::groove::quantize_groove_source` on every source lane)
    /// and activates the new groove — all as ONE undo step. Without it the
    /// groove is only added: activating it over its own unquantized source
    /// would double the feel.
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
        let groove_id = group
            .rack
            .as_ref()
            .map(|rack| rack.next_groove_id())
            .unwrap_or(1);
        let sources = lanes
            .iter()
            .map(|lane| {
                let data = scenes
                    .track_pools
                    .get(lane.track)
                    .and_then(|pool| pool.get(lane.pattern))
                    .ok_or_else(|| format!("Track {} lost its pattern", lane.track + 1))?;
                Ok(GroovePadSource {
                    pad_note: lane.pad_note,
                    hits: heard_hits(&data),
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
            let rack = app
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
                .and_then(|group| group.rack.as_mut())
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            rack.grooves.push(groove);
            if quantize {
                rack.groove.active = Some(GrooveRef::Rack(groove_id));
            }
            Ok(groove_id)
        })
    }

    /// Picks the groove a rack plays through (`None` = off). A rack groove
    /// must exist in the rack's list. One undo step.
    pub fn set_rack_active_groove_recorded(
        &mut self,
        group_id: u64,
        active: Option<GrooveRef>,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Set rack groove", move |app| {
            let rack = app
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
                .and_then(|group| group.rack.as_mut())
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            if let Some(GrooveRef::Rack(id)) = &active {
                if rack.groove_by_id(*id).is_none() {
                    return Err(format!("Drum rack has no groove {id}"));
                }
            }
            if rack.groove.active == active {
                return Err("Rack groove is unchanged".to_string());
            }
            rack.groove.active = active;
            Ok(())
        })
    }
    /// Installs a loaded kit's grooves and selection on a rack (kit version
    /// 5), as one recorded edit. Grooves merge into the rack's list under
    /// fresh ids; a kit reference to one of its own grooves follows the id
    /// map. Nothing to install (no grooves, default settings) records nothing.
    pub(super) fn install_kit_grooves(
        &mut self,
        group_id: u64,
        grooves: &[ProjectGroove],
        settings: &RackGrooveSettings,
    ) -> Result<(), String> {
        if grooves.is_empty() && settings.is_default() {
            return Ok(());
        }
        let grooves = grooves.to_vec();
        let settings = settings.clone();
        self.apply_recorded_bus_group_structure_mutation("Load kit grooves", move |app| {
            let rack = app
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
                .and_then(|group| group.rack.as_mut())
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            let id_map = import_grooves(rack, &grooves);
            install_groove_settings(rack, &settings, &id_map);
            Ok(())
        })
    }

    /// Plays one rack's groove on another: copies `groove_id` from
    /// `source_group_id`'s list into `target_group_id`'s (reusing an identical
    /// groove already there) and activates it, as one undo step. The target's
    /// own amounts stay. Returns the groove's id in the target rack.
    ///
    /// Pads map by `pad_note`: a target pad whose note has a row in the
    /// groove plays that row, every other pad the shared row. On the source
    /// rack itself this just activates the groove; re-applying a groove the
    /// target already plays records nothing.
    pub fn apply_rack_groove_from_rack_recorded(
        &mut self,
        target_group_id: u64,
        source_group_id: u64,
        groove_id: GrooveId,
    ) -> Result<GrooveId, String> {
        let groove = self
            .groups
            .iter()
            .find(|group| group.id == source_group_id)
            .ok_or_else(|| format!("Track group {source_group_id} does not exist"))?
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {source_group_id} is not a drum rack"))?
            .groove_by_id(groove_id)
            .cloned()
            .ok_or_else(|| format!("Drum rack has no groove {groove_id}"))?;
        if target_group_id == source_group_id {
            let active = Some(GrooveRef::Rack(groove_id));
            let already = self
                .groups
                .iter()
                .find(|group| group.id == target_group_id)
                .and_then(|group| group.rack.as_ref())
                .is_some_and(|rack| rack.groove.active == active);
            if !already {
                self.set_rack_active_groove_recorded(target_group_id, active)?;
            }
            return Ok(groove_id);
        }
        // Already copied and playing: nothing to record.
        let target = self
            .groups
            .iter()
            .find(|group| group.id == target_group_id)
            .and_then(|group| group.rack.as_ref())
            .ok_or_else(|| format!("Track group {target_group_id} is not a drum rack"))?;
        if let Some(existing) = target.grooves.iter().find(|own| own.same_feel(&groove)) {
            if target.groove.active == Some(GrooveRef::Rack(existing.id)) {
                return Ok(existing.id);
            }
        }
        self.apply_recorded_bus_group_structure_mutation("Apply groove from rack", move |app| {
            let rack = app
                .groups
                .iter_mut()
                .find(|group| group.id == target_group_id)
                .and_then(|group| group.rack.as_mut())
                .ok_or_else(|| format!("Track group {target_group_id} is not a drum rack"))?;
            let id_map = import_grooves(rack, std::slice::from_ref(&groove));
            let (_, id) = id_map
                .first()
                .copied()
                .ok_or_else(|| format!("Groove '{}' is malformed", groove.name))?;
            rack.groove.active = Some(GrooveRef::Rack(id));
            Ok(id)
        })
    }
}
