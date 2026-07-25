//! Song-mode reactive bindings (docs/song-mode-spec.md section 12): builds
//! and diff-publishes the `SEQ.song-*` / `SEQ.use-arrangement` values each
//! frame from `App` transport state plus the committed song.

use super::*;

use sequencer::app::song_transport::SongTransportMode;
use sequencer::sequencer::{
    arrangement_scene_spans, state_at_beat, ArrClip, ProjectScenes, ProjectSong, ProjectSongRow,
    SceneSpan, StepParam,
};

/// Scalar song bindings published to `SEQ.*`, snapshotted per frame so each
/// reactive is only rewritten when its value changed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SongBindingsSnapshot {
    pub(crate) exists: bool,
    pub(crate) use_arrangement: bool,
    pub(crate) mode: &'static str,
    /// Current row ordinal during song playback, else -1.
    pub(crate) current_row: f64,
    /// Current row stable id during song playback, else -1.
    pub(crate) current_row_id: f64,
    pub(crate) row_count: f64,
    /// Smooth render-rate song position (spec 10.2); 0.0 while inactive.
    pub(crate) position_beats: f64,
    pub(crate) end_beat: f64,
    pub(crate) loop_enabled: bool,
    /// Latched failure state of the most recent arrangement capture
    /// (docs/song-mode-spec.md 12); cleared when the next capture starts.
    pub(crate) capture_failed: bool,
    pub(crate) capture_error: Option<String>,
    /// Latched rejection of the most recent song editing primitive, cleared
    /// by the next successful edit. Bound to `SEQ.song-edit-error` so the
    /// arrangement view can surface it (the step tile hides the status line).
    pub(crate) edit_error: Option<String>,
    /// Per-track take-lane state (takes spec 10/11.2 UX): 0 = the lane is
    /// not playing a take (pattern lanes stay fully editable — "jam with the
    /// step sequencer"), 1 = take-governed (dimmed, non-interactive steps +
    /// lit Back-to-Song button), 2 = a take lane the performer manually
    /// latched away (editable again; grey button returns it to the song).
    pub(crate) take_lane_states: Vec<u8>,
    /// True while any lane is manual-override latched during song playback
    /// (takes spec 10): the SONG indicator glows amber and the Back to Song
    /// control appears.
    pub(crate) manual_latch: bool,
    /// The bound clip's `(track, clip-id)` when a timeline selection holds the
    /// binding (rule 1), for the bound-clip highlight. `None` under rules 2/3.
    pub(crate) bound_clip: Option<(usize, u64)>,
    /// The committed region selection as `(track-a track-b start end
    /// scene-lane?)` (docs/arrangement-region-editing-spec.md 4.1), or `None`.
    /// Rust-owned so every lane's `:selection-rect` survives a view switch.
    pub(crate) region: Option<(usize, usize, f64, f64, bool)>,
}

