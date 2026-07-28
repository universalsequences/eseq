//! Take lifecycle primitives (takes spec 6.4) and the region→take
//! conversion harness (spec 13 Phase C): the dev-facing way to produce a
//! playable take before take recording exists, and the deletion path both
//! share.
//!
//! Both primitives mutate scenes (take pool + chunk patterns) and the
//! committed arrangement (take clips) together and commit ONE history entry —
//! an `EditPatch::Composite` pairing a `SceneStructurePatch` with an
//! `ArrangementStructurePatch`. Patch order inside the composite is chosen so
//! replay validation always sees the take exist before any arrangement state
//! that references it (undo replays a composite in reverse).

use crate::sequencer::{
    insert_clip_sorted, occlude_span, ArrClip, LaneSource, ProjectArrangement, ProjectScenes,
    ProjectSong, TakeId, TrackPatternData, MAX_STEPS,
};

use super::edit::finish_active_gesture;
use super::history::{ArrangementStructurePatch, EditPatch, SceneStructurePatch};
use super::App;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::app::edit::{redo, undo};
    use crate::app::song_edit::SongRowSpec;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternId, PatternSnapshot, SequencerState, StepParam,
    };

    /// One-track, three-scene app; per-track pool ids are 1..=3 with scene
    /// j's cell holding PatternId(j + 1). Every pattern is 16 active steps
    /// with transpose == its pool id.
    fn test_app() -> App {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.with_scenes_mut(|scenes| {
            for id in 1..=3u64 {
                let data = scenes.track_pools[0]
                    .get_mut(PatternId(id))
                    .expect("pool pattern");
                data.track_params.num_steps = 16;
                for step in 0..16 {
                    data.track_bits[step / 64] |= 1 << (step % 64);
                    data.step_data[step][StepParam::Transpose.index()] = id as f32;
                }
            }
        });
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
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app
    }

    /// Three-row song: 0.0 scene 0, 4.0 scene 1, 8.0 scene 2, end 16.
    fn app_with_song() -> App {
        let mut app = test_app();
        app.arr_replace_rows(
            vec![
                SongRowSpec {
                    start_beat: 0.0,
                    scene: 0,
                    overrides: Vec::new(),
                },
                SongRowSpec {
                    start_beat: 4.0,
                    scene: 1,
                    overrides: Vec::new(),
                },
                SongRowSpec {
                    start_beat: 8.0,
                    scene: 2,
                    overrides: Vec::new(),
                },
            ],
            16.0,
            false,
        )
        .expect("arr_replace_rows succeeds");
        app
    }

    /// The rows whose `track` lane plays a TAKE. Every lane carries an
    /// override on every compiled row now (a covered lane its clip's, an
    /// uncovered one an explicit empty), so the take rows are selected by
    /// source rather than by the presence of an override.
    fn take_lane_overrides(song: &ProjectSong, track: usize) -> Vec<(f64, Option<u64>, f64)> {
        song.rows
            .iter()
            .filter_map(|row| {
                row.overrides
                    .iter()
                    .find(|over| over.track == track && over.take_id.is_some())
                    .map(|over| (row.start_beat, over.take_id, over.offset_steps))
            })
            .collect()
    }

    #[test]
    fn region_to_take_copies_resolved_content_and_repoints_the_lane() {
        let mut app = app_with_song();
        let depth = app.history.undo_len();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");

        // The take covers 8 beats of 16th steps = 32 steps in one chunk.
        let take = app.state.track_take(0, take_id).expect("take exists");
        assert_eq!(take.total_len_steps, 32);
        assert_eq!(take.chunks.len(), 1);

        // Chunk content samples the region's resolved clips exactly: the
        // first 16 steps come from scene 1's pattern (transpose 2), the next
        // 16 from scene 2's (transpose 3) — so committed playback of the
        // region is unchanged.
        let chunk = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0].get(take.chunks[0]).cloned().unwrap()
        });
        for step in 0..32 {
            assert!(
                chunk.track_bits[step / 64] >> (step % 64) & 1 == 1,
                "step {step} active"
            );
            let expected = if step < 16 { 2.0 } else { 3.0 };
            assert_eq!(
                chunk.step_data[step][StepParam::Transpose.index()],
                expected,
                "step {step}"
            );
        }
        for step in 32..crate::sequencer::MAX_STEPS {
            assert!(
                chunk.track_bits[step / 64] >> (step % 64) & 1 == 0,
                "step {step} past the take end stays empty"
            );
        }

        // The region's rows now point at the take, re-anchored per row:
        // offset = steps(row.start - 4.0) — the row at 8.0 continues the
        // take at step 16 instead of restarting it.
        let song = app.state.committed_song().expect("song");
        assert_eq!(
            take_lane_overrides(&song, 0),
            vec![(4.0, Some(take_id.0), 0.0), (8.0, Some(take_id.0), 16.0)]
        );
        // Content outside the region is untouched: row 0 still plays the clip
        // scene 0 stamped there (P1 at step 0).
        assert_eq!(
            song.rows[0]
                .overrides
                .iter()
                .find(|over| over.track == 0)
                .map(|over| (over.pattern_id, over.take_id, over.offset_steps)),
            Some((Some(1), None, 0.0))
        );
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");

        // The chunk is hidden from the clip grid.
        let visible: Vec<u64> = app.state.with_project_scenes(|scenes| {
            scenes
                .track_pattern_cells(0)
                .iter()
                .map(|cell| cell.pattern_id.0)
                .collect()
        });
        assert!(!visible.contains(&take.chunks[0].0), "{visible:?}");

        // One undo restores both the song and the take pool; redo replays.
        undo(&mut app);
        assert!(app.state.track_take(0, take_id).is_none());
        let song = app.state.committed_song().expect("song");
        assert!(take_lane_overrides(&song, 0).is_empty());
        redo(&mut app);
        assert!(app.state.track_take(0, take_id).is_some());
        let song = app.state.committed_song().expect("song");
        assert_eq!(take_lane_overrides(&song, 0).len(), 2);
    }

    #[test]
    fn take_delete_removes_chunks_and_song_references_in_one_entry() {
        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let chunks = app.state.track_take(0, take_id).expect("take").chunks;
        let depth = app.history.undo_len();

        app.song_take_delete(0, take_id.0).expect("delete succeeds");
        assert!(app.state.track_take(0, take_id).is_none());
        app.state.with_project_scenes(|scenes| {
            for chunk in &chunks {
                assert!(
                    !scenes.track_pools[0].contains(*chunk),
                    "chunk {} must leave the pool",
                    chunk.0
                );
            }
            // Scene cells are untouched.
            for (idx, scene) in scenes.scenes.iter().enumerate() {
                assert_eq!(scene.cells[0], Some(PatternId(idx as u64 + 1)));
            }
        });
        // Overrides referencing the take are gone; the clips that played it
        // were removed, so those spans are silent.
        let song = app.state.committed_song().expect("song");
        assert!(take_lane_overrides(&song, 0).is_empty());
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");

        // One undo restores the take, its chunks, and the song references.
        undo(&mut app);
        let take = app.state.track_take(0, take_id).expect("take restored");
        assert_eq!(take.chunks, chunks);
        let song = app.state.committed_song().expect("song");
        assert_eq!(take_lane_overrides(&song, 0).len(), 2);
        redo(&mut app);
        assert!(app.state.track_take(0, take_id).is_none());
    }

    #[test]
    fn region_to_take_rejects_empty_regions() {
        let mut app = app_with_song();
        // Silence the whole region first: nothing to convert. Silence is the
        // absence of clips, so delete everything the region touches.
        loop {
            let doomed = app
                .state
                .committed_arrangement()
                .expect("arrangement")
                .track_lanes[0]
                .iter()
                .find(|clip| clip.start_beat < 12.0 && clip.end_beat > 4.0)
                .map(|clip| clip.id);
            let Some(id) = doomed else { break };
            app.arr_clip_delete(id).expect("clip deletes");
        }
        let depth = app.history.undo_len();
        let error = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect_err("empty region must be rejected");
        assert!(error.contains("resolves no content"), "{error}");
        assert_eq!(app.history.undo_len(), depth, "no history entry");
    }
}

