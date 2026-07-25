//! Song-mode editing primitives (docs/song-mode-spec.md section 5.6).
//!
//! Every song edit — host commands today, timeline gestures later — reduces
//! to the closed primitive set implemented here as methods on `App`. Each
//! primitive validates against spec 5.3 (plus the 5.6 gesture semantics),
//! applies atomically to the committed song, and commits exactly one history
//! entry (`SongStructurePatch`, a whole-object memento including
//! `next_row_id`). A failed primitive changes nothing and returns an
//! actionable error. Primitives are rejected while song playback or
//! arrangement capture is active — see `App::song_edits_locked`.

use crate::sequencer::{
    state_at_beat, LaneSource, PatternId, ProjectSong, ProjectSongRow, ProjectSongTrackOverride,
    SongRowId,
};

use super::edit::finish_active_gesture;
use super::history::{EditPatch, SongStructurePatch};
use super::App;

/// Error returned by every primitive while a Slice B transport mode locks
/// song editing (spec 5.6/13: single launch authority).
pub const SONG_EDITS_LOCKED_ERROR: &str =
    "song editing is unavailable during song playback/capture";

/// Caller-facing row description for `song_replace` (wholesale replacement:
/// `def-song`, future capture commit). Ids are allocated by the primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct SongRowSpec {
    pub start_beat: f64,
    pub scene: usize,
    pub overrides: Vec<ProjectSongTrackOverride>,
}

/// Sort overrides into ascending track order (the stored canonical form,
/// spec 5.3) and reject duplicates with an actionable error.
fn canonical_overrides(
    mut overrides: Vec<ProjectSongTrackOverride>,
) -> Result<Vec<ProjectSongTrackOverride>, String> {
    overrides.sort_by_key(|over| over.track);
    for pair in overrides.windows(2) {
        if pair[0].track == pair[1].track {
            return Err(format!(
                "More than one override was given for track {}",
                pair[0].track + 1
            ));
        }
    }
    Ok(overrides)
}

fn finite_beat(name: &str, beat: f64) -> Result<f64, String> {
    if !beat.is_finite() || beat < 0.0 {
        return Err(format!("{name} must be a finite, non-negative beat (got {beat})"));
    }
    Ok(beat)
}

impl App {
    /// Whether the song editing primitives must be rejected right now.
    ///
    /// Slice B seam: song edits are forbidden during `SongPlayback` and
    /// `ArrangementCapture` (spec 5.6/13). Those transport modes do not
    /// exist yet; Slice B's transport authority enum MUST feed
    /// `song_transport_locks_edits` (or replace this body with a mode
    /// check) when it lands.
    pub fn song_edits_locked(&self) -> bool {
        self.song_transport_locks_edits
    }

    /// Restore the committed song to `target` (undo/redo replay). Validates
    /// a `Some` target against the current project before installing it.
    pub(crate) fn restore_committed_song_state(
        &mut self,
        target: &Option<ProjectSong>,
    ) -> Result<(), String> {
        if let Some(song) = target {
            let scenes = self.state.capture_project_scenes();
            song.validate(&scenes)
                .map_err(|error| format!("Song history no longer matches the project: {error}"))?;
        }
        self.state.set_committed_song(target.clone());
        Ok(())
    }

    /// Shared primitive tail: validate the candidate, install it atomically,
    /// and commit exactly one history entry. On any error the committed song
    /// is untouched and no history entry is created.
    pub(super) fn commit_song_edit(
        &mut self,
        label: &'static str,
        before: Option<ProjectSong>,
        after: Option<ProjectSong>,
    ) -> Result<(), String> {
        if let Some(song) = &after {
            let scenes = self.state.capture_project_scenes();
            song.validate(&scenes)?;
        }
        self.state.set_committed_song(after.clone());
        finish_active_gesture(self);
        let patch = SongStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history
            .commit(label, None, EditPatch::Song(patch), retained_bytes);
        Ok(())
    }

