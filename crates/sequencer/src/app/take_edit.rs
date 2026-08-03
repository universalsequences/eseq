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
    insert_clip_sorted, occlude_span, ArrClip, ClipId, LaneSource, PatternSnapshot,
    ProjectArrangement, ProjectScenes, ProjectSong, TakeId, TrackPatternData, MAX_STEPS,
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
                assert!(scenes.track_pools[0].edit(PatternId(id), |data| {
                    data.track_params.num_steps = 16;
                    for step in 0..16 {
                        data.track_bits[step / 64] |= 1 << (step % 64);
                        data.step_data[step][StepParam::Transpose.index()] = id as f32;
                    }
                }));
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
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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
            scenes.track_pools[0].get(take.chunks[0]).unwrap()
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
    fn empty_take_clip_create_is_silent_selected_and_one_undo_entry() {
        let mut app = app_with_song();
        let depth = app.history.undo_len();

        let (take_id, clip_id) = app
            .arr_empty_take_clip_create(0, 12.0, 20.0)
            .expect("empty take clip creates");

        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(arrangement.end_beat, 20.0, "the clip extends the song");
        let clip = arrangement
            .track_lanes[0]
            .iter()
            .find(|clip| clip.id == clip_id)
            .expect("created clip");
        assert_eq!((clip.start_beat, clip.end_beat), (12.0, 20.0));
        assert_eq!(clip.source(), LaneSource::Take(take_id));

        let take = app.state.track_take(0, take_id).expect("created take");
        assert_eq!(take.total_len_steps, 32);
        assert_eq!(take.chunks.len(), 1);
        let chunk = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(take.chunks[0])
                .expect("take chunk")
        });
        assert!(
            chunk.track_bits.iter().all(|word| *word == 0),
            "the new take must contain no triggers"
        );
        assert!(
            chunk.chord_snapshot.steps.iter().all(Vec::is_empty),
            "the new take must contain no chord notes"
        );
        assert_eq!(
            app.song_clip_selection.map(|selection| selection.clip_id),
            Some(clip_id),
            "the piano-roll focus follows the created clip"
        );
        assert_eq!(
            app.song_region_selection.map(|region| {
                (region.track_a, region.track_b, region.start_beat, region.end_beat)
            }),
            Some((0, 0, 12.0, 20.0)),
            "the created clip receives the normal one-clip selection"
        );
        assert_eq!(app.history.undo_len(), depth + 1, "one gesture, one entry");

        undo(&mut app);
        assert!(app.state.track_take(0, take_id).is_none());
        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(arrangement.end_beat, 16.0);
        assert!(
            arrangement.find_clip(clip_id).is_none(),
            "undo removes the clip together with its take"
        );
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

    /// A track-0 clip note bit inside one of `take`'s chunks.
    fn chunk_bit(app: &App, chunk: PatternId, step: usize) -> bool {
        app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(chunk)
                .map(|data| data.track_bits[step / 64] >> (step % 64) & 1 == 1)
                .unwrap_or(false)
        })
    }

    /// The clip-panel Length drag (clip-edit-target follow-up): frames
    /// coalesce into ONE undo entry; growth mints silence chunks whose device
    /// state matches, and undo/redo restore take and song together.
    #[test]
    fn take_set_length_coalesces_growth_into_one_undo_entry() {
        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let depth = app.history.undo_len();
        let merge_key = crate::app::history::MergeKey::new("test-take-length".to_string());

        // Two drag frames: 40, then 300 (crosses into a second chunk).
        app.song_take_set_length_coalesced(0, take_id, 40.0, merge_key.clone())
            .expect("frame 1");
        app.song_take_set_length_coalesced(0, take_id, 300.0, merge_key.clone())
            .expect("frame 2");
        crate::app::edit::finish_active_gesture(&mut app);

        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 300);
        assert_eq!(take.chunks.len(), 2, "one silence chunk minted");
        assert_eq!(app.history.undo_len(), depth + 1, "the whole drag is one entry");

        undo(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 32);
        assert_eq!(take.chunks.len(), 1);
        redo(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 300);
        assert_eq!(take.chunks.len(), 2);
    }

    /// A drag that returns to its starting length commits nothing: the
    /// minted-then-unneeded silence chunk is garbage-collected on the way
    /// back and the staged entry is discarded.
    #[test]
    fn take_set_length_round_trip_discards_the_entry() {
        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let depth = app.history.undo_len();
        let merge_key = crate::app::history::MergeKey::new("test-take-length".to_string());

        app.song_take_set_length_coalesced(0, take_id, 300.0, merge_key.clone())
            .expect("out");
        app.song_take_set_length_coalesced(0, take_id, 32.0, merge_key.clone())
            .expect("and back");
        crate::app::edit::finish_active_gesture(&mut app);

        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 32);
        assert_eq!(take.chunks.len(), 1, "the minted chunk was collected");
        assert_eq!(app.history.undo_len(), depth, "no undo entry for a no-op drag");
    }

    /// Shrinking keeps chunks that still hold notes (re-growing restores
    /// them); the length clamps so no clip's offset can fall outside the take.
    #[test]
    fn take_set_length_shrink_keeps_noted_chunks_and_clamps_to_clip_offsets() {
        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let merge_key = crate::app::history::MergeKey::new("test-take-length".to_string());
        app.song_take_set_length_coalesced(0, take_id, 300.0, merge_key.clone())
            .expect("grow");
        crate::app::edit::finish_active_gesture(&mut app);
        // A note in the second chunk (take step 260 → chunk 1, local 4).
        let chunk1 = app.state.track_take(0, take_id).expect("take").chunks[1];
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(chunk1, |data| {
                data.track_bits[0] |= 1 << 4;
            }));
        });

        // Split the take clip at beat 8 so a clip anchored at source step 16
        // exists: the shrink clamp must keep every clip offset inside the
        // take (`validate` rejects offset ≥ length).
        let clip_id = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| clip.take_id == Some(take_id.0))
            .expect("take clip painted")
            .id;
        app.arr_clip_split(clip_id, 8.0).expect("split");

        // Shrink to 1: clamps to the largest clip offset + 1, and the noted
        // chunk stays claimed.
        app.song_take_set_length_coalesced(0, take_id, 1.0, merge_key.clone())
            .expect("shrink");
        crate::app::edit::finish_active_gesture(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 17, "clamped to max clip offset + 1");
        assert_eq!(take.chunks.len(), 2, "the noted chunk survives the shrink");
        assert!(chunk_bit(&app, chunk1, 4));

        // Re-growing needs no new mint and the note is still there.
        app.song_take_set_length_coalesced(0, take_id, 300.0, merge_key)
            .expect("re-grow");
        crate::app::edit::finish_active_gesture(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 300);
        assert_eq!(take.chunks.len(), 2);
        assert!(chunk_bit(&app, chunk1, 4));
    }

    /// Dragging a take clip's right edge past its playable end grows the
    /// take (the drag asks for more length): take + clip resize in ONE
    /// composite undo entry.
    #[test]
    fn resizing_a_take_clip_past_its_end_grows_the_take_in_one_entry() {
        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let clip_id = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| clip.take_id == Some(take_id.0))
            .expect("take clip painted")
            .id;
        let depth = app.history.undo_len();

        // Playable end is beat 12 (32 steps at 4/beat from beat 4); drag the
        // right edge to beat 20 → the take must grow to 64 steps.
        app.arr_clip_resize(clip_id, 4.0, 20.0).expect("resize grows");
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 64);
        let (_, clip) = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .find_clip(clip_id)
            .map(|(track, clip)| (track, *clip))
            .expect("clip survives");
        assert_eq!((clip.start_beat, clip.end_beat), (4.0, 20.0));
        assert_eq!(app.history.undo_len(), depth + 1, "one composite entry");

        undo(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 32);
        let (_, clip) = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .find_clip(clip_id)
            .map(|(track, clip)| (track, *clip))
            .expect("clip restored");
        assert_eq!((clip.start_beat, clip.end_beat), (4.0, 12.0));

        redo(&mut app);
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 64);

        // The coalesced picker path still clamps rather than growing.
        let merge_key = crate::app::history::MergeKey::new("test-clip-resize".to_string());
        app.arr_clip_resize_coalesced(clip_id, 4.0, 64.0, merge_key)
            .expect("coalesced resize clamps");
        crate::app::edit::finish_active_gesture(&mut app);
        let (_, clip) = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .find_clip(clip_id)
            .map(|(track, clip)| (track, *clip))
            .expect("clip");
        assert_eq!(clip.end_beat, 20.0, "clamped to the playable end, no growth");
        let take = app.state.track_take(0, take_id).expect("take");
        assert_eq!(take.total_len_steps, 64);
    }

    /// §18.2 item 4 must-pass: two takes on one track share the scene's Mix
    /// by default, so a fader edit made while take 2 is selected is heard on
    /// take 1 (and the cell) — one fader for the track's sound.
    #[test]
    fn fader_edit_with_take_2_selected_is_heard_on_take_1() {
        use crate::app::sound_binding::{BoundSource, SongClipSelection};
        use crate::app::AppCommand;

        let mut app = app_with_song();
        let take_1 = app
            .song_region_to_take(0, 0.0, 4.0)
            .expect("take 1 converts");
        let take_2 = app
            .song_region_to_take(0, 8.0, 12.0)
            .expect("take 2 converts");
        let (chunk_1, sound_1, sound_2) = app.state.with_project_scenes(|scenes| {
            let t1 = scenes.take_pools[0].get(take_1).expect("take 1");
            let t2 = scenes.take_pools[0].get(take_2).expect("take 2");
            (t1.chunks[0], t1.sound, t2.sound)
        });
        assert_eq!(sound_1, sound_2, "both takes share the cell's sound (§17.3)");

        let clip_2 = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| clip.take_id == Some(take_2.0))
            .expect("take 2 clip painted")
            .id;
        app.set_arrangement_view_visible(true);
        app.set_song_clip_selection(Some(SongClipSelection {
            track: 0,
            clip_id: clip_2,
            source: BoundSource::Take(take_2),
        }));
        app.sync_track_sound_bindings();

        crate::app::edit::try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume { track: 0, value: 0.35 },
        )
        .expect("volume edit applies");
        crate::app::edit::finish_active_gesture(&mut app);

        let heard_on_take_1 = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(chunk_1)
                .expect("take 1 chunk")
                .track_params
                .volume
        });
        assert_eq!(
            heard_on_take_1, 0.35,
            "the shared Mix carries the fader to the unselected take"
        );
    }

    /// Mixer/track-param edits follow the sound binding and land on the
    /// bound source's Mix ENTITY (takes spec 17.3): the take shares the
    /// scene cell's sound by construction, so a take-bound fader move is
    /// heard by every referent of that Mix — cell included. One write, one
    /// entity, no fan-out and no divergence.
    #[test]
    fn mixer_volume_with_a_bound_take_writes_the_shared_mix() {
        use crate::app::sound_binding::{BoundSource, SongClipSelection};
        use crate::app::AppCommand;

        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let chunk0 = app.state.track_take(0, take_id).expect("take").chunks[0];
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(chunk0, |data| {
                data.track_params.volume = 0.9;
            }));
        });
        let scene_pattern = app
            .state
            .effective_track_pattern_id(0)
            .expect("effective pattern");
        // Ref identity (§17.3): the conversion shared the bound cell's
        // Patch/Mix pair, so the 0.9 written through the chunk above already
        // reached the scene cell — they are one Mix.
        app.state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[0].refs(chunk0),
                scenes.track_pools[0].refs(scene_pattern),
                "take chunks and the scene cell name the same entities"
            );
            assert_eq!(
                scenes.track_pools[0]
                    .get(scene_pattern)
                    .expect("scene pattern")
                    .track_params
                    .volume,
                0.9,
                "an entity write through the chunk is heard by the cell"
            );
        });

        let clip_id = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| clip.take_id == Some(take_id.0))
            .expect("take clip painted")
            .id;
        app.set_arrangement_view_visible(true);
        app.set_song_clip_selection(Some(SongClipSelection {
            track: 0,
            clip_id,
            source: BoundSource::Take(take_id),
        }));
        app.sync_track_sound_bindings();
        assert_eq!(app.state.pattern.track_params[0].get_volume(), 0.9);

        crate::app::edit::try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume { track: 0, value: 0.3 },
        )
        .expect("volume edit applies");
        crate::app::edit::finish_active_gesture(&mut app);

        let chunk_volume = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(chunk0)
                .expect("chunk pattern")
                .track_params
                .volume
        });
        assert_eq!(chunk_volume, 0.3, "the bound take owns the fader value");
        assert_eq!(
            app.state.pattern.track_params[0].get_volume(),
            0.3,
            "the live mirror (what the user hears while bound) moved too"
        );
        let scene_volume_after = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .track_params
                .volume
        });
        assert_eq!(
            scene_volume_after, 0.3,
            "shared Mix (§17.3): the fader edit applies to every referent"
        );
        // The chunk keeps its full step width — a fader move must never
        // resize a take chunk.
        let chunk_steps = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(chunk0)
                .expect("chunk pattern")
                .track_params
                .num_steps
        });
        assert_eq!(chunk_steps, MAX_STEPS);

        // Undo restores the chunk; the next binding sync re-borrows the
        // restored value into the live mirror.
        undo(&mut app);
        let chunk_volume = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(chunk0)
                .expect("chunk pattern")
                .track_params
                .volume
        });
        assert_eq!(chunk_volume, 0.9);
        app.sync_track_sound_bindings();
        assert_eq!(app.state.pattern.track_params[0].get_volume(), 0.9);
    }

    /// Playback of a take lane must SOUND the take's device state (takes
    /// spec 16.2/16.7), not the scene cell's: after every row transition
    /// stomps the live mirror with the scene pattern, the next tick's
    /// binding sync must re-borrow the chunk's devices and re-push them.
    #[test]
    fn row_transitions_rebind_and_repush_the_audible_takes_device_state() {
        use crate::app::sound_binding::{BoundSource, SongClipSelection};

        let mut app = app_with_song();
        let take_id = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("conversion succeeds");
        let chunk0 = app.state.track_take(0, take_id).expect("take").chunks[0];
        // The take's device snapshot diverges from the scene cells: a
        // distinctive volume on the chunk (the scene patterns keep default).
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(chunk0, |data| {
                data.track_params.volume = 0.25;
            }));
        });
        let scene_volume = app.state.pattern.track_params[0].get_volume();
        assert!((scene_volume - 0.25).abs() > 1e-3, "fixture needs divergence");

        // Select the take clip (rule 1) with the arrangement on screen.
        let clip_id = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| clip.take_id == Some(take_id.0))
            .expect("take clip painted")
            .id;
        app.set_arrangement_view_visible(true);
        app.set_song_clip_selection(Some(SongClipSelection {
            track: 0,
            clip_id,
            source: BoundSource::Take(take_id),
        }));
        app.sync_track_sound_bindings();
        assert_eq!(
            app.state.pattern.track_params[0].get_volume(),
            0.25,
            "stopped, the bound take's devices are the live mirror (16.2)"
        );

        // Play the song from beat 0 (row 0 plays the scene's pattern clip;
        // the take starts at beat 4).
        app.set_use_arrangement(true).expect("arrangement mode");
        app.song_transport_play(false).expect("song playback");
        let song = app.active_runtime_song.clone().expect("active song");
        app.sync_track_sound_bindings();

        // Reach the row where the take is audible.
        let take_row = song
            .rows
            .iter()
            .position(|row| {
                row.resolved_sources.first().copied()
                    == Some(crate::sequencer::LaneSource::Take(take_id))
            })
            .expect("a row plays the take");
        app.mirror_song_row_applied(&crate::sequencer::AudibleSongRowApplied {
            row_id: song.rows[take_row].id,
            row_ordinal: take_row,
            effective_beat: song.rows[take_row].start_beat,
            effective_sample: 0,
            wrapped: false,
        })
        .expect("mirror succeeds");
        // The row apply released the borrow and restored the scene pattern;
        // the reactive tick's sync must now re-borrow AND re-push.
        app.sync_track_sound_bindings();
        assert_eq!(
            app.loaded_sound_binding[0],
            Some(BoundSource::Take(take_id)),
            "the tick re-binds the audible take (rule 2/1)"
        );
        assert_eq!(
            app.state.pattern.track_params[0].get_volume(),
            0.25,
            "the live mirror shows the take's devices again"
        );
        assert!(
            app.sound_binding_monitored[0],
            "the audible take's sound was re-pushed to the engine after the \
             row apply stomped it (16.7: what is audible IS the mirror)"
        );
        assert!(!app.sound_binding_is_silent(0));
        app.song_transport_stop().expect("stop");
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

