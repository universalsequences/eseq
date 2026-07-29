//! Edit focus — the editors' resolved target (docs/clip-edit-target-spec.md 3).
//!
//! The focus IS the sound binding (`song_clip_selection`, locked decision 1):
//! there is no second "which clip" state that could disagree with the panel,
//! the monitor, or the record-clone template. This module only *resolves* the
//! binding onto concrete note storage for the editors:
//!
//! - `Live` — the target is the track's effective pattern, whose authoritative
//!   copy is the live mirror lanes (today's editor behavior, by construction).
//! - `Pattern` — a pinned pool pattern that is NOT currently effective; reads
//!   and writes address the pool `TrackPatternData` directly (spec 3.4).
//! - `Take` — a pinned take; the note axis is the take's continuous step axis,
//!   mapped through `TrackTake::chunk_step_at` onto its chunk patterns.

use crate::sequencer::{PatternId, TakeId, MAX_STEPS};

use super::sound_binding::BoundSource;
use super::App;

/// Where a track's edits land, resolved from the sound binding at use time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFocus {
    /// The effective pattern: live mirror lanes are authoritative.
    Live { track: usize },
    /// A pinned pool pattern that is not currently effective.
    Pattern { track: usize, pattern: PatternId },
    /// A pinned take: edits address chunk patterns through the take axis.
    Take { track: usize, take: TakeId },
}

impl EditFocus {
    pub fn track(self) -> usize {
        match self {
            EditFocus::Live { track }
            | EditFocus::Pattern { track, .. }
            | EditFocus::Take { track, .. } => track,
        }
    }

    /// True when the live mirror is the write target (today's paths apply).
    pub fn is_live(self) -> bool {
        matches!(self, EditFocus::Live { .. })
    }
}

impl App {
    /// Resolve the track's edit focus from the sound binding (spec 3.1/3.2).
    ///
    /// A bound pattern that happens to be the effective pattern resolves to
    /// `Live` — that is `capture_pattern_step_cells`'s
    /// live-when-effective-else-pool rule applied at resolution time, so the
    /// editors and the write path always agree on where the truth is.
    pub fn track_edit_focus(&self, track: usize) -> EditFocus {
        match self.track_sound_binding(track).source {
            Some(BoundSource::Take(take)) => EditFocus::Take { track, take },
            Some(BoundSource::Pattern(pattern)) => {
                if self.state.effective_track_pattern_id(track) == Some(pattern) {
                    EditFocus::Live { track }
                } else {
                    EditFocus::Pattern { track, pattern }
                }
            }
            // A bare track has nothing pinned; editors keep addressing the
            // live lanes (which a first edit materializes, takes spec 11.1).
            None => EditFocus::Live { track },
        }
    }