    pub(super) fn require_song_edit_unlocked(&self) -> Result<(), String> {
        if self.song_edits_locked() {
            return Err(SONG_EDITS_LOCKED_ERROR.to_string());
        }
        Ok(())
    }

    /// Steps-per-beat and step count of `pattern_id` in `track`'s pool under
    /// the pattern's base timebase — the `steps()` mapping used for offset
    /// stamping (takes spec 7.2/7.4). Per-step timebase plocks deliberately
    /// do not participate in stamping (spec 15); the runtime resolves the
    /// stamped step offset through the track's live boundaries.
    fn pattern_step_mapping(&self, track: usize, pattern_id: u64) -> Option<(f64, f64)> {
        self.state.with_project_scenes(|scenes| {
            let data = scenes.track_pools.get(track)?.get(PatternId(pattern_id))?;
            let num_steps = data.track_params.num_steps.max(1);
            let step_beats = data.track_params.timebase.step_beats(num_steps);
            (step_beats > 0.0).then(|| (1.0 / step_beats, num_steps as f64))
        })
    }

    /// Steps-per-beat and playable length of `take_id` on `track` (the
    /// chunk-domain `steps()` mapping, takes spec 6.1: chunks are
    /// `MAX_STEPS`-long patterns under the first chunk's base timebase).
    pub(crate) fn take_step_mapping(&self, track: usize, take_id: u64) -> Option<(f64, f64)> {
        self.state.with_project_scenes(|scenes| {
            let take = scenes
                .take_pools
                .get(track)?
                .get(crate::sequencer::TakeId(take_id))?;
            let first_chunk = scenes
                .track_pools
                .get(track)?
                .get(*take.chunks.first()?)?;
            let step_beats = first_chunk
                .track_params
                .timebase
                .step_beats(crate::sequencer::MAX_STEPS);
            (step_beats > 0.0).then(|| (1.0 / step_beats, take.total_len_steps as f64))
        })
    }

    /// Advance `offset_steps` by `delta_beats` of playback in the given
    /// pattern's step domain, normalized into `[0, num_steps)`. Offsets
    /// within stamping epsilon of a pattern boundary collapse to 0 so
    /// scene-resolved lanes stay implicit whenever they can.
    pub(crate) fn advanced_offset(
        &self,
        track: usize,
        pattern_id: u64,
        offset_steps: f64,
        delta_beats: f64,
    ) -> f64 {
        let Some((steps_per_beat, num_steps)) = self.pattern_step_mapping(track, pattern_id) else {
            return offset_steps;
        };
        let advanced = (offset_steps + delta_beats * steps_per_beat).rem_euclid(num_steps);
        if advanced < 1e-9 || advanced > num_steps - 1e-9 {
            0.0
        } else {
            advanced
        }
    }

