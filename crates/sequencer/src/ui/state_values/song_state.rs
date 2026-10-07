//! Song-mode reactive bindings (docs/song-mode-spec.md section 12): builds
//! and diff-publishes the `SEQ.song-*` values each
//! frame from `App` transport state plus the committed song. The arrangement
//! reads the host kinds (`song`, `clip`, `scene-span`, kind-bindings spec
//! §14.2d); the helpers here that build their content are shared with them.

use super::*;

use sequencer::app::song_transport::SongTransportMode;
use sequencer::sequencer::{
    state_at_beat, ArrClip, ProjectScenes, ProjectSong, ProjectSongRow, StepParam,
};

/// Scalar song bindings published to `SEQ.*`, snapshotted per frame so each
/// reactive is only rewritten when its value changed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SongBindingsSnapshot {
    pub(crate) exists: bool,
    /// "" while not recording, else "take" / "dub"
    /// (docs/unified-transport-spec.md 8).
    pub(crate) recording_kind: &'static str,
    /// Current row ordinal during song playback, else -1.
    pub(crate) current_row: f64,
    /// Current row stable id during song playback, else -1.
    pub(crate) current_row_id: f64,
    pub(crate) row_count: f64,
    pub(crate) loop_enabled: bool,
    /// Latched failure state of the most recent arrangement capture
    /// (docs/song-mode-spec.md 12); cleared when the next capture starts.
    pub(crate) capture_failed: bool,
    pub(crate) capture_error: Option<String>,
}

/// Per-frame diff state for the song bindings: the committed song is cached
/// and re-read only when `committed_song_revision` changes
/// (`set_committed_arrangement` bumps it).
#[derive(Default)]
pub(crate) struct SongFrameState {
    pub(crate) revision: Option<u64>,
    pub(crate) cached_song: Option<ProjectSong>,
    pub(crate) prev: Option<SongBindingsSnapshot>,
}

/// Display grain of the record head (spec 3.3): the head advances every
/// frame, so it is floored to this grid before it can force a publish. A
/// 16th note at 4/4 — the finest step timebase any lane records on, so a
/// provisional clip still grows one step at a time.
const PENDING_HEAD_QUANTUM: f64 = 0.25;

/// One pending take lane's flattened content (`song.pending-lanes`).
#[derive(Clone, PartialEq)]
pub(crate) struct PendingLaneContent {
    pub(crate) track: usize,
    pub(crate) punch_in_beat: f64,
    step_beats: f64,
    /// Where the committed clip would END if capture stopped right now:
    /// `P + ceil(max_end_steps) * step_beats`, the stop-commit's own punch-out
    /// formula (`register_pending_takes`). The drawn span never falls short of
    /// this, so the feedback always contains the music it recorded — and after
    /// a Stop taken at the last note's end the two spans are identical
    /// (spec 6 item 1, round trip).
    content_end_beat: f64,
    pub(crate) num_steps: usize,
    pub(crate) length_beats: f64,
    pub(crate) events: Vec<(f64, f64, f64, f64)>,
}

impl PendingLaneContent {
    /// What the drawn span's end needs (no events).
    pub(crate) fn span(&self) -> PendingLaneSpan {
        PendingLaneSpan {
            punch_in_beat: self.punch_in_beat,
            step_beats: self.step_beats,
            content_end_beat: self.content_end_beat,
        }
    }
}

/// The part of a [`PendingLaneContent`] its span's end follows the head
/// with: cheap to keep per lane between content rebuilds.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PendingLaneSpan {
    punch_in_beat: f64,
    step_beats: f64,
    content_end_beat: f64,
}

impl PendingLaneSpan {
    /// The drawn span's end with the record head at `head_beat`: the
    /// growing edge, floored to the lane's own step so the span advances a
    /// step at a time, never shorter than the music it already holds.
    pub(crate) fn end_beat(self, head_beat: f64) -> f64 {
        let grown = if self.step_beats > 0.0 {
            let steps = ((head_beat - self.punch_in_beat) / self.step_beats).floor();
            self.punch_in_beat + steps.max(0.0) * self.step_beats
        } else {
            self.punch_in_beat
        };
        grown.max(self.content_end_beat)
    }
}

