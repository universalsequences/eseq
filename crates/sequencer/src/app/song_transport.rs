//! Song-mode transport authority state machine (docs/song-mode-spec.md 7/13).
//!
//! Exactly one launch authority is active at a time: `Stopped`,
//! `SessionPlayback`, `SongPlayback`, or `ArrangementCapture`. The mode lives
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

/// The single active launch authority (docs/song-mode-spec.md 13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SongTransportMode {
    #[default]
    Stopped,
    SessionPlayback,
    SongPlayback,
    ArrangementCapture,
}

impl SongTransportMode {
    /// Reactive-binding string (docs/song-mode-spec.md 12): `Stopped` and
    /// `SessionPlayback` both read "session" — the binding reports the launch
    /// authority the next/current playback uses, not the play state (that is
    /// `SEQ.playing`).
    pub fn binding_str(self) -> &'static str {
        match self {
            SongTransportMode::Stopped | SongTransportMode::SessionPlayback => "session",
            SongTransportMode::SongPlayback => "song-playback",
            SongTransportMode::ArrangementCapture => "arrangement-capture",
        }
    }
}

/// Rejection for manual scene/track-pattern launches during song playback
/// (docs/song-mode-spec.md 7.3).
pub const MANUAL_LAUNCH_DURING_SONG_ERROR: &str =
    "Manual launches are unavailable during song playback: stop the transport, disable \
     Use Arrangement, or enter arrangement recording";

/// Rejection for toggling `Use Arrangement` while the transport is playing
/// (docs/song-mode-spec.md 7.1).
pub const USE_ARRANGEMENT_WHILE_PLAYING_ERROR: &str =
    "Use Arrangement cannot change while the transport is playing; stop the transport first";

impl App {
    fn set_song_transport_mode(&mut self, mode: SongTransportMode) {
        self.song_transport_mode = mode;
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
        {
            return false;
        }
        self.begin_song_capture_take(self.active_song_start_beat.unwrap_or(0.0));
        self.set_song_transport_mode(SongTransportMode::ArrangementCapture);
        true
    }

    /// Back to Song (takes spec 10): clear the manual-override latch so the
    /// affected lanes snap back to whatever the song resolves at the
    /// current beat with anchored phase. Audible on the next scheduled
    /// chunk; the control-side mirror re-applies the current row here.
    pub fn back_to_song(&mut self) -> Result<String, String> {
        if !self.song_playback_authority_active() {
            return Err("Back to Song is only available during song playback".to_string());
        }
        if self.state.song_manual_latch_mask() == 0 && !self.state.song_scene_latch() {
            return Ok("No manual overrides are latched".to_string());
        }
        // Pending quantized manual launches must not fire after the return.
        let _ = self.state.quantized_launches().cancel_all();
        self.state.clear_song_manual_latch();
        if let Some(song) = self.active_runtime_song.clone() {
            let ordinal = self
                .state
                .song_playback()
                .shared()
                .current_row_ordinal()
                .min(song.rows.len().saturating_sub(1));
            if let Some(row) = song.rows.get(ordinal) {
                self.apply_song_row_control(row.scene, &row.overrides, false)?;
                self.song_mirrored_row = Some(ordinal);
                self.song_row_mirror_epoch += 1;
                // The row apply released any bound device loan and pushed the
                // row's scene-cell devices; re-resolve the bindings in the same
                // step so no lane keeps a stale loaded snapshot for a tick.
                self.sync_track_sound_bindings();
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
                self.apply_song_row_control(row.scene, &row.overrides, false)?;
                self.song_mirrored_row = Some(ordinal);
                self.song_row_mirror_epoch += 1;
                // Same as `back_to_song`: the row apply dropped the loan and
                // pushed the row's devices, so re-resolve the bindings now.
                self.sync_track_sound_bindings();
            }
        }
        Ok(format!("Track {}: back to song", track + 1))
    }

