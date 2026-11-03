//! Arrangement replay of recorded scene pushes (epic eseq-vum0).
//!
//! The push lane's regions are played back at control rate through the same
//! `apply_scene_push_*` entry points the live transport gesture uses, so a
//! replayed push morphs exactly what the performed one did. The driver runs
//! once per control frame — the UI event loop live, every render block in an
//! offline bounce — after the frame's song rows were mirrored, so a push that
//! starts on a scene change morphs from the new scene. A live gesture always
//! wins: while one is held the driver leaves the push alone.

use crate::sequencer::ClipId;

use super::App;

/// Who drives the macro engine's single scene push slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ScenePushOwner {
    #[default]
    Idle,
    /// The performer's transport gesture.
    Live,
    /// Arrangement replay of push region `region`, last sent `value`.
    Replay { region: ClipId, value: f32 },
}

impl ScenePushOwner {
    pub fn is_live(self) -> bool {
        self == Self::Live
    }
}

impl App {
    /// Drive the scene push from the committed push lane at song beat
    /// `beat` (`None`: no arrangement playback, so any replayed push ends).
    pub fn drive_arrangement_scene_push(&mut self, beat: Option<f64>) {
        if self.scene_push_owner.is_live() {
            return;
        }
        let wanted = beat.and_then(|beat| {
            self.state.with_committed_arrangement(|arrangement| {
                let region = arrangement?.scene_push_at(beat)?;
                Some((region.id, region.target_scene, region.value_at(beat)))
            })
        });
        match (self.scene_push_owner, wanted) {
            (ScenePushOwner::Replay { region, value }, Some((id, _, wanted_value))) if region == id => {
                if wanted_value != value {
                    self.apply_scene_push_value(wanted_value);
                    self.scene_push_owner = ScenePushOwner::Replay { region, value: wanted_value };
                }
            }
            // Beginning a push ends the one before it in the engine.
            (_, Some((id, target_scene, value))) => {
                self.apply_scene_push_begin(target_scene, value);
                self.scene_push_owner = ScenePushOwner::Replay { region: id, value };
            }
            (ScenePushOwner::Replay { .. }, None) => {
                self.apply_scene_push_end();
                self.scene_push_owner = ScenePushOwner::Idle;
            }
            (_, None) => {}
        }
    }

    /// [`Self::drive_arrangement_scene_push`] at rendered sample
    /// `rendered_sample` of the song clock, then the schedule of the pushes
    /// the scheduler is about to stamp note-ons through
    /// ([`Self::schedule_arrangement_scene_pushes`]).
    pub fn drive_arrangement_scene_push_at_sample(&mut self, rendered_sample: u64) {
        let beat = self.state.song_playback().shared().position_beats(rendered_sample);
        self.drive_arrangement_scene_push(beat);
        self.schedule_arrangement_scene_pushes(rendered_sample);
    }
}

/// How far past the scheduler's lookahead the push schedule reaches: the
/// frames between two passes of the UI loop.
const SCHEDULE_SLACK_LOOKAHEADS: f64 = 0.5;

impl App {
    /// Hand the scheduler the recorded pushes it is about to schedule
    /// through: every push region overlapping song time from rendered
    /// sample `rendered_sample` to the scheduler's lookahead past it (plus
    /// slack). Republished only when that set, the song or the playing scene
    /// (what a push morphs from) changes; cleared while a live gesture owns
    /// the push or the song is stopped.
    pub fn schedule_arrangement_scene_pushes(&mut self, rendered_sample: u64) {
        let lookahead = self.state.scheduler_lookahead_samples();
        let frontier_sample =
            rendered_sample + (lookahead as f64 * (1.0 + SCHEDULE_SLACK_LOOKAHEADS)) as u64;
        let shared = self.state.song_playback().shared();
        let window = shared
            .position_beats(rendered_sample)
            .zip(shared.position_beats(frontier_sample));
        let regions: Vec<crate::sequencer::ScenePushRegion> = match window {
            Some((now, frontier)) if !self.scene_push_owner.is_live() => {
                self.state.with_committed_arrangement(|arrangement| {
                    let Some(arrangement) = arrangement else {
                        return Vec::new();
                    };
                    // A loop wrap inside the window: it runs to the end, then
                    // on from zero.
                    let (first, wrapped) = if frontier >= now {
                        ((now, frontier), None)
                    } else {
                        ((now, arrangement.end_beat), Some((0.0, frontier)))
                    };
                    (arrangement.push_lane.iter())
                        .filter(|region| {
                            std::iter::once(first)
                                .chain(wrapped)
                                .any(|(a, b)| region.start_beat <= b && region.end_beat > a)
                        })
                        .cloned()
                        .collect()
                })
            }
            _ => Vec::new(),
        };
        let key = ScheduledScenePushesKey {
            revision: self.state.committed_song_revision(),
            scene: self.state.current_scene_index(),
            ids: regions.iter().map(|region| region.id).collect(),
        };
        if self.scheduled_scene_pushes == key {
            return;
        }
        let regions: Vec<_> = regions
            .into_iter()
            .filter_map(|region| {
                let mappings = self.scene_push_mappings(region.target_scene)?;
                let params = self.macro_engine.scene_push_params(&mappings);
                Some(crate::sequencer::ScheduledScenePush { region, params })
            })
            .collect();
        let schedule = (!regions.is_empty())
            .then(|| std::sync::Arc::new(crate::sequencer::ScenePushSchedule { regions }));
        self.state.publish_scene_push_schedule(schedule);
        self.scheduled_scene_pushes = key;
    }
}

