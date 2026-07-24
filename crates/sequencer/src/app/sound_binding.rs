//! Sound binding — device-parameter ownership per track
//! (docs/takes-and-additive-arrangement-recording-spec.md 16).
//!
//! Every pool pattern owns a full device snapshot, and a take's chunks are
//! frozen copies of one. Without a binding the device UI would keep reading
//! and writing the scene-effective pattern while song playback sounds a
//! take's frozen snapshot — the panel lies and edits are inaudible.
//!
//! The invariant (16.2): per track there is exactly ONE bound source, and
//! the panel display, the live monitor sound, and the take punch-in clone
//! template all read from it. This module owns the resolution (16.3) and the
//! selection lifecycle (16.6); routing edits through it lives in `edit.rs`
//! and the panel read surfaces.

use crate::sequencer::{LaneSource, PatternId, SongRowId, TakeId};

use super::App;

/// The arrangement's fixed bar grid, matching the timeline ruler.
const BEATS_PER_BAR: f64 = 4.0;

/// The device-parameter source a track is bound to. `Empty`/bare tracks
/// resolve to `None` rather than a variant — there is nothing to display or
/// edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundSource {
    /// A take: the snapshot lives on every one of its chunks, which must
    /// never diverge (16.4) — writes fan out to all of them.
    Take(TakeId),
    /// An ordinary pool pattern (a track clip, or the effective scene
    /// pattern under rule 3).
    Pattern(PatternId),
}

impl BoundSource {
    pub fn take(self) -> Option<TakeId> {
        match self {
            BoundSource::Take(id) => Some(id),
            BoundSource::Pattern(_) => None,
        }
    }

    pub fn pattern(self) -> Option<PatternId> {
        match self {
            BoundSource::Pattern(id) => Some(id),
            BoundSource::Take(_) => None,
        }
    }

    /// `LaneSource::Empty` carries no device state: a bare lane is not a
    /// binding, it falls through to the next rule.
    fn from_lane(source: LaneSource) -> Option<Self> {
        match source {
            LaneSource::Take(id) => Some(BoundSource::Take(id)),
            LaneSource::Pattern(id) => Some(BoundSource::Pattern(id)),
            LaneSource::Empty => None,
        }
    }
}

/// Which rule of 16.3 produced the binding. Drives the panel header (16.6)
/// and tells the edit path whether it is on the legacy scene-pattern route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingOrigin {
    /// Rule 1: an explicit timeline clip selection. Always wins.
    Selection,
    /// Rule 2: the track's audible resolved source under song playback.
    Playback,
    /// Rule 3: the effective scene pattern — today's behavior, and the only
    /// rule outside song mode.
    Scene,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackBinding {
    pub source: Option<BoundSource>,
    pub origin: BindingOrigin,
}

impl TrackBinding {
    /// True while the binding is the plain scene pattern, i.e. every legacy
    /// device path is already correct and needs no rerouting.
    pub fn is_scene(self) -> bool {
        self.origin == BindingOrigin::Scene
    }
}

/// Persistent arrangement clip selection (16.6). Timeline state like the
/// playhead or zoom: it survives view switches and transport, and changes
/// only through the four causes in 16.6. The resolved `source` is stamped at
/// selection time — selection is intent about a take/clip, not about
/// whatever a later row edit happens to resolve at that row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongClipSelection {
    pub track: usize,
    /// First row id of the merged clip — the timeline's gesture identity,
    /// used to render the bound-clip highlight.
    pub row_id: SongRowId,
    pub source: BoundSource,
}

/// The 16.3 order, first match wins. Pure so the ordering is testable
/// without an `App`: callers supply each rule's already-validated candidate.
///
/// - `selection`: rule 1, `Some` only when a live selection targets this
///   track and its source still exists.
/// - `audible`: rule 2, `Some` only while song playback is authoritative for
///   this track (a manually latched lane is not the song's).
/// - `effective`: rule 3, the effective scene pattern.
pub(crate) fn resolve_binding(
    selection: Option<BoundSource>,
    audible: Option<LaneSource>,
    effective: Option<PatternId>,
) -> TrackBinding {
    if let Some(source) = selection {
        return TrackBinding {
            source: Some(source),
            origin: BindingOrigin::Selection,
        };
    }
    // An `Empty` audible lane has no device state of its own; the panel
    // keeps showing the track's session sound rather than going blank.
    if let Some(source) = audible.and_then(BoundSource::from_lane) {
        return TrackBinding {
            source: Some(source),
            origin: BindingOrigin::Playback,
        };
    }
    TrackBinding {
        source: effective.map(BoundSource::Pattern),
        origin: BindingOrigin::Scene,
    }
}

