//! Sound binding — device-parameter ownership per track
//! (docs/takes-and-additive-arrangement-recording-spec.md 16).
//!
//! Every pool pattern and take references pooled Patch/Mix entities (takes
//! spec 17.2). Without a binding the device UI would keep reading and
//! writing the scene-effective sound while song playback sounds a take's or
//! clip's — the panel lies and edits are inaudible.
//!
//! The invariant (16.2): per track there is exactly ONE bound source, and
//! the panel display, the live monitor sound, and the take punch-in clone
//! template all read from it. This module owns the resolution (16.3) and the
//! selection lifecycle (16.6); routing edits through it lives in `edit.rs`
//! and the panel read surfaces.

use crate::sequencer::{ClipId, LaneSource, PatternId, RuntimeSongRow, SoundRefs, TakeId};

use super::App;

/// The arrangement's fixed bar grid, matching the timeline ruler.
const BEATS_PER_BAR: f64 = 4.0;

/// The device-parameter source a track is bound to. `Empty`/bare tracks
/// resolve to `None` rather than a variant — there is nothing to display or
/// edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundSource {
    /// A take: one Patch/Mix pair for the whole take (§17.2); its chunks all
    /// reference that pair, so a single entity write reaches every referent.
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
    /// The STORED clip's id (arrangement-lane-model-spec 12) — the timeline's
    /// gesture identity, used to render the bound-clip highlight.
    pub clip_id: ClipId,
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
    pub(crate) fn selected_bound_source(&self, track: usize) -> Option<BoundSource> {
        // Dormant while the timeline is off screen (16.6): in the Seq tab
        // nothing renders the bound clip, so a selection silently owning the
        // device panel reads as the panel showing the wrong sound.
        if !self.arrangement_view_visible {
            return None;
        }
        // Dormant on a latched lane while the song sounds (takes spec 10):
        // the scheduler plays a latched lane from the LIVE mirror (the
        // lookahead merges base-snapshot tracks for latched lanes), so a
        // selection borrow here is not display-only — it would audibly stamp
        // the selected take's devices onto the performer's clip the moment
        // the mirror is published. While the override holds, the lane's
        // sound, panel, and edit target are all the performer's clip; the
        // selection re-binds when the latch clears or the transport stops.
        if self.song_playback_authority_active()
            && self.state.is_playing()
            && track < 64
            && self.state.song_manual_latch_mask() >> track & 1 == 1
        {
            return None;
        }
        let selection = self.song_clip_selection?;
        if selection.track != track {
            return None;
        }
        self.bound_source_alive(track, selection.source)
            .then_some(selection.source)
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
        // A structural arrangement edit nulls the mirrored row until the next
        // scheduler-authoritative notice (`rebuild_active_song_after_arrangement_edit`).
        // Keying rule 2 off it alone would unbind EVERY track for that whole
        // window — the panel jumps back to the scene pattern and its sound is
        // pushed to the engine mid-playback. The scheduler's own row ordinal
        // is authoritative in the gap.
        let ordinal = match self.song_mirrored_row {
            Some(row) => row,
            None => self
                .state
                .song_playback()
                .shared()
                .current_row_ordinal(),
        };
        let row = song.rows.get(ordinal)?;
        row.resolved_sources.get(track).copied()
    }

    /// True while `source` still exists in the track's pools.
    fn bound_source_alive(&self, track: usize, source: BoundSource) -> bool {
        self.state.with_project_scenes(|scenes| match source {
            BoundSource::Take(id) => scenes
                .take_pools
                .get(track)
                .is_some_and(|takes| takes.contains(id)),
            BoundSource::Pattern(id) => scenes
                .track_pools
                .get(track)
                .is_some_and(|pool| pool.contains(id)),
        })
    }

    /// Gaps hold refs (§17.3): the last non-empty rule-2 source, still alive.
    /// An empty span between clips keeps binding (and sounding) this instead
    /// of resetting to the scene pattern.
    fn held_lane_source(&self, track: usize) -> Option<BoundSource> {
        let held = self.song_held_sources.get(track).copied().flatten()?;
        self.bound_source_alive(track, held).then_some(held)
    }

    /// Rule 2's lane source with the §17.3 gap hold applied: an `Empty` span
    /// resolves to the held source when one exists.
    fn audible_or_held_lane_source(&self, track: usize) -> Option<LaneSource> {
        match self.audible_lane_source(track)? {
            LaneSource::Empty => Some(match self.held_lane_source(track) {
                Some(BoundSource::Take(id)) => LaneSource::Take(id),
                Some(BoundSource::Pattern(id)) => LaneSource::Pattern(id),
                None => LaneSource::Empty,
            }),
            source => Some(source),
        }
    }

    /// Lanes whose sound survives an upcoming row transition unchanged
    /// (§17.3 gap hold + §2.8 borrow-release seam).
    ///
    /// A row apply releases every device loan, repaints each released lane's
    /// mirror from the lane *owner* (the scene cell, or the track-sound
    /// carrier), and then pushes that repaint at the engine — all before the
    /// gap hold gets a chance to re-resolve in `sync_track_sound_bindings`.
    /// For a lane the next row resolves to exactly what is already borrowed
    /// (a take followed by an empty span is the audible case: the ringing
    /// tail is still the take's), that whole round trip is a transient of a
    /// sound the user never dialed in, plus a defaults push that flattens
    /// the p-locks of any still-sounding note.
    ///
    /// Holding the lane out of both halves makes the boundary a no-op for
    /// it: nothing repaints the mirror, nothing pushes at the engine, and
    /// the binding sync afterwards finds the loan already in place.
    pub(crate) fn row_device_hold_mask(&self, row: &RuntimeSongRow) -> u64 {
        // Rule 2 only owns the lane while the song is actually sounding;
        // stopped or in session mode the row apply's repaint is correct.
        if !(self.song_playback_authority_active() && self.state.is_playing()) {
            return 0;
        }
        let borrowed = self.state.sound_binding_borrowed_mask();
        let latched = self.state.song_manual_latch_mask();
        let mut mask = 0u64;
        for track in 0..self.tracks.len().min(64) {
            if borrowed >> track & 1 == 0 || latched >> track & 1 == 1 {
                continue;
            }
            // A selection (rule 1) outranks the lane, and its borrow is
            // display-only while the song sounds — the row's own push is
            // what keeps the engine on the audible sound there.
            if self.selected_bound_source(track).is_some() {
                continue;
            }
            let next = match row.resolved_sources.get(track).copied() {
                Some(LaneSource::Take(id)) => Some(BoundSource::Take(id)),
                Some(LaneSource::Pattern(id)) => Some(BoundSource::Pattern(id)),
                // The gap hold: an empty span keeps sounding the last
                // non-empty source rather than resetting to the lane owner.
                Some(LaneSource::Empty) => self.held_lane_source(track),
                None => None,
            };
            let Some(next) = next else { continue };
            if self.loaded_sound_binding.get(track).copied().flatten() == Some(next)
                && self.bound_source_alive(track, next)
            {
                mask |= 1u64 << track;
            }
        }
        mask
    }

    /// The track's bound source (16.3). Cheap enough for per-frame reads:
    /// one scenes lock, no pattern clones.
    pub fn track_sound_binding(&self, track: usize) -> TrackBinding {
        // Rule 3 is VIEW-KEYED (track-sound spec §2.2.2): in arrangement
        // context the TRACK owns the sound, so rule 3a sits out entirely —
        // whatever cells exist are inert-but-visible. Dropping the candidate
        // keeps the origin `Scene` with no source, and every consumer falls
        // to the track sound (rule 3b) — what the lane is monitoring. Seq
        // context is the classic scene+pattern world.
        let effective = if self.arrangement_view_visible {
            None
        } else {
            self.state.effective_track_pattern_id(track)
        };
        resolve_binding(
            self.selected_bound_source(track),
            self.audible_or_held_lane_source(track),
            effective,
        )
    }

    /// Every pool pattern referencing the bound source's sound: one id for
    /// a pattern, every chunk for a take (they share one pair, §17.2).
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
    /// take's FIRST chunk stands for the take (every chunk references the
    /// take's one Patch/Mix pair, §17.2, so any chunk names the sound).
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

    /// The bound source's sound (§17.2): the `(patch_ref, mix_ref)` pair the
    /// binding resolves to. Rule 3 splits on a bare lane (track-sound spec
    /// §2.2): the effective scene pattern's refs when the cell (or override)
    /// actually resolves a pattern — rule 3a — else the TRACK SOUND's — rule
    /// 3b. Always resolves ("no steps" never means "no sound"), so this is
    /// `None` only for an out-of-range track.
    pub(crate) fn bound_sound_refs(&self, track: usize) -> Option<SoundRefs> {
        let source = self.track_sound_binding(track).source;
        // §2.2.2 (rev 4): the cell-refs fallback is rule 3a by another name,
        // so it obeys the same view rule — in arrangement context the
        // `track_sound_refs` fallback below supplies the owner instead.
        let arrangement = self.arrangement_view_visible;
        self.state.with_project_scenes(|scenes| {
            let from_source = match source {
                Some(BoundSource::Take(id)) => scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(id))
                    .map(|take| take.sound),
                Some(BoundSource::Pattern(id)) => scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.refs(id)),
                None => None,
            };
            from_source
                .or_else(|| {
                    if arrangement {
                        return None;
                    }
                    scenes
                        .effective_pattern_id(track)
                        .and_then(|id| scenes.track_pools.get(track)?.refs(id))
                })
                .or_else(|| scenes.track_sound_refs(track))
                .or_else(|| scenes.effective_sound_refs(track))
        })
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
    ///
    /// Since rev 4 this is also the OWNERSHIP switch (track-sound spec
    /// §2.2.2/§2.9): the one seam where ownership legitimately changes
    /// wholesale, so it moves edits OUT to the old owners and the new owners
    /// IN, in that order.
    pub fn set_arrangement_view_visible(&mut self, visible: bool) {
        if self.arrangement_view_visible == visible {
            // The state-side mirror of the flag can still be stale (a fresh
            // `SequencerState` after a project load starts at `false`), so
            // re-assert it unconditionally — cheap, and the ownership masks
            // are read from it.
            self.state.set_arrangement_context(visible);
            return;
        }
        // (a) Leaving a view: save back to THAT view's owners (§2.9.1). The
        // masks are still derived from the old context flag, so track-owned
        // lanes persist into their track sounds. Only the arrangement→Seq
        // direction runs it: Seq-context edits already write through to the
        // cell at edit time (device values and — since rev 4 — track params),
        // so a blind mirror→cell save on the way OUT of Seq view would add no
        // durability and could only clobber a cell from a mirror that some
        // borrow or row apply desynced (the §2.8 litmus).
        if self.arrangement_view_visible {
            self.state.save_current_pattern_snapshot(
                self.tracks.len(),
                &self.graph.track_buffer_ids,
                &self.graph.track_sample_rates,
                &self.tracks,
                &self.graph.track_instrument_types,
            );
        }
        // (b) Flip the context. Both copies move together: the App's own
        // reads (rule 3, device-edit targets) and the state's (save-back
        // masks, resync, `mirror_device_pattern_id`).
        self.arrangement_view_visible = visible;
        self.state.set_arrangement_context(visible);
        // (c) Entering a view: install the new owners into the mirror. Skipped
        // while song playback holds authority — the rows own the lanes there
        // and a wholesale repaint would fight the scheduler; the next stop's
        // resync installs the owners anyway.
        if !self.song_playback_authority_active() {
            if visible {
                // Arrangement: the track sound on every lane rules 1/2 do not
                // claim. Note content is untouched — only the device half.
                self.state.install_track_sounds_into_mirror();
            } else {
                // Seq: an ordinary resync installs the current scene's cells,
                // restoring rev-1 session behavior wholesale (including
                // clearing any track-sound hold).
                self.state.resync_live_grid_to_current_scene();
            }
        }
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
        if self.song_held_sources.len() != self.tracks.len() {
            self.song_held_sources.resize(self.tracks.len(), None);
        }
        // Gaps hold refs (§17.3): remember each lane's last non-empty rule-2
        // source. `Empty` keeps the hold (that is the point); losing song
        // authority for the lane (stop, manual latch, no song) clears it.
        let song_authoritative = self.song_playback_authority_active();
        for track in 0..self.tracks.len() {
            match (song_authoritative, self.audible_lane_source(track)) {
                (true, Some(LaneSource::Take(id))) => {
                    self.song_held_sources[track] = Some(BoundSource::Take(id));
                }
                (true, Some(LaneSource::Pattern(id))) => {
                    self.song_held_sources[track] = Some(BoundSource::Pattern(id));
                }
                (true, Some(LaneSource::Empty)) => {}
                (false, _) | (true, None) => self.song_held_sources[track] = None,
            }
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
            // The gap hold counts as audible (§17.3): in an empty span the
            // engine keeps the held sound, so edits to it stay live and the
            // monitor never resets.
            let audible = self
                .audible_or_held_lane_source(track)
                .and_then(BoundSource::from_lane);
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
        // Gap hold (§17.3): a held source in an empty span is audible-class —
        // the engine is carrying its params, so edits to it must stay live.
        let audible = self
            .audible_or_held_lane_source(track)
            .and_then(BoundSource::from_lane);
        loaded != audible
    }

    /// The pool pattern naming `source`'s sound: a take is represented by
    /// its first chunk (all chunks share the take's pair, §17.2).
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
    ///
    /// A take edit names the take by its FIRST chunk, while preflight stores
    /// the per-row CHUNK id — so the guard matches the whole chunk set, not
    /// just the id it was handed. Otherwise a take whose rows all resolve a
    /// later chunk (a clip offset past chunk 0) would skip the re-preflight
    /// and keep the pre-edit sound.
    pub fn invalidate_song_rows_for_pattern(&mut self, track: usize, pattern: PatternId) {
        if !self.song_playback_authority_active() {
            return;
        }
        let mut candidates = self.take_sibling_chunks(track, pattern);
        candidates.push(pattern);
        let affected = self.active_runtime_song.as_ref().is_some_and(|song| {
            song.rows.iter().any(|row| {
                row.resolved_pattern_ids
                    .get(track)
                    .copied()
                    .flatten()
                    .is_some_and(|resolved| candidates.contains(&resolved))
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

    /// **Push to pattern** (16.5, S2 semantics per §17.5): re-link the
    /// current scene's effective pattern (and its cell) to the bound
    /// source's `(patch_ref, mix_ref)`. Reference semantics: the scene now
    /// shares the take's sound, and future edits to it follow. Kept as a
    /// thin alias of the palette re-link until S3 gives it a real home.
    pub fn push_bound_sound_to_pattern(&mut self, track: usize) -> Result<String, String> {
        let binding = self.track_sound_binding(track);
        if binding.is_scene() {
            return Err("The track is already bound to its scene pattern".to_string());
        }
        let refs = self
            .bound_sound_refs(track)
            .ok_or_else(|| "The bound source has no sound".to_string())?;
        let target = self
            .state
            .effective_track_pattern_id(track)
            .ok_or_else(|| "The current scene has no pattern on this track".to_string())?;
        self.commit_sound_relink(
            track,
            &[target],
            &[],
            &[],
            Some(refs.patch),
            Some(refs.mix),
            "Push sound to pattern",
        )
    }

    /// **Apply to all takes on track** (16.5, S2 semantics): re-link every
    /// take on the track (take + chunks) to the bound source's refs. With
    /// takes sharing by default this is a repair/convergence tool for
    /// deliberately forked or legacy-imported takes. With no explicit
    /// binding the scene-effective refs are the source (rule 3, matching the
    /// §16.5 behavior): the gesture converges every take to the scene sound.
    pub fn apply_bound_sound_to_all_takes(&mut self, track: usize) -> Result<String, String> {
        let refs = self
            .bound_sound_refs(track)
            .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
        let takes: Vec<TakeId> = self.state.with_project_scenes(|scenes| {
            scenes
                .take_pools
                .get(track)
                .map(|takes| takes.takes.iter().map(|take| take.id).collect())
                .unwrap_or_default()
        });
        if takes.is_empty() {
            return Err(format!("Track {} has no takes", track + 1));
        }
        self.commit_sound_relink(
            track,
            &[],
            &takes,
            &[],
            Some(refs.patch),
            Some(refs.mix),
            "Apply sound to all takes",
        )
    }

    /// One re-link gesture = one undo entry (§17.4: an entity edit — and a
    /// repoint — affects all referents through one history entry). `patch` /
    /// `mix` may each be `None` to keep a target's current half (palette
    /// Apply is patch-only, §17.6); `cells` are bare-cell scene indices.
    pub(crate) fn commit_sound_relink(
        &mut self,
        track: usize,
        patterns: &[PatternId],
        takes: &[TakeId],
        cells: &[usize],
        patch: Option<crate::sequencer::PatchId>,
        mix: Option<crate::sequencer::MixId>,
        label: &'static str,
    ) -> Result<String, String> {
        let before = self.capture_synchronized_scene_structure_state()?;
        let changed = self
            .state
            .relink_track_sound_refs_masked(track, patterns, takes, cells, patch, mix)?;
        if changed == 0 {
            return Ok(format!("{label}: already linked"));
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
        // A re-link is a repoint (§17.10): rebind the engine through the one
        // restore seam, then re-preflight the rows that resolve the targets.
        self.after_sound_repoint(track);
        for pattern in patterns {
            self.invalidate_song_rows_for_pattern(track, *pattern);
        }
        let take_chunks: Vec<PatternId> = self.state.with_project_scenes(|scenes| {
            scenes
                .take_pools
                .get(track)
                .map(|pool| {
                    takes
                        .iter()
                        .filter_map(|id| pool.get(*id))
                        .flat_map(|take| take.chunks.iter().copied())
                        .collect()
                })
                .unwrap_or_default()
        });
        for chunk in take_chunks {
            self.invalidate_song_rows_for_pattern(track, chunk);
        }
        Ok(format!("{label}: {changed} referent(s) re-linked"))
    }

    /// The one seam every sound repoint routes through (§17.10 / macro spec
    /// §3.6): drop the lane's device loan so the next binding sync
    /// re-borrows the repointed sound, re-push restored defaults through the
    /// effective (macro-aware) layer, and re-stamp lock identity so p-locks
    /// and key locks survive the rebind (§18.2 item 6).
    pub(crate) fn after_sound_repoint(&mut self, track: usize) {
        if track < self.loaded_sound_binding.len() {
            self.loaded_sound_binding[track] = None;
        }
        self.state.release_bound_track_device_state(track);
        // Bare or track-owned lane (track-sound spec §2.2 rule 3b / §2.2.2):
        // the mirror is the track sound and no cell restore exists to
        // re-load it after a repoint — do it here so a palette Apply/Fork on
        // such a lane is audible. In arrangement context this includes lanes
        // whose cell resolves (inert-but-visible); read the mask AFTER the
        // release above so a just-released borrow counts as track-owned.
        let track_owned = track < 64 && self.state.track_owned_lane_mask() >> track & 1 == 1;
        if track_owned || self.state.effective_track_pattern_id(track).is_none() {
            self.state.restore_track_sound_to_mirror(track);
        } else {
            // Cell-owned lane (Seq context, §2.2 rule 3): the repoint moved
            // the cell onto a DIFFERENT Patch and nothing else repaints the
            // mirror — the release above only restores a lane that was
            // actually borrowed. Leaving the outgoing sound in the mirror
            // makes the next save-back (`capture_synchronized_scene_
            // structure_state`, a scene switch, a save) write it into
            // whatever the cell resolves to NOW, clobbering the incoming
            // pool Patch: the palette-Apply data loss of eseq-md9, where
            // auditioning entries converged every sound onto one content.
            self.state.restore_effective_cell_sound_to_mirror(track);
        }
        self.sync_track_sound_bindings();
        // §3.6: re-route every restored default through the effective layer —
        // this also revalidates macro mappings and re-asserts engaged
        // overrides instead of letting the repoint clobber them.
        self.push_all_restored_defaults();
        self.re_stamp_track_lock_identity(track);
    }

    /// The `param_node_id` re-stamping pass for one track (§18.2 item 6):
    /// re-sync every live slot against the graph's current descriptor so
    /// restored p-locks and key locks carry ids the staleness guards accept
    /// (`plock_variants.rs` / `audio/params.rs`). Mirrors the per-track body
    /// of `rebind_live_track_runtime_after_delete`.
    pub(crate) fn re_stamp_track_lock_identity(&mut self, track: usize) {
        use std::sync::atomic::Ordering;
        if let (Some(descs), Some(chain)) = (
            self.graph.effect_descriptors.get(track),
            self.state.pattern.effect_chains.get(track),
        ) {
            for (slot_idx, slot) in chain.iter().enumerate() {
                let Some(desc) = descs.get(slot_idx) else {
                    continue;
                };
                let node_id = slot.node_id.load(Ordering::Relaxed);
                slot.sync_descriptor(desc, node_id);
            }
        }
        if self.graph.track_instrument_types.get(track)
            == Some(&crate::sequencer::InstrumentType::Custom)
        {
            if let Some(desc) = self.graph.instrument_descriptors.get(track) {
                if let Some(slot) = self.state.pattern.instrument_slots.get(track) {
                    let node_id = slot.node_id.load(Ordering::Relaxed);
                    slot.sync_descriptor(desc, node_id);
                }
            }
        }
    }

    /// Select the clip a timeline gesture picked (16.6 causes 1–2). The
    /// timeline names the STORED clip by its id (lane spec 12), so the source
    /// is read straight off the clip and stamped into the selection:
    /// selection is intent about THIS take or clip, not about whatever a
    /// later edit resolves at that beat.
    pub fn select_song_clip(&mut self, track: usize, clip_id: ClipId) -> Result<(), String> {
        let arrangement = self
            .state
            .committed_arrangement()
            .ok_or_else(|| "The project has no committed song".to_string())?;
        let (clip_track, clip) = arrangement
            .find_clip(clip_id)
            .ok_or_else(|| format!("The arrangement has no clip with id {}", clip_id.0))?;
        if clip_track != track {
            return Err(format!(
                "Clip {} is on track {}, not track {}",
                clip_id.0,
                clip_track + 1,
                track + 1
            ));
        }
        // An empty clip is not a sound; a gesture on one is a deselect.
        let selection = BoundSource::from_lane(clip.source()).map(|source| SongClipSelection {
            track,
            clip_id,
            source,
        });
        self.set_song_clip_selection(selection);
        Ok(())
    }

    /// `select_song_clip` plus the region the clip occupies
    /// (docs/arrangement-region-editing-spec.md 4.1, amended): a title-bar
    /// click is BOTH gestures — it binds the track's sound to this clip and
    /// selects the clip's span as a one-track region, so the body lights up
    /// and copy/delete have something to act on. The span comes from the
    /// timeline because that is where the drawn item's extent lives. `None`
    /// (no clip under the pointer) clears the region.
    pub fn select_song_clip_span(
        &mut self,
        track: usize,
        clip_id: ClipId,
        span: Option<(f64, f64)>,
    ) -> Result<(), String> {
        self.select_song_clip(track, clip_id)?;
        // An empty lane is a deselect, not a selection: it must not leave a
        // region highlighting a clip that is not there.
        match span.filter(|_| self.song_clip_selection.is_some()) {
            Some((start, end)) => {
                self.set_song_region_for_clip(super::song_region::SongRegionSelection::new(
                    track, track, start, end,
                ));
            }
            None => {
                self.clear_song_region();
            }
        }
        Ok(())
    }

    /// Auto-select a freshly committed take (16.3/16.6 cause 3) so
    /// post-record tweaks bind to what the performer just played.
    pub(crate) fn select_committed_take(&mut self, track: usize, take: TakeId) {
        let clip_id = self.state.committed_arrangement().and_then(|arrangement| {
            arrangement
                .track_lanes
                .get(track)?
                .iter()
                .find(|clip| clip.take_id == Some(take.0))
                .map(|clip| clip.id)
        });
        let Some(clip_id) = clip_id else { return };
        self.set_song_clip_selection(Some(SongClipSelection {
            track,
            clip_id,
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
        self.push_solo_mutes();
        // Same sampler exemption as `push_all_restored_defaults`: sampler
        // voices get their params stamped per note (including p-locks), so
        // pushing slot DEFAULTS onto live voices would stomp a sounding
        // note's p-locked start/end — moving the region past the playhead
        // hard-kills the voice (the boundary-coincident silent-loop bug).
        if self.graph.track_instrument_types.get(track)
            == Some(&crate::sequencer::InstrumentType::Rack)
        {
            self.push_rack_slot_instrument_defaults_for_track(track);
        } else if !self.is_sampler_track(track) && !self.instrument_defaults_push_would_stomp(track)
        {
            self.push_instrument_defaults_for_track(track);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use crate::app::{command::AppCommand, edit::try_apply_command, AudioBuses};
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternSnapshot, SequencerState, TrackPatternData,
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
    pub(crate) fn app_with_take() -> (App, TakeId, PatternId, Vec<PatternId>) {
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
        // §2.6 seeding, re-run once the cell actually carries the instrument:
        // `ProjectScenes`' construction-time `ensure_track_sounds` fired
        // before the descriptor existed, so the carrier would otherwise hold
        // a device-less Patch and every rule-3b edit target would be empty.
        // Write through the carrier's OWN entities rather than re-seeding or
        // forking, both of which would leave orphans in the pool (the palette
        // cleanup pin counts them).
        state.with_scenes_mut(|scenes| {
            let carrier = scenes.track_sounds[0].expect("the carrier exists");
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("the cell resolves");
            assert!(scenes.track_pools[0].store(carrier, data));
        });

        let chunk = || -> TrackPatternData {
            let mut data = state
                .with_project_scenes(|scenes| scenes.effective_track_pattern(0))
                .expect("effective pattern");
            data.clear_step_content();
            data
        };
        let take = state
            .register_track_take(0, None, vec![chunk(), chunk()], 300, None)
            .expect("take registers");
        let chunks = state
            .with_project_scenes(|scenes| scenes.take_pools[0].get(take).unwrap().chunks.clone());

        // The arrangement is the stored model (lane spec 2/12); installing it
        // compiles the equivalent one-row song, so the timeline names the
        // take by ClipId(0).
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new_take(
            crate::sequencer::ClipId(0),
            0.0,
            16.0,
            take.0,
            0.0,
        ));
        arrangement.next_clip_id = 1;
        state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");

        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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
        // Rev 4: ownership is view-keyed, and the state-side consumers read
        // their own copy of the flag (§2.2.2). Set both directly — the
        // public setter runs the view-switch seam (§2.9), which a fixture
        // has no business firing.
        app.state.set_arrangement_context(true);
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
        app.select_song_clip(0, ClipId(0))
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
        // The mechanism is ref identity, not mirrored writes (§17.2): every
        // chunk references the take's one Patch/Mix pair, and the scene
        // pattern references a different one.
        app.state.with_project_scenes(|scenes| {
            let take_refs = scenes.track_pools[0].refs(chunks[0]).expect("chunk refs");
            for chunk in &chunks {
                assert_eq!(scenes.track_pools[0].refs(*chunk), Some(take_refs));
            }
            assert_ne!(
                scenes.track_pools[0].refs(scene_pattern),
                Some(take_refs),
                "this take was registered with a private pair"
            );
        });
    }

    /// 16.6 cause 1: deselecting returns the binding — and the edit target —
    /// to rule 3, which under rev 4 is view-keyed (track-sound spec §2.2.2).
    /// In the arrangement view that is the TRACK SOUND; the scene cell is
    /// inert-but-visible and must not absorb the edit.
    #[test]
    fn deselecting_returns_edits_to_the_view_owner() {
        let (mut app, _take, scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0))
            .expect("clip selects");
        app.set_song_clip_selection(None);
        assert!(app.track_sound_binding(0).is_scene());
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");

        let chunk_before = instrument_default(&app, chunks[0]);
        let scene_before = instrument_default(&app, scene_pattern);
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

        assert_eq!(
            instrument_default(&app, carrier),
            target,
            "arrangement view: rule 3 is the track sound"
        );
        assert_eq!(
            instrument_default(&app, scene_pattern),
            scene_before,
            "the cell is inert-but-visible in arrangement view"
        );
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
        app.select_song_clip(0, ClipId(0))
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
            clip_id: ClipId(0),
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
        app.select_song_clip(0, ClipId(0))
            .expect("clip selects");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );
        assert!(!app.sound_binding_is_silent(0));
    }

    /// 16.5: Push to pattern promotes the bound take's sound to the current
    /// scene's pattern, as one undo entry.
    #[test]
    fn push_to_pattern_promotes_the_bound_takes_sound() {
        let (mut app, _take, scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0))
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
        // S2: push-to-pattern is a RE-LINK (§17.5) — the scene pattern and
        // its cell now reference the take's entities, so future edits to the
        // take's sound follow automatically.
        app.state.with_project_scenes(|scenes| {
            let take_refs = scenes.track_pools[0].refs(chunks[0]).expect("chunk refs");
            assert_eq!(
                scenes.track_pools[0].refs(scene_pattern),
                Some(take_refs),
                "the scene pattern re-linked to the take's refs"
            );
            assert_eq!(
                scenes.scenes[scenes.current_scene].cell_sounds[0], take_refs,
                "the cell followed its pattern"
            );
        });
    }

    /// eseq-ut5j: pressing a scene during arrangement capture must not
    /// retune the takes that share the track sound.
    ///
    /// The launch repaints EVERY lane's mirror from the scene's cells. Under
    /// the old take-lane carve-out the lane it declined to claim came out
    /// unlatched, unborrowed and — in arrangement context — track-owned,
    /// with the scene cell's devices in its mirror; the stop save-back then
    /// persisted that into the shared track-sound entities and every take
    /// referencing them retuned (§2.4.1 sharing turns one poisoned write
    /// into a whole track's history changing sound). Claiming the lane
    /// latches it out of `track_owned_lane_mask`, so the save-back is a
    /// self-write into the pinned cell instead.
    #[test]
    fn a_capture_scene_launch_never_retunes_takes_sharing_the_track_sound() {
        let (mut app, _take, _take_value, _carrier_value) = app_with_take_then_gap();
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");
        let carrier_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].refs(carrier))
            .expect("carrier refs");
        // A take that SHARES the track sound, like every take punch-in mints
        // (takes spec §17.3 "record → share").
        let chunk = app
            .state
            .with_project_scenes(|scenes| scenes.effective_track_pattern(0))
            .expect("effective pattern");
        let shared_take = app
            .state
            .register_track_take(0, None, vec![chunk], 300, Some(carrier_refs))
            .expect("take registers sharing the carrier");
        let shared_chunk = app.state.with_project_scenes(|scenes| {
            scenes.take_pools[0].get(shared_take).expect("take").chunks[0]
        });
        let carrier_before = instrument_default(&app, carrier);
        assert_eq!(
            instrument_default(&app, shared_chunk),
            carrier_before,
            "the take and the track sound are one entity"
        );

        // The user dials a DIFFERENT sound into the scene cell from the Seq
        // view — legitimately the cell's, never the track's.
        app.set_arrangement_view_visible(false);
        let cell_value = carrier_before - 0.25;
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: cell_value,
            },
        )
        .expect("device edit applies");
        crate::app::edit::finish_active_gesture(&mut app);
        app.set_arrangement_view_visible(true);
        assert_eq!(
            instrument_default(&app, carrier),
            carrier_before,
            "the Seq-view edit stayed on the cell"
        );

        // Record, press the scene mid-pass, stop.
        app.song_transport_play(true).expect("capture starts");
        app.apply_pattern_launch(&crate::quantized_launch::PatternLaunchTarget::Scene {
            scene: 0,
        })
        .expect("scene launches during capture");
        assert_eq!(
            app.state.song_manual_latch_mask() & 1,
            1,
            "the scene claims the take lane, so it leaves track_owned_lane_mask"
        );
        let _ = app.song_transport_stop();

        assert_eq!(
            instrument_default(&app, carrier),
            carrier_before,
            "the stop save-back must not write the scene cell's sound into the track sound"
        );
        assert_eq!(
            instrument_default(&app, shared_chunk),
            carrier_before,
            "so the takes sharing it keep the sound they were recorded with"
        );
    }

    /// eseq-2lji / track-sound spec §2.5 rev 5: dialing a sound into a scene
    /// whose cell is EMPTY must stay scene-local.
    ///
    /// Rule 3's Seq-context fallback used to hand a bare lane's device edit
    /// to the track-sound CARRIER. Every take the track has recorded shares
    /// that carrier (§2.4.1), so one knob turn silently retuned the whole
    /// track's history — and nothing in the Seq view said the lane was bare,
    /// so the edit looked like it belonged to the scene the user was
    /// standing in. The edit now materializes the cell instead, forking the
    /// sound off the track sound.
    #[test]
    fn a_seq_view_edit_on_a_bare_cell_materializes_it_instead_of_the_track_sound() {
        let (mut app, _take, _take_value, _carrier_value) = app_with_take_then_gap();
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");
        let carrier_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].refs(carrier))
            .expect("carrier refs");
        // A take that SHARES the track sound, as every punch-in mints
        // (§17.3 "record → share").
        let chunk = app
            .state
            .with_project_scenes(|scenes| scenes.effective_track_pattern(0))
            .expect("effective pattern");
        let shared_take = app
            .state
            .register_track_take(0, None, vec![chunk], 300, Some(carrier_refs))
            .expect("take registers sharing the carrier");
        let shared_chunk = app.state.with_project_scenes(|scenes| {
            scenes.take_pools[0].get(shared_take).expect("take").chunks[0]
        });
        // The scene's cell for this lane is empty: the user dials the sound
        // in BEFORE punching a pattern into it.
        app.state.with_scenes_mut(|scenes| {
            scenes.scenes[0].cells[0] = None;
        });
        let carrier_before = instrument_default(&app, carrier);
        assert!(
            app.state.effective_track_pattern_id(0).is_none(),
            "the lane is bare in this scene"
        );

        app.set_arrangement_view_visible(false);
        let target = carrier_before - 0.25;
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: target,
            },
        )
        .expect("device edit applies");
        crate::app::edit::finish_active_gesture(&mut app);

        let cell = app
            .state
            .effective_track_pattern_id(0)
            .expect("the edit materialized the scene's cell");
        assert_eq!(
            instrument_default(&app, cell),
            target,
            "the edit landed on the new cell"
        );
        assert_eq!(
            instrument_default(&app, carrier),
            carrier_before,
            "the track sound is untouched"
        );
        assert_eq!(
            instrument_default(&app, shared_chunk),
            carrier_before,
            "so every take sharing it keeps the sound it was recorded with"
        );
        // The fork is the point: the new cell must own its Patch/Mix, or the
        // NEXT edit would poison the takes instead of this one.
        app.state.with_project_scenes(|scenes| {
            assert_ne!(
                scenes.track_pools[0].refs(cell),
                scenes.track_pools[0].refs(carrier),
                "the materialized cell forked its sound off the track sound"
            );
        });
    }

    /// eseq-wypz: a row's take lane must put the TAKE's sound behind the
    /// panel, not the scene cell's.
    ///
    /// `apply_song_row_latched` deliberately keeps a take lane's SESSION
    /// identity on the scene cell (the live grid must not show take chunks),
    /// and it did that with a full `restore_to` — which also dragged the
    /// cell's DEVICE half into the mirror. The caller's
    /// `push_all_restored_defaults` then handed that to the engine, and only
    /// the binding sync a step later re-borrowed the take and pushed the
    /// right sound. On a project where the cell and the take have diverged
    /// (dial a sound into a scene, then play the arrangement) the wrong
    /// sound is what you hear.
    #[test]
    fn a_rows_take_lane_installs_the_takes_sound_not_the_scene_cells() {
        let (app, take, take_value, cell_value) = app_with_take_then_gap();
        let scene_pattern = app
            .state
            .effective_track_pattern_id(0)
            .expect("the scene cell resolves");
        let chunk = app.state.with_project_scenes(|scenes| {
            scenes.take_pools[0].get(take).expect("take").chunks[0]
        });
        assert_ne!(
            take_value, cell_value,
            "the repro needs the take and the scene cell to have diverged"
        );

        let live = |app: &App| app.state.pattern.instrument_slots[0].defaults.get(0);
        // Put the CELL's sound behind the panel, so a stale repaint is
        // visible as "nothing moved" rather than accidentally correct.
        app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0].get(scene_pattern).expect("cell")
        });
        assert_eq!(live(&app), cell_value, "the mirror starts on the cell");

        // The row plays the take on this lane while the scene cell exists —
        // exactly the shape `restore_to` used to repaint over.
        app.state
            .apply_song_row_latched(
                0,
                &[(0, Some(chunk))],
                1,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
                false,
                0,
                false,
                0,
            )
            .expect("the row applies");

        assert_eq!(
            live(&app),
            take_value,
            "the take lane's mirror is the take's sound, so the defaults push \
             that follows cannot hand the engine the cell's"
        );
    }

    /// The value the LIVE mirror would hand the engine for track 0's first
    /// instrument parameter — what every `push_all_restored_defaults` reads.
    fn live_instrument_default(app: &App) -> f32 {
        app.state.pattern.instrument_slots[0].defaults.get(0)
    }

    /// `app_with_take` reshaped into the eseq-cwx8 repro: the take covers
    /// `[0, 8)` with an empty span after it, and the take's sound is a
    /// DIFFERENT Patch from the one the scene cell / track-sound carrier
    /// holds — so any repaint from the lane owner is audible rather than
    /// coincidentally identical.
    ///
    /// Returns the app, the take, and (take value, carrier value).
    fn app_with_take_then_gap() -> (App, TakeId, f32, f32) {
        let (app, take, scene_pattern, chunks) = app_with_take();
        const TAKE_VALUE: f32 = 0.1875;
        app.state.with_scenes_mut(|scenes| {
            for chunk in &chunks {
                let mut data = scenes.track_pools[0].get(*chunk).expect("chunk in pool");
                data.instrument_slot.defaults[0] = TAKE_VALUE;
                assert!(scenes.track_pools[0].store(*chunk, data));
            }
        });
        let carrier_value = instrument_default(&app, scene_pattern);
        assert_ne!(
            carrier_value, TAKE_VALUE,
            "the repro needs the take's sound to differ from the lane owner's"
        );

        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new_take(
            ClipId(0),
            0.0,
            8.0,
            take.0,
            0.0,
        ));
        arrangement.next_clip_id = 1;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");
        (app, take, TAKE_VALUE, carrier_value)
    }

    /// The ordinal of the first row whose track-0 lane resolves `Empty`.
    fn empty_row_ordinal(app: &App) -> usize {
        app.active_runtime_song
            .as_ref()
            .expect("runtime song")
            .rows
            .iter()
            .position(|row| matches!(row.resolved_sources.first(), Some(LaneSource::Empty)))
            .expect("the arrangement compiles an empty span row")
    }

    /// eseq-cwx8: crossing take -> empty through the REAL row-mirror path
    /// (`mirror_song_row_applied` -> `apply_song_row_latched` ->
    /// `release_borrowed_lanes` -> `push_all_restored_defaults`) must not
    /// hand the engine the lane owner's sound for the window before the
    /// §17.3 gap hold re-resolves. The lane's loan is held across the whole
    /// apply, so nothing repaints the mirror and nothing pushes.
    #[test]
    fn take_to_empty_boundary_never_repaints_the_lane_owner_sound() {
        let (mut app, take, take_value, carrier_value) = app_with_take_then_gap();
        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "row 0 plays the take"
        );
        assert_eq!(
            live_instrument_default(&app),
            take_value,
            "the take's sound is what the engine is being handed"
        );

        let empty_row = empty_row_ordinal(&app);
        let row = app.active_runtime_song.as_ref().expect("runtime song").rows[empty_row].clone();
        assert_eq!(
            app.row_device_hold_mask(&row),
            1,
            "the gap row resolves track 0 to the already-borrowed take"
        );
        let epoch_before = app.sound_binding_epoch;
        let notice = crate::sequencer::AudibleSongRowApplied {
            row_id: row.id,
            row_ordinal: empty_row,
            effective_beat: row.start_beat,
            effective_sample: 0,
            wrapped: false,
        };
        app.mirror_song_row_applied(&notice)
            .expect("the row mirror applies");

        assert_eq!(
            app.state.sound_binding_borrowed_mask() & 1,
            1,
            "the loan is never dropped, so no owner repaint and no engine push \
             can happen in the window before the gap hold re-resolves"
        );
        assert_eq!(
            app.sound_binding_epoch, epoch_before,
            "the binding never moved: the gap holds the same take"
        );
        assert_eq!(
            live_instrument_default(&app),
            take_value,
            "the boundary left the take's sound behind the panel"
        );
        assert_ne!(live_instrument_default(&app), carrier_value);
        assert_eq!(
            app.loaded_sound_binding[0],
            Some(BoundSource::Take(take)),
            "the gap holds the take's sound loaded (§17.3)"
        );
        assert!(!app.sound_binding_is_silent(0), "the held sound is the monitor");
        app.song_transport_stop().expect("stop succeeds");
    }

    /// Why the hold mask exists: with it cleared, the row apply's blanket
    /// `release_bound_device_state` repaints the released lane from its
    /// owner (the track-sound carrier in arrangement context) — the snap
    /// eseq-cwx8 reported, and the state that `push_all_restored_defaults`
    /// would then push at the engine.
    #[test]
    fn a_row_apply_without_the_hold_repaints_the_lane_owner() {
        let (mut app, _take, take_value, carrier_value) = app_with_take_then_gap();
        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();
        assert_eq!(live_instrument_default(&app), take_value);

        let empty_row = empty_row_ordinal(&app);
        let row = app.active_runtime_song.as_ref().expect("runtime song").rows[empty_row].clone();
        let scene = row.scene.unwrap_or_else(|| app.state.current_scene_index());
        let apply = |app: &App, hold_mask: u64| {
            app.state
                .apply_song_row_latched(
                    scene,
                    &row.overrides,
                    1,
                    &app.graph.track_buffer_ids,
                    &app.graph.track_sample_rates,
                    &app.tracks,
                    &app.graph.track_instrument_types,
                    false,
                    0,
                    false,
                    hold_mask,
                )
                .expect("the row applies");
        };

        apply(&app, 1);
        assert_eq!(
            live_instrument_default(&app),
            take_value,
            "the held lane keeps its loan and its sound"
        );

        apply(&app, 0);
        assert_eq!(
            live_instrument_default(&app),
            carrier_value,
            "released without the hold, the lane snaps to its owner's sound"
        );
        app.song_transport_stop().expect("stop succeeds");
    }

    /// Gaps hold refs (§17.3 / §18.2 item 3): when the playhead leaves the
    /// take clip for an empty span, the binding — and the monitor — retain
    /// the take's sound instead of resetting to the scene pattern. An empty
    /// span is *no events*, not *no sound*.
    #[test]
    fn empty_spans_hold_the_last_resolved_binding() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        // Re-shape the arrangement: the take covers [0, 8) and [8, 16) is an
        // empty span, so the compiled song has a row whose lane resolves
        // Empty.
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new_take(
            ClipId(0),
            0.0,
            8.0,
            take.0,
            0.0,
        ));
        arrangement.next_clip_id = 1;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");

        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "row 0 plays the take"
        );
        assert!(!app.sound_binding_is_silent(0));

        // The playhead reaches the empty span (the row after the clip).
        let song = app.active_runtime_song.as_ref().expect("runtime song");
        let empty_row = song
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row.resolved_sources.first(),
                    Some(crate::sequencer::LaneSource::Empty)
                )
            })
            .expect("the arrangement compiles an empty span row");
        app.song_mirrored_row = Some(empty_row);
        app.sync_track_sound_bindings();

        let binding = app.track_sound_binding(0);
        assert_eq!(
            binding.source,
            Some(BoundSource::Take(take)),
            "the gap holds the last resolved source (§17.3)"
        );
        assert_eq!(binding.origin, BindingOrigin::Playback);
        assert_eq!(
            app.loaded_sound_binding[0],
            Some(BoundSource::Take(take)),
            "the mirror keeps the held sound loaded — no reset to scene params"
        );
        assert!(
            !app.sound_binding_is_silent(0),
            "the held sound stays the monitor: no silence in the gap"
        );
        app.song_transport_stop().expect("stop succeeds");
        app.sync_track_sound_bindings();
        assert!(
            app.track_sound_binding(0).is_scene(),
            "stopping clears the hold and falls back to the scene"
        );
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
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new(
            ClipId(0),
            0.0,
            16.0,
            Some(other.0),
        ));
        arrangement.next_clip_id = 1;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");

        let epoch = app.sound_binding_epoch;
        app.select_song_clip(0, ClipId(0))
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

    /// A latched lane's sound belongs to the performer's clip, not the
    /// arrangement: when the playhead crosses a take region on an overridden
    /// track, the take's sound must NOT be pushed onto the lane — the
    /// overriding clip owns the sound until Back to Arrangement.
    #[test]
    fn a_latched_lane_keeps_the_overriding_clips_sound_across_take_rows() {
        let (mut app, _take, scene_pattern, chunks) = app_with_take();
        // Give the take a sound of its own so a leak is observable.
        app.state.with_scenes_mut(|scenes| {
            for chunk in &chunks {
                assert!(scenes.track_pools[0].edit(*chunk, |data| {
                    data.instrument_slot.defaults[0] = 0.75;
                }));
            }
        });
        let scene_value = instrument_default(&app, scene_pattern);
        assert_ne!(scene_value, 0.75);

        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            0.75,
            "row 0 audibly plays the take"
        );

        // The performer overrides the lane: a manual launch always latches.
        app.apply_manual_pattern_launch(&crate::quantized_launch::PatternLaunchTarget::Scene {
            scene: 0,
        })
        .expect("manual launch");
        assert_eq!(app.state.song_manual_latch_mask() & 1, 1);
        app.sync_track_sound_bindings();
        assert!(
            app.track_sound_binding(0).is_scene(),
            "a latched lane binds its own clip"
        );
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            scene_value,
            "the override's clip owns the audible sound"
        );

        // The playhead re-enters the take's row while the latch holds (loop
        // wrap): the row mirror must leave the latched lane's sound alone.
        let row_id = app.active_runtime_song.as_ref().expect("runtime song").rows[0].id;
        app.mirror_song_row_applied(&crate::sequencer::AudibleSongRowApplied {
            row_id,
            row_ordinal: 0,
            effective_beat: 0.0,
            effective_sample: 0,
            wrapped: true,
        })
        .expect("row mirror");
        assert!(app.track_sound_binding(0).is_scene());
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            scene_value,
            "crossing a take region must not steal the latched lane's sound"
        );
        app.song_transport_stop().expect("stop succeeds");
    }

    /// Same guarantee with a take clip SELECTED in the timeline (the state
    /// right after recording takes — `select_committed_take` leaves the take
    /// bound): overriding the lane must not let the selection's device
    /// borrow become the lane's audible sound. The scheduler schedules a
    /// latched lane from the PUBLISHED live snapshot, so whatever device
    /// state sits in the mirror at publish time IS the per-note sound.
    #[test]
    fn a_selected_take_does_not_resound_a_latched_lane() {
        let (mut app, take, scene_pattern, chunks) = app_with_take();
        app.state.with_scenes_mut(|scenes| {
            for chunk in &chunks {
                assert!(scenes.track_pools[0].edit(*chunk, |data| {
                    data.instrument_slot.defaults[0] = 0.75;
                }));
            }
        });
        let scene_value = instrument_default(&app, scene_pattern);
        assert_ne!(scene_value, 0.75);

        // The take clip is selected/bound — the post-recording state.
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();

        // The performer overrides the lane.
        app.apply_manual_pattern_launch(&crate::quantized_launch::PatternLaunchTarget::Scene {
            scene: 0,
        })
        .expect("manual launch");
        assert_eq!(app.state.song_manual_latch_mask() & 1, 1);
        app.sync_track_sound_bindings();

        // The playhead crosses the take's row while the latch holds.
        let row_id = app.active_runtime_song.as_ref().expect("runtime song").rows[0].id;
        app.mirror_song_row_applied(&crate::sequencer::AudibleSongRowApplied {
            row_id,
            row_ordinal: 0,
            effective_beat: 0.0,
            effective_sample: 0,
            wrapped: true,
        })
        .expect("row mirror");

        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            scene_value,
            "the latched lane's mirror must keep the clip's sound"
        );
        // The published snapshot is what the scheduler stamps per note on a
        // latched lane — it must carry the clip's device state.
        app.state.publish_scheduler_snapshot();
        let published = app.state.latest_scheduler_snapshot();
        assert_eq!(
            published.tracks[0].instrument_slot.defaults.first().copied(),
            Some(scene_value),
            "the scheduler must stamp the clip's sound on the latched lane"
        );
        assert!(
            app.track_sound_binding(0).is_scene(),
            "while the override sounds, the lane's panel is the clip too"
        );
        app.song_transport_stop().expect("stop succeeds");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "stopping re-binds the dormant selection for tuning"
        );
    }

    /// Empty every scene cell on track 0 (the takes-only workflow the
    /// track-sound spec exists for): cells cleared, grid patterns deleted.
    /// The track sound carrier survives in the pool.
    fn empty_track_lane(app: &mut App) {
        app.state.with_scenes_mut(|scenes| {
            let scene_count = scenes.scenes.len();
            for scene_idx in 0..scene_count {
                if let Some(id) = scenes.clear_cell(scene_idx, 0) {
                    scenes.delete_track_pattern(0, id);
                }
            }
        });
        assert!(app.state.effective_track_pattern_id(0).is_none());
    }

    /// Track-sound spec §2.2 (symptom 1): a knob touch on a bare lane mints
    /// no scene cell — the edit lands on the TRACK SOUND's Patch entity.
    #[test]
    fn knob_touch_on_a_bare_lane_creates_no_cell_and_edits_the_track_sound() {
        let (mut app, _take, _scene_pattern, chunks) = app_with_take();
        empty_track_lane(&mut app);
        let refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_sound_refs(0))
            .expect("the track sound resolves");

        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: 0.123,
            },
        )
        .expect("a bare-lane device edit applies");

        app.state.with_project_scenes(|scenes| {
            for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
                assert_eq!(
                    scene.cells[0], None,
                    "scene {} minted no cell for the knob touch",
                    scene_idx + 1
                );
            }
            let patch = &scenes.track_pools[0].sounds.patches[&refs.patch];
            assert_eq!(
                patch.instrument_slot.defaults.first().copied(),
                Some(0.123),
                "the edit landed on the track sound's Patch entity"
            );
            for chunk in &chunks {
                assert_ne!(
                    scenes.track_pools[0]
                        .get(*chunk)
                        .expect("chunk")
                        .instrument_slot
                        .defaults
                        .first()
                        .copied(),
                    Some(0.123),
                    "the committed take's sound is untouched"
                );
            }
        });
    }

    /// Track-sound spec §2.3 (symptom 2): ghost live-grid steps left behind
    /// by a deleted clip never re-materialize a cell when Play saves the
    /// session.
    #[test]
    fn play_does_not_resurrect_a_deleted_clip_from_ghost_live_grid_steps() {
        let (mut app, _take, _scene_pattern, chunks) = app_with_take();
        empty_track_lane(&mut app);
        // Simulate pre-fix leftovers: active steps sitting in the live grid.
        app.state.pattern.patterns[0].set_step_active(3, true);

        app.song_transport_play(false).expect("song playback starts");
        app.song_transport_stop().expect("stop succeeds");

        app.state.with_project_scenes(|scenes| {
            for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
                assert_eq!(
                    scene.cells[0], None,
                    "scene {} resurrected a cell from ghost steps",
                    scene_idx + 1
                );
            }
            let grid_patterns = scenes.track_pools[0]
                .patterns
                .keys()
                .filter(|id| Some(**id) != scenes.track_sounds[0])
                .filter(|id| !chunks.contains(id))
                .count();
            assert_eq!(grid_patterns, 0, "no pool pattern was minted");
        });
    }

    /// Track-sound spec §2.7 (symptom 3): Play/Pause on a bare lane moves
    /// neither the audible device state nor the palette's resolved sound —
    /// the track sound is transport- and scene-independent.
    #[test]
    fn pause_does_not_change_the_audible_sound_or_palette_selection_on_a_bare_lane() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        // Two scenes so arrangement playback actually moves current_scene.
        app.state.with_scenes_mut(|scenes| {
            scenes.new_scene();
            scenes.current_scene = 0;
        });
        empty_track_lane(&mut app);
        // An arrangement whose rows reference both scenes; the lane itself
        // has no clips.
        app.arr_replace_rows(
            vec![
                crate::app::song_edit::SongRowSpec {
                    start_beat: 0.0,
                    scene: 0,
                    overrides: Vec::new(),
                },
                crate::app::song_edit::SongRowSpec {
                    start_beat: 8.0,
                    scene: 1,
                    overrides: Vec::new(),
                },
            ],
            16.0,
            false,
        )
        .expect("rows replace");

        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: 0.42,
            },
        )
        .expect("bare-lane device edit applies");
        let palette_before = app
            .resolve_palette_target(
                0,
                app.palette_target_or_binding(0, None),
            )
            .expect("palette target resolves")
            .current;

        // Play from beat 9 — the cursor row's scene is scene 1, so the old
        // bug re-owned the lane through scenes.current_scene.
        app.arrangement_cursor_beat = 9.0;
        app.song_transport_play(false).expect("song playback starts");
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            0.42,
            "Play leaves the bare lane's audible device state alone"
        );
        app.song_transport_stop().expect("stop succeeds");
        app.sync_track_sound_bindings();

        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            0.42,
            "Pause neither retunes the lane nor restores a scene cell"
        );
        let palette_after = app
            .resolve_palette_target(
                0,
                app.palette_target_or_binding(0, None),
            )
            .expect("palette target resolves")
            .current;
        assert_eq!(
            palette_before, palette_after,
            "the palette's resolved sound does not flap across Play/Pause"
        );
        app.state.with_project_scenes(|scenes| {
            for scene in &scenes.scenes {
                assert_eq!(scene.cells[0], None, "the lane stays bare");
            }
        });
    }

    /// Move the ownership context without firing the §2.9 view-switch seam:
    /// resolution pins want the rule, not the seam's mirror traffic.
    pub(crate) fn set_view_context(app: &mut App, arrangement: bool) {
        app.arrangement_view_visible = arrangement;
        app.state.set_arrangement_context(arrangement);
    }

    /// Track-sound spec §2.2.2 (symptom 8, the 8-bar scenario): ownership is
    /// keyed to the VIEW, not to transport history. STOPPED, nothing ever
    /// played, cells resolving — in arrangement view rule 3a sits out and the
    /// TRACK SOUND owns the lane; in Seq view the classic cell owns it. No
    /// silencing, no playback, no cursor involved.
    #[test]
    fn ownership_follows_the_view_not_the_transport() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let cell_refs = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].refs(scene_pattern))
            .expect("the scene cell has a sound");
        let track_sound = app
            .state
            .with_project_scenes(|scenes| scenes.track_sound_refs(0))
            .expect("track sound resolves");
        assert_ne!(cell_refs, track_sound, "a leak must be observable");
        assert!(!app.state.is_playing(), "the whole point: stopped");
        assert!(
            !app.state.is_scene_silenced(0),
            "and never silenced — rev 2/3's predicate would bind the cell here"
        );
        assert_eq!(
            app.state.effective_track_pattern_id(0),
            Some(scene_pattern),
            "the cell resolves; it is inert-but-visible, not absent"
        );

        set_view_context(&mut app, true);
        assert_eq!(
            app.track_sound_binding(0).source,
            None,
            "arrangement view: rule 3a sits out"
        );
        assert_eq!(
            app.bound_sound_refs(0),
            Some(track_sound),
            "arrangement view: the track owns the sound (rule 3b)"
        );

        set_view_context(&mut app, false);
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Pattern(scene_pattern)),
            "Seq view: the pure scene+pattern world returns"
        );
        assert_eq!(app.bound_sound_refs(0), Some(cell_refs));
    }

    /// Track-sound spec §5.3.6d: in SEQ context nothing about rev 4 is
    /// visible — a device edit on a lane with a cell writes the CELL, and the
    /// track sound stays dormant. The twin of
    /// `deselecting_returns_edits_to_the_view_owner`.
    #[test]
    fn seq_context_device_edits_still_write_the_cell() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        set_view_context(&mut app, false);
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");
        let carrier_before = instrument_default(&app, carrier);
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

        assert_eq!(
            instrument_default(&app, scene_pattern),
            target,
            "Seq view: the classic cell owns the edit"
        );
        assert_eq!(
            instrument_default(&app, carrier),
            carrier_before,
            "the track sound is dormant in Seq view"
        );
    }

    /// Track-sound spec §2.9/§5.3.6b: the view switch is a first-class mirror
    /// seam. Leaving arrangement view persists the mirror into the TRACK
    /// SOUND and entering Seq view installs the scene's cells; switching back
    /// reinstalls the track sound with the edit still on it.
    #[test]
    fn a_view_switch_roundtrip_moves_edits_out_and_owners_in() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");
        let pool_volume = |app: &App, id: PatternId| {
            app.state.with_project_scenes(|scenes| {
                scenes.track_pools[0]
                    .get(id)
                    .expect("pattern resolves")
                    .track_params
                    .volume
            })
        };
        let cell_volume = pool_volume(&app, scene_pattern);
        assert_ne!(cell_volume.to_bits(), 0.77f32.to_bits());
        // A live mixer move in arrangement context: unsaved, mirror-only.
        app.state.pattern.track_params[0].set_volume(0.77);

        app.set_arrangement_view_visible(false);

        assert_eq!(
            pool_volume(&app, carrier).to_bits(),
            0.77f32.to_bits(),
            "leaving arrangement view saves back to the track sound (§2.9.1)"
        );
        assert_eq!(
            app.state.pattern.track_params[0].get_volume().to_bits(),
            cell_volume.to_bits(),
            "entering Seq view installs the scene's cells (§2.9.2)"
        );

        app.set_arrangement_view_visible(true);

        assert_eq!(
            app.state.pattern.track_params[0].get_volume().to_bits(),
            0.77f32.to_bits(),
            "entering arrangement view reinstalls the track sound, edit intact"
        );
    }

    /// Track-sound spec §2.9/§5.3.6c (symptom 8's second half): track params
    /// write through to the owning entity AT EDIT TIME, so a mirror repaint
    /// — here the borrow/release cycle a selection performs — cannot discard
    /// them. Before rev 4 `polyphonic` lived only in the mirror until a stop
    /// save-back that anything could preempt.
    #[test]
    fn track_params_write_through_to_the_owner_at_edit_time() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        let carrier = app
            .state
            .track_sound_pattern_id(0)
            .expect("the track sound resolves");
        let before = app.state.pattern.track_params[0].is_polyphonic();

        try_apply_command(&mut app, AppCommand::ToggleTrackPolyphonic { track: 0 })
            .expect("the toggle applies");

        assert_eq!(
            app.state.with_project_scenes(|scenes| scenes.track_pools[0]
                .get(carrier)
                .expect("carrier resolves")
                .track_params
                .polyphonic),
            !before,
            "the edit reached the owning entity immediately"
        );

        // A mirror repaint with no save-back in between.
        app.state.release_bound_device_state();
        app.state.install_track_sounds_into_mirror();

        assert_eq!(
            app.state.pattern.track_params[0].is_polyphonic(),
            !before,
            "the setting survives the repaint"
        );
    }

    /// Regression (16.3 rule 2): a structural arrangement edit made DURING
    /// song playback nulls the mirrored row until the next scheduler notice.
    /// The binding must keep following what the song is sounding through that
    /// window — otherwise every track silently unbinds and the scene
    /// pattern's sound is pushed to the engine mid-playback.
    #[test]
    fn an_arrangement_edit_during_playback_keeps_the_audible_binding() {
        let (mut app, take, _scene_pattern, chunks) = app_with_take();
        // Give the take a sound of its own so an unbind is observable.
        app.state.with_scenes_mut(|scenes| {
            for chunk in &chunks {
                assert!(scenes.track_pools[0].edit(*chunk, |data| {
                    data.instrument_slot.defaults[0] = 0.75;
                }));
            }
        });
        app.song_transport_play(false).expect("song playback starts");
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "row zero plays the take"
        );
        assert_eq!(app.state.pattern.instrument_slots[0].defaults.get(0), 0.75);

        // Any structural arrangement edit during playback goes through
        // `rebuild_active_song_after_arrangement_edit`, which drops the
        // mirrored row.
        app.arr_set_loop(true).expect("loop toggles");
        assert_eq!(app.song_mirrored_row, None);
        app.sync_track_sound_bindings();
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "the take stays bound across the rebuild"
        );
        assert_eq!(
            app.state.pattern.instrument_slots[0].defaults.get(0),
            0.75,
            "the take's device state is not replaced by the scene pattern's"
        );
        app.song_transport_stop().expect("stop succeeds");
    }
}
