//! Song-mode reactive bindings (docs/song-mode-spec.md section 12): builds
//! and diff-publishes the `SEQ.song-*` / `SEQ.use-arrangement` values each
//! frame from `App` transport state plus the committed song.

use super::*;

use sequencer::app::song_transport::SongTransportMode;
use sequencer::sequencer::{
    project_lanes, state_at_beat, LaneClip, PatternId, ProjectScenes, ProjectSong,
    ProjectSongRow, StepParam,
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
}

/// Per-frame diff state for the song bindings: the committed song is cached
/// and `song-rows` rebuilt only when `committed_song_revision` changes. The
/// lane projection (`song-lanes`) and `scene-names` also depend on the live
/// scenes, which have no revision counter, so they diff by value: recomputed
/// each frame (cheap — rows x tracks `Copy` spans) but republished to Lisp
/// only when the derived data actually changed.
#[derive(Default)]
pub(crate) struct SongFrameState {
    pub(crate) revision: Option<u64>,
    pub(crate) cached_song: Option<ProjectSong>,
    pub(crate) prev: Option<SongBindingsSnapshot>,
    pub(crate) cached_lanes: Option<Vec<Vec<LaneClip>>>,
    pub(crate) cached_scene_names: Option<Vec<String>>,
    /// Pattern-pool event snapshots for the patterns the lane projection
    /// references (`song-lane-events`), rekeyed when the projection or the
    /// pattern epoch changes — not per frame.
    pub(crate) cached_lane_events: Option<Vec<Vec<LanePatternEvents>>>,
    pub(crate) prev_pattern_epoch: Option<u64>,
}

/// Flattened preview events for one pool pattern referenced by a track's
/// lane clips (docs/arrangement-timeline-ui-spec.md 7.1): raw musical events
/// `(time-in-steps, transpose, velocity)` plus the pattern length. The Lisp
/// view owns turning these into normalized dot payloads; the widget never
/// sees steps or timebases.
#[derive(Clone, PartialEq)]
pub(crate) struct LanePatternEvents {
    pub(crate) pattern_id: u64,
    pub(crate) num_steps: usize,
    /// One pattern cycle in musical beats (`num_steps * step_beats` of the
    /// pattern's timebase) — what the view needs to tile a looping clip.
    pub(crate) length_beats: f64,
    pub(crate) events: Vec<(f64, f64, f64)>,
}

/// Bound on published events per pattern so a pathological pattern cannot
/// bloat the reactive value; the view additionally caps dots per item.
const LANE_PATTERN_EVENT_CAP: usize = 1024;

/// Collect the distinct pool patterns each track's lane clips resolve to and
/// flatten their step/chord snapshots into preview events.
pub(crate) fn collect_lane_pattern_events(
    lanes: &[Vec<LaneClip>],
    scenes: &ProjectScenes,
) -> Vec<Vec<LanePatternEvents>> {
    lanes
        .iter()
        .enumerate()
        .map(|(track, clips)| {
            let mut ids: Vec<u64> = clips
                .iter()
                .filter_map(|clip| clip.pattern.map(|pattern| pattern.0))
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids.into_iter()
                .filter_map(|id| {
                    let data = scenes.track_pools.get(track)?.get(PatternId(id))?;
                    let num_steps = data.track_params.num_steps.max(1);
                    let length_beats =
                        data.track_params.timebase.step_beats(num_steps) * num_steps as f64;
                    let mut events = Vec::new();
                    for step in 0..num_steps.min(data.step_data.len()) {
                        if events.len() >= LANE_PATTERN_EVENT_CAP {
                            break;
                        }
                        let velocity =
                            f64::from(data.step_data[step][StepParam::Velocity as usize]);
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
                                    events.push((
                                        step as f64 + f64::from(delay),
                                        f64::from(*transpose),
                                        velocity,
                                    ));
                                }
                            }
                            _ => {
                                let active = (data.track_bits[step / 64] >> (step % 64)) & 1 == 1;
                                if active {
                                    let delay = f64::from(
                                        data.step_data[step][StepParam::Delay as usize],
                                    );
                                    let transpose = f64::from(
                                        data.step_data[step][StepParam::Transpose as usize],
                                    );
                                    events.push((step as f64 + delay, transpose, velocity));
                                }
                            }
                        }
                    }
                    events.truncate(LANE_PATTERN_EVENT_CAP);
                    Some(LanePatternEvents {
                        pattern_id: id,
                        num_steps,
                        length_beats,
                        events,
                    })
                })
                .collect()
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
                        .map(|(time, transpose, velocity)| {
                            Rc::new(RefCell::new(Value::List(vec![
                                Rc::new(RefCell::new(Value::Number(*time))),
                                Rc::new(RefCell::new(Value::Number(*transpose))),
                                Rc::new(RefCell::new(Value::Number(*velocity))),
                            ])))
                        })
                        .collect();
                    let mut map = HashMap::new();
                    map.insert(
                        "pattern-id".to_string(),
                        Rc::new(RefCell::new(Value::Number(pattern.pattern_id as f64))),
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
    }
}

