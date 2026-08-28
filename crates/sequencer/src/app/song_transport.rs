//! Song-mode transport authority state machine (docs/song-mode-spec.md 7/13).
//!
//! Exactly one launch authority is active at a time: `Stopped`,
//! `SongPlayback`, or `ArrangementCapture` (docs/unified-transport-spec.md). The mode lives
//! on `App` (the control thread orchestrates transport start/stop and mirrors
//! scheduler-authoritative row transitions); Play/Stop/Record from UI, Lisp,
//! and keyboard all route through the methods here rather than toggling the
//! transport atomic directly. Invalid transitions return a clear error and
//! leave the prior state unchanged (spec 13).
//!
//! Performance capture (Slice C, spec 7.4/8/10.3/10.4) plugs in at three
//! seams here: mode entry opens the staging take (`song_transport_play`
//! capture branch), Stop consolidates and commits it atomically through
//! `song_replace` (`song_transport_stop`), and Cancel discards it
//! (`song_capture_cancel`). Audible-launch observation lives in
//! `App::apply_pattern_launch` (src/app/mod.rs); the take itself is in
//! `song_capture.rs`. A notice overflow
//! (`state.song_playback().take_notice_overflow()`) fails the capture and
//! Stop refuses to commit (spec 10.3).

use std::sync::Arc;

use crate::sequencer::{AudibleSongRowApplied, PatternId};

use super::App;

/// The single active launch authority (docs/unified-transport-spec.md 4.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SongTransportMode {
    #[default]
    Stopped,
    SongPlayback,
    ArrangementCapture,
}

impl SongTransportMode {
    /// Reactive-binding string (docs/song-mode-spec.md 12, amended by
    /// docs/unified-transport-spec.md 4.3).
    pub fn binding_str(self) -> &'static str {
        match self {
            SongTransportMode::Stopped => "stopped",
            SongTransportMode::SongPlayback => "song-playback",
            SongTransportMode::ArrangementCapture => "arrangement-capture",
        }
    }
}

/// What an engaged recording writes (docs/unified-transport-spec.md 5):
/// stamped once when recording engages from the view under the performer —
/// arrangement view records a take, the session/Seq view overdubs into the
/// looping pattern clips. Switching views mid-recording never reroutes notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingKind {
    Capture,
    Overdub,
}

impl App {
    fn set_song_transport_mode(&mut self, mode: SongTransportMode) {
        self.song_transport_mode = mode;
        // The recording kind is transport-scoped (unified-transport spec 5):
        // it survives view switches but never a stop.
        if mode == SongTransportMode::Stopped {
            self.recording_kind = None;
        }
    }

    /// Whether the transport is playing from the state machine's viewpoint:
    /// either a mode is active or the raw transport atomic is set (legacy
    /// paths may still start session playback without entering the machine).
    fn transport_engaged(&self) -> bool {
        self.song_transport_mode != SongTransportMode::Stopped || self.state.is_playing()
    }

