//! Arrangement-capture staging take (docs/song-mode-spec.md 7.4, 8, 10.3,
//! 10.4).
//!
//! While `ArrangementCapture` is active the control thread accumulates one
//! lightweight `CaptureLaunchEvent` per audible launch, observed at the
//! central `App::apply_pattern_launch` seam so UI, Lisp, MIDI, and keyboard
//! launches capture identically (spec 8.1). Every event carries the
//! authoritative audible beat: the scheduler-stamped grid deadline for
//! quantized launches (spec 8.3) or the scheduler's rendered-beat clock read
//! at application time for immediate ones (spec 8.2, never snapped) — both in
//! the same `rendered_beats` clock domain `quantized_launch::launch_deadline`
//! uses. Beats are stored relative to the capture's beat origin (the rendered
//! beat at capture start; the transport starts at song beat zero, spec
//! 7.4.2).
//!
//! Stop consolidates the take per spec 10.4 (sort by audible beat, group per
//! boundary with scene-clears-overrides, drop adjacent identical states) and
//! commits it onto the stored **arrangement**
//! (docs/arrangement-lane-model-spec.md 9): scene launches become scene
//! events, per-track launches become clips, and the punch region is spliced
//! by ordinary clip trimming (`occlude_span`). One project mutation, one undo
//! entry. Overflow or any validation/compile failure leaves the previous
//! committed arrangement intact and surfaces an actionable error.

use std::collections::BTreeMap;

use crate::quantized_launch::PatternLaunchTarget;
use crate::sequencer::{
    insert_clip_sorted, occlude_span, stamped_clip_override, ArrClip, ClipId, PatternId,
    ProjectArrangement, ProjectScenes, ProjectSongTrackOverride, SceneEvent, SongProjectContext,
};

use super::edit::finish_active_gesture;
use super::App;

/// One complete consolidated session state at a capture boundary
/// (spec 10.3). Overrides are kept sorted by track (BTreeMap iteration
/// order at construction), so duplicates are structurally impossible.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedSongState {
    pub start_beat: f64,
    pub scene: usize,
    pub overrides: Vec<(usize, PatternId)>,
    /// Tracks the performer has launch authority over at this boundary
    /// (takes spec 9.4/10): a scene launch touches every track, a track
    /// launch adds its tracks. Lanes NOT here inherit the pre-existing
    /// arrangement's resolution at commit (capture runs on top of song
    /// playback, spec 9.3). All tracks when capturing from an empty song
    /// (the performer is the sole authority there).
    pub touched: std::collections::BTreeSet<usize>,
}

/// The resolved launch identity of one captured event.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CaptureLaunchKind {
    /// A scene launch: sets the base scene and clears every override.
    /// `take_lanes` are the lanes that were playing takes at the launch
    /// moment — a scene launch does NOT claim those (takes survive the
    /// capture unless the performer intentionally clip-launches the lane).
    Scene { scene: usize, take_lanes: u64 },
    /// A masked track-pattern launch: installs these per-track overrides.
    Tracks { overrides: Vec<(usize, PatternId)> },
}

/// One audible launch observed at the central seam, with its authoritative
/// audible beat relative to the capture origin.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaptureLaunchEvent {
    pub(crate) beat: f64,
    pub(crate) kind: CaptureLaunchKind,
}

/// The control-side staging take (spec 10.3). The committed song is never
/// touched while this exists; Stop consolidates and commits it, Cancel
/// discards it.
pub struct SongCaptureTake {
    /// The scheduler rendered-beat clock value at capture start: the take's
    /// beat-zero origin. Every recorded beat is `raw - origin` (clamped to
    /// zero) so rows are relative to song start.
    origin_beats: f64,
    /// The resolved session state at capture start: the beat-zero row
    /// (spec 7.4.3).
    initial: CapturedSongState,
    /// True when capture began with NO committed song. The stop-commit then
    /// takes the `(None, _)` arm and splices from beat ZERO, so the initial
    /// state really does become arrangement content from bar 1 — which is
    /// what the provisional surface has to show
    /// (docs/realtime-arrangement-feedback-spec.md 3.2). With a committed
    /// song the splice starts at the first captured launch and everything
    /// before it is the pre-existing arrangement, already drawn as committed
    /// clips.
    whole_song: bool,
    events: Vec<CaptureLaunchEvent>,
}