/// Per-frame diff state for the song bindings: the committed song and
/// arrangement are cached and re-read only when `committed_song_revision`
/// changes (`set_committed_arrangement` bumps it). The lane surfaces
/// (`song-lanes`, `scene-spans`) are functions of the arrangement alone, so
/// they follow the revision; `scene-names` depends on the live scenes, which
/// have no revision counter, so it diffs by value.
#[derive(Default)]
pub(crate) struct SongFrameState {
    pub(crate) revision: Option<u64>,
    pub(crate) cached_song: Option<ProjectSong>,
    pub(crate) cached_arrangement: Option<sequencer::sequencer::ProjectArrangement>,
    pub(crate) prev: Option<SongBindingsSnapshot>,
    /// The stored clip lanes, published verbatim as `SEQ.song-lanes`.
    pub(crate) cached_lanes: Option<Vec<Vec<ArrClip>>>,
    pub(crate) cached_scene_spans: Option<Vec<SceneSpan>>,
    pub(crate) cached_scene_names: Option<Vec<String>>,
    /// Pattern-pool event snapshots for the patterns the lane projection
    /// references (`song-lane-events`), rekeyed when the projection or the
    /// pattern epoch changes — not per frame.
    pub(crate) cached_lane_events: Option<Vec<Vec<LanePatternEvents>>>,
    pub(crate) prev_pattern_epoch: Option<u64>,
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
                    let data = scenes.track_pools.get(track)?.get(PatternId(id))?;
                    let num_steps = data.track_params.num_steps.max(1);
                    let length_beats =
                        data.track_params.timebase.step_beats(num_steps) * num_steps as f64;
                    let mut events = Vec::new();
                    flatten_pattern_events(data, 0.0, num_steps, &mut events);
                    events.truncate(LANE_PATTERN_EVENT_CAP);
                    Some(LanePatternEvents {
                        pattern_id: id,
                        take_id: None,
                        num_steps,
                        length_beats,
                        events,
                    })
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
                    flatten_pattern_events(data, base as f64, limit, &mut events);
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

/// `song-lane-events` value: per track, a list of
/// `{pattern-id, num-steps, events: ((time transpose velocity)...)}` maps.
pub(crate) fn build_song_lane_events_value(events: &[Vec<LanePatternEvents>]) -> Value {
    let tracks = events
        .iter()
        .map(|patterns| {
            let patterns = patterns
                .iter()
                .map(|pattern| {
                    let events = pattern
                        .events
                        .iter()
                        .map(|(time, transpose, velocity, duration)| {
                            Rc::new(RefCell::new(Value::List(vec![
                                Rc::new(RefCell::new(Value::Number(*time))),
                                Rc::new(RefCell::new(Value::Number(*transpose))),
                                Rc::new(RefCell::new(Value::Number(*velocity))),
                                Rc::new(RefCell::new(Value::Number(*duration))),
                            ])))
                        })
                        .collect();
                    let mut map = HashMap::new();
                    map.insert(
                        "pattern-id".to_string(),
                        Rc::new(RefCell::new(match pattern.take_id {
                            // Take entries carry no pattern identity.
                            Some(_) => Value::Nil,
                            None => Value::Number(pattern.pattern_id as f64),
                        })),
                    );
                    map.insert(
                        "take-id".to_string(),
                        Rc::new(RefCell::new(match pattern.take_id {
                            Some(id) => Value::Number(id as f64),
                            None => Value::Nil,
                        })),
                    );
                    map.insert(
                        "num-steps".to_string(),
                        Rc::new(RefCell::new(Value::Number(pattern.num_steps as f64))),
                    );
                    map.insert(
                        "length-beats".to_string(),
                        Rc::new(RefCell::new(Value::Number(pattern.length_beats))),
                    );
                    map.insert(
                        "events".to_string(),
                        Rc::new(RefCell::new(Value::List(events))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(patterns)))
        })
        .collect();
    Value::List(tracks)
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
    let mode = app.song_transport_mode.binding_str();
    let position = app.state.song_position_beats();
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
        use_arrangement: app.use_arrangement,
        mode,
        current_row,
        current_row_id,
        row_count: song.map(|song| song.rows.len()).unwrap_or(0) as f64,
        // Quantized to a milli-beat for display: still render-rate smooth,
        // but sub-display-precision jitter does not force a reactive cycle
        // every frame.
        position_beats: (position.unwrap_or(0.0) * 1000.0).round() / 1000.0,
        end_beat: song.map(|song| song.end_beat).unwrap_or(0.0),
        loop_enabled: song.map(|song| song.loop_enabled).unwrap_or(false),
        capture_failed: app.song_capture_failed,
        capture_error: app.song_capture_error.clone(),
        edit_error: app.song_edit_error.clone(),
        manual_latch: app.state.song_manual_latch_mask() != 0,
        take_lane_states: song_take_lane_states(app),
        bound_clip: app
            .song_clip_selection
            .map(|selection| (selection.track, selection.clip_id.0)),
        region: app.song_region_selection.map(|region| {
            (
                region.track_a,
                region.track_b,
                region.start_beat,
                region.end_beat,
                region.scene_lane,
            )
        }),
    }
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
                let latched = *track < 64 && latch >> track & 1 == 1;
                *state = if latched { 2 } else { 1 };
            }
        }
    });
    states
}

fn number_field(map: &mut HashMap<String, Rc<RefCell<Value>>>, key: &str, value: f64) {
    map.insert(key.to_string(), Rc::new(RefCell::new(Value::Number(value))));
}

