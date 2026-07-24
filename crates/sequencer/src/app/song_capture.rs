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
//! commits atomically through the existing `song_replace` primitive: one
//! project mutation, one undo entry, fresh row ids. Overflow or any
//! normalization/validation failure leaves the previous committed song
//! intact and surfaces an actionable error.

use std::collections::BTreeMap;

use crate::quantized_launch::PatternLaunchTarget;
use crate::sequencer::PatternId;

use super::song_edit::SongRowSpec;
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
    events: Vec<CaptureLaunchEvent>,
}

impl SongCaptureTake {
    pub(crate) fn event_count(&self) -> usize {
        self.events.len()
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
        let touched: std::collections::BTreeSet<usize> =
            if self.state.committed_song().is_some() {
                std::collections::BTreeSet::new()
            } else {
                (0..self.tracks.len()).collect()
            };
        self.song_capture_take = Some(SongCaptureTake {
            origin_beats,
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
    }

    /// Stop-commit (spec 7.4.7/10.4): normalize the take and atomically
    /// replace the committed song through the `song_replace` primitive (one
    /// project mutation, one undo entry, fresh row ids). `end_raw_beats` is
    /// the scheduler rendered-beat clock at Stop — the same clock the events
    /// were recorded against. On any failure the previous committed song is
    /// intact, the failure state is latched for the `song-capture-failed` /
    /// `song-capture-error` bindings, and the take is discarded.
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
        let previous = self.state.committed_song();
        // Keep the previous song's loop preference; a fresh project captures
        // a non-looping song.
        let loop_enabled = previous
            .as_ref()
            .map(|song| song.loop_enabled)
            .unwrap_or(false);

        // Splice stopgap (takes spec 9.5): with an existing committed song,
        // the commit replaces it only from the FIRST captured launch event's
        // beat onward — hitting record and listening before the first launch
        // must not erase the head of the song. Recording over an empty
        // project keeps the whole-song commit (spec 9.3: "record from an
        // empty song").
        let punch_in = take
            .events
            .iter()
            .map(|event| event.beat)
            .min_by(|a, b| a.partial_cmp(b).expect("capture beats are finite"));

        let mut specs: Vec<SongRowSpec> = Vec::new();
        let final_end_beat;
        match (&previous, punch_in) {
            (Some(previous), None) => {
                // No launches performed: nothing to splice; the committed
                // song is untouched (spec 9.1) — unless takes were recorded,
                // in which case they commit onto the unchanged rows.
                if pending.is_empty() {
                    return Ok(
                        "Arrangement capture ended: no launches captured; the committed \
                         song is unchanged"
                            .to_string(),
                    );
                }
                specs.extend(previous.rows.iter().map(|row| SongRowSpec {
                    start_beat: row.start_beat,
                    scene: row.scene,
                    overrides: row.overrides.clone(),
                }));
                final_end_beat = previous.end_beat;
            }
            (Some(previous), Some(punch_in)) => {
                // Full region splice (takes spec 9.1/9.2): replace-in-place
                // between the first captured launch `P` and the stop beat
                // `Q`. Existing rows before `P` survive verbatim (offsets
                // and explicit-empty overrides included).
                let punch_out = end_beat.max(punch_in);
                specs.extend(
                    previous
                        .rows
                        .iter()
                        .filter(|row| row.start_beat < punch_in)
                        .map(|row| SongRowSpec {
                            start_beat: row.start_beat,
                            scene: row.scene,
                            overrides: row.overrides.clone(),
                        }),
                );
                // Captured state over [P, Q): the captured row governing
                // `punch_in` (re-based to start exactly there), then every
                // later captured row inside the region. Row zero of
                // `captured` always exists, so a governing row is
                // guaranteed. Untouched lanes inherit the pre-existing
                // arrangement's resolution, materialized (spec 9.4).
                let governing = captured
                    .iter()
                    .rposition(|row| row.start_beat <= punch_in)
                    .expect("captured rows always include a beat-zero row");
                for (idx, row) in captured.iter().enumerate().skip(governing) {
                    let start_beat = if idx == governing {
                        punch_in
                    } else {
                        row.start_beat
                    };
                    if start_beat > 0.0 && start_beat >= punch_out {
                        // A launch audible at or after the Stop boundary was
                        // never part of the audible performance: drop it.
                        continue;
                    }
                    specs.push(self.stamped_captured_row_spec(start_beat, row, Some(previous)));
                }
                // The row beginning at `Q` restores the pre-existing
                // arrangement from `Q` onward (spec 9.2 step 5): nothing
                // after `Q` moves; rows past `Q` survive verbatim.
                if punch_out < previous.end_beat {
                    if !previous
                        .rows
                        .iter()
                        .any(|row| row.start_beat == punch_out)
                    {
                        if let Some(governing_prev) =
                            crate::sequencer::state_at_beat(previous, punch_out)
                        {
                            specs.push(SongRowSpec {
                                start_beat: punch_out,
                                scene: governing_prev.scene,
                                overrides: self.split_row_state(governing_prev, punch_out),
                            });
                        }
                    }
                    specs.extend(
                        previous
                            .rows
                            .iter()
                            .filter(|row| row.start_beat >= punch_out)
                            .map(|row| SongRowSpec {
                                start_beat: row.start_beat,
                                scene: row.scene,
                                overrides: row.overrides.clone(),
                            }),
                    );
                }
                final_end_beat = previous.end_beat.max(end_beat);
            }
            (None, _) => {
                // Whole-song commit, exactly as before this stopgap.
                specs.extend(
                    captured
                        .iter()
                        .filter(|row| row.start_beat == 0.0 || row.start_beat < end_beat)
                        .map(|row| self.stamped_captured_row_spec(row.start_beat, row, None)),
                );
                final_end_beat = end_beat;
            }
        }
        // The seam between the preserved head and the spliced tail may leave
        // adjacent identical states; drop the later one (the same canonical
        // form `ProjectSong::normalize` enforces) so validation accepts it.
        specs.dedup_by(|later, earlier| {
            earlier.scene == later.scene && earlier.overrides == later.overrides
        });

        // No takes and (by the arm above) no launches ⇒ nothing to commit
        // for a fresh project either: the transport ran and nothing was
        // performed.
        if pending.is_empty() {
            let row_count = specs.len();
            self.song_replace(specs, final_end_beat, loop_enabled)
                .map_err(|error| {
                    format!("the captured take could not be committed: {error}")
                })?;
            return Ok(format!(
                "Arrangement capture committed: {row_count} row(s), end beat \
                 {final_end_beat:.3}"
            ));
        }
        self.commit_capture_with_takes(specs, final_end_beat, loop_enabled, pending)
    }

    /// Atomic commit of the launch splice PLUS the recorded takes (takes
    /// spec 8.5): register every pending take, rebuild the song from the
    /// splice specs, repoint each recorded region `[P, Q)` at its take (rows
    /// re-anchored with `offset = steps(row.start - P)`), extend the song
    /// end when a take runs past it, and commit ONE composite undo entry
    /// (scenes + song). Any failure rolls both back.
    fn commit_capture_with_takes(
        &mut self,
        specs: Vec<SongRowSpec>,
        end_beat: f64,
        loop_enabled: bool,
        pending: Vec<(usize, super::take_recording::PendingTakeLane)>,
    ) -> Result<String, String> {
        use super::history::{EditPatch, SceneStructurePatch, SongStructurePatch};

        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        let song_before = self.state.committed_song();
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

        // Build the spliced song (mirrors `song_replace`: sorted rows, fresh
        // ids continuing the previous allocator).
        let mut song = crate::sequencer::ProjectSong {
            rows: Vec::with_capacity(specs.len()),
            end_beat,
            loop_enabled,
            next_row_id: song_before
                .as_ref()
                .map(|song| song.next_row_id)
                .unwrap_or(0),
        };
        let mut sorted = specs;
        sorted.sort_by(|a, b| {
            a.start_beat
                .partial_cmp(&b.start_beat)
                .expect("song row start beats are finite")
        });
        let build = (|| -> Result<crate::sequencer::ProjectSong, String> {
            for spec in sorted {
                let row_id = song.allocate_row_id()?;
                let mut overrides = spec.overrides;
                overrides.sort_by_key(|over| over.track);
                song.rows.push(crate::sequencer::ProjectSongRow {
                    id: row_id,
                    start_beat: spec.start_beat,
                    scene: spec.scene,
                    overrides,
                });
            }
            // A take running past the song end extends it (spec 8.5).
            for lane in &lanes {
                song.end_beat = song.end_beat.max(lane.punch_out_beat);
            }
            let mut song = song.clone();
            for lane in &lanes {
                song = self.paint_take_region(
                    &song,
                    lane.track,
                    lane.punch_in_beat,
                    lane.punch_out_beat.min(song.end_beat),
                    lane.take_id,
                    lane.step_beats,
                )?;
            }
            song.normalize();
            Ok(song)
        })();
        let song_after = match build {
            Ok(song) => song,
            Err(error) => {
                rollback(self, &lanes)?;
                return Err(format!("recorded takes could not be committed: {error}"));
            }
        };
        {
            let scenes = self.state.capture_project_scenes();
            if let Err(error) = song_after.validate(&scenes) {
                rollback(self, &lanes)?;
                return Err(format!(
                    "the captured take could not be committed: {error}"
                ));
            }
        }
        self.state.set_committed_song(Some(song_after.clone()));
        let scenes_after = self.state.capture_project_scenes();
        let scene_patch = SceneStructurePatch {
            before: scenes_before,
            after: scenes_after,
        };
        let song_patch = SongStructurePatch {
            before: song_before,
            after: Some(song_after.clone()),
        };
        let retained_bytes = scene_patch.retained_bytes() + song_patch.retained_bytes();
        // Scenes first: redo restores the takes before the song referencing
        // them; undo removes the references before the takes (see
        // take_edit.rs for the same ordering rationale).
        self.history.commit(
            "Record arrangement takes",
            None,
            EditPatch::Composite(vec![
                EditPatch::SceneStructure(scene_patch),
                EditPatch::Song(song_patch),
            ]),
            retained_bytes,
        );
        Ok(format!(
            "Arrangement capture committed: {} row(s), {} take(s), end beat {:.3}",
            song_after.rows.len(),
            lanes.len(),
            song_after.end_beat
        ))
    }
}

impl App {
    /// Turn one consolidated captured state into a row spec with free-run
    /// phase stamped (takes spec 7.2/9.4): during capture every audible
    /// pattern free-runs against the global clock, so its position at any
    /// row start is `steps(start_beat) mod L`. Stamping that as the lane's
    /// offset makes committed playback reproduce the performance — an
    /// unquantized scene launch mid-bar re-enters its patterns mid-pattern
    /// ON the grid, instead of re-anchoring step 0 at an off-beat. Lanes
    /// resolved through the scene cell get a materialized override whenever
    /// their offset is nonzero (locked decision: phase lives on overrides
    /// only).
    fn stamped_captured_row_spec(
        &self,
        start_beat: f64,
        row: &CapturedSongState,
        previous: Option<&crate::sequencer::ProjectSong>,
    ) -> SongRowSpec {
        let scene_cells: Vec<Option<PatternId>> = self.state.with_project_scenes(|scenes| {
            (0..scenes.track_pools.len())
                .map(|track| {
                    scenes
                        .scenes
                        .get(row.scene)
                        .and_then(|scene| scene.cells.get(track))
                        .copied()
                        .flatten()
                })
                .collect()
        });
        // Inheritance for untouched lanes (takes spec 9.4): the pre-existing
        // arrangement's resolution at this beat, with lane offsets advanced
        // so playback continues unchanged. `split_row_state` computes
        // exactly that (override sources advanced; scene-resolved lanes
        // materialized only when their offset is nonzero).
        let inherited = previous.and_then(|previous| {
            crate::sequencer::state_at_beat(previous, start_beat).map(|governing| {
                (
                    governing.scene,
                    self.split_row_state(governing, start_beat),
                    governing.start_beat,
                )
            })
        });
        let mut overrides = Vec::new();
        for (track, cell) in scene_cells.iter().enumerate() {
            if !row.touched.contains(&track) {
                if let Some(previous) = previous {
                    // Untouched lane: the song kept playing it (spec 9.3).
                    // Materialize its inherited resolution as an override so
                    // scene-clears-overrides consolidation cannot silence it
                    // (spec 9.4).
                    let over = match &inherited {
                        Some((prev_scene, inherited_overrides, prev_row_start)) => {
                            match inherited_overrides
                                .iter()
                                .find(|over| over.track == track)
                            {
                                Some(over) => *over,
                                None => {
                                    // Scene-resolved in the pre-existing
                                    // arrangement with offset 0 (or a
                                    // whole-cycle offset the split
                                    // collapsed): materialize explicitly.
                                    let prev_cell =
                                        self.state.with_project_scenes(|scenes| {
                                            scenes
                                                .scenes
                                                .get(*prev_scene)
                                                .and_then(|scene| scene.cells.get(track))
                                                .copied()
                                                .flatten()
                                        });
                                    match prev_cell {
                                        Some(pattern) => {
                                            let offset = self.advanced_offset(
                                                track,
                                                pattern.0,
                                                0.0,
                                                start_beat - prev_row_start,
                                            );
                                            crate::sequencer::ProjectSongTrackOverride {
                                                track,
                                                pattern_id: Some(pattern.0),
                                                take_id: None,
                                                offset_steps: offset,
                                            }
                                        }
                                        None => crate::sequencer::ProjectSongTrackOverride::new(
                                            track, None,
                                        ),
                                    }
                                }
                            }
                        }
                        // Past the pre-existing song's end: the lane was
                        // silent there.
                        None => crate::sequencer::ProjectSongTrackOverride::new(track, None),
                    };
                    let _ = previous;
                    overrides.push(over);
                    continue;
                }
            }
            let explicit = row
                .overrides
                .iter()
                .find(|(over_track, _)| *over_track == track)
                .map(|(_, id)| *id);
            let Some(pattern) = explicit.or(*cell) else {
                continue;
            };
            let offset_steps = self.advanced_offset(track, pattern.0, 0.0, start_beat);
            if explicit.is_some() || offset_steps != 0.0 {
                overrides.push(crate::sequencer::ProjectSongTrackOverride {
                    track,
                    pattern_id: Some(pattern.0),
                    take_id: None,
                    offset_steps,
                });
            }
        }
        SongRowSpec {
            start_beat,
            scene: row.scene,
            overrides,
        }
    }
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
