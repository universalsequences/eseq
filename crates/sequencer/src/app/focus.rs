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
        // Borrowed, never cloned: this runs once per rendered frame while the
        // song plays, and `committed_arrangement` deep-clones every lane.
        let (clip_start_beat, clip_end_beat, clip_offset_steps) =
            self.state.with_committed_arrangement(|arrangement| {
                let arrangement = arrangement?;
                // Prefer the selected clip (the pinned intent); otherwise any
                // clip of this source under the playhead (rule-2 audible
                // focus).
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
                    None => lane.iter().find(|clip| {
                        matches_focus(clip) && pos >= clip.start_beat && pos < clip.end_beat
                    })?,
                };
                Some((clip.start_beat, clip.end_beat, clip.offset_steps))
            })?;
        if pos < clip_start_beat || pos >= clip_end_beat {
            return None;
        }
        let delta_beats = pos - clip_start_beat;
        self.state.with_project_scenes(|scenes| {
            use crate::sequencer::SongCompileContext;
            match focus {
                EditFocus::Pattern { pattern, .. } => {
                    // Advance in the pattern's REAL geometry (timebase/sync
                    // plocks included) so the resolved source step matches
                    // what the runtime clock plays at `pos`.
                    let geometry = scenes.song_track_pattern_geometry(track, pattern.0)?;
                    Some(crate::sequencer::pattern_play_step(
                        geometry.advance(clip_offset_steps, delta_beats),
                        0.0,
                        (0.0, geometry.num_steps() as f64),
                    ))
                }
                EditFocus::Take { take, .. } => {
                    let (steps_per_beat, total_len) =
                        scenes.song_track_take_step_mapping(track, take.0)?;
                    let p = clip_offset_steps + delta_beats * steps_per_beat;
                    (p < total_len).then_some(p)
                }
                EditFocus::Live { .. } => None,
            }
        })
    }

    /// Loop-bar retarget (spec 5, locked decision 3): resize a PINNED
    /// pattern's `num_steps` — the shared pattern, every clip referencing it.
    /// The effective pattern keeps today's `SetTrackNumSteps` live path (the
    /// lisp routes it there), so this refuses effective targets outright.
    ///
    /// Applied per drag frame; frames coalesce into ONE undo entry through
    /// the merge-key gesture (sealed by `finish_focused_pattern_num_steps`
    /// or any other edit).
    pub fn set_pinned_pattern_num_steps(
        &mut self,
        track: usize,
        num_steps: usize,
    ) -> Result<bool, String> {
        let EditFocus::Pattern { pattern, .. } = self.track_edit_focus(track) else {
            return Err(
                "The loop bar only resizes a pinned pattern; takes are read-only and the \
                 live pattern uses the track path"
                    .to_string(),
            );
        };
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let target = crate::sequencer::TrackPatternId {
            track: track_id,
            pattern,
        };
        let num_steps = num_steps.clamp(1, MAX_STEPS);
        let merge_key = crate::app::history::MergeKey::new(format!(
            "focus-num-steps:{}:{}",
            target.track.0, target.pattern.0
        ));
        if self
            .history
            .active_gesture()
            .map(|gesture| &gesture.merge_key)
            != Some(&merge_key)
        {
            crate::app::edit::finish_active_gesture(self);
        }
        let current_before = self.state.capture_pattern_track_params(track, pattern)?;
        let base_note_bits = self
            .state
            .capture_pattern_instrument_base_note_offset(track, pattern)?
            .to_bits();
        let original_before = self
            .history
            .active_gesture_patch(&merge_key)
            .and_then(|patch| match patch {
                crate::app::history::EditPatch::TrackParams(patch)
                    if patch.target == target =>
                {
                    Some(patch.before.clone())
                }
                _ => None,
            });
        let gesture_before = original_before.clone().unwrap_or(current_before.clone());
        if current_before.num_steps == num_steps {
            // The frame changes nothing; if the whole DRAG is back at its
            // starting length, drop the staged entry so releasing commits no
            // no-op undo step (which would also have cleared the redo stack).
            if gesture_before.num_steps == num_steps {
                self.history.discard_active_gesture_entry(&merge_key);
            }
            return Ok(false);
        }
        self.state
            .with_pool_pattern_mut(track, pattern, |data| {
                data.track_params.num_steps = num_steps;
            })
            .ok_or_else(|| "The pinned pattern no longer exists".to_string())?;
        let after = self.state.capture_pattern_track_params(track, pattern)?;
        if gesture_before.num_steps == after.num_steps {
            // Dragged back to the start: the value moved this frame but the
            // gesture as a whole is a no-op.
            self.history.discard_active_gesture_entry(&merge_key);
            crate::app::edit::invalidate_song_rows_for_edit(self, track, pattern);
            return Ok(true);
        }
        let patch = crate::app::history::TrackParamsPatch {
            target,
            before: gesture_before,
            after,
            instrument_base_note_offset_before: base_note_bits,
            instrument_base_note_offset_after: base_note_bits,
        };
        let retained_bytes = patch.retained_bytes();
        crate::app::edit::ensure_coalescing_gesture(self, &merge_key);
        self.history.stage_active_gesture(
            "Resize pattern loop",
            &merge_key,
            crate::app::history::EditPatch::TrackParams(patch),
            retained_bytes,
        );
        // Deferred while the gesture is open, drained at the seal — so a
        // playing song re-preflights ONCE per drag, not per pointer frame.
        crate::app::edit::invalidate_song_rows_for_edit(self, track, pattern);
        Ok(true)
    }

    /// Seal the loop-bar drag's coalescing gesture into one undo entry; the
    /// seal drains the deferred song-row refresh queued per frame above.
    pub fn finish_focused_pattern_num_steps(&mut self, _track: usize) {
        crate::app::edit::finish_active_gesture(self);
    }

    /// Clip-panel Length edit for a TAKE focus (the pattern arm uses
    /// `set_pinned_pattern_num_steps` / the live path): resize the take's
    /// playable length. Picker drag frames coalesce into one undo entry.
    pub fn set_focused_take_length(
        &mut self,
        track: usize,
        len_steps: f64,
    ) -> Result<(), String> {
        let EditFocus::Take { take, .. } = self.track_edit_focus(track) else {
            return Err(
                "The Length field only resizes a take here; patterns use the loop path"
                    .to_string(),
            );
        };
        let merge_key = crate::app::history::MergeKey::new(format!(
            "focus-take-length:{}:{}",
            track, take.0
        ));
        self.song_take_set_length_coalesced(track, take, len_steps, merge_key)
    }

    /// The ACTIVE pinned clip: the timeline selection while it is live
    /// (arrangement view on screen, source alive — the same dormancy rule the
    /// sound binding applies). This deliberately does NOT go through the
    /// resolved focus: a pinned clip whose pattern happens to be the
    /// effective one resolves `Live` for the WRITE path, but it is still a
    /// pinned clip for the clip-shaped surfaces (window overlay, panel
    /// fields, band slide).
    pub(crate) fn active_clip_selection(
        &self,
        track: usize,
    ) -> Option<crate::app::sound_binding::SongClipSelection> {
        self.selected_bound_source(track)?;
        self.song_clip_selection
            .filter(|selection| selection.track == track)
    }

    /// The pinned clip's source kind for the UI (`"pattern"` / `"take"`);
    /// `None` without an active clip selection. Gates the clip-shaped
    /// gestures (band slide) independently of the resolved WRITE focus.
    pub fn focus_clip_source_kind(&self, track: usize) -> Option<&'static str> {
        Some(match self.active_clip_selection(track)?.source {
            crate::app::sound_binding::BoundSource::Pattern(_) => "pattern",
            crate::app::sound_binding::BoundSource::Take(_) => "take",
        })
    }

    /// Band-body slide (spec 5): slide the pinned clip's loop window by
    /// `delta_steps` — today ≡ a phase shift of `offset_steps`, one undoable
    /// arrangement edit. Only meaningful with an explicit clip selection.
    pub fn slide_focused_clip_offset(
        &mut self,
        track: usize,
        delta_steps: f64,
    ) -> Result<(), String> {
        let selection = self
            .active_clip_selection(track)
            .ok_or_else(|| "No pinned clip to slide".to_string())?;
        self.arr_clip_slide_offset(selection.clip_id, delta_steps)
    }

    /// The pinned clip's panel fields (spec 6): `(start_beat, end_beat,
    /// offset_steps)`. `None` without an active clip selection — the panel
    /// hides Start/End/Offset in follow mode. Keyed off the SELECTION, not
    /// the resolved focus: a pinned clip stays a clip even when its pattern
    /// happens to be the effective one.
    pub fn focus_clip_fields(&self, track: usize) -> Option<(f64, f64, f64)> {
        let selection = self.active_clip_selection(track)?;
        let arrangement = self.state.committed_arrangement()?;
        let (_, clip) = arrangement.find_clip(selection.clip_id)?;
        Some((clip.start_beat, clip.end_beat, clip.offset_steps))
    }

    /// Clip-panel Start/End edit (spec 6): lowers to `arr_clip_resize` on the
    /// selected clip, keeping the one-clip region in step.
    pub fn resize_focused_clip(
        &mut self,
        track: usize,
        start_beat: f64,
        end_beat: f64,
    ) -> Result<(), String> {
        let selection = self
            .active_clip_selection(track)
            .ok_or_else(|| "No pinned clip to resize".to_string())?;
        // Number-picker drags fire per pointer frame: coalesce the whole
        // drag into one arrangement undo entry.
        let merge_key = crate::app::history::MergeKey::new(format!(
            "focus-clip-resize:{}",
            selection.clip_id.0
        ));
        self.arr_clip_resize_coalesced(selection.clip_id, start_beat, end_beat, merge_key)?;
        self.refresh_song_region_for_clip(selection.clip_id);
        Ok(())
    }

    /// Clip-panel Start-offset edit (spec 6): absolute source step, signed
    /// entry allowed (a pattern's negative pickup wraps into the top half).
    pub fn set_focused_clip_offset(
        &mut self,
        track: usize,
        offset_steps: f64,
    ) -> Result<(), String> {
        let selection = self
            .active_clip_selection(track)
            .ok_or_else(|| "No pinned clip to offset".to_string())?;
        let merge_key = crate::app::history::MergeKey::new(format!(
            "focus-clip-offset:{}",
            selection.clip_id.0
        ));
        self.arr_clip_set_offset_coalesced(selection.clip_id, offset_steps, merge_key)?;
        // A take offset raise re-clamps the clip's END (the playable tail
        // shrank): keep the one-clip region in step, like a resize does.
        self.refresh_song_region_for_clip(selection.clip_id);
        Ok(())
    }

    /// The loop-window overlay for the focused clip (spec 5):
    /// `(marker, span, repeat)` — the start marker at `offset_steps`, the
    /// played window when the clip span is shorter than one source pass
    /// (`span.1` may exceed the source length: the window wraps), and the
    /// repeat count when it covers several passes. `None` in follow mode or
    /// without an explicit clip selection.
    pub fn focus_window_overlay(
        &self,
        track: usize,
    ) -> Option<(f64, Option<(f64, f64)>, Option<f64>)> {
        use crate::app::sound_binding::BoundSource;
        // Keyed off the SELECTION (the pinned clip), not the resolved focus:
        // a clip whose pattern is the effective one still shows its window.
        let selection = self.active_clip_selection(track)?;
        let arrangement = self.state.committed_arrangement()?;
        let (_, clip) = arrangement.find_clip(selection.clip_id)?;
        let span_beats = clip.end_beat - clip.start_beat;
        self.state.with_project_scenes(|scenes| {
            use crate::sequencer::SongCompileContext;
            let is_pattern = matches!(selection.source, BoundSource::Pattern(_));
            let (steps_per_beat, source_len) = match selection.source {
                BoundSource::Pattern(pattern) => {
                    // Average steps-per-beat over the pattern's REAL cycle
                    // (timebase/sync plocks included): the overlay's span and
                    // repeat count then reflect the audible loop length.
                    let geometry = scenes.song_track_pattern_geometry(track, pattern.0)?;
                    let num_steps = geometry.num_steps() as f64;
                    (num_steps / geometry.cycle_beats(), num_steps)
                }
                BoundSource::Take(take) => {
                    scenes.song_track_take_step_mapping(track, take.0)?
                }
            };
            let span_steps = span_beats * steps_per_beat;
            // Normalize into the source: a loop-bar shrink can leave a clip
            // offset beyond the new length until playback re-stamps it.
            let marker = if is_pattern {
                crate::sequencer::pattern_play_step(clip.offset_steps, 0.0, (0.0, source_len))
            } else {
                clip.offset_steps.clamp(0.0, (source_len - 1.0).max(0.0))
            };
            if span_steps + 1e-6 < source_len {
                Some((marker, Some((marker, marker + span_steps)), None))
            } else if span_steps > source_len + 1e-6 && is_pattern {
                Some((marker, None, Some((span_steps / source_len).round().max(2.0))))
            } else {
                Some((marker, None, None))
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

    fn install_pattern_clip(app: &mut App, scene_pattern: PatternId, span: (f64, f64)) -> PatternId {
        let other = app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0]
                .get(scene_pattern)
                .expect("scene pattern")
                .clone();
            scenes.track_pools[0].insert(data)
        });
        let mut arrangement = crate::sequencer::ProjectArrangement::new(1, 64.0);
        arrangement.track_lanes[0].push(crate::sequencer::ArrClip::new(
            ClipId(0),
            span.0,
            span.1,
            Some(other.0),
        ));
        arrangement.next_clip_id = 1;
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("arrangement installs");
        app.select_song_clip(0, ClipId(0)).expect("clip selects");
        other
    }

    /// Spec 5 (locked decision 3): the loop bar in pinned focus resizes the
    /// SHARED pool pattern — the live track params never move — and a whole
    /// drag coalesces into one undo entry.
    #[test]
    fn pinned_loop_bar_resizes_the_pool_pattern_as_one_entry() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let other = install_pattern_clip(&mut app, scene_pattern, (0.0, 16.0));
        assert!(matches!(
            app.track_edit_focus(0),
            EditFocus::Pattern { .. }
        ));
        let live_before = app.state.pattern.track_params[0].get_num_steps();
        let depth = app.history.undo_len();

        // Two drag frames, one gesture.
        assert!(app.set_pinned_pattern_num_steps(0, 24).expect("resize applies"));
        assert!(app.set_pinned_pattern_num_steps(0, 32).expect("resize applies"));
        app.finish_focused_pattern_num_steps(0);

        let state = app.state.clone();
        let pool_steps = move |pattern| {
            state
                .with_pool_pattern(0, pattern, |data| data.track_params.num_steps)
                .expect("pattern in pool")
        };
        assert_eq!(pool_steps(other), 32);
        assert_eq!(
            app.state.pattern.track_params[0].get_num_steps(),
            live_before,
            "the live track params are never touched by a pinned resize"
        );
        assert_eq!(app.history.undo_len(), depth + 1, "one undo entry per drag");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(pool_steps(other), 16, "undo restores the pool length");

        // A drag that returns to its starting length leaves NO undo entry
        // (a staged no-op patch would eat an undo step and clear redo).
        let depth = app.history.undo_len();
        assert!(app.set_pinned_pattern_num_steps(0, 24).expect("frame applies"));
        assert!(app.set_pinned_pattern_num_steps(0, 16).expect("frame applies"));
        app.finish_focused_pattern_num_steps(0);
        assert_eq!(pool_steps(other), 16);
        assert_eq!(app.history.undo_len(), depth, "round-trip drag is a no-op");

        // Follow mode keeps the band on the live track path.
        app.set_song_clip_selection(None);
        assert!(app.set_pinned_pattern_num_steps(0, 24).is_err());
    }

    /// Spec 5: band-body slide = phase. The offset wraps through
    /// `pattern_play_step`, is one undoable arrangement edit, and take clips
    /// refuse it (they play linearly).
    #[test]
    fn band_slide_edits_the_pinned_clips_offset() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 16.0));
        let offset = |app: &App| {
            app.state
                .committed_arrangement()
                .and_then(|arrangement| {
                    arrangement
                        .find_clip(ClipId(0))
                        .map(|(_, clip)| clip.offset_steps)
                })
                .expect("clip exists")
        };
        assert_eq!(offset(&app), 0.0);
        let depth = app.history.undo_len();
        app.slide_focused_clip_offset(0, 20.0).expect("slide applies");
        // Scene pattern is 16 steps: 20 wraps to 4.
        assert_eq!(offset(&app), 4.0);
        assert_eq!(app.history.undo_len(), depth + 1);
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(offset(&app), 0.0, "undo restores the phase");

        // The take clip refuses the slide outright.
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("take selects");
        assert!(app.slide_focused_clip_offset(0, 4.0).is_err());
    }

    /// Spec 5: the clip-window overlay — marker at the offset, the played
    /// window when the span is under one pass, a repeat count past it.
    #[test]
    fn focus_window_overlay_reports_marker_window_and_repeats() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        // 16-step pattern at 4 steps per beat: a 2-beat clip = 8 steps.
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 2.0));
        app.slide_focused_clip_offset(0, 12.0).expect("slide applies");
        let (marker, span, repeat) =
            app.focus_window_overlay(0).expect("overlay for pinned clip");
        assert_eq!(marker, 12.0);
        assert_eq!(span, Some((12.0, 20.0)), "the window wraps past the end");
        assert_eq!(repeat, None);

        // A 12-beat clip = 48 steps = 3 passes of the 16-step source.
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 12.0));
        let (_, span, repeat) = app.focus_window_overlay(0).expect("overlay");
        assert_eq!(span, None);
        assert_eq!(repeat, Some(3.0));

        // Follow mode has no overlay.
        app.set_song_clip_selection(None);
        assert!(app.focus_window_overlay(0).is_none());
    }

    /// Spec 6 clip panel: Start/End lower to arr_clip_resize on the pinned
    /// clip; the fields read back the stored clip; follow mode hides them.
    #[test]
    fn clip_panel_fields_read_and_resize_the_pinned_clip() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 8.0));
        assert_eq!(app.focus_clip_fields(0), Some((0.0, 8.0, 0.0)));

        app.resize_focused_clip(0, 2.0, 6.0).expect("resize applies");
        let (start, end, offset) = app.focus_clip_fields(0).expect("fields");
        assert_eq!((start, end), (2.0, 6.0));
        // Left trim re-stamps the phase (spec 8 of the region spec): 2 beats
        // at 4 steps per beat = 8 steps into the 16-step pattern.
        assert_eq!(offset, 8.0);

        app.set_song_clip_selection(None);
        assert_eq!(app.focus_clip_fields(0), None, "follow mode hides fields");
    }

    /// Review regression: the panel's number-picker fires per pointer frame,
    /// so consecutive Start/End (and Offset) edits must coalesce into ONE
    /// arrangement undo entry, and a round-trip discards it.
    #[test]
    fn clip_panel_picker_frames_coalesce_into_one_undo_entry() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 8.0));
        let depth = app.history.undo_len();
        app.resize_focused_clip(0, 0.0, 7.0).expect("frame applies");
        app.resize_focused_clip(0, 0.0, 6.0).expect("frame applies");
        crate::app::edit::finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), depth + 1, "one entry per drag");
        assert_eq!(app.focus_clip_fields(0).map(|(_, end, _)| end), Some(6.0));

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.focus_clip_fields(0).map(|(_, end, _)| end),
            Some(8.0),
            "undo restores the pre-drag span in one step"
        );

        // Round trip: back to the starting span leaves no entry behind.
        let depth = app.history.undo_len();
        app.resize_focused_clip(0, 0.0, 7.0).expect("frame applies");
        app.resize_focused_clip(0, 0.0, 8.0).expect("frame applies");
        crate::app::edit::finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), depth, "round-trip drag is a no-op");
    }

    /// Spec 6 signed start offset: setting an absolute (possibly negative)
    /// source step wraps for patterns — the Ableton pickup entry — and
    /// clamps at 0 for takes.
    #[test]
    fn clip_panel_offset_wraps_patterns_and_clamps_takes() {
        let (mut app, _take, scene_pattern, _chunks) = app_with_take();
        let _other = install_pattern_clip(&mut app, scene_pattern, (0.0, 8.0));
        // Pickup entry: −1 on a 16-step pattern lands on step 15.
        app.set_focused_clip_offset(0, -1.0).expect("offset applies");
        assert_eq!(app.focus_clip_fields(0).unwrap().2, 15.0);
        app.set_focused_clip_offset(0, 20.0).expect("offset applies");
        assert_eq!(app.focus_clip_fields(0).unwrap().2, 4.0);

        // Take: clamps at 0, never wraps.
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, ClipId(0)).expect("take selects");
        app.set_focused_clip_offset(0, -5.0).expect("offset applies");
        assert_eq!(app.focus_clip_fields(0).unwrap().2, 0.0);
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