/// One captured launch's effect on one track lane: the pattern it put there,
/// flattened for drawing. The clip's END is not here — it runs to the next
/// launch on the lane, or to the record head, which moves without the
/// content changing.
#[derive(Clone, PartialEq)]
pub(crate) struct PendingTrackEventContent {
    pub(crate) track: usize,
    pub(crate) start_beat: f64,
    pub(crate) pattern_id: u64,
    pub(crate) num_steps: usize,
    pub(crate) length_beats: f64,
    pub(crate) events: Vec<(f64, f64, f64, f64)>,
}

/// The provisional surface's content, keyed by `App::pending_revision`.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct PendingContent {
    pub(crate) origin_beat: f64,
    pub(crate) lanes: Vec<PendingLaneContent>,
    /// (start beat, scene position) per captured scene launch.
    pub(crate) scene_events: Vec<(f64, usize)>,
    pub(crate) track_events: Vec<PendingTrackEventContent>,
}

/// The record head while a capture take exists, floored to
/// `PENDING_HEAD_QUANTUM` (0 before the record clock has an anchor); `None`
/// with no capture take: the host kinds' `song.pending-head`.
pub(crate) fn quantized_pending_head(app: &app::App) -> Option<f64> {
    let head = app
        .pending_capture_active()
        .then(|| app.pending_capture_head_beat().unwrap_or(0.0))?;
    Some((head / PENDING_HEAD_QUANTUM).floor().max(0.0) * PENDING_HEAD_QUANTUM)
}

/// What the provisional content is built from, beyond the capture take
/// itself (`App::pending_revision`, which also moves when a take begins or
/// ends): the pool content and the project scenes the captured launches
/// name (a launched pattern's steps / length, a scene's cell assignment).
/// The host kinds rebuild it only when this moves.
pub(crate) fn pending_content_key(app: &app::App) -> (u64, u64, u64) {
    (
        app.pending_revision,
        app.state.pool_content_revision(),
        app.state.project_scenes_revision(),
    )
}

/// The capture take's provisional content ([`build_pending_content`]);
/// `None` with no capture take. Built only when [`pending_content_key`]
/// moved.
pub(crate) fn pending_capture_content(app: &app::App) -> Option<PendingContent> {
    app.with_pending_capture(|pending| build_pending_content(app, pending))
}

/// Flatten the borrowed capture state into owned, publishable content. Runs
/// only on a frame where [`pending_content_key`] moved.
fn build_pending_content(
    app: &app::App,
    pending: sequencer::app::pending_capture::PendingCapture<'_>,
) -> PendingContent {
    let lanes = pending
        .lanes
        .iter()
        .map(|lane| {
            // The stop-commit's `total_len_steps` (takes spec 8.5): the drawn
            // content and the committed take cover the same steps.
            let total_len = lane.max_end_steps.ceil().max(1.0) as usize;
            let mut events = Vec::new();
            for (chunk_idx, chunk) in lane.chunks.iter().enumerate() {
                let base = chunk_idx * sequencer::sequencer::MAX_STEPS;
                let limit = total_len
                    .saturating_sub(base)
                    .min(sequencer::sequencer::MAX_STEPS);
                if limit == 0 || events.len() >= LANE_PATTERN_EVENT_CAP {
                    break;
                }
                flatten_pattern_events(chunk, base as f64, limit, &mut events);
            }
            events.truncate(LANE_PATTERN_EVENT_CAP);
            PendingLaneContent {
                track: lane.track,
                punch_in_beat: lane.punch_in_beat,
                step_beats: lane.step_beats,
                content_end_beat: lane.punch_in_beat + total_len as f64 * lane.step_beats,
                num_steps: total_len,
                length_beats: total_len as f64 * lane.step_beats,
                events,
            }
        })
        .collect();
    // The clips a captured launch put on the track lanes, flattened from the
    // pool patterns they name.
    let track_events = app.state.with_project_scenes(|scenes| {
        pending
            .track_events
            .iter()
            .filter_map(|(start_beat, track, pattern)| {
                let data = scenes.track_pools.get(*track)?.get(*pattern)?;
                let num_steps = data.track_params.num_steps.max(1);
                let mut events = Vec::new();
                flatten_pattern_events(&data, 0.0, num_steps, &mut events);
                events.truncate(LANE_PATTERN_EVENT_CAP);
                Some(PendingTrackEventContent {
                    track: *track,
                    start_beat: *start_beat,
                    pattern_id: pattern.0,
                    num_steps,
                    length_beats: data.track_params.timebase.step_beats(num_steps)
                        * num_steps as f64,
                    events,
                })
            })
            .collect()
    });
    PendingContent {
        origin_beat: pending.origin_beat,
        lanes,
        scene_events: pending.scene_events,
        track_events,
    }
}