    /// The focused source's step-axis length (spec 3.5): pattern targets use
    /// their `num_steps`, takes their playable `total_len_steps`. Published as
    /// `SEQ.focus-num-steps` — a sibling of `SEQ.tp-num-steps`, which keeps
    /// meaning the live value until the step grid is ported.
    pub fn focus_num_steps(&self, track: usize) -> usize {
        match self.track_edit_focus(track) {
            EditFocus::Live { track } => self
                .state
                .pattern
                .track_params
                .get(track)
                .map(|params| params.get_num_steps())
                .unwrap_or(16)
                .min(MAX_STEPS),
            EditFocus::Pattern { track, pattern } => self
                .state
                .capture_pattern_num_steps(track, pattern)
                .unwrap_or(16)
                .min(MAX_STEPS),
            EditFocus::Take { track, take } => self.state.with_project_scenes(|scenes| {
                scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(take))
                    .map(|take| take.total_len_steps as usize)
                    .unwrap_or(16)
            }),
        }
    }

    /// Human-readable focus label for the editor header (spec 3.5):
    /// `"Pattern 3 — 4 clips"` / `"Take 2"` / `"Pattern 5 (scene)"`. The
    /// clip-use count is a lane scan at publish time, not a new index.
    pub fn focus_label(&self, track: usize) -> Option<String> {
        match self.track_edit_focus(track) {
            EditFocus::Take { track, take } => {
                Some(self.state.track_take(track, take)?.name)
            }
            EditFocus::Pattern { track, pattern } => {
                let clips = self.pattern_clip_use_count(track, pattern);
                Some(if clips > 0 {
                    format!(
                        "Pattern {} — {} clip{}",
                        pattern.0,
                        clips,
                        if clips == 1 { "" } else { "s" }
                    )
                } else {
                    format!("Pattern {}", pattern.0)
                })
            }
            EditFocus::Live { track } => {
                let pattern = self.state.effective_track_pattern_id(track)?;
                Some(format!("Pattern {} (scene)", pattern.0))
            }
        }
    }

    /// Editor playhead on the FOCUS axis (spec 3.3.4). `None` = hidden.
    ///
    /// Live focus keeps today's behavior (the live playhead). A pinned focus
    /// shows a playhead only while the song is actually sounding it: the
    /// position is clip-relative,
    /// `offset_steps + (song_pos − clip_start)·steps_per_beat`, wrapped for
    /// patterns (through `pattern_play_step`) and linear-silent-past-end for
    /// takes; hidden whenever the song position is outside the clip span.
    pub fn focus_playhead_step(&self, track: usize, live_playhead: usize) -> Option<f64> {
        let focus = self.track_edit_focus(track);
        if focus.is_live() {
            return Some(live_playhead as f64);
        }
        if !(self.song_playback_authority_active() && self.state.is_playing()) {
            return None;
        }
        let pos = self.state.song_position_beats()?;
        let arrangement = self.state.committed_arrangement()?;
        // Prefer the selected clip (the pinned intent); otherwise any clip of
        // this source under the playhead (rule-2 audible focus).
        let lane = arrangement.track_lanes.get(track)?;
        let matches_focus = |clip: &crate::sequencer::ArrClip| match focus {
            EditFocus::Pattern { pattern, .. } => clip.pattern_id == Some(pattern.0),
            EditFocus::Take { take, .. } => clip.take_id == Some(take.0),
            EditFocus::Live { .. } => false,
        };
        let selected = self
            .song_clip_selection
            .filter(|selection| selection.track == track)
            .and_then(|selection| arrangement.find_clip(selection.clip_id))
            .map(|(_, clip)| clip)
            .filter(|clip| matches_focus(clip));
        let clip = match selected {
            Some(clip) => clip,
            None => lane
                .iter()
                .find(|clip| matches_focus(clip) && pos >= clip.start_beat && pos < clip.end_beat)?,
        };
        if pos < clip.start_beat || pos >= clip.end_beat {
            return None;
        }
        let delta_beats = pos - clip.start_beat;
        self.state.with_project_scenes(|scenes| {
            use crate::sequencer::SongCompileContext;
            match focus {
                EditFocus::Pattern { pattern, .. } => {
                    let (steps_per_beat, num_steps) =
                        scenes.song_track_pattern_step_mapping(track, pattern.0)?;
                    Some(crate::sequencer::pattern_play_step(
                        clip.offset_steps,
                        delta_beats * steps_per_beat,
                        (0.0, num_steps),
                    ))
                }
                EditFocus::Take { take, .. } => {
                    let (steps_per_beat, total_len) =
                        scenes.song_track_take_step_mapping(track, take.0)?;
                    let p = clip.offset_steps + delta_beats * steps_per_beat;
                    (p < total_len).then_some(p)
                }
                EditFocus::Live { .. } => None,
            }
        })
    }

    /// How many arrangement clips on `track` reference `pattern` — the
    /// "editing shared material" tell for the loop bar and header (spec 5).
    pub fn pattern_clip_use_count(&self, track: usize, pattern: PatternId) -> usize {
        self.state
            .committed_arrangement()
            .and_then(|arrangement| {
                arrangement.track_lanes.get(track).map(|lane| {
                    lane.iter()
                        .filter(|clip| clip.pattern_id == Some(pattern.0))
                        .count()
                })
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sound_binding::tests::app_with_take;
    use crate::sequencer::ClipId;

    /// Session mode / nothing selected: the focus is the live mirror.
    #[test]
    fn follow_mode_resolves_to_the_live_lanes() {
        let (app, _take, _scene_pattern, _chunks) = app_with_take();
        assert_eq!(app.track_edit_focus(0), EditFocus::Live { track: 0 });
    }

    /// A selected take pins the take axis as the edit target.
    #[test]
    fn selecting_a_take_clip_pins_the_take_focus() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        assert_eq!(app.track_edit_focus(0), EditFocus::Take { track: 0, take });
        // Take length is the playable length, not a pattern num-steps.
        assert_eq!(app.focus_num_steps(0), 300);
        assert_eq!(app.focus_label(0).as_deref(), Some("Take 1"));
    }

    /// A pinned pattern that IS the effective pattern resolves to Live — the
    /// capture rule applied at resolution time.
    #[test]
    fn a_pinned_effective_pattern_resolves_to_live() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 16.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new(
            ClipId(0),
            0.0,
            16.0,
            Some(scene_pattern.0),
        ));
        arrangement.next_clip_id = 1;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        assert_eq!(app.track_edit_focus(0), EditFocus::Live { track: 0 });
    }

    /// A pinned NON-effective pattern is a pool target, and clears back to
    /// Live when the binding is dropped (spec 3.3 lifecycle).
    #[test]
    fn a_pinned_other_pattern_is_a_pool_target_until_deselected() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let other = app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .clone();
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
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        assert_eq!(
            app.track_edit_focus(0),
            EditFocus::Pattern { track: 0, pattern: other }
        );
        assert_eq!(
            app.focus_label(0).as_deref(),
            Some(format!("Pattern {} — 1 clip", other.0).as_str())
        );

        app.set_song_clip_selection(None);
        assert_eq!(app.track_edit_focus(0), EditFocus::Live { track: 0 });
    }

    /// Leaving the arrangement view drops the pinned focus (dormancy) — the
    /// worst failure mode is the step grid "mysteriously not editing what's
    /// playing" in session mode (spec 3.3.2).
    #[test]
    fn hiding_the_arrangement_view_returns_focus_to_live() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        assert!(matches!(app.track_edit_focus(0), EditFocus::Take { .. }));
        app.set_arrangement_view_visible(false);
        assert_eq!(app.track_edit_focus(0), EditFocus::Live { track: 0 });
    }

    fn pool_step_active(app: &App, pattern: PatternId, step: usize) -> bool {
        app.state
            .with_pool_pattern(0, pattern, |data| {
                data.track_bits[step / 64] >> (step % 64) & 1 == 1
            })
            .expect("pattern in pool")
    }

    fn set_pool_step_active(app: &App, pattern: PatternId, step: usize) {
        app.state
            .with_pool_pattern_mut(0, pattern, |data| {
                data.track_bits[step / 64] |= 1 << (step % 64);
            })
            .expect("pattern in pool");
    }

    /// Spec 3.4: a pinned non-effective pattern gets POOL writes — the live
    /// mirror is untouched, the edit records one undoable entry, and undo
    /// restores the pool copy.
    #[test]
    fn a_pinned_pattern_edit_writes_the_pool_and_spares_the_live_mirror() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let other = app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .clone();
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
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        let focus = app.track_edit_focus(0);
        assert_eq!(focus, EditFocus::Pattern { track: 0, pattern: other });

        let depth = app.history.undo_len();
        let outcome = crate::app::edit::apply_recorded_focus_step_mutation(
            &mut app,
            focus,
            &[2],
            "Test pool note",
            |app| {
                set_pool_step_active(app, other, 2);
                Ok(())
            },
        )
        .expect("pool edit applies");
        assert!(matches!(outcome, crate::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");
        assert!(pool_step_active(&app, other, 2));
        assert!(
            !app.state.pattern.patterns[0].is_active(2),
            "the live mirror is never touched by a pool-target write"
        );
        assert!(
            !pool_step_active(&app, scene_pattern, 2),
            "the scene pattern is not dual-written"
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!pool_step_active(&app, other, 2), "undo restores the pool");
    }

    /// Spec 3.4 take writes: focus-axis steps map through `chunk_step_at`
    /// onto the owning chunk's pool pattern; a multi-chunk gesture is ONE
    /// history entry; the silent tail rejects writes.
    #[test]
    fn take_edits_map_through_chunks_as_one_history_entry() {
        let (mut app, _take, _scene_pattern, chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        let focus = app.track_edit_focus(0);
        assert!(matches!(focus, EditFocus::Take { .. }));

        // Steps 2 and 260 on the take axis: chunk 0 local 2, chunk 1 local 4.
        let depth = app.history.undo_len();
        let outcome = crate::app::edit::apply_recorded_focus_step_mutation(
            &mut app,
            focus,
            &[2, 260],
            "Test take notes",
            |app| {
                set_pool_step_active(app, chunks[0], 2);
                set_pool_step_active(app, chunks[1], 4);
                Ok(())
            },
        )
        .expect("take edit applies");
        assert!(matches!(outcome, crate::app::edit::EditOutcome::Applied(_)));
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(!pool_step_active(&app, chunks[0], 2));
        assert!(!pool_step_active(&app, chunks[1], 4));
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(pool_step_active(&app, chunks[0], 2));
        assert!(pool_step_active(&app, chunks[1], 4));

        // Past the playable end (total_len_steps = 300): rejected outright.
        assert!(crate::app::edit::FocusStepGesture::begin(
            &mut app,
            focus,
            &[300],
            "Silent tail"
        )
        .is_err());
    }

    /// Review regression: a Live-focus history edit must publish the
    /// scheduler snapshot exactly like the legacy recorded-step path did via
    /// `replay_step_patch(Redo)` — without it the new note is silent until
    /// an unrelated action publishes.
    #[test]
    fn a_live_focus_edit_publishes_the_scheduler_snapshot() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        let focus = app.track_edit_focus(0);
        assert!(focus.is_live());
        let before = app.state.latest_scheduler_snapshot();
        crate::app::edit::apply_recorded_focus_step_mutation(
            &mut app,
            focus,
            &[2],
            "Test live note",
            |app| {
                app.state.pattern.patterns[0].set_step_active(2, true);
                Ok(())
            },
        )
        .expect("live edit applies");
        let after = app.state.latest_scheduler_snapshot();
        assert!(
            !std::sync::Arc::ptr_eq(&before, &after),
            "a live-target edit must publish the scheduler snapshot"
        );
    }

    /// Review regression (spec 3.3.3): in follow mode the focus carries no
    /// pattern id, so the bail must compare the effective pattern against
    /// the pattern the gesture BEGAN on — a launch mid-drag aborts, never
    /// silently retargets onto the launched scene's pattern.
    #[test]
    fn a_live_gesture_bails_when_the_effective_pattern_moves() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let focus = app.track_edit_focus(0);
        assert!(focus.is_live());
        let mut gesture =
            crate::app::edit::FocusStepGesture::begin(&mut app, focus, &[2], "Test drag")
                .expect("gesture begins");
        // Simulate a launch: the scene cell now resolves a different pattern.
        app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .clone();
            let id = scenes.track_pools[0].insert(data);
            scenes.scenes[0].cells[0] = Some(id);
        });
        assert!(
            gesture.capture_additional_steps(&mut app, &[3]).is_err(),
            "a moved effective pattern must abort the follow-mode drag"
        );
        gesture.rollback(&mut app).expect("rollback succeeds");
    }

    /// Spec 3.3.3: the gesture bails when the RESOLVED focus moves under it —
    /// here a deselection mid-drag.
    #[test]
    fn a_gesture_bails_when_the_focus_moves_under_it() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        let focus = app.track_edit_focus(0);
        let mut gesture =
            crate::app::edit::FocusStepGesture::begin(&mut app, focus, &[2], "Test drag")
                .expect("gesture begins");
        gesture
            .capture_additional_steps(&mut app, &[3])
            .expect("same focus extends fine");

        app.set_song_clip_selection(None);
        assert!(
            gesture.capture_additional_steps(&mut app, &[4]).is_err(),
            "a moved focus must abort the drag, not retarget it"
        );
        gesture.rollback(&mut app).expect("rollback succeeds");
    }
}