/// Read-only `song-rows` value (spec 12): a list of
/// `{id, start-beat, scene, overrides: ((track pattern-id)...)}` maps.
pub(crate) fn build_song_rows_value(song: Option<&ProjectSong>) -> Value {
    let Some(song) = song else {
        return Value::List(vec![]);
    };
    let rows = song
        .rows
        .iter()
        .map(|row| {
            let overrides = row
                .overrides
                .iter()
                .map(|over| {
                    let pattern_value = match over.pattern_id {
                        Some(id) => Value::Number(id as f64),
                        // Explicit-empty override: the track plays nothing.
                        None => Value::Nil,
                    };
                    Rc::new(RefCell::new(Value::List(vec![
                        Rc::new(RefCell::new(Value::Number(over.track as f64))),
                        Rc::new(RefCell::new(pattern_value)),
                    ])))
                })
                .collect();
            let mut map = HashMap::new();
            map.insert(
                "id".to_string(),
                Rc::new(RefCell::new(Value::Number(row.id.0 as f64))),
            );
            map.insert(
                "start-beat".to_string(),
                Rc::new(RefCell::new(Value::Number(row.start_beat))),
            );
            map.insert(
                "scene".to_string(),
                Rc::new(RefCell::new(Value::Number(row.scene as f64))),
            );
            map.insert(
                "overrides".to_string(),
                Rc::new(RefCell::new(Value::List(overrides))),
            );
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(rows)
}

/// Read-only `song-lanes` value (docs/arrangement-timeline-ui-spec.md 5.5/6):
/// the per-track lane projection as a list (one entry per track) of clip-span
/// lists. Each clip is `{row-id, start-beat, end-beat, pattern-id, from-override}`
/// with `pattern-id` `Nil` for spans where the row resolves no pattern for the
/// track (sparse lanes render nothing for those spans).
pub(crate) fn build_song_lanes_value(lanes: Option<&Vec<Vec<LaneClip>>>) -> Value {
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
                    map.insert(
                        "row-id".to_string(),
                        Rc::new(RefCell::new(Value::Number(clip.row_id.0 as f64))),
                    );
                    map.insert(
                        "start-beat".to_string(),
                        Rc::new(RefCell::new(Value::Number(clip.start_beat))),
                    );
                    map.insert(
                        "end-beat".to_string(),
                        Rc::new(RefCell::new(Value::Number(clip.end_beat))),
                    );
                    map.insert(
                        "pattern-id".to_string(),
                        Rc::new(RefCell::new(match clip.pattern {
                            Some(id) => Value::Number(id.0 as f64),
                            None => Value::Nil,
                        })),
                    );
                    map.insert(
                        "from-override".to_string(),
                        Rc::new(RefCell::new(Value::Bool(clip.from_override))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(clips)))
        })
        .collect();
    Value::List(tracks)
}

fn build_scene_names_value(names: &[String]) -> Value {
    Value::List(
        names
            .iter()
            .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
            .collect(),
    )
}

/// Per-frame publish of the song bindings (spec 12). `song-rows` is rebuilt
/// only when the committed-song revision changes; the lane projection and
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
    if frame.revision != Some(revision) {
        frame.cached_song = app.state.committed_song();
        rt.set_reactive(
            "SEQ",
            "song-rows",
            build_song_rows_value(frame.cached_song.as_ref()),
        );
        frame.revision = Some(revision);
        dirty = true;
    }
    let (lanes, scene_names) = app.state.with_project_scenes(|scenes| {
        (
            frame
                .cached_song
                .as_ref()
                .map(|song| project_lanes(song, scenes)),
            scenes
                .scenes
                .iter()
                .map(|scene| scene.name.clone())
                .collect::<Vec<_>>(),
        )
    });
    let lanes_changed = frame.cached_lanes != lanes;
    if lanes_changed {
        rt.set_reactive("SEQ", "song-lanes", build_song_lanes_value(lanes.as_ref()));
        frame.cached_lanes = lanes;
        dirty = true;
    }
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
