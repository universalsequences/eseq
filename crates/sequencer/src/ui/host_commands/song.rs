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
    "song-set-end",
    "song-set-loop",
    "song-replace",
    "song-clear",
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
/// `{track, pattern-id}` map.
fn parse_override(value: &Value) -> Result<ProjectSongTrackOverride, String> {
    match value {
        Value::List(items) => {
            let numbers: Vec<f64> = items
                .iter()
                .filter_map(|item| match &*item.borrow() {
                    Value::Number(n) => Some(*n),
                    _ => None,
                })
                .collect();
            if numbers.len() != 2 {
                return Err(
                    "override entries must be (track pattern-id) number pairs".to_string()
                );
            }
            override_from_numbers(numbers[0], numbers[1])
        }
        Value::Map(map) => {
            let track = map_number(map, "track")
                .ok_or_else(|| "override entry is missing :track".to_string())?;
            let pattern_id = map_number(map, "pattern-id")
                .or_else(|| map_number(map, "pattern_id"))
                .ok_or_else(|| "override entry is missing :pattern-id".to_string())?;
            override_from_numbers(track, pattern_id)
        }
        _ => Err("override entries must be (track pattern-id) pairs or maps".to_string()),
    }
}

fn override_from_numbers(track: f64, pattern_id: f64) -> Result<ProjectSongTrackOverride, String> {
    if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
        return Err("override track must be a non-negative integer".to_string());
    }
    if !pattern_id.is_finite() || pattern_id < 1.0 || pattern_id.fract() != 0.0 {
        return Err("override pattern-id must be a positive integer".to_string());
    }
    Ok(ProjectSongTrackOverride {
        track: track as usize,
        pattern_id: pattern_id as u64,
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

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
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
            ProjectSongTrackOverride { track: 1, pattern_id: 3 }
        );
        assert_eq!(
            parse_override(&value_map(vec![
                ("track", Value::Number(2.0)),
                ("pattern-id", Value::Number(5.0)),
            ]))
            .unwrap(),
            ProjectSongTrackOverride { track: 2, pattern_id: 5 }
        );
        assert!(parse_override(&pair(-1.0, 3.0)).is_err());
        assert!(parse_override(&pair(1.0, 0.0)).is_err(), "pattern id 0 is reserved");
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
        assert_eq!(spec.overrides, vec![ProjectSongTrackOverride { track: 0, pattern_id: 3 }]);

        let missing_scene = value_map(vec![("start-beat", Value::Number(0.0))]);
        assert!(parse_row_spec(&missing_scene).is_err());
    }
}
