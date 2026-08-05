//! Take recording (takes spec section 8).
//!
//! While arrangement capture is active, note input on ARMED tracks is
//! retargeted from the live-pattern write path into a per-track pending take
//! (`ui/input.rs` calls `App::take_record_note`). The pending take is minted
//! lazily at the first recorded note (punch-in, spec 8.3), notes land at
//! clip-relative positions `steps(beat - P)` stamped on the
//! latency-compensated record clock (spec 8.4), and chunk rollover extends
//! the pending buffers. Content lives in DETACHED `TrackPatternData` buffers
//! until commit — cancel is a plain drop, and the pattern pool never holds
//! unregistered chunks.
//!
//! Commit happens inside the capture stop-commit (`song_capture.rs`):
//! pending lanes register as takes and one take clip per lane is painted over
//! `[P, Q)`, in the same single undo entry as the launch splice (spec 8.5).

use crate::record_quantize::RecordQuantize;
use crate::sequencer::{
    PatternSnapshot, SoundRefs, StepParam, TakeId, TrackPatternData, MAX_STEPS,
};

use super::song_transport::SongTransportMode;
use super::App;

/// One armed track's in-flight take (spec 8.3/8.4).
///
/// `Clone` exists for one reason: the stop-commit keeps a restore copy so a
/// failed commit hands the performance back instead of destroying it.
#[derive(Clone)]
pub(crate) struct PendingTakeLane {
    /// Punch-in beat `P` on the arrangement timeline, aligned per the
    /// quantize policy at the first note.
    pub(crate) punch_in_beat: f64,
    /// Beats per chunk-domain step (the track's base timebase at the
    /// punch-in moment; chunks are `MAX_STEPS`-long patterns).
    pub(crate) step_beats: f64,
    /// Chunk content under construction. Detached from the pattern pool
    /// until commit.
    pub(crate) chunks: Vec<TrackPatternData>,
    /// Cleared template used to mint rollover chunks.
    template: TrackPatternData,
    /// The bound cell's sound at punch-in (§17.3 "take record → share"):
    /// recording performs the current sound rather than minting a new one,
    /// so the committed take references this pair instead of cloning it.
    sound: Option<SoundRefs>,
    /// Furthest written step end (note-on + duration), in take steps:
    /// finalizes `total_len_steps` (spec 8.5, release tail included).
    pub(crate) max_end_steps: f64,
}

/// Per-capture take recording session: one optional pending lane per track.
#[derive(Clone)]
pub struct TakeRecordingSession {
    /// Arrangement beat corresponding to scheduler/record-clock beat zero.
    /// Shared with launch capture so notes and spliced rows use one domain.
    timeline_start_beat: f64,
    lanes: Vec<Option<PendingTakeLane>>,
}

impl TakeRecordingSession {
    pub(crate) fn new(timeline_start_beat: f64, track_count: usize) -> Self {
        Self {
            timeline_start_beat,
            lanes: (0..track_count).map(|_| None).collect(),
        }
    }

    pub(crate) fn has_pending_content(&self) -> bool {
        self.lanes
            .iter()
            .any(|lane| lane.as_ref().is_some_and(|lane| lane.max_end_steps > 0.0))
    }

    /// Borrowing view of the lanes that have punched in, for the provisional
    /// read surface (docs/realtime-arrangement-feedback-spec.md 3.2). Lanes
    /// with no content yet are omitted: a punched-in-but-silent lane has
    /// nothing to draw.
    pub(crate) fn pending_lane_views(&self) -> Vec<super::pending_capture::PendingCaptureLane<'_>> {
        self.lanes
            .iter()
            .enumerate()
            .filter_map(|(track, lane)| {
                let lane = lane.as_ref().filter(|lane| lane.max_end_steps > 0.0)?;
                Some(super::pending_capture::PendingCaptureLane {
                    track,
                    punch_in_beat: lane.punch_in_beat,
                    step_beats: lane.step_beats,
                    max_end_steps: lane.max_end_steps,
                    chunks: &lane.chunks,
                })
            })
            .collect()
    }

    /// Drain the lanes that actually recorded content.
    pub(crate) fn into_pending(self) -> Vec<(usize, PendingTakeLane)> {
        self.lanes
            .into_iter()
            .enumerate()
            .filter_map(|(track, lane)| {
                lane.filter(|lane| lane.max_end_steps > 0.0)
                    .map(|lane| (track, lane))
            })
            .collect()
    }
}