/// Ceiling on a take's playable length in steps (16 chunks). Keeps a runaway
/// panel drag or clip resize from minting an unbounded chunk list.
pub(crate) const TAKE_MAX_LEN_STEPS: u32 = 4096;

impl App {
    /// The smallest length `take_id` can shrink to without invalidating the
    /// committed arrangement: every clip playing the take must keep its
    /// offset strictly inside the take (`validate` rejects offset ≥ length).
    pub(crate) fn take_min_len_from_clips(&self, track: usize, take_id: TakeId) -> u32 {
        self.state
            .committed_arrangement()
            .and_then(|arrangement| {
                arrangement.track_lanes.get(track).map(|lane| {
                    lane.iter()
                        .filter(|clip| clip.take_id == Some(take_id.0))
                        .map(|clip| clip.offset_steps.floor().max(0.0) as u32 + 1)
                        .max()
                        .unwrap_or(1)
                })
            })
            .unwrap_or(1)
    }

    /// Set a take's playable length (the clip panel's Length field). Applied
    /// per picker drag frame; frames sharing `merge_key` coalesce into ONE
    /// staged undo entry — a composite of the scenes mutation (chunk list +
    /// `total_len_steps`) and an arrangement reinstall, so undo/redo restore
    /// and recompile both sides together. Clamped to `[max clip offset + 1,
    /// TAKE_MAX_LEN_STEPS]`; a drag that returns to its start discards the
    /// entry (empty minted chunks are garbage-collected on the way back).
    pub(crate) fn song_take_set_length_coalesced(
        &mut self,
        track: usize,
        take_id: TakeId,
        len_steps: f64,
        merge_key: crate::app::history::MergeKey,
    ) -> Result<(), String> {
        if self.song_edits_locked() {
            return Err(super::song_edit::SONG_EDITS_LOCKED_ERROR.to_string());
        }
        if !len_steps.is_finite() {
            return Err("Take length must be finite".to_string());
        }
        let take = self
            .state
            .track_take(track, take_id)
            .ok_or_else(|| format!("take {} does not exist on track {}", take_id.0, track + 1))?;
        let min_len = self.take_min_len_from_clips(track, take_id);
        // A take recorded past the ceiling (or a clip anchored past it) must
        // still be editable: raise the cap to whatever the take already needs
        // so the clamp bounds can never cross (`clamp` panics on min > max).
        let max_len = TAKE_MAX_LEN_STEPS.max(min_len).max(take.total_len_steps);
        let len = (len_steps.round() as i64).clamp(min_len as i64, max_len as i64) as u32;
        let continuing = self
            .history
            .active_gesture()
            .map(|gesture| &gesture.merge_key)
            == Some(&merge_key);
        if take.total_len_steps == len && !continuing {
            return Ok(());
        }
        // The first frame captures a synchronized before-state (sealing any
        // other gesture); continuation frames must NOT — the seal would
        // commit the very entry being coalesced.
        let scenes_current = if continuing {
            self.state.capture_project_scenes()
        } else {
            self.capture_synchronized_scene_structure_state()?
        };
        let arrangement_current = self.state.committed_arrangement();
        if take.total_len_steps != len {
            self.state.resize_track_take(track, take_id, len)?;
            // Reinstall to revalidate and recompile the song against the new
            // take shape (and bump the revision the lanes redraw on).
            if let Some(arrangement) = arrangement_current.clone() {
                if let Err(error) = self.state.set_committed_arrangement(Some(arrangement)) {
                    self.restore_scene_structure_state(&scenes_current)?;
                    return Err(format!(
                        "resizing the take left an invalid song and was rolled back: {error}"
                    ));
                }
            }
        }
        let scenes_after = self.state.capture_project_scenes();
        let (scenes_before, arrangement_before) = self
            .history
            .active_gesture_patch(&merge_key)
            .and_then(|patch| match patch {
                EditPatch::Composite(parts) => match (parts.first(), parts.get(1)) {
                    (
                        Some(EditPatch::Arrangement(arrangement)),
                        Some(EditPatch::SceneStructure(scenes)),
                    ) => Some((scenes.before.clone(), arrangement.before.clone())),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or((scenes_current, arrangement_current.clone()));
        // `ProjectScenes` has no equality; the take entity (chunk list +
        // length) is what this edit changes, so it decides the round-trip
        // discard. (A grow-then-shrink drag leaves only the pool's id
        // allocator bumped — not worth an undo entry.)
        let take_before = scenes_before
            .take_pools
            .get(track)
            .and_then(|takes| takes.get(take_id))
            .cloned();
        let take_after = self.state.track_take(track, take_id);
        if take_before == take_after && arrangement_before == arrangement_current {
            self.history.discard_active_gesture_entry(&merge_key);
        } else {
            let scene_patch = SceneStructurePatch {
                before: scenes_before,
                after: scenes_after,
            };
            let arrangement_patch = ArrangementStructurePatch {
                before: arrangement_before,
                after: arrangement_current,
            };
            let retained_bytes = scene_patch.retained_bytes()
                + arrangement_patch.retained_bytes() * 2;
            crate::app::edit::ensure_coalescing_gesture(self, &merge_key);
            // Only the arrangement patch recompiles the song; restoring the
            // scenes (the take's chunk list + length) does not. Composites
            // replay forward on redo and in REVERSE on undo, so the
            // arrangement is staged on BOTH sides of the scenes: whichever
            // direction runs, the last patch applied is an arrangement
            // reinstall that recompiles against the restored take geometry.
            self.history.stage_active_gesture(
                "Resize take",
                &merge_key,
                EditPatch::Composite(vec![
                    EditPatch::Arrangement(arrangement_patch.clone()),
                    EditPatch::SceneStructure(scene_patch),
                    EditPatch::Arrangement(arrangement_patch),
                ]),
                retained_bytes,
            );
        }
        self.rebuild_active_song_after_arrangement_edit();
        Ok(())
    }

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

    /// Mint a silent take over `[start_beat, end_beat)`, paint one take-backed
    /// arrangement clip, and select it as the edit focus. This is the
    /// arrangement background double-click primitive: the take owns real
    /// chunk storage from the start, so the piano roll can write into it
    /// without inventing a sourceless "empty clip".
    ///
    /// The take pool, chunk patterns, clip, and any song-end extension are one
    /// composite history entry. The template follows take recording's rule:
    /// clone the track's bound/effective sound state, then clear every
    /// step-owned lane.
    pub fn arr_empty_take_clip_create(
        &mut self,
        track: usize,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<(TakeId, ClipId), String> {
        if self.song_edits_locked() {
            return Err(super::song_edit::SONG_EDITS_LOCKED_ERROR.to_string());
        }
        if !start_beat.is_finite()
            || !end_beat.is_finite()
            || start_beat < 0.0
            || end_beat <= start_beat
        {
            return Err(format!(
                "An empty take clip needs a finite, positive span (got \
                 [{start_beat}, {end_beat}))"
            ));
        }
        let arrangement_before = self.require_arrangement()?;
        if track >= arrangement_before.track_lanes.len() {
            return Err(format!("Track {} has no arrangement lane", track + 1));
        }

        let bound = self.bound_read_pattern(track);
        // An empty take clip performs the current sound (§17.3): it shares
        // the bound cell's refs exactly like a recorded take.
        let bound_sound = self.bound_sound_refs(track);
        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        let mut template = scenes_before
            .track_pools
            .get(track)
            .and_then(|pool| bound.and_then(|id| pool.get(id)))
            .or_else(|| scenes_before.effective_track_pattern(track))
            .or_else(|| PatternSnapshot::new_default(1, &[]).track_pattern_data(0))
            .ok_or_else(|| format!("Could not build an empty take for track {}", track + 1))?;
        template.track_params.num_steps = MAX_STEPS;
        template.clear_step_content();
        let step_beats = template.track_params.timebase.step_beats(MAX_STEPS);
        if !(step_beats > 0.0) {
            return Err("The track's take template has a degenerate timebase".to_string());
        }
        let total_len_steps =
            ((end_beat - start_beat) / step_beats - 1e-6).ceil().max(1.0) as u32;
        let chunk_count = (total_len_steps as usize).div_ceil(MAX_STEPS);
        let chunks = vec![template; chunk_count];

        let take_id = self
            .state
            .register_track_take(track, None, chunks, total_len_steps, bound_sound)?;
        let mut arrangement_after = arrangement_before.clone();
        arrangement_after.end_beat = arrangement_after.end_beat.max(end_beat);
        let scenes = self.state.capture_project_scenes();
        let clip_id = match self.paint_take_clip(
            &mut arrangement_after,
            &scenes,
            track,
            start_beat,
            end_beat,
            take_id,
        ) {
            Ok(clip_id) => clip_id,
            Err(error) => {
                self.state.remove_track_take(track, take_id)?;
                self.restore_scene_structure_state(&scenes_before)?;
                return Err(error);
            }
        };
        if let Err(error) = self
            .state
            .set_committed_arrangement(Some(arrangement_after.clone()))
        {
            self.state.remove_track_take(track, take_id)?;
            self.restore_scene_structure_state(&scenes_before)?;
            return Err(format!(
                "empty take creation produced an invalid song and was rolled back: {error}"
            ));
        }

        let scenes_after = self.state.capture_project_scenes();
        finish_active_gesture(self);
        let scene_patch = SceneStructurePatch {
            before: scenes_before,
            after: scenes_after,
        };
        let arrangement_patch = ArrangementStructurePatch {
            before: Some(arrangement_before),
            after: Some(arrangement_after),
        };
        let retained_bytes = scene_patch.retained_bytes() + arrangement_patch.retained_bytes();
        self.history.commit(
            "Create take clip",
            None,
            EditPatch::Composite(vec![
                EditPatch::SceneStructure(scene_patch),
                EditPatch::Arrangement(arrangement_patch),
            ]),
            retained_bytes,
        );
        self.rebuild_active_song_after_arrangement_edit();

        // Selection is intentionally outside history, like every other clip
        // click. The take id is fresh, so its newly committed clip is the
        // unambiguous focus target.
        self.select_committed_take(track, take_id);
        self.set_song_region_for_clip(super::song_region::SongRegionSelection::new(
            track,
            track,
            start_beat,
            end_beat,
        ));
        Ok((take_id, clip_id))
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
        let arrangement = self.require_arrangement()?;
        // The compiled song is what resolves the region's audible content
        // (backdrop included); the arrangement is what the result is written
        // onto. An uncommitted (still-empty) arrangement resolves nothing,
        // so compile the fallback rather than erroring.
        let song = match self.state.committed_song() {
            Some(song) => song,
            None => self.state.with_project_scenes(|scenes| {
                crate::sequencer::compile_arrangement(&arrangement, scenes)
            })?,
        };
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
        // A consolidated region plays as the track currently sounds: share
        // the bound cell's refs (§17.3) rather than freezing the rendered
        // chunks' device state into a private pair.
        let bound_sound = self.bound_sound_refs(track);
        let take_id = self
            .state
            .register_track_take(track, None, chunks, total_len_steps, bound_sound)?;

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
            chunks[dst / MAX_STEPS].copy_step_content_from(dst % MAX_STEPS, &src_data, src_step);
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
    ) -> Result<ClipId, String> {
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
        Ok(id)
    }
}
