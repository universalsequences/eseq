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
    insert_clip_sorted, lower_rows_to_arrangement, occlude_span, pattern_play_step,
    restamped_clip, stamp_scene_clips, ArrClip, ClipId, LaneSource, ProjectArrangement,
    ProjectScenes, ProjectSongRow, SongCompileContext, SongRowId,
};

use super::edit::finish_active_gesture;
use super::history::{ArrangementStructurePatch, EditPatch};
use super::song_edit::SongRowSpec;
use super::App;

/// Refusal for `arr_clip_create` with no source. Silence is the absence of a
/// clip (spec 6.1/6.2), so "create a silent clip" asks for nothing at all;
/// the ops that *can* mean something with an empty source (set-source, the
/// clip-create host command) delete or clear instead of erroring.
pub const EMPTY_CLIP_ERROR: &str =
    "A clip must have a source: silence is an empty stretch of lane, not an empty clip — \
     use delete to silence a span";

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
            .reconcile_committed_arrangement_track_lanes()?;
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
        // Borrow the live scenes for the edit instead of cloning the whole
        // project pattern store: the clone was ~11 ms on a realistic project
        // and ran on every single-clip primitive.
        self.state
            .with_project_scenes(|scenes| edit(&mut after, scenes))?;
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
        let id = after.allocate_clip_id()?;
        let mut clip = ArrClip::new(id, start_beat, end_beat, None);
        match source {
            LaneSource::Empty => return Err(EMPTY_CLIP_ERROR.to_string()),
            LaneSource::Pattern(pattern) => clip.pattern_id = Some(pattern.0),
            LaneSource::Take(take) => clip.take_id = Some(take.0),
        }
        clip.offset_steps = offset_steps;
        self.state.with_project_scenes(|scenes| {
            occlude_span(&mut after, scenes, track, start_beat, end_beat)
        })?;
        insert_clip_sorted(&mut after, track, clip);
        self.commit_arrangement_edit("Create clip", Some(before), Some(after))?;
        Ok(id)
    }

    /// Delete a clip. The span becomes SILENT (spec 6.2): the clip is gone
    /// from the timeline and nothing plays there — deleting is exactly the
    /// clip not playing.
    pub fn arr_clip_delete(&mut self, clip_id: ClipId) -> Result<(), String> {
        self.edit_arrangement("Delete clip", |arrangement, _scenes| {
            let (track, _) = Self::locate_clip(arrangement, clip_id)?;
            Self::take_clip(arrangement, track, clip_id);
            Ok(())
        })
    }

    /// Silence `[start_beat, end_beat)` on `track`: remove the clips it
    /// covers and trim the ones it only overlaps, storing nothing.
    ///
    /// This is what "draw an empty clip here" means now (spec 6.2): the
    /// gesture asks for silence over a span, and silence is an absence, so the
    /// span is cleared rather than filled with a sourceless object. One undo
    /// entry, and a span that was already silent is an exact no-op.
    pub fn arr_clip_clear_span(
        &mut self,
        track: usize,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<(), String> {
        let start_beat = finite_beat("Clip start beat", start_beat)?;
        let end_beat = finite_beat("Clip end beat", end_beat)?;
        if end_beat <= start_beat {
            return Err(format!(
                "A cleared span must have positive length (got [{start_beat}, {end_beat}))"
            ));
        }
        self.edit_arrangement("Clear clips", move |arrangement, scenes| {
            if track >= arrangement.track_lanes.len() {
                return Err(format!("Track {} has no arrangement lane", track + 1));
            }
            occlude_span(arrangement, scenes, track, start_beat, end_beat)
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
            // trimmed clip will actually carry. Borrow the scenes — cloning
            // the whole pattern store here doubled the cost of every resize.
            let restamped = self
                .state
                .with_project_scenes(|scenes| restamped_clip(scenes, track, &clip, new_start_beat));
            match restamped {
                Some(restamped) => match self.take_clip_playable_end(track, &restamped) {
                    Some(limit) => new_end_beat.min(limit).max(new_start_beat),
                    None => new_end_beat,
                },
                // The re-anchored clip would have nothing left to play.
                None => new_start_beat,
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
            let mut resized = restamped_clip(scenes, track, &clip, new_start_beat)
                .ok_or_else(|| EMPTY_CLIP_ERROR.to_string())?;
            resized.end_beat = clamped_end;
            occlude_span(arrangement, scenes, track, new_start_beat, clamped_end)?;
            insert_clip_sorted(arrangement, track, resized);
            arrangement.end_beat = arrangement.end_beat.max(clamped_end);
            Ok(())
        })
    }

    /// Slide a pattern clip's loop window by `delta_steps` (clip-edit-target
    /// spec 5): the window is the whole pattern today, so sliding it by k is
    /// audibly identical to shifting phase by k — `offset_steps` advances by
    /// `delta_steps`, wrapped through `pattern_play_step`. Take clips never
    /// wrap; their band is read-only and this refuses them.
    pub fn arr_clip_slide_offset(
        &mut self,
        clip_id: ClipId,
        delta_steps: f64,
    ) -> Result<(), String> {
        if !delta_steps.is_finite() {
            return Err("Loop-window slide delta must be finite".to_string());
        }
        self.edit_arrangement("Slide clip loop window", move |arrangement, scenes| {
            let (track, clip) = Self::locate_clip(arrangement, clip_id)?;
            let Some(pattern_id) = clip.pattern_id else {
                return Err(
                    "Only pattern clips have a loop window to slide; a take plays linearly"
                        .to_string(),
                );
            };
            let (_, num_steps) = scenes
                .song_track_pattern_step_mapping(track, pattern_id)
                .ok_or_else(|| "The clip's pattern no longer exists".to_string())?;
            let next = pattern_play_step(clip.offset_steps, delta_steps, (0.0, num_steps));
            let slid = arrangement.track_lanes[track]
                .iter_mut()
                .find(|candidate| candidate.id == clip_id)
                .expect("located above");
            slid.offset_steps = next;
            Ok(())
        })
    }

    /// Set a clip's phase anchor to an absolute source step (clip-edit-target
    /// spec 6, the clip panel's Start offset field). Pattern offsets wrap
    /// through `pattern_play_step` (a negative "pickup" entry lands in the
    /// top half); take offsets clamp to `[0, total_len)` and the clip end
    /// re-clamps to the playable end the new offset leaves.
    pub fn arr_clip_set_offset(
        &mut self,
        clip_id: ClipId,
        offset_steps: f64,
    ) -> Result<(), String> {
        if !offset_steps.is_finite() {
            return Err("Clip start offset must be finite".to_string());
        }
        // Take end re-clamp needs App context; compute before the edit.
        let take_end_limit = {
            let arrangement = self.require_arrangement()?;
            let (track, clip) = Self::locate_clip(&arrangement, clip_id)?;
            if clip.take_id.is_some() {
                let mut probed = clip;
                probed.offset_steps = offset_steps.max(0.0);
                self.take_clip_playable_end(track, &probed)
            } else {
                None
            }
        };
        self.edit_arrangement("Set clip start offset", move |arrangement, scenes| {
            let (track, clip) = Self::locate_clip(arrangement, clip_id)?;
            let next = if let Some(take_id) = clip.take_id {
                let (_, total_len) = scenes
                    .song_track_take_step_mapping(track, take_id)
                    .ok_or_else(|| "The clip's take no longer exists".to_string())?;
                offset_steps.clamp(0.0, (total_len - 1.0).max(0.0))
            } else if let Some(pattern_id) = clip.pattern_id {
                let (_, num_steps) = scenes
                    .song_track_pattern_step_mapping(track, pattern_id)
                    .ok_or_else(|| "The clip's pattern no longer exists".to_string())?;
                pattern_play_step(offset_steps, 0.0, (0.0, num_steps))
            } else {
                return Err("An empty clip has no start offset".to_string());
            };
            let end_limit = take_end_limit
                .map(|limit| limit.max(clip.start_beat + 1.0))
                .unwrap_or(f64::INFINITY);
            let edited = arrangement.track_lanes[track]
                .iter_mut()
                .find(|candidate| candidate.id == clip_id)
                .expect("located above");
            edited.offset_steps = next;
            edited.end_beat = edited.end_beat.min(end_limit);
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
        let right_id = after.allocate_clip_id()?;
        // A right half with nothing left to play (a take past its end) is not
        // stored: that span is silent, and silence is a gap.
        let right = self
            .state
            .with_project_scenes(|scenes| restamped_clip(scenes, track, &clip, beat))
            .map(|mut right| {
                right.id = right_id;
                right
            });
        {
            let lane = &mut after.track_lanes[track];
            let index = lane
                .iter()
                .position(|candidate| candidate.id == clip_id)
                .expect("located above");
            lane[index].end_beat = beat;
            if let Some(right) = right {
                lane.insert(index + 1, right);
            }
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
        if matches!(source, LaneSource::Empty) {
            // "Make this span silent" and "remove this clip" are the same
            // operation in this model (spec 6.2), so do the operation.
            return self.arr_clip_delete(clip_id);
        }
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

    // --- scene-lane ops (spec 8) ----------------------------------------

    /// The span a scene event governs: from its own beat to the next event's,
    /// else the arrangement end. This is the window an insert/set/move
    /// re-stamps.
    fn scene_event_span(arrangement: &ProjectArrangement, index: usize) -> (f64, f64) {
        let start = arrangement.scene_lane[index].start_beat;
        let end = arrangement
            .scene_lane
            .get(index + 1)
            .map(|next| next.start_beat)
            .unwrap_or(arrangement.end_beat)
            .min(arrangement.end_beat);
        (start, end)
    }

    /// Insert a scene change at `beat`: it STAMPS the scene's cells as clips
    /// across its whole span (spec 6.2/8), truncating whatever was there —
    /// the Ableton truncation rule, one undo entry. A track whose cell in
    /// that scene is empty gets no clip and is silent.
    pub fn arr_scene_event_insert(&mut self, beat: f64, scene: usize) -> Result<(), String> {
        let beat = finite_beat("Scene event beat", beat)?;
        self.edit_arrangement("Insert scene event", move |arrangement, scenes| {
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
            let (start, end) = Self::scene_event_span(arrangement, position);
            stamp_scene_clips(arrangement, scenes, start, end)
        })
    }

    /// Move the scene change at `from_beat` to `to_beat`, re-stamping both the
    /// span it vacates (now the predecessor's) and the one it lands on. The
    /// event at 0.0 cannot move: an arrangement always starts on a scene.
    pub fn arr_scene_event_move(&mut self, from_beat: f64, to_beat: f64) -> Result<(), String> {
        let from_beat = finite_beat("Scene event beat", from_beat)?;
        let to_beat = finite_beat("Scene event beat", to_beat)?;
        self.edit_arrangement("Move scene event", move |arrangement, scenes| {
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
            // The window both spans live in, resolved before the move so the
            // vacated stretch is re-stamped with whatever now governs it.
            let (vacated_start, vacated_end) = Self::scene_event_span(arrangement, index);
            arrangement.scene_lane[index].start_beat = to_beat;
            arrangement.scene_lane.sort_by(|a, b| {
                a.start_beat
                    .partial_cmp(&b.start_beat)
                    .expect("scene event beats are finite")
            });
            let moved = arrangement
                .scene_lane
                .iter()
                .position(|event| event.start_beat == to_beat)
                .expect("the moved event is in the lane");
            let (moved_start, moved_end) = Self::scene_event_span(arrangement, moved);
            stamp_scene_clips(
                arrangement,
                scenes,
                vacated_start.min(moved_start),
                vacated_end.max(moved_end),
            )
        })
    }

    /// Point the scene change at `beat` at a different scene, re-stamping its
    /// whole span with the new scene's cells (spec 8: changing which scene an
    /// event names replaces the clips it stamped).
    pub fn arr_scene_event_set(&mut self, beat: f64, scene: usize) -> Result<(), String> {
        let beat = finite_beat("Scene event beat", beat)?;
        self.edit_arrangement("Set scene event", move |arrangement, scenes| {
            let index = Self::scene_event_index(arrangement, beat)?;
            arrangement.scene_lane[index].scene = scene;
            let (start, end) = Self::scene_event_span(arrangement, index);
            stamp_scene_clips(arrangement, scenes, start, end)
        })
    }

    /// Remove the scene change at `beat`: the marker goes and **the clips
    /// stay** (spec 8/14, locked).
    ///
    /// This is the one scene op that does NOT re-stamp, and the asymmetry is
    /// deliberate. Insert/set/move all mean "launch this scene here", so
    /// replacing the content under them is the intent. Remove means "clean up
    /// this marker" — the user's original complaint was that deleting scene
    /// changes to tidy the scene row destroyed the pattern changes riding
    /// beneath them. Clips are the truth now, so a removal leaves the
    /// predecessor's label spanning clips that a since-removed scene stamped.
    /// That reads honestly: what plays is the clips.
    ///
    /// Removing the event at 0.0 is rejected, exactly as removing row zero was.
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
            .map_err(|error| format!("Arrangement history no longer matches the project: {error}"))?;
        self.rebuild_active_song_after_arrangement_edit();
        Ok(())
    }

    /// Re-preflight structural arrangement edits on the control thread and
    /// hand the scheduler an immutable replacement. The scheduler decides
    /// whether the current row is identity-compatible or must remain pending
    /// until its next boundary.
    pub(super) fn rebuild_active_song_after_arrangement_edit(&mut self) {
        if !self.song_playback_authority_active() {
            return;
        }
        let Ok(song) = self.state.preflight_runtime_song() else {
            return;
        };
        if self.state.rebuild_song_playback(std::sync::Arc::clone(&song)).is_ok() {
            self.active_runtime_song = Some(song);
            // A structural remap can change the current row's ordinal without
            // re-entering it. Do not let the old ordinal suppress the next
            // real scheduler-authoritative RowApplied notice.
            self.song_mirrored_row = None;
        }
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
        self.rebuild_active_song_after_arrangement_edit();
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

    #[test]
    fn arrangement_edit_repairs_lanes_missing_for_already_appended_tracks() {
        let mut app = test_app();
        let mut arrangement = ProjectArrangement::new(2, 16.0);
        arrangement.scene_lane.push(SceneEvent {
            start_beat: 8.0,
            scene: 1,
        });
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("two-track arrangement");

        // Reproduce the regression state: project topology grew from two
        // tracks to four while the committed arrangement stayed at two lanes.
        app.state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(4, &[]),
                PatternSnapshot::new_default(4, &[]),
                PatternSnapshot::new_default(4, &[]),
            ],
            0,
        );
        app.tracks.extend(["Track 3".to_string(), "Track 4".to_string()]);
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(4).unwrap();

        app.arr_set_loop(true)
            .expect("the edit boundary repairs missing arrangement lanes");

        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(arrangement.track_lanes.len(), 4);
        for track in 2..4 {
            assert_eq!(
                arrangement.track_lanes[track]
                    .iter()
                    .map(|clip| (clip.start_beat, clip.end_beat, clip.pattern_id))
                    .collect::<Vec<_>>(),
                vec![(0.0, 8.0, Some(1)), (8.0, 16.0, Some(2))]
            );
        }
        assert!(arrangement.loop_enabled);
        assert_song_matches_arrangement(&app);
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
        app.song_transport_mode =
            crate::app::song_transport::SongTransportMode::ArrangementCapture;
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
    /// compiling the result must reproduce what the row path SOUNDED like —
    /// same beats, same scenes, same per-track resolution.
    ///
    /// Raw override lists are deliberately no longer comparable: the row model
    /// left an unmentioned lane to be resolved from the row's scene cell,
    /// while the lane model stamps that cell as a clip and compiles it to an
    /// explicit override. Both play the same pattern at the same phase, which
    /// is what these fixtures assert.
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

    /// What one row resolves each track to at its own start beat, under the
    /// PLAYBACK rule in `preflight_runtime_song`: the override if there is
    /// one, else the row's scene cell at phase 0.
    fn row_resolution(
        app: &App,
        row: &ProjectSongRow,
        track_count: usize,
    ) -> Vec<Option<(u64, f64)>> {
        app.state.with_project_scenes(|scenes| {
            (0..track_count)
                .map(|track| match row.overrides.iter().find(|over| over.track == track) {
                    Some(over) => over.pattern_id.map(|id| (id, over.offset_steps)),
                    None => crate::sequencer::SongCompileContext::song_scene_cell(
                        scenes, row.scene, track,
                    )
                    .map(|id| (id, 0.0)),
                })
                .collect()
        })
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
            .map(|row| (row.start_beat, row.scene, row_resolution(&arr_app, row, 2)))
            .collect();
        let rows: Vec<_> = from_rows
            .rows
            .iter()
            .map(|row| (row.start_beat, row.scene, row_resolution(&arr_app, row, 2)))
            .collect();
        assert_eq!(
            lanes, rows,
            "def-song must compile back to the same audible song"
        );
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
        // One DECLARED clip spanning both rows, then the clip the scene event
        // at beat 8 stamped, surviving where the declared run ended.
        assert_eq!(
            lane(&app, 0),
            vec![(0.0, 16.0, Some(2), 0.0), (16.0, 24.0, Some(2), 0.0)],
            "phase-continuous rows merge into one clip: {:?}",
            arrangement.track_lanes[0]
        );

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
            lane(&app, 0),
            vec![
                // the retrigger stays its own clip, re-anchored at step 0 ...
                (0.0, 2.0, Some(2), 0.0),
                (2.0, 16.0, Some(2), 0.0),
                // ... and past it the scene-1 stamp shows through. It was
                // stamped free-run at beat 2 (8 steps) and left-trimmed to
                // beat 16, so 8 + 56 == 64 steps == step 0: beat 16 is on the
                // global grid, and the stamp is locked to it.
                (16.0, 24.0, Some(2), 0.0),
            ],
            "a retrigger keeps its own clip: {:?}",
            arrangement.track_lanes[0]
        );
        let _ = rows;
    }

    /// A def-song whose scene changes land off the pattern grid still leaves
    /// every stamped clip GRID-LOCKED: the phase at any beat is
    /// `steps(beat) mod L`, so scene 1 launching at beat 2 does not shift the
    /// rhythm of the lanes it stamps.
    #[test]
    fn def_song_lowering_keeps_stamped_clips_grid_locked() {
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
        // Beat 16 is 64 steps on the global grid, and 64 mod 16 == 0: both
        // lanes are exactly on a pattern boundary there, regardless of the
        // scene change having landed at beat 2.
        assert_eq!(
            last.overrides
                .iter()
                .map(|over| (over.track, over.pattern_id, over.offset_steps))
                .collect::<Vec<_>>(),
            vec![(0, Some(2), 0.0), (1, Some(2), 0.0)]
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
        assert!(first.next_clip_id > 0, "the lowering allocated ids");
        app.arr_replace_rows(
            vec![spec(0.0, 0, vec![ov(1, 2)]), spec(2.0, 0, Vec::new())],
            16.0,
            false,
        )
        .expect("lowers again");
        let second = app.state.committed_arrangement().expect("arrangement");
        assert!(
            second.next_clip_id > first.next_clip_id,
            "the allocator only ever moves forward"
        );
        assert!(
            second
                .track_lanes
                .iter()
                .flatten()
                .all(|clip| clip.id.0 >= first.next_clip_id),
            "ids are never reused across lowerings: {:?}",
            second.track_lanes
        );
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

    /// The id of the clip covering `beat` on `track`. Tests address clips
    /// positionally for convenience; the UI addresses them by id (spec 12).
    fn clip_at(app: &App, track: usize, beat: f64) -> ClipId {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[track]
            .iter()
            .find(|clip| clip.contains(beat))
            .unwrap_or_else(|| panic!("no clip covers beat {beat} on track {track}"))
            .id
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

    /// The phase (in steps) the COMMITTED SONG puts `track` at on `beat`.
    /// The fixture's patterns are all 16 steps at four steps per beat, so the
    /// arithmetic is spelled out rather than borrowed from the compiler.
    fn phase_at(app: &App, track: usize, beat: f64) -> f64 {
        let song = app.state.committed_song().expect("committed song");
        let row = song
            .rows
            .iter()
            .rev()
            .find(|row| row.start_beat <= beat)
            .expect("a row governs every beat");
        let over = row
            .overrides
            .iter()
            .find(|over| over.track == track)
            .expect("every lane states its resolution");
        assert!(over.pattern_id.is_some(), "the fixture uses pattern clips");
        (over.offset_steps + (beat - row.start_beat) * 4.0).rem_euclid(16.0)
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
        let id = clip_at(&app, 0, 8.0);
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
        let id = clip_at(&app, 0, 8.0);
        app.arr_clip_resize(id, 8.0, 16.0).expect("no-op resize");
        // Give it a nonzero anchor so rigidity is observable.
        app.arr_clip_split(id, 10.0).expect("split");
        let right = clip_at(&app, 0, 10.0);
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
        let id = clip_at(&app, 0, 8.0);
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
        let id = clip_at(&app, 0, 8.0);
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
        let id = clip_at(&app, 0, 8.0);
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

        // "Make this span silent" and "remove this clip" are the same
        // operation (spec 6.2), so setting an empty source DELETES the clip.
        let depth = app.history.undo_len();
        app.arr_clip_set_source(id, LaneSource::Empty)
            .expect("an empty source silences the span");
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");
        assert_eq!(
            lane(&app, 0),
            vec![(0.0, 4.0, Some(1), 0.0), (10.0, 16.0, Some(2), 8.0)],
            "the clip is gone from the timeline, leaving silence"
        );
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

    /// The user-visible guarantee that started the lane pivot: removing a
    /// scene change to tidy the scene row must NEVER destroy clips. Insert,
    /// set, and move all mean "launch this scene here" and re-stamp; remove
    /// means "clean up this marker" and touches nothing but the marker.
    /// Removing the event at 0.0 is rejected, exactly as removing row zero was.
    #[test]
    fn scene_event_remove_merges_into_the_predecessor_and_never_touches_clips() {
        let mut app = app_with_clips();
        app.arr_scene_event_insert(8.0, 1).expect("insert");
        // Scene 1's cell (P2) is stamped from the event to the song end,
        // truncating the clip that was there.
        assert_eq!(
            lane(&app, 0),
            vec![(0.0, 4.0, Some(1), 0.0), (8.0, 32.0, Some(2), 0.0)]
        );
        let clips_before = lane(&app, 0);

        app.arr_scene_event_remove(8.0).expect("remove");
        assert_eq!(scene_lane(&app), vec![(0.0, 0)], "the span merges back");
        assert_eq!(
            lane(&app, 0),
            clips_before,
            "the clips the removed scene stamped survive it — what plays is \
             the clips, and only the marker was deleted"
        );

        let depth = app.history.undo_len();
        let error = app.arr_scene_event_remove(0.0).expect_err("row zero rule");
        assert!(error.contains("must start on a scene"), "{error}");
        assert_eq!(app.history.undo_len(), depth, "a rejection commits nothing");
    }

    /// The user's gesture, end to end: "when i resize the end of a scene to
    /// be shorter (and thus make the next scene launch happen sooner), the
    /// resulting track clips it modifies ends up produces a jump in timing".
    ///
    /// Shortening a scene span IS moving the next scene event earlier, which
    /// re-stamps. Because stamping free-runs against the global clock, the
    /// clips it writes stay grid-locked: the boundary decides how much of the
    /// pattern is heard, never when its steps fall.
    #[test]
    fn shortening_a_scene_span_never_moves_the_rhythm_of_the_clips_below() {
        let mut app = test_app();
        // Scenes at 0/8/16 over 32 beats; patterns are 16 steps at four
        // steps per beat, so a cycle is 4 beats and source step 0 lands on
        // every multiple of 4.
        app.arr_replace_rows(
            vec![spec(0.0, 0, Vec::new()), spec(8.0, 1, Vec::new()), spec(16.0, 2, Vec::new())],
            32.0,
            false,
        )
        .expect("lowers");
        let before: Vec<f64> = (0..32).map(|q| phase_at(&app, 0, q as f64)).collect();

        // Drag the scene-1 boundary from 8 back to 5 — three beats shorter,
        // and deliberately off the 4-beat cycle.
        app.arr_scene_event_move(8.0, 5.0).expect("boundary moves");
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (5.0, 1), (16.0, 2)]);
        // 5 beats == 20 steps; 20 mod 16 == 4.
        assert_eq!(
            lane(&app, 0),
            vec![
                (0.0, 5.0, Some(1), 0.0),
                (5.0, 16.0, Some(2), 4.0),
                (16.0, 32.0, Some(3), 0.0),
            ]
        );

        // The rhythm is untouched everywhere the SOURCE did not change: each
        // lane still reaches step 0 on the same absolute beats.
        for q in 0..32 {
            let beat = q as f64;
            assert_eq!(
                phase_at(&app, 0, beat),
                before[q],
                "the scene resize moved the rhythm at beat {beat}"
            );
        }
        for beat in [4.0, 8.0, 12.0, 16.0, 20.0] {
            assert_eq!(phase_at(&app, 0, beat), 0.0, "beat {beat} is still step 0");
        }
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

        // The last scene change is the other floor: insert it, then clear
        // every clip (including the ones it stamped) so only it can block.
        app.arr_scene_event_insert(12.0, 1).expect("insert");
        loop {
            let ids: Vec<ClipId> = app
                .state
                .committed_arrangement()
                .expect("arrangement")
                .track_lanes
                .iter()
                .flatten()
                .map(|clip| clip.id)
                .collect();
            let Some(id) = ids.first().copied() else { break };
            app.arr_clip_delete(id).expect("clip deletes");
        }
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

    /// Every new primitive is refused while a transport mode locks song
    /// editing, and refusing commits nothing.
    #[test]
    fn clip_and_scene_primitives_are_rejected_while_song_editing_is_locked() {
        let mut app = app_with_clips();
        let id = clip_at(&app, 0, 8.0);
        let before = app.state.committed_arrangement();
        let depth = app.history.undo_len();
        app.song_transport_mode =
            crate::app::song_transport::SongTransportMode::ArrangementCapture;

        let locked = crate::app::song_edit::SONG_EDITS_LOCKED_ERROR;
        let errors: Vec<String> = vec![
            app.arr_clip_create(0, 4.0, 6.0, LaneSource::Empty, 0.0)
                .map(|_| ())
                .unwrap_err(),
            app.arr_clip_delete(id).unwrap_err(),
            app.arr_clip_move(id, 20.0).unwrap_err(),
            app.arr_clip_resize(id, 8.0, 12.0).unwrap_err(),
            app.arr_clip_split(id, 10.0).map(|_| ()).unwrap_err(),
            app.arr_clip_set_source(id, LaneSource::Pattern(PatternId(1)))
                .unwrap_err(),
            app.arr_clip_clear_span(0, 4.0, 6.0).unwrap_err(),
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