    /// Set the persisted `Use Arrangement` preference (spec 7.1). Rejected
    /// while playing; changing it while stopped only selects what the next
    /// Play does.
    pub fn set_use_arrangement(&mut self, enabled: bool) -> Result<(), String> {
        if self.use_arrangement == enabled {
            return Ok(());
        }
        if self.transport_engaged() {
            return Err(USE_ARRANGEMENT_WHILE_PLAYING_ERROR.to_string());
        }
        self.use_arrangement = enabled;
        if !enabled {
            // Leaving song mode entirely: the timeline selection has no
            // surface left to explain itself, so it is dropped rather than
            // left binding the device panel to a take (takes spec 16.6).
            self.set_song_clip_selection(None);
        }
        Ok(())
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

    /// Play, routed through the mode table in docs/song-mode-spec.md section
    /// 1. `record` is the transport record signal at Play time (pattern/note
    /// record toggle or `seq-song-capture-arm`). Returns the entered mode.
    pub fn song_transport_play(&mut self, record: bool) -> Result<SongTransportMode, String> {
        if self.transport_engaged() {
            return Err("Transport is already playing".to_string());
        }
        if !self.use_arrangement {
            self.state.start_playback();
            self.set_song_transport_mode(SongTransportMode::SessionPlayback);
            return Ok(SongTransportMode::SessionPlayback);
        }
        if record {
            // Arrangement capture: recording always runs ON TOP of song
            // playback (takes spec 9.3, empty-arrangement spec 6) — the
            // arrangement always exists, so there is no bootstrap mode. The
            // song plays (an empty one plays silence) and keeps launch
            // authority wherever the performer hasn't overridden it; manual
            // launches latch (spec 10) and are captured for the [P, Q)
            // splice. Open-ended (spec 7.4): the song end is not a stopping
            // point while recording — grooving past it extends the
            // arrangement rather than cutting the take off.
            let start_beat = self.arrangement_cursor_beat;
            self.start_song_playback_at(start_beat, true)?;
            self.begin_song_capture_take(
                self.active_song_start_beat
                    .expect("song start records its normalized beat"),
            );
            self.set_song_transport_mode(SongTransportMode::ArrangementCapture);
            return Ok(SongTransportMode::ArrangementCapture);
        }
        self.start_song_playback_at(self.arrangement_cursor_beat, false)?;
        Ok(SongTransportMode::SongPlayback)
    }

    /// Start the song at an arrangement-timeline beat: save the live session,
    /// preflight, apply the row governing that beat (with an epoch bump — the
    /// transport is stopped), hand the song and beat to the scheduler, then
    /// start the transport.
    fn start_song_playback_at(
        &mut self,
        requested_start_beat: f64,
        open_ended: bool,
    ) -> Result<(), String> {
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
            .ok_or_else(|| {
                format!("Song playback could not start: no row governs beat {start_beat}")
            })?;
        let Some(row) = song.rows.get(row_ordinal).cloned() else {
            return Err("Song playback could not start: the song has no rows".to_string());
        };
        // The song is the only launch authority from here: drop any pending
        // quantized session launches so none fires mid-song.
        let _ = self.state.quantized_launches().cancel_all();
        self.apply_song_row_control(row.scene, &row.overrides, true)?;
        self.state
            .start_song_playback(Arc::clone(&song), start_beat, open_ended)
            .map_err(|error| format!("Song playback could not start: {error}"))?;
        self.active_runtime_song = Some(song);
        self.active_song_start_beat = Some(start_beat);
        self.song_mirrored_row = Some(row_ordinal);
        self.state.start_playback();
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
            SongTransportMode::SessionPlayback => {
                self.state.stop_playback();
                self.set_song_transport_mode(SongTransportMode::Stopped);
                Ok(None)
            }
            SongTransportMode::SongPlayback => {
                let teardown = self.state.stop_song_playback();
                self.active_runtime_song = None;
                self.active_song_start_beat = None;
                self.song_mirrored_row = None;
                // The latch is transient transport state (takes spec 10).
                self.state.clear_song_manual_latch();
                self.state.stop_playback();
                self.set_song_transport_mode(SongTransportMode::Stopped);
                // Hand the live grid back to the scene: the last row played
                // may have silenced lanes it resolved nothing for, and that
                // silencing belongs to the song, not to session mode.
                self.state.resync_live_grid_to_current_scene();
                teardown.map_err(|error| format!("Song playback teardown failed: {error}"))?;
                Ok(Some("Song playback stopped".to_string()))
            }
            SongTransportMode::ArrangementCapture => {
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
                // Unlock the song editing primitives before committing: the
                // commit itself goes through `song_replace`.
                self.set_song_transport_mode(SongTransportMode::Stopped);
                let result = self.finish_song_capture_take(end_raw_beats).map(Some);
                // The latch clears only AFTER the commit: the commit's
                // scene-sync snapshot must still see latched lanes as stale
                // (their live grid holds the performer's launch, not the
                // current scene's pattern) or it writes that content over
                // the scene cell's real pattern.
                self.state.clear_song_manual_latch();
                // Capture ran on top of song playback, so the same row-owned
                // lane state has to be handed back to the scene.
                if playback_teardown.is_some() {
                    self.state.resync_live_grid_to_current_scene();
                }
                if let Some(Err(error)) = playback_teardown {
                    return Err(format!("Song playback teardown failed: {error}"));
                }
                result
            }
        }
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
        self.state.clear_song_manual_latch();
        self.state.stop_playback();
        self.set_song_transport_mode(SongTransportMode::Stopped);
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
        self.apply_song_row_control(row.scene, &row.overrides, false)?;
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
        )?;
        self.graph_controller().apply_sample_ids(&sample_ids);
        let _ = self
            .graph_controller()
            .sync_track_instrument_run_modes_from_live_state();
        self.graph_controller().sync_current_pattern_mod_routes();
        self.push_all_restored_defaults();
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
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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