/// Quantization grid for a take lane, expressed in the lane's own chunk-domain
/// steps (spec 8.3/8.4). `Sixteenth` means "the track's own step grid" (note
/// positions round to whole steps), and every coarser grid is rounded to a
/// whole number of steps so that punch-in `P` and the note positions inside the
/// clip snap to ONE grid even when the track's timebase is not commensurate
/// with the record-quantize grid (e.g. 1/16 quantize on a 1/8 track).
/// `None` for `Off`, which preserves the performed sub-step phase.
fn take_grid_steps(quantize: RecordQuantize, step_beats: f64) -> Option<f64> {
    let grid_beats = quantize.grid_beats()?;
    if !(step_beats > 0.0) {
        return None;
    }
    match quantize {
        RecordQuantize::Sixteenth => Some(1.0),
        _ => Some((grid_beats / step_beats).round().max(1.0)),
    }
}

/// A registered-and-finalized pending lane, ready to be painted as a clip.
pub(crate) struct CommittedTakeLane {
    pub(crate) track: usize,
    pub(crate) take_id: TakeId,
    pub(crate) punch_in_beat: f64,
    pub(crate) punch_out_beat: f64,
    pub(crate) step_beats: f64,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::app::edit::undo;
    use crate::app::song_edit::SongRowSpec;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, SequencerState,
    };
    use std::sync::atomic::Ordering;

    fn test_app() -> App {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
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
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app
    }

    /// App with a two-row committed song (0.0 scene 0, 8.0 scene 1, end 16)
    /// in ArrangementCapture, plus the record-clock anchor instant for beat
    /// 0.0. Presses address beats as FUTURE instants relative to the anchor
    /// (the monotonic test-clock origin cannot represent the past).
    fn capture_app() -> (App, Instant) {
        let mut app = test_app();
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
        .expect("arr_replace_rows succeeds");
        app.begin_song_capture_take(0.0);
        app.song_transport_mode = SongTransportMode::ArrangementCapture;
        // First publish initializes the monotonic clock origin; the real
        // anchor sits 1 ms later so its origin-relative timestamp is
        // non-zero (a zero anchor reads as "no anchor yet").
        let now = Instant::now();
        app.state.transport.record_clock.publish(0.0, now);
        let anchor = now
            .checked_add(Duration::from_millis(1))
            .expect("anchor instant");
        app.state.transport.record_clock.publish(0.0, anchor);
        // Unquantized recording: sub-step phase must survive as note delay.
        app.state.transport.record_quantize.store(
            crate::record_quantize::RecordQuantize::Off as u32,
            Ordering::Relaxed,
        );
        (app, anchor)
    }

    /// 120 BPM default: one beat = 0.5 s after the anchor.
    fn press_at_beats(anchor: Instant, beats: f64) -> Instant {
        anchor
            .checked_add(Duration::from_secs_f64(beats * 0.5))
            .expect("press instant")
    }

    #[test]
    fn take_notes_land_at_latency_compensated_clip_positions() {
        let (mut app, anchor) = capture_app();
        // 20 ms of output latency at 120 BPM = 0.04 beats: the performer
        // heard beat 12.1 while the raw clock read 12.14.
        app.state
            .transport
            .record_latency_seconds
            .store(0.02_f32.to_bits(), Ordering::Relaxed);
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.14), 60.0, 2.0));
        // Second note: heard at beat 14.35 -> take step 9 with 0.4 delay.
        assert!(app.take_record_note(0, press_at_beats(anchor, 14.39), 64.0, 1.0));

        let session = app.take_recording.as_ref().expect("session active");
        let lane = session.lanes[0].as_ref().expect("lane punched in");
        // Punch-in: heard beat 12.1 floored to the 16th grid = 12.0; the
        // 0.1-beat remainder (0.4 steps) lives as the note's delay.
        assert!((lane.punch_in_beat - 12.0).abs() < 1e-6, "{}", lane.punch_in_beat);
        let chunk = &lane.chunks[0];
        assert!(chunk.track_bits[0] & 1 == 1, "step 0 active");
        assert!((chunk.chord_snapshot.delays[0][0] - 0.4).abs() < 1e-4);
        assert_eq!(chunk.chord_snapshot.steps[0], vec![60.0]);
        // (14.35 - 12.0) / 0.25 = 9.4 -> step 9, delay 0.4.
        assert!(chunk.track_bits[0] >> 9 & 1 == 1, "step 9 active");
        assert!((chunk.chord_snapshot.delays[9][0] - 0.4).abs() < 1e-4);
        assert_eq!(chunk.chord_snapshot.steps[9], vec![64.0]);
        // Live pattern untouched: retargeting bypassed the live write path.
        assert!(!app.state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn take_notes_add_the_mid_song_transport_start_to_record_clock_beats() {
        let (mut app, anchor) = capture_app();
        app.begin_song_capture_take(8.0);
        assert!(app.take_record_note(0, press_at_beats(anchor, 2.1), 60.0, 1.0));

        let session = app.take_recording.as_ref().expect("session active");
        let lane = session.lanes[0].as_ref().expect("lane punched in");
        assert!(
            (lane.punch_in_beat - 10.0).abs() < 1e-6,
            "raw beat 2.1 from an arrangement start at 8 must punch in on \
             the beat-10 sixteenth grid, got {}",
            lane.punch_in_beat
        );
    }

    #[test]
    fn take_notes_stamp_at_press_time_not_release_time() {
        // The live path resolves take positions at key RELEASE, passing the
        // press instant — which by then predates the newest record-clock
        // anchor (republished every audio block). The stamp must extrapolate
        // BACKWARDS to the press beat; clamping to the anchor would land the
        // note (and the punch-in) at the release instant, shifting the whole
        // clip late by the first note's hold time.
        let (mut app, anchor) = capture_app();
        // The newest anchor sits at beat 20 (10 s after beat zero) — the
        // audio callback kept publishing while the note was held.
        app.state
            .transport
            .record_clock
            .publish(20.0, press_at_beats(anchor, 20.0));
        // The note was PRESSED back at beat 12.1.
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.1), 60.0, 2.0));
        let session = app.take_recording.as_ref().expect("session active");
        let lane = session.lanes[0].as_ref().expect("lane punched in");
        assert!(
            (lane.punch_in_beat - 12.0).abs() < 1e-6,
            "punch-in must stay at the press beat, got {}",
            lane.punch_in_beat
        );
        let chunk = &lane.chunks[0];
        assert!(chunk.track_bits[0] & 1 == 1, "note lands at take step 0");
    }

    #[test]
    fn quantized_punch_in_snaps_to_the_tracks_own_step_grid() {
        // 1/16 record quantize on a 1/8 track: the note grid inside the clip
        // is the track's STEP grid, so the punch-in has to use the same grid.
        // Snapping P to an absolute 0.25-beat grid would start the clip a
        // quarter-step off the track's timebase and shift every take note.
        let (mut app, anchor) = capture_app();
        app.state.transport.record_quantize.store(
            RecordQuantize::Sixteenth as u32,
            Ordering::Relaxed,
        );
        app.state.with_scenes_mut(|scenes| {
            let pool = scenes.track_pools.get_mut(0).expect("track pool");
            for pattern in pool.patterns.values_mut().map(Arc::make_mut) {
                pattern.seq.params.timebase = crate::sequencer::Timebase::Eighth;
            }
        });
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.3), 60.0, 1.0));
        let session = app.take_recording.as_ref().expect("session active");
        let lane = session.lanes[0].as_ref().expect("lane punched in");
        assert!((lane.step_beats - 0.5).abs() < 1e-9, "{}", lane.step_beats);
        assert!(
            (lane.punch_in_beat - 12.5).abs() < 1e-6,
            "punch-in must land on the track's 1/8 step grid, got {}",
            lane.punch_in_beat
        );
        // The note itself lands on take step 0, i.e. exactly at P.
        assert!(lane.chunks[0].track_bits[0] & 1 == 1, "note at take step 0");
    }

    #[test]
    fn chunk_rollover_extends_pending_chunks() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 4.0), 60.0, 1.0));
        // 256 steps past the punch-in = 64 beats later: chunk 1.
        assert!(app.take_record_note(0, press_at_beats(anchor, 4.0 + 64.0 + 0.5), 62.0, 1.0));
        let session = app.take_recording.as_ref().expect("session");
        let lane = session.lanes[0].as_ref().expect("lane");
        assert_eq!(lane.chunks.len(), 2);
        assert!(lane.chunks[1].track_bits[0] & (1 << 2) != 0, "chunk 1 step 2");
    }

    #[test]
    fn stop_commit_registers_takes_and_splices_rows_in_one_entry() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.1), 60.0, 2.0));
        assert!(app.take_record_note(0, press_at_beats(anchor, 14.35), 64.0, 1.0));
        let depth = app.history.undo_len();
        app.song_transport_mode = SongTransportMode::Stopped;
        let status = app
            .finish_song_capture_take(40.0)
            .expect("commit succeeds");
        assert!(status.contains("1 take(s)"), "{status}");

        // Take: last note-on at step 9 + duration 1 (+0.4 delay) -> ceil
        // 10.4 = 11 steps; Q = 12.0 + 11 * 0.25 = 14.75.
        let takes = app.state.track_takes(0);
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0].total_len_steps, 11);
        assert_eq!(takes[0].chunks.len(), 1);

        let song = app.state.committed_song().expect("song");
        let take_rows: Vec<(f64, f64)> = song
            .rows
            .iter()
            .filter_map(|row| {
                row.overrides
                    .iter()
                    .find(|over| over.track == 0 && over.take_id.is_some())
                    .map(|over| (row.start_beat, over.offset_steps))
            })
            .collect();
        assert_eq!(take_rows, vec![(12.0, 0.0)], "one take row at the punch-in");
        // The restore row at Q hands the lane back to the scene cell.
        assert!(song
            .rows
            .iter()
            .any(|row| (row.start_beat - 14.75).abs() < 1e-9));
        assert_eq!(song.end_beat, 16.0, "no extension needed");
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");

        // One undo removes the take, its chunks, and the spliced rows.
        undo(&mut app);
        assert!(app.state.track_takes(0).is_empty());
        let song = app.state.committed_song().expect("song");
        assert_eq!(song.rows.len(), 2);
        // Every lane still states its resolution explicitly (spec 6.2/7);
        // what undo restored is that none of them plays a take any more.
        assert!(song
            .rows
            .iter()
            .all(|row| row.overrides.iter().all(|over| over.take_id.is_none())));
    }

    /// §17.3 "take record → share": punch-in stores the bound cell's refs,
    /// so the committed take references the scene's Patch/Mix pair instead
    /// of minting a private clone — and an entity edit made through either
    /// referent is heard by both.
    /// Two-track app mirroring `capture_app`'s song shape.
    fn two_track_app() -> App {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.replace_pattern_repository(
            vec![
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
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
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
        .expect("arr_replace_rows succeeds");
        app.state.transport.record_quantize.store(
            crate::record_quantize::RecordQuantize::Off as u32,
            Ordering::Relaxed,
        );
        app
    }

    /// User repro: record a take on track 2, stop-commit, restart, record a
    /// take on track 1, stop-commit. The committed arrangement and take
    /// pools must stay mutually consistent — the field failure was "Track 1
    /// clip 1 references take 0 which is not in track 1's take pool", which
    /// bricks the transport (preflight validation fails on every Play).
    #[test]
    fn sequential_single_track_captures_keep_take_references_valid() {
        let mut app = two_track_app();

        // Capture session 1: take on track index 1.
        app.begin_song_capture_take(0.0);
        app.song_transport_mode = SongTransportMode::ArrangementCapture;
        let now = Instant::now();
        app.state.transport.record_clock.publish(0.0, now);
        let anchor = now
            .checked_add(Duration::from_millis(1))
            .expect("anchor instant");
        app.state.transport.record_clock.publish(0.0, anchor);
        assert!(app.take_record_note(1, press_at_beats(anchor, 5.0), 60.0, 2.0));
        app.song_transport_mode = SongTransportMode::Stopped;
        app.finish_song_capture_take(12.0).expect("commit 1");
        assert_eq!(app.state.track_takes(1).len(), 1);

        // Capture session 2: take on track index 0, with a splice window
        // [1, 14) that spans the FIRST take's clip — the untouched-lane
        // inheritance must re-materialize track 2's take inside the splice.
        app.begin_song_capture_take(0.0);
        app.song_transport_mode = SongTransportMode::ArrangementCapture;
        assert!(app.take_record_note(0, press_at_beats(anchor, 1.0), 60.0, 2.0));
        app.song_transport_mode = SongTransportMode::Stopped;
        app.finish_song_capture_take(14.0).expect("commit 2");

        // Both takes exist in their own pools and every reference resolves:
        // the state must preflight (this is what Play runs) and the
        // committed arrangement must validate.
        assert_eq!(app.state.track_takes(0).len(), 1, "track 1's take pool");
        assert_eq!(app.state.track_takes(1).len(), 1, "track 2's take pool");
        app.state
            .preflight_runtime_song()
            .expect("the committed state must stay playable");
    }

    /// The same repro through the FULL transport path — the arrangement is
    /// itself created by a first capture (with the silent-start auto-latch),
    /// and both take sessions run on top of live song playback with the
    /// stop boundary read from the record clock, exactly as in the app.
    #[test]
    fn full_transport_sequential_take_captures_stay_playable() {
        let mut app = two_track_app();
        app.arr_clear().expect("start from an empty arrangement");
        app.set_arrangement_view_visible(true);

        let clock_zero = Instant::now();
        app.state.transport.record_clock.publish(0.0, clock_zero);
        let mut anchor = clock_zero
            .checked_add(Duration::from_millis(1))
            .expect("anchor instant");

        // Capture A: creates the short arrangement (the auto-latched scene
        // is the captured initial state; no other launches).
        app.state.transport.record_clock.publish(0.0, anchor);
        app.song_transport_play(true).expect("capture A starts");
        app.state
            .transport
            .record_clock
            .publish(8.0, press_at_beats(anchor, 8.0));
        app.song_transport_stop().expect("capture A commits");

        // Capture B: restart playback from the parked cursor at beat 5, then
        // engage recording MID-PLAYBACK (the promote path), take on track
        // index 1. Raw record-clock beats are transport-relative; the
        // timeline offset is the mid-song start.
        anchor = anchor
            .checked_add(Duration::from_secs(10))
            .expect("anchor instant");
        app.state.transport.record_clock.publish(0.0, anchor);
        app.set_arrangement_cursor(5.0, 0);
        app.song_transport_play(false).expect("playback restarts");
        app.stamp_recording_kind_for_note();
        assert_eq!(
            app.song_transport_mode,
            SongTransportMode::ArrangementCapture,
            "recording engaged mid-playback promotes into capture"
        );
        assert!(app.take_record_note(1, press_at_beats(anchor, 0.5), 60.0, 2.0));
        app.state
            .transport
            .record_clock
            .publish(7.0, press_at_beats(anchor, 7.0));
        app.song_transport_stop().expect("capture B commits");
        assert_eq!(app.state.track_takes(1).len(), 1);

        // Capture C: restart from beat 0 the same way, take on track index
        // 0, splice spanning B's clip.
        anchor = anchor
            .checked_add(Duration::from_secs(10))
            .expect("anchor instant");
        app.state.transport.record_clock.publish(0.0, anchor);
        app.set_arrangement_cursor(0.0, 0);
        app.song_transport_play(false).expect("playback restarts");
        app.stamp_recording_kind_for_note();
        assert!(app.take_record_note(0, press_at_beats(anchor, 1.0), 60.0, 2.0));
        app.state
            .transport
            .record_clock
            .publish(14.0, press_at_beats(anchor, 14.0));
        app.song_transport_stop().expect("capture C commits");

        assert_eq!(app.state.track_takes(0).len(), 1, "track 1's take pool");
        assert_eq!(app.state.track_takes(1).len(), 1, "track 2's take pool");
        app.state
            .preflight_runtime_song()
            .expect("the committed state must stay playable");
    }

    #[test]
    fn punch_in_shares_the_bound_cells_sound_refs() {
        let (mut app, anchor) = capture_app();
        let expected = app
            .state
            .with_project_scenes(|scenes| scenes.effective_sound_refs(0))
            .expect("effective refs resolve");
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.1), 60.0, 2.0));
        app.song_transport_mode = SongTransportMode::Stopped;
        app.finish_song_capture_take(40.0).expect("commit succeeds");

        let takes = app.state.track_takes(0);
        assert_eq!(takes.len(), 1);
        assert_eq!(
            takes[0].sound, expected,
            "the take shares the bound cell's Patch/Mix (takes spec 17.3)"
        );
        // Sharing is structural: writing the scene pattern's entities is
        // heard through the take's chunks with no fan-out anywhere.
        let scene_pattern = app
            .state
            .effective_track_pattern_id(0)
            .expect("scene pattern");
        let chunk = takes[0].chunks[0];
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(scene_pattern, |data| {
                data.instrument_base_note_offset = 5.0;
            }));
            let heard = scenes.track_pools[0].get(chunk).expect("chunk resolves");
            assert_eq!(heard.instrument_base_note_offset.to_bits(), 5.0f32.to_bits());
        });
    }

    #[test]
    fn recording_past_the_song_end_extends_it() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 15.5), 60.0, 8.0));
        app.song_transport_mode = SongTransportMode::Stopped;
        app.finish_song_capture_take(40.0).expect("commit succeeds");
        let song = app.state.committed_song().expect("song");
        // P = 15.5, 8 duration steps -> total 8 steps, Q = 15.5 + 2.0 = 17.5.
        assert!((song.end_beat - 17.5).abs() < 1e-9, "{}", song.end_beat);
    }

    #[test]
    fn cancel_discards_pending_takes_without_touching_pools() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 2.0), 60.0, 1.0));
        let pool_len = app
            .state
            .with_project_scenes(|scenes| scenes.track_pools[0].patterns.len());
        app.discard_song_capture_take();
        assert!(app.take_recording.is_none());
        assert!(app.state.track_takes(0).is_empty());
        assert_eq!(
            app.state
                .with_project_scenes(|scenes| scenes.track_pools[0].patterns.len()),
            pool_len,
            "pending chunks never touched the pool"
        );
    }

    #[test]
    fn failed_commit_keeps_the_recorded_take() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 12.1), 60.0, 2.0));
        // A dropped capture notice fails the commit (spec 10.3): the take may
        // be incomplete, so the arrangement stays as it was.
        for _ in 0..300 {
            app.state
                .song_playback()
                .push_notice(crate::sequencer::SongPlaybackNotice::Ended {
                    end_beat: 16.0,
                    end_sample: 0,
                });
        }
        let song_before = app.state.committed_song();
        app.song_transport_mode = SongTransportMode::Stopped;
        let error = app
            .finish_song_capture_take(40.0)
            .expect_err("overflow fails the commit");
        assert!(error.contains("overflow"), "{error}");
        assert_eq!(app.state.committed_song(), song_before);
        // The recorded performance is NOT destroyed along with the failed
        // commit: it stays pending until the performer discards it (Cancel)
        // or a new capture replaces it.
        let session = app.take_recording.as_ref().expect("take preserved");
        assert!(session.has_pending_content());
        assert!(
            app.state.track_takes(0).is_empty(),
            "a failed commit leaves nothing in the take pool"
        );
    }

    #[test]
    fn stop_without_notes_or_launches_commits_nothing() {
        let (mut app, anchor) = capture_app();
        let song_before = app.state.committed_song();
        let depth = app.history.undo_len();
        app.song_transport_mode = SongTransportMode::Stopped;
        let status = app.finish_song_capture_take(40.0).expect("no-op stop");
        assert!(status.contains("unchanged"), "{status}");
        assert_eq!(app.state.committed_song(), song_before);
        assert_eq!(app.history.undo_len(), depth);
    }
}

