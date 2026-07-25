//! Arrangement editing primitives (docs/arrangement-lane-model-spec.md 8).
//!
//! The closed primitive set every arrangement edit reduces to: clip ops on a
//! track lane, scene-event ops on the scene lane, and the whole-arrangement
//! ops. Every primitive is guarded by `require_song_edit_unlocked`, commits
//! exactly one `ArrangementStructurePatch`, and ends with validate →
//! recompile → install (`SequencerState::set_committed_arrangement`, which
//! keeps the compiled song and `song_revision` in lockstep). A failed
//! primitive changes nothing and creates no history entry.
//!
//! Overlap is never rejected (spec 14, locked): every op that places or grows
//! a clip first calls `occlude_span`, so the incoming clip always wins and
//! non-overlap stays an invariant the ops maintain.

use crate::sequencer::{
    insert_clip_sorted, lower_rows_to_arrangement, occlude_span, restamped_clip, ArrClip, ClipId,
    LaneSource, ProjectArrangement, ProjectScenes, ProjectSongRow, SongRowId,
};

use super::edit::finish_active_gesture;
use super::history::{ArrangementStructurePatch, EditPatch};
use super::song_edit::SongRowSpec;
use super::App;

/// Reject a beat that cannot address anything on the timeline.
fn finite_beat(name: &str, beat: f64) -> Result<f64, String> {
    if !beat.is_finite() || beat < 0.0 {
        return Err(format!(
            "{name} must be a finite, non-negative beat (got {beat})"
        ));
    }
    Ok(beat)
}

impl App {
    /// The committed arrangement, or the standard "nothing to edit" error.
    fn require_arrangement(&self) -> Result<ProjectArrangement, String> {
        self.state
            .committed_arrangement()
            .ok_or_else(|| "The project has no arrangement".to_string())
    }

    /// Locate a clip by id, or report it missing (a stale gesture must never
    /// silently edit a different clip).
    fn locate_clip(
        arrangement: &ProjectArrangement,
        clip_id: ClipId,
    ) -> Result<(usize, ArrClip), String> {
        arrangement
            .find_clip(clip_id)
            .map(|(track, clip)| (track, *clip))
            .ok_or_else(|| format!("Arrangement clip id {} does not exist", clip_id.0))
    }

    /// Remove `clip_id` from its lane, returning the clip. Callers that move
    /// or resize take the clip out first so `occlude_span` cannot truncate the
    /// very clip being edited.
    fn take_clip(arrangement: &mut ProjectArrangement, track: usize, clip_id: ClipId) -> ArrClip {
        let lane = &mut arrangement.track_lanes[track];
        let index = lane
            .iter()
            .position(|clip| clip.id == clip_id)
            .expect("caller located the clip first");
        lane.remove(index)
    }

    /// The last beat a take clip can still play (takes spec 6.1: takes never
    /// wrap, so any span past that is silence). `None` for pattern/empty
    /// clips and for takes with no step mapping.
    fn take_clip_playable_end(&self, track: usize, clip: &ArrClip) -> Option<f64> {
        let take_id = clip.take_id?;
        let (steps_per_beat, total_len) = self.state.with_project_scenes(|scenes| {
            crate::sequencer::SongCompileContext::song_track_take_step_mapping(
                scenes, track, take_id,
            )
        })?;
        (steps_per_beat > 0.0)
            .then(|| clip.start_beat + (total_len - clip.offset_steps).max(0.0) / steps_per_beat)
    }