impl App {
    /// Rule 1 candidate: the selection's source, dropped when it no longer
    /// exists (16.6 cause 4 — deleting the selected clip falls back without
    /// any explicit unbinding).
    fn selected_bound_source(&self, track: usize) -> Option<BoundSource> {
        // Dormant while the timeline is off screen (16.6): in the Seq tab
        // nothing renders the bound clip, so a selection silently owning the
        // device panel reads as the panel showing the wrong sound.
        if !self.arrangement_view_visible {
            return None;
        }
        let selection = self.song_clip_selection?;
        if selection.track != track {
            return None;
        }
        let alive = self.state.with_project_scenes(|scenes| match selection.source {
            BoundSource::Take(id) => scenes
                .take_pools
                .get(track)
                .is_some_and(|takes| takes.contains(id)),
            BoundSource::Pattern(id) => scenes
                .track_pools
                .get(track)
                .is_some_and(|pool| pool.contains(id)),
        });
        alive.then_some(selection.source)
    }

    /// Rule 2 candidate: what the track is actually sounding under song
    /// playback. A manually latched lane is the performer's, not the song's
    /// (takes spec 10), so it falls through to rule 3 like session mode.
    fn audible_lane_source(&self, track: usize) -> Option<LaneSource> {
        if !self.song_playback_authority_active() {
            return None;
        }
        if track < 64 && self.state.song_manual_latch_mask() >> track & 1 == 1 {
            return None;
        }
        let song = self.active_runtime_song.as_ref()?;
        let row = song.rows.get(self.song_mirrored_row?)?;
        row.resolved_sources.get(track).copied()
    }

    /// The track's bound source (16.3). Cheap enough for per-frame reads:
    /// one scenes lock, no pattern clones.
    pub fn track_sound_binding(&self, track: usize) -> TrackBinding {
        resolve_binding(
            self.selected_bound_source(track),
            self.audible_lane_source(track),
            self.state.effective_track_pattern_id(track),
        )
    }

