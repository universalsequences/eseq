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
//! Slice C (performance capture) hooks the marked `Slice C: capture staging
//! hook` seams: mode entry (`song_transport_play` capture branch), the
//! Stop-commit point (`song_transport_stop`), and Cancel
//! (`song_capture_cancel`). Audible-launch observation for capture belongs in
//! `App::apply_pattern_launch` (src/app/mod.rs), and capture must refuse to
//! commit when `state.song_playback().take_notice_overflow()` reports lost
//! notices (spec 10.3).

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
        // Song edits are locked while a song-authority mode is active
        // (spec 5.6/13); `song_edit.rs` reads this flag.
        self.song_transport_locks_edits = matches!(
            mode,
            SongTransportMode::SongPlayback | SongTransportMode::ArrangementCapture
        );
    }

    /// Whether the transport is playing from the state machine's viewpoint:
    /// either a mode is active or the raw transport atomic is set (legacy
    /// paths may still start session playback without entering the machine).
    fn transport_engaged(&self) -> bool {
        self.song_transport_mode != SongTransportMode::Stopped || self.state.is_playing()
    }

    /// Spec 7.3: while `SongPlayback` is active the song is the only launch
    /// authority; every manual scene/track-pattern launch entry point checks
    /// this and rejects with the same message.
    pub fn manual_launch_rejection(&self) -> Option<&'static str> {
        (self.song_transport_mode == SongTransportMode::SongPlayback)
            .then_some(MANUAL_LAUNCH_DURING_SONG_ERROR)
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
            // Arrangement capture (spec 7.4): transport starts at beat zero,
            // the committed song is NOT played, and the performer keeps the
            // session launch controls.
            // Slice C: capture staging hook — begin the staging take here by
            // capturing the resolved session state as the beat-zero row.
            self.state.start_playback();
            self.set_song_transport_mode(SongTransportMode::ArrangementCapture);
            return Ok(SongTransportMode::ArrangementCapture);
        }
        self.start_song_playback_from_zero()?;
        Ok(SongTransportMode::SongPlayback)
    }

    /// The documented song start flow (spec 7.3 / state/song_playback.rs):
    /// save the live session into the current scene, preflight, apply row
    /// zero (with an epoch bump — the transport is stopped), hand the song to
    /// the scheduler, then start the transport.
    fn start_song_playback_from_zero(&mut self) -> Result<(), String> {
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
        let Some(row0) = song.rows.first().cloned() else {
            return Err("Song playback could not start: the song has no rows".to_string());
        };
        // The song is the only launch authority from here: drop any pending
        // quantized session launches so none fires mid-song.
        let _ = self.state.quantized_launches().cancel_all();
        self.apply_song_row_control(row0.scene, &row0.overrides, true)?;
        self.state
            .start_song_playback(Arc::clone(&song), 0.0)
            .map_err(|error| format!("Song playback could not start: {error}"))?;
        self.active_runtime_song = Some(song);
        self.song_mirrored_row = Some(0);
        self.state.start_playback();
        self.set_song_transport_mode(SongTransportMode::SongPlayback);
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
                self.song_mirrored_row = None;
                self.state.stop_playback();
                self.set_song_transport_mode(SongTransportMode::Stopped);
                teardown.map_err(|error| format!("Song playback teardown failed: {error}"))?;
                Ok(Some("Song playback stopped".to_string()))
            }
            SongTransportMode::ArrangementCapture => {
                // Slice C: capture staging hook — Stop-commit point: validate
                // and normalize the staging take, then atomically replace the
                // committed song (refusing on notice overflow via
                // `state.song_playback().take_notice_overflow()`). Slice B
                // has no take: stop and report that nothing was committed.
                self.state.stop_playback();
                self.set_song_transport_mode(SongTransportMode::Stopped);
                Ok(Some(
                    "Arrangement capture stopped; nothing was committed (capture lands in a \
                     later slice)"
                        .to_string(),
                ))
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
        // Slice C: capture staging hook — discard the staging take here.
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
        if self.song_transport_mode != SongTransportMode::SongPlayback {
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
        // The start flow already applied row zero; skip the duplicate initial
        // notice but always mirror loop wraps (they re-enter row zero).
        if !notice.wrapped && self.song_mirrored_row == Some(notice.row_ordinal) {
            return Ok(());
        }
        self.apply_song_row_control(row.scene, &row.overrides, false)?;
        self.song_mirrored_row = Some(notice.row_ordinal);
        Ok(())
    }

    /// Scheduler reached `end_beat` with looping disabled (spec 7.3.5): stop
    /// through the state machine.
    pub fn handle_song_playback_ended(&mut self) -> Result<Option<String>, String> {
        if self.song_transport_mode != SongTransportMode::SongPlayback {
            return Ok(None);
        }
        self.song_transport_stop()
            .map(|_| Some("Song ended".to_string()))
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
        scene: usize,
        overrides: &[(usize, PatternId)],
        bump_pattern_epoch: bool,
    ) -> Result<(), String> {
        if scene != self.state.current_scene_index() {
            self.switch_bus_pattern(scene);
        }
        let sample_ids = self.state.apply_song_row(
            scene,
            overrides,
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
            bump_pattern_epoch,
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
    use crate::app::{AudioBuses, PatternLaunchError};
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

    /// Three-row song: scenes 0/1/2 at beats 0/4/8, end beat 16.
    fn app_with_song() -> App {
        let mut app = test_app();
        app.song_replace(
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
        .expect("song_replace succeeds");
        app
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
        assert!(app.song_edits_locked(), "song edits lock during playback");
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
    fn play_with_arrangement_on_and_no_song_fails_without_starting() {
        let mut app = test_app();
        app.set_use_arrangement(true).expect("toggle while stopped");
        let error = app
            .song_transport_play(false)
            .expect_err("empty song must not play");
        assert!(error.contains("no committed song"), "{error}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert!(!app.song_edits_locked());
    }

    #[test]
    fn play_with_arrangement_and_record_enters_capture_without_playing_song() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).expect("toggle while stopped");
        let song_before = app.state.committed_song();
        let mode = app.song_transport_play(true).expect("capture starts");
        assert_eq!(mode, SongTransportMode::ArrangementCapture);
        assert!(app.state.is_playing());
        assert!(app.song_edits_locked(), "song edits lock during capture");
        assert!(
            app.active_runtime_song.is_none(),
            "capture must not play the committed song"
        );
        assert_eq!(app.song_transport_mode.binding_str(), "arrangement-capture");
        assert!(
            app.manual_launch_rejection().is_none(),
            "the performer stays launch authority during capture"
        );

        let status = app.song_transport_stop().expect("stop succeeds");
        assert!(
            status.unwrap().contains("nothing was committed"),
            "Slice B stop must report that no take was committed"
        );
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert_eq!(
            app.state.committed_song(),
            song_before,
            "the committed song is untouched"
        );
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
            app.song_transport_stop().expect("stop succeeds");
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
            app.song_transport_stop().expect("stop succeeds");
        }
        // While stopped the toggle works and selects the next Play.
        app.set_use_arrangement(true).expect("toggle while stopped");
        assert!(app.use_arrangement);
    }

    #[test]
    fn manual_launches_are_rejected_only_during_song_playback() {
        let mut app = app_with_song();
        assert!(app.manual_launch_rejection().is_none());
        app.set_use_arrangement(true).unwrap();
        app.song_transport_play(false).expect("song playback");
        assert_eq!(
            app.manual_launch_rejection(),
            Some(MANUAL_LAUNCH_DURING_SONG_ERROR)
        );
        let error = app
            .apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect_err("manual launch during song playback must fail");
        assert_eq!(error, PatternLaunchError::SongPlaybackActive);
        assert!(
            app.drain_due_pattern_launches().is_empty(),
            "quantized launches are dropped during song playback"
        );
        app.song_transport_stop().expect("stop succeeds");
        assert!(app.manual_launch_rejection().is_none());
        app.apply_manual_pattern_launch(&PatternLaunchTarget::Scene { scene: 1 })
            .expect("manual launch works again after stop");
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
        let status = app.song_capture_cancel().expect("cancel succeeds");
        assert!(status.contains("take discarded"), "{status}");
        assert_eq!(app.song_transport_mode, SongTransportMode::Stopped);
        assert!(!app.state.is_playing());
        assert_eq!(app.state.committed_song(), song_before);
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
    fn song_edit_primitives_are_locked_in_song_modes_and_unlocked_after_stop() {
        let mut app = app_with_song();
        app.set_use_arrangement(true).unwrap();
        for record in [false, true] {
            app.song_transport_play(record).expect("play succeeds");
            let error = app.song_set_loop(true).expect_err("edits must be locked");
            assert_eq!(error, crate::app::song_edit::SONG_EDITS_LOCKED_ERROR);
            app.song_transport_stop().expect("stop succeeds");
            app.song_set_loop(app.state.committed_song().unwrap().loop_enabled)
                .expect("no-op edit succeeds when unlocked");
        }
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