    /// Run `edit` against a clone of the committed arrangement inside the
    /// project's live scene context, then install and commit it as one entry.
    fn edit_arrangement(
        &mut self,
        label: &'static str,
        edit: impl FnOnce(&mut ProjectArrangement, &ProjectScenes) -> Result<(), String>,
    ) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.require_arrangement()?;
        let mut after = before.clone();
        let scenes = self.state.capture_project_scenes();
        edit(&mut after, &scenes)?;
        if after == before {
            // Exact no-op: no install, no history entry.
            return Ok(());
        }
        self.commit_arrangement_edit(label, Some(before), Some(after))
    }

    // --- clip ops (spec 8) ----------------------------------------------

    /// Create a clip on `track` over `[start_beat, end_beat)`, truncating
    /// whatever it lands on (spec 8/14). Returns the new clip's id.
    pub fn arr_clip_create(
        &mut self,
        track: usize,
        start_beat: f64,
        end_beat: f64,
        source: LaneSource,
        offset_steps: f64,
    ) -> Result<ClipId, String> {
        self.require_song_edit_unlocked()?;
        let start_beat = finite_beat("Clip start beat", start_beat)?;
        let end_beat = finite_beat("Clip end beat", end_beat)?;
        if end_beat <= start_beat {
            return Err(format!(
                "A clip must have a positive span (got [{start_beat}, {end_beat}))"
            ));
        }
        if !offset_steps.is_finite() || offset_steps < 0.0 {
            return Err(format!(
                "Clip offset must be a finite, non-negative step count (got {offset_steps})"
            ));
        }
        let before = self.require_arrangement()?;
        if track >= before.track_lanes.len() {
            return Err(format!("Track {} has no arrangement lane", track + 1));
        }
        let mut after = before.clone();
        let scenes = self.state.capture_project_scenes();
        let id = after.allocate_clip_id()?;
        let mut clip = ArrClip::new(id, start_beat, end_beat, None);
        match source {
            LaneSource::Empty => {}
            LaneSource::Pattern(pattern) => clip.pattern_id = Some(pattern.0),
            LaneSource::Take(take) => clip.take_id = Some(take.0),
        }
        clip.offset_steps = offset_steps;
        occlude_span(&mut after, &scenes, track, start_beat, end_beat)?;
        insert_clip_sorted(&mut after, track, clip);
        self.commit_arrangement_edit("Create clip", Some(before), Some(after))?;
        Ok(id)
    }

    /// Delete a clip. The lane rejoins the scene backdrop over its span
    /// (spec 6.2): a gap is not silence, it is "whatever the scene says".
    pub fn arr_clip_delete(&mut self, clip_id: ClipId) -> Result<(), String> {
        self.edit_arrangement("Delete clip", |arrangement, _scenes| {
            let (track, _) = Self::locate_clip(arrangement, clip_id)?;
            Self::take_clip(arrangement, track, clip_id);
            Ok(())
        })
    }

    /// Move a clip rigidly to `new_start_beat` (takes spec 7.4): the span
    /// length and `offset_steps` are unchanged, so the clip plays exactly the
    /// same music somewhere else. It truncates whatever it lands on.
    pub fn arr_clip_move(&mut self, clip_id: ClipId, new_start_beat: f64) -> Result<(), String> {
        let new_start_beat = finite_beat("Clip start beat", new_start_beat)?;
        self.edit_arrangement("Move clip", move |arrangement, scenes| {
            let (track, clip) = Self::locate_clip(arrangement, clip_id)?;
            if clip.start_beat == new_start_beat {
                return Ok(());
            }
            let new_end_beat = new_start_beat + (clip.end_beat - clip.start_beat);
            let mut moved = Self::take_clip(arrangement, track, clip_id);
            moved.start_beat = new_start_beat;
            moved.end_beat = new_end_beat;
            occlude_span(arrangement, scenes, track, new_start_beat, new_end_beat)?;
            insert_clip_sorted(arrangement, track, moved);
            arrangement.end_beat = arrangement.end_beat.max(new_end_beat);
            Ok(())
        })
    }

    /// Resize a clip to `[new_start_beat, new_end_beat)`.
    ///
    /// The left edge is a phase edit (spec 8): trimming re-stamps
    /// `offset_steps` by the split rule so the surviving part keeps playing
    /// what it played, and growing left runs the same arithmetic backwards.
    /// The right edge is pure occlusion — the clip loops on, it just stops
    /// later — clamped for takes, which have a finite length and nothing to
    /// play past it. Growing over a neighbour truncates the neighbour.
    pub fn arr_clip_resize(
        &mut self,
        clip_id: ClipId,
        new_start_beat: f64,
        new_end_beat: f64,
    ) -> Result<(), String> {
        let new_start_beat = finite_beat("Clip start beat", new_start_beat)?;
        let new_end_beat = finite_beat("Clip end beat", new_end_beat)?;
        if new_end_beat <= new_start_beat {
            return Err(format!(
                "A clip must have a positive span (got [{new_start_beat}, {new_end_beat}))"
            ));
        }
        let clamped_end = {
            let arrangement = self.require_arrangement()?;
            let (track, clip) = Self::locate_clip(&arrangement, clip_id)?;
            // Re-stamp first: the playable length depends on the offset the
            // trimmed clip will actually carry.
            let scenes = self.state.capture_project_scenes();
            let restamped = restamped_clip(&scenes, track, &clip, new_start_beat);
            match self.take_clip_playable_end(track, &restamped) {
                Some(limit) => new_end_beat.min(limit).max(new_start_beat),
                None => new_end_beat,
            }
        };
        if clamped_end <= new_start_beat {
            return Err(
                "Resizing this take clip to a positive span is impossible: its source has no \
                 audio left at that start beat"
                    .to_string(),
            );
        }
        self.edit_arrangement("Resize clip", move |arrangement, scenes| {
            let (track, clip) = Self::locate_clip(arrangement, clip_id)?;
            if clip.start_beat == new_start_beat && clip.end_beat == clamped_end {
                return Ok(());
            }
            Self::take_clip(arrangement, track, clip_id);
            let mut resized = restamped_clip(scenes, track, &clip, new_start_beat);
            resized.end_beat = clamped_end;
            occlude_span(arrangement, scenes, track, new_start_beat, clamped_end)?;
            insert_clip_sorted(arrangement, track, resized);
            arrangement.end_beat = arrangement.end_beat.max(clamped_end);
            Ok(())
        })
    }

    /// Split a clip at `beat` into two clips playing the same uninterrupted
    /// music: the left half keeps the id and anchor, the right half gets a
    /// fresh id and `offset += steps(beat - start)`.
    pub fn arr_clip_split(&mut self, clip_id: ClipId, beat: f64) -> Result<ClipId, String> {
        self.require_song_edit_unlocked()?;
        let beat = finite_beat("Split beat", beat)?;
        let before = self.require_arrangement()?;
        let (track, clip) = Self::locate_clip(&before, clip_id)?;
        if beat <= clip.start_beat || beat >= clip.end_beat {
            return Err(format!(
                "Cannot split clip {} at beat {beat}: the beat must fall strictly inside its \
                 span [{}, {})",
                clip_id.0, clip.start_beat, clip.end_beat
            ));
        }
        let mut after = before.clone();
        let scenes = self.state.capture_project_scenes();
        let right_id = after.allocate_clip_id()?;
        let mut right = restamped_clip(&scenes, track, &clip, beat);
        right.id = right_id;
        {
            let lane = &mut after.track_lanes[track];
            let index = lane
                .iter()
                .position(|candidate| candidate.id == clip_id)
                .expect("located above");
            lane[index].end_beat = beat;
            lane.insert(index + 1, right);
        }
        self.commit_arrangement_edit("Split clip", Some(before), Some(after))?;
        Ok(right_id)
    }

    /// Swap a clip's content in place, keeping its span and identity. The
    /// phase anchor resets to the new source's step 0: an offset measured in
    /// the old source's steps means nothing in the new one.
    pub fn arr_clip_set_source(
        &mut self,
        clip_id: ClipId,
        source: LaneSource,
    ) -> Result<(), String> {
        self.edit_arrangement("Set clip source", move |arrangement, _scenes| {
            let (track, _) = Self::locate_clip(arrangement, clip_id)?;
            let lane = &mut arrangement.track_lanes[track];
            let clip = lane
                .iter_mut()
                .find(|clip| clip.id == clip_id)
                .expect("located above");
            clip.pattern_id = None;
            clip.take_id = None;
            clip.offset_steps = 0.0;
            match source {
                LaneSource::Empty => {}
                LaneSource::Pattern(pattern) => clip.pattern_id = Some(pattern.0),
                LaneSource::Take(take) => clip.take_id = Some(take.0),
            }
            Ok(())
        })
    }

    /// Resolve the clip a timeline gesture named by position rather than by
    /// id. Until the UI read surface publishes stored clip ids (spec 12,
    /// phase 5) a gesture addresses its clip by the span it drew on: the
    /// merged view span can start before or end after the stored clip (the
    /// projection merges phase-continuous backdrop spans into it), so the
    /// search accepts the clip covering `beat` and otherwise the first clip
    /// starting inside `[beat, end_hint)`.
    pub fn arrangement_clip_at(
        &self,
        track: usize,
        beat: f64,
        end_hint: f64,
    ) -> Result<ClipId, String> {
        let arrangement = self.require_arrangement()?;
        let lane = arrangement
            .track_lanes
            .get(track)
            .ok_or_else(|| format!("Track {} has no arrangement lane", track + 1))?;
        if let Some(clip) = lane.iter().find(|clip| clip.contains(beat)) {
            return Ok(clip.id);
        }
        lane.iter()
            .find(|clip| clip.start_beat >= beat && clip.start_beat < end_hint)
            .map(|clip| clip.id)
            .ok_or_else(|| {
                format!(
                    "Track {} has no clip at beat {beat}; the timeline selection is stale",
                    track + 1
                )
            })
    }

    // --- scene-lane ops (spec 8) ----------------------------------------

    /// Insert a scene change at `beat`. Clips are untouched: a clip is opaque
    /// while it spans a beat (spec 6.2), so a new scene event only changes
    /// what the *uncovered* lanes play.
    pub fn arr_scene_event_insert(&mut self, beat: f64, scene: usize) -> Result<(), String> {
        let beat = finite_beat("Scene event beat", beat)?;
        self.edit_arrangement("Insert scene event", move |arrangement, _scenes| {
            if beat >= arrangement.end_beat {
                return Err(format!(
                    "Cannot insert a scene change at beat {beat}: the arrangement ends at beat \
                     {}; extend it first",
                    arrangement.end_beat
                ));
            }
            if arrangement
                .scene_lane
                .iter()
                .any(|event| event.start_beat == beat)
            {
                return Err(format!(
                    "A scene change already starts at beat {beat}; set or move it instead"
                ));
            }
            let position = arrangement
                .scene_lane
                .iter()
                .position(|event| event.start_beat > beat)
                .unwrap_or(arrangement.scene_lane.len());
            arrangement.scene_lane.insert(
                position,
                crate::sequencer::SceneEvent {
                    start_beat: beat,
                    scene,
                },
            );
            Ok(())
        })
    }

    /// Move the scene change at `from_beat` to `to_beat`. The event at 0.0
    /// cannot move: an arrangement always starts on a governing scene.
    pub fn arr_scene_event_move(&mut self, from_beat: f64, to_beat: f64) -> Result<(), String> {
        let from_beat = finite_beat("Scene event beat", from_beat)?;
        let to_beat = finite_beat("Scene event beat", to_beat)?;
        self.edit_arrangement("Move scene event", move |arrangement, _scenes| {
            let index = Self::scene_event_index(arrangement, from_beat)?;
            if from_beat == to_beat {
                return Ok(());
            }
            if index == 0 {
                return Err(
                    "Moving the first scene change away from beat 0.0 is rejected: the \
                     arrangement must start on a scene"
                        .to_string(),
                );
            }
            if to_beat <= 0.0 {
                return Err(
                    "Cannot move a scene change onto beat 0.0: the first scene change already \
                     starts there"
                        .to_string(),
                );
            }
            if to_beat >= arrangement.end_beat {
                return Err(format!(
                    "Cannot move the scene change to beat {to_beat}: the arrangement ends at \
                     beat {}; extend it first",
                    arrangement.end_beat
                ));
            }
            if arrangement
                .scene_lane
                .iter()
                .any(|event| event.start_beat == to_beat)
            {
                return Err(format!(
                    "Cannot move the scene change to beat {to_beat}: another scene change \
                     already starts there"
                ));
            }
            arrangement.scene_lane[index].start_beat = to_beat;
            arrangement.scene_lane.sort_by(|a, b| {
                a.start_beat
                    .partial_cmp(&b.start_beat)
                    .expect("scene event beats are finite")
            });
            Ok(())
        })
    }

    /// Point the scene change at `beat` at a different scene.
    pub fn arr_scene_event_set(&mut self, beat: f64, scene: usize) -> Result<(), String> {
        let beat = finite_beat("Scene event beat", beat)?;
        self.edit_arrangement("Set scene event", move |arrangement, _scenes| {
            let index = Self::scene_event_index(arrangement, beat)?;
            arrangement.scene_lane[index].scene = scene;
            Ok(())
        })
    }

    /// Remove the scene change at `beat`: the predecessor's scene extends over
    /// it (spec 8/14, locked). Clips can never be touched by this — that is
    /// the whole user-visible point of the lane model. Removing the event at
    /// 0.0 is rejected, exactly as removing row zero was.
    pub fn arr_scene_event_remove(&mut self, beat: f64) -> Result<(), String> {
        let beat = finite_beat("Scene event beat", beat)?;
        self.edit_arrangement("Remove scene event", move |arrangement, _scenes| {
            let index = Self::scene_event_index(arrangement, beat)?;
            if index == 0 {
                return Err(
                    "Removing the first scene change is rejected: the arrangement must start on \
                     a scene at beat 0.0"
                        .to_string(),
                );
            }
            arrangement.scene_lane.remove(index);
            Ok(())
        })
    }

    fn scene_event_index(arrangement: &ProjectArrangement, beat: f64) -> Result<usize, String> {
        arrangement
            .scene_lane
            .iter()
            .position(|event| event.start_beat == beat)
            .ok_or_else(|| format!("No scene change starts at beat {beat}"))
    }

    // --- whole-arrangement ops ------------------------------------------

    /// Set the arrangement's explicit end beat. Shrinking past the last clip
    /// (or the last scene change) is refused rather than silently dropping
    /// content (spec 15, v1 decision); the UI clamps the handle.
    pub fn arr_set_end(&mut self, end_beat: f64) -> Result<(), String> {
        let end_beat = finite_beat("Arrangement end beat", end_beat)?;
        self.edit_arrangement("Set song end", move |arrangement, _scenes| {
            if end_beat <= 0.0 {
                return Err(format!(
                    "Arrangement end beat {end_beat} must be greater than zero"
                ));
            }
            let last_clip_end = arrangement
                .track_lanes
                .iter()
                .flatten()
                .map(|clip| clip.end_beat)
                .fold(0.0f64, f64::max);
            if end_beat < last_clip_end {
                return Err(format!(
                    "Cannot shorten the arrangement to beat {end_beat}: a clip runs to beat \
                     {last_clip_end}; trim or delete it first"
                ));
            }
            let last_scene_start = arrangement
                .scene_lane
                .last()
                .map(|event| event.start_beat)
                .unwrap_or(0.0);
            if end_beat <= last_scene_start {
                return Err(format!(
                    "Cannot shorten the arrangement to beat {end_beat}: a scene change starts at \
                     beat {last_scene_start}; remove it first"
                ));
            }
            arrangement.end_beat = end_beat;
            Ok(())
        })
    }

    /// Enable or disable arrangement looping.
    pub fn arr_set_loop(&mut self, enabled: bool) -> Result<(), String> {
        self.edit_arrangement("Set song loop", move |arrangement, _scenes| {
            arrangement.loop_enabled = enabled;
            Ok(())
        })
    }
}