impl SongCaptureTake {
    pub(crate) fn event_count(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn origin_beats(&self) -> f64 {
        self.origin_beats
    }

    pub(crate) fn initial(&self) -> &CapturedSongState {
        &self.initial
    }

    pub(crate) fn whole_song(&self) -> bool {
        self.whole_song
    }

    /// The launches captured so far, for the provisional read surface
    /// (docs/realtime-arrangement-feedback-spec.md 3.2).
    pub(crate) fn events(&self) -> &[CaptureLaunchEvent] {
        &self.events
    }
}

/// Consolidate a take into the final row states (spec 10.4): stable-sort by
/// audible beat, group events sharing one boundary (scene launch clears
/// overrides before that boundary's track launches consolidate, regardless
/// of input order), then drop adjacent identical states keeping the earlier
/// row.
fn consolidate(initial: &CapturedSongState, events: &[CaptureLaunchEvent]) -> Vec<CapturedSongState> {
    let mut sorted: Vec<&CaptureLaunchEvent> = events.iter().collect();
    // Stable: events at the same boundary keep their application order.
    sorted.sort_by(|a, b| a.beat.partial_cmp(&b.beat).expect("capture beats are finite"));

    let mut rows: Vec<CapturedSongState> = vec![initial.clone()];
    let mut idx = 0;
    while idx < sorted.len() {
        let boundary = sorted[idx].beat;
        let mut end = idx;
        while end < sorted.len() && sorted[end].beat == boundary {
            end += 1;
        }
        let group = &sorted[idx..end];
        idx = end;

        let previous = rows.last().expect("rows always holds the initial state");
        // Spec 10.4: an audible scene launch clears all overrides before the
        // boundary's track launches consolidate — the LAST scene launch in
        // the group provides the base, and every track launch in the group
        // applies on top, regardless of input-event ordering.
        let last_scene = group
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                CaptureLaunchKind::Scene { scene, take_lanes } => {
                    Some((*scene, *take_lanes))
                }
                CaptureLaunchKind::Tracks { .. } => None,
            });
        let mut touched = previous.touched.clone();
        let (scene, mut overrides) = match last_scene {
            Some((scene, take_lanes)) => {
                // A scene launch latches every lane EXCEPT the ones playing
                // takes at that moment (takes spec 10, refined): scene
                // changes are pattern-lane gestures — a take lane is only
                // claimed by an intentional clip launch on that track. The
                // excluded lanes stay untouched, so the splice materializes
                // their inherited (take) resolution.
                touched.extend((0..crate::sequencer::MAX_TRACKS).filter(|track| {
                    *track >= 64 || take_lanes >> *track & 1 == 0
                }));
                (scene, BTreeMap::new())
            }
            None => (
                previous.scene,
                previous.overrides.iter().copied().collect::<BTreeMap<_, _>>(),
            ),
        };
        for event in group {
            if let CaptureLaunchKind::Tracks { overrides: pairs } = &event.kind {
                for (track, pattern) in pairs {
                    overrides.insert(*track, *pattern);
                    touched.insert(*track);
                }
            }
        }
        let state = CapturedSongState {
            start_beat: boundary,
            scene,
            overrides: overrides.into_iter().collect(),
            touched,
        };
        if boundary <= 0.0 {
            // A launch audible exactly at the capture start replaces the
            // beat-zero row's state.
            *rows.last_mut().expect("rows is non-empty") = CapturedSongState {
                start_beat: 0.0,
                ..state
            };
        } else {
            rows.push(state);
        }
    }

    // Spec 7.4.6/10.4.4: repeated identical states produce no row. The
    // earlier row survives, mirroring `ProjectSong::normalize`. Authority
    // (`touched`) participates in identity: relaunching the current scene
    // during capture-on-playback audibly takes the lanes over (takes spec
    // 9.3/10) even though scene+overrides look unchanged.
    rows.dedup_by(|later, earlier| {
        earlier.scene == later.scene
            && earlier.overrides == later.overrides
            && earlier.touched == later.touched
    });
    rows
}

impl App {
    /// Begin the staging take at capture start (spec 7.4.1-7.4.3): clear any
    /// previous failure state, establish the beat origin from the scheduler's
    /// rendered-beat clock, and record the current RESOLVED session state
    /// (current scene plus current track overrides) as the beat-zero row.
    /// The committed song is untouched and not played.
    pub(crate) fn begin_song_capture_take(&mut self) {
        self.song_capture_failed = false;
        self.song_capture_error = None;
        // A stale overflow left over from earlier song playback must not
        // fail this capture: the flag is sticky, so drain it now.
        let _ = self.state.song_playback().take_notice_overflow();
        let scenes = self.state.capture_project_scenes();
        let overrides = scenes
            .track_overrides
            .iter()
            .enumerate()
            .filter_map(|(track, over)| over.map(|id| (track, id)))
            .collect();
        // Capture always begins with the transport stopped and starting from
        // song beat zero (spec 7.4.2/9.3), so the origin IS beat zero. The
        // scheduler's rendered-beat clock is not sampled here: pre-start it
        // is an asynchronously published leftover of the previous playback,
        // and any nonzero reading would shift every captured launch and take
        // note late by that amount.
        let origin_beats = 0.0;
        // With a committed song, capture runs ON TOP of song playback
        // (takes spec 9.3): the song keeps launch authority until the
        // performer touches a lane, so the initial state starts untouched.
        // Recording from an empty song keeps the performer as sole
        // authority (every lane touched — the pre-spec whole-song capture).
        let whole_song = self.state.committed_song().is_none();
        let touched: std::collections::BTreeSet<usize> = if whole_song {
            (0..self.tracks.len()).collect()
        } else {
            std::collections::BTreeSet::new()
        };
        self.song_capture_take = Some(SongCaptureTake {
            origin_beats,
            whole_song,
            initial: CapturedSongState {
                start_beat: 0.0,
                scene: scenes.current_scene,
                overrides,
                touched,
            },
            events: Vec::new(),
        });
        // Take recording rides the same capture pass (takes spec 8.2): one
        // transport gesture, two streams, one commit. Same beat origin.
        self.take_recording = Some(super::take_recording::TakeRecordingSession::new(
            origin_beats,
            self.tracks.len(),
        ));
    }

