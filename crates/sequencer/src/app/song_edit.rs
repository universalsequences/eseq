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
    state_at_beat, ProjectSong, ProjectSongRow, ProjectSongTrackOverride, SongRowId,
};

use super::edit::finish_active_gesture;
use super::history::{EditPatch, SongStructurePatch};
use super::App;

/// Error returned by every primitive while a Slice B transport mode locks
/// song editing (spec 5.6/13: single launch authority).
pub const SONG_EDITS_LOCKED_ERROR: &str =
    "song editing is unavailable during song playback/capture";

/// End beat assigned when inserting the first row into an empty song: the
/// insert beat (required to be 0.0 per spec 5.3) plus this many quarter-note
/// beats. Callers are expected to follow up with `song-set-end`; the default
/// only exists so the created one-row song is immediately valid
/// (`end_beat > last start_beat`). The status line reports it.
pub const EMPTY_SONG_DEFAULT_LENGTH_BEATS: f64 = 4.0;

/// Result of `App::song_row_insert`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongRowInsertOutcome {
    pub row_id: SongRowId,
    /// `Some(end_beat)` when the insert created the song from empty and
    /// assigned `EMPTY_SONG_DEFAULT_LENGTH_BEATS` as the documented default
    /// end; callers should surface it in the status message.
    pub created_with_end_beat: Option<f64>,
}

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
    fn commit_song_edit(
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

    fn require_song_edit_unlocked(&self) -> Result<(), String> {
        if self.song_edits_locked() {
            return Err(SONG_EDITS_LOCKED_ERROR.to_string());
        }
        Ok(())
    }

    /// Insert a row at `start_beat` splitting the governing span (spec 5.6).
    ///
    /// On an empty song the insert must be at beat 0.0 and the created song
    /// gets `EMPTY_SONG_DEFAULT_LENGTH_BEATS` as its documented default end
    /// (reported through the returned outcome).
    pub fn song_row_insert(
        &mut self,
        start_beat: f64,
        scene: usize,
        overrides: Vec<ProjectSongTrackOverride>,
    ) -> Result<SongRowInsertOutcome, String> {
        self.require_song_edit_unlocked()?;
        let start_beat = finite_beat("Song row start beat", start_beat)?;
        let overrides = canonical_overrides(overrides)?;
        let before = self.state.committed_song();

        let (after, outcome) = match &before {
            None => {
                if start_beat != 0.0 {
                    return Err(format!(
                        "The project has no song yet; the first row must be inserted at beat 0.0 \
                         (got {start_beat})"
                    ));
                }
                let mut song = ProjectSong {
                    rows: Vec::new(),
                    end_beat: EMPTY_SONG_DEFAULT_LENGTH_BEATS,
                    loop_enabled: false,
                    next_row_id: 0,
                };
                let row_id = song.allocate_row_id()?;
                song.rows.push(ProjectSongRow {
                    id: row_id,
                    start_beat,
                    scene,
                    overrides,
                });
                let outcome = SongRowInsertOutcome {
                    row_id,
                    created_with_end_beat: Some(song.end_beat),
                };
                (song, outcome)
            }
            Some(existing) => {
                let mut song = existing.clone();
                if start_beat >= song.end_beat {
                    return Err(format!(
                        "Cannot insert a song row at beat {start_beat}: the song ends at beat {}; \
                         extend it with song-set-end first",
                        song.end_beat
                    ));
                }
                if song.rows.iter().any(|row| row.start_beat == start_beat) {
                    return Err(format!(
                        "A song row already starts at beat {start_beat}; edit it with \
                         song-row-set-state or move it first"
                    ));
                }
                if let Some(governing) = state_at_beat(&song, start_beat) {
                    if governing.scene == scene && governing.overrides == overrides {
                        return Err(format!(
                            "Inserting at beat {start_beat} was rejected: the new row's state is \
                             identical to the governing row's and would be normalized away; \
                             insert a different state or use song-row-set-state"
                        ));
                    }
                }
                let row_id = song.allocate_row_id()?;
                let position = song
                    .rows
                    .iter()
                    .position(|row| row.start_beat > start_beat)
                    .unwrap_or(song.rows.len());
                song.rows.insert(
                    position,
                    ProjectSongRow {
                        id: row_id,
                        start_beat,
                        scene,
                        overrides,
                    },
                );
                let outcome = SongRowInsertOutcome {
                    row_id,
                    created_with_end_beat: None,
                };
                (song, outcome)
            }
        };

        self.commit_song_edit("Insert song row", before, Some(after))?;
        Ok(outcome)
    }

    /// Remove a row; the previous row's span extends over it. Removing the
    /// last remaining row clears the song (spec 5.6).
    pub fn song_row_remove(&mut self, row_id: SongRowId) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let before = self.state.committed_song();
        let Some(existing) = &before else {
            return Err("The project has no song".to_string());
        };
        let Some(index) = existing.rows.iter().position(|row| row.id == row_id) else {
            return Err(format!("Song row id {} does not exist", row_id.0));
        };
        if existing.rows.len() == 1 {
            // Removing the last remaining row is song-clear.
            return self.commit_song_edit("Remove song row", before, None);
        }
        if index == 0 {
            return Err(
                "Removing the first song row is rejected: the song must start at beat 0.0. \
                 Move another row to beat 0.0 first, or clear the song"
                    .to_string(),
            );
        }
        let mut song = existing.clone();
        song.rows.remove(index);
        self.commit_song_edit("Remove song row", before, Some(song))
    }

    /// Move a row to a new start beat; ordering is re-derived from
    /// `start_beat`. Moving row zero away from 0.0 or colliding exactly with
    /// another row's start is rejected, not auto-resolved (spec 5.6).
    pub fn song_row_move(&mut self, row_id: SongRowId, new_start_beat: f64) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let new_start_beat = finite_beat("Song row start beat", new_start_beat)?;
        let before = self.state.committed_song();
        let Some(existing) = &before else {
            return Err("The project has no song".to_string());
        };
        let Some(index) = existing.rows.iter().position(|row| row.id == row_id) else {
            return Err(format!("Song row id {} does not exist", row_id.0));
        };
        if existing.rows[index].start_beat == new_start_beat {
            // Exact no-op: nothing changes, no history entry.
            return Ok(());
        }
        if index == 0 {
            return Err(
                "Moving row zero away from beat 0.0 is rejected: the song must start at beat 0.0"
                    .to_string(),
            );
        }
        if new_start_beat >= existing.end_beat {
            return Err(format!(
                "Cannot move the song row to beat {new_start_beat}: the song ends at beat {}; \
                 extend it with song-set-end first",
                existing.end_beat
            ));
        }
        if let Some(collision) = existing
            .rows
            .iter()
            .find(|row| row.id != row_id && row.start_beat == new_start_beat)
        {
            return Err(format!(
                "Cannot move the song row to beat {new_start_beat}: row id {} already starts \
                 there",
                collision.id.0
            ));
        }
        let mut song = existing.clone();
        song.rows[index].start_beat = new_start_beat;
        song.rows.sort_by(|a, b| {
            a.start_beat
                .partial_cmp(&b.start_beat)
                .expect("song row start beats are finite")
        });
        self.commit_song_edit("Move song row", before, Some(song))
    }

    /// Replace a row's complete launch state (base scene plus the full
    /// override set), preserving its id and position.
    pub fn song_row_set_state(
        &mut self,
        row_id: SongRowId,
        scene: usize,
        overrides: Vec<ProjectSongTrackOverride>,
    ) -> Result<(), String> {
        self.require_song_edit_unlocked()?;
        let overrides = canonical_overrides(overrides)?;
        let before = self.state.committed_song();
        let Some(existing) = &before else {
            return Err("The project has no song".to_string());
        };
        let Some(index) = existing.rows.iter().position(|row| row.id == row_id) else {
            return Err(format!("Song row id {} does not exist", row_id.0));
        };
        if existing.rows[index].scene == scene && existing.rows[index].overrides == overrides {
            // Exact no-op: nothing changes, no history entry.
            return Ok(());
        }
        let mut song = existing.clone();
        song.rows[index].scene = scene;
        song.rows[index].overrides = overrides;
        self.commit_song_edit("Set song row state", before, Some(song))
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
        ProjectSongTrackOverride {
            track,
            pattern_id: Some(pattern_id),
        }
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
    fn insert_into_empty_song_creates_song_with_documented_default_end() {
        let mut app = test_app();
        assert!(app.state.committed_song().is_none());
        let depth = app.history.undo_len();
        let outcome = app
            .song_row_insert(0.0, 1, vec![ov(0, 3)])
            .expect("first insert at 0.0 succeeds");
        assert_eq!(outcome.row_id, SongRowId(0));
        assert_eq!(
            outcome.created_with_end_beat,
            Some(EMPTY_SONG_DEFAULT_LENGTH_BEATS)
        );
        let song = committed(&app);
        assert_eq!(song.rows.len(), 1);
        assert_eq!(song.rows[0].scene, 1);
        assert_eq!(song.rows[0].overrides, vec![ov(0, 3)]);
        assert_eq!(song.end_beat, EMPTY_SONG_DEFAULT_LENGTH_BEATS);
        assert_eq!(song.next_row_id, 1);
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
    }

    #[test]
    fn insert_into_empty_song_off_zero_is_rejected() {
        let mut app = test_app();
        let depth = app.history.undo_len();
        let error = app
            .song_row_insert(8.0, 0, Vec::new())
            .expect_err("empty-song insert off 0.0 must fail");
        assert!(error.contains("beat 0.0"), "{error}");
        assert!(app.state.committed_song().is_none());
        assert_eq!(app.history.undo_len(), depth);
    }

    #[test]
    fn insert_splits_governing_span() {
        let mut app = app_with_song();
        let depth = app.history.undo_len();
        let outcome = app
            .song_row_insert(6.0, 0, vec![ov(0, 2)])
            .expect("split insert succeeds");
        assert_eq!(outcome.created_with_end_beat, None);
        let song = committed(&app);
        let starts: Vec<f64> = song.rows.iter().map(|row| row.start_beat).collect();
        assert_eq!(starts, vec![0.0, 4.0, 6.0, 8.0]);
        assert_eq!(song.rows[2].id, outcome.row_id);
        assert_eq!(song.next_row_id, 4);
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn insert_identical_to_governing_state_is_rejected() {
        let mut app = app_with_song();
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_insert(6.0, 1, Vec::new()).map(|_| ()),
            "identical to the governing row",
        );
    }

    #[test]
    fn insert_collision_and_past_end_are_rejected() {
        let mut app = app_with_song();
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_insert(4.0, 2, Vec::new()).map(|_| ()),
            "already starts at beat 4",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_insert(16.0, 0, Vec::new()).map(|_| ()),
            "song-set-end",
        );
    }

    #[test]
    fn remove_extends_previous_span_and_keeps_ids() {
        let mut app = app_with_song();
        let song = committed(&app);
        let middle = song.rows[1].id;
        let depth = app.history.undo_len();
        app.song_row_remove(middle).expect("remove succeeds");
        let song = committed(&app);
        let ids: Vec<u64> = song.rows.iter().map(|row| row.id.0).collect();
        assert_eq!(ids, vec![0, 2]);
        assert_eq!(song.rows[1].start_beat, 8.0);
        assert_eq!(song.next_row_id, 3, "removal never rewinds the allocator");
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn remove_first_of_many_and_unknown_id_are_rejected() {
        let mut app = app_with_song();
        let first = committed(&app).rows[0].id;
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_remove(first),
            "must start at beat 0.0",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_remove(SongRowId(99)),
            "does not exist",
        );
    }

    #[test]
    fn remove_last_remaining_row_clears_song() {
        let mut app = test_app();
        let outcome = app.song_row_insert(0.0, 0, Vec::new()).unwrap();
        let depth = app.history.undo_len();
        app.song_row_remove(outcome.row_id).expect("remove succeeds");
        assert!(app.state.committed_song().is_none());
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn move_reorders_rows_keeping_ids_attached_to_states() {
        let mut app = app_with_song();
        let moved = committed(&app).rows[1].id; // scene 1 @ 4.0
        let depth = app.history.undo_len();
        app.song_row_move(moved, 12.0).expect("move succeeds");
        let song = committed(&app);
        let rows: Vec<(u64, f64, usize)> = song
            .rows
            .iter()
            .map(|row| (row.id.0, row.start_beat, row.scene))
            .collect();
        // Row id 1 (scene 1) now sorts after row id 2 (scene 2).
        assert_eq!(rows, vec![(0, 0.0, 0), (2, 8.0, 2), (1, 12.0, 1)]);
        assert_eq!(song.next_row_id, 3, "move preserves ids");
        assert_eq!(app.history.undo_len(), depth + 1);
    }

    #[test]
    fn move_rejections_and_noop() {
        let mut app = app_with_song();
        let song = committed(&app);
        let (first, middle) = (song.rows[0].id, song.rows[1].id);
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_move(first, 2.0),
            "row zero",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_move(middle, 8.0),
            "already starts",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_move(middle, 16.0),
            "song-set-end",
        );
        // Exact no-op: success without a history entry.
        let depth = app.history.undo_len();
        app.song_row_move(middle, 4.0).expect("no-op move succeeds");
        assert_eq!(app.history.undo_len(), depth);
    }

    #[test]
    fn set_state_preserves_id_and_rejects_normalization_deletions() {
        let mut app = app_with_song();
        let middle = committed(&app).rows[1].id;
        let depth = app.history.undo_len();
        app.song_row_set_state(middle, 0, vec![ov(0, 3)])
            .expect("set-state succeeds");
        let song = committed(&app);
        assert_eq!(song.rows[1].id, middle);
        assert_eq!(song.rows[1].scene, 0);
        assert_eq!(song.rows[1].overrides, vec![ov(0, 3)]);
        assert_eq!(song.next_row_id, 3);
        assert_eq!(app.history.undo_len(), depth + 1);

        // Making the row identical to its predecessor would normalize the
        // edited row away: rejected instead (spec 5.6).
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_set_state(middle, 0, Vec::new()),
            "identical launch states",
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_set_state(SongRowId(99), 0, Vec::new()),
            "does not exist",
        );
        // Nonexistent scene fails 5.3 validation.
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_set_state(middle, 7, Vec::new()),
            "scene 8",
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
        app.song_row_insert(6.0, 0, vec![ov(0, 1)]).unwrap();
        let after = app.state.committed_song();
        assert_ne!(before, after);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), before);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), after);

        // Replace bumps the allocator; undo must rewind it exactly.
        app.song_replace(vec![spec(0.0, 2, Vec::new())], 8.0, false)
            .unwrap();
        assert_eq!(committed(&app).next_row_id, 5);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.committed_song(), after);
        assert_eq!(committed(&app).next_row_id, 4);

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
        let row = committed(&app).rows[1].id;
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_insert(2.0, 2, Vec::new()).map(|_| ()),
            SONG_EDITS_LOCKED_ERROR,
        );
        assert_rejected_unchanged(&mut app, |app| app.song_row_remove(row), SONG_EDITS_LOCKED_ERROR);
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_move(row, 6.0),
            SONG_EDITS_LOCKED_ERROR,
        );
        assert_rejected_unchanged(
            &mut app,
            |app| app.song_row_set_state(row, 0, Vec::new()),
            SONG_EDITS_LOCKED_ERROR,
        );
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

    #[test]
    fn duplicate_override_tracks_are_rejected() {
        let mut app = app_with_song();
        assert_rejected_unchanged(
            &mut app,
            |app| {
                app.song_row_set_state(
                    committed(app).rows[1].id,
                    0,
                    vec![ov(0, 1), ov(0, 2)],
                )
            },
            "More than one override",
        );
    }
}