impl App {
    /// Restore the committed arrangement to `target` (undo/redo replay).
    /// Recompiles, so the compiled song is restored with it and the two can
    /// never come back out of step.
    pub(crate) fn restore_committed_arrangement_state(
        &mut self,
        target: &Option<ProjectArrangement>,
    ) -> Result<(), String> {
        self.state
            .set_committed_arrangement(target.clone())
            .map_err(|error| format!("Arrangement history no longer matches the project: {error}"))
    }

    /// Shared primitive tail: install the candidate (validating and
    /// recompiling it) and commit exactly one history entry. On any error the
    /// committed arrangement is untouched and no history entry is created.
    pub(super) fn commit_arrangement_edit(
        &mut self,
        label: &'static str,
        before: Option<ProjectArrangement>,
        after: Option<ProjectArrangement>,
    ) -> Result<(), String> {
        self.state.set_committed_arrangement(after.clone())?;
        finish_active_gesture(self);
        let patch = ArrangementStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history
            .commit(label, None, EditPatch::Arrangement(patch), retained_bytes);
        Ok(())
    }

    /// Replace the arrangement wholesale (`def-song`, later capture commit).
    pub fn arr_replace(&mut self, arrangement: ProjectArrangement) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.state.committed_arrangement();
        self.commit_arrangement_edit("Replace arrangement", before, Some(arrangement))
    }

    /// Remove the committed arrangement (and with it the compiled song)
    /// entirely.
    pub fn arr_clear(&mut self) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.state.committed_arrangement();
        if before.is_none() {
            return Err("The project has no arrangement to clear".to_string());
        }
        self.commit_arrangement_edit("Clear arrangement", before, None)
    }

    /// Lower a declarative row list (`def-song`, spec 8 last paragraph) to an
    /// arrangement and commit it. Clip ids continue from the previous
    /// arrangement's allocator (`next_clip_id` is monotonic within a project
    /// and ids are never reused).
    pub fn arr_replace_rows(
        &mut self,
        rows: Vec<SongRowSpec>,
        end_beat: f64,
        loop_enabled: bool,
    ) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let next_clip_id = self
            .state
            .committed_arrangement()
            .map(|arrangement| arrangement.next_clip_id)
            .unwrap_or(0);
        let arrangement = self.state.with_project_scenes(|scenes| {
            lower_row_specs_to_arrangement(
                rows,
                end_beat,
                loop_enabled,
                self.tracks.len(),
                next_clip_id,
                scenes,
            )
        })?;
        let before = self.state.committed_arrangement();
        self.commit_arrangement_edit("Replace arrangement", before, Some(arrangement))
    }
}

