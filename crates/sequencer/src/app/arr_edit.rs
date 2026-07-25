//! Arrangement editing primitives (docs/arrangement-lane-model-spec.md 8).
//!
//! Phase 2 of the spec's phasing lands only the whole-arrangement ops —
//! `arr_replace` and `arr_clear`, direct ports of `song_replace` /
//! `song_clear` — plus the `def-song` lowering that feeds them. Every
//! primitive is guarded by `require_song_edit_unlocked`, commits exactly one
//! `ArrangementStructurePatch`, and ends with validate → recompile → install
//! (`SequencerState::set_committed_arrangement`, which keeps the compiled
//! song and `song_revision` in lockstep). A failed primitive changes nothing.

use crate::sequencer::{
    lower_rows_to_arrangement, ProjectArrangement, ProjectSongRow, SongRowId,
};

use super::edit::finish_active_gesture;
use super::history::{ArrangementStructurePatch, EditPatch};
use super::song_edit::SongRowSpec;
use super::App;

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

/// Validate a declarative row list (`def-song`) exactly as `song_replace`
/// would, then lower it to lanes with `lower_rows_to_arrangement`.
///
/// The lowering itself lives in `sequencer::state::arrangement` because
/// `set_committed_song` needs it too. What lives here is the *declarative*
/// input contract: `SongRowSpec` is unvalidated caller data, while the
/// lowering assumes everything `ProjectSong::validate` guarantees. Each check
/// below is a verbatim port of the row path's, message included, so a
/// definition `song_replace` refused is refused identically here.
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
        compile_arrangement, default_empty_effect_chain, ArrClip, ClipId, PatternId,
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

    /// Assert the exact relationship a lowering guarantees (spec 8): compile
    /// reproduces the song row for row — same beats, same scenes, every
    /// declared override verbatim — and may only *add* overrides, on tracks
    /// the row did not mention, for lanes whose scene-backdrop phase the row
    /// model could not express.
    #[track_caller]
    fn assert_compiles_back_to(compiled: &ProjectSong, song: &ProjectSong) {
        assert_eq!(compiled.end_beat, song.end_beat);
        assert_eq!(compiled.loop_enabled, song.loop_enabled);
        assert_eq!(
            compiled.rows.iter().map(|row| row.start_beat).collect::<Vec<_>>(),
            song.rows.iter().map(|row| row.start_beat).collect::<Vec<_>>(),
            "every row start is a boundary and no boundary is invented"
        );
        for (compiled_row, row) in compiled.rows.iter().zip(&song.rows) {
            assert_eq!(compiled_row.scene, row.scene, "beat {}", row.start_beat);
            for over in &row.overrides {
                assert!(
                    compiled_row.overrides.contains(over),
                    "beat {}: declared override {over:?} must survive verbatim, got {:?}",
                    row.start_beat,
                    compiled_row.overrides
                );
            }
            for over in &compiled_row.overrides {
                if row.overrides.contains(over) {
                    continue;
                }
                assert!(
                    !row.overrides.iter().any(|declared| declared.track == over.track),
                    "beat {}: track {} was declared but compiled differently ({over:?})",
                    row.start_beat,
                    over.track + 1
                );
                assert_ne!(
                    over.offset_steps, 0.0,
                    "beat {}: the only added overrides are materialized backdrop phase",
                    row.start_beat
                );
            }
        }
    }

    /// The row path is the legacy authoring path, but only the arrangement is
    /// serialized — so a song installed through it must arrive with an
    /// arrangement, or the next save would write `arrangement: null` and
    /// destroy the project's song.
    #[test]
    fn set_committed_song_derives_an_arrangement_that_compiles_back() {
        let app = test_app();
        // Clips, an explicit-empty, and a take — every source kind.
        // Install a 300-step, two-chunk take on track 0 through the same
        // seam the loader uses.
        let chunk = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].get(PatternId(1)).cloned())
            .expect("track 0 pool holds scene 0's pattern");
        app.state.install_project_arrangement(
            &[],
            vec![
                (
                    4,
                    vec![(3, "Take 4".to_string(), 300, vec![chunk.clone(), chunk])],
                ),
                (0, Vec::new()),
            ],
        );
        let take = crate::sequencer::TakeId(3);
        let song = ProjectSong {
            rows: vec![
                ProjectSongRow {
                    id: SongRowId(7),
                    start_beat: 0.0,
                    scene: 0,
                    overrides: vec![ov(0, 2)],
                },
                ProjectSongRow {
                    id: SongRowId(8),
                    start_beat: 8.0,
                    scene: 1,
                    overrides: vec![
                        ProjectSongTrackOverride::new_take(0, take.0, 0.0),
                        ProjectSongTrackOverride::new(1, None),
                    ],
                },
                ProjectSongRow {
                    id: SongRowId(9),
                    start_beat: 16.0,
                    scene: 1,
                    overrides: Vec::new(),
                },
            ],
            end_beat: 32.0,
            loop_enabled: true,
            next_row_id: 10,
        };
        app.state.set_committed_song(Some(song.clone()));

        let arrangement = app
            .state
            .committed_arrangement()
            .expect("a row-path song must bring an arrangement");
        assert_eq!(arrangement.end_beat, 32.0);
        assert!(arrangement.loop_enabled);
        // One event per scene change, not one per row.
        assert_eq!(arrangement.scene_lane.len(), 2);
        assert_compiles_back_to(&compiled(&app, &arrangement), &song);

        // Clearing still clears both.
        app.state.set_committed_song(None);
        assert_eq!(app.state.committed_arrangement(), None);
        assert_eq!(app.state.committed_song(), None);
    }

    /// Every row-path edit must survive a save/reload: the song is not
    /// serialized any more, so if the derived arrangement were missing the
    /// whole song would vanish from the file.
    #[test]
    fn row_path_edit_then_save_preserves_the_song() {
        let mut app = test_app();
        app.arr_replace(ProjectArrangement {
            scene_lane: vec![SceneEvent {
                start_beat: 0.0,
                scene: 0,
            }],
            track_lanes: vec![vec![ArrClip::new(ClipId(0), 0.0, 8.0, Some(2))], Vec::new()],
            end_beat: 16.0,
            loop_enabled: false,
            next_clip_id: 1,
        })
        .expect("arrangement installs");

        // A row primitive — the surviving legacy path — rewrites the song.
        app.song_row_insert(12.0, 1, vec![ov(1, 3)])
            .expect("row insert applies");
        let song = app.state.committed_song().expect("song after the row edit");
        assert!(song.rows.iter().any(|row| row.start_beat == 12.0));

        // Save: only the arrangement reaches the file.
        let scenes = app.state.capture_project_scenes();
        let arrangement = app
            .state
            .committed_arrangement()
            .expect("the row edit must leave an arrangement to save");
        let serialized = crate::sequencer::arrangement_for_serialization(&arrangement, &scenes)
            .expect("serializable");
        let json = serde_json::to_string(&serialized).expect("serialize");

        // Reload: compile against the (unchanged) live scenes.
        let reloaded: ProjectArrangement = serde_json::from_str(&json).expect("deserialize");
        app.state
            .set_committed_arrangement(Some(reloaded))
            .expect("the reloaded arrangement installs");
        assert_compiles_back_to(
            &app.state.committed_song().expect("song after reload"),
            &song,
        );
    }

    /// A song that does not fit the project cannot lower. The fallback is
    /// documented: the song still installs (playback is unaffected), the
    /// arrangement is cleared rather than left stale, and the reason goes to
    /// stderr.
    #[test]
    fn set_committed_song_clears_the_arrangement_when_lowering_fails() {
        let app = test_app();
        app.state
            .set_committed_arrangement(Some(ProjectArrangement::new(2, 16.0)))
            .expect("arrangement installs");
        // Pattern 9 is in no pool, so validation of the lowered arrangement
        // fails.
        let song = ProjectSong {
            rows: vec![ProjectSongRow {
                id: SongRowId(0),
                start_beat: 0.0,
                scene: 0,
                overrides: vec![ov(0, 9)],
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
            "never left stale: an arrangement that cannot be derived is cleared"
        );
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
    #[track_caller]
    fn assert_def_song_round_trips(rows: Vec<SongRowSpec>, end_beat: f64, loop_enabled: bool) {
        let mut row_app = test_app();
        row_app
            .song_replace(rows.clone(), end_beat, loop_enabled)
            .expect("song_replace succeeds");
        let from_rows = row_app.state.committed_song().expect("row song");

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
            .arr_replace_rows(rows.clone(), 50.0, false)
            .expect_err("a row past the end must be rejected, not silently dropped");
        assert!(error.contains("greater than the last row's start beat"), "{error}");
        assert_eq!(app.state.committed_arrangement(), None);
        // Exactly what the row path says, too.
        let mut row_app = test_app();
        let row_error = row_app
            .song_replace(rows, 50.0, false)
            .expect_err("the row path rejects it as well");
        assert_eq!(row_error, error);
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
}
