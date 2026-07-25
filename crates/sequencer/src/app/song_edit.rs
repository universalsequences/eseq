//! Song-mode shared vocabulary (docs/song-mode-spec.md section 5.6).
//!
//! The row *editing* primitives that used to live here are gone: authoring
//! happens on the arrangement (`app/arr_edit.rs`,
//! docs/arrangement-lane-model-spec.md 8) and rows are only the compiled
//! playback representation. What remains is what the arrangement path and the
//! transport still share — the edit lock, the declarative row description
//! `def-song` parses into, the `steps()` mappings offsets are stamped with,
//! and the undo-replay hook for the committed song.

use crate::sequencer::{PatternId, ProjectSongTrackOverride};

use super::App;

/// Error returned by every primitive while a Slice B transport mode locks
/// song editing (spec 5.6/13: single launch authority).
pub const SONG_EDITS_LOCKED_ERROR: &str =
    "song editing is unavailable during song playback/capture";

/// Caller-facing row description for the declarative arrangement replacement
/// (`def-song` → `App::arr_replace_rows`, which lowers rows to lanes).
#[derive(Clone, Debug, PartialEq)]
pub struct SongRowSpec {
    pub start_beat: f64,
    pub scene: usize,
    pub overrides: Vec<ProjectSongTrackOverride>,
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

}