    /// Launch state for a row split at `split_beat` inside `governing`'s span
    /// (takes spec 7.4): the same resolved sources with every pattern lane's
    /// offset advanced by `steps(split_beat - governing.start_beat)`, so a
    /// split made to edit one track is phase-transparent to every other
    /// track. Scene-resolved lanes whose advanced offset is nonzero get a
    /// materialized override (locked decision: phase lives on overrides
    /// only; rows and scenes carry none).
    pub(super) fn split_row_state(
        &self,
        governing: &ProjectSongRow,
        split_beat: f64,
    ) -> Vec<ProjectSongTrackOverride> {
        let delta_beats = split_beat - governing.start_beat;
        let scene_cells: Vec<Option<PatternId>> = self.state.with_project_scenes(|scenes| {
            (0..scenes.track_pools.len())
                .map(|track| {
                    scenes
                        .scenes
                        .get(governing.scene)
                        .and_then(|scene| scene.cells.get(track))
                        .copied()
                        .flatten()
                })
                .collect()
        });
        let mut overrides = Vec::new();
        for (track, cell) in scene_cells.iter().enumerate() {
            let existing = governing.overrides.iter().find(|over| over.track == track);
            match existing {
                Some(over) => {
                    let mut over = *over;
                    if let Some(take_id) = over.take_id {
                        // Take lanes advance linearly, never wrapping (takes
                        // spec 6.1); a split past the take end becomes an
                        // explicit-empty override (the silent tail).
                        if let Some((steps_per_beat, total_len)) =
                            self.take_step_mapping(track, take_id)
                        {
                            let advanced = over.offset_steps + delta_beats * steps_per_beat;
                            if advanced >= total_len - 1e-6 {
                                over = ProjectSongTrackOverride::new(track, None);
                            } else {
                                over.offset_steps = advanced.max(0.0);
                            }
                        }
                    } else if let Some(pattern_id) = over.pattern_id {
                        over.offset_steps = self.advanced_offset(
                            track,
                            pattern_id,
                            over.offset_steps,
                            delta_beats,
                        );
                    }
                    overrides.push(over);
                }
                None => {
                    // Scene-resolved lane: materialize only when continuity
                    // needs a nonzero offset at the split beat.
                    if let Some(pattern) = cell {
                        let offset =
                            self.advanced_offset(track, pattern.0, 0.0, delta_beats);
                        if offset != 0.0 {
                            overrides.push(ProjectSongTrackOverride {
                                track,
                                pattern_id: Some(pattern.0),
                                take_id: None,
                                offset_steps: offset,
                            });
                        }
                    }
                }
            }
        }
        overrides
    }

    /// Split the song at `beat` so a row starts exactly there, preserving what
    /// was playing (offsets advanced per `split_row_state`, so the split is
    /// inaudible). A no-op when a row already starts there.
    ///
    /// Shared by the paint helper's two edge splits and by the ripple insert
    /// in `song_region_duplicate`, which needs the suffix to begin on a real
    /// row before it slides right.
    pub(super) fn split_song_row_at(
        &self,
        song: &mut ProjectSong,
        beat: f64,
    ) -> Result<(), String> {
        if song.rows.iter().any(|row| row.start_beat == beat) {
            return Ok(());
        }
        let governing = state_at_beat(song, beat)
            .ok_or_else(|| format!("no song row governs beat {beat}"))?;
        let (scene, overrides) = (governing.scene, self.split_row_state(governing, beat));
        let row_id = song.allocate_row_id()?;
        let position = song
            .rows
            .iter()
            .position(|row| row.start_beat > beat)
            .unwrap_or(song.rows.len());
        song.rows.insert(
            position,
            ProjectSongRow {
                id: row_id,
                start_beat: beat,
                scene,
                overrides,
            },
        );
        Ok(())
    }