impl App {
    /// Whether take recording is currently retargeting armed-track notes.
    pub fn take_recording_active(&self) -> bool {
        self.take_recording.is_some()
            && self.song_transport_mode == SongTransportMode::ArrangementCapture
    }

    /// Record one performed note into `track`'s pending take (spec 8.3/8.4).
    /// Returns `true` when the note was consumed (the caller must NOT write
    /// it into the live pattern), `false` when take recording does not apply
    /// (not capturing, no record-clock anchor) — the caller falls back to
    /// the existing live-pattern path.
    ///
    /// `press_time` is the note-on instant; positions are stamped on the
    /// latency-compensated record clock (`record_beats_at_instant`), the
    /// same clock immediate launches capture against.
    pub fn take_record_note(
        &mut self,
        track: usize,
        press_time: std::time::Instant,
        transpose: f32,
        duration_steps: f32,
    ) -> bool {
        if !self.take_recording_active() {
            return false;
        }
        let Some(raw_beats) = self.state.record_beats_at_instant(press_time) else {
            return false;
        };
        let quantize = RecordQuantize::from_atomic(
            self.state
                .transport
                .record_quantize
                .load(std::sync::atomic::Ordering::Relaxed) as u8,
        );
        // Template for a lazily minted lane: the track's BOUND source
        // (takes spec 16.2 — punch-in performs whatever the panel shows and
        // the monitor sounds), else a default lane for bare tracks. Only the
        // sequence half of the template survives registration; the sound is
        // SHARED via the bound refs (§17.3), not cloned.
        let bound = self.bound_read_pattern(track);
        let bound_sound = self.bound_sound_refs(track);
        let template = || {
            let mut data = self
                .state
                .with_project_scenes(|scenes| {
                    let pool = scenes.track_pools.get(track)?;
                    bound
                        .and_then(|id| pool.get(id))
                        .or_else(|| scenes.effective_track_pattern(track))
                })
                .or_else(|| {
                    PatternSnapshot::new_default(1, &[]).track_pattern_data(0)
                })?;
            data.track_params.num_steps = MAX_STEPS;
            data.clear_step_content();
            Some(data)
        };
        let Some(session) = self.take_recording.as_mut() else {
            return false;
        };
        let song_beat = (session.timeline_start_beat + raw_beats).max(0.0);
        let Some(slot) = session.lanes.get_mut(track) else {
            return false;
        };
        if slot.is_none() {
            let Some(template) = template() else {
                return false;
            };
            let step_beats = template.track_params.timebase.step_beats(MAX_STEPS);
            if !(step_beats > 0.0) {
                return false;
            }
            // Punch-in (spec 8.3): grid quantize puts P on the note's
            // quantized boundary — the SAME grid the note positions below use,
            // so the clip start never lands off the track's step grid; Off
            // floors the exact beat to the step grid (the sub-step remainder
            // becomes the note's step-0 delay).
            let punch_in_beat = match take_grid_steps(quantize, step_beats) {
                Some(grid_steps) => {
                    let grid = grid_steps * step_beats;
                    (song_beat / grid).round() * grid
                }
                None => (song_beat / step_beats).floor() * step_beats,
            }
            .max(0.0);
            *slot = Some(PendingTakeLane {
                punch_in_beat,
                step_beats,
                chunks: vec![template.clone()],
                template,
                sound: bound_sound,
                max_end_steps: 0.0,
            });
        }
        let lane = slot.as_mut().expect("lane minted above");

        // Clip-relative position in take steps (spec 8.4).
        let pos_steps = (song_beat - lane.punch_in_beat) / lane.step_beats;
        let (step, delay) = match take_grid_steps(quantize, lane.step_beats) {
            Some(grid_steps) => (
                ((pos_steps / grid_steps).round() * grid_steps)
                    .round()
                    .max(0.0) as usize,
                0.0,
            ),
            None => {
                let step = pos_steps.floor().max(0.0);
                (step as usize, (pos_steps - step).clamp(0.0, 1.0) as f32)
            }
        };
        // Chunk rollover (spec 8.4): extend with fresh template chunks.
        while step >= lane.chunks.len() * MAX_STEPS {
            lane.chunks.push(lane.template.clone());
        }
        let chunk = &mut lane.chunks[step / MAX_STEPS];
        let local = step % MAX_STEPS;
        chunk.track_bits[local / 64] |= 1 << (local % 64);
        chunk.chord_snapshot.steps[local].push(transpose);
        chunk.chord_snapshot.durations[local].push(duration_steps);
        chunk.chord_snapshot.delays[local].push(delay);
        let first_note = chunk.chord_snapshot.steps[local][0];
        chunk.step_data[local][StepParam::Transpose.index()] = first_note;
        chunk.step_data[local][StepParam::Velocity.index()] = 1.0;
        chunk.step_data[local][StepParam::Duration.index()] = duration_steps;
        lane.max_end_steps = lane
            .max_end_steps
            .max(step as f64 + 1.0)
            .max(step as f64 + f64::from(delay) + f64::from(duration_steps));
        // One of the two writers of provisional content (spec 3.3): the
        // `SEQ.song-pending` dots rebuild only when this moves.
        self.pending_revision = self.pending_revision.wrapping_add(1);
        true
    }

    /// Register every pending lane as a take (chunks enter the pattern pool
    /// here for the first time) and return the per-lane splice coordinates.
    /// Called by the capture stop-commit inside its atomic commit path.
    pub(crate) fn register_pending_takes(
        &mut self,
        pending: Vec<(usize, PendingTakeLane)>,
    ) -> Result<Vec<CommittedTakeLane>, String> {
        let mut committed = Vec::with_capacity(pending.len());
        for (track, lane) in pending {
            // Punch-out (spec 8.5): the step after the last note-on rounded
            // up to the grid, extended by the final release tail.
            let total_len_steps = lane.max_end_steps.ceil().max(1.0) as u32;
            let needed_chunks = (total_len_steps as usize).div_ceil(MAX_STEPS);
            let mut chunks = lane.chunks;
            chunks.truncate(needed_chunks.max(1));
            let take_id = self
                .state
                .register_track_take(track, None, chunks, total_len_steps, lane.sound)?;
            committed.push(CommittedTakeLane {
                track,
                take_id,
                punch_in_beat: lane.punch_in_beat,
                punch_out_beat: lane.punch_in_beat
                    + total_len_steps as f64 * lane.step_beats,
                step_beats: lane.step_beats,
            });
        }
        Ok(committed)
    }
}
