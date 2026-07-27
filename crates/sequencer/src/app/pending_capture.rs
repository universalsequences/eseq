//! Provisional (uncommitted) arrangement-capture content, read-only
//! (docs/realtime-arrangement-feedback-spec.md 3).
//!
//! While `ArrangementCapture` runs, the pending takes
//! (`take_recording::PendingTakeLane`) and the captured launches
//! (`song_capture::CaptureLaunchEvent`) live on the control thread and are
//! invisible to every reactive surface — the arrangement lanes stay empty
//! until Stop. This module is the read side that lets the UI draw them as
//! they grow: a borrowing view, so a per-frame read never clones a chunk.
//!
//! Everything here is INERT (spec 7): the content has no `ClipId` yet, so it
//! is drawn and never addressed by a gesture. The view carries no ids for
//! exactly that reason.

use crate::sequencer::{PatternId, TrackPatternData};

use super::song_capture::CaptureLaunchKind;
use super::App;

/// One in-flight take, borrowed from its `PendingTakeLane`.
pub struct PendingCaptureLane<'a> {
    pub track: usize,
    /// Punch-in beat `P` on the arrangement timeline.
    pub punch_in_beat: f64,
    /// Beats per chunk-domain step; chunks are `MAX_STEPS`-long patterns.
    pub step_beats: f64,
    /// Furthest written step end in take steps — the same quantity the
    /// stop-commit rounds up into `total_len_steps`, so the drawn span and
    /// the committed clip's span agree (spec 6 item 1, round trip).
    pub max_end_steps: f64,
    /// The detached chunks under construction, in take order.
    pub chunks: &'a [TrackPatternData],
}

/// The provisional state of the running capture (spec 3.2), borrowed.
pub struct PendingCapture<'a> {
    /// Arrangement beat corresponding to scheduler/record-clock beat zero.
    pub origin_beat: f64,
    pub lanes: Vec<PendingCaptureLane<'a>>,
    /// Captured SCENE launches as `(start-beat, scene)`, beat-ordered.
    pub scene_events: Vec<(f64, usize)>,
    /// What each captured launch put on each TRACK lane, as
    /// `(start-beat, track, pattern)`, beat-ordered. A track launch
    /// contributes its own overrides; a scene launch expands to the scene's
    /// cell pattern on every lane it claims — the clips underneath a
    /// captured scene change are what the splice will actually write, so
    /// they are what the feedback has to show.
    pub track_events: Vec<(f64, usize, PatternId)>,
}

impl App {
    /// Whether a capture take is staging provisional content. The one
    /// boolean an idle frame pays (spec 3.3).
    pub fn pending_capture_active(&self) -> bool {
        self.song_capture_take.is_some()
    }

    /// The record head in the capture's beat domain: the latency-compensated
    /// record clock (the same clock take notes and immediate launches are
    /// stamped on), clamped to the published song position so a provisional
    /// clip never draws ahead of the playhead the lanes render (spec 3.2).
    /// `None` when the record clock has no anchor yet.
    pub fn pending_capture_head_beat(&self) -> Option<f64> {
        let timeline_start = self.song_capture_take.as_ref()?.timeline_start_beat();
        let head = self
            .state
            .record_beats_at_instant(std::time::Instant::now())
            .map(|raw| (timeline_start + raw).max(0.0))
            .or_else(|| self.state.song_position_beats())?;
        Some(match self.state.song_position_beats() {
            Some(position) => head.min(position),
            None => head,
        })
    }

    /// Run `f` against the provisional capture state without cloning it.
    /// `None` — and `f` never runs — unless a capture take is active, so the
    /// common path pays one boolean (spec 3.3) and every capture exit path
    /// (stop, cancel, failure) clears the surface by construction: all three
    /// drop `song_capture_take`.
    pub fn with_pending_capture<R>(&self, f: impl FnOnce(PendingCapture<'_>) -> R) -> Option<R> {
        let take = self.song_capture_take.as_ref()?;
        let mut ordered: Vec<&super::song_capture::CaptureLaunchEvent> =
            take.events().iter().collect();
        // Stable, so launches sharing a boundary keep their application
        // order — the same tie-break the stop-commit's `consolidate` uses.
        ordered.sort_by(|a, b| {
            a.beat
                .partial_cmp(&b.beat)
                .expect("capture beats are finite")
        });
        let mut scene_events = Vec::new();
        let mut track_events = Vec::new();
        // The state capture STARTED in. It only becomes arrangement content
        // when there is no committed song — the stop-commit then splices
        // from beat zero rather than from the first launch — so drawing it
        // otherwise would paint over the pre-existing arrangement with
        // something the commit will never write.
        if take.whole_song() {
            let initial = take.initial();
            scene_events.push((0.0, initial.scene));
            for track in 0..self.tracks.len() {
                let pattern = initial
                    .overrides
                    .iter()
                    .find(|(over, _)| *over == track)
                    .map(|(_, pattern)| *pattern)
                    .or_else(|| self.state.scene_track_pattern_id(initial.scene, track));
                if let Some(pattern) = pattern {
                    track_events.push((0.0, track, pattern));
                }
            }
        }
        for event in ordered {
            match &event.kind {
                CaptureLaunchKind::Scene { scene, take_lanes } => {
                    scene_events.push((event.beat, *scene));
                    for track in 0..self.tracks.len() {
                        // A scene launch does NOT claim a lane playing a
                        // take (song_capture.rs `consolidate`), so that lane
                        // keeps showing what it is really playing.
                        if track < 64 && take_lanes >> track & 1 == 1 {
                            continue;
                        }
                        if let Some(pattern) = self.state.scene_track_pattern_id(*scene, track) {
                            track_events.push((event.beat, track, pattern));
                        }
                    }
                }
                CaptureLaunchKind::Tracks { overrides } => {
                    for (track, pattern) in overrides {
                        track_events.push((event.beat, *track, *pattern));
                    }
                }
            }
        }
        let lanes = self
            .take_recording
            .as_ref()
            .map(|session| session.pending_lane_views())
            .unwrap_or_default();
        Some(f(PendingCapture {
            origin_beat: take.timeline_start_beat(),
            lanes,
            scene_events,
            track_events,
        }))
    }
}
