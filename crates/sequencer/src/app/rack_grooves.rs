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

use super::*;
use crate::groove::{
    extract_groove, heard_hits, quantize_groove_source, GrooveExtractOptions, GrooveId,
    GroovePadSource, GrooveRef,
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
}