/// Flattened preview events for one pool pattern referenced by a track's
/// lane clips (docs/arrangement-timeline-ui-spec.md 7.1): raw musical events
/// `(time-in-steps, transpose, velocity, duration-in-steps)` plus the pattern
/// length. The Lisp view owns turning these into normalized dot payloads; the
/// widget never sees steps or timebases.
#[derive(Clone, PartialEq)]
pub(crate) struct LanePatternEvents {
    pub(crate) pattern_id: u64,
    /// `Some` when this entry is a TAKE's aggregated content (takes spec
    /// 11.3): `pattern_id` is then meaningless (0), `num_steps` is the
    /// take's total playable length, and event times run continuously
    /// across chunk boundaries.
    pub(crate) take_id: Option<u64>,
    pub(crate) num_steps: usize,
    /// One pattern cycle in musical beats (`num_steps * step_beats` of the
    /// pattern's timebase) — what the view needs to tile a looping clip.
    /// For a take entry: the take's full length in beats (takes never tile).
    pub(crate) length_beats: f64,
    pub(crate) events: Vec<(f64, f64, f64, f64)>,
}

/// Smallest published note length in steps, mirroring the piano roll's floor
/// (`ui/piano_roll.rs`) so a zero-length step still reads as a note.
const LANE_MIN_NOTE_DURATION: f64 = 0.03125;

/// Flatten one pattern's step/chord content into `(time, transpose,
/// velocity, duration)` events, with times based at `base_step` and truncated
/// at `step_limit` steps of the pattern. Durations are in the same step units
/// as `time` (docs/arrangement-region-editing-spec.md 3.2).
fn flatten_pattern_events(
    data: &sequencer::sequencer::TrackPatternData,
    base_step: f64,
    step_limit: usize,
    events: &mut Vec<(f64, f64, f64, f64)>,
) {
    for step in 0..step_limit.min(data.step_data.len()) {
        if events.len() >= LANE_PATTERN_EVENT_CAP {
            break;
        }
        let velocity = f64::from(data.step_data[step][StepParam::Velocity as usize]);
        let step_duration = f64::from(data.step_data[step][StepParam::Duration as usize])
            .max(LANE_MIN_NOTE_DURATION);
        let chord = data.chord_snapshot.steps.get(step);
        match chord {
            Some(notes) if !notes.is_empty() => {
                for (voice, transpose) in notes.iter().enumerate() {
                    let delay = data
                        .chord_snapshot
                        .delays
                        .get(step)
                        .and_then(|delays| delays.get(voice))
                        .copied()
                        .unwrap_or(0.0);
                    // A voice with no recorded duration inherits the step's
                    // (piano-roll precedent, `piano_roll.rs`).
                    let duration = data
                        .chord_snapshot
                        .durations
                        .get(step)
                        .and_then(|durations| durations.get(voice))
                        .map(|duration| f64::from(*duration))
                        .filter(|duration| *duration > 0.0)
                        .unwrap_or(step_duration)
                        .max(LANE_MIN_NOTE_DURATION);
                    events.push((
                        base_step + step as f64 + f64::from(delay),
                        f64::from(*transpose),
                        velocity,
                        duration,
                    ));
                }
            }
            _ => {
                let active = (data.track_bits[step / 64] >> (step % 64)) & 1 == 1;
                if active {
                    let delay = f64::from(data.step_data[step][StepParam::Delay as usize]);
                    let transpose =
                        f64::from(data.step_data[step][StepParam::Transpose as usize]);
                    events.push((
                        base_step + step as f64 + delay,
                        transpose,
                        velocity,
                        step_duration,
                    ));
                }
            }
        }
    }
}

/// Bound on published events per pattern so a pathological pattern cannot
/// bloat the reactive value; the view additionally caps dots per item.
const LANE_PATTERN_EVENT_CAP: usize = 1024;

/// Collect the distinct pool patterns each track's lane clips resolve to and
/// flatten their step/chord snapshots into preview events.
pub(crate) fn arrangement_pattern_preview(
    scenes: &ProjectScenes,
    track: usize,
    id: PatternId,
) -> Option<LanePatternEvents> {
    let data = scenes.track_pools.get(track)?.get(id)?;
    let num_steps = data.track_params.num_steps.max(1);
    let mut events = Vec::new();
    flatten_pattern_events(&data, 0.0, num_steps, &mut events);
    events.truncate(LANE_PATTERN_EVENT_CAP);
    Some(LanePatternEvents {
        pattern_id: id.0,
        take_id: None,
        num_steps,
        length_beats: data.track_params.timebase.step_beats(num_steps) * num_steps as f64,
        events,
    })
}

