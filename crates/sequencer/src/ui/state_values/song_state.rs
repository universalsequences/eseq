//! Song-mode reactive bindings (docs/song-mode-spec.md section 12): builds
//! and diff-publishes the `SEQ.song-*` / `SEQ.use-arrangement` values each
//! frame from `App` transport state plus the committed song.

use super::*;

use sequencer::app::song_transport::SongTransportMode;
use sequencer::sequencer::{state_at_beat, ProjectSong, ProjectSongRow};

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
/// and `song-rows` rebuilt only when `committed_song_revision` changes.
#[derive(Default)]
pub(crate) struct SongFrameState {
    pub(crate) revision: Option<u64>,
    pub(crate) cached_song: Option<ProjectSong>,
    pub(crate) prev: Option<SongBindingsSnapshot>,
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
                    Rc::new(RefCell::new(Value::List(vec![
                        Rc::new(RefCell::new(Value::Number(over.track as f64))),
                        Rc::new(RefCell::new(Value::Number(over.pattern_id as f64))),
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

/// Per-frame publish of the song bindings (spec 12). `song-rows` is rebuilt
/// only when the committed-song revision changes; scalars publish on change;
/// the render-rate `song-position-beats` publishes only while the transport
/// panel is visible. Returns true when a reactive cycle is needed.
pub(crate) fn sync_song_state(
    rt: &mut Runtime,
    app: &app::App,
    frame: &mut SongFrameState,
    transport_visible: bool,
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
    if position_changed && transport_visible {
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
