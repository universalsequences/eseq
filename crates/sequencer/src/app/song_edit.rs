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

use super::{song_transport::SongTransportMode, App};

/// Error returned by every primitive while arrangement capture owns the
/// pending splice it will commit at Stop.
pub const SONG_EDITS_LOCKED_ERROR: &str =
    "song editing is unavailable during arrangement capture";

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
    /// Playback can safely rebuild the scheduler's immutable runtime song.
    /// Capture remains locked because it owns a growing `[P, Q)` splice and
    /// must commit that splice atomically at Stop.
    pub fn song_edits_locked(&self) -> bool {
        self.song_transport_mode == SongTransportMode::ArrangementCapture
    }

    pub(super) fn require_song_edit_unlocked(&self) -> Result<(), String> {
        if self.song_edits_locked() {
            return Err(SONG_EDITS_LOCKED_ERROR.to_string());
        }
        Ok(())
    }

    /// The real beat↔step geometry of `pattern_id` in `track`'s pool — the
    /// `steps()` mapping used for offset stamping (takes spec 7.1/7.2/7.4).
    /// Per-step timebase and sync plocks participate: stamping must invert
    /// the same boundaries the runtime resolves the stamped offset through,
    /// or anchored playback of an unquantized capture drifts against the
    /// transport (and sync plocks snap to the wrong grid).
    fn pattern_geometry(
        &self,
        track: usize,
        pattern_id: u64,
    ) -> Option<crate::sequencer::PatternStepGeometry> {
        self.state.with_project_scenes(|scenes| {
            let data = scenes.track_pools.get(track)?.get(PatternId(pattern_id))?;
            Some(data.step_geometry())
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
    /// pattern's real step geometry, normalized into `[0, num_steps)`.
    /// Offsets within stamping epsilon of a pattern boundary collapse to 0
    /// so scene-resolved lanes stay implicit whenever they can.
    pub(crate) fn advanced_offset(
        &self,
        track: usize,
        pattern_id: u64,
        offset_steps: f64,
        delta_beats: f64,
    ) -> f64 {
        let Some(geometry) = self.pattern_geometry(track, pattern_id) else {
            return offset_steps;
        };
        geometry.advanced_offset(offset_steps, delta_beats)
    }

}