pub(crate) fn collect_lane_pattern_events(
    lanes: &[Vec<ArrClip>],
    scenes: &ProjectScenes,
) -> Vec<Vec<LanePatternEvents>> {
    lanes
        .iter()
        .enumerate()
        .map(|(track, clips)| {
            let mut ids: Vec<u64> = clips.iter().filter_map(|clip| clip.pattern_id).collect();
            ids.sort_unstable();
            ids.dedup();
            let mut entries: Vec<LanePatternEvents> = ids
                .into_iter()
                .filter_map(|id| {
                    arrangement_pattern_preview(scenes, track, PatternId(id))
                })
                .collect();
            // Take entries (takes spec 11.3): one aggregated entry per take
            // the lane references, MIDI-dot content concatenated across
            // chunks on a continuous step axis.
            let mut take_ids: Vec<u64> = clips
                .iter()
                .filter_map(|clip| clip.take_id)
                .collect();
            take_ids.sort_unstable();
            take_ids.dedup();
            for take_id in take_ids {
                let Some(take) = scenes
                    .take_pools
                    .get(track)
                    .and_then(|takes| takes.get(sequencer::sequencer::TakeId(take_id)))
                else {
                    continue;
                };
                let Some(first_chunk) = take
                    .chunks
                    .first()
                    .and_then(|id| scenes.track_pools.get(track)?.get(*id))
                else {
                    continue;
                };
                let step_beats = first_chunk
                    .track_params
                    .timebase
                    .step_beats(sequencer::sequencer::MAX_STEPS);
                let total_len = take.total_len_steps.max(1) as usize;
                let mut events = Vec::new();
                for (chunk_idx, chunk_id) in take.chunks.iter().enumerate() {
                    let Some(data) =
                        scenes.track_pools.get(track).and_then(|pool| pool.get(*chunk_id))
                    else {
                        continue;
                    };
                    let base = chunk_idx * sequencer::sequencer::MAX_STEPS;
                    let limit = total_len
                        .saturating_sub(base)
                        .min(sequencer::sequencer::MAX_STEPS);
                    if limit == 0 || events.len() >= LANE_PATTERN_EVENT_CAP {
                        break;
                    }
                    flatten_pattern_events(&data, base as f64, limit, &mut events);
                }
                events.truncate(LANE_PATTERN_EVENT_CAP);
                entries.push(LanePatternEvents {
                    pattern_id: 0,
                    take_id: Some(take_id),
                    num_steps: total_len,
                    length_beats: step_beats * total_len as f64,
                    events,
                });
            }
            entries
        })
        .collect()
}

/// The `seq-arrangement-pattern` preview value (pattern placement):
/// `{pattern-id, take-id, num-steps, length-beats, events}`.
pub(crate) fn build_pattern_preview_value(pattern: &LanePatternEvents) -> Value {
    map_value([
        ("pattern-id", if pattern.take_id.is_some() { Value::Nil }
            else { Value::Number(pattern.pattern_id as f64) }),
        ("take-id", pattern.take_id.map(|id| Value::Number(id as f64)).unwrap_or(Value::Nil)),
        ("num-steps", Value::Number(pattern.num_steps as f64)),
        ("length-beats", Value::Number(pattern.length_beats)),
        ("events", pattern_events_value(&pattern.events)),
    ])
}

/// A pattern's or take's flattened events as `((time transpose velocity
/// duration) …)` (`clip.events`, the pending rows' `events`).
pub(crate) fn pattern_events_value(events: &[(f64, f64, f64, f64)]) -> Value {
    list_value(events.iter().map(|(time, pitch, velocity, duration)| {
        list_value(
            [*time, *pitch, *velocity, *duration]
                .into_iter()
                .map(Value::Number),
        )
    }))
}

/// The row governing `beats` for display purposes: `state_at_beat` semantics
/// (loop-normalized), with the last row covering the transient `end_beat`
/// readout of a non-looping song.
fn display_row_at_beat(song: &ProjectSong, beats: f64) -> Option<&ProjectSongRow> {
    state_at_beat(song, beats).or_else(|| {
        (beats >= song.end_beat).then(|| song.rows.last()).flatten()
    })
}

