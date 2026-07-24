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
}

/// The resolved launch identity of one captured event.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CaptureLaunchKind {
    /// A scene launch: sets the base scene and clears every override.
    Scene { scene: usize },
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
                CaptureLaunchKind::Scene { scene } => Some(*scene),
                CaptureLaunchKind::Tracks { .. } => None,
            });
        let (scene, mut overrides) = match last_scene {
            Some(scene) => (scene, BTreeMap::new()),
            None => (
                previous.scene,
                previous.overrides.iter().copied().collect::<BTreeMap<_, _>>(),
            ),
        };
        for event in group {
            if let CaptureLaunchKind::Tracks { overrides: pairs } = &event.kind {
                for (track, pattern) in pairs {
                    overrides.insert(*track, *pattern);
                }
            }
        }
        let state = CapturedSongState {
            start_beat: boundary,
            scene,
            overrides: overrides.into_iter().collect(),
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
    // earlier row survives, mirroring `ProjectSong::normalize`.
    rows.dedup_by(|later, earlier| {
        earlier.scene == later.scene && earlier.overrides == later.overrides
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
        self.song_capture_take = Some(SongCaptureTake {
            origin_beats: self.state.scheduler_rendered_beats(),
            initial: CapturedSongState {
                start_beat: 0.0,
                scene: scenes.current_scene,
                overrides,
            },
            events: Vec::new(),
        });
    }

    /// Discard the staging take (Cancel, spec 7.4.8). The committed song is
    /// preserved by construction: the take never touched it.
    pub(crate) fn discard_song_capture_take(&mut self) {
        self.song_capture_take = None;
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
            PatternLaunchTarget::Scene { scene } => CaptureLaunchKind::Scene { scene: *scene },
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
            (Some(_), None) => {
                // No launches performed: nothing to splice; the committed
                // song is untouched and no undo entry is created (spec 9.1).
                return Ok(
                    "Arrangement capture ended: no launches captured; the committed song \
                     is unchanged"
                        .to_string(),
                );
            }
            (Some(previous), Some(punch_in)) => {
                // Existing rows before the punch-in survive verbatim
                // (offsets and explicit-empty overrides included).
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
                // Captured state from the punch-in on: the captured row
                // governing `punch_in` (re-based to start exactly there),
                // then every later captured row. Row zero of `captured`
                // always exists, so a governing row is guaranteed.
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
                    if start_beat > 0.0 && start_beat >= end_beat {
                        // A launch audible at or after the Stop boundary was
                        // never part of the audible performance: drop it.
                        continue;
                    }
                    specs.push(captured_row_spec(start_beat, row));
                }
                final_end_beat = previous.end_beat.max(end_beat);
            }
            (None, _) => {
                // Whole-song commit, exactly as before this stopgap.
                specs.extend(
                    captured
                        .iter()
                        .filter(|row| row.start_beat == 0.0 || row.start_beat < end_beat)
                        .map(|row| captured_row_spec(row.start_beat, row)),
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

        let row_count = specs.len();
        self.song_replace(specs, final_end_beat, loop_enabled)
            .map_err(|error| format!("the captured take could not be committed: {error}"))?;
        Ok(format!(
            "Arrangement capture committed: {row_count} row(s), end beat {final_end_beat:.3}"
        ))
    }
}

fn captured_row_spec(start_beat: f64, row: &CapturedSongState) -> SongRowSpec {
    SongRowSpec {
        start_beat,
        scene: row.scene,
        overrides: row
            .overrides
            .iter()
            .map(|(track, id)| {
                crate::sequencer::ProjectSongTrackOverride::new(*track, Some(id.0))
            })
            .collect(),
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
        }
    }

    fn scene_event(beat: f64, scene: usize) -> CaptureLaunchEvent {
        CaptureLaunchEvent {
            beat,
            kind: CaptureLaunchKind::Scene { scene },
        }
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
                state(4.0, 1, Vec::new()),
                state(4.7, 2, Vec::new()),
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
        let expected = state(4.0, 2, vec![(0, PatternId(3)), (1, PatternId(5))]);
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
                state(2.5, 1, vec![(0, PatternId(9)), (1, PatternId(4))]),
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
                state(2.0, 1, Vec::new()),
                state(6.0, 0, Vec::new()),
            ]
        );
    }

    #[test]
    fn launch_at_beat_zero_replaces_the_initial_row() {
        let initial = state(0.0, 0, vec![(0, PatternId(1))]);
        let rows = consolidate(&initial, &[scene_event(0.0, 2)]);
        assert_eq!(rows, vec![state(0.0, 2, Vec::new())]);
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
        assert_eq!(rows, vec![initial, state(4.0, 0, vec![(0, PatternId(3))])]);
    }
}