    /// Discard the staging take (Cancel, spec 7.4.8). The committed song is
    /// preserved by construction: the take never touched it.
    pub(crate) fn discard_song_capture_take(&mut self) {
        self.song_capture_take = None;
        // Pending take content lives in detached buffers (takes spec 8.5
        // Cancel): dropping it touches neither the pattern pool nor the song.
        self.take_recording = None;
    }

    /// Record one successful audible launch. Called from
    /// `App::apply_pattern_launch` only (the central seam, spec 8.1/14.4);
    /// no-op unless a take is active. `audible_beats` is in the scheduler's
    /// rendered-beat clock domain; it is stored relative to the take origin.
    pub(crate) fn record_song_capture_launch(
        &mut self,
        target: &PatternLaunchTarget,
        audible_beats: f64,
    ) {
        // Resolve the launch identity before mutably borrowing the take:
        // a SceneTracks launch installs the target scene's cell patterns as
        // overrides (see `ProjectScenes::launch_scene_tracks`).
        let kind = match target {
            PatternLaunchTarget::Scene { scene } => CaptureLaunchKind::Scene {
                scene: *scene,
                // The lanes playing takes when the scene was launched keep
                // them (the audible latch skipped these lanes too, so the
                // song is still playing their takes underneath).
                take_lanes: self.state.song_take_lane_mask(),
            },
            PatternLaunchTarget::SceneTracks { scene, tracks } => CaptureLaunchKind::Tracks {
                overrides: tracks
                    .iter()
                    .filter_map(|track| {
                        self.state
                            .scene_track_pattern_id(*scene, *track)
                            .map(|id| (*track, id))
                    })
                    .collect(),
            },
        };
        let Some(take) = self.song_capture_take.as_mut() else {
            return;
        };
        take.events.push(CaptureLaunchEvent {
            beat: (audible_beats - take.origin_beats).max(0.0),
            kind,
        });
        // The second writer of provisional content (spec 3.3).
        self.pending_revision = self.pending_revision.wrapping_add(1);
    }

    /// Observe a manual CLIP launch (the mixer clip grid's per-track
    /// pattern launch, which does not route through `apply_pattern_launch`):
    /// latch the lane (takes spec 10) and record the capture event. A clip
    /// launch is the intentional way to take a lane over — it claims the
    /// lane even when it is playing a take.
    pub fn observe_manual_clip_launch(&mut self, track: usize, pattern_id: PatternId) {
        if self.song_playback_authority_active() {
            self.state.latch_song_manual_override([track]);
        }
        let Some(take) = self.song_capture_take.as_mut() else {
            return;
        };
        let beat = self
            .state
            .record_beats_at_instant(std::time::Instant::now())
            .unwrap_or_else(|| self.state.scheduler_rendered_beats())
            .max(0.0);
        take.events.push(CaptureLaunchEvent {
            beat: (beat - take.origin_beats).max(0.0),
            kind: CaptureLaunchKind::Tracks {
                overrides: vec![(track, pattern_id)],
            },
        });
        // Same event list as `record_song_capture_launch`, so the same
        // counter: leaving it out would freeze the provisional surface after
        // a manual clip launch.
        self.pending_revision = self.pending_revision.wrapping_add(1);
    }

    /// Stop-commit (spec 7.4.7/10.4, lane spec 9): decompose the consolidated
    /// take into scene events and clips and splice them into the committed
    /// **arrangement** (one project mutation, one undo entry).
    /// `end_raw_beats` is the scheduler rendered-beat clock at Stop — the same
    /// clock the events were recorded against. On any failure the previous
    /// committed arrangement is intact, the failure state is latched for the
    /// `song-capture-failed` / `song-capture-error` bindings, and the take is
    /// discarded.
    pub(crate) fn finish_song_capture_take(
        &mut self,
        end_raw_beats: f64,
    ) -> Result<String, String> {
        let result = self.try_finish_song_capture_take(end_raw_beats);
        self.song_capture_take = None;
        self.take_recording = None;
        if let Err(error) = &result {
            self.song_capture_failed = true;
            self.song_capture_error = Some(error.clone());
        }
        result
    }