/// Build the scalar binding snapshot from app + committed song. The current
/// row is derived exactly from the committed song at the rendered position
/// (`state_at_beat`), not from the scheduler's shared atomics, which run up
/// to a lookahead window early.
pub(crate) fn build_song_bindings_snapshot(
    app: &app::App,
    song: Option<&ProjectSong>,
) -> SongBindingsSnapshot {
    // Capturing over an EMPTY song runs the plain session transport, so the
    // song-playback position atomics are inactive and every arrangement lane
    // drew its playhead pinned at beat 0. The capture's own record head is
    // the same clock the launches and take notes are stamped on, so it is
    // the honest fallback; capture ON TOP of song playback keeps the
    // scheduler position, which `pending_capture_head_beat` clamps to anyway.
    let position = song_position(&app.state, app.pending_capture_head_beat());
    let song_playing = app.song_transport_mode == SongTransportMode::SongPlayback;
    let (current_row, current_row_id) = match (song, position) {
        (Some(song), Some(beats)) if song_playing => match display_row_at_beat(song, beats) {
            Some(row) => {
                let ordinal = song
                    .rows
                    .iter()
                    .position(|candidate| candidate.id == row.id)
                    .unwrap_or(0);
                (ordinal as f64, row.id.0 as f64)
            }
            None => (-1.0, -1.0),
        },
        _ => (-1.0, -1.0),
    };
    SongBindingsSnapshot {
        exists: song.is_some(),
        recording_kind: song_recording_kind_label(app.recording_kind),
        current_row,
        current_row_id,
        row_count: song.map(|song| song.rows.len()).unwrap_or(0) as f64,
        loop_enabled: song.map(|song| song.loop_enabled).unwrap_or(false),
        capture_failed: app.song_capture_failed,
        capture_error: app.song_capture_error.clone(),
    }
}

/// The song position (`song.position`): the song
/// playback position, else `capture_head` (capturing over an EMPTY song runs
/// the plain session transport, so the playback position is inactive and the
/// capture's record head is the honest clock; see
/// `App::pending_capture_head_beat`).
pub(crate) fn song_position(state: &SequencerState, capture_head: Option<f64>) -> Option<f64> {
    state.song_position_beats().or(capture_head)
}

/// [`song_position`] as shown: 0 while inactive, quantized to a milli-beat
/// (still render-rate smooth, but sub-display jitter forces no reactive
/// cycle every frame).
pub(crate) fn displayed_song_position_beats(position: Option<f64>) -> f64 {
    (position.unwrap_or(0.0) * 1000.0).round() / 1000.0
}

/// "" while not recording, else "take" (arrangement capture) or "dub"
/// (docs/unified-transport-spec.md 8): `SEQ.song-recording-kind`,
/// `song.recording-kind`.
pub(crate) fn song_recording_kind_label(
    kind: Option<sequencer::app::song_transport::RecordingKind>,
) -> &'static str {
    match kind {
        Some(sequencer::app::song_transport::RecordingKind::Capture) => "take",
        Some(sequencer::app::song_transport::RecordingKind::Overdub) => "dub",
        None => "",
    }
}

/// Some lane, or the scene, is manually latched away from the song (takes
/// spec 10): `song.manual-latch`.
pub(crate) fn song_manual_latch(state: &SequencerState) -> bool {
    state.song_manual_latch_mask() != 0 || state.song_scene_latch()
}

/// Whether `track`'s bit is set in the manual-latch `mask`
/// (`track.latched`).
pub(crate) fn song_lane_latched(mask: u64, track: usize) -> bool {
    track < 64 && mask >> track & 1 == 1
}

/// Per-track take-lane state for the Seq grid (takes spec 10/11.2 UX):
/// 0 = not a take lane, 1 = take-governed, 2 = take lane manually latched.
/// A lane counts as a take lane when the CURRENTLY MIRRORED song row
/// resolves it to a take-claimed chunk pattern — ordinary pattern lanes are
/// never dimmed or blocked, even mid-song-playback.
pub(crate) fn song_take_lane_states(app: &app::App) -> Vec<u8> {
    let mut states = vec![0u8; app.tracks.len()];
    if !app.song_playback_authority_active() {
        return states;
    }
    let Some(song) = app.active_runtime_song.as_ref() else {
        return states;
    };
    let Some(row) = app
        .song_mirrored_row
        .and_then(|ordinal| song.rows.get(ordinal))
    else {
        return states;
    };
    let latch = app.state.song_manual_latch_mask();
    app.state.with_project_scenes(|scenes| {
        for (track, id) in &row.overrides {
            let Some(id) = *id else { continue };
            let Some(state) = states.get_mut(*track) else {
                continue;
            };
            let claimed = scenes
                .take_pools
                .get(*track)
                .is_some_and(|takes| takes.is_claimed(id));
            if claimed {
                *state = if song_lane_latched(latch, *track) {
                    2
                } else {
                    1
                };
            }
        }
    });
    states
}