fn optional_number_field(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
    value: Option<u64>,
) {
    map.insert(
        key.to_string(),
        Rc::new(RefCell::new(match value {
            Some(value) => Value::Number(value as f64),
            None => Value::Nil,
        })),
    );
}

/// Read-only `song-lanes` value (arrangement-lane-model-spec 12): the STORED
/// clips, one list per track, each `{clip-id, start-beat, end-beat,
/// pattern-id, take-id, offset-steps}`. Real identity — the view never merges
/// or re-derives anything, and `from-override` is gone: every lane item IS a
/// clip. Lane gaps carry no entry: they are silence.
pub(crate) fn build_song_lanes_value(lanes: Option<&Vec<Vec<ArrClip>>>) -> Value {
    let Some(lanes) = lanes else {
        return Value::List(vec![]);
    };
    let tracks = lanes
        .iter()
        .map(|clips| {
            let clips = clips
                .iter()
                .map(|clip| {
                    let mut map = HashMap::new();
                    number_field(&mut map, "clip-id", clip.id.0 as f64);
                    number_field(&mut map, "start-beat", clip.start_beat);
                    number_field(&mut map, "end-beat", clip.end_beat);
                    optional_number_field(&mut map, "pattern-id", clip.pattern_id);
                    optional_number_field(&mut map, "take-id", clip.take_id);
                    number_field(&mut map, "offset-steps", clip.offset_steps);
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(clips)))
        })
        .collect();
    Value::List(tracks)
}