    fn try_finish_song_capture_take(&mut self, end_raw_beats: f64) -> Result<String, String> {
        let Some(take) = self.song_capture_take.take() else {
            return Err("no arrangement-capture take is active".to_string());
        };
        // Pending take-recording lanes commit together with the launch
        // splice as one undo entry (takes spec 8.2/8.5).
        let pending = self
            .take_recording
            .take()
            .map(|session| session.into_pending())
            .unwrap_or_default();
        // Spec 10.3: a lost notice means the take may be incomplete; it must
        // never be committed.
        if self.state.song_playback().take_notice_overflow() {
            return Err(
                "capture events were lost (notice channel overflow); the take was not \
                 committed and the previous song is unchanged"
                    .to_string(),
            );
        }
        let end_beat = (end_raw_beats - take.origin_beats).max(0.0);
        let captured = consolidate(&take.initial, &take.events);
        let previous = self.state.committed_arrangement();

        // Punch region (takes spec 9.1): `P` is the first captured launch —
        // hitting record and listening before the first launch must not erase
        // the head of the song — and `Q` is the Stop beat. Recording over an
        // empty project keeps the whole-song commit (spec 9.3: "record from an
        // empty song"), which is the same splice over `[0, Q)` onto an empty
        // arrangement.
        let punch_in = take
            .events
            .iter()
            .map(|event| event.beat)
            .min_by(|a, b| a.partial_cmp(b).expect("capture beats are finite"));

        // The beats the performer changed SCENE at. Consolidated states carry
        // a scene at every boundary (a track launch inherits the previous
        // one), so only the launch stream can say where a scene *event*
        // belongs — writing one at a track launch would move the backdrop
        // under every lane the performer never touched.
        let scene_launch_beats: Vec<f64> = take
            .events
            .iter()
            .filter(|event| matches!(event.kind, CaptureLaunchKind::Scene { .. }))
            .map(|event| event.beat)
            .collect();

        let scenes = self.state.capture_project_scenes();
        let arrangement = match (&previous, punch_in) {
            (Some(previous), None) => {
                // No launches performed: nothing to splice; the committed
                // arrangement is untouched (spec 9.1) — unless takes were
                // recorded, in which case they paint onto it unchanged.
                if pending.is_empty() {
                    return Ok(
                        "Arrangement capture ended: no launches captured; the committed \
                         song is unchanged"
                            .to_string(),
                    );
                }
                previous.clone()
            }
            (Some(previous), Some(punch_in)) => self.spliced_arrangement(
                previous,
                &scenes,
                &captured,
                &scene_launch_beats,
                punch_in,
                end_beat,
            )?,
            (None, _) => {
                let base = ProjectArrangement {
                    scene_lane: vec![SceneEvent {
                        start_beat: 0.0,
                        scene: captured[0].scene,
                    }],
                    track_lanes: vec![Vec::new(); scenes.song_track_count()],
                    end_beat,
                    loop_enabled: false,
                    next_clip_id: 0,
                };
                self.spliced_arrangement(&base, &scenes, &captured, &scene_launch_beats, 0.0, end_beat)?
            }
        };

        // Guarded like every other authoring path; a no-op stop above returns
        // before reaching it, exactly as the row primitive's check did.
        self.require_song_edit_unlocked()?;
        if pending.is_empty() {
            let before = previous;
            self.commit_arrangement_edit("Capture arrangement", before, Some(arrangement))
                .map_err(|error| format!("the captured take could not be committed: {error}"))?;
            let row_count = self
                .state
                .committed_song()
                .map(|song| song.rows.len())
                .unwrap_or(0);
            let end = self
                .state
                .committed_arrangement()
                .map(|arrangement| arrangement.end_beat)
                .unwrap_or(0.0);
            return Ok(format!(
                "Arrangement capture committed: {row_count} row(s), end beat {end:.3}"
            ));
        }
        self.commit_capture_with_takes(arrangement, previous, pending)
    }