    /// Manual launches are ALWAYS allowed (takes spec 10, superseding the
    /// song-mode spec 7.3 wall): a manual launch during song playback takes
    /// effect audibly and sets the manual-override latch for its scope —
    /// the song's launch authority is suspended for latched lanes until
    /// Back to Song clears it (see `apply_pattern_launch_at` and
    /// `back_to_song`). The method survives so every historical call site
    /// keeps compiling; it now never rejects.
    pub fn manual_launch_rejection(&self) -> Option<&'static str> {
        None
    }

    /// Whether the committed song is currently the playback authority: song
    /// playback proper, or arrangement capture running on top of it (takes
    /// spec 9.3). Manual launches latch in either mode.
    pub fn song_playback_authority_active(&self) -> bool {
        self.active_runtime_song.is_some()
            && matches!(
                self.song_transport_mode,
                SongTransportMode::SongPlayback | SongTransportMode::ArrangementCapture
            )
    }

    /// Recording engaged while the song is ALREADY playing: promote
    /// `SongPlayback` into `ArrangementCapture` so armed note input records
    /// into takes (takes spec 8) and commits with the splice at Stop.
    ///
    /// Without this, record-arm-then-record during song playback left the
    /// mode at `SongPlayback`, `take_recording_active()` read false, and the
    /// performance fell through to the live-pattern write path — layering an
    /// unrolled arrangement performance into the scene's looping clip.
    ///
    /// The active song start remains the offset from the raw record clock to
    /// the arrangement timeline. Returns whether the promotion happened.
    pub fn promote_song_playback_to_capture(&mut self) -> bool {
        if self.song_transport_mode != SongTransportMode::SongPlayback
            || self.song_capture_take.is_some()
            || self.recording_kind.is_some()
        {
            return false;
        }
        self.begin_song_capture_take(self.active_song_start_beat.unwrap_or(0.0));
        self.set_song_transport_mode(SongTransportMode::ArrangementCapture);
        self.recording_kind = Some(RecordingKind::Capture);
        true
    }

    /// Stamp the recording kind from the view under the performer at the
    /// first armed note of a recording engaged mid-playback
    /// (unified-transport spec 5): the arrangement view promotes into
    /// arrangement capture; the session/Seq view stamps loop overdub. A
    /// recording that already stamped its kind is never re-routed.
    pub fn stamp_recording_kind_for_note(&mut self) {
        if self.recording_kind.is_some() {
            return;
        }
        if self.arrangement_view_visible {
            self.promote_song_playback_to_capture();
        } else if self.state.is_playing() {
            self.recording_kind = Some(RecordingKind::Overdub);
        }
    }

    /// Claim one lane for loop overdub (unified-transport spec 5.1): while
    /// the song is the playback authority, the armed lane is latched so the
    /// pattern being overdubbed is stable across row boundaries and the
    /// layered notes are audible (latched lanes merge the live snapshot per
    /// chunk). Returns false when the lane refuses overdub — a lane whose
    /// row currently plays a take is never silently overwritten.
    pub fn claim_overdub_lane(&mut self, track: usize) -> bool {
        if !self.song_playback_authority_active() {
            return true;
        }
        let bit = 1u64 << track.min(63);
        if self.state.song_manual_latch_mask() & bit == 0 {
            if self.state.song_take_lane_mask() & bit != 0 {
                return false;
            }
            self.state.latch_song_manual_override(std::iter::once(track));
            self.song_row_mirror_epoch += 1;
        }
        // Pin the lane's override to the pattern it is playing — even when
        // the silent-start auto-latch already latched it. The pin makes
        // every masked save-back (row mirror, scene switch, transport stop)
        // a SELF-WRITE, so the live-recorded content persists into the pool
        // instead of being skipped as stale and lost at the stop resync.
        self.state.pin_track_override_to_effective(track);
        true
    }

    /// A track created while the song is the playback authority is invisible
    /// to the preflighted row snapshots (frozen at Play): no row can resolve
    /// content for it, so it would stay silent with a dead playhead until the
    /// next transport start. Latch it like a manual launch (takes spec 10) so
    /// it free-runs the live grid immediately — the lookahead merge appends
    /// lanes the row snapshot doesn't know about — and pin the override so
    /// masked save-backs persist the performer's edits (spec 5.1).
    pub fn latch_track_created_during_song_playback(&mut self, track: usize) {
        if !self.song_playback_authority_active() {
            return;
        }
        self.state.latch_song_manual_override(std::iter::once(track));
        self.state.pin_track_override_to_effective(track);
        self.song_row_mirror_epoch += 1;
    }

    /// Back to Song (takes spec 10): clear the manual-override latch so the
    /// affected lanes snap back to whatever the song resolves at the
    /// current beat with anchored phase. Audible on the next scheduled
    /// chunk; the control-side mirror re-applies the current row here.
    pub fn back_to_song(&mut self) -> Result<String, String> {
        if self.state.song_manual_latch_mask() == 0 && !self.state.song_scene_latch() {
            return Ok("No manual overrides are latched".to_string());
        }
        // Pending quantized manual launches must not fire after the return.
        let _ = self.state.quantized_launches().cancel_all();
        let released_latch = self.state.song_manual_latch_mask();
        self.state.clear_song_manual_latch();
        if !self.song_playback_authority_active() {
            // The latch survives transport stop (Ableton's Back to
            // Arrangement): clearing it while stopped hands the latched
            // lanes' live grid back to the scene so the next Play is fully
            // arrangement-governed.
            self.state.resync_live_grid_to_current_scene();
            // Claim-end reinstall (track-sound spec §2.8): the resync holds
            // track-owned lanes, but a just-released lane's mirror is still
            // the performer's launch — reinstall the owner before the next
            // save-back can persist it into the shared track-sound entities.
            self.state
                .restore_track_sounds_to_mirror_masked(released_latch);
            self.sync_track_sound_bindings();
            return Ok("Back to arrangement: manual overrides cleared".to_string());
        }
        if let Some(song) = self.active_runtime_song.clone() {
            let ordinal = self
                .state
                .song_playback()
                .shared()
                .current_row_ordinal()
                .min(song.rows.len().saturating_sub(1));
            if let Some(row) = song.rows.get(ordinal) {
                self.apply_song_row_control(row.scene, &row.overrides, false, 0)?;
                self.song_mirrored_row = Some(ordinal);
                self.song_row_mirror_epoch += 1;
                // The row apply released any bound device loan and pushed the
                // row's scene-cell devices; re-resolve the bindings in the same
                // step so no lane keeps a stale loaded snapshot for a tick.
                self.sync_track_sound_bindings();
                // Claim-end reinstall (track-sound spec §2.8): a released
                // lane the row resolves nothing for is track-owned again
                // (the binding sync above re-borrowed the audible ones, so
                // the mask spares those) — reinstall the owner so its mirror
                // stops holding the performer's launch.
                self.state
                    .restore_track_sounds_to_mirror_masked(released_latch);
            }
        }
        Ok("Back to song: manual overrides cleared".to_string())
    }

    /// Per-track Back to Song (takes spec 10 UX): clear one lane's
    /// manual-override latch so it snaps back to whatever the song resolves
    /// at the current beat; other latched lanes stay the performer's.
    pub fn back_to_song_track(&mut self, track: usize) -> Result<String, String> {
        if !self.song_playback_authority_active() {
            return Err("Back to Song is only available during song playback".to_string());
        }
        if track >= self.tracks.len() {
            return Err(format!("Track {} does not exist", track + 1));
        }
        if self.state.song_manual_latch_mask() >> track.min(63) & 1 == 0 {
            return Ok(format!("Track {} is not manually overridden", track + 1));
        }
        self.state.clear_song_manual_latch_track(track);
        if let Some(song) = self.active_runtime_song.clone() {
            let ordinal = self
                .state
                .song_playback()
                .shared()
                .current_row_ordinal()
                .min(song.rows.len().saturating_sub(1));
            if let Some(row) = song.rows.get(ordinal) {
                self.apply_song_row_control(row.scene, &row.overrides, false, 0)?;
                self.song_mirrored_row = Some(ordinal);
                self.song_row_mirror_epoch += 1;
                // Same as `back_to_song`: the row apply dropped the loan and
                // pushed the row's devices, so re-resolve the bindings now.
                self.sync_track_sound_bindings();
                // Claim-end reinstall (§2.8), scoped to the one released
                // lane; a no-op when the binding sync re-borrowed it.
                self.state
                    .restore_track_sounds_to_mirror_masked(1u64 << track.min(63));
            }
        }
        Ok(format!("Track {}: back to song", track + 1))
    }

    /// Arm/disarm arrangement capture for the next Play (spec 7.4). Only
    /// meaningful while stopped: capture cannot begin or be armed mid-play.
    pub fn set_song_capture_armed(&mut self, armed: bool) -> Result<(), String> {
        if self.song_capture_armed == armed {
            return Ok(());
        }
        if self.transport_engaged() {
            return Err(
                "Arrangement capture arming can only change while the transport is stopped"
                    .to_string(),
            );
        }
        self.song_capture_armed = armed;
        Ok(())
    }

    /// Play (docs/unified-transport-spec.md 4.1): the arrangement is the only
    /// transport — playback always starts from the arrangement cursor,
    /// open-ended. `record` is the transport record signal at Play time
    /// (pattern/note record toggle or `seq-song-capture-arm`); the active
    /// view picks what it records (spec 5): arrangement view → arrangement
    /// capture, session view → loop overdub. Returns the entered mode.
    pub fn song_transport_play(&mut self, record: bool) -> Result<SongTransportMode, String> {
        if self.transport_engaged() {
            return Err("Transport is already playing".to_string());
        }
        let start_beat = self.arrangement_cursor_beat;
        if record && self.arrangement_view_visible {
            // Arrangement capture: recording always runs ON TOP of song
            // playback (takes spec 9.3, empty-arrangement spec 6) — the
            // arrangement always exists, so there is no bootstrap mode. The
            // song plays (an empty one plays silence) and keeps launch
            // authority wherever the performer hasn't overridden it; manual
            // launches latch (spec 10) and are captured for the [P, Q)
            // splice. Open-ended (spec 7.4): the song end is not a stopping
            // point while recording — grooving past it extends the
            // arrangement rather than cutting the take off.
            self.prepare_song_playback_at(start_beat)?;
            self.begin_song_capture_take(
                self.active_song_start_beat
                    .expect("song start records its normalized beat"),
            );
            self.set_song_transport_mode(SongTransportMode::ArrangementCapture);
            self.recording_kind = Some(RecordingKind::Capture);
            // Auto-latch AFTER the take opens (so the launch is captured as
            // the pass's initial state, spec 4.1) but BEFORE the transport
            // starts — the scheduler fills its first lookahead window the
            // moment the transport flips, and a latch applied after that
            // misses every step-1 event on the first pass.
            self.auto_latch_selected_scene_on_silent_start();
            self.state.start_playback();
            return Ok(SongTransportMode::ArrangementCapture);
        }
        self.prepare_song_playback_at(start_beat)?;
        if record {
            self.recording_kind = Some(RecordingKind::Overdub);
        }
        // Same ordering rule as the capture branch: latch first, then start.
        self.auto_latch_selected_scene_on_silent_start();
        self.state.start_playback();
        Ok(SongTransportMode::SongPlayback)
    }

    /// Auto-latch on a silent start (unified-transport spec 4.1): when the
    /// row governing the Play position is an unscened row that resolves
    /// every lane to silence (empty arrangement, unscened gap, past the
    /// content), fire the currently selected scene as a latched manual
    /// launch — Play always makes the sound the performer is looking at,
    /// and `->SONG` lights to show the transport is overridden. An authored
    /// gap (a SCENED row whose lanes are all explicit-empty) is intentional
    /// silence and is never overridden.
    fn auto_latch_selected_scene_on_silent_start(&mut self) {
        let silent_start = self
            .active_runtime_song
            .as_ref()
            .zip(self.active_song_start_beat)
            .is_some_and(|(song, start_beat)| {
                // Past the arrangement end nothing can be authored: always
                // jam space (the "play after the arrangement" gesture).
                if start_beat >= song.end_beat {
                    return true;
                }
                song.row_index_at_beat(start_beat)
                    .and_then(|ordinal| song.rows.get(ordinal))
                    .is_some_and(|row| {
                        row.scene.is_none()
                            && row.resolved_pattern_ids.iter().all(Option::is_none)
                    })
            });
        if !silent_start {
            return;
        }
        let scene = self.state.current_scene_index();
        // Manual-launch preamble (mirrors `apply_manual_pattern_launch`).
        let _ = self.state.quantized_launches().cancel_all();
        self.scene_macro_runtime.clear();
        let mut touched = self.macro_engine.release_all_scene_macros();
        touched.extend(self.macro_engine.end_scene_push());
        self.send_macro_targets(touched);
        // The transport has NOT started yet (the latch-before-start ordering
        // rule), so the record clock still extrapolates from the PREVIOUS
        // run's anchor — a wall-time-stale read that once stamped this
        // launch tens of beats late, dropping the initial scene from the
        // capture and committing a sliver event near the stop boundary. The
        // capture stamp is the explicit raw start beat 0.
        //
        // Best-effort: a scene that cannot launch (no cells yet) leaves the
        // silent arrangement playing, which is the pre-latch state anyway.
        let _ = self.apply_pattern_launch_at(
            &crate::quantized_launch::PatternLaunchTarget::Scene { scene },
            Some(0.0),
            false,
        );
    }

    /// Prepare song playback at an arrangement-timeline beat: save the live
    /// session, preflight, apply the row governing that beat (with an epoch
    /// bump — the transport is stopped), hand the song and beat to the
    /// scheduler, and enter `SongPlayback`. Deliberately does NOT start the
    /// transport: the caller starts it after any silent-start auto-latch so
    /// the first lookahead window already sees the latched lanes
    /// (unified-transport spec 10). Always open-ended (spec 4.2): reaching
    /// `end_beat` without looping never stops the transport — the last row
    /// keeps sounding past the end and a latched jam is never cut off by an
    /// arrangement the performer is ignoring.
    fn prepare_song_playback_at(&mut self, requested_start_beat: f64) -> Result<(), String> {
        // The arrangement always exists (empty-arrangement spec 4.3); a
        // state that was never seeded installs the empty one, which plays
        // silence.
        if self.state.committed_arrangement().is_none() {
            self.state
                .set_committed_arrangement(Some(self.empty_arrangement()))
                .map_err(|error| format!("Song playback could not start: {error}"))?;
        }
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            return Err(
                "Song playback could not start: the current session state could not be saved"
                    .to_string(),
            );
        }
        let song = self
            .state
            .preflight_runtime_song()
            .map_err(|error| format!("Song playback could not start: {error}"))?;
        let start_beat = song
            .normalize_start_beat(requested_start_beat)
            .map_err(|error| format!("Song playback could not start: {error}"))?;
        let row_ordinal = song
            .row_index_at_beat(start_beat)
            .or_else(|| {
                // Past-end starts are governed by the last row (open-ended
                // jam room, spec 4.2); the silent-start auto-latch then owns
                // what actually sounds.
                (start_beat >= song.end_beat).then(|| song.rows.len().saturating_sub(1))
            })
            .ok_or_else(|| {
                format!("Song playback could not start: no row governs beat {start_beat}")
            })?;
        let Some(row) = song.rows.get(row_ordinal).cloned() else {
            return Err("Song playback could not start: the song has no rows".to_string());
        };
        // The song is the only launch authority from here: drop any pending
        // quantized session launches so none fires mid-song.
        let _ = self.state.quantized_launches().cancel_all();
        self.apply_song_row_control(row.scene, &row.overrides, true, 0)?;
        self.state
            .start_song_playback(Arc::clone(&song), start_beat, true)
            .map_err(|error| format!("Song playback could not start: {error}"))?;
        self.active_runtime_song = Some(song);
        self.active_song_start_beat = Some(start_beat);
        self.song_mirrored_row = Some(row_ordinal);
        self.set_song_transport_mode(SongTransportMode::SongPlayback);
        // The row apply above pushed the row's launch state over the engine.
        // Re-resolve the sound bindings NOW (takes spec 16.2/16.7) instead of
        // waiting for the next reactive tick: an audible bound take must
        // sound its own device snapshot from the first sample, not one frame
        // late — and a non-audible selection stays display-only.
        self.sync_track_sound_bindings();
        Ok(())
    }

    /// Stop, per mode (spec 13). Returns an optional status message.
    pub fn song_transport_stop(&mut self) -> Result<Option<String>, String> {
        match self.song_transport_mode {
            SongTransportMode::Stopped => {
                // Legacy paths can start session playback without entering
                // the machine; stopping the raw transport keeps them working.
                if self.state.is_playing() {
                    self.state.stop_playback();
                }
                Ok(None)
            }
            SongTransportMode::SongPlayback => {
                // One transition, one publication (bead eseq-sj01). Without
                // this scope the arm publishes at least twice — `stop_playback`
                // and then `resync_live_grid_to_current_scene` — and each
                // publication costs a whole-project deep capture on this thread
                // plus a whole-project deep free wherever the last `Arc`
                // reference lands. The deferred publish runs when the scope
                // ends, after the resync's `pattern_epoch` bump and
                // `schedule_mod_resync`, so the scheduler still observes every
                // epoch it needs.
                let state = Arc::clone(&self.state);
                state.coalesce_publishes(|| self.song_playback_stop())
            }
            SongTransportMode::ArrangementCapture => self.arrangement_capture_stop(),
        }
    }

    /// The `SongTransportMode::SongPlayback` arm of [`Self::song_transport_stop`].
    /// Split out so the whole transition can run inside one
    /// `coalesce_publishes` scope (bead eseq-sj01).
    fn song_playback_stop(&mut self) -> Result<Option<String>, String> {
        let teardown = self.state.stop_song_playback();
        self.active_runtime_song = None;
        self.active_song_start_beat = None;
        self.song_mirrored_row = None;
        // Persist the live grid BEFORE the latch clears and the
        // resync below re-launches from the pool: overdub-claimed
        // lanes (override-pinned) self-write their recorded content
        // into the pattern they play; every other stale lane is
        // skipped by the mask, exactly like the row mirror's
        // save-backs. Skipping this save discards live recordings.
        let _ = self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        // The latch SURVIVES the stop (Ableton's Back to Arrangement
        // semantics): pausing and playing again keeps the performer's
        // overrides; only the explicit Back-to-Arrangement gesture
        // (or a capture punch-out) hands the lanes back. The TAKE
        // governance mask does NOT survive — nothing plays a take
        // while stopped, and a stale mask keeps suppressing the clip
        // grid's cell lights and blocks scene launches from claiming
        // the lane (regression the user caught: stopped clip clicks
        // "did nothing" on a lane that had played a take).
        self.state.set_song_take_lane_mask(0);
        self.state.stop_playback();
        self.set_song_transport_mode(SongTransportMode::Stopped);
        // Hand NON-latched lanes back to the scene: the last row
        // played may have silenced lanes it resolved nothing for,
        // and that silencing belongs to the song, not to session
        // mode. Latched lanes keep the performer's live content.
        self.state.resync_live_grid_to_current_scene();
        teardown.map_err(|error| format!("Song playback teardown failed: {error}"))?;
        Ok(Some("Song playback stopped".to_string()))
    }

    /// The `SongTransportMode::ArrangementCapture` arm of
    /// [`Self::song_transport_stop`].
    fn arrangement_capture_stop(&mut self) -> Result<Option<String>, String> {
        // Stop-commit (spec 7.4.7/10.4): the authoritative Stop
        // boundary is the latency-compensated record clock — the same
        // clock every capture event was recorded against (immediate
        // launches, manual clip launches and take notes all stamp
        // `record_beats_at_instant`) — read BEFORE the transport stops
        // (the scheduler rewinds its clock once it observes the
        // stopped transport). Reading the raw scheduler frontier here
        // would place Q one output-latency ahead of the events it
        // bounds; it stays only as the fallback for the case where the
        // audio callback never published a record-clock anchor.
        let end_raw_beats = self
            .state
            .record_beats_at_instant(std::time::Instant::now())
            .unwrap_or_else(|| self.state.scheduler_rendered_beats())
            .max(0.0);
        // Capture-on-playback teardown (takes spec 9.3): the song
        // was playing underneath; the latch auto-clears at
        // punch-out (spec 10) — the committed song now CONTAINS the
        // performance.
        let playback_teardown = if self.active_runtime_song.is_some() {
            self.active_runtime_song = None;
            self.active_song_start_beat = None;
            self.song_mirrored_row = None;
            Some(self.state.stop_song_playback())
        } else {
            None
        };
        self.state.stop_playback();
        // Persist the live grid like the SongPlayback arm does
        // (track-sound spec §2.3): the save is masked, so latched
        // lanes with pins self-write, other stale lanes are skipped,
        // and bare lanes flow their device tweaks into the TRACK
        // SOUND — device edits made while recording must survive the
        // stop. Runs BEFORE the latch clears so the masking still
        // sees latched lanes as stale.
        let _ = self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        // Unlock the song editing primitives before committing: the
        // commit itself goes through `song_replace`.
        self.set_song_transport_mode(SongTransportMode::Stopped);
        let result = self.finish_song_capture_take(end_raw_beats).map(Some);
        // The latch clears only AFTER the commit: the commit's
        // scene-sync snapshot must still see latched lanes as stale
        // (their live grid holds the performer's launch, not the
        // current scene's pattern) or it writes that content over
        // the scene cell's real pattern.
        let released_latch = self.state.song_manual_latch_mask();
        self.state.clear_song_manual_latch();
        // Capture ran on top of song playback, so the same row-owned
        // lane state has to be handed back to the scene.
        if playback_teardown.is_some() {
            self.state.resync_live_grid_to_current_scene();
        }
        // Claim-end reinstall (track-sound spec §2.8): a lane the
        // latch just released keeps the performer's LAUNCH in its
        // mirror — the resync above deliberately holds track-owned
        // lanes (it assumes their mirror is already the track sound).
        // In arrangement context the track owns them again, so put
        // the track sound's device half back NOW; otherwise the next
        // save-back persists the launch's stock state into the
        // shared track-sound entities, retuning every take sharing
        // them. Also makes the audible state honest: the launch is
        // over, the user hears the track sound again.
        self.state
            .restore_track_sounds_to_mirror_masked(released_latch);
        if let Some(Err(error)) = playback_teardown {
            return Err(format!("Song playback teardown failed: {error}"));
        }
        result
    }

    /// Toggle Play/Stop through the state machine (the `seq-toggle-play`
    /// route). Returns a status message when the transition produced one.
    pub fn song_transport_toggle_play(&mut self, record: bool) -> Result<Option<String>, String> {
        if self.transport_engaged() {
            self.song_transport_stop()
        } else {
            self.song_transport_play(record).map(|_| None)
        }
    }

    /// Cancel arrangement capture (spec 13): discard the take, preserve the
    /// committed song, stop the transport. Only valid during capture.
    pub fn song_capture_cancel(&mut self) -> Result<String, String> {
        if self.song_transport_mode != SongTransportMode::ArrangementCapture {
            return Err(
                "seq-song-capture-cancel is only valid during arrangement capture".to_string(),
            );
        }
        self.discard_song_capture_take();
        if self.active_runtime_song.is_some() {
            self.active_runtime_song = None;
            self.active_song_start_beat = None;
            self.song_mirrored_row = None;
            let _ = self.state.stop_song_playback();
        }
        let released_latch = self.state.song_manual_latch_mask();
        self.state.clear_song_manual_latch();
        self.state.stop_playback();
        self.set_song_transport_mode(SongTransportMode::Stopped);
        // Claim-end reinstall (track-sound spec §2.8): the cancel discards
        // the take but the released lanes' mirrors still hold the
        // performer's launches — reinstall the owner so the next save-back
        // cannot persist them into the shared track-sound entities.
        self.state
            .restore_track_sounds_to_mirror_masked(released_latch);
        Ok("Arrangement capture cancelled; take discarded, committed song preserved".to_string())
    }

    /// Control-side mirror of one scheduler-authoritative row transition
    /// (spec 10.2): re-apply the row's scene+overrides to UI-visible state
    /// WITHOUT bumping the pattern epoch (a bump would invalidate the
    /// scheduler's in-flight lookahead window mid-playback).
    pub fn mirror_song_row_applied(
        &mut self,
        notice: &AudibleSongRowApplied,
    ) -> Result<(), String> {
        if !self.song_playback_authority_active() {
            return Ok(());
        }
        // Row-transition save-back seam (takes spec 17.10): the row apply
        // saves the outgoing scene; an engaged macro override must never be
        // what gets persisted into the pool entities.
        self.debug_assert_no_macro_override_leak();
        // Song-loop wrap while recording (takes spec 12): recording across
        // the wrap is disallowed — punch-out is forced at `end_beat` and
        // the pass commits what exists.
        if notice.wrapped && self.song_transport_mode == SongTransportMode::ArrangementCapture {
            self.song_transport_stop()?;
            return Ok(());
        }
        let Some(song) = self.active_runtime_song.clone() else {
            return Ok(());
        };
        let Some(row) = song.rows.get(notice.row_ordinal) else {
            return Err(format!(
                "Song row notice referenced ordinal {} outside the active song",
                notice.row_ordinal
            ));
        };
        // The start flow already applied the row governing its start beat;
        // skip that duplicate initial notice, but always mirror loop wraps.
        if !notice.wrapped && self.song_mirrored_row == Some(notice.row_ordinal) {
            return Ok(());
        }
        // Lanes the incoming row resolves to what is already borrowed stay
        // claimed across the apply (takes spec §17.3): the boundary must not
        // hand the engine the lane owner's sound, or a defaults push, in the
        // window before `sync_track_sound_bindings` re-resolves below.
        let device_hold_mask = self.row_device_hold_mask(row);
        self.apply_song_row_control(row.scene, &row.overrides, false, device_hold_mask)?;
        self.song_mirrored_row = Some(notice.row_ordinal);
        self.song_row_mirror_epoch += 1;
        // The row apply released any bound device loan and pushed the row's
        // launch state (a take lane's SCENE-CELL devices — the row snapshot
        // plays the take's notes, but defaults are pushed control-side).
        // Re-resolve the bindings synchronously so an audible take re-borrows
        // its own device snapshot and re-pushes it in the same mirror step.
        self.sync_track_sound_bindings();
        Ok(())
    }

    /// Scheduler reached `end_beat` with looping disabled (spec 7.3.5): stop
    /// through the state machine. During arrangement capture the stop is
    /// the forced punch-out — the pass commits what exists (takes spec 12).
    pub fn handle_song_playback_ended(&mut self) -> Result<Option<String>, String> {
        match self.song_transport_mode {
            SongTransportMode::SongPlayback => self
                .song_transport_stop()
                .map(|_| Some("Song ended".to_string())),
            SongTransportMode::ArrangementCapture if self.active_runtime_song.is_some() => {
                self.song_transport_stop()
            }
            _ => Ok(None),
        }
    }

    /// Scheduler could not install the song: surface the error and unwind.
    pub fn handle_song_playback_start_failed(&mut self, error: &str) -> String {
        let _ = self.song_transport_stop();
        format!("Song playback failed to start: {error}")
    }

    /// Apply one song row's complete launch state on the control thread:
    /// scene + full override set as one atomic operation, then the same
    /// graph rebinds `apply_pattern_launch` performs (sampler buffers, run
    /// modes, mod routes, restored defaults).
    fn apply_song_row_control(
        &mut self,
        scene: Option<usize>,
        overrides: &[(usize, Option<PatternId>)],
        bump_pattern_epoch: bool,
        device_hold_mask: u64,
    ) -> Result<(), String> {
        // An unscened row (empty-arrangement spec 4.2) recalls no scene: the
        // Seq view stays where it is, and the row's explicit overrides fully
        // describe every lane.
        let scene = scene.unwrap_or_else(|| self.state.current_scene_index());
        // Latched lanes stay the performer's (takes spec 10): the mirror
        // must neither restore their live state nor clear their session
        // override slot.
        let latched_mask = self.state.song_manual_latch_mask();
        // A manual SCENE launch latched the scene identity too: the row's
        // scene must not recall its bus/group fx or move the current scene
        // (scene-keyed reactive bindings audibly follow it) while the
        // performer holds the launch.
        let scene_latched = self.state.song_scene_latch();
        if !scene_latched && scene != self.state.current_scene_index() {
            self.switch_bus_pattern(scene);
        }
        let sample_ids = self.state.apply_song_row_latched(
            scene,
            overrides,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
            bump_pattern_epoch,
            latched_mask,
            scene_latched,
            device_hold_mask,
        )?;
        self.graph_controller().apply_sample_ids(&sample_ids);
        let _ = self
            .graph_controller()
            .sync_track_instrument_run_modes_from_live_state();
        self.graph_controller().sync_current_pattern_mod_routes();
        self.push_all_restored_defaults_except(device_hold_mask);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::app::song_edit::SongRowSpec;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::quantized_launch::PatternLaunchTarget;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternSnapshot, SequencerState, SongPlaybackNotice,
    };

    /// One-track app with three scenes (pool ids 1..=3, scene j holding
    /// PatternId(j + 1)), mirroring `song_edit::tests::test_app`.
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
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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

    /// Three-row song: scenes 0/1/2 at beats 0/4/8, end beat 16.
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

    /// Bead eseq-sj01: one Stop, one publication. Every scheduler-snapshot
    /// publication costs a whole-project deep capture on this thread and a
    /// whole-project deep free wherever the last `Arc` reference lands — which
    /// was the audio callback. The stop arm used to publish at least twice
    /// (`stop_playback`, then `resync_live_grid_to_current_scene`); only the
    /// last one describes the state the user ends up in.
    #[test]
    fn stopping_song_playback_publishes_exactly_one_scheduler_snapshot() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback starts");
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);

        let before = app.state.scheduler_snapshot_version();
        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(
            app.state.scheduler_snapshot_version(),
            before + 1,
            "the whole stop transition must coalesce into one publication"
        );

        // The coalesced publication runs AFTER the resync's epoch bumps, so
        // the scheduler still observes the stopped transport and the new epoch.
        let published = app.state.latest_scheduler_snapshot();
        assert!(!published.transport.playing);
        assert_eq!(
            published.transport.pattern_epoch,
            app.state
                .transport
                .pattern_epoch
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    /// A song row that resolves nothing for a lane silences it, and stopping
    /// in the ARRANGEMENT view HOLDS that lane (track-sound spec §2.2.2/§2.3,
    /// rev 4): the track owns the lane there, so re-launching its cell at Stop
    /// would retune the track to a sound the performer never heard. The clip
    /// stays unlaunched (dot off) until an explicit launch re-installs it.
    /// In Seq view the rev-1 behavior returns — see
    /// `stopping_in_seq_view_resyncs_cells_classically`.
    #[test]
    fn stopping_song_playback_holds_lanes_the_last_row_left_empty() {
        let mut app = app_with_song();
        app.arrangement_view_visible = true;
        app.state.set_arrangement_context(true);
        app.song_transport_play(false).expect("song playback starts");
        app.apply_song_row_control(Some(0), &[(0, None)], false, 0)
            .expect("sparse row applies");
        assert!(
            app.state.is_scene_silenced(0),
            "an explicit-empty lane is silenced while the row plays"
        );

        app.song_transport_stop().expect("stop succeeds");

        assert!(
            app.state.is_scene_silenced(0),
            "the held lane's cell stays unlaunched after the stop"
        );

        // The scene workflow is preserved: an explicit launch re-installs the
        // cell and clears the hold.
        let cell = app
            .state
            .scene_track_pattern_id(app.state.current_scene_index(), 0)
            .expect("the scene still owns a cell on track 0");
        assert!(
            app.state.launch_track_pattern(
                0,
                cell,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[crate::sequencer::InstrumentType::Sampler],
            ),
            "explicit launch"
        );
        assert!(!app.state.is_scene_silenced(0), "launching re-engages the cell");
    }

    /// Track-sound spec §5.3 (rev 4, the other half of the view rule): in SEQ
    /// context the stop resync is classic — the scene's cells reinstall over
    /// the mirror and the lane un-silences, exactly as before rev 2. Nothing
    /// about the transport changed; only the view the user stands in.
    #[test]
    fn stopping_in_seq_view_resyncs_cells_classically() {
        let mut app = app_with_song();
        assert!(!app.arrangement_view_visible, "the Seq tab");
        app.song_transport_play(false).expect("song playback starts");
        app.apply_song_row_control(Some(0), &[(0, None)], false, 0)
            .expect("sparse row applies");
        assert!(app.state.is_scene_silenced(0));

        app.song_transport_stop().expect("stop succeeds");

        assert!(
            !app.state.is_scene_silenced(0),
            "the scene resolves a pattern for track 0, so its clip is launched again"
        );
    }

    /// Track-sound spec §2.3 (symptom 6/8): the stop resync must not
    /// `restore_to` a track-owned lane whose session cells survive — that is
    /// the audible snap, pushing a sound the performer never heard over the
    /// one they did. The mirror is held and the cell stays unlaunched.
    #[test]
    fn stopping_does_not_retune_a_track_owned_lane_whose_cells_survive() {
        use std::sync::Arc;
        let mut app = app_with_song();
        app.arrangement_view_visible = true;
        app.state.set_arrangement_context(true);
        app.song_transport_play(false).expect("song playback starts");
        app.apply_song_row_control(Some(0), &[(0, None)], false, 0)
            .expect("explicit-empty row applies");
        assert!(app.state.is_scene_silenced(0));
        let cell = app
            .state
            .effective_track_pattern_id(0)
            .expect("the session cell survives the empty row");
        // Give the cell a stored sound that differs from what the performer
        // is hearing, so a `restore_to` is observable.
        app.state.with_scenes_mut(|scenes| {
            let refs = scenes.track_pools[0].refs(cell).expect("cell refs");
            let mut mix = (*scenes.track_pools[0].sounds.mixes[&refs.mix]).clone();
            mix.volume = 0.11;
            scenes.track_pools[0]
                .sounds
                .mixes
                .insert(refs.mix, Arc::new(mix));
        });
        app.state.pattern.track_params[0].set_volume(0.77);

        app.song_transport_stop().expect("stop succeeds");

        assert_eq!(
            app.state.pattern.track_params[0].get_volume().to_bits(),
            0.77f32.to_bits(),
            "the held lane keeps the mirror the performer was hearing"
        );
        assert!(
            app.state.is_scene_silenced(0),
            "a held cell stays unlaunched after the stop"
        );
    }

    #[test]
    fn record_in_session_view_stamps_loop_overdub() {
        // Unified-transport spec 5: the session view records loop overdub —
        // song playback mode, no staging take, no song-edit lock; armed
        // notes fall through to the live-pattern write.
        let mut app = app_with_song();
        let mode = app.song_transport_play(true).expect("play succeeds");
        assert_eq!(mode, SongTransportMode::SongPlayback);
        assert_eq!(app.recording_kind, Some(RecordingKind::Overdub));
        assert!(!app.take_recording_active());
        assert!(!app.song_edits_locked());
        assert!(app.song_capture_take.is_none());
        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(
            app.recording_kind, None,
            "the recording kind is transport-scoped"
        );
    }

    #[test]
    fn overdub_claims_the_armed_lane_with_a_latch() {
        // Unified-transport spec 5.1: the first overdubbed note latches its
        // lane so the target pattern is stable across row boundaries and
        // the layered notes are audible.
        let mut app = app_with_song();
        app.song_transport_play(true).expect("overdub playback");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        assert!(app.claim_overdub_lane(0));
        assert_eq!(app.state.song_manual_latch_mask(), 1);
        assert!(
            app.claim_overdub_lane(0),
            "an already-latched lane stays claimable"
        );
        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the overdub claim survives the stop (back-to-arrangement model)"
        );
        app.back_to_song().expect("clear the latch");
    }

    #[test]
    fn stopped_manual_launches_latch_as_overrides() {
        // Unified-transport rev 3 (user-decided): a manual launch is an
        // override of the arrangement EVEN WHILE STOPPED — the gesture
        // always means the same thing and always lights the
        // back-to-arrangement indicator; Play then plays the override.
        let mut app = app_with_song();
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("stopped scene launch");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "a stopped launch latches when an arrangement exists"
        );
        assert!(app.state.song_scene_latch());

        app.song_transport_play(false).expect("play keeps the override");
        assert_eq!(app.state.song_manual_latch_mask(), 1);
        assert_eq!(
            app.state.current_scene_index(),
            1,
            "the scene latch keeps the launched scene as current"
        );
        app.song_transport_stop().expect("stop succeeds");
        app.back_to_song().expect("back to arrangement");
        assert_eq!(app.state.song_manual_latch_mask(), 0);

        // Empty arrangements latch too (user-decided: one gesture, one
        // meaning — the indicator being lit on fresh projects is fine).
        let mut fresh = app_with_song();
        fresh.arr_clear().expect("empty the arrangement");
        fresh
            .apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("stopped scene launch on the empty arrangement");
        assert_eq!(
            fresh.state.song_manual_latch_mask(),
            1,
            "the launch gesture always latches"
        );
    }

    #[test]
    fn latch_survives_stop_and_the_next_play() {
        // Ableton's Back to Arrangement: pausing and playing again keeps
        // the performer's overrides; only the explicit gesture clears them.
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("manual launch latches");
        assert_eq!(app.state.song_manual_latch_mask(), 1);
        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(app.state.song_manual_latch_mask(), 1, "latch survives stop");

        app.song_transport_play(false).expect("play again");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the restarted transport keeps the performer's overrides"
        );
        app.song_transport_stop().expect("stop succeeds");
        app.back_to_song().expect("back to arrangement while stopped");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        assert!(!app.state.song_scene_latch(), "the scene latch clears too");
    }

    #[test]
    fn track_created_during_song_playback_latches_its_lane() {
        // A track added mid-play is unknown to the preflighted row
        // snapshots: creation latches it like a manual launch so it
        // free-runs the live grid instead of staying silent until the
        // next transport start.
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        app.latch_track_created_during_song_playback(1);
        assert_eq!(app.state.song_manual_latch_mask(), 1 << 1);
        app.song_transport_stop().expect("stop succeeds");
        app.back_to_song().expect("clear the latch");

        // While stopped, track creation must not latch anything.
        app.latch_track_created_during_song_playback(1);
        assert_eq!(app.state.song_manual_latch_mask(), 0);
    }

    #[test]
    fn play_enters_song_playback() {
        let mut app = app_with_song();
        let mode = app.song_transport_play(false).expect("play succeeds");
        assert_eq!(mode, SongTransportMode::SongPlayback);
        assert!(app.state.is_playing());
        assert!(
            !app.song_edits_locked(),
            "ordinary song editing stays available during playback"
        );
        assert!(app.active_runtime_song.is_some());
        assert_eq!(app.song_transport_mode.binding_str(), "song-playback");
        // Row zero was applied as the launch state.
        assert_eq!(app.state.current_scene_index(), 0);

        let status = app.song_transport_stop().expect("stop succeeds");
        assert_eq!(status.as_deref(), Some("Song playback stopped"));
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert!(!app.song_edits_locked());
        assert!(app.active_runtime_song.is_none());
    }

    #[test]
    fn arrangement_cursor_starts_playback_and_capture_at_that_song_beat() {
        let mut playback = app_with_song();
        playback.set_arrangement_cursor(9.0, 0);
        playback
            .song_transport_play(false)
            .expect("mid-song playback starts");
        assert_eq!(playback.active_song_start_beat, Some(9.0));
        assert_eq!(
            playback.state.current_scene_index(),
            2,
            "the row governing the cursor must be applied before playback"
        );
        let commands = playback.state.song_playback().drain_commands();
        assert!(commands.iter().any(|command| matches!(
            command,
            crate::sequencer::SongPlaybackCommand::Start { start_beat, .. }
                if *start_beat == 9.0
        )));
        playback.song_transport_stop().expect("playback stops");
        assert_eq!(playback.active_song_start_beat, None);

        let mut capture = app_with_song();
        capture.set_arrangement_view_visible(true);
        capture.set_arrangement_cursor(9.0, 0);
        capture
            .song_transport_play(true)
            .expect("mid-song capture starts");
        capture.record_song_capture_launch(
            &PatternLaunchTarget::Scene { scene: 1 },
            1.5,
        );
        let take = capture.song_capture_take.as_ref().expect("capture take");
        assert_eq!(take.timeline_start_beat(), 9.0);
        assert_eq!(
            take.events()[0].beat,
            10.5,
            "scheduler beat zero must map to the selected arrangement beat"
        );
        capture.state.set_scheduler_rendered_beats(2.0);
        capture
            .song_transport_stop()
            .expect("mid-song capture commits");
        let arrangement = capture
            .state
            .committed_arrangement()
            .expect("committed arrangement");
        assert_eq!(arrangement.scene_at_beat(10.75), Some(1));
        assert_eq!(
            arrangement.scene_at_beat(11.25),
            Some(2),
            "the pre-existing arrangement resumes at the translated stop beat"
        );
        assert_eq!(capture.active_song_start_beat, None);
        capture.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn play_on_an_empty_arrangement_auto_latches_the_selected_scene() {
        // Unified-transport spec 4.1: the arrangement always exists and Play
        // always starts it — and on a SILENT start (the empty arrangement
        // resolves every lane to silence) the currently selected scene is
        // fired as a latched manual launch, so Play always makes the sound
        // the performer is looking at.
        let mut app = test_app();
        let mode = app.song_transport_play(false).expect("empty song plays");
        assert_eq!(mode, SongTransportMode::SongPlayback);
        assert!(app.state.is_playing());
        let arrangement = app
            .state
            .committed_arrangement()
            .expect("play installed the empty arrangement");
        assert!(arrangement.is_empty());
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the silent start latched the selected scene over the song"
        );
        assert!(
            !app.state.is_scene_silenced(0),
            "the latched scene's clip is audible, not the silent row"
        );
        app.song_transport_stop().expect("stops cleanly");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the auto-latch survives the stop like any manual latch"
        );
        app.back_to_song().expect("clear the latch");
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn overdubbed_steps_survive_transport_stop() {
        // The blank-project repro: Seq view, arm a track, play+record, tap
        // some trigs, stop — the steps must NOT vanish. Overdub writes only
        // the live grid; the stop resync re-launches the scene from the
        // pool, so the claim's override pin + the stop save-back are what
        // carry the recording into the pool pattern.
        let mut app = test_app();
        app.song_transport_play(true).expect("overdub playback");
        assert_eq!(app.recording_kind, Some(RecordingKind::Overdub));
        assert!(app.claim_overdub_lane(0), "the armed lane claims");
        // The live keyboard write the input path performs on a recorded note.
        app.state.pattern.patterns[0].toggle_step(3);
        assert!(app.state.pattern.patterns[0].is_active(3));

        app.song_transport_stop().expect("stop succeeds");

        assert!(
            app.state.pattern.patterns[0].is_active(3),
            "the recorded step survives the stop resync"
        );
        // And it reached the pool: clearing the latch re-launches the lane
        // from the scene, which must still hold the recording.
        app.back_to_song().expect("back to arrangement");
        assert!(
            app.state.pattern.patterns[0].is_active(3),
            "the recording persisted into the scene's pool pattern"
        );
    }

    #[test]
    fn play_past_the_arrangement_end_is_jam_space() {
        // Unified-transport spec 4.2: the cursor may park past `end_beat`
        // and Play must start there (open-ended), auto-latching the
        // selected scene — the "play after the arrangement like hardware"
        // gesture. This used to be rejected by `normalize_start_beat`.
        let mut app = app_with_song();
        app.set_arrangement_cursor(24.0, 0);
        app.song_transport_play(false).expect("past-end play starts");
        assert!(app.state.is_playing());
        assert_eq!(app.active_song_start_beat, Some(24.0));
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "a past-end start is a silent start: the selected scene latches"
        );
        app.song_transport_stop().expect("stop succeeds");
    }

    #[test]
    fn auto_latch_capture_stamp_ignores_the_stale_record_clock() {
        // The record clock extrapolates from the PREVIOUS run's anchor and
        // grows with wall time while stopped. The auto-latch fires BEFORE
        // the transport starts, so reading the clock stamped the initial
        // scene tens of beats late — the capture committed with no scene at
        // beat 0 and a sliver event near the stop boundary. The stamp must
        // be the explicit raw start beat 0.
        let mut app = test_app();
        // A stale anchor from a "previous run", far in the past wall-time.
        let now = std::time::Instant::now();
        app.state.transport.record_clock.publish(0.0, now);
        app.state.transport.record_clock.publish(
            40.0,
            now.checked_add(std::time::Duration::from_millis(1))
                .expect("anchor instant"),
        );
        app.set_arrangement_view_visible(true);
        app.song_transport_play(true).expect("capture starts");
        let take = app.song_capture_take.as_ref().expect("capture take");
        assert_eq!(take.event_count(), 1, "the auto-latch launch is captured");
        assert!(
            take.events()[0].beat.abs() < 1e-9,
            "the initial scene is stamped at beat 0, not at the stale \
             clock read (got {})",
            take.events()[0].beat
        );
        let _ = app.song_capture_cancel();
    }

    #[test]
    fn play_on_arranged_content_never_auto_latches() {
        // The auto-latch is for SILENT starts only (unified-transport spec
        // 4.1): a start on a row that resolves content plays the
        // arrangement untouched.
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        app.song_transport_stop().expect("stop succeeds");
    }

    #[test]
    fn play_with_arrangement_and_record_enters_capture_on_top_of_playback() {
        let mut app = app_with_song();
        app.set_arrangement_view_visible(true);
        let song_before = app.state.committed_song();
        let mode = app.song_transport_play(true).expect("capture starts");
        assert_eq!(mode, SongTransportMode::ArrangementCapture);
        assert!(app.state.is_playing());
        assert!(app.song_edits_locked(), "song edits lock during capture");
        assert!(
            app.active_runtime_song.is_some(),
            "capture runs ON TOP of song playback (takes spec 9.3)"
        );
        assert_eq!(app.song_transport_mode.binding_str(), "arrangement-capture");
        assert!(
            app.manual_launch_rejection().is_none(),
            "the performer stays launch authority during capture"
        );
        assert!(
            app.song_capture_take.is_some(),
            "capture opens the staging take"
        );
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "the committed song is untouched while capture runs"
        );

        // Stop at rendered beat 8: with no launches performed there is no
        // splice — the committed song is untouched and no undo entry is
        // created (takes spec 9.1/9.5).
        app.state.set_scheduler_rendered_beats(8.0);
        let status = app.song_transport_stop().expect("stop resolves the take");
        assert!(
            status.unwrap().contains("unchanged"),
            "stop reports the launch-free no-op"
        );
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert!(app.song_capture_take.is_none());
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "a launch-free capture never rewrites the committed song"
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn recording_mid_song_playback_promotes_into_arrangement_capture() {
        // Arming a track and engaging record while the song is ALREADY
        // playing must record into a take, not into the scene's looping
        // pattern — so the note path promotes the mode first.
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        assert!(!app.take_recording_active());
        assert!(app.promote_song_playback_to_capture(), "promotion happens");
        assert_eq!(app.song_transport_mode, SongTransportMode::ArrangementCapture);
        assert_eq!(app.recording_kind, Some(RecordingKind::Capture));
        assert!(app.song_capture_take.is_some());
        assert!(
            app.take_recording_active(),
            "armed note input now retargets into takes"
        );
        assert!(
            !app.promote_song_playback_to_capture(),
            "promotion is idempotent"
        );
        // Stop runs the capture stop-commit (nothing performed here, so the
        // committed song is unchanged) and leaves no capture behind.
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop resolves the capture");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(app.song_capture_take.is_none());
        assert!(!app.promote_song_playback_to_capture(), "no-op when stopped");
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn overdub_recording_never_promotes_into_capture() {
        // The kind is stamped for the whole recording (unified-transport
        // spec 5): an overdub engaged in the session view never reroutes
        // into a take, even via the promotion path.
        let mut app = app_with_song();
        app.song_transport_play(true).expect("overdub playback");
        assert_eq!(app.recording_kind, Some(RecordingKind::Overdub));
        assert!(!app.promote_song_playback_to_capture());
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);
        assert!(!app.take_recording_active());
        app.song_transport_stop().unwrap();
    }

    #[test]
    fn note_stamp_routes_by_the_active_view() {
        // stamp_recording_kind_for_note (unified-transport spec 5): the view
        // under the performer at the first armed note picks the kind.
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        app.set_arrangement_view_visible(true);
        app.stamp_recording_kind_for_note();
        assert_eq!(app.recording_kind, Some(RecordingKind::Capture));
        assert_eq!(app.song_transport_mode, SongTransportMode::ArrangementCapture);
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("capture stop-commits");
        app.state.set_scheduler_rendered_beats(0.0);

        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        app.stamp_recording_kind_for_note();
        assert_eq!(app.recording_kind, Some(RecordingKind::Overdub));
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);
        app.song_transport_stop().unwrap();
    }

    #[test]
    fn play_while_playing_is_rejected_in_every_mode() {
        let mut app = app_with_song();
        for (arrangement_view, record) in [(false, false), (false, true), (true, true)] {
            app.set_arrangement_view_visible(arrangement_view);
            let mode = app.song_transport_play(record).expect("play succeeds");
            let error = app
                .song_transport_play(record)
                .expect_err("second play must fail");
            assert!(error.contains("already playing"), "{error}");
            assert_eq!(app.song_transport_mode, mode, "mode unchanged");
            // A zero-length capture take fails commit validation on Stop;
            // the transport still stops (spec 13).
            let _ = app.song_transport_stop();
            assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        }
    }

    #[test]
    fn manual_launches_latch_during_song_playback_and_back_to_song_clears() {
        let mut app = app_with_song();
        assert!(app.manual_launch_rejection().is_none());
        app.song_transport_play(false).expect("song playback");
        // Takes spec 10 (supersedes song-mode spec 7.3): a manual launch is
        // audible and latches its scope instead of being rejected.
        assert!(app.manual_launch_rejection().is_none());
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("manual launch during song playback latches");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "a scene launch latches every track (one-track app)"
        );
        // Back to Song clears the latch and re-applies the current row.
        let status = app.back_to_song().expect("back to song");
        assert!(status.contains("Back to song"), "{status}");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        // A track launch latches only its track.
        app.apply_manual_pattern_launch(&PatternLaunchTarget::SceneTracks {
            scene: 1,
            tracks: vec![0],
        })
        .expect("track launch latches its track");
        assert_eq!(app.state.song_manual_latch_mask(), 1);
        // The latch SURVIVES the stop (Ableton back-to-arrangement): only
        // the explicit gesture clears it, and it works while stopped too.
        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "stopping keeps the performer's overrides latched"
        );
        let status = app.back_to_song().expect("clear latch while stopped");
        assert!(status.contains("Back to arrangement"), "{status}");
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        assert!(app.manual_launch_rejection().is_none());
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("manual launch works after stop");
    }

    /// Clobber regression (variant: latched scene launch): a manual scene
    /// launch during song playback wipes the lane's override pointer, so the
    /// live grid holds the performer's pattern while `current_scene` keeps
    /// advancing with the row mirror. The mirror's save-back must skip that
    /// lane — an unmasked save writes the performer's pattern data over
    /// whatever scene cell the previous row left current.
    #[test]
    fn latched_scene_launch_never_clobbers_other_scene_patterns() {
        use crate::sequencer::{PatternId, PatternSnapshot};
        let mut app = app_with_song();
        let mut snapshots = vec![
            PatternSnapshot::new_default(1, &[]),
            PatternSnapshot::new_default(1, &[]),
            PatternSnapshot::new_default(1, &[]),
        ];
        snapshots[0].track_bits[0][0] = 0b1;
        snapshots[1].track_bits[0][0] = 0b11;
        snapshots[2].track_bits[0][0] = 0b111;
        app.state.replace_pattern_repository(snapshots, 0);
        app.state.resync_live_grid_to_current_scene();
        app.song_transport_play(false).expect("song playback");
        let song = app.active_runtime_song.clone().expect("active song");

        // Performer launches scene 3 (pattern 3) — the lane latches and its
        // live grid now holds pattern 3's content with no override pinned.
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 2 })
            .expect("scene launch latches");
        assert_eq!(app.state.song_manual_latch_mask(), 1);

        // Two row transitions: the first save-back targets the launched
        // scene (self-write), the second targets row 1's scene — the one an
        // unmasked save clobbers with pattern 3's data.
        for (ordinal, beat) in [(1usize, 4.0f64), (2, 8.0)] {
            app.mirror_song_row_applied(&AudibleSongRowApplied {
                row_id: song.rows[ordinal].id,
                row_ordinal: ordinal,
                effective_beat: beat,
                effective_sample: (beat * 44_100.0) as u64,
                wrapped: false,
            })
            .expect("row mirror");
        }

        app.state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[0].get(PatternId(1)).unwrap().track_bits[0],
                0b1,
                "pattern 1 must keep its own steps"
            );
            assert_eq!(
                scenes.track_pools[0].get(PatternId(2)).unwrap().track_bits[0],
                0b11,
                "pattern 2 must keep its own steps"
            );
        });
        app.song_transport_stop().unwrap();
    }

    /// Clobber regression (variant: gap lanes, no performer gesture): an
    /// explicit-empty row override silences the lane but deliberately keeps
    /// the previous content in the live grid. Two rows later that stale
    /// content sits under a different `current_scene`; the mirror's
    /// save-back must skip silenced lanes or it writes the stale pattern
    /// over that scene's real cell.
    #[test]
    fn gap_silenced_lane_never_saves_stale_content_over_other_scenes() {
        use crate::sequencer::{PatternId, PatternSnapshot, ProjectSongTrackOverride};
        let mut app = test_app();
        let mut snapshots = vec![
            PatternSnapshot::new_default(1, &[]),
            PatternSnapshot::new_default(1, &[]),
            PatternSnapshot::new_default(1, &[]),
        ];
        snapshots[0].track_bits[0][0] = 0b1;
        snapshots[1].track_bits[0][0] = 0b11;
        snapshots[2].track_bits[0][0] = 0b111;
        app.state.replace_pattern_repository(snapshots, 0);
        app.state.resync_live_grid_to_current_scene();
        let gap = |start_beat: f64, scene: usize| SongRowSpec {
            start_beat,
            scene,
            overrides: vec![ProjectSongTrackOverride::new(0, None)],
        };
        app.arr_replace_rows(
            vec![
                gap(0.0, 0),
                gap(4.0, 1),
                SongRowSpec {
                    start_beat: 8.0,
                    scene: 2,
                    overrides: Vec::new(),
                },
            ],
            16.0,
            false,
        )
        .expect("song committed");
        app.song_transport_play(false).expect("song playback");
        let song = app.active_runtime_song.clone().expect("active song");

        // Row 0 (gap over scene 1's cell) applied at start; the lane is
        // silenced with pattern 1's content left live. Rows 1 and 2 then
        // advance current_scene under that stale content.
        assert!(app.state.is_scene_silenced(0), "gap row silences the lane");
        for (ordinal, beat) in [(1usize, 4.0f64), (2, 8.0)] {
            app.mirror_song_row_applied(&AudibleSongRowApplied {
                row_id: song.rows[ordinal].id,
                row_ordinal: ordinal,
                effective_beat: beat,
                effective_sample: (beat * 44_100.0) as u64,
                wrapped: false,
            })
            .expect("row mirror");
        }

        app.state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[0].get(PatternId(2)).unwrap().track_bits[0],
                0b11,
                "scene 2's pattern must keep its own steps"
            );
        });
        app.song_transport_stop().unwrap();
    }

    #[test]
    fn per_track_back_to_song_clears_only_that_lane() {
        let mut app = test_app_two_tracks();
        app.arr_replace_rows(
            vec![SongRowSpec {
                start_beat: 0.0,
                scene: 0,
                overrides: Vec::new(),
            }],
            16.0,
            false,
        )
        .expect("song committed");
        assert!(app
            .back_to_song_track(0)
            .is_err(), "only valid during song playback");
        app.song_transport_play(false).expect("song playback");
        app.apply_manual_pattern_launch(&PatternLaunchTarget::SceneTracks {
            scene: 1,
            tracks: vec![0, 1],
        })
        .expect("both lanes latch");
        assert_eq!(app.state.song_manual_latch_mask(), 0b11);
        let status = app.back_to_song_track(0).expect("track 0 returns");
        assert!(status.contains("back to song"), "{status}");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            0b10,
            "track 1 stays the performer's"
        );
        let status = app.back_to_song_track(0).expect("idempotent");
        assert!(status.contains("not manually overridden"), "{status}");
        app.song_transport_stop().expect("stop succeeds");
    }

    #[test]
    fn capture_cancel_is_only_valid_during_capture() {
        let mut app = app_with_song();
        let error = app
            .song_capture_cancel()
            .expect_err("cancel while stopped must fail");
        assert!(error.contains("only valid during arrangement capture"), "{error}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);

        app.song_transport_play(false).expect("song playback");
        let error = app
            .song_capture_cancel()
            .expect_err("cancel during song playback must fail");
        assert!(error.contains("only valid during arrangement capture"), "{error}");
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);
        app.song_transport_stop().unwrap();

        let song_before = app.state.committed_song();
        app.set_arrangement_view_visible(true);
        app.song_transport_play(true).expect("capture starts");
        // Record something into the take: cancel must still discard it all.
        app.state.set_scheduler_rendered_beats(2.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("launch during capture succeeds");
        assert_eq!(
            app.song_capture_take.as_ref().map(|take| take.event_count()),
            Some(1)
        );
        let status = app.song_capture_cancel().expect("cancel succeeds");
        assert!(status.contains("take discarded"), "{status}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert!(app.song_capture_take.is_none(), "cancel discards the take");
        assert!(!app.song_capture_failed, "cancel is not a capture failure");
        assert_eq!(app.state.committed_song(), song_before);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_arm_only_changes_while_stopped() {
        let mut app = app_with_song();
        app.set_song_capture_armed(true).expect("arm while stopped");
        assert!(app.song_capture_armed);
        app.song_transport_play(false).expect("play");
        let error = app
            .set_song_capture_armed(false)
            .expect_err("arm change while playing must fail");
        assert!(error.contains("stopped"), "{error}");
        assert!(app.song_capture_armed);
        app.song_transport_stop().unwrap();
        app.set_song_capture_armed(false).expect("disarm while stopped");
    }

    #[test]
    fn toggle_play_routes_play_and_stop_through_the_machine() {
        let mut app = app_with_song();
        app.song_transport_toggle_play(false).expect("toggle starts");
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);
        app.song_transport_toggle_play(false).expect("toggle stops");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
    }

    #[test]
    fn song_edit_primitives_are_available_in_playback_but_locked_during_capture() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("play succeeds");
        app.arr_set_loop(true)
            .expect("song playback allows arrangement edits");
        let _ = app.song_transport_stop();

        app.set_arrangement_view_visible(true);
        app.song_transport_play(true).expect("capture starts");
        let error = app
            .arr_set_loop(false)
            .expect_err("capture owns the arrangement splice");
        assert_eq!(
            error,
            "song editing is unavailable during arrangement capture"
        );
        let _ = app.song_transport_stop();
        app.arr_set_loop(false)
            .expect("editing unlocks after capture stops");
    }

    #[test]
    fn mirror_applies_later_rows_and_skips_the_duplicate_initial_row() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        assert_eq!(app.state.current_scene_index(), 0);
        let song = app.active_runtime_song.clone().expect("active song");

        // Duplicate initial row-zero notice: skipped (already applied with
        // the start-time epoch bump).
        let epoch_before = app
            .state
            .transport
            .pattern_epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        app.mirror_song_row_applied(&AudibleSongRowApplied {
            row_id: song.rows[0].id,
            row_ordinal: 0,
            effective_beat: 0.0,
            effective_sample: 0,
            wrapped: false,
        })
        .expect("mirror succeeds");
        assert_eq!(app.state.current_scene_index(), 0);

        // Row 1 becomes the UI-visible scene without an epoch bump.
        app.mirror_song_row_applied(&AudibleSongRowApplied {
            row_id: song.rows[1].id,
            row_ordinal: 1,
            effective_beat: 4.0,
            effective_sample: 44_100,
            wrapped: false,
        })
        .expect("mirror succeeds");
        assert_eq!(app.state.current_scene_index(), 1);
        let epoch_after = app
            .state
            .transport
            .pattern_epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            epoch_before, epoch_after,
            "control-side mirrors must never bump the pattern epoch"
        );
        app.song_transport_stop().unwrap();
    }

    #[test]
    fn scheduler_end_notice_stops_through_the_machine() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        let status = app
            .handle_song_playback_ended()
            .expect("ended handling succeeds");
        assert_eq!(status.as_deref(), Some("Song ended"));
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert!(app.active_runtime_song.is_none());
    }

    #[test]
    fn start_failed_notice_unwinds_to_stopped() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        let message = app.handle_song_playback_start_failed("boom");
        assert!(message.contains("boom"), "{message}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
    }

    #[test]
    fn song_start_flow_sends_the_scheduler_start_command() {
        let mut app = app_with_song();
        app.song_transport_play(false).expect("song playback");
        let commands = app.state.song_playback().drain_commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(
                    command,
                    crate::sequencer::SongPlaybackCommand::Start { start_beat, .. }
                        if *start_beat == 0.0
                )),
            "start flow must hand the preflighted song to the scheduler at beat zero"
        );
        app.song_transport_stop().unwrap();
        let commands = app.state.song_playback().drain_commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, crate::sequencer::SongPlaybackCommand::Stop)),
            "stop must tear down scheduler-side song playback"
        );
        // The mailbox notice path stays usable after the round trip.
        app.state
            .song_playback()
            .push_notice(SongPlaybackNotice::Ended {
                end_beat: 16.0,
                end_sample: 0,
            });
        assert_eq!(app.state.drain_song_playback_notices().len(), 1);
    }

    // ---------------------------------------------------------------
    // Note edit-through (docs/realtime-arrangement-feedback-spec.md 5)
    // ---------------------------------------------------------------

    /// The same three-row song, but every row explicitly empties track 0, so
    /// no row resolves the pattern a step edit lands on.
    fn app_with_song_resolving_no_pattern() -> App {
        let mut app = test_app();
        let empty_row = |start_beat: f64, scene: usize| SongRowSpec {
            start_beat,
            scene,
            overrides: vec![crate::sequencer::ProjectSongTrackOverride::new(0, None)],
        };
        app.arr_replace_rows(
            vec![empty_row(0.0, 0), empty_row(4.0, 1), empty_row(8.0, 2)],
            16.0,
            false,
        )
        .expect("arr_replace_rows succeeds");
        app
    }

    fn playing_song_app(mut app: App) -> App {
        app.song_transport_play(false).expect("song playback starts");
        // Drop the Start command so later drains see only edit-through work.
        app.state.song_playback().drain_commands();
        app
    }

    /// The `Refresh` songs queued for the scheduler since the last drain.
    fn drained_refreshes(app: &App) -> Vec<std::sync::Arc<crate::sequencer::RuntimeSong>> {
        app.state
            .song_playback()
            .drain_commands()
            .into_iter()
            .filter_map(|command| match command {
                crate::sequencer::SongPlaybackCommand::Refresh { song } => Some(song),
                _ => None,
            })
            .collect()
    }

    fn drained_rebuilds(app: &App) -> Vec<std::sync::Arc<crate::sequencer::RuntimeSong>> {
        app.state
            .song_playback()
            .drain_commands()
            .into_iter()
            .filter_map(|command| match command {
                crate::sequencer::SongPlaybackCommand::Rebuild { song } => Some(song),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn arrangement_edit_and_undo_during_playback_each_send_one_rebuild() {
        let mut app = playing_song_app(app_with_song());
        let clip = app
            .state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[0]
            .iter()
            .find(|clip| (clip.start_beat - 8.0).abs() < 1e-9)
            .expect("clip ahead of the playhead")
            .id;
        let depth = app.history.undo_len();

        app.arr_clip_move(clip, 10.0)
            .expect("clip edits are allowed during song playback");
        assert_eq!(
            app.history.undo_len(),
            depth + 1,
            "one edit remains one undo entry"
        );
        assert_eq!(
            drained_rebuilds(&app).len(),
            1,
            "the structural commit sends exactly one Rebuild"
        );
        assert_eq!(
            app.song_mirrored_row,
            None,
            "a remapped ordinal cannot suppress the next real row notice"
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            drained_rebuilds(&app).len(),
            1,
            "history replay rides the same Rebuild seam"
        );
        app.song_transport_stop().unwrap();
    }

    /// Slice 3's whole premise: the refreshed rows must be accepted by the
    /// scheduler's cheap content path, which only swaps when row layout is
    /// identical.
    fn assert_swaps_in_place(
        before: &std::sync::Arc<crate::sequencer::RuntimeSong>,
        refreshed: &std::sync::Arc<crate::sequencer::RuntimeSong>,
    ) {
        let mut runtime = crate::sequencer::SongPlaybackRuntime::new(
            std::sync::Arc::clone(before),
            0.0,
            1.0,
        )
        .expect("song playback runtime");
        assert!(
            runtime.replace_song_in_place(std::sync::Arc::clone(refreshed)),
            "a note/geometry edit must not move row layout: the Refresh has to swap in place"
        );
    }

    fn toggle_step(app: &mut App, step: usize) {
        crate::app::edit::try_apply_command(app, crate::app::AppCommand::ToggleStep { track: 0, step })
            .expect("step edit applies");
    }

    /// 5.1: a step commit on a pattern the playing song resolves ends with a
    /// `Refresh` whose swap succeeds — and it does that WITHOUT touching the
    /// committed song (5.2: the dots ride pool content, not the song).
    #[test]
    fn a_step_edit_during_song_playback_refreshes_the_scheduler_rows() {
        let mut app = playing_song_app(app_with_song());
        let before = app.active_runtime_song.clone().expect("active song");
        let song_revision = app.state.committed_song_revision();
        let pool_revision = app.state.pool_content_revision();

        toggle_step(&mut app, 3);

        let refreshes = drained_refreshes(&app);
        assert_eq!(refreshes.len(), 1, "one commit, one refresh");
        assert_swaps_in_place(&before, &refreshes[0]);
        assert!(
            app.active_runtime_song
                .as_ref()
                .is_some_and(|song| std::sync::Arc::ptr_eq(song, &refreshes[0])),
            "the control side keeps the rows it handed the scheduler"
        );
        assert!(
            app.state.pool_content_revision() > pool_revision,
            "the lane dots key off pool content"
        );
        assert_eq!(
            app.state.committed_song_revision(),
            song_revision,
            "a note edit is not a song edit"
        );
        app.song_transport_stop().unwrap();
    }

    /// 5.1: a `PatternGeometry` length change keeps row layout identical too —
    /// the song's beat math comes from the arrangement, not the pattern
    /// length — so it rides the same `Refresh`, not a rebuild.
    #[test]
    fn a_pattern_length_change_during_song_playback_still_swaps_via_refresh() {
        let mut app = playing_song_app(app_with_song());
        let before = app.active_runtime_song.clone().expect("active song");
        let pool_revision = app.state.pool_content_revision();

        crate::app::edit::try_apply_command(
            &mut app,
            crate::app::AppCommand::SetTrackNumSteps { track: 0, n: 7 },
        )
        .expect("length change applies");

        let refreshes = drained_refreshes(&app);
        assert_eq!(refreshes.len(), 1, "one geometry commit, one refresh");
        assert_swaps_in_place(&before, &refreshes[0]);
        assert!(app.state.pool_content_revision() > pool_revision);
        app.song_transport_stop().unwrap();
    }

    /// 5.1: the `affected` check is what keeps this cheap — an edit to a
    /// pattern no row resolves must not preflight at all.
    #[test]
    fn a_step_edit_no_row_resolves_never_preflights() {
        let mut app = playing_song_app(app_with_song_resolving_no_pattern());
        assert!(
            app.active_runtime_song
                .as_ref()
                .expect("active song")
                .rows
                .iter()
                .all(|row| row.resolved_pattern_ids[0].is_none()),
            "fixture must leave track 0 unresolved on every row"
        );
        let pool_revision = app.state.pool_content_revision();

        toggle_step(&mut app, 3);

        assert!(
            drained_refreshes(&app).is_empty(),
            "no row uses this pattern, so nothing is re-preflighted"
        );
        assert!(
            app.state.pool_content_revision() > pool_revision,
            "the pool still changed: the dots refresh even when the song ignores it"
        );
        app.song_transport_stop().unwrap();
    }

    /// 5.1: undo and redo ride the same seam, so a step edit reverted during
    /// playback un-sounds where it was heard — and moves the dots back.
    #[test]
    fn undo_and_redo_of_a_step_edit_refresh_the_rows_and_the_dots() {
        let mut app = playing_song_app(app_with_song());
        let before = app.active_runtime_song.clone().expect("active song");
        toggle_step(&mut app, 3);
        app.state.song_playback().drain_commands();

        for replay in ["undo", "redo"] {
            let pool_revision = app.state.pool_content_revision();
            let outcome = match replay {
                "undo" => crate::app::edit::undo(&mut app),
                _ => crate::app::edit::redo(&mut app),
            };
            assert!(
                matches!(outcome, crate::app::history::HistoryReplay::Applied(_)),
                "{replay} applies: {outcome:?}"
            );
            let refreshes = drained_refreshes(&app);
            assert_eq!(refreshes.len(), 1, "{replay} refreshes the rows once");
            assert_swaps_in_place(&before, &refreshes[0]);
            assert!(
                app.state.pool_content_revision() > pool_revision,
                "{replay} moves pool content, so the dots rebuild"
            );
        }
        app.song_transport_stop().unwrap();
    }

    /// 5.1 / 7: no coalescing beyond the gesture deferral. With a gesture
    /// open, the replay parks the invalidation in
    /// `pending_song_row_invalidation` and the commit's own gesture close
    /// flushes it — exactly one preflight, never two.
    #[test]
    fn a_step_edit_under_an_open_gesture_flushes_one_invalidation_at_gesture_end() {
        use crate::app::history::{ActiveGesture, GestureId, MergeKey};

        let mut app = playing_song_app(app_with_song());
        let before = app.active_runtime_song.clone().expect("active song");
        app.history
            .begin_gesture(ActiveGesture {
                id: GestureId(1),
                merge_key: MergeKey::new("device-drag"),
            })
            .expect("gesture starts");

        toggle_step(&mut app, 3);

        let refreshes = drained_refreshes(&app);
        assert_eq!(refreshes.len(), 1, "one flush, not one per replay");
        assert_swaps_in_place(&before, &refreshes[0]);
        assert!(
            app.pending_song_row_invalidation.is_none(),
            "the deferred slot is drained by the gesture close, not left dangling"
        );
        assert!(app.history.active_gesture().is_none());
        app.song_transport_stop().unwrap();
    }

    /// Start arrangement capture on an app whose rendered-beat clock is 0.
    fn start_capture(app: &mut App) {
        app.state.set_scheduler_rendered_beats(0.0);
        app.set_arrangement_view_visible(true);
        let mode = app.song_transport_play(true).expect("capture starts");
        assert_eq!(mode, SongTransportMode::ArrangementCapture);
    }

    fn committed(app: &App) -> crate::sequencer::ProjectSong {
        app.state.committed_song().expect("song committed")
    }

    fn row_tuples(
        song: &crate::sequencer::ProjectSong,
    ) -> Vec<(f64, usize, Vec<(usize, Option<u64>)>)> {
        song.rows
            .iter()
            .map(|row| {
                (
                    row.start_beat,
                    row.scene.expect("captured rows carry a scene"),
                    row.overrides
                        .iter()
                        .map(|over| (over.track, over.pattern_id))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn capture_into_an_empty_arrangement_captures_the_auto_latched_scene() {
        // Empty-arrangement spec 6 + unified-transport spec 4.1: capture is
        // one code path — a [P, Q) splice into the arrangement that exists,
        // the empty one included. A SILENT start auto-latches the selected
        // scene inside the capture, so what commits is what was HEARD: the
        // scene from beat 0, with the performer's track launch layered as
        // an override. The canvas keeps its default length.
        let mut app = app_with_song();
        app.arr_clear().expect("start from an empty arrangement");

        start_capture(&mut app);
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the silent start latched the selected scene into the capture"
        );
        app.apply_manual_pattern_launch(&PatternLaunchTarget::SceneTracks {
            scene: 1,
            tracks: vec![0],
        })
        .expect("captured launch");
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");

        let arrangement = app
            .state
            .committed_arrangement()
            .expect("capture commits an arrangement");
        assert_eq!(
            arrangement.scene_lane.len(),
            1,
            "the auto-latched scene is the captured initial state"
        );
        assert_eq!(arrangement.scene_lane[0].scene, 0);
        assert_eq!(arrangement.scene_lane[0].start_beat, 0.0);
        assert_eq!(arrangement.track_lanes[0].len(), 1);
        let clip = arrangement.track_lanes[0][0];
        assert_eq!(clip.start_beat, 0.0);
        assert_eq!(clip.end_beat, 8.0);
        assert_eq!(
            clip.pattern_id,
            Some(2),
            "the track launch's pattern (scene 1's cell) wins the lane"
        );
        assert_eq!(
            arrangement.end_beat,
            crate::sequencer::DEFAULT_ARRANGEMENT_END,
            "the empty canvas keeps its default length"
        );

        let song = committed(&app);
        assert_eq!(song.rows[0].start_beat, 0.0);
        assert_eq!(
            song.rows[0].scene,
            Some(0),
            "the captured pass is governed by the auto-latched scene"
        );
        assert_eq!(
            song.rows[0].overrides[0].pattern_id,
            Some(2),
            "the clip is the launch"
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_splices_from_first_launch_preserving_the_head() {
        // Splice stopgap (takes spec 9.5): with an existing committed song,
        // the commit replaces it only from the FIRST captured launch's beat
        // onward — content before the punch-in survives verbatim.
        let mut app = app_with_song(); // rows at 0/4/8, end 16
        let depth = app.history.undo_len();
        start_capture(&mut app);
        // Listen for 6 beats before the first (and only) launch.
        app.state.set_scheduler_rendered_beats(6.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 2 })
            .expect("captured launch");
        app.state.set_scheduler_rendered_beats(10.0);
        app.song_transport_stop().expect("stop commits the splice");
        let song = committed(&app);
        assert_eq!(
            row_tuples(&song),
            vec![
                // The head of the song is preserved, not nuked to beat 0:
                // these are the clips scene events 0 and 4 stamped.
                (0.0, 0, vec![(0, Some(1))]),
                (4.0, 1, vec![(0, Some(2))]),
                // From the punch-in on, the capture is the authority (the
                // old row at 8 is inside the replaced region). The lane is
                // materialized with the free-run phase stamp (spec 9.4), and
                // the pre-existing clip resumes at Q playing the same pattern
                // at the same phase — so `normalize` collapses that boundary
                // away entirely, which IS the phase-continuity proof.
                (6.0, 2, vec![(0, Some(3))]),
            ]
        );
        // 6 beats into a 4-beat/16-step free-run = 24 steps mod 16 = 8.
        assert_eq!(song.rows[2].overrides[0].offset_steps, 8.0);
        assert_eq!(song.end_beat, 16.0, "the existing song end survives");
        assert_eq!(app.history.undo_len(), depth + 1, "one commit, one entry");
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_immediate_launch_retains_exact_fractional_scheduler_beat() {
        let mut app = app_with_song();
        start_capture(&mut app);
        // An unquantized launch is audible at the scheduler-derived beat at
        // application time; nothing may snap it to a grid (spec 8.2).
        app.state.set_scheduler_rendered_beats(2.375);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("immediate launch");
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        assert_eq!(
            row_tuples(&song),
            vec![
                (0.0, 0, vec![(0, Some(1))]),
                (2.375, 1, vec![(0, Some(2))]),
                // Full splice (takes spec 9.2): the pre-existing clip at the
                // punch-out beat survives — nothing after Q is nuked.
                (8.0, 2, vec![(0, Some(3))]),
            ],
            "the fractional beat must survive to the committed row exactly"
        );
        // Free-run phase stamping (spec 9.4): committed playback re-enters
        // the pattern mid-phase ON the grid, exactly as performed. 2.375
        // beats at 4 steps/beat = offset 9.5 steps.
        assert_eq!(song.rows[1].overrides[0].offset_steps, 9.5);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_quantized_launch_stores_the_audible_boundary_not_request_time() {
        let mut app = app_with_song();
        start_capture(&mut app);

        // Request a bar-quantized launch mid-grid-interval (rendered 2.6).
        app.state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::Scene { scene: 2 },
                crate::quantized_launch::LaunchQuantize::Bar,
                crate::quantized_launch::QuantizedLaunchOwner::Transport,
                app.state.scene_count(),
                app.tracks.len(),
                None,
            )
            .expect("schedule succeeds");
        let mut pending = crate::quantized_launch::PendingQuantizedLaunches::default();
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 2.6, 2.6, true, false);
        // The launch becomes due after the boundary; the control thread
        // drains it late (rendered 4.2) — the quantized path through
        // `apply_pattern_launch` must still capture the stamped 4.0.
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 4.2, 4.2, true, false);
        app.state.set_scheduler_rendered_beats(4.2);
        let results = app.drain_due_pattern_launches();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());

        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        assert_eq!(
            row_tuples(&song),
            // The captured clip plays scene 2's cell from beat 4 at free-run
            // phase 0, and the pre-existing clip from beat 8 plays the same
            // cell at the same phase, so the two rows collapse into one.
            vec![(0.0, 0, vec![(0, Some(1))]), (4.0, 2, vec![(0, Some(3))])],
            "the captured beat is the scheduled grid boundary"
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    /// A session-mode quantized launch is applied by the scheduler at the
    /// boundary (chunk split); the control drain must MIRROR it — same
    /// state/graph work, scene index moves, but NO pattern-epoch bump (a
    /// bump would drop the in-flight events including the boundary step) —
    /// and acknowledge so the scheduler drops its snapshot override.
    #[test]
    fn scheduler_applied_boundary_launch_mirrors_without_epoch_bump() {
        let mut app = test_app();
        app.state.start_playback();

        let token = app
            .state
            .schedule_quantized_pattern_launch(
                PatternLaunchTarget::Scene { scene: 1 },
                crate::quantized_launch::LaunchQuantize::Bar,
                crate::quantized_launch::QuantizedLaunchOwner::Transport,
            )
            .expect("schedule while playing");
        let epoch_before = app
            .state
            .transport
            .pattern_epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        let mirror_epoch_before = app.song_row_mirror_epoch;

        // Scheduler side: the playing session request routes to the
        // boundary machinery (the accessor preflighted a snapshot) and
        // installs at the bar boundary.
        let mut pending = crate::quantized_launch::PendingQuantizedLaunches::default();
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 0.5, 0.5, true, false);
        app.state.set_scheduler_rendered_beats(4.0);
        let (_, install) = pending.next_session_chunk(4.0, 1_000.0, 512);
        assert!(matches!(
            install,
            crate::quantized_launch::SessionLaunchInstall::AllTracks
        ));
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 4.0, 4.0, true, false);

        // Control side: the due is a mirror, not a full launch.
        let results = app.drain_due_pattern_launches();
        assert_eq!(results.len(), 1);
        let outcome = results[0].as_ref().expect("mirror applies");
        assert_eq!(outcome.token, Some(token));
        assert_eq!(app.state.current_scene_index(), 1);
        assert_eq!(
            app.state
                .transport
                .pattern_epoch
                .load(std::sync::atomic::Ordering::Relaxed),
            epoch_before,
            "the mirror must not bump the pattern epoch"
        );
        assert_eq!(
            app.song_row_mirror_epoch,
            mirror_epoch_before + 1,
            "the mirror drives the UI pattern resync through the mirror-epoch seam"
        );

        // The acknowledgement releases the scheduler's snapshot override.
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 4.1, 4.1, true, false);
        let base = app.state.latest_scheduler_snapshot();
        assert!(pending.session_snapshot(&base).is_none());
        app.state.stop_playback();
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_consolidates_same_boundary_scene_and_track_launches() {
        // Track launch first, scene launch second at one audible boundary:
        // the row must contain the new scene PLUS the track override even
        // though the audible scene launch cleared it in session state
        // (spec 10.4: scene clears overrides before same-boundary track
        // launches consolidate, regardless of input order).
        for scene_first in [false, true] {
            let mut app = app_with_song();
            start_capture(&mut app);
            app.state.set_scheduler_rendered_beats(4.0);
            let scene = PatternLaunchTarget::Scene { scene: 2 };
            let tracks = PatternLaunchTarget::SceneTracks {
                scene: 1,
                tracks: vec![0],
            };
            let (first, second) = if scene_first {
                (&scene, &tracks)
            } else {
                (&tracks, &scene)
            };
            app.apply_manual_pattern_launch(first).expect("first launch");
            app.apply_manual_pattern_launch(second).expect("second launch");
            app.state.set_scheduler_rendered_beats(8.0);
            app.song_transport_stop().expect("stop commits");
            let song = committed(&app);
            assert_eq!(
                row_tuples(&song),
                vec![
                    (0.0, 0, vec![(0, Some(1))]),
                    (4.0, 2, vec![(0, Some(2))]),
                    // The pre-existing arrangement resumes at Q (spec 9.2):
                    // scene 2's own cell, which the capture never claimed.
                    (8.0, 2, vec![(0, Some(3))]),
                ],
                "scene_first={scene_first}: one row, scene 2 plus the track override"
            );
            app.state.set_scheduler_rendered_beats(0.0);
        }
    }

    #[test]
    fn capture_repeated_identical_state_produces_no_row() {
        let mut app = app_with_song();
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(2.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("launch");
        app.state.set_scheduler_rendered_beats(4.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("identical relaunch");
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        assert_eq!(
            row_tuples(&song),
            // The lane carries the free-run phase stamp (spec 9.4): 2 beats
            // at 4 steps/beat = offset 8. The pre-existing arrangement
            // resumes at the punch-out (spec 9.2).
            vec![
                (0.0, 0, vec![(0, Some(1))]),
                (2.0, 1, vec![(0, Some(2))]),
                (8.0, 2, vec![(0, Some(3))]),
            ],
            "the identical relaunch must not create a second row"
        );
        assert_eq!(song.rows[1].overrides[0].offset_steps, 8.0);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    /// Two-track sibling of `test_app` (pool ids 1..=3 per track).
    fn test_app_two_tracks() -> App {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
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
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        app
    }

    #[test]
    fn capture_splice_preserves_content_before_p_and_after_q() {
        // Previous song rows at 0/4/8, end 16. Capture launches only inside
        // (5.0, 6.5): the row at 4 splits, the row at 8 survives verbatim,
        // and a restore row appears at Q with the pre-existing state's
        // advanced phase.
        let mut app = app_with_song();
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(5.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 0 })
            .expect("launch");
        app.state.set_scheduler_rendered_beats(6.5);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        assert_eq!(
            row_tuples(&song),
            vec![
                (0.0, 0, vec![(0, Some(1))]),
                // Content before P is untouched (the old clip at 4 keeps its
                // span up to the punch-in).
                (4.0, 1, vec![(0, Some(2))]),
                // The performance owns [5.0, 6.5): free-run stamp 5 beats =
                // 20 steps mod 16 = 4.
                (5.0, 0, vec![(0, Some(1))]),
                // At Q the pre-existing clip resumes — `occlude_span`
                // left-trimmed it and re-stamped its phase, so it re-enters
                // mid-pattern (6.5 beats = 26 steps mod 16 = 10) with no
                // restore machinery involved.
                (6.5, 1, vec![(0, Some(2))]),
                // Content after Q is untouched.
                (8.0, 2, vec![(0, Some(3))]),
            ]
        );
        assert_eq!(song.rows[2].overrides[0].offset_steps, 4.0);
        assert_eq!(song.rows[3].overrides[0].offset_steps, 10.0);
        assert_eq!(song.end_beat, 16.0);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    /// The resolved `(pattern, step position)` of one lane at `beat` under a
    /// compiled song: the row's override if it has one, else the row scene's
    /// cell advanced from the row start. This is what the runtime plays.
    fn lane_phase_at(
        app: &App,
        song: &crate::sequencer::ProjectSong,
        track: usize,
        beat: f64,
    ) -> (Option<u64>, f64) {
        let row = crate::sequencer::state_at_beat(song, beat).expect("a row governs the beat");
        let delta = beat - row.start_beat;
        match row.overrides.iter().find(|over| over.track == track) {
            Some(over) => match over.pattern_id {
                Some(pattern) => (
                    Some(pattern),
                    app.advanced_offset(track, pattern, over.offset_steps, delta),
                ),
                None => (None, 0.0),
            },
            None => {
                let cell = app.state.with_project_scenes(|scenes| {
                    row.scene
                        .and_then(|scene| scenes.scenes.get(scene))
                        .and_then(|scene| scene.cells.get(track))
                        .copied()
                        .flatten()
                        .map(|pattern| pattern.0)
                });
                match cell {
                    Some(pattern) => (
                        Some(pattern),
                        app.advanced_offset(track, pattern, 0.0, delta),
                    ),
                    None => (None, 0.0),
                }
            }
        }
    }

    #[test]
    fn capture_leaves_untouched_lanes_unwritten_and_phase_continuous() {
        // Two tracks; the performer only launches track 0. Track 1 keeps
        // playing the committed song underneath (takes spec 9.3). In the lane
        // model that inheritance needs NO representation: the lane is simply
        // NOT MODIFIED, so the clips that already covered the punch region
        // keep playing straight through it. (The row model had to materialize
        // an override carrying the inherited pattern and its advanced phase
        // onto every captured row, because a row's scene column would
        // otherwise silence it.)
        let mut app = test_app_two_tracks();
        app.arr_replace_rows(
            vec![
                SongRowSpec {
                    start_beat: 0.0,
                    scene: 0,
                    overrides: Vec::new(),
                },
                SongRowSpec {
                    start_beat: 8.0,
                    scene: 1,
                    overrides: Vec::new(),
                },
            ],
            16.0,
            false,
        )
        .expect("song committed");
        let before = committed(&app);
        let before_arrangement = app
            .state
            .committed_arrangement()
            .expect("the def-song lowering commits an arrangement");

        start_capture(&mut app);
        // 4.5 beats in, so the untouched lane sits mid-pattern (18 steps into
        // a 16-step cycle) exactly where inheritance is easiest to get wrong.
        app.state.set_scheduler_rendered_beats(4.5);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::SceneTracks {
            scene: 2,
            tracks: vec![0],
        })
        .expect("track launch");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "only the launched track latches"
        );
        app.state.set_scheduler_rendered_beats(6.0);
        app.song_transport_stop().expect("stop commits");

        let arrangement = app
            .state
            .committed_arrangement()
            .expect("the capture commits an arrangement");
        // The headline win: the untouched lane is byte-identical to what it
        // was — not trimmed, not re-stamped, not written at all.
        assert_eq!(
            arrangement.track_lanes[1], before_arrangement.track_lanes[1],
            "the untouched lane must not be modified"
        );
        // The performer's lane is one clip over the punch region, stamped
        // with the free-run phase (takes spec 7.2): 4.5 beats = 18 steps mod
        // the 16-step pattern = 2.
        let clip = arrangement.track_lanes[0]
            .iter()
            .find(|clip| clip.start_beat == 4.5)
            .expect("the launch opens a clip at the punch-in");
        assert_eq!(clip.pattern_id, Some(3), "scene 2's cell for track 0");
        assert!((clip.offset_steps - 2.0).abs() < 1e-9, "{}", clip.offset_steps);
        assert_eq!(clip.end_beat, 6.0, "the clip closes at the punch-out");

        let song = committed(&app);
        // The compiled song states the untouched lane's resolution on every
        // boundary another track's clip created, but only ever as the clip
        // the row's own scene stamped there — never a launch the performer
        // did not make.
        for row in &song.rows {
            let Some(over) = row.overrides.iter().find(|over| over.track == 1) else {
                continue;
            };
            let cell = app.state.with_project_scenes(|scenes| {
                scenes.scenes[row.scene.expect("captured rows carry a scene")].cells[1]
                    .map(|pattern| pattern.0)
            });
            assert_eq!(
                (over.pattern_id, over.take_id),
                (cell, None),
                "beat {}: the untouched lane may only be phase-materialized",
                row.start_beat
            );
        }
        // And it plays exactly what it played before the capture — through
        // the punch region, across the punch-out, and past the song's own
        // scene change at beat 8.
        for beat in [3.0, 4.5, 5.0, 6.0, 7.5, 8.0, 9.25, 15.0] {
            assert_eq!(
                lane_phase_at(&app, &song, 1, beat),
                lane_phase_at(&app, &before, 1, beat),
                "track 1 must play phase-continuously at beat {beat}"
            );
        }
        // The latch auto-clears at punch-out (takes spec 10).
        assert_eq!(app.state.song_manual_latch_mask(), 0);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    /// End-to-end reproduction of the "sync/timebase p-locks out of time
    /// after arrangement capture" bug: arrangement-record from session,
    /// launch a scene with a p-locked pattern at an unquantized beat, and
    /// follow the free-run phase stamp through EVERY layer the audio path
    /// consumes — the captured clip, the compiled row override, and the
    /// preflighted runtime row's `lane_offsets` (what the scheduler anchors
    /// the clock with). Each must carry the REAL-geometry free-run phase.
    #[test]
    fn capture_of_plocked_pattern_stamps_real_geometry_phase_end_to_end() {
        use crate::sequencer::{PatternId, PatternSnapshot, StepParam, Timebase};
        let mut app = test_app_two_tracks();
        // Scene 1's cell for track 0 is pool pattern 2. Give it the p-locked
        // shape: step 0 is a half-beat step synced to the 1-beat grid
        // (padding the cycle to 5.0 beats), step 5 synced to the same grid
        // (a mid-pattern wait). A uniform 16th-note ruler would call this
        // pattern 4.0 beats long; the real cycle is 5.0.
        let mut snapshots = vec![
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
        ];
        snapshots[1].timebase_plock_snapshots[0][0] = Some(Timebase::Eighth as u32);
        snapshots[1].step_data[0][0][StepParam::Sync.index()] = 3.0;
        snapshots[1].step_data[0][5][StepParam::Sync.index()] = 3.0;
        app.state.replace_pattern_repository(snapshots, 0);

        // The performance: record from session, free-run to beat 5.3, launch
        // scene 1 unquantized, stop at beat 12.
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(5.3);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("scene launch");
        app.state.set_scheduler_rendered_beats(12.0);
        app.song_transport_stop().expect("stop commits");

        // What session free-run audibly played at 5.3: 5.3 mod the real
        // 5.0-beat cycle = 0.3 beats = 60% through the half-beat step 0.
        let geometry = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(PatternId(2))
                .expect("scene 1's cell pattern")
                .step_geometry()
        });
        assert!(
            (geometry.cycle_beats() - 5.0).abs() < 1e-9,
            "fixture cycle: {}",
            geometry.cycle_beats()
        );
        let expected = geometry.steps_at_beats(5.3);
        assert!((expected - 0.6).abs() < 1e-9, "free-run phase: {expected}");

        // Layer 1: the captured clip.
        let arrangement = app
            .state
            .committed_arrangement()
            .expect("capture commits an arrangement");
        let clip = arrangement.track_lanes[0]
            .iter()
            .find(|clip| (clip.start_beat - 5.3).abs() < 1e-9)
            .expect("the launch opens a clip at the punch-in");
        assert_eq!(clip.pattern_id, Some(2), "scene 1's cell for track 0");
        assert!(
            (clip.offset_steps - expected).abs() < 1e-6,
            "clip stamped {} but free-run played {}",
            clip.offset_steps,
            expected
        );

        // Layer 2: the compiled row override.
        let song = committed(&app);
        let row = song
            .rows
            .iter()
            .find(|row| (row.start_beat - 5.3).abs() < 1e-9)
            .expect("a row at the launch beat");
        let over = row
            .overrides
            .iter()
            .find(|over| over.track == 0)
            .expect("launched lane override");
        assert!(
            (over.offset_steps - expected).abs() < 1e-6,
            "row stamped {} but free-run played {}",
            over.offset_steps,
            expected
        );

        // Layer 3: the preflighted runtime row the scheduler anchors with.
        let runtime = app
            .state
            .preflight_runtime_song()
            .expect("preflight succeeds");
        let runtime_row = runtime
            .rows
            .iter()
            .find(|row| (row.start_beat - 5.3).abs() < 1e-9)
            .expect("a runtime row at the launch beat");
        assert!(
            (runtime_row.lane_offsets[0] - expected).abs() < 1e-6,
            "runtime lane offset {} but free-run played {}",
            runtime_row.lane_offsets[0],
            expected
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    /// The user-reported repro: a committed song, arrangement-record started
    /// AT THE CURSOR (record-clock zero = cursor beat), and an unquantized
    /// scene launch of a p-locked pattern. The launched lane audibly
    /// free-runs against the RECORD clock, so the stamp must be
    /// `steps(beat - cursor)`, not `steps(timeline_beat)` — on a pattern
    /// whose real cycle (5.0 here) doesn't divide the cursor position, the
    /// two differ and the timeline-domain stamp plays back rotated and off
    /// the sync grid.
    #[test]
    fn capture_from_cursor_stamps_record_clock_phase_for_plocked_pattern() {
        use crate::sequencer::{PatternId, PatternSnapshot, StepParam, Timebase};
        let mut app = test_app_two_tracks();
        let mut snapshots = vec![
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
        ];
        // Same p-locked shape as the end-to-end test: real cycle 5.0 beats.
        snapshots[1].timebase_plock_snapshots[0][0] = Some(Timebase::Eighth as u32);
        snapshots[1].step_data[0][0][StepParam::Sync.index()] = 3.0;
        snapshots[1].step_data[0][5][StepParam::Sync.index()] = 3.0;
        app.state.replace_pattern_repository(snapshots, 0);
        app.arr_replace_rows(
            vec![SongRowSpec {
                start_beat: 0.0,
                scene: 0,
                overrides: Vec::new(),
            }],
            16.0,
            false,
        )
        .expect("song committed");

        // Record from bar 2: cursor at beat 4.0. The record clock's zero is
        // the cursor, so a launch at raw beat 1.3 is timeline beat 5.3.
        app.arrangement_cursor_beat = 4.0;
        app.state.set_scheduler_rendered_beats(0.0);
        app.set_arrangement_view_visible(true);
        let mode = app.song_transport_play(true).expect("capture starts");
        assert_eq!(mode, SongTransportMode::ArrangementCapture);
        app.state.set_scheduler_rendered_beats(1.3);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("scene launch");
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");

        let geometry = app.state.with_project_scenes(|scenes| {
            scenes.track_pools[0]
                .get(PatternId(2))
                .expect("scene 1's cell pattern")
                .step_geometry()
        });
        // What the performer heard: free-run 1.3 beats into the record
        // clock. What the timeline-domain stamp would wrongly claim: 5.3.
        let heard = geometry.steps_at_beats(1.3);
        let timeline_stamp = geometry.steps_at_beats(5.3);
        assert!(
            (heard - timeline_stamp).abs() > 0.5,
            "fixture must discriminate the two domains: {heard} vs {timeline_stamp}"
        );

        let arrangement = app
            .state
            .committed_arrangement()
            .expect("capture commits an arrangement");
        let clip = arrangement.track_lanes[0]
            .iter()
            .find(|clip| (clip.start_beat - 5.3).abs() < 1e-9)
            .expect("the launch opens a clip at the punch-in");
        assert_eq!(clip.pattern_id, Some(2));
        assert!(
            (clip.offset_steps - heard).abs() < 1e-6,
            "clip stamped {} but the performer heard record-clock phase {}",
            clip.offset_steps,
            heard
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_scene_launch_claims_take_lanes_and_spares_what_came_before() {
        // Takes spec 10 rev 5 (eseq-ut5j): pressing a scene during capture
        // STOPS whatever take the arrangement was playing and plays the
        // scene — the launch claims every lane, take lanes included, and the
        // commit splices the scene over the take. The claim starts at the
        // launch beat: the part of the take BEFORE it is untouched.
        use crate::sequencer::{PatternSnapshot, ProjectSongTrackOverride, MAX_STEPS};
        let mut app = test_app_two_tracks();
        let mut chunk = PatternSnapshot::new_default(2, &[])
            .track_pattern_data(0)
            .expect("chunk template");
        chunk.track_params.num_steps = MAX_STEPS;
        chunk.track_bits[0] |= 1;
        // 64 steps = 16 beats: the take spans the whole song, so it is
        // still audible at the scene-launch beat.
        let take_id = app
            .state
            .register_track_take(0, None, vec![chunk], 64, None)
            .expect("take registers");
        app.arr_replace_rows(
            vec![SongRowSpec {
                start_beat: 0.0,
                scene: 0,
                overrides: vec![ProjectSongTrackOverride {
                    track: 0,
                    pattern_id: None,
                    take_id: Some(take_id.0),
                    offset_steps: 0.0,
                }],
            }],
            16.0,
            false,
        )
        .expect("song committed");
        start_capture(&mut app);
        assert_eq!(
            app.state.song_take_lane_mask(),
            1,
            "row zero marks track 0 as a take lane"
        );
        app.state.set_scheduler_rendered_beats(4.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("scene launch");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            0b11,
            "the scene launch latches EVERY lane, the take lane included"
        );
        assert_eq!(
            app.state.song_take_lane_mask(),
            0,
            "a claimed lane is no longer governed by its take"
        );
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);

        // What came BEFORE the launch is untouched: the take still governs
        // track 0 on every row starting before beat 4.
        let before: Vec<_> = song
            .rows
            .iter()
            .filter(|row| row.start_beat < 4.0)
            .collect();
        assert!(!before.is_empty(), "the pre-launch span survives the splice");
        for row in &before {
            let track0 = row
                .overrides
                .iter()
                .find(|over| over.track == 0)
                .unwrap_or_else(|| panic!("row at {} lost its take lane", row.start_beat));
            assert_eq!(
                track0.take_id,
                Some(take_id.0),
                "the take before the launch beat is intact"
            );
            assert_eq!(track0.offset_steps, 0.0, "and keeps its phase anchor");
        }

        // Inside the punch region [launch, Stop) the scene replaced the take
        // outright — not layered underneath it. (Past Stop the pre-existing
        // arrangement resumes, take included: that span was never captured.)
        let spliced = song
            .rows
            .iter()
            .find(|row| row.start_beat == 4.0)
            .expect("spliced row at the launch beat");
        assert_eq!(spliced.scene, Some(1), "the launched scene governs the row");
        assert!(
            song.rows
                .iter()
                .filter(|row| row.start_beat >= 4.0 && row.start_beat < 8.0)
                .all(|row| row
                    .overrides
                    .iter()
                    .all(|over| over.track != 0 || over.take_id.is_none())),
            "the scene replaced the take for the whole punch region"
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn clip_launch_latches_the_lane_and_splices_at_the_launch_beat() {
        // The mixer grid clip click (`set-scene-cell` path) is a
        // performance gesture: it must latch the lane — so later
        // arrangement row changes stop stealing it until punch-out — and
        // land in the capture at the beat it was performed.
        let mut app = app_with_song();
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(3.0);
        app.observe_manual_clip_launch(0, PatternId(2));
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the clip launch latches its lane"
        );
        assert_eq!(
            app.song_capture_take.as_ref().map(|take| take.event_count()),
            Some(1),
            "the clip launch is captured"
        );
        // A later mirrored row transition must leave the latched lane alone.
        let song = app.active_runtime_song.clone().expect("active song");
        app.mirror_song_row_applied(&AudibleSongRowApplied {
            row_id: song.rows[1].id,
            row_ordinal: 1,
            effective_beat: 4.0,
            effective_sample: 44_100,
            wrapped: false,
        })
        .expect("mirror succeeds");
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "row transitions do not clear the performer's latch"
        );
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        let row = song
            .rows
            .iter()
            .find(|row| (row.start_beat - 3.0).abs() < 1e-9)
            .expect("spliced row at the launch beat, not after");
        let over = row
            .overrides
            .iter()
            .find(|over| over.track == 0)
            .expect("clip override on the launched lane");
        assert_eq!(over.pattern_id, Some(2));
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn scene_launch_latches_scene_identity_against_row_mirrors() {
        // Takes spec 10: a manual scene launch latches GLOBALLY — including
        // the scene identity. A later recorded/committed row passing through
        // must not recall its scene's bus pattern, move the current scene,
        // or flip the `current_pattern` atomic (scene-keyed reactive
        // instrument bindings and bus/group fx recall audibly follow them),
        // even though the row's step content is already latch-protected.
        let mut app = app_with_song();
        start_capture(&mut app);
        assert_eq!(app.state.current_scene_index(), 0, "row zero applied");

        app.state.set_scheduler_rendered_beats(2.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("scene launch");
        assert_eq!(app.state.current_scene_index(), 1);
        assert!(app.state.song_scene_latch(), "scene launch latches the scene identity");

        // The committed song's later scene-2 row passes through underneath.
        let song = app.active_runtime_song.clone().expect("active song");
        app.mirror_song_row_applied(&AudibleSongRowApplied {
            row_id: song.rows[2].id,
            row_ordinal: 2,
            effective_beat: 8.0,
            effective_sample: 88_200,
            wrapped: false,
        })
        .expect("mirror succeeds");

        assert_eq!(
            app.state.current_scene_index(),
            1,
            "the row mirror must not steal the performer's current scene"
        );
        assert_eq!(
            app.state.current_pattern_index(),
            1,
            "the `current_pattern` atomic stays the performer's scene"
        );
        assert_eq!(
            app.state.song_manual_latch_mask(),
            1,
            "the track latch survives the mirror"
        );

        // Per-track Back to Song frees the lane but the scene identity is
        // still the performer's until a full Back to Song / punch-out.
        app.back_to_song_track(0).expect("per-track back to song");
        assert!(
            app.state.song_scene_latch(),
            "per-track Back to Song leaves the scene latch"
        );

        // Full Back to Song returns the scene identity to the song's row.
        app.back_to_song().expect("back to song");
        assert!(!app.state.song_scene_latch());
        assert_eq!(
            app.state.current_scene_index(),
            0,
            "back to song re-applies the governing row's scene"
        );
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn clip_launch_capture_keeps_untouched_take_lane_continuous() {
        // Two lanes playing long takes; the performer clip-launches ONLY
        // track 0 mid-take and stops before the takes end. Track 1 must
        // keep its take through the spliced region AND after the punch-out
        // restore row — continuous offsets, no truncation.
        use crate::sequencer::{PatternSnapshot, ProjectSongTrackOverride, MAX_STEPS};
        let mut app = test_app_two_tracks();
        let mut take_ids = Vec::new();
        for track in 0..2 {
            let mut chunk = PatternSnapshot::new_default(2, &[])
                .track_pattern_data(track)
                .expect("chunk template");
            chunk.track_params.num_steps = MAX_STEPS;
            chunk.track_bits[0] |= 1;
            // 64 steps = 16 beats at the default sixteenth timebase.
            take_ids.push(
                app.state
                    .register_track_take(track, None, vec![chunk], 64, None)
                    .expect("take registers"),
            );
        }
        app.arr_replace_rows(
            vec![SongRowSpec {
                start_beat: 0.0,
                scene: 0,
                overrides: (0..2)
                    .map(|track| ProjectSongTrackOverride {
                        track,
                        pattern_id: None,
                        take_id: Some(take_ids[track].0),
                        offset_steps: 0.0,
                    })
                    .collect(),
            }],
            16.0,
            false,
        )
        .expect("song committed");
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(3.0);
        app.observe_manual_clip_launch(0, PatternId(2));
        app.state.set_scheduler_rendered_beats(5.5);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        // Track 1: every row across the whole song still references its
        // take with the offset matching the row start (4 steps per beat).
        for row in &song.rows {
            let over = row
                .overrides
                .iter()
                .find(|over| over.track == 1)
                .unwrap_or_else(|| {
                    panic!(
                        "track 1 lost its take override at row {}",
                        row.start_beat
                    )
                });
            assert_eq!(
                over.take_id,
                Some(take_ids[1].0),
                "track 1 still plays its take at row {}",
                row.start_beat
            );
            assert!(
                (over.offset_steps - row.start_beat * 4.0).abs() < 1e-6,
                "track 1 take offset continuous at row {}: got {}",
                row.start_beat,
                over.offset_steps
            );
        }
        // Track 0: the clip governs [3.0, 5.5), the take resumes at 5.5.
        let clip_row = song
            .rows
            .iter()
            .find(|row| (row.start_beat - 3.0).abs() < 1e-9)
            .expect("spliced row at the clip launch");
        let track0 = clip_row
            .overrides
            .iter()
            .find(|over| over.track == 0)
            .expect("clip override");
        assert_eq!(track0.pattern_id, Some(2));
        let restore_row = song
            .rows
            .iter()
            .find(|row| (row.start_beat - 5.5).abs() < 1e-9)
            .expect("restore row at punch-out");
        let track0 = restore_row
            .overrides
            .iter()
            .find(|over| over.track == 0)
            .expect("track 0 restored");
        assert_eq!(
            track0.take_id,
            Some(take_ids[0].0),
            "track 0's take resumes at punch-out"
        );
        assert!((track0.offset_steps - 22.0).abs() < 1e-6);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_stop_commits_atomically_with_one_undo_entry() {
        let mut app = app_with_song();
        let song_before = app.state.committed_song();
        let depth = app.history.undo_len();
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(2.5);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("launch");
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        assert_eq!(
            app.history.undo_len(),
            depth + 1,
            "the whole capture commit is exactly one undo entry"
        );
        let captured = app.state.committed_song();
        assert_ne!(captured, song_before);
        // Rows are compiled output now, so their ids are positional (lane
        // spec 7 step 4); the identity that must never be reused lives on
        // clips, and the capture's clips continue that allocator.
        let song = committed(&app);
        assert_eq!(song.next_row_id, song.rows.len() as u64);
        let arrangement = app
            .state
            .committed_arrangement()
            .expect("the capture commits an arrangement");
        assert!(
            arrangement
                .track_lanes
                .iter()
                .flatten()
                .all(|clip| clip.id.0 < arrangement.next_clip_id),
            "clip ids stay below the allocator"
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "undo restores the previous committed song"
        );
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.state.committed_song(), captured);
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn capture_overflow_prevents_commit_and_populates_error_bindings() {
        let mut app = app_with_song();
        let song_before = app.state.committed_song();
        start_capture(&mut app);
        app.state.set_scheduler_rendered_beats(2.0);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("launch");
        // Trip the sticky notice-overflow flag (bounded channel capacity is
        // 256; the extra pushes are dropped and latch the flag, spec 10.3).
        for _ in 0..300 {
            app.state
                .song_playback()
                .push_notice(SongPlaybackNotice::Ended {
                    end_beat: 0.0,
                    end_sample: 0,
                });
        }
        app.state.set_scheduler_rendered_beats(8.0);
        let error = app
            .song_transport_stop()
            .expect_err("overflow must prevent commit");
        assert!(error.contains("overflow"), "{error}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "the previous song is preserved"
        );
        assert!(app.song_capture_failed);
        assert!(
            app.song_capture_error
                .as_deref()
                .is_some_and(|error| error.contains("overflow")),
            "the error binding is populated"
        );
        assert!(app.song_capture_take.is_none());
        // Drain the notices the test stuffed into the channel.
        let _ = app.state.drain_song_playback_notices();

        // The failure state clears when the next capture starts.
        start_capture(&mut app);
        assert!(!app.song_capture_failed);
        assert!(app.song_capture_error.is_none());
        app.song_capture_cancel().expect("cancel cleans up");
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn zero_length_capture_is_a_no_op_that_preserves_the_previous_song() {
        // Stop on the very beat of the first launch: the punch region is
        // empty, so nothing is splicable. The commit is a graceful no-op —
        // no stray scene event, no history entry, previous song untouched
        // (empty-arrangement spec 6).
        let mut app = app_with_song();
        app.arr_clear().expect("clear song");
        let song_before = app.state.committed_song();
        let arrangement_before = app.state.committed_arrangement();
        start_capture(&mut app);
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("captured launch");
        let status = app
            .song_transport_stop()
            .expect("a zero-length capture stops cleanly");
        assert!(
            status.is_some_and(|status| status.contains("unchanged")),
            "the stop reports the no-op"
        );
        assert_eq!(app.state.committed_song(), song_before);
        assert_eq!(app.state.committed_arrangement(), arrangement_before);
        assert!(!app.song_capture_failed);
        assert!(app.song_capture_error.is_none());
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
    }

    /// Repro for the "latched lane loses device edits at Play" bug: empty
    /// arrangement → Play auto-latches the scene → Stop (latch survives) →
    /// add an effect while stopped → Play again. The second Play's scene
    /// launch must NOT restore a pre-effect pool snapshot over the live
    /// effect chain (num_params dropping to 0 makes every param edit fail
    /// validation with "effect parameter does not exist").
    #[test]
    fn effect_added_while_stopped_and_latched_survives_the_next_play() {
        let mut app = test_app();
        // Play #1 on the empty arrangement: silent start auto-latches the
        // selected scene; the latch survives the stop.
        app.song_transport_play(false).expect("play #1");
        app.song_transport_stop().expect("stop");
        assert_ne!(
            app.state.song_manual_latch_mask(),
            0,
            "precondition: the auto-latch survives the stop"
        );
        // "Add a builtin effect" while stopped: stamp the descriptor onto the
        // live slot and run the same pool sync the add path runs
        // (apply_builtin_effect_to_slot_with_modulator).
        let desc = crate::effects::EffectDescriptor::builtin_insert("Space Echo")
            .expect("Space Echo is a builtin");
        let slot_idx = crate::effects::BUILTIN_SLOT_COUNT;
        app.state.pattern.effect_chains[0][slot_idx].apply_descriptor_with_modulator(
            &desc, 7, 0,
        );
        app.state
            .sync_effect_slot_with_modulator_in_track_patterns(0, slot_idx, &desc, 7, 0);
        let params_before = app.state.pattern.effect_chains[0][slot_idx]
            .num_params
            .load(std::sync::atomic::Ordering::Relaxed);
        assert!(params_before > 0, "the live slot holds the effect");

        // Play #2: the scene re-launch must keep the effect.
        app.song_transport_play(false).expect("play #2");
        let params_after = app.state.pattern.effect_chains[0][slot_idx]
            .num_params
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            params_after, params_before,
            "play clobbered the freshly added effect slot from a stale pool snapshot"
        );
        app.song_transport_stop().expect("stop #2");
    }

    #[test]
    fn binding_strings_cover_every_mode() {
        assert_eq!(SongTransportMode::Stopped.binding_str(), "stopped");
        assert_eq!(SongTransportMode::SongPlayback.binding_str(), "song-playback");
        assert_eq!(
            SongTransportMode::ArrangementCapture.binding_str(),
            "arrangement-capture"
        );
    }
}