    /// The row surgery every arrangement paint shares (region spec 5.2): split
    /// the song at both region edges (phase-transparently, `split_row_state`)
    /// and set `track`'s override on every row inside `[start_beat, end_beat)`,
    /// re-anchoring each row into the painted clip.
    ///
    /// Operates on a caller-owned `&mut ProjectSong` and does NOT normalize,
    /// validate or commit: the region primitives call it once per painted span
    /// on ONE clone and commit the result as a single undo entry. The two
    /// single-span entry points (`song_track_paint_anchored`,
    /// `paint_take_region`) are thin wrappers that add their own validation and
    /// normalization.
    ///
    /// Anchoring (takes spec 7.1): each painted row is stamped
    /// `offset = anchor_offset + steps(row.start_beat - anchor_beat)` in the
    /// source's own step domain, so the span plays as one continuous clip
    /// across its internal row splits. Pattern sources wrap at the pattern
    /// length; take sources advance linearly and never wrap (takes spec 6.1).
    pub(super) fn paint_source_region(
        &self,
        song: &mut ProjectSong,
        track: usize,
        start_beat: f64,
        end_beat: f64,
        source: LaneSource,
        anchor_beat: f64,
        anchor_offset_steps: f64,
    ) -> Result<(), String> {
        // Steps-per-beat of a take source, resolved once: takes advance
        // linearly, so the per-row offset is a plain multiplication.
        let take_steps_per_beat = match source {
            LaneSource::Take(take_id) => Some(
                self.take_step_mapping(track, take_id.0)
                    .map(|(steps_per_beat, _)| steps_per_beat)
                    .ok_or_else(|| {
                        format!(
                            "take {} on track {} has no step mapping to paint with",
                            take_id.0,
                            track + 1
                        )
                    })?,
            ),
            _ => None,
        };

        // Restore row: the state in effect at `end_beat` must resume there,
        // with lane offsets advanced so the split itself is inaudible.
        if end_beat < song.end_beat {
            self.split_song_row_at(song, end_beat)?;
        }
        // Split row at the paint start (row zero always exists at 0.0, so a
        // governing row is guaranteed).
        self.split_song_row_at(song, start_beat)?;

        // Set the painted override on every row inside the region.
        for idx in 0..song.rows.len() {
            let row_start = song.rows[idx].start_beat;
            if row_start < start_beat || row_start >= end_beat {
                continue;
            }
            let over = match source {
                LaneSource::Empty => ProjectSongTrackOverride::new(track, None),
                LaneSource::Pattern(pattern_id) => ProjectSongTrackOverride {
                    track,
                    pattern_id: Some(pattern_id.0),
                    take_id: None,
                    offset_steps: self.advanced_offset(
                        track,
                        pattern_id.0,
                        anchor_offset_steps,
                        row_start - anchor_beat,
                    ),
                },
                LaneSource::Take(take_id) => {
                    let steps_per_beat = take_steps_per_beat.expect("resolved for take sources");
                    ProjectSongTrackOverride::new_take(
                        track,
                        take_id.0,
                        (anchor_offset_steps + (row_start - anchor_beat) * steps_per_beat).max(0.0),
                    )
                }
            };
            let row = &mut song.rows[idx];
            row.overrides.retain(|existing| existing.track != track);
            row.overrides.push(over);
            row.overrides.sort_by_key(|over| over.track);
        }
        Ok(())
    }