    /// Splice the consolidated capture into `previous` over `[P, Q)` (lane
    /// spec 9, takes spec 9.2).
    ///
    /// The region boundaries are the punch region: `P` is the first captured
    /// launch's audible beat, `Q` the Stop beat (never before `P`). Everything
    /// outside `[P, Q)` is left untouched — including clips the region only
    /// clips into, which `occlude_span` trims and re-stamps exactly as any
    /// other write op does, so the pre-existing arrangement resumes at `Q`
    /// with its phase intact. No restore rows are constructed.
    fn spliced_arrangement(
        &self,
        previous: &ProjectArrangement,
        scenes: &ProjectScenes,
        captured: &[CapturedSongState],
        scene_launch_beats: &[f64],
        punch_in: f64,
        stop_beat: f64,
    ) -> Result<ProjectArrangement, String> {
        let punch_out = stop_beat.max(punch_in);
        // The captured state governing `P`, re-based to start exactly there,
        // plus every later captured state inside the region. Row zero of
        // `captured` always exists, so a governing state is guaranteed.
        let governing = captured
            .iter()
            .rposition(|state| state.start_beat <= punch_in)
            .expect("captured states always include a beat-zero state");
        let mut states: Vec<CapturedSongState> = Vec::new();
        for (idx, state) in captured.iter().enumerate().skip(governing) {
            let start_beat = if idx == governing {
                punch_in
            } else {
                state.start_beat
            };
            if start_beat > 0.0 && start_beat >= punch_out {
                // A launch audible at or after the Stop boundary was never
                // part of the audible performance: drop it.
                continue;
            }
            states.push(CapturedSongState {
                start_beat,
                ..state.clone()
            });
        }
        // The scene the ARRANGEMENT is playing as the region opens — the
        // baseline the captured scene changes are measured against.
        let baseline_scene = previous
            .scene_at_beat(punch_in)
            .unwrap_or(captured[governing].scene);

        let mut arrangement = previous.clone();
        arrangement.end_beat = previous.end_beat.max(punch_out);
        Self::splice_scene_lane(
            &mut arrangement,
            previous,
            &states,
            scene_launch_beats,
            baseline_scene,
            punch_out,
        );
        for track in 0..arrangement.track_lanes.len() {
            self.splice_captured_lane(&mut arrangement, scenes, track, &states, punch_out)?;
        }
        Ok(arrangement)
    }

    /// Write the captured scene *changes* onto the scene lane.
    ///
    /// A scene event is emitted only where the performance actually changed
    /// scene, so a capture of nothing but track launches leaves the scene lane
    /// alone. Pre-existing scene events are removed only from the first
    /// captured change onward: a scene launch claims every lane (takes spec
    /// 10), so from there the performance owns the backdrop, while before it
    /// the lanes the performer never touched must keep the scene changes they
    /// were playing. At `Q` the pre-existing scene resumes.
    fn splice_scene_lane(
        arrangement: &mut ProjectArrangement,
        previous: &ProjectArrangement,
        states: &[CapturedSongState],
        scene_launch_beats: &[f64],
        baseline_scene: usize,
        punch_out: f64,
    ) {
        let mut captured_events: Vec<SceneEvent> = Vec::new();
        let mut effective = baseline_scene;
        for state in states {
            let launched = scene_launch_beats
                .iter()
                .any(|beat| *beat == state.start_beat);
            if launched && state.scene != effective {
                captured_events.push(SceneEvent {
                    start_beat: state.start_beat,
                    scene: state.scene,
                });
                effective = state.scene;
            }
        }
        let Some(splice_start) = captured_events.first().map(|event| event.start_beat) else {
            return;
        };
        // Resolved against the ORIGINAL lane, before anything is removed.
        let restore_scene = previous.scene_at_beat(punch_out);

        let mut lane: Vec<SceneEvent> = previous
            .scene_lane
            .iter()
            .copied()
            .filter(|event| event.start_beat < splice_start || event.start_beat >= punch_out)
            .collect();
        lane.extend(captured_events.iter().copied());
        // Restore event at `Q`: the pre-existing scene resumes. It is omitted
        // when the performance left that very scene in effect — the lane
        // holds changes only, and a redundant event would re-anchor every
        // backdrop lane's phase at `Q` (an event's beat IS its phase anchor,
        // spec 7). Where the pre-existing backdrop phase differs anyway,
        // `restore_lane_at_punch_out` carries it on a clip instead.
        if punch_out < arrangement.end_beat {
            if let Some(scene) = restore_scene {
                if scene != effective && !lane.iter().any(|event| event.start_beat == punch_out) {
                    lane.push(SceneEvent {
                        start_beat: punch_out,
                        scene,
                    });
                }
            }
        }
        lane.sort_by(|a, b| {
            a.start_beat
                .partial_cmp(&b.start_beat)
                .expect("scene event beats are finite")
        });
        arrangement.scene_lane = lane;
    }