impl App {
    /// Delete a take (takes spec 6.4): its chunk patterns leave the pattern
    /// pool and every arrangement clip playing it is removed, so those spans
    /// go silent. One undo entry restores both.
    pub fn song_take_delete(&mut self, track: usize, take_id: u64) -> Result<(), String> {
        if self.song_edits_locked() {
            return Err(super::song_edit::SONG_EDITS_LOCKED_ERROR.to_string());
        }
        let take_id = TakeId(take_id);
        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        let arrangement_before = self.state.committed_arrangement();

        self.state.remove_track_take(track, take_id)?;

        let arrangement_after = arrangement_before.as_ref().map(|arrangement| {
            let mut arrangement = arrangement.clone();
            if let Some(lane) = arrangement.track_lanes.get_mut(track) {
                lane.retain(|clip| clip.take_id != Some(take_id.0));
            }
            arrangement
        });
        if let Some(arrangement) = arrangement_after.clone() {
            // Installing validates and recompiles; a failure installs nothing,
            // so only the scenes mutation has to roll back.
            if let Err(error) = self.state.set_committed_arrangement(Some(arrangement)) {
                self.restore_scene_structure_state(&scenes_before)?;
                return Err(format!(
                    "deleting the take left an invalid song and was rolled back: {error}"
                ));
            }
        }
        let scenes_after = self.state.capture_project_scenes();
        finish_active_gesture(self);
        let scene_patch = SceneStructurePatch {
            before: scenes_before,
            after: scenes_after,
        };
        let arrangement_patch = ArrangementStructurePatch {
            before: arrangement_before,
            after: arrangement_after,
        };
        let retained_bytes = scene_patch.retained_bytes() + arrangement_patch.retained_bytes();
        // Arrangement first: undo replays in reverse (scenes-with-take restored
        // before the take-referencing arrangement), redo forward (the
        // reference-free arrangement validates against any scenes).
        self.history.commit(
            "Delete take",
            None,
            EditPatch::Composite(vec![
                EditPatch::Arrangement(arrangement_patch),
                EditPatch::SceneStructure(scene_patch),
            ]),
            retained_bytes,
        );
        self.rebuild_active_song_after_arrangement_edit();
        Ok(())
    }