pub(crate) fn scene_bank_auto_label(mut index: usize) -> String {
    let mut reversed = Vec::new();
    loop {
        reversed.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    reversed.reverse();
    String::from_utf8(reversed).expect("scene bank labels contain only ASCII letters")
}

/// A scene bank's display label: its auto label (`A`, `B`, …), with its
/// name after a dash when it has one: the `bank` host kind's `label`.
pub(crate) fn scene_bank_label(index: usize, name: Option<&str>) -> String {
    let auto_label = scene_bank_auto_label(index);
    match name {
        Some(name) => format!("{auto_label} — {name}"),
        None => auto_label,
    }
}

/// The scene the transport's pending quantized launch waits for, if any.
/// The host kinds' `queued` fields read it.
pub(crate) fn queued_transport_scene(state: &SequencerState) -> Option<usize> {
    use sequencer::quantized_launch::{PatternLaunchTarget, QuantizedLaunchOwner};
    state
        .quantized_launches()
        .pending_target(QuantizedLaunchOwner::Transport)
        .and_then(|target| match target {
            PatternLaunchTarget::Scene { scene }
            | PatternLaunchTarget::SceneTracks { scene, .. } => Some(scene),
            PatternLaunchTarget::TrackPattern { .. } => None,
        })
}

/// The pattern a track's pending quantized clip launch waits for, if any:
/// the just-assigned scene cell (the click assigns the cell up front and
/// defers the audible launch), or the pattern a song-authority override
/// launch names (`cell.queued`).
pub(crate) fn queued_track_clip(state: &SequencerState, track: usize) -> Option<u64> {
    use sequencer::quantized_launch::{PatternLaunchTarget, QuantizedLaunchOwner};
    match state
        .quantized_launches()
        .pending_target(QuantizedLaunchOwner::TrackClip(track as u32))?
    {
        PatternLaunchTarget::SceneTracks { scene, .. } => {
            state.scene_track_pattern_id(scene, track).map(|id| id.0)
        }
        PatternLaunchTarget::TrackPattern { pattern, .. } => Some(pattern),
        PatternLaunchTarget::Scene { .. } => None,
    }
}

/// Per-frame publish of the song bindings (spec 12). The committed song is
/// re-read only when the committed-song revision changes; scalars publish on
/// change. Returns true when a reactive cycle is needed.
pub(crate) fn sync_song_state(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SongFrameState,
) -> bool {
    let mut dirty = false;
    let revision = app.state.committed_song_revision();
    if frame.revision != Some(revision) {
        frame.cached_song = app.state.committed_song();
        frame.revision = Some(revision);
        dirty = true;
    }
    let next = build_song_bindings_snapshot(app, frame.cached_song.as_ref());
    let prev = frame.prev.as_ref();
    macro_rules! publish_on_change {
        ($field:literal, $accessor:ident, $value:expr) => {
            if prev.map(|prev| prev.$accessor != next.$accessor).unwrap_or(true) {
                rt.set_reactive("SEQ", $field, $value);
                dirty = true;
            }
        };
    }
    publish_on_change!("song-exists", exists, Value::Bool(next.exists));
    publish_on_change!(
        "song-recording-kind",
        recording_kind,
        Value::String(next.recording_kind.to_string())
    );
    publish_on_change!("song-current-row", current_row, Value::Number(next.current_row));
    publish_on_change!(
        "song-current-row-id",
        current_row_id,
        Value::Number(next.current_row_id)
    );
    publish_on_change!("song-row-count", row_count, Value::Number(next.row_count));
    publish_on_change!(
        "song-loop-enabled",
        loop_enabled,
        Value::Bool(next.loop_enabled)
    );
    publish_on_change!(
        "song-capture-failed",
        capture_failed,
        Value::Bool(next.capture_failed)
    );
    publish_on_change!(
        "song-capture-error",
        capture_error,
        match &next.capture_error {
            Some(error) => Value::String(error.clone()),
            None => Value::Nil,
        }
    );
    frame.prev = Some(next);
    dirty
}