    /// Decompose one lane's captured launches into clips over `[T, Q)`, where
    /// `T` is the beat the performer first took the lane over (takes spec
    /// 9.2's "touched").
    ///
    /// An **untouched** lane is not written at all — no clip, no trim — so its
    /// pre-existing clips keep playing straight through the region. That is
    /// the whole inheritance rule, and under the clip-only model it needs no
    /// machinery at all: the clips that covered the region still cover it.
    ///
    /// A touched lane's launches become clips: each state's resolved source
    /// (the explicit launch else the captured scene's cell — a scene launch
    /// stamps every lane it claims) with the free-run offset `steps(beat) mod
    /// L` stamped (takes spec 7.2), opened where it first differs and closed
    /// where the lane changes again or at `Q`. A state resolving to nothing
    /// (an empty scene cell) opens no clip: the lane is genuinely silent
    /// there. At `Q` the pre-existing clips resume — `occlude_span`
    /// left-trimmed and re-stamped whatever the region cut into, so nothing
    /// has to be materialized to restore them.
    fn splice_captured_lane(
        &self,
        arrangement: &mut ProjectArrangement,
        scenes: &ProjectScenes,
        track: usize,
        states: &[CapturedSongState],
        punch_out: f64,
    ) -> Result<bool, String> {
        let Some(first) = states
            .iter()
            .position(|state| state.touched.contains(&track))
        else {
            return Ok(false);
        };
        let touched_beat = states[first].start_beat;
        if touched_beat >= punch_out {
            return Ok(false);
        }
        occlude_span(arrangement, scenes, track, touched_beat, punch_out)?;

        let mut clips: Vec<ArrClip> = Vec::new();
        let mut open: Option<ArrClip> = None;
        for state in &states[first..] {
            let beat = state.start_beat;
            if beat >= punch_out {
                break;
            }
            let desired = self.captured_lane_resolution(scenes, track, state, beat);
            if let Some(clip) = open.as_ref() {
                if lane_resolution_matches(&stamped_clip_override(scenes, track, clip, beat), desired)
                {
                    // The open clip already plays exactly this here.
                    continue;
                }
            }
            if let Some(mut clip) = open.take() {
                clip.end_beat = beat;
                clips.push(clip);
            }
            let Some((pattern_id, offset_steps)) = desired else {
                continue;
            };
            open = Some(ArrClip {
                id: ClipId(0), // assigned once the lane's clips are inserted
                start_beat: beat,
                end_beat: punch_out,
                pattern_id: Some(pattern_id),
                take_id: None,
                offset_steps,
            });
        }
        if let Some(mut clip) = open.take() {
            clip.end_beat = punch_out;
            clips.push(clip);
        }
        for mut clip in clips {
            if clip.end_beat <= clip.start_beat {
                continue;
            }
            clip.id = arrangement.allocate_clip_id()?;
            insert_clip_sorted(arrangement, track, clip);
        }
        Ok(true)
    }

    /// What `track` played at `beat` during the performance: the explicit
    /// launch if the performer made one, else the captured scene's cell, with
    /// the free-run phase stamped (takes spec 7.2 — every audible pattern
    /// free-runs against the global clock, so its position is
    /// `steps(beat) mod L`). `None` when the lane resolved to nothing.
    fn captured_lane_resolution(
        &self,
        scenes: &ProjectScenes,
        track: usize,
        state: &CapturedSongState,
        beat: f64,
    ) -> Option<(u64, f64)> {
        let pattern_id = match state
            .overrides
            .iter()
            .find(|(over_track, _)| *over_track == track)
        {
            Some((_, pattern)) => pattern.0,
            None => crate::sequencer::SongCompileContext::song_scene_cell(
                scenes,
                state.scene,
                track,
            )?,
        };
        Some((pattern_id, self.advanced_offset(track, pattern_id, 0.0, beat)))
    }

    /// Paint one take clip per recorded lane over its punch region (takes spec
    /// 8.5, lane spec 9): `offset 0` by construction — recording writes
    /// clip-relative positions, so take step 0 IS the punch-in — truncating
    /// whatever it lands on like any other clip write.
    fn paint_take_clips(
        &self,
        arrangement: &mut ProjectArrangement,
        scenes: &ProjectScenes,
        lanes: &[super::take_recording::CommittedTakeLane],
    ) -> Result<(), String> {
        // A take running past the song end extends it (spec 8.5).
        for lane in lanes {
            arrangement.end_beat = arrangement.end_beat.max(lane.punch_out_beat);
        }
        for lane in lanes {
            if lane.track >= arrangement.track_lanes.len() {
                return Err(format!("Track {} has no arrangement lane", lane.track + 1));
            }
            let end_beat = lane.punch_out_beat.min(arrangement.end_beat);
            if end_beat <= lane.punch_in_beat {
                continue;
            }
            self.paint_take_clip(
                arrangement,
                scenes,
                lane.track,
                lane.punch_in_beat,
                end_beat,
                lane.take_id,
            )?;
        }
        Ok(())
    }