    /// Every pool pattern carrying the bound source's device snapshot: one
    /// id for a pattern, every chunk for a take (16.4/16.8).
    pub fn bound_source_patterns(&self, source: BoundSource, track: usize) -> Vec<PatternId> {
        match source {
            BoundSource::Pattern(id) => vec![id],
            BoundSource::Take(id) => self.state.with_project_scenes(|scenes| {
                scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(id))
                    .map(|take| take.chunks.clone())
                    .unwrap_or_default()
            }),
        }
    }

    /// The pool pattern the panel reads and a single-target edit writes: the
    /// take's FIRST chunk stands for the take (chunks never diverge, 16.4).
    pub fn bound_read_pattern(&self, track: usize) -> Option<PatternId> {
        match self.track_sound_binding(track).source? {
            BoundSource::Pattern(id) => Some(id),
            BoundSource::Take(id) => self.state.with_project_scenes(|scenes| {
                scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(id))
                    .and_then(|take| take.chunks.first().copied())
            }),
        }
    }

    /// Human-readable binding label for the panel header (16.6):
    /// `Take 2 · bars 0–2` vs `Pattern 2 (scene)`.
    pub fn track_binding_label(&self, track: usize) -> Option<String> {
        let binding = self.track_sound_binding(track);
        match binding.source? {
            BoundSource::Take(id) => {
                let name = self.state.track_take(track, id)?.name;
                Some(match self.take_bar_span(track, id) {
                    Some((start, end)) => format!("{name} · bars {start}–{end}"),
                    None => name,
                })
            }
            BoundSource::Pattern(id) => Some(match binding.origin {
                BindingOrigin::Scene => format!("Pattern {} (scene)", id.0),
                _ => format!("Pattern {}", id.0),
            }),
        }
    }

    /// Inclusive one-based bar range a take occupies in the committed song,
    /// from the rows that reference it. `None` when the take is not placed.
    fn take_bar_span(&self, track: usize, take: TakeId) -> Option<(u64, u64)> {
        let song = self.state.committed_song()?;
        let mut first = f64::INFINITY;
        let mut last = f64::NEG_INFINITY;
        for (index, row) in song.rows.iter().enumerate() {
            let references = row.overrides.iter().any(|over| {
                over.track == track && over.take_id == Some(take.0)
            });
            if !references {
                continue;
            }
            let end = song
                .rows
                .get(index + 1)
                .map(|next| next.start_beat)
                .unwrap_or(song.end_beat);
            first = first.min(row.start_beat);
            last = last.max(end);
        }
        if !first.is_finite() || last <= first {
            return None;
        }
        let bar = |beat: f64| (beat / BEATS_PER_BAR).floor().max(0.0) as u64 + 1;
        // The end beat is exclusive: a take ending exactly on a bar line
        // occupies the bar before it.
        Some((bar(first), bar((last - 1e-9).max(first))))
    }

    /// Follow the arrangement view on/off screen (16.6 dormancy). Called
    /// from the reactive tick before the bindings resolve; re-resolves only
    /// on a real transition, where the panel and monitor must both move.
    pub fn set_arrangement_view_visible(&mut self, visible: bool) {
        if self.arrangement_view_visible == visible {
            return;
        }
        self.arrangement_view_visible = visible;
        self.sync_track_sound_bindings();
    }

    /// Set/clear the timeline clip selection (16.6). Returns true when the
    /// selection actually changed, so callers can resync the binding only on
    /// a real transition.
    pub fn set_song_clip_selection(&mut self, selection: Option<SongClipSelection>) -> bool {
        if self.song_clip_selection == selection {
            return false;
        }
        self.song_clip_selection = selection;
        self.sync_track_sound_bindings();
        true
    }

    /// Make the live device mirror match every track's bound source (16.2),
    /// pushing the newly bound sound to the engine so the monitor leg
    /// changes with the panel. Cheap when nothing moved: one resolve per
    /// track and no writes.
    ///
    /// Called from the reactive tick, so song row transitions (rule 2) and
    /// selection changes (rule 1) both land here, as does the implicit
    /// release a session save-back performs (`release_bound_device_state`).
    pub fn sync_track_sound_bindings(&mut self) {
        if self.loaded_sound_binding.len() != self.tracks.len() {
            self.loaded_sound_binding.resize(self.tracks.len(), None);
        }
        if self.sound_binding_monitored.len() != self.tracks.len() {
            // A track with nothing borrowed is monitoring by construction:
            // the engine already reflects the mirror.
            self.sound_binding_monitored.resize(self.tracks.len(), true);
        }
        // While the song is sounding, a binding that is NOT what the playhead
        // is currently playing is display + edit only (16.7): the performer
        // must keep hearing the arrangement while tuning a past or future
        // clip. The tweaks become audible when the playhead reaches it.
        let song_sounding = self.song_playback_authority_active() && self.state.is_playing();
        let borrowed = self.state.sound_binding_borrowed_mask();
        for track in 0..self.tracks.len() {
            // A lane the state released underneath us (a launch or row
            // transition saved the session) is no longer loaded, whatever we
            // last put there.
            if track >= 64 || borrowed >> track & 1 == 0 {
                self.loaded_sound_binding[track] = None;
            }
            let binding = self.track_sound_binding(track);
            let desired = match binding.origin {
                // Rule 3 is the mirror's natural content: nothing to borrow.
                BindingOrigin::Scene => None,
                _ => binding.source,
            };
            let audible = self.audible_lane_source(track).and_then(BoundSource::from_lane);
            let monitors = !song_sounding || desired.is_none() || desired == audible;
            if desired != self.loaded_sound_binding[track] {
                match desired {
                    Some(source) => {
                        let Some(pattern) = self.source_read_pattern(track, source) else {
                            continue;
                        };
                        let data = self.state.with_project_scenes(|scenes| {
                            scenes
                                .track_pools
                                .get(track)
                                .and_then(|pool| pool.get(pattern))
                                .cloned()
                        });
                        let Some(data) = data else { continue };
                        if !self.state.borrow_track_device_state(track, pattern, &data) {
                            continue;
                        }
                    }
                    None => self.state.release_bound_track_device_state(track),
                }
                self.loaded_sound_binding[track] = desired;
                self.sound_binding_epoch += 1;
                // The engine now lags the mirror; whether it catches up is
                // the monitor decision below.
                self.sound_binding_monitored[track] = false;
            }
            if !monitors {
                // Silent binding: leave the engine on the audible row's
                // sound, and remember that it no longer matches the mirror.
                self.sound_binding_monitored[track] = false;
                continue;
            }
            if !self.sound_binding_monitored[track] {
                // Either the binding moved onto what is sounding, or the
                // selection was released — push the mirror out for real.
                self.sound_binding_monitored[track] = true;
                self.push_track_sound_to_engine(track);
            }
        }
    }

    /// True while the mirror holds a bound source the engine must NOT hear
    /// (16.7): a clip selected in the timeline that the playhead is not
    /// currently playing. Every "push the mirror's value to the engine" path
    /// checks this, so tuning a past or future clip is display + edit only
    /// and the arrangement keeps sounding what it was sounding.
    ///
    /// Derived from live state rather than the `sound_binding_monitored`
    /// bookkeeping on purpose: a song row transition RELEASES the borrow and
    /// pushes the row's sound through these same senders, and that push must
    /// never be swallowed — with nothing borrowed, the mirror is the track's
    /// own sound and is always audible.
    pub(crate) fn sound_binding_is_silent(&self, track: usize) -> bool {
        if track >= 64 || self.state.sound_binding_borrowed_mask() >> track & 1 == 0 {
            return false;
        }
        // Stopped or in session mode the bound source IS the monitor (16.2):
        // hearing what you are editing is the whole point.
        if !(self.song_playback_authority_active() && self.state.is_playing()) {
            return false;
        }
        let loaded = self.loaded_sound_binding.get(track).copied().flatten();
        let audible = self.audible_lane_source(track).and_then(BoundSource::from_lane);
        loaded != audible
    }

    /// The pool pattern holding `source`'s device snapshot: a take is
    /// represented by its first chunk (chunks never diverge, 16.4).
    fn source_read_pattern(&self, track: usize, source: BoundSource) -> Option<PatternId> {
        match source {
            BoundSource::Pattern(id) => Some(id),
            BoundSource::Take(id) => self.state.with_project_scenes(|scenes| {
                scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(id))
                    .and_then(|take| take.chunks.first().copied())
            }),
        }
    }

    /// Edit-through (16.7): a device edit that landed on a pool pattern the
    /// playing song resolves to must reach the prebuilt row snapshots, which
    /// cloned that pattern at preflight. Re-preflight and swap the rows in
    /// place; the audible row already got the value pushed straight to the
    /// engine, so this is about the rows still ahead of the playhead.
    ///
    /// A no-op outside song playback, and when no row uses the pattern.
    pub fn invalidate_song_rows_for_pattern(&mut self, track: usize, pattern: PatternId) {
        if !self.song_playback_authority_active() {
            return;
        }
        let affected = self.active_runtime_song.as_ref().is_some_and(|song| {
            song.rows.iter().any(|row| {
                row.resolved_pattern_ids.get(track).copied().flatten() == Some(pattern)
            })
        });
        if !affected {
            return;
        }
        let Ok(song) = self.state.preflight_runtime_song() else {
            return;
        };
        if self.state.refresh_song_playback(std::sync::Arc::clone(&song)).is_ok() {
            self.active_runtime_song = Some(song);
        }
    }

    /// The other chunks of the take that claims `pattern` on `track`, or an
    /// empty list when `pattern` is an ordinary pool pattern. A device write
    /// to one chunk must be mirrored onto these (16.4) — chunks never
    /// diverge in device state.
    pub(crate) fn take_sibling_chunks(&self, track: usize, pattern: PatternId) -> Vec<PatternId> {
        self.state.with_project_scenes(|scenes| {
            scenes
                .take_pools
                .get(track)
                .and_then(|takes| {
                    takes
                        .takes
                        .iter()
                        .find(|take| take.chunks.contains(&pattern))
                })
                .map(|take| {
                    take.chunks
                        .iter()
                        .copied()
                        .filter(|chunk| *chunk != pattern)
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// **Push to pattern** (16.5): promote the bound source's sound to the
    /// track's working sound by copying it into the effective pattern of the
    /// CURRENT SCENE only. Other scenes' patterns are what per-scene sound
    /// design protects, so the blast radius stops here — the track-wide
    /// broadcast is the deferred "Apply sound to entire track" gesture, not
    /// a variant of this one.
    pub fn push_bound_sound_to_pattern(&mut self, track: usize) -> Result<String, String> {
        let binding = self.track_sound_binding(track);
        if binding.is_scene() {
            return Err("The track is already bound to its scene pattern".to_string());
        }
        let source = self
            .bound_read_pattern(track)
            .ok_or_else(|| "The bound source has no device snapshot".to_string())?;
        let target = self
            .state
            .effective_track_pattern_id(track)
            .ok_or_else(|| "The current scene has no pattern on this track".to_string())?;
        self.commit_sound_propagation(track, source, &[target], "Push sound to pattern")
    }

    /// **Apply to all takes on track** (16.5): the bound source's sound onto
    /// every take on the track, every chunk.
    pub fn apply_bound_sound_to_all_takes(&mut self, track: usize) -> Result<String, String> {
        let source = self
            .bound_read_pattern(track)
            .ok_or_else(|| "The track has no bound sound".to_string())?;
        let targets: Vec<PatternId> = self.state.with_project_scenes(|scenes| {
            scenes
                .take_pools
                .get(track)
                .map(|takes| takes.claimed().collect())
                .unwrap_or_default()
        });
        if targets.is_empty() {
            return Err(format!("Track {} has no takes", track + 1));
        }
        self.commit_sound_propagation(track, source, &targets, "Apply sound to all takes")
    }

    /// One propagation gesture = one undo entry (16.5).
    fn commit_sound_propagation(
        &mut self,
        track: usize,
        source: PatternId,
        targets: &[PatternId],
        label: &'static str,
    ) -> Result<String, String> {
        let before = self.capture_synchronized_scene_structure_state()?;
        let copied = self
            .state
            .copy_track_pattern_device_state(track, source, targets)?;
        if copied == 0 {
            return Ok(format!("{label}: nothing to copy"));
        }
        let after = self.state.capture_project_scenes();
        crate::app::edit::finish_active_gesture(self);
        let patch = crate::app::history::SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            label,
            None,
            crate::app::history::EditPatch::SceneStructure(patch),
            retained_bytes,
        );
        self.invalidate_song_rows_for_pattern(track, source);
        for target in targets {
            self.invalidate_song_rows_for_pattern(track, *target);
        }
        Ok(format!("{label}: {copied} pattern(s) updated"))
    }

    /// Select the clip a timeline gesture picked (16.6 causes 1–2). The
    /// timeline identifies a clip by the first row id of its merged span, so
    /// the source is resolved here — override first, else the row's scene
    /// cell — and stamped into the selection: selection is intent about
    /// THIS take or clip, not about whatever a later row edit resolves.
    pub fn select_song_clip(&mut self, track: usize, row_id: SongRowId) -> Result<(), String> {
        let song = self
            .state
            .committed_song()
            .ok_or_else(|| "The project has no committed song".to_string())?;
        let row = song
            .rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or_else(|| format!("Song has no row with id {}", row_id.0))?;
        let source = match row
            .overrides
            .iter()
            .find(|over| over.track == track)
            .map(|over| over.source())
        {
            Some(source) => source,
            None => self
                .state
                .scene_track_pattern_id(row.scene, track)
                .map(LaneSource::Pattern)
                .unwrap_or(LaneSource::Empty),
        };
        // An empty lane is not a clip; a gesture on one is a deselect.
        let selection = BoundSource::from_lane(source).map(|source| SongClipSelection {
            track,
            row_id,
            source,
        });
        self.set_song_clip_selection(selection);
        Ok(())
    }

    /// Auto-select a freshly committed take (16.3/16.6 cause 3) so
    /// post-record tweaks bind to what the performer just played.
    pub(crate) fn select_committed_take(&mut self, track: usize, take: TakeId) {
        let row_id = self.state.committed_song().and_then(|song| {
            song.rows
                .iter()
                .find(|row| {
                    row.overrides
                        .iter()
                        .any(|over| over.track == track && over.take_id == Some(take.0))
                })
                .map(|row| row.id)
        });
        let Some(row_id) = row_id else { return };
        self.set_song_clip_selection(Some(SongClipSelection {
            track,
            row_id,
            source: BoundSource::Take(take),
        }));
    }

    /// Push one track's whole live device state to the audio graph — the
    /// per-track half of `push_all_restored_defaults`. This is the monitor
    /// leg of 16.2: the sound changes when the binding does.
    pub(crate) fn push_track_sound_to_engine(&mut self, track: usize) {
        if track >= self.tracks.len() {
            return;
        }
        self.push_track_volume(track);
        self.push_track_pan(track);
        self.push_track_mute(track);
        self.push_send_gain(track);
        for slot_idx in 0..self.state.pattern.effect_chains[track].len() {
            let num_params = self.state.pattern.effect_chains[track][slot_idx]
                .num_params
                .load(std::sync::atomic::Ordering::Relaxed) as usize;
            for param_idx in 0..num_params {
                self.send_effective_slot_param(track, slot_idx, param_idx);
            }
        }
        self.push_track_solo_mutes();
        self.push_instrument_defaults_for_track(track);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use crate::app::{command::AppCommand, edit::try_apply_command, AudioBuses};
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternSnapshot, ProjectSong, ProjectSongRow,
        ProjectSongTrackOverride, SequencerState, TrackPatternData,
    };

    const TAKE: BoundSource = BoundSource::Take(TakeId(3));
    const CLIP: BoundSource = BoundSource::Pattern(PatternId(7));
    const SCENE: PatternId = PatternId(2);

    #[test]
    fn selection_wins_over_playback_and_scene() {
        let binding = resolve_binding(
            Some(TAKE),
            Some(LaneSource::Pattern(PatternId(9))),
            Some(SCENE),
        );
        assert_eq!(binding.source, Some(TAKE));
        assert_eq!(binding.origin, BindingOrigin::Selection);

        // Rule 1 wins with no playback at all (paused with a selection).
        let binding = resolve_binding(Some(CLIP), None, Some(SCENE));
        assert_eq!(binding.source, Some(CLIP));
        assert_eq!(binding.origin, BindingOrigin::Selection);
    }

    #[test]
    fn playback_binds_when_nothing_is_selected() {
        let binding = resolve_binding(None, Some(LaneSource::Take(TakeId(3))), Some(SCENE));
        assert_eq!(binding.source, Some(TAKE));
        assert_eq!(binding.origin, BindingOrigin::Playback);

        let binding = resolve_binding(None, Some(LaneSource::Pattern(PatternId(7))), Some(SCENE));
        assert_eq!(binding.source, Some(CLIP));
        assert_eq!(binding.origin, BindingOrigin::Playback);
    }

    #[test]
    fn empty_audible_lane_falls_through_to_the_scene_pattern() {
        let binding = resolve_binding(None, Some(LaneSource::Empty), Some(SCENE));
        assert_eq!(binding.source, Some(BoundSource::Pattern(SCENE)));
        assert_eq!(binding.origin, BindingOrigin::Scene);
    }

    #[test]
    fn session_mode_is_always_the_scene_pattern() {
        let binding = resolve_binding(None, None, Some(SCENE));
        assert_eq!(binding.source, Some(BoundSource::Pattern(SCENE)));
        assert!(binding.is_scene());

        // A bare track binds to nothing at all.
        let binding = resolve_binding(None, None, None);
        assert_eq!(binding.source, None);
        assert!(binding.is_scene());
    }

    /// One track, one scene, and a two-chunk take placed over the whole song.
    fn app_with_take() -> (App, TakeId, PatternId, Vec<PatternId>) {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(vec![PatternSnapshot::new_default(1, &[])], 0);
        state.restore_current_pattern_from_repository().unwrap();
        // A device edit needs a device: give the track a descriptor so the
        // instrument slot actually has parameters to route.
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[crate::sequencer::InstrumentType::Sampler],
        ));
        let scene_pattern = state
            .effective_track_pattern_id(0)
            .expect("scene 0 has a pattern on track 0");

        let chunk = || -> TrackPatternData {
            let mut data = state
                .with_project_scenes(|scenes| scenes.effective_track_pattern(0).cloned())
                .expect("effective pattern");
            data.clear_step_content();
            data
        };
        let take = state
            .register_track_take(0, None, vec![chunk(), chunk()], 300)
            .expect("take registers");
        let chunks = state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).unwrap().chunks.clone());

        state.set_committed_song(Some(ProjectSong {
            rows: vec![ProjectSongRow {
                id: crate::sequencer::SongRowId(0),
                start_beat: 0.0,
                scene: 0,
                overrides: vec![ProjectSongTrackOverride::new_take(0, take.0, 0.0)],
            }],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 1,
        }));

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
        app.graph.instrument_descriptors = vec![descriptor];
        // These cases are all "the user is looking at the timeline": rule 1
        // is dormant while the arrangement view is off screen (16.6).
        app.arrangement_view_visible = true;
        (app, take, scene_pattern, chunks)
    }

    fn instrument_default(app: &App, pattern: PatternId) -> f32 {
        app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(pattern)
                .expect("pattern in pool")
                .instrument_slot
                .defaults
                .first()
                .copied()
                .expect("instrument slot has a first parameter")
        })
    }

    /// 16.4: with a take bound, a device edit writes the take — every chunk
    /// of it — and never the scene pattern. No dual-write.
    #[test]
    fn take_bound_device_edit_fans_out_to_chunks_and_spares_the_scene_pattern() {
        let (mut app, take, scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );

        let scene_before = instrument_default(&app, scene_pattern);
        let target = scene_before - 0.25;
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: target,
            },
        )
        .expect("device edit applies");

        for chunk in &chunks {
            assert_eq!(
                instrument_default(&app, *chunk),
                target,
                "every chunk of the bound take carries the edit"
            );
        }
        assert_eq!(
            instrument_default(&app, scene_pattern),
            scene_before,
            "the scene pattern is never dual-written"
        );
    }

    /// 16.6 cause 1: deselecting returns the binding — and the edit target —
    /// to the effective scene pattern.
    #[test]
    fn deselecting_returns_edits_to_the_scene_pattern() {
        let (mut app, _take, scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");
        app.set_song_clip_selection(None);
        assert!(app.track_sound_binding(0).is_scene());

        let chunk_before = instrument_default(&app, chunks[0]);
        let target = chunk_before - 0.25;
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: target,
            },
        )
        .expect("device edit applies");

        assert_eq!(instrument_default(&app, scene_pattern), target);
        assert_eq!(
            instrument_default(&app, chunks[0]),
            chunk_before,
            "an unbound take keeps its frozen sound"
        );
    }

    /// 16.6 dormancy: leaving the arrangement view hands the panel — and the
    /// edit target — back to the scene pattern; returning re-binds the same
    /// selection. Without this the Seq tab keeps showing a take's devices
    /// with nothing on screen explaining why.
    #[test]
    fn selection_is_dormant_while_the_arrangement_view_is_hidden() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");

        app.set_arrangement_view_visible(false);
        let binding = app.track_sound_binding(0);
        assert!(binding.is_scene(), "the Seq tab binds the scene pattern");
        assert_eq!(binding.source, Some(BoundSource::Pattern(scene_pattern)));
        assert!(
            app.song_clip_selection.is_some(),
            "dormant, not cleared: the timeline selection survives the switch"
        );

        app.set_arrangement_view_visible(true);
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "returning to the timeline re-binds the selection"
        );
    }

    /// 16.7: while the song plays, selecting a clip the playhead is NOT on
    /// binds the panel and the edit target but must stay SILENT — you keep
    /// hearing the arrangement while you tune a past or future clip.
    /// Deselecting (or the playhead reaching it) hands the monitor back.
    #[test]
    fn a_non_audible_selection_is_display_and_edit_only_while_the_song_plays() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        // A second clip that is nowhere in the song: selecting it can never
        // be what the playhead is playing.
        let other = app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .clone();
            scenes.track_pools[0].insert(data)
        });
        app.set_use_arrangement(true).expect("song mode on");
        app.song_transport_play(false).expect("song playback starts");
        // Row zero plays the take; nothing selected -> the take is audible
        // AND monitored.
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );
        assert!(!app.sound_binding_is_silent(0), "what plays is what sounds");

        app.set_song_clip_selection(Some(SongClipSelection {
            track: 0,
            row_id: crate::sequencer::SongRowId(0),
            source: BoundSource::Pattern(other),
        }));
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Pattern(other)),
            "the panel follows the selection"
        );
        assert!(
            app.sound_binding_is_silent(0),
            "a clip the playhead is not on must not be heard"
        );

        // Deselect: the audible take owns the panel and the monitor again.
        app.set_song_clip_selection(None);
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );
        assert!(!app.sound_binding_is_silent(0));
        app.song_transport_stop().expect("stop succeeds");
    }

    /// Stopped, the bound source IS the monitor (16.2) — tweaking a selected
    /// clip while paused must be audible, otherwise sound design is deaf.
    #[test]
    fn a_selection_is_audible_while_the_transport_is_stopped() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );
        assert!(!app.sound_binding_is_silent(0));
    }

    /// Leaving song mode outright has no timeline left to explain a binding,
    /// so the selection is dropped rather than left dormant.
    #[test]
    fn turning_off_use_arrangement_clears_the_selection() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        app.set_use_arrangement(true).expect("song mode on");
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");
        app.set_use_arrangement(false).expect("back to session mode");
        assert_eq!(app.song_clip_selection, None);
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Pattern(scene_pattern))
        );
    }

    /// 16.5: Push to pattern promotes the bound take's sound to the current
    /// scene's pattern, as one undo entry.
    #[test]
    fn push_to_pattern_promotes_the_bound_takes_sound() {
        let (mut app, _take, scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");
        let target = instrument_default(&app, scene_pattern) - 0.25;
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: target,
            },
        )
        .expect("device edit applies");
        assert_ne!(instrument_default(&app, scene_pattern), target);

        // Close the knob gesture first: its own entry is not this gesture's.
        crate::app::edit::finish_active_gesture(&mut app);
        let depth = app.history.undo_len();
        app.push_bound_sound_to_pattern(0).expect("push applies");
        assert_eq!(instrument_default(&app, scene_pattern), target);
        assert_eq!(instrument_default(&app, chunks[0]), target);
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");
    }

    /// The common case: a plain pattern clip, not a take. Selecting one binds
    /// the panel to that pool pattern even though the scene still resolves a
    /// different one, and bumps the epoch that republishes the panels.
    #[test]
    fn selecting_a_pattern_clip_binds_that_pattern_not_the_scenes() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let other = app.state.with_scenes_mut(|scenes| {
            let mut data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern in pool")
                .clone();
            data.instrument_slot.defaults[0] = 0.125;
            scenes.track_pools[0].insert(data)
        });
        app.state.set_committed_song(Some(ProjectSong {
            rows: vec![ProjectSongRow {
                id: crate::sequencer::SongRowId(0),
                start_beat: 0.0,
                scene: 0,
                overrides: vec![ProjectSongTrackOverride::new(0, Some(other.0))],
            }],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 1,
        }));

        let epoch = app.sound_binding_epoch;
        app.select_song_clip(0, crate::sequencer::SongRowId(0))
            .expect("clip selects");

        let binding = app.track_sound_binding(0);
        assert_eq!(binding.source, Some(BoundSource::Pattern(other)));
        assert!(!binding.is_scene(), "selection wins over the scene cell");
        assert_eq!(
            app.track_binding_label(0).as_deref(),
            Some(format!("Pattern {}", other.0).as_str())
        );
        assert_ne!(app.sound_binding_epoch, epoch, "panels must republish");
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            0.125,
            "the live mirror carries the selected clip's devices"
        );
    }
}