    /// A song row that resolves nothing for a lane silences it. That is the
    /// song's state, not the scene's: stopping must hand the lane back, or
    /// session mode shows the scene's pattern sitting there unlaunched until
    /// the performer switches scenes and back.
    #[test]
    fn stopping_song_playback_unsilences_lanes_the_last_row_left_empty() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).expect("toggle while stopped");
        app.song_transport_play(false).expect("song playback starts");
        app.apply_song_row_control(Some(0), &[(0, None)], false)
            .expect("sparse row applies");
        assert!(
            app.state.is_scene_silenced(0),
            "an explicit-empty lane is silenced while the row plays"
        );

        app.song_transport_stop().expect("stop succeeds");

        assert!(
            !app.state.is_scene_silenced(0),
            "the scene resolves a pattern for track 0, so its clip is launched again"
        );
    }

    #[test]
    fn play_with_arrangement_off_enters_session_playback() {
        let mut app = test_app();
        let mode = app.song_transport_play(false).expect("play succeeds");
        assert_eq!(mode, SongTransportMode::SessionPlayback);
        assert_eq!(app.song_transport_mode, SongTransportMode::SessionPlayback);
        assert!(app.state.is_playing());
        assert!(!app.song_edits_locked());
        assert_eq!(app.song_transport_mode.binding_str(), "session");

        app.song_transport_stop().expect("stop succeeds");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
    }

    #[test]
    fn play_with_arrangement_off_and_record_stays_session_playback() {
        // Section 1 table: arrangement off + record on = existing
        // pattern/note recording behavior (session playback authority).
        let mut app = test_app();
        let mode = app.song_transport_play(true).expect("play succeeds");
        assert_eq!(mode, SongTransportMode::SessionPlayback);
    }

    #[test]
    fn play_with_arrangement_on_enters_song_playback() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).expect("toggle while stopped");
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
        playback
            .set_use_arrangement(true)
            .expect("toggle while stopped");
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
        capture
            .set_use_arrangement(true)
            .expect("toggle while stopped");
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
    fn play_with_arrangement_on_and_no_song_plays_the_empty_arrangement() {
        // Empty-arrangement spec 4.3: the arrangement always exists, so Play
        // in arrangement mode starts even on a never-seeded project — it
        // plays the empty arrangement (silence), Ableton-style, and installs
        // it on the way.
        let mut app = test_app();
        app.set_use_arrangement(true).expect("toggle while stopped");
        let mode = app.song_transport_play(false).expect("empty song plays");
        assert_eq!(mode, SongTransportMode::SongPlayback);
        assert!(app.state.is_playing());
        let arrangement = app
            .state
            .committed_arrangement()
            .expect("play installed the empty arrangement");
        assert!(arrangement.is_empty());
        app.song_transport_stop().expect("stops cleanly");
        app.state.set_scheduler_rendered_beats(0.0);
    }

    #[test]
    fn play_with_arrangement_and_record_enters_capture_on_top_of_playback() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).expect("toggle while stopped");
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
        app.set_use_arrangement(true).unwrap();
        app.song_transport_play(false).expect("song playback");
        assert!(!app.take_recording_active());
        assert!(app.promote_song_playback_to_capture(), "promotion happens");
        assert_eq!(app.song_transport_mode, SongTransportMode::ArrangementCapture);
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
    fn session_playback_recording_is_left_alone() {
        // Arrangement off: record + play is ordinary live pattern recording.
        let mut app = app_with_song();
        app.song_transport_play(true).expect("session playback");
        assert!(!app.promote_song_playback_to_capture());
        assert_eq!(app.song_transport_mode, SongTransportMode::SessionPlayback);
        assert!(!app.take_recording_active());
        app.song_transport_stop().unwrap();
    }

    #[test]
    fn play_while_playing_is_rejected_in_every_mode() {
        let mut app = app_with_song();
        for (use_arrangement, record) in [(false, false), (true, false), (true, true)] {
            app.set_use_arrangement(use_arrangement).unwrap();
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
    fn use_arrangement_toggle_is_rejected_while_playing() {
        let mut app = app_with_song();
        for (use_arrangement, record) in [(false, false), (true, false), (true, true)] {
            app.set_use_arrangement(use_arrangement).unwrap();
            app.song_transport_play(record).expect("play succeeds");
            let error = app
                .set_use_arrangement(!use_arrangement)
                .expect_err("toggle while playing must fail");
            assert_eq!(error, USE_ARRANGEMENT_WHILE_PLAYING_ERROR);
            assert_eq!(app.use_arrangement, use_arrangement, "state unchanged");
            // Zero-length capture commits fail; the transport still stops.
            let _ = app.song_transport_stop();
            assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        }
        // While stopped the toggle works and selects the next Play.
        app.set_use_arrangement(true).expect("toggle while stopped");
        assert!(app.use_arrangement);
    }

    #[test]
    fn manual_launches_latch_during_song_playback_and_back_to_song_clears() {
        let mut app = app_with_song();
        assert!(app.manual_launch_rejection().is_none());
        app.set_use_arrangement(true).unwrap();
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
        // The latch is transient transport state: stop clears it.
        app.song_transport_stop().expect("stop succeeds");
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
        app.set_use_arrangement(true).unwrap();
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
        app.set_use_arrangement(true).unwrap();
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
        app.set_use_arrangement(true).unwrap();
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

        app.song_transport_play(false).expect("session playback");
        let error = app
            .song_capture_cancel()
            .expect_err("cancel during session playback must fail");
        assert!(error.contains("only valid during arrangement capture"), "{error}");
        assert_eq!(app.song_transport_mode, SongTransportMode::SessionPlayback);
        app.song_transport_stop().unwrap();

        app.set_use_arrangement(true).unwrap();
        let song_before = app.state.committed_song();
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
        app.set_use_arrangement(true).unwrap();
        app.song_transport_toggle_play(false).expect("toggle starts");
        assert_eq!(app.song_transport_mode, SongTransportMode::SongPlayback);
        app.song_transport_toggle_play(false).expect("toggle stops");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
    }

    #[test]
    fn song_edit_primitives_are_available_in_playback_but_locked_during_capture() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).unwrap();
        app.song_transport_play(false).expect("play succeeds");
        app.arr_set_loop(true)
            .expect("song playback allows arrangement edits");
        let _ = app.song_transport_stop();

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
        app.set_use_arrangement(true).unwrap();
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
        app.set_use_arrangement(true).unwrap();
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
        app.set_use_arrangement(true).unwrap();
        app.song_transport_play(false).expect("song playback");
        let message = app.handle_song_playback_start_failed("boom");
        assert!(message.contains("boom"), "{message}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
    }

    #[test]
    fn song_start_flow_sends_the_scheduler_start_command() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).unwrap();
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
        app.set_use_arrangement(true).expect("toggle while stopped");
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
        app.set_use_arrangement(true).expect("toggle while stopped");
        app.state.set_scheduler_rendered_beats(0.0);
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
    fn capture_into_an_empty_arrangement_splices_from_the_first_launch() {
        // Empty-arrangement spec 6: capture is one code path — a [P, Q)
        // splice into the arrangement that exists, the empty one included.
        // A track launch at the capture origin opens a clip there; the
        // canvas keeps its default length and the scene lane stays empty
        // (a track launch is not a scene change).
        let mut app = app_with_song();
        app.arr_clear().expect("start from an empty arrangement");

        start_capture(&mut app);
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
        assert!(
            arrangement.scene_lane.is_empty(),
            "a track launch writes no scene event"
        );
        assert_eq!(arrangement.track_lanes[0].len(), 1);
        let clip = arrangement.track_lanes[0][0];
        assert_eq!(clip.start_beat, 0.0);
        assert_eq!(clip.end_beat, 8.0);
        assert_eq!(clip.pattern_id, Some(2), "scene 1's cell for track 0");
        assert_eq!(
            arrangement.end_beat,
            crate::sequencer::DEFAULT_ARRANGEMENT_END,
            "the empty canvas keeps its default length"
        );

        let song = committed(&app);
        assert_eq!(song.rows[0].start_beat, 0.0);
        assert_eq!(song.rows[0].scene, None, "the span is unscened");
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
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
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
        app.set_use_arrangement(true).expect("toggle while stopped");
        app.state.set_scheduler_rendered_beats(0.0);
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
    fn capture_scene_launch_preserves_take_lanes() {
        // Takes spec 10 refined: a scene launch during arrangement capture
        // must NOT claim lanes that are playing takes — only an intentional
        // clip launch does. The take lane neither latches audibly nor gets
        // spliced over at commit.
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
            0b10,
            "the scene launch latches the pattern lane but not the take lane"
        );
        app.state.set_scheduler_rendered_beats(8.0);
        app.song_transport_stop().expect("stop commits");
        let song = committed(&app);
        let spliced = song
            .rows
            .iter()
            .find(|row| row.start_beat == 4.0)
            .expect("spliced row at the launch beat");
        let track0 = spliced
            .overrides
            .iter()
            .find(|over| over.track == 0)
            .expect("take lane materialized in the spliced row");
        assert_eq!(
            track0.take_id,
            Some(take_id.0),
            "the take survives the scene-launch capture"
        );
        assert!(
            song.rows
                .iter()
                .filter(|row| row.start_beat >= 4.0 && row.start_beat < 8.0)
                .all(|row| row
                    .overrides
                    .iter()
                    .any(|over| over.track == 0 && over.take_id == Some(take_id.0))),
            "no spliced row replaces the take with a scene pattern"
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

    #[test]
    fn binding_strings_cover_every_mode() {
        assert_eq!(SongTransportMode::Stopped.binding_str(), "session");
        assert_eq!(SongTransportMode::SessionPlayback.binding_str(), "session");
        assert_eq!(SongTransportMode::SongPlayback.binding_str(), "song-playback");
        assert_eq!(
            SongTransportMode::ArrangementCapture.binding_str(),
            "arrangement-capture"
        );
    }
}