    /// Atomic commit of the launch splice PLUS the recorded takes (takes
    /// spec 8.5): register every pending take, paint one take clip per
    /// recorded lane onto the spliced arrangement, and commit ONE composite
    /// undo entry (scenes + arrangement). Any failure rolls both back.
    fn commit_capture_with_takes(
        &mut self,
        arrangement: ProjectArrangement,
        arrangement_before: Option<ProjectArrangement>,
        pending: Vec<(usize, super::take_recording::PendingTakeLane)>,
    ) -> Result<String, String> {
        use super::history::{ArrangementStructurePatch, EditPatch, SceneStructurePatch};

        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        let lanes = match self.register_pending_takes(pending) {
            Ok(lanes) => lanes,
            Err(error) => {
                self.restore_scene_structure_state(&scenes_before)?;
                return Err(format!("recorded takes could not be registered: {error}"));
            }
        };
        let rollback = |app: &mut App,
                        lanes: &[super::take_recording::CommittedTakeLane]|
         -> Result<(), String> {
            for lane in lanes {
                app.state.remove_track_take(lane.track, lane.take_id)?;
            }
            app.restore_scene_structure_state(&scenes_before)
        };

        // The take pool changed, so the paint runs against the POST-register
        // scenes: the clips reference takes that only exist now.
        let scenes = self.state.capture_project_scenes();
        let mut arrangement = arrangement;
        if let Err(error) = self.paint_take_clips(&mut arrangement, &scenes, &lanes) {
            rollback(self, &lanes)?;
            return Err(format!("recorded takes could not be committed: {error}"));
        }
        if let Err(error) = self
            .state
            .set_committed_arrangement(Some(arrangement.clone()))
        {
            rollback(self, &lanes)?;
            return Err(format!("the captured take could not be committed: {error}"));
        }
        let scenes_after = self.state.capture_project_scenes();
        finish_active_gesture(self);
        let scene_patch = SceneStructurePatch {
            before: scenes_before,
            after: scenes_after,
        };
        let arrangement_patch = ArrangementStructurePatch {
            before: arrangement_before,
            after: Some(arrangement.clone()),
        };
        let retained_bytes = scene_patch.retained_bytes() + arrangement_patch.retained_bytes();
        // Scenes first: redo restores the takes before the arrangement
        // referencing them; undo removes the references before the takes (see
        // take_edit.rs for the same ordering rationale).
        self.history.commit(
            "Record arrangement takes",
            None,
            EditPatch::Composite(vec![
                EditPatch::SceneStructure(scene_patch),
                EditPatch::Arrangement(arrangement_patch),
            ]),
            retained_bytes,
        );
        // Recording auto-selects (takes spec 16.3): post-record tweaks bind
        // to the take the performer just played. With several lanes recorded
        // the lowest track wins — the binding selection is a single clip.
        if let Some(lane) = lanes.iter().min_by_key(|lane| lane.track) {
            self.select_committed_take(lane.track, lane.take_id);
        }
        let row_count = self
            .state
            .committed_song()
            .map(|song| song.rows.len())
            .unwrap_or(0);
        Ok(format!(
            "Arrangement capture committed: {row_count} row(s), {} take(s), end beat {:.3}",
            lanes.len(),
            arrangement.end_beat
        ))
    }
}

/// Two lane resolutions (source pool id + phase in steps) are the same launch
/// when they name the same pattern at the same phase. Offsets are compared
/// with the stamping epsilon: the free-run and scene-anchored derivations of
/// one phase are algebraically equal but not bit-identical.
fn lane_resolutions_equal(a: Option<(u64, f64)>, b: Option<(u64, f64)>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((pattern_a, offset_a)), Some((pattern_b, offset_b))) => {
            pattern_a == pattern_b && (offset_a - offset_b).abs() < 1e-9
        }
        _ => false,
    }
}