    /// Set the song's explicit end beat.
    pub fn song_set_end(&mut self, end_beat: f64) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let end_beat = finite_beat("Song end beat", end_beat)?;
        let before = self.state.committed_song();
        let Some(existing) = &before else {
            return Err("The project has no song".to_string());
        };
        if existing.end_beat == end_beat {
            return Ok(());
        }
        let mut song = existing.clone();
        song.end_beat = end_beat;
        self.commit_song_edit("Set song end", before, Some(song))
    }

    /// Enable or disable song looping.
    pub fn song_set_loop(&mut self, enabled: bool) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.state.committed_song();
        let Some(existing) = &before else {
            return Err("The project has no song".to_string());
        };
        if existing.loop_enabled == enabled {
            return Ok(());
        }
        let mut song = existing.clone();
        song.loop_enabled = enabled;
        self.commit_song_edit("Set song loop", before, Some(song))
    }

    /// Replace the song wholesale (`def-song`, future capture commit).
    /// Allocates fresh row ids continuing from the previous song's allocator
    /// (`next_row_id` is monotonic within a project and ids are never
    /// reused, spec 5.2). Returns the new rows' ids in row order.
    pub fn song_replace(
        &mut self,
        rows: Vec<SongRowSpec>,
        end_beat: f64,
        loop_enabled: bool,
    ) -> Result<Vec<SongRowId>, String> {
        self.require_song_edit_unlocked()?;
        if rows.is_empty() {
            return Err(
                "song-replace requires at least one row; use song-clear to remove the song"
                    .to_string(),
            );
        }
        let end_beat = finite_beat("Song end beat", end_beat)?;
        let before = self.state.committed_song();
        let mut song = ProjectSong {
            rows: Vec::with_capacity(rows.len()),
            end_beat,
            loop_enabled,
            next_row_id: before.as_ref().map(|song| song.next_row_id).unwrap_or(0),
        };
        let mut sorted = rows;
        for row in &mut sorted {
            row.start_beat = finite_beat("Song row start beat", row.start_beat)?;
        }
        sorted.sort_by(|a, b| {
            a.start_beat
                .partial_cmp(&b.start_beat)
                .expect("song row start beats are finite")
        });
        let mut ids = Vec::with_capacity(sorted.len());
        for spec in sorted {
            let row_id = song.allocate_row_id()?;
            ids.push(row_id);
            song.rows.push(ProjectSongRow {
                id: row_id,
                start_beat: spec.start_beat,
                scene: spec.scene,
                overrides: canonical_overrides(spec.overrides)?,
            });
        }
        self.commit_song_edit("Replace song", before, Some(song))?;
        Ok(ids)
    }

    /// Remove the committed song entirely.
    pub fn song_clear(&mut self) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.state.committed_song();
        if before.is_none() {
            return Err("The project has no song to clear".to_string());
        }
        self.commit_song_edit("Clear song", before, None)
    }
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
        default_empty_effect_chain, PatternSnapshot, SequencerState,
    };

    /// One-track app with three scenes; per-track pattern-pool ids are 1..=3
    /// with scene j's cell holding PatternId(j + 1).
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
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
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

    /// Three-row song: 0.0 scene 0, 4.0 scene 1, 8.0 scene 2, end 16.
    fn app_with_song() -> App {
        let mut app = test_app();
        app.song_replace(
            vec![
                spec(0.0, 0, Vec::new()),
                spec(4.0, 1, Vec::new()),
                spec(8.0, 2, Vec::new()),
            ],
            16.0,
            false,
        )
        .expect("song_replace succeeds");
        app
    }

    fn committed(app: &App) -> ProjectSong {
        app.state.committed_song().expect("song committed")
    }

    #[track_caller]
    fn assert_rejected_unchanged(
        app: &mut App,
        run: impl FnOnce(&mut App) -> Result<(), String>,
        expect_in_error: &str,
    ) {
        let song_before = app.state.committed_song();
        let depth_before = app.history.undo_len();
        let error = run(app).expect_err("primitive must be rejected");
        assert!(
            error.contains(expect_in_error),
            "error {error:?} should contain {expect_in_error:?}"
        );
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "a rejected primitive must leave the song unchanged"
        );
        assert_eq!(
            app.history.undo_len(),
            depth_before,
            "a rejected primitive must not create history entries"
        );
    }

    #[test]
    fn set_end_and_set_loop_validate_and_commit_one_entry_each() {
        let mut app = app_with_song();
        let depth = app.history.undo_len();
        app.song_set_end(32.0).expect("set-end succeeds");
        assert_eq!(committed(&app).end_beat, 32.0);
        assert_eq!(app.history.undo_len(), depth + 1);
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_set_end(8.0),
            "greater than the last row's start beat",
        );

        let depth = app.history.undo_len();
        app.song_set_loop(true).expect("set-loop succeeds");
        assert!(committed(&app).loop_enabled);
        assert_eq!(app.history.undo_len(), depth + 1);
        // Same-value set is a no-op with no entry.
        app.song_set_loop(true).expect("no-op set-loop succeeds");
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn replace_allocates_fresh_ids_continuing_the_allocator() {
        let mut app = app_with_song();
        assert_eq!(committed(&app).next_row_id, 3);
        let depth = app.history.undo_len();
        let ids = app
            .song_replace(
                vec![spec(0.0, 2, Vec::new()), spec(10.5, 0, vec![ov(0, 2)])],
                24.0,
                true,
            )
            .expect("replace succeeds");
        assert_eq!(ids, vec![SongRowId(3), SongRowId(4)]);
        let song = committed(&app);
        assert_eq!(song.next_row_id, 5, "ids are never reused within a project");
        assert_eq!(song.rows[1].start_beat, 10.5, "fractional beats preserved");
        assert!(song.loop_enabled);
        assert_eq!(app.history.undo_len(), depth + 1, "wholesale replace is one entry");
    }

    #[test]
    fn replace_validation_failure_leaves_previous_song() {
        let mut app = app_with_song();
        assert_rejected_unchanged(
            &mut app,
            |app| {
                app.song_replace(vec![spec(0.0, 9, Vec::new())], 8.0, false)
                    .map(|_| ())
            },
            "scene 10",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_replace(Vec::new(), 8.0, false).map(|_| ()),
            "at least one row",
        );
        // First row not at 0.0 fails 5.3.
        assert_rejected_unchanged(
            &mut app,
            |app| {
                app.song_replace(vec![spec(2.0, 0, Vec::new())], 8.0, false)
                    .map(|_| ())
            },
            "must start at beat 0.0",
        );
    }

    #[test]
    fn clear_removes_song_and_requires_one() {
        let mut app = app_with_song();
        let depth = app.history.undo_len();
        app.song_clear().expect("clear succeeds");
        assert!(app.state.committed_song().is_none());
        assert_eq!(app.history.undo_len(), depth + 1);
        let error = app.song_clear().expect_err("clearing nothing fails");
        assert!(error.contains("no song"), "{error}");
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn undo_redo_restore_exact_song_including_next_row_id() {
        let mut app = app_with_song();
        let before = app.state.committed_song();
        app.song_set_end(24.0).unwrap();
        let after = app.state.committed_song();
        assert_ne!(before, after);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), before);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), after);

        // Replace bumps the allocator; undo must rewind it exactly.
        app.song_replace(vec![spec(0.0, 2, Vec::new())], 8.0, false)
            .unwrap();
        assert_eq!(committed(&app).next_row_id, 4);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), after);
        assert_eq!(committed(&app).next_row_id, 3);

        // Clearing round-trips through None.
        app.song_clear().unwrap();
        assert!(app.state.committed_song().is_none());
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), after);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.committed_song().is_none());
    }

    #[test]
    fn primitives_are_rejected_while_song_edits_are_locked() {
        let mut app = app_with_song();
        app.song_transport_locks_edits = true;
        assert_rejected_unchanged(&mut app, |app| app.song_set_end(64.0), SONG_EDITS_LOCKED_ERROR);
        assert_rejected_unchanged(&mut app, |app| app.song_set_loop(true), SONG_EDITS_LOCKED_ERROR);
        assert_rejected_unchanged(
            &mut app,
            |app| {
                app.song_replace(vec![spec(0.0, 0, Vec::new())], 8.0, false)
                    .map(|_| ())
            },
            SONG_EDITS_LOCKED_ERROR,
        );
        assert_rejected_unchanged(&mut app, |app| app.song_clear(), SONG_EDITS_LOCKED_ERROR);

        // Unlocking restores editability (the Slice B seam is just a flag).
        app.song_transport_locks_edits = false;
        app.song_set_loop(true).expect("unlocked primitive succeeds");
    }

    /// `song_replace` still canonicalizes its override sets: the declarative
    /// row path (`def-song`, capture commit) is the only caller left, and it
    /// must keep refusing two overrides for one track.
    #[test]
    fn duplicate_override_tracks_are_rejected() {
        let mut app = app_with_song();
        assert_rejected_unchanged(
            &mut app,
            |app| {
                app.song_replace(
                    vec![spec(0.0, 0, vec![ov(0, 1), ov(0, 2)])],
                    16.0,
                    false,
                )
                .map(|_| ())
            },
            "More than one override",
        );
    }
}