/// Read-only `scene-spans` value (spec 12): one span per scene EVENT,
/// `{start-beat, end-beat, scene}`. Replaces the scene half of the retired
/// `song-rows`; the scene lane renders these directly, so a clip edge on some
/// track can no longer fragment it.
pub(crate) fn build_scene_spans_value(spans: Option<&Vec<SceneSpan>>) -> Value {
    let Some(spans) = spans else {
        return Value::List(vec![]);
    };
    Value::List(
        spans
            .iter()
            .map(|span| {
                let mut map = HashMap::new();
                number_field(&mut map, "start-beat", span.start_beat);
                number_field(&mut map, "end-beat", span.end_beat);
                number_field(&mut map, "scene", span.scene as f64);
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

fn build_scene_names_value(names: &[String]) -> Value {
    Value::List(
        names
            .iter()
            .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
            .collect(),
    )
}

/// Per-frame publish of the song bindings (spec 12). The committed song and
/// arrangement are re-read only when the committed-song revision changes; the
/// scene names diff by value (the scenes side has no revision counter);
/// scalars publish on change; the render-rate `song-position-beats` publishes
/// only while a panel that renders it (transport or arrangement) is visible.
/// Returns true when a reactive cycle is needed.
pub(crate) fn sync_song_state(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SongFrameState,
    song_position_visible: bool,
) -> bool {
    let mut dirty = false;
    let revision = app.state.committed_song_revision();
    // `song-lanes` and `scene-spans` are functions of the arrangement ALONE,
    // so they are rebuilt only when the revision moves.
    let mut lanes_changed = false;
    if frame.revision != Some(revision) {
        frame.cached_song = app.state.committed_song();
        let arrangement = app.state.committed_arrangement();
        let lanes = arrangement
            .as_ref()
            .map(|arrangement| arrangement.track_lanes.clone());
        let scene_spans = arrangement.as_ref().map(arrangement_scene_spans);
        if frame.cached_lanes != lanes {
            rt.set_reactive("SEQ", "song-lanes", build_song_lanes_value(lanes.as_ref()));
            frame.cached_lanes = lanes;
            lanes_changed = true;
        }
        if frame.cached_scene_spans != scene_spans {
            rt.set_reactive(
                "SEQ",
                "scene-spans",
                build_scene_spans_value(scene_spans.as_ref()),
            );
            frame.cached_scene_spans = scene_spans;
        }
        frame.cached_arrangement = arrangement;
        frame.revision = Some(revision);
        dirty = true;
    }
    let scene_names = app.state.with_project_scenes(|scenes| {
        scenes
            .scenes
            .iter()
            .map(|scene| scene.name.clone())
            .collect::<Vec<_>>()
    });
    if frame.cached_scene_names.as_ref() != Some(&scene_names) {
        rt.set_reactive("SEQ", "scene-names", build_scene_names_value(&scene_names));
        frame.cached_scene_names = Some(scene_names);
        dirty = true;
    }
    // Preview events for the patterns the projection references: re-snapshot
    // only when the projection itself or the pattern data (epoch) changed,
    // then diff by value so an unchanged snapshot publishes nothing
    // (docs/arrangement-timeline-ui-spec.md 7.1: recompute on pattern/row
    // change, not per frame).
    let pattern_epoch = app
        .state
        .transport
        .pattern_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    if lanes_changed || frame.prev_pattern_epoch != Some(pattern_epoch) {
        let events = match frame.cached_lanes.as_ref() {
            Some(lanes) => app
                .state
                .with_project_scenes(|scenes| collect_lane_pattern_events(lanes, scenes)),
            None => Vec::new(),
        };
        if frame.cached_lane_events.as_ref() != Some(&events) {
            rt.set_reactive(
                "SEQ",
                "song-lane-events",
                build_song_lane_events_value(&events),
            );
            frame.cached_lane_events = Some(events);
            dirty = true;
        }
        frame.prev_pattern_epoch = Some(pattern_epoch);
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
        "use-arrangement",
        use_arrangement,
        Value::Bool(next.use_arrangement)
    );
    publish_on_change!("song-mode", mode, Value::String(next.mode.to_string()));
    publish_on_change!("song-current-row", current_row, Value::Number(next.current_row));
    publish_on_change!(
        "song-current-row-id",
        current_row_id,
        Value::Number(next.current_row_id)
    );
    publish_on_change!("song-row-count", row_count, Value::Number(next.row_count));
    publish_on_change!("song-end-beat", end_beat, Value::Number(next.end_beat));
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
    publish_on_change!(
        "song-manual-latch",
        manual_latch,
        Value::Bool(next.manual_latch)
    );
    publish_on_change!(
        "song-edit-error",
        edit_error,
        match &next.edit_error {
            Some(error) => Value::String(error.clone()),
            None => Value::Nil,
        }
    );
    let governed_changed = prev
        .map(|prev| prev.take_lane_states != next.take_lane_states)
        .unwrap_or(true);
    if governed_changed {
        let items: Vec<Rc<RefCell<Value>>> = next
            .take_lane_states
            .iter()
            .map(|state| Rc::new(RefCell::new(Value::Number(*state as f64))))
            .collect();
        rt.set_reactive("SEQ", "song-track-governed", Value::List(items));
        // The take-governed dim rides the step-cell color channels (the
        // header keeps its full track color); resync them so the step
        // shells restyle live as rows enter/leave take lanes.
        super::track_and_mixer::sync_track_mute_visual_binding_fields(
            rt,
            app,
            &app.state,
            0..next.take_lane_states.len(),
            false,
        );
        dirty = true;
    }
    publish_on_change!(
        "song-bound-clip",
        bound_clip,
        match next.bound_clip {
            Some((track, row_id)) => Value::List(vec![
                Rc::new(RefCell::new(Value::Number(track as f64))),
                Rc::new(RefCell::new(Value::Number(row_id as f64))),
            ]),
            None => Value::Nil,
        }
    );
    publish_on_change!(
        "song-region",
        region,
        match next.region {
            Some((track_a, track_b, start, end, scene_lane)) => Value::List(vec![
                Rc::new(RefCell::new(Value::Number(track_a as f64))),
                Rc::new(RefCell::new(Value::Number(track_b as f64))),
                Rc::new(RefCell::new(Value::Number(start))),
                Rc::new(RefCell::new(Value::Number(end))),
                Rc::new(RefCell::new(Value::Bool(scene_lane))),
            ]),
            None => Value::Nil,
        }
    );
    let position_changed = prev
        .map(|prev| prev.position_beats != next.position_beats)
        .unwrap_or(true);
    if position_changed && song_position_visible {
        rt.set_reactive(
            "SEQ",
            "song-position-beats",
            Value::Number(next.position_beats),
        );
        dirty = true;
    }
    frame.prev = Some(next);
    dirty
}