/// Whether the override a clip compiles to at some beat is the same launch as
/// the captured resolution there — i.e. whether the clip plays it already.
fn lane_resolution_matches(
    over: &ProjectSongTrackOverride,
    resolution: Option<(u64, f64)>,
) -> bool {
    if over.take_id.is_some() {
        return false;
    }
    lane_resolutions_equal(over.pattern_id.map(|id| (id, over.offset_steps)), resolution)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(start_beat: f64, scene: usize, overrides: Vec<(usize, PatternId)>) -> CapturedSongState {
        CapturedSongState {
            start_beat,
            scene,
            overrides,
            touched: std::collections::BTreeSet::new(),
        }
    }

    /// A scene launch latches every lane (takes spec 10).
    fn all_touched(mut state: CapturedSongState) -> CapturedSongState {
        state.touched = (0..crate::sequencer::MAX_TRACKS).collect();
        state
    }

    /// A track launch latches its tracks.
    fn touched(mut state: CapturedSongState, tracks: &[usize]) -> CapturedSongState {
        state.touched = tracks.iter().copied().collect();
        state
    }

    fn scene_event(beat: f64, scene: usize) -> CaptureLaunchEvent {
        CaptureLaunchEvent {
            beat,
            kind: CaptureLaunchKind::Scene {
                scene,
                take_lanes: 0,
            },
        }
    }

    /// A scene launch performed while `take_lanes` were playing takes.
    fn scene_event_with_take_lanes(
        beat: f64,
        scene: usize,
        take_lanes: u64,
    ) -> CaptureLaunchEvent {
        CaptureLaunchEvent {
            beat,
            kind: CaptureLaunchKind::Scene { scene, take_lanes },
        }
    }

    #[test]
    fn scene_launch_does_not_claim_take_lanes() {
        let initial = state(0.0, 0, vec![]);
        let rows = consolidate(
            &initial,
            &[scene_event_with_take_lanes(4.0, 1, 0b10)],
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[1].touched.contains(&0), "pattern lane is claimed");
        assert!(
            !rows[1].touched.contains(&1),
            "the lane playing a take stays untouched — its take survives the splice"
        );
        assert!(rows[1].touched.contains(&2));
    }

    fn tracks_event(beat: f64, overrides: Vec<(usize, PatternId)>) -> CaptureLaunchEvent {
        CaptureLaunchEvent {
            beat,
            kind: CaptureLaunchKind::Tracks { overrides },
        }
    }

    #[test]
    fn consolidate_keeps_initial_row_and_orders_by_beat() {
        let initial = state(0.0, 0, vec![(1, PatternId(2))]);
        // Recorded out of order: a quantized deadline (4.0) can be drained
        // after an immediate launch at 4.7 was applied.
        let rows = consolidate(
            &initial,
            &[scene_event(4.7, 2), scene_event(4.0, 1)],
        );
        assert_eq!(
            rows,
            vec![
                state(0.0, 0, vec![(1, PatternId(2))]),
                all_touched(state(4.0, 1, Vec::new())),
                all_touched(state(4.7, 2, Vec::new())),
            ]
        );
    }

    #[test]
    fn same_boundary_scene_clears_overrides_regardless_of_event_order() {
        let initial = state(0.0, 0, Vec::new());
        let launches = [
            tracks_event(4.0, vec![(0, PatternId(3))]),
            scene_event(4.0, 2),
            tracks_event(4.0, vec![(1, PatternId(5))]),
        ];
        let rows = consolidate(&initial, &launches);
        let expected = all_touched(state(4.0, 2, vec![(0, PatternId(3)), (1, PatternId(5))]));
        assert_eq!(rows, vec![initial.clone(), expected.clone()]);

        // Reversed input order: identical result (spec 10.4).
        let mut reversed = launches.to_vec();
        reversed.reverse();
        let rows = consolidate(&initial, &reversed);
        assert_eq!(rows, vec![initial, expected]);
    }

    #[test]
    fn same_boundary_track_launch_updates_previous_state_without_scene() {
        let initial = state(0.0, 1, vec![(0, PatternId(9))]);
        let rows = consolidate(
            &initial,
            &[tracks_event(2.5, vec![(1, PatternId(4))])],
        );
        assert_eq!(
            rows,
            vec![
                initial,
                touched(state(2.5, 1, vec![(0, PatternId(9)), (1, PatternId(4))]), &[1]),
            ]
        );
    }

    #[test]
    fn repeated_identical_state_produces_no_row() {
        let initial = state(0.0, 0, Vec::new());
        let rows = consolidate(
            &initial,
            &[scene_event(2.0, 1), scene_event(4.0, 1), scene_event(6.0, 0)],
        );
        assert_eq!(
            rows,
            vec![
                state(0.0, 0, Vec::new()),
                all_touched(state(2.0, 1, Vec::new())),
                all_touched(state(6.0, 0, Vec::new())),
            ]
        );
    }

    #[test]
    fn launch_at_beat_zero_replaces_the_initial_row() {
        let initial = state(0.0, 0, vec![(0, PatternId(1))]);
        let rows = consolidate(&initial, &[scene_event(0.0, 2)]);
        assert_eq!(rows, vec![all_touched(state(0.0, 2, Vec::new()))]);
    }

    #[test]
    fn later_track_launch_for_same_track_wins_within_a_boundary() {
        let initial = state(0.0, 0, Vec::new());
        let rows = consolidate(
            &initial,
            &[
                tracks_event(4.0, vec![(0, PatternId(1))]),
                tracks_event(4.0, vec![(0, PatternId(3))]),
            ],
        );
        assert_eq!(
            rows,
            vec![initial, touched(state(4.0, 0, vec![(0, PatternId(3))]), &[0])]
        );
    }
}
