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
//! pending lanes register as takes and the song rows over `[P, Q)` are
//! repointed at them, in the same single undo entry as the launch splice
//! (spec 8.5).

use crate::record_quantize::RecordQuantize;
use crate::sequencer::{PatternSnapshot, StepParam, TakeId, TrackPatternData, MAX_STEPS};

use super::song_transport::SongTransportMode;
use super::App;

/// One armed track's in-flight take (spec 8.3/8.4).
pub(crate) struct PendingTakeLane {
    /// Punch-in beat `P` in the capture's song-beat domain (relative to the
    /// capture origin), aligned per the quantize policy at the first note.
    pub(crate) punch_in_beat: f64,
    /// Beats per chunk-domain step (the track's base timebase at the
    /// punch-in moment; chunks are `MAX_STEPS`-long patterns).
    pub(crate) step_beats: f64,
    /// Chunk content under construction. Detached from the pattern pool
    /// until commit.
    pub(crate) chunks: Vec<TrackPatternData>,
    /// Cleared template used to mint rollover chunks.
    template: TrackPatternData,
    /// Furthest written step end (note-on + duration), in take steps:
    /// finalizes `total_len_steps` (spec 8.5, release tail included).
    pub(crate) max_end_steps: f64,
}

/// Per-capture take recording session: one optional pending lane per track.
pub struct TakeRecordingSession {
    /// The capture's beat-zero origin on the scheduler rendered-beat clock —
    /// identical to the launch capture's origin, so take positions and
    /// spliced rows share one beat domain.
    origin_beats: f64,
    lanes: Vec<Option<PendingTakeLane>>,
}

impl TakeRecordingSession {
    pub(crate) fn new(origin_beats: f64, track_count: usize) -> Self {
        Self {
            origin_beats,
            lanes: (0..track_count).map(|_| None).collect(),
        }
    }

    pub(crate) fn has_pending_content(&self) -> bool {
        self.lanes
            .iter()
            .any(|lane| lane.as_ref().is_some_and(|lane| lane.max_end_steps > 0.0))
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

/// A registered-and-finalized pending lane, ready for row surgery.
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
        app
    }

    /// App with a two-row committed song (0.0 scene 0, 8.0 scene 1, end 16)
    /// in ArrangementCapture, plus the record-clock anchor instant for beat
    /// 0.0. Presses address beats as FUTURE instants relative to the anchor
    /// (the monotonic test-clock origin cannot represent the past).
    fn capture_app() -> (App, Instant) {
        let mut app = test_app();
        app.song_replace(
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
        .expect("song_replace succeeds");
        app.begin_song_capture_take();
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
        app.song_transport_locks_edits = false;
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
        assert!(song.rows.iter().all(|row| row.overrides.is_empty()));
    }

    #[test]
    fn recording_past_the_song_end_extends_it() {
        let (mut app, anchor) = capture_app();
        assert!(app.take_record_note(0, press_at_beats(anchor, 15.5), 60.0, 8.0));
        app.song_transport_mode = SongTransportMode::Stopped;
        app.song_transport_locks_edits = false;
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
    fn stop_without_notes_or_launches_commits_nothing() {
        let (mut app, anchor) = capture_app();
        let song_before = app.state.committed_song();
        let depth = app.history.undo_len();
        app.song_transport_mode = SongTransportMode::Stopped;
        app.song_transport_locks_edits = false;
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
        // Template for a lazily minted lane: the track's effective pattern
        // (device state rides along), else a default lane for bare tracks.
        let template = || {
            let mut data = self
                .state
                .with_project_scenes(|scenes| scenes.effective_track_pattern(track).cloned())
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
        let song_beat = (raw_beats - session.origin_beats).max(0.0);
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
            // quantized boundary; Off floors the exact beat to the step grid
            // (the sub-step remainder becomes the note's step-0 delay).
            let punch_in_beat = match quantize.grid_beats() {
                Some(grid) => (song_beat / grid).round() * grid,
                None => match quantize {
                    RecordQuantize::Off => (song_beat / step_beats).floor() * step_beats,
                    // Sixteenth: nearest step boundary.
                    _ => (song_beat / step_beats).round() * step_beats,
                },
            }
            .max(0.0);
            *slot = Some(PendingTakeLane {
                punch_in_beat,
                step_beats,
                chunks: vec![template.clone()],
                template,
                max_end_steps: 0.0,
            });
        }
        let lane = slot.as_mut().expect("lane minted above");

        // Clip-relative position in take steps (spec 8.4).
        let pos_steps = (song_beat - lane.punch_in_beat) / lane.step_beats;
        let (step, delay) = match quantize {
            RecordQuantize::Off => {
                let step = pos_steps.floor().max(0.0);
                (step as usize, (pos_steps - step).clamp(0.0, 1.0) as f32)
            }
            RecordQuantize::Sixteenth => (pos_steps.round().max(0.0) as usize, 0.0),
            _ => {
                let grid_steps = (quantize
                    .grid_beats()
                    .expect("non-off record quantization must define a grid")
                    / lane.step_beats)
                    .max(1.0e-9);
                (
                    ((pos_steps / grid_steps).round() * grid_steps)
                        .round()
                        .max(0.0) as usize,
                    0.0,
                )
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
                .register_track_take(track, None, chunks, total_len_steps)?;
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