    /// Convert one track's resolved arrangement content over
    /// `[start_beat, end_beat)` into a take and point the region's lane at
    /// it (takes spec 13 Phase C — the dev validation harness for take
    /// playback; also the seed of consolidate/flatten). Content is sampled
    /// per destination step from the region's resolved clips (patterns or
    /// takes, offsets honored), so committed playback of the region is
    /// unchanged. One undo entry.
    pub fn song_region_to_take(
        &mut self,
        track: usize,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<TakeId, String> {
        if self.song_edits_locked() {
            return Err(super::song_edit::SONG_EDITS_LOCKED_ERROR.to_string());
        }
        if !start_beat.is_finite() || !end_beat.is_finite() || start_beat < 0.0 {
            return Err("conversion region beats must be finite and non-negative".to_string());
        }
        let arrangement = self
            .state
            .committed_arrangement()
            .ok_or_else(|| "The project has no song".to_string())?;
        // The compiled song is what resolves the region's audible content
        // (backdrop included); the arrangement is what the result is written
        // onto.
        let song = self
            .state
            .committed_song()
            .ok_or_else(|| "The project has no song".to_string())?;
        if start_beat >= arrangement.end_beat {
            return Err(format!(
                "conversion region starts at beat {start_beat} but the song ends at {}",
                arrangement.end_beat
            ));
        }
        let end_beat = end_beat.min(arrangement.end_beat);
        if end_beat <= start_beat {
            return Err(format!(
                "conversion region must have a positive span (got [{start_beat}, {end_beat}))"
            ));
        }

        // Build the chunk contents from the region's resolved lane clips.
        let (chunks, total_len_steps) =
            self.render_region_chunks(track, &song, start_beat, end_beat)?;

        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        let arrangement_before = Some(arrangement.clone());
        let take_id = self
            .state
            .register_track_take(track, None, chunks, total_len_steps)?;

        let mut arrangement_after = arrangement;
        let scenes = self.state.capture_project_scenes();
        let painted = self.paint_take_clip(
            &mut arrangement_after,
            &scenes,
            track,
            start_beat,
            end_beat,
            take_id,
        );
        if let Err(error) = painted {
            self.state.remove_track_take(track, take_id)?;
            self.restore_scene_structure_state(&scenes_before)?;
            return Err(error);
        }
        if let Err(error) = self
            .state
            .set_committed_arrangement(Some(arrangement_after.clone()))
        {
            self.state.remove_track_take(track, take_id)?;
            self.restore_scene_structure_state(&scenes_before)?;
            return Err(format!(
                "region conversion produced an invalid song and was rolled back: {error}"
            ));
        }
        let scenes_after = self.state.capture_project_scenes();
        finish_active_gesture(self);
        let scene_patch = SceneStructurePatch {
            before: scenes_before,
            after: scenes_after,
        };
        let arrangement_patch = ArrangementStructurePatch {
            before: arrangement_before,
            after: Some(arrangement_after),
        };
        let retained_bytes = scene_patch.retained_bytes() + arrangement_patch.retained_bytes();
        // Scenes first: redo restores the take before the arrangement that
        // references it; undo removes the clip before the take.
        self.history.commit(
            "Convert region to take",
            None,
            EditPatch::Composite(vec![
                EditPatch::SceneStructure(scene_patch),
                EditPatch::Arrangement(arrangement_patch),
            ]),
            retained_bytes,
        );
        self.rebuild_active_song_after_arrangement_edit();
        Ok(take_id)
    }

    /// Sample the resolved content of `track`'s lane over the region into
    /// freshly minted chunk patterns (takes spec 6.1: `MAX_STEPS`-long
    /// chunks; the region's first resolved pattern is the template for
    /// track-level state and the take's step domain).
    fn render_region_chunks(
        &self,
        track: usize,
        song: &ProjectSong,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<(Vec<TrackPatternData>, u32), String> {
        let scenes = self.state.capture_project_scenes();
        if track >= scenes.track_pools.len() {
            return Err(format!("track {} does not exist", track + 1));
        }
        let lanes = crate::sequencer::project_lanes(song, &scenes);
        let lane = lanes
            .get(track)
            .ok_or_else(|| format!("track {} has no lane projection", track + 1))?;

        // Template: the first non-empty clip in the region.
        let template_data = lane
            .iter()
            .filter(|clip| clip.end_beat > start_beat && clip.start_beat < end_beat)
            .find_map(|clip| match clip.source {
                LaneSource::Pattern(id) => scenes.track_pools[track].get(id),
                LaneSource::Take(id) => scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(id))
                    .and_then(|take| take.chunks.first())
                    .and_then(|chunk| scenes.track_pools[track].get(*chunk)),
                LaneSource::Empty => None,
            })
            .ok_or_else(|| {
                "the region resolves no content on this track; nothing to convert".to_string()
            })?;
        let step_beats = template_data
            .track_params
            .timebase
            .step_beats(MAX_STEPS);
        if !(step_beats > 0.0) {
            return Err("the region's template pattern has a degenerate timebase".to_string());
        }
        let total_len_steps = ((end_beat - start_beat) / step_beats - 1e-6).ceil().max(1.0) as u32;

        let mut template = template_data.clone();
        template.track_params.num_steps = MAX_STEPS;
        template.clear_step_content();
        let chunk_count = (total_len_steps as usize).div_ceil(MAX_STEPS);
        let mut chunks = vec![template; chunk_count];

        for dst in 0..total_len_steps as usize {
            let beat = start_beat + dst as f64 * step_beats;
            let Some(clip) = lane
                .iter()
                .find(|clip| clip.start_beat <= beat + 1e-9 && beat + 1e-9 < clip.end_beat)
            else {
                continue;
            };
            let clip_beats = beat - clip.start_beat;
            let (src_data, src_step) = match clip.source {
                LaneSource::Empty => continue,
                LaneSource::Pattern(id) => {
                    let Some(data) = scenes.track_pools[track].get(id) else {
                        continue;
                    };
                    let num_steps = data.track_params.num_steps.max(1);
                    let src_step_beats =
                        data.track_params.timebase.step_beats(num_steps);
                    if !(src_step_beats > 0.0) {
                        continue;
                    }
                    let p = (clip.offset_steps + clip_beats / src_step_beats)
                        .rem_euclid(num_steps as f64);
                    (data, (p + 1e-6).floor() as usize % num_steps)
                }
                LaneSource::Take(id) => {
                    let Some(take) = scenes
                        .take_pools
                        .get(track)
                        .and_then(|takes| takes.get(id))
                    else {
                        continue;
                    };
                    let Some(first_chunk) = take
                        .chunks
                        .first()
                        .and_then(|chunk| scenes.track_pools[track].get(*chunk))
                    else {
                        continue;
                    };
                    let src_step_beats = first_chunk
                        .track_params
                        .timebase
                        .step_beats(MAX_STEPS);
                    if !(src_step_beats > 0.0) {
                        continue;
                    }
                    let p = clip.offset_steps + clip_beats / src_step_beats + 1e-6;
                    let Some((chunk_idx, local)) = take.chunk_step_at(p) else {
                        continue;
                    };
                    let Some(data) = scenes.track_pools[track].get(take.chunks[chunk_idx])
                    else {
                        continue;
                    };
                    (data, local.floor() as usize)
                }
            };
            chunks[dst / MAX_STEPS].copy_step_content_from(dst % MAX_STEPS, src_data, src_step);
        }
        Ok((chunks, total_len_steps))
    }

    /// Point one lane's `[start_beat, end_beat)` at `take_id`: a single take
    /// clip anchored at source step 0, truncating whatever it lands on exactly
    /// as any other clip write does (spec 8/14). Shared with the capture
    /// commit's take painting.
    pub(super) fn paint_take_clip(
        &self,
        arrangement: &mut ProjectArrangement,
        scenes: &ProjectScenes,
        track: usize,
        start_beat: f64,
        end_beat: f64,
        take_id: TakeId,
    ) -> Result<(), String> {
        if track >= arrangement.track_lanes.len() {
            return Err(format!("Track {} has no arrangement lane", track + 1));
        }
        if end_beat <= start_beat {
            return Err(format!(
                "A take clip must have a positive span (got [{start_beat}, {end_beat}))"
            ));
        }
        occlude_span(arrangement, scenes, track, start_beat, end_beat)?;
        let id = arrangement.allocate_clip_id()?;
        insert_clip_sorted(
            arrangement,
            track,
            ArrClip::new_take(id, start_beat, end_beat, take_id.0, 0.0),
        );
        Ok(())
    }
}