/// What the published push schedule was built from: the push params' `from`
/// values are the playing scene's, so a scene change rebuilds it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ScheduledScenePushesKey {
    revision: u64,
    scene: usize,
    ids: Vec<ClipId>,
}

impl App {
    /// Delete push regions `ids` (one undo step).
    pub fn scene_pushes_delete(&mut self, ids: &[ClipId]) -> Result<(), String> {
        let ids = ids.to_vec();
        self.edit_arrangement("Delete scene push", move |arrangement, _scenes| {
            for id in &ids {
                arrangement
                    .take_scene_push(*id)
                    .ok_or_else(|| format!("Scene push {} does not exist", id.0))?;
            }
            Ok(())
        })
    }

    /// Move push region `id` to start at `start_beat`, curve and length
    /// kept; what it lands on is trimmed, as a clip move truncates.
    pub fn scene_push_move(&mut self, id: ClipId, start_beat: f64) -> Result<(), String> {
        if !start_beat.is_finite() || start_beat < 0.0 {
            return Err(format!("Scene push start {start_beat} must be a non-negative beat"));
        }
        self.replace_scene_push("Move scene push", id, move |region| region.moved_to(start_beat))
    }

    /// Clear `[start, end)` of the push lane (a range swept on the lane):
    /// pushes inside go, ones it cuts keep the parts outside.
    pub fn scene_pushes_clear_span(&mut self, start: f64, end: f64) -> Result<(), String> {
        let (start, end) = (start.min(end).max(0.0), start.max(end));
        if !(start.is_finite() && end.is_finite()) || end <= start {
            return Err(format!("Scene push range {start}..{end} is empty"));
        }
        self.edit_arrangement("Clear scene pushes", move |arrangement, _scenes| {
            arrangement.occlude_scene_push_span(start, end)
        })
    }

    /// Resize push region `id` to `[start_beat, end_beat)`. Cutting an edge
    /// in trims the curve there; pulling one out holds the curve's value at
    /// that edge. What the new span covers is trimmed, as a clip resize
    /// truncates its neighbour.
    pub fn scene_push_resize(&mut self, id: ClipId, start_beat: f64, end_beat: f64) -> Result<(), String> {
        if !(start_beat.is_finite() && end_beat.is_finite()) || start_beat < 0.0 || end_beat <= start_beat {
            return Err(format!("Scene push span {start_beat}..{end_beat} is not a valid span"));
        }
        self.replace_scene_push("Resize scene push", id, move |region| {
            region.resized(start_beat, end_beat)
        })
    }

    /// Replace push region `id` with `edit` of it (one undo step): what the
    /// new span lands on is trimmed, as a clip edit truncates its
    /// neighbour, and the song grows to hold it.
    fn replace_scene_push(
        &mut self,
        label: &'static str,
        id: ClipId,
        edit: impl FnOnce(&crate::sequencer::ScenePushRegion) -> crate::sequencer::ScenePushRegion,
    ) -> Result<(), String> {
        self.edit_arrangement(label, move |arrangement, _scenes| {
            let region = arrangement
                .take_scene_push(id)
                .ok_or_else(|| format!("Scene push {} does not exist", id.0))?;
            let edited = edit(&region);
            arrangement.occlude_scene_push_span(edited.start_beat, edited.end_beat)?;
            arrangement.end_beat = arrangement.end_beat.max(edited.end_beat);
            arrangement.insert_scene_push_sorted(edited);
            Ok(())
        })
    }
}