/// Validate a declarative row list (`def-song`) exactly as the retired
/// `song_replace` primitive did, then lower it to lanes with
/// `lower_rows_to_arrangement`.
///
/// What lives here is the *declarative* input contract: `SongRowSpec` is
/// unvalidated caller data, while the lowering assumes everything
/// `ProjectSong::validate` guarantees. Each check below is a verbatim port of
/// the row path's, message included, so a definition the row path refused is
/// refused identically here.
fn lower_row_specs_to_arrangement<C: crate::sequencer::ArrangementContext>(
    rows: Vec<SongRowSpec>,
    end_beat: f64,
    loop_enabled: bool,
    track_count: usize,
    next_clip_id: u64,
    ctx: &C,
) -> Result<ProjectArrangement, String> {
    if rows.is_empty() {
        return Err(
            "def-song requires at least one row; use song-clear to remove the arrangement"
                .to_string(),
        );
    }
    let mut rows = rows;
    for row in &rows {
        if !row.start_beat.is_finite() || row.start_beat < 0.0 {
            return Err(format!(
                "Song row start beat must be a finite, non-negative beat (got {})",
                row.start_beat
            ));
        }
    }
    for row in &mut rows {
        row.overrides.sort_by_key(|over| over.track);
        for pair in row.overrides.windows(2) {
            if pair[0].track == pair[1].track {
                return Err(format!(
                    "More than one override was given for track {}",
                    pair[0].track + 1
                ));
            }
        }
    }
    rows.sort_by(|a, b| {
        a.start_beat
            .partial_cmp(&b.start_beat)
            .expect("row start beats are finite")
    });
    for (idx, pair) in rows.windows(2).enumerate() {
        if pair[0].scene == pair[1].scene && pair[0].overrides == pair[1].overrides {
            return Err(format!(
                "Song rows {} and {} contain identical launch states; \
                 normalization removes the redundant later row",
                idx + 1,
                idx + 2
            ));
        }
    }

    // Row ids are irrelevant to the lowering (clips get their own identity),
    // so the placeholder id below is never read.
    let rows: Vec<ProjectSongRow> = rows
        .into_iter()
        .map(|spec| ProjectSongRow {
            id: SongRowId(0),
            start_beat: spec.start_beat,
            scene: spec.scene,
            overrides: spec.overrides,
        })
        .collect();
    lower_rows_to_arrangement(&rows, end_beat, loop_enabled, track_count, next_clip_id, ctx)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::app::edit::{redo, undo};
    use crate::app::history::HistoryReplay;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        compile_arrangement, default_empty_effect_chain, ArrClip, ClipId, LaneSource, PatternId,
        PatternSnapshot, ProjectSong, ProjectSongRow, ProjectSongTrackOverride, SceneEvent,
        SequencerState, SongRowId,
    };

    /// Two-track app with three scenes; per-track pattern-pool ids are 1..=3
    /// with scene j's cell holding PatternId(j + 1). Every pattern is the
    /// default 16 sixteenth-note steps, so a pattern is four beats long.
    fn test_app() -> App {
        let state = SequencerState::new(2, vec![default_empty_effect_chain(), default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ],
            0,
        );
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        app
    }

    fn ov(track: usize, pattern_id: u64) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride::new(track, Some(pattern_id))
    }

    fn spec(start_beat: f64, scene: usize, overrides: Vec<ProjectSongTrackOverride>) -> SongRowSpec {
        SongRowSpec {
            start_beat,
            scene,
            overrides,
        }
    }

    /// Compile `arrangement` the way `set_committed_arrangement` does.
    fn compiled(app: &App, arrangement: &ProjectArrangement) -> ProjectSong {
        compile_arrangement(arrangement, &app.state.capture_project_scenes())
            .expect("arrangement compiles")
    }

    /// The invariant `set_committed_arrangement` exists to hold (spec 7).
    #[track_caller]
    fn assert_song_matches_arrangement(app: &App) {
        match app.state.committed_arrangement() {
            Some(arrangement) => assert_eq!(
                app.state.committed_song(),
                Some(compiled(app, &arrangement)),
                "the committed song must be the compiled arrangement"
            ),
            None => assert_eq!(app.state.committed_song(), None),
        }
    }

    #[test]
    fn set_committed_arrangement_installs_the_compiled_song_and_bumps_the_revision() {
        let app = test_app();
        let arrangement = ProjectArrangement {
            scene_lane: vec![SceneEvent {
                start_beat: 0.0,
                scene: 0,
            }],
            track_lanes: vec![vec![ArrClip::new(ClipId(0), 4.0, 8.0, Some(2))], Vec::new()],
            end_beat: 16.0,
            loop_enabled: true,
            next_clip_id: 1,
        };
        let revision = app.state.committed_song_revision();
        app.state
            .set_committed_arrangement(Some(arrangement.clone()))
            .expect("arrangement installs");
        assert_eq!(app.state.committed_arrangement(), Some(arrangement.clone()));
        assert_eq!(
            app.state.committed_song(),
            Some(compiled(&app, &arrangement))
        );
        assert!(app.state.committed_song_revision() > revision);

        // Clearing clears both.
        app.state
            .set_committed_arrangement(None)
            .expect("clearing always succeeds");
        assert_eq!(app.state.committed_arrangement(), None);
        assert_eq!(app.state.committed_song(), None);
    }

    /// A compile failure must install nothing: song and arrangement can never
    /// be left disagreeing.
    #[test]
    fn set_committed_arrangement_installs_nothing_when_the_compile_fails() {
        let app = test_app();
        let good = ProjectArrangement::new(2, 16.0);
        app.state
            .set_committed_arrangement(Some(good.clone()))
            .expect("empty arrangement installs");
        let song = app.state.committed_song();

        let mut bad = good.clone();
        bad.scene_lane[0].scene = 9; // no such scene
        let error = app
            .state
            .set_committed_arrangement(Some(bad))
            .expect_err("an invalid arrangement must be rejected");
        assert!(error.contains("references scene 10"), "{error}");
        assert_eq!(app.state.committed_arrangement(), Some(good));
        assert_eq!(app.state.committed_song(), song);
    }

    /// The row model is compiled output, not an authoring path: installing a
    /// song directly (project reset, playback tests) must not leave an
    /// arrangement standing that did not compile to it.
    #[test]
    fn set_committed_song_clears_the_arrangement() {
        let app = test_app();
        app.state
            .set_committed_arrangement(Some(ProjectArrangement::new(2, 16.0)))
            .expect("arrangement installs");
        let song = ProjectSong {
            rows: vec![ProjectSongRow {
                id: SongRowId(0),
                start_beat: 0.0,
                scene: 0,
                overrides: vec![ov(0, 2)],
            }],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 1,
        };
        app.state.set_committed_song(Some(song.clone()));
        assert_eq!(app.state.committed_song(), Some(song));
        assert_eq!(
            app.state.committed_arrangement(),
            None,
            "a directly installed song can never be the compile of a stored arrangement"
        );

        app.state.set_committed_song(None);
        assert_eq!(app.state.committed_song(), None);
        assert_eq!(app.state.committed_arrangement(), None);
    }

    #[test]
    fn arr_replace_and_clear_commit_one_entry_each_and_undo_restores_both() {
        let mut app = test_app();
        let arrangement = ProjectArrangement {
            scene_lane: vec![
                SceneEvent {
                    start_beat: 0.0,
                    scene: 0,
                },
                SceneEvent {
                    start_beat: 8.0,
                    scene: 1,
                },
            ],
            track_lanes: vec![vec![ArrClip::new(ClipId(0), 2.0, 6.0, Some(3))], Vec::new()],
            end_beat: 16.0,
            loop_enabled: false,
            next_clip_id: 1,
        };
        let depth = app.history.undo_len();
        app.arr_replace(arrangement.clone()).expect("arr_replace");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one entry");
        let song = app.state.committed_song().expect("compiled song installed");
        assert_song_matches_arrangement(&app);

        app.arr_clear().expect("arr_clear");
        assert_eq!(app.state.committed_arrangement(), None);
        assert_eq!(app.state.committed_song(), None);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)), "undo the clear");
        assert_eq!(app.state.committed_arrangement(), Some(arrangement.clone()));
        assert_eq!(app.state.committed_song(), Some(song.clone()));

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)), "undo the replace");
        assert_eq!(app.state.committed_arrangement(), None);
        assert_eq!(app.state.committed_song(), None);

        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)), "redo the replace");
        assert_eq!(app.state.committed_arrangement(), Some(arrangement));
        assert_eq!(app.state.committed_song(), Some(song));
        assert_song_matches_arrangement(&app);

        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)), "redo the clear");
        assert_eq!(app.state.committed_arrangement(), None);
        assert_eq!(app.state.committed_song(), None);
    }

    #[test]
    fn arr_clear_on_an_empty_project_is_rejected_without_a_history_entry() {
        let mut app = test_app();
        let depth = app.history.undo_len();
        let error = app.arr_clear().expect_err("nothing to clear");
        assert!(error.contains("no arrangement"), "{error}");
        assert_eq!(app.history.undo_len(), depth);
    }

    #[test]
    fn arrangement_primitives_are_rejected_while_song_editing_is_locked() {
        let mut app = test_app();
        app.song_transport_locks_edits = true;
        let error = app
            .arr_replace(ProjectArrangement::new(2, 16.0))
            .expect_err("locked");
        assert_eq!(error, crate::app::song_edit::SONG_EDITS_LOCKED_ERROR);
        let error = app.arr_clear().expect_err("locked");
        assert_eq!(error, crate::app::song_edit::SONG_EDITS_LOCKED_ERROR);
        let error = app
            .arr_replace_rows(vec![spec(0.0, 0, Vec::new())], 16.0, false)
            .expect_err("locked");
        assert_eq!(error, crate::app::song_edit::SONG_EDITS_LOCKED_ERROR);
        assert_eq!(app.state.committed_arrangement(), None);
    }

    /// The `def-song` contract (spec 8): lowering the declarative rows and
    /// compiling the result must reproduce exactly the `ProjectSong` the row
    /// path produced — same beats, same scenes, same overrides.
    /// The `ProjectSong` the declarative rows describe verbatim — what the
    /// retired row primitive built: sorted by start beat, overrides in
    /// canonical track order, ids positional.
    fn song_from_specs(rows: &[SongRowSpec], end_beat: f64, loop_enabled: bool) -> ProjectSong {
        let mut sorted = rows.to_vec();
        sorted.sort_by(|a, b| {
            a.start_beat
                .partial_cmp(&b.start_beat)
                .expect("row start beats are finite")
        });
        ProjectSong {
            rows: sorted
                .into_iter()
                .enumerate()
                .map(|(idx, spec)| {
                    let mut overrides = spec.overrides;
                    overrides.sort_by_key(|over| over.track);
                    ProjectSongRow {
                        id: SongRowId(idx as u64),
                        start_beat: spec.start_beat,
                        scene: spec.scene,
                        overrides,
                    }
                })
                .collect(),
            end_beat,
            loop_enabled,
            next_row_id: rows.len() as u64,
        }
    }

    #[track_caller]
    fn assert_def_song_round_trips(rows: Vec<SongRowSpec>, end_beat: f64, loop_enabled: bool) {
        let from_rows = song_from_specs(&rows, end_beat, loop_enabled);

        let mut arr_app = test_app();
        arr_app
            .arr_replace_rows(rows, end_beat, loop_enabled)
            .expect("arr_replace_rows succeeds");
        let from_lanes = arr_app.state.committed_song().expect("compiled song");

        // Row ids are positional in the compiled model and allocated by the
        // row path, so compare everything else field by field.
        assert_eq!(from_lanes.end_beat, from_rows.end_beat);
        assert_eq!(from_lanes.loop_enabled, from_rows.loop_enabled);
        let lanes: Vec<_> = from_lanes
            .rows
            .iter()
            .map(|row| (row.start_beat, row.scene, row.overrides.clone()))
            .collect();
        let rows: Vec<_> = from_rows
            .rows
            .iter()
            .map(|row| (row.start_beat, row.scene, row.overrides.clone()))
            .collect();
        assert_eq!(lanes, rows, "def-song must compile back to the same song");
    }

    #[test]
    fn def_song_lowering_round_trips_to_the_row_path_song() {
        // Scene changes only.
        assert_def_song_round_trips(
            vec![
                spec(0.0, 0, Vec::new()),
                spec(4.0, 1, Vec::new()),
                spec(8.0, 2, Vec::new()),
            ],
            16.0,
            true,
        );
        // Overrides that persist across rows, are dropped, and go explicitly
        // empty — every boundary is a whole number of four-beat patterns, so
        // no backdrop phase is materialized.
        assert_def_song_round_trips(
            vec![
                spec(0.0, 0, Vec::new()),
                spec(16.0, 0, vec![ov(0, 2)]),
                spec(24.0, 0, vec![ov(0, 2), ProjectSongTrackOverride::new(1, None)]),
                spec(32.0, 0, vec![ProjectSongTrackOverride::new(1, None)]),
                spec(40.0, 0, Vec::new()),
            ],
            48.0,
            false,
        );
        // A scene that repeats after a change is a second scene event.
        assert_def_song_round_trips(
            vec![
                spec(0.0, 0, Vec::new()),
                spec(8.0, 1, vec![ov(1, 3)]),
                spec(16.0, 0, vec![ov(1, 3)]),
            ],
            24.0,
            false,
        );
    }

    /// The clip merge rule: a repeated override merges into the open clip
    /// only when the clip would compile to exactly that override, so a
    /// re-anchored (retriggering) row stays a row.
    #[test]
    fn def_song_lowering_merges_phase_continuous_rows_and_splits_retriggers() {
        // Patterns are four beats; rows eight beats apart, so the open clip's
        // phase at the second row is 0 — exactly the declared override, so
        // the two rows are one clip. (The rows differ by scene because the
        // row model rejects adjacent identical launch states.)
        let mut app = test_app();
        app.arr_replace_rows(
            vec![
                spec(0.0, 0, vec![ov(0, 2)]),
                spec(8.0, 1, vec![ov(0, 2)]),
                spec(16.0, 1, Vec::new()),
            ],
            24.0,
            false,
        )
        .expect("lowers");
        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(
            arrangement.track_lanes[0].len(),
            1,
            "phase-continuous rows merge into one clip: {:?}",
            arrangement.track_lanes[0]
        );
        assert_eq!(arrangement.track_lanes[0][0].start_beat, 0.0);
        assert_eq!(arrangement.track_lanes[0][0].end_beat, 16.0);

        // Two beats apart the pattern is mid-cycle, so re-declaring it at
        // offset 0 is a retrigger and must stay two clips. (The rows differ
        // by scene: the row model rejects adjacent identical launch states,
        // and so does the lowering.)
        let rows = vec![
            spec(0.0, 0, vec![ov(0, 2)]),
            spec(2.0, 1, vec![ov(0, 2)]),
            spec(16.0, 1, Vec::new()),
        ];
        let mut app = test_app();
        app.arr_replace_rows(rows.clone(), 24.0, false)
            .expect("lowers");
        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(
            arrangement.track_lanes[0].len(),
            2,
            "a retrigger keeps its own clip: {:?}",
            arrangement.track_lanes[0]
        );
        assert_eq!(arrangement.track_lanes[0][0].start_beat, 0.0);
        assert_eq!(arrangement.track_lanes[0][0].end_beat, 2.0);
        assert_eq!(arrangement.track_lanes[0][1].start_beat, 2.0);
        assert_eq!(arrangement.track_lanes[0][1].offset_steps, 0.0);
        let _ = rows;
    }

    /// The one intended divergence from the row path (spec 4, goal 3): a lane
    /// riding the scene backdrop keeps its phase across a boundary instead of
    /// retriggering from step 0. The row model gives backdrop lanes no phase
    /// memory, so compile materializes the offset; a def-song whose rows are
    /// not a whole number of pattern loops from the governing scene event
    /// therefore gains overrides the row path did not emit.
    #[test]
    fn def_song_lowering_materializes_the_scene_backdrop_phase_rows_dropped() {
        let rows = vec![
            spec(0.0, 0, vec![ov(0, 2)]),
            spec(2.0, 1, vec![ov(0, 2)]),
            spec(16.0, 1, Vec::new()),
        ];
        let mut app = test_app();
        app.arr_replace_rows(rows, 24.0, false).expect("lowers");
        let song = app.state.committed_song().expect("compiled song");
        let last = song.rows.last().expect("a row at beat 16");
        assert_eq!(last.start_beat, 16.0);
        // Scene 1 launched at beat 2; 14 beats later both lanes are
        // 56 steps == 8 steps (mod the 16-step pattern) into its cells.
        assert_eq!(
            last.overrides
                .iter()
                .map(|over| (over.track, over.pattern_id, over.offset_steps))
                .collect::<Vec<_>>(),
            vec![(0, Some(2), 8.0), (1, Some(2), 8.0)]
        );
        assert_song_matches_arrangement(&app);
    }

    #[test]
    fn def_song_lowering_rejects_rows_at_or_past_the_end_beat() {
        let mut app = test_app();
        let rows = vec![spec(0.0, 0, Vec::new()), spec(100.0, 1, vec![ov(0, 2)])];
        let error = app
            .arr_replace_rows(rows, 50.0, false)
            .expect_err("a row past the end must be rejected, not silently dropped");
        assert!(error.contains("greater than the last row's start beat"), "{error}");
        assert_eq!(app.state.committed_arrangement(), None);
    }

    /// The row model rejects adjacent identical launch states; the lowering
    /// must reject exactly the same definitions.
    #[test]
    fn def_song_lowering_rejects_adjacent_identical_rows() {
        let mut app = test_app();
        let error = app
            .arr_replace_rows(
                vec![spec(0.0, 0, vec![ov(0, 2)]), spec(2.0, 0, vec![ov(0, 2)])],
                16.0,
                false,
            )
            .expect_err("identical adjacent rows are rejected");
        assert!(error.contains("identical launch states"), "{error}");
        assert_eq!(app.state.committed_arrangement(), None);
    }

    #[test]
    fn def_song_lowering_collapses_the_scene_lane_to_actual_changes() {
        let mut app = test_app();
        app.arr_replace_rows(
            vec![
                spec(0.0, 0, Vec::new()),
                spec(8.0, 0, vec![ov(0, 2)]),
                spec(16.0, 1, vec![ov(0, 2)]),
                spec(24.0, 1, Vec::new()),
            ],
            32.0,
            false,
        )
        .expect("lowers");
        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(
            arrangement
                .scene_lane
                .iter()
                .map(|event| (event.start_beat, event.scene))
                .collect::<Vec<_>>(),
            vec![(0.0, 0), (16.0, 1)],
            "one event per scene *change*, not one per row"
        );
        assert_song_matches_arrangement(&app);
    }

    #[test]
    fn def_song_lowering_rejects_duplicate_track_overrides() {
        let mut app = test_app();
        let error = app
            .arr_replace_rows(
                vec![spec(0.0, 0, vec![ov(0, 2), ov(0, 3)])],
                16.0,
                false,
            )
            .expect_err("duplicate overrides are rejected");
        assert!(error.contains("More than one override"), "{error}");
        assert_eq!(app.state.committed_arrangement(), None);
    }

    #[test]
    fn def_song_lowering_continues_the_clip_id_allocator() {
        let mut app = test_app();
        app.arr_replace_rows(
            vec![spec(0.0, 0, vec![ov(0, 2)]), spec(2.0, 0, Vec::new())],
            16.0,
            false,
        )
        .expect("lowers");
        let first = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(first.next_clip_id, 1);
        app.arr_replace_rows(
            vec![spec(0.0, 0, vec![ov(1, 2)]), spec(2.0, 0, Vec::new())],
            16.0,
            false,
        )
        .expect("lowers again");
        let second = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(second.track_lanes[1][0].id, ClipId(1), "ids never reused");
        assert_eq!(second.next_clip_id, 2);
    }
    // --- clip + scene primitives (spec 8) -------------------------------

    /// A two-track app holding one scene event and, on track 0, the clips
    /// `[0,4)` P1 and `[8,16)` P2 — a lane with a gap in the middle, which
    /// is where the interesting truncation cases live.
    fn app_with_clips() -> App {
        let mut app = test_app();
        let mut arrangement = ProjectArrangement::new(2, 32.0);
        for (start, end, pattern) in [(0.0, 4.0, 1u64), (8.0, 16.0, 2)] {
            let id = arrangement.allocate_clip_id().expect("clip id");
            arrangement.track_lanes[0].push(ArrClip::new(id, start, end, Some(pattern)));
        }
        app.arr_replace(arrangement).expect("arrangement installs");
        app
    }

    fn lane(app: &App, track: usize) -> Vec<(f64, f64, Option<u64>, f64)> {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[track]
            .iter()
            .map(|clip| (clip.start_beat, clip.end_beat, clip.pattern_id, clip.offset_steps))
            .collect()
    }

    fn scene_lane(app: &App) -> Vec<(f64, usize)> {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .scene_lane
            .iter()
            .map(|event| (event.start_beat, event.scene))
            .collect()
    }

    /// Every primitive is exactly one history entry, and undo restores BOTH
    /// the arrangement and the compiled song (they can never drift apart).
    #[track_caller]
    fn assert_one_entry_and_undoable(app: &mut App, run: impl FnOnce(&mut App)) {
        let arrangement_before = app.state.committed_arrangement();
        let song_before = app.state.committed_song();
        let depth = app.history.undo_len();
        run(app);
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one history entry");
        let arrangement_after = app.state.committed_arrangement();
        let song_after = app.state.committed_song();
        assert_ne!(arrangement_after, arrangement_before, "the edit did something");
        assert_song_matches_arrangement(app);

        assert!(matches!(undo(app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_arrangement(), arrangement_before);
        assert_eq!(app.state.committed_song(), song_before);
        assert!(matches!(redo(app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_arrangement(), arrangement_after);
        assert_eq!(app.state.committed_song(), song_after);
    }

    #[test]
    fn clip_create_lands_in_the_gap_as_one_entry() {
        let mut app = app_with_clips();
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_clip_create(0, 4.0, 8.0, LaneSource::Pattern(PatternId(3)), 0.0)
                .expect("clip creates");
        });
        assert_eq!(
            lane(&app, 0),
            vec![
                (0.0, 4.0, Some(1), 0.0),
                (4.0, 8.0, Some(3), 0.0),
                (8.0, 16.0, Some(2), 0.0),
            ]
        );
    }

    /// Spec 14, locked: the incoming clip always wins. One create exercises
    /// all four truncation cases at once.
    #[test]
    fn clip_create_truncates_everything_it_lands_on() {
        let mut app = test_app();
        let mut arrangement = ProjectArrangement::new(2, 64.0);
        for (start, end) in [(0.0, 8.0), (8.0, 12.0), (12.0, 16.0), (16.0, 32.0)] {
            let id = arrangement.allocate_clip_id().expect("clip id");
            arrangement.track_lanes[0].push(ArrClip::new(id, start, end, Some(1)));
        }
        app.arr_replace(arrangement).expect("installs");

        app.arr_clip_create(0, 6.0, 20.0, LaneSource::Pattern(PatternId(3)), 0.0)
            .expect("clip creates");
        assert_eq!(
            lane(&app, 0),
            vec![
                // right-trimmed at the incoming start (anchor untouched)
                (0.0, 6.0, Some(1), 0.0),
                (6.0, 20.0, Some(3), 0.0),
                // left-trimmed at the incoming end; 4 beats == 16 steps,
                // which wraps to 0 in the 16-step pattern
                (20.0, 32.0, Some(1), 0.0),
            ],
            "the two fully covered clips are gone, the edges trimmed"
        );
    }

    /// A create landing strictly inside one clip splits it around itself,
    /// and the right fragment re-stamps its phase.
    #[test]
    fn clip_create_splits_a_clip_it_lands_inside() {
        let mut app = test_app();
        let mut arrangement = ProjectArrangement::new(2, 32.0);
        let id = arrangement.allocate_clip_id().expect("clip id");
        arrangement.track_lanes[0].push(ArrClip::new(id, 0.0, 16.0, Some(1)));
        app.arr_replace(arrangement).expect("installs");

        app.arr_clip_create(0, 6.0, 10.0, LaneSource::Pattern(PatternId(3)), 0.0)
            .expect("clip creates");
        assert_eq!(
            lane(&app, 0),
            vec![
                (0.0, 6.0, Some(1), 0.0),
                (6.0, 10.0, Some(3), 0.0),
                // 10 beats == 40 steps, 40 mod 16 == 8
                (10.0, 16.0, Some(1), 8.0),
            ]
        );
    }

    #[test]
    fn clip_delete_removes_one_object_and_leaves_the_rest() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_clip_delete(id).expect("clip deletes");
        });
        assert_eq!(lane(&app, 0), vec![(0.0, 4.0, Some(1), 0.0)]);
        let error = app.arr_clip_delete(id).expect_err("already gone");
        assert!(error.contains("does not exist"), "{error}");
    }

    /// Takes spec 7.4: a move is RIGID — same length, same offset, so the
    /// clip plays exactly the same music somewhere else.
    #[test]
    fn clip_move_is_rigid_and_truncates_what_it_lands_on() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        app.arr_clip_resize(id, 8.0, 16.0).expect("no-op resize");
        // Give it a nonzero anchor so rigidity is observable.
        app.arr_clip_split(id, 10.0).expect("split");
        let right = app.arrangement_clip_at(0, 10.0, 10.0).expect("right half");
        assert_eq!(lane(&app, 0)[2], (10.0, 16.0, Some(2), 8.0));

        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_clip_move(right, 2.0).expect("clip moves");
        });
        assert_eq!(
            lane(&app, 0),
            vec![
                // the [0,4) clip is right-trimmed by the arrival
                (0.0, 2.0, Some(1), 0.0),
                (2.0, 8.0, Some(2), 8.0),
                (8.0, 10.0, Some(2), 0.0),
            ],
            "the moved clip keeps its length and its offset"
        );
    }

    /// Spec 8: a left trim re-stamps the phase (the clip keeps playing what
    /// it played there); the right edge is pure occlusion.
    #[test]
    fn clip_resize_restamps_the_left_edge_and_occludes_the_right() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_clip_resize(id, 10.0, 14.0).expect("clip resizes");
        });
        // 2 beats into the clip == 8 sixteenth steps.
        assert_eq!(lane(&app, 0)[1], (10.0, 14.0, Some(2), 8.0));

        // Growing left runs the same arithmetic backwards, and growing right
        // truncates the neighbour it eats into.
        app.arr_clip_resize(id, 2.0, 20.0).expect("clip grows");
        assert_eq!(
            lane(&app, 0),
            vec![(0.0, 2.0, Some(1), 0.0), (2.0, 20.0, Some(2), 8.0)],
            "8 beats back == -32 steps, and 8 - 32 wraps to 8 in 16 steps"
        );
    }

    /// A take has a finite length, so the right edge clamps to what is left
    /// of it rather than trailing silence (spec 8).
    #[test]
    fn clip_resize_clamps_a_take_to_its_playable_length() {
        let mut app = test_app();
        let chunk = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].get(PatternId(1)).cloned())
            .expect("pool pattern");
        // 40 steps at four per beat == 10 beats of content.
        let take = app
            .state
            .register_track_take(0, Some("Take".to_string()), vec![chunk], 40)
            .expect("take registers");
        let mut arrangement = ProjectArrangement::new(2, 64.0);
        let id = arrangement.allocate_clip_id().expect("clip id");
        arrangement.track_lanes[0].push(ArrClip::new_take(id, 0.0, 4.0, take.0, 0.0));
        app.arr_replace(arrangement).expect("installs");

        app.arr_clip_resize(id, 0.0, 32.0).expect("clip grows");
        assert_eq!(
            app.state.committed_arrangement().unwrap().track_lanes[0][0].end_beat,
            10.0,
            "a take clip cannot be longer than the take"
        );
    }

    #[test]
    fn clip_split_keeps_the_music_uninterrupted() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        let depth = app.history.undo_len();
        let right = app.arr_clip_split(id, 10.0).expect("clip splits");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one entry");
        assert_eq!(
            lane(&app, 0),
            vec![
                (0.0, 4.0, Some(1), 0.0),
                (8.0, 10.0, Some(2), 0.0),
                // 2 beats == 8 sixteenth steps into the pattern
                (10.0, 16.0, Some(2), 8.0),
            ]
        );
        assert_ne!(right, id, "the right half is its own object");
        assert_song_matches_arrangement(&app);

        // A split beat outside the clip is rejected, not clamped.
        for beat in [8.0, 16.0, 20.0] {
            let error = app.arr_clip_split(id, beat).expect_err("outside the span");
            assert!(error.contains("strictly inside"), "{error}");
        }
    }

    #[test]
    fn clip_set_source_swaps_content_in_place() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        app.arr_clip_split(id, 10.0).expect("split for a nonzero anchor");
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_clip_set_source(id, LaneSource::Pattern(PatternId(3)))
                .expect("source swaps");
        });
        assert_eq!(
            lane(&app, 0)[1],
            (8.0, 10.0, Some(3), 0.0),
            "the span survives, the anchor resets to the new source's step 0"
        );

        // Explicit-empty is a source like any other: silence that still
        // occludes the scene backdrop.
        app.arr_clip_set_source(id, LaneSource::Empty)
            .expect("goes silent");
        assert_eq!(lane(&app, 0)[1], (8.0, 10.0, None, 0.0));
    }

    // --- scene lane -----------------------------------------------------

    #[test]
    fn scene_events_insert_move_set_and_remove() {
        let mut app = app_with_clips();
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_scene_event_insert(8.0, 1).expect("insert");
        });
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1)]);

        app.arr_scene_event_set(8.0, 2).expect("set");
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 2)]);

        app.arr_scene_event_move(8.0, 20.0).expect("move");
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (20.0, 2)]);

        app.arr_scene_event_remove(20.0).expect("remove");
        assert_eq!(scene_lane(&app), vec![(0.0, 0)]);
        assert_song_matches_arrangement(&app);
    }

    /// The user-visible point of the lane model (spec 4 goal 2): removing a
    /// scene change merges its span into the predecessor and CANNOT touch a
    /// clip. Removing the event at 0.0 is rejected, exactly as removing row
    /// zero was.
    #[test]
    fn scene_event_remove_merges_into_the_predecessor_and_never_touches_clips() {
        let mut app = app_with_clips();
        app.arr_scene_event_insert(8.0, 1).expect("insert");
        let clips_before = lane(&app, 0);

        app.arr_scene_event_remove(8.0).expect("remove");
        assert_eq!(scene_lane(&app), vec![(0.0, 0)], "the span merges back");
        assert_eq!(lane(&app, 0), clips_before, "the clips are untouched");

        let depth = app.history.undo_len();
        let error = app.arr_scene_event_remove(0.0).expect_err("row zero rule");
        assert!(error.contains("must start on a scene"), "{error}");
        assert_eq!(app.history.undo_len(), depth, "a rejection commits nothing");
    }

    #[test]
    fn scene_event_ops_reject_collisions_and_missing_events() {
        let mut app = app_with_clips();
        app.arr_scene_event_insert(8.0, 1).expect("insert");

        let error = app.arr_scene_event_insert(8.0, 2).expect_err("collision");
        assert!(error.contains("already starts at beat 8"), "{error}");
        let error = app.arr_scene_event_move(8.0, 0.0).expect_err("onto zero");
        assert!(error.contains("beat 0.0"), "{error}");
        let error = app.arr_scene_event_move(0.0, 4.0).expect_err("event zero");
        assert!(error.contains("must start on a scene"), "{error}");
        let error = app.arr_scene_event_set(9.0, 1).expect_err("no event there");
        assert!(error.contains("No scene change starts at beat 9"), "{error}");
        let error = app
            .arr_scene_event_insert(64.0, 1)
            .expect_err("past the end");
        assert!(error.contains("ends at beat 32"), "{error}");
    }

    // --- whole-arrangement ops ------------------------------------------

    /// Spec 15 (v1): the end refuses to shrink past the last clip rather
    /// than silently dropping it. The UI clamps the handle.
    #[test]
    fn set_end_refuses_to_shrink_past_the_last_clip() {
        let mut app = app_with_clips();
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_set_end(64.0).expect("grows");
        });

        let depth = app.history.undo_len();
        let error = app.arr_set_end(12.0).expect_err("a clip runs to 16");
        assert!(error.contains("a clip runs to beat 16"), "{error}");
        assert_eq!(app.history.undo_len(), depth);
        app.arr_set_end(16.0).expect("exactly the last clip end is fine");

        // The last scene change is the other floor: clear the clips so only
        // it can block, then try to shrink onto it.
        for beat in [0.0, 8.0] {
            let id = app.arrangement_clip_at(0, beat, beat).expect("clip");
            app.arr_clip_delete(id).expect("clip deletes");
        }
        app.arr_scene_event_insert(12.0, 1).expect("insert");
        let error = app.arr_set_end(12.0).expect_err("a scene change is there");
        assert!(error.contains("scene change starts at beat 12"), "{error}");
    }

    #[test]
    fn set_loop_commits_once_and_no_ops_when_unchanged() {
        let mut app = app_with_clips();
        assert_one_entry_and_undoable(&mut app, |app| {
            app.arr_set_loop(true).expect("loop on");
        });
        let depth = app.history.undo_len();
        app.arr_set_loop(true).expect("already on");
        assert_eq!(app.history.undo_len(), depth, "a no-op commits nothing");
    }

    /// A gesture names its clip by the span it drew on, which may reach past
    /// the stored clip in either direction (the view merges phase-continuous
    /// backdrop spans into it).
    #[test]
    fn arrangement_clip_at_resolves_a_gesture_span() {
        let app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("covering clip");
        assert_eq!(app.arrangement_clip_at(0, 12.0, 12.0).unwrap(), id);
        // A span starting in the gap still finds the clip it reaches.
        assert_eq!(app.arrangement_clip_at(0, 6.0, 16.0).unwrap(), id);
        let error = app.arrangement_clip_at(0, 5.0, 6.0).expect_err("nothing there");
        assert!(error.contains("no clip at beat 5"), "{error}");
    }

    /// Every new primitive is refused while a transport mode locks song
    /// editing, and refusing commits nothing.
    #[test]
    fn clip_and_scene_primitives_are_rejected_while_song_editing_is_locked() {
        let mut app = app_with_clips();
        let id = app.arrangement_clip_at(0, 8.0, 8.0).expect("clip at 8");
        let before = app.state.committed_arrangement();
        let depth = app.history.undo_len();
        app.song_transport_locks_edits = true;

        let locked = crate::app::song_edit::SONG_EDITS_LOCKED_ERROR;
        let errors: Vec<String> = vec![
            app.arr_clip_create(0, 4.0, 6.0, LaneSource::Empty, 0.0)
                .map(|_| ())
                .unwrap_err(),
            app.arr_clip_delete(id).unwrap_err(),
            app.arr_clip_move(id, 20.0).unwrap_err(),
            app.arr_clip_resize(id, 8.0, 12.0).unwrap_err(),
            app.arr_clip_split(id, 10.0).map(|_| ()).unwrap_err(),
            app.arr_clip_set_source(id, LaneSource::Empty).unwrap_err(),
            app.arr_scene_event_insert(8.0, 1).unwrap_err(),
            app.arr_scene_event_move(0.0, 4.0).unwrap_err(),
            app.arr_scene_event_set(0.0, 1).unwrap_err(),
            app.arr_scene_event_remove(0.0).unwrap_err(),
            app.arr_set_end(64.0).unwrap_err(),
            app.arr_set_loop(true).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(error, locked);
        }
        assert_eq!(app.state.committed_arrangement(), before);
        assert_eq!(app.history.undo_len(), depth);
    }

    /// Without an arrangement there is nothing to edit, and saying so must
    /// not create history.
    #[test]
    fn primitives_report_a_missing_arrangement() {
        let mut app = test_app();
        let depth = app.history.undo_len();
        let error = app
            .arr_clip_create(0, 0.0, 4.0, LaneSource::Empty, 0.0)
            .expect_err("no arrangement");
        assert!(error.contains("no arrangement"), "{error}");
        let error = app.arr_scene_event_insert(4.0, 1).expect_err("no arrangement");
        assert!(error.contains("no arrangement"), "{error}");
        let error = app.arr_set_end(64.0).expect_err("no arrangement");
        assert!(error.contains("no arrangement"), "{error}");
        assert_eq!(app.history.undo_len(), depth);
    }
}
