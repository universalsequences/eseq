//! Host commands wrapping the song-mode editing primitives
//! (docs/song-mode-spec.md 5.6/12). Each command routes to one `App` method
//! in `sequencer::app::song_edit`, which validates, applies atomically, and
//! commits exactly one undo entry; success and failure surface on the status
//! line.

use crate::*;

use sequencer::sequencer::{ProjectSongTrackOverride, SongRowId};
use sequencer::app::song_edit::SongRowSpec;

pub(super) const COMMANDS: &[&str] = &[
    "song-row-insert",
    "song-row-remove",
    "song-row-move",
    "song-row-set-state",
    "song-track-paint",
    "song-set-end",
    "song-set-loop",
    "song-replace",
    "song-clear",
    // Transport authority (docs/song-mode-spec.md 12/13): routed through the
    // state machine in app/song_transport.rs.
    "song-transport-toggle-play",
    "song-transport-play",
    "song-use-arrangement",
    "song-capture-arm",
    "song-capture-cancel",
    "song-status",
];

fn payload_map(payload: &Value) -> Result<&HashMap<String, Rc<RefCell<Value>>>, String> {
    match payload {
        Value::Map(map) => Ok(map),
        _ => Err("invalid payload: expected a map".to_string()),
    }
}

fn require_number(
    map: &HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Result<f64, String> {
    map_number(map, key).ok_or_else(|| format!("missing or non-numeric :{key}"))
}

fn require_row_id(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<SongRowId, String> {
    let value = require_number(map, "row-id")?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err("row-id must be a non-negative integer".to_string());
    }
    Ok(SongRowId(value as u64))
}

fn require_scene(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<usize, String> {
    map_usize(map, "scene").ok_or_else(|| "missing or invalid :scene".to_string())
}

/// Parse one override from either a `(track pattern-id)` pair list or a
/// `{track, pattern-id}` map. A pattern-id of `nil` or `0` is an
/// explicit-empty override (the track plays nothing for the row); positive
/// integers are pool ids.
fn parse_override(value: &Value) -> Result<ProjectSongTrackOverride, String> {
    match value {
        Value::List(items) => {
            if items.len() != 2 {
                return Err(
                    "override entries must be (track pattern-id) pairs".to_string()
                );
            }
            let track = match &*items[0].borrow() {
                Value::Number(n) => *n,
                _ => return Err("override track must be a number".to_string()),
            };
            let pattern_id = match &*items[1].borrow() {
                Value::Number(n) => Some(*n),
                Value::Nil => None,
                _ => {
                    return Err(
                        "override pattern-id must be a number or nil (explicit-empty)"
                            .to_string(),
                    )
                }
            };
            override_from_numbers(track, pattern_id)
        }
        Value::Map(map) => {
            let track = map_number(map, "track")
                .ok_or_else(|| "override entry is missing :track".to_string())?;
            let has_key = map.contains_key("pattern-id") || map.contains_key("pattern_id");
            let pattern_id = map_number(map, "pattern-id")
                .or_else(|| map_number(map, "pattern_id"));
            if pattern_id.is_none() && !has_key {
                return Err("override entry is missing :pattern-id".to_string());
            }
            override_from_numbers(track, pattern_id)
        }
        _ => Err("override entries must be (track pattern-id) pairs or maps".to_string()),
    }
}

fn override_from_numbers(
    track: f64,
    pattern_id: Option<f64>,
) -> Result<ProjectSongTrackOverride, String> {
    if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
        return Err("override track must be a non-negative integer".to_string());
    }
    let pattern_id = match pattern_id {
        None => None,
        Some(id) if id == 0.0 => None,
        Some(id) => {
            if !id.is_finite() || id < 1.0 || id.fract() != 0.0 {
                return Err(
                    "override pattern-id must be a positive integer, or 0/nil for \
                     explicit-empty"
                        .to_string(),
                );
            }
            Some(id as u64)
        }
    };
    Ok(ProjectSongTrackOverride {
        track: track as usize,
        pattern_id,
    })
}

fn parse_overrides(
    map: &HashMap<String, Rc<RefCell<Value>>>,
) -> Result<Vec<ProjectSongTrackOverride>, String> {
    let Some(cell) = map.get("overrides") else {
        return Ok(Vec::new());
    };
    match &*cell.borrow() {
        Value::Nil => Ok(Vec::new()),
        Value::List(items) => items
            .iter()
            .map(|item| parse_override(&item.borrow()))
            .collect(),
        _ => Err("overrides must be a list".to_string()),
    }
}

fn parse_row_spec(value: &Value) -> Result<SongRowSpec, String> {
    let Value::Map(map) = value else {
        return Err("song rows must be maps with :start-beat/:scene/:overrides".to_string());
    };
    Ok(SongRowSpec {
        start_beat: require_number(map, "start-beat")?,
        scene: require_scene(map)?,
        overrides: parse_overrides(map)?,
    })
}

fn parse_rows(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<Vec<SongRowSpec>, String> {
    let Some(cell) = map.get("rows") else {
        return Err("missing :rows".to_string());
    };
    match &*cell.borrow() {
        Value::List(items) => items
            .iter()
            .map(|item| parse_row_spec(&item.borrow()))
            .collect(),
        _ => Err(":rows must be a list".to_string()),
    }
}

fn run(name: &str, payload: &Value, app: &mut app::App) -> Result<String, String> {
    match name {
        "song-row-insert" => {
            let map = payload_map(payload)?;
            let start_beat = require_number(map, "start-beat")?;
            let scene = require_scene(map)?;
            let overrides = parse_overrides(map)?;
            let outcome = app.song_row_insert(start_beat, scene, overrides)?;
            Ok(match outcome.created_with_end_beat {
                Some(end_beat) => format!(
                    "Created song: row {} at beat {start_beat} (default end beat {end_beat}; \
                     adjust with song-set-end)",
                    outcome.row_id.0
                ),
                None => format!("Inserted song row {} at beat {start_beat}", outcome.row_id.0),
            })
        }
        "song-row-remove" => {
            let map = payload_map(payload)?;
            let row_id = require_row_id(map)?;
            app.song_row_remove(row_id)?;
            Ok(format!("Removed song row {}", row_id.0))
        }
        "song-row-move" => {
            let map = payload_map(payload)?;
            let row_id = require_row_id(map)?;
            let start_beat = require_number(map, "start-beat")?;
            app.song_row_move(row_id, start_beat)?;
            Ok(format!("Moved song row {} to beat {start_beat}", row_id.0))
        }
        "song-row-set-state" => {
            let map = payload_map(payload)?;
            let row_id = require_row_id(map)?;
            let scene = require_scene(map)?;
            let overrides = parse_overrides(map)?;
            app.song_row_set_state(row_id, scene, overrides)?;
            Ok(format!("Set song row {} state", row_id.0))
        }
        "song-track-paint" => {
            let map = payload_map(payload)?;
            let track = require_number(map, "track")?;
            if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
                return Err("track must be a non-negative integer".to_string());
            }
            let start_beat = require_number(map, "start-beat")?;
            let end_beat = require_number(map, "end-beat")?;
            // pattern-id nil/absent/0 = explicit-empty (silence the track).
            let pattern_id = match map.get("pattern-id").map(|cell| cell.borrow().clone()) {
                None | Some(Value::Nil) => None,
                Some(Value::Number(id)) if id == 0.0 => None,
                Some(Value::Number(id))
                    if id.is_finite() && id >= 1.0 && id.fract() == 0.0 =>
                {
                    Some(id as u64)
                }
                _ => {
                    return Err(
                        "pattern-id must be a positive integer, or 0/nil for silence"
                            .to_string(),
                    )
                }
            };
            app.song_track_paint(track as usize, start_beat, end_beat, pattern_id)?;
            Ok(match pattern_id {
                Some(id) => format!(
                    "Painted pattern {id} on track {} over beats {start_beat}-{end_beat}",
                    track as usize + 1
                ),
                None => format!(
                    "Silenced track {} over beats {start_beat}-{end_beat}",
                    track as usize + 1
                ),
            })
        }
        "song-set-end" => {
            let map = payload_map(payload)?;
            let end_beat = require_number(map, "end-beat")?;
            app.song_set_end(end_beat)?;
            Ok(format!("Set song end to beat {end_beat}"))
        }
        "song-set-loop" => {
            let map = payload_map(payload)?;
            let enabled = map_bool(map, "enabled");
            app.song_set_loop(enabled)?;
            Ok(format!(
                "Song loop {}",
                if enabled { "enabled" } else { "disabled" }
            ))
        }
        "song-replace" => {
            let map = payload_map(payload)?;
            let rows = parse_rows(map)?;
            let end_beat = require_number(map, "end-beat")?;
            let loop_enabled = map_bool(map, "loop");
            let name = map_string(map, "name");
            let ids = app.song_replace(rows, end_beat, loop_enabled)?;
            Ok(match name {
                Some(name) => format!(
                    "Committed song \"{name}\": {} row(s), end beat {end_beat}",
                    ids.len()
                ),
                None => format!("Replaced song: {} row(s), end beat {end_beat}", ids.len()),
            })
        }
        "song-clear" => {
            app.song_clear()?;
            Ok("Cleared song".to_string())
        }
        _ => Err(format!("unknown song command: {name}")),
    }
}

/// Whether the record signal selects arrangement capture at Play time
/// (docs/song-mode-spec.md section 1): the explicit capture arm or the
/// existing pattern/note record toggle.
fn transport_record_signal(app: &app::App, ctx: &LoopCtx<'_>) -> bool {
    app.song_capture_armed || ctx.shared.recording.load(Ordering::Relaxed)
}

fn run_transport(
    name: &str,
    payload: &Value,
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
) -> Result<Option<String>, String> {
    match name {
        "song-transport-toggle-play" => {
            let record = transport_record_signal(app, ctx);
            app.song_transport_toggle_play(record)
        }
        "song-transport-play" => {
            let record = transport_record_signal(app, ctx);
            app.song_transport_play(record).map(|mode| match mode {
                sequencer::app::song_transport::SongTransportMode::SongPlayback => {
                    Some("Song playback started".to_string())
                }
                sequencer::app::song_transport::SongTransportMode::ArrangementCapture => {
                    Some("Arrangement capture started".to_string())
                }
                _ => None,
            })
        }
        "song-use-arrangement" => {
            let map = payload_map(payload)?;
            let enabled = map_bool(map, "enabled");
            app.set_use_arrangement(enabled)?;
            Ok(Some(format!(
                "Use Arrangement {}",
                if enabled { "on" } else { "off" }
            )))
        }
        "song-capture-arm" => {
            let map = payload_map(payload)?;
            let armed = map_bool(map, "armed");
            app.set_song_capture_armed(armed)?;
            Ok(Some(format!(
                "Arrangement capture {}",
                if armed { "armed" } else { "disarmed" }
            )))
        }
        "song-capture-cancel" => app.song_capture_cancel().map(Some),
        "song-status" => Ok(Some(song_status_summary(app))),
        _ => Err(format!("unknown song transport command: {name}")),
    }
}

/// Status-line summary for `seq-song-status` (docs/song-mode-spec.md 12).
fn song_status_summary(app: &app::App) -> String {
    let mode = app.song_transport_mode.binding_str();
    let arrangement = if app.use_arrangement { "on" } else { "off" };
    match app.state.committed_song() {
        Some(song) => format!(
            "Song: {} row(s), end beat {}, loop {} — mode {mode}, Use Arrangement {arrangement}",
            song.rows.len(),
            song.end_beat,
            if song.loop_enabled { "on" } else { "off" },
        ),
        None => format!("No committed song — mode {mode}, Use Arrangement {arrangement}"),
    }
}

/// Apply a song editing-primitive command outside the interactive event loop
/// (headless capture setup). Transport commands need the shared loop context
/// and are not supported there; returns `None` for any non-song-edit command.
pub(crate) fn apply_song_edit_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Option<Result<String, String>> {
    if !COMMANDS.contains(&name) || TRANSPORT_COMMANDS.contains(&name) {
        return None;
    }
    Some(run(name, payload, app))
}

const TRANSPORT_COMMANDS: &[&str] = &[
    "song-transport-toggle-play",
    "song-transport-play",
    "song-use-arrangement",
    "song-capture-arm",
    "song-capture-cancel",
    "song-status",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    if TRANSPORT_COMMANDS.contains(&name) {
        match run_transport(name, &payload, app, ctx) {
            Ok(Some(status)) => editor.handle_host_event(HostEvent::Status(status)),
            Ok(None) => {}
            Err(error) => {
                editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}")))
            }
        }
        return;
    }
    match run(name, &payload, app) {
        Ok(status) => editor.handle_host_event(HostEvent::Status(status)),
        Err(error) => editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_map(fields: Vec<(&str, Value)>) -> Value {
        let mut map = HashMap::new();
        for (key, value) in fields {
            map.insert(key.to_string(), Rc::new(RefCell::new(value)));
        }
        Value::Map(map)
    }

    fn pair(track: f64, pattern: f64) -> Value {
        Value::List(vec![
            Rc::new(RefCell::new(Value::Number(track))),
            Rc::new(RefCell::new(Value::Number(pattern))),
        ])
    }

    #[test]
    fn override_entries_parse_from_pairs_and_maps() {
        assert_eq!(
            parse_override(&pair(1.0, 3.0)).unwrap(),
            ProjectSongTrackOverride { track: 1, pattern_id: Some(3) }
        );
        assert_eq!(
            parse_override(&value_map(vec![
                ("track", Value::Number(2.0)),
                ("pattern-id", Value::Number(5.0)),
            ]))
            .unwrap(),
            ProjectSongTrackOverride { track: 2, pattern_id: Some(5) }
        );
        assert!(parse_override(&pair(-1.0, 3.0)).is_err());
        // Pattern id 0 and nil both mean explicit-empty (the track plays
        // nothing for the row).
        assert_eq!(
            parse_override(&pair(1.0, 0.0)).unwrap(),
            ProjectSongTrackOverride { track: 1, pattern_id: None }
        );
        assert_eq!(
            parse_override(&Value::List(vec![
                Rc::new(RefCell::new(Value::Number(1.0))),
                Rc::new(RefCell::new(Value::Nil)),
            ]))
            .unwrap(),
            ProjectSongTrackOverride { track: 1, pattern_id: None }
        );
        assert!(parse_override(&pair(1.0, -2.0)).is_err());
        assert!(parse_override(&pair(1.0, 1.5)).is_err());
        assert!(parse_override(&Value::Number(3.0)).is_err());
    }

    #[test]
    fn row_specs_parse_with_fractional_beats() {
        let row = value_map(vec![
            ("start-beat", Value::Number(47.5)),
            ("scene", Value::Number(2.0)),
            (
                "overrides",
                Value::List(vec![Rc::new(RefCell::new(pair(0.0, 3.0)))]),
            ),
        ]);
        let spec = parse_row_spec(&row).unwrap();
        assert_eq!(spec.start_beat, 47.5);
        assert_eq!(spec.scene, 2);
        assert_eq!(
            spec.overrides,
            vec![ProjectSongTrackOverride { track: 0, pattern_id: Some(3) }]
        );

        let missing_scene = value_map(vec![("start-beat", Value::Number(0.0))]);
        assert!(parse_row_spec(&missing_scene).is_err());
    }
}
