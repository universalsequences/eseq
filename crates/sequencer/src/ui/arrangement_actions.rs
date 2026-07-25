//! Arrangement-timeline gesture translation
//! (docs/arrangement-timeline-ui-spec.md 9): the one seam that lowers a
//! finished timeline gesture into arrangement editing primitives
//! (docs/arrangement-lane-model-spec.md 8). Live drag actions never reach
//! this module — the Lisp view keeps them as ghost preview state and forwards
//! only the terminal action, augmented with the ghost's final values. Each
//! returned command is one validated, atomic, one-undo-entry primitive; this
//! module never mutates anything itself.
//!
//! Gestures address the model the way the read surface lets them. Until
//! `SEQ.song-lanes` publishes stored clip ids (lane spec 12, phase 5) the
//! view still speaks compiled ROW ids for the scene lane and merged clip
//! SPANS for the track lanes, so this module translates both into the
//! beat-addressed primitives: a row id becomes its start beat, and a clip
//! span becomes `(:track, :at-beat, :at-end)`, which the host command
//! resolves through `App::arrangement_clip_at`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eseqlisp::vm::Value;

use sequencer::sequencer::{ProjectArrangement, ProjectSong, SongRowId};

/// One song host command to enqueue: `(name, payload)`.
pub(crate) type SongCommand = (&'static str, Value);

fn action_map(action: &Value) -> Result<&HashMap<String, Rc<RefCell<Value>>>, String> {
    match action {
        Value::Map(map) => Ok(map),
        _ => Err("expected an action map".to_string()),
    }
}

fn map_field(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<Value> {
    map.get(key).map(|value| value.borrow().clone())
}

fn require_beat(
    map: &HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Result<f64, String> {
    match map_field(map, key) {
        Some(Value::Number(value)) if value.is_finite() => Ok(value),
        _ => Err(format!("missing or non-finite :{key}")),
    }
}

fn require_row_id(
    map: &HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Result<u64, String> {
    match map_field(map, key) {
        Some(Value::Number(value))
            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 =>
        {
            Ok(value as u64)
        }
        _ => Err(format!("missing or invalid :{key}")),
    }
}

fn require_scene(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<f64, String> {
    match map_field(map, "scene") {
        Some(Value::Number(value))
            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 =>
        {
            Ok(value)
        }
        _ => Err("missing or invalid :scene".to_string()),
    }
}

fn action_type(map: &HashMap<String, Rc<RefCell<Value>>>) -> Option<String> {
    match map_field(map, "type") {
        Some(Value::Keyword(name)) | Some(Value::String(name)) => Some(name),
        _ => None,
    }
}

fn payload(fields: Vec<(&str, Value)>) -> Value {
    Value::Map(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

/// The start beat of the compiled row a scene-lane gesture named. `Err` when
/// the id is not in the song at all — a stale gesture must never silently
/// edit a different part of the timeline.
fn row_start_beat(song: &ProjectSong, row_id: u64) -> Result<f64, String> {
    song.rows
        .iter()
        .find(|row| row.id == SongRowId(row_id))
        .map(|row| row.start_beat)
        .ok_or_else(|| format!("song has no row with id {row_id}"))
}

/// The scene change immediately after `beat`, or `None` when `beat` sits in
/// the last scene span (whose end is the arrangement end).
fn next_scene_event_beat(arrangement: &ProjectArrangement, beat: f64) -> Option<f64> {
    arrangement
        .scene_lane
        .iter()
        .find(|event| event.start_beat > beat)
        .map(|event| event.start_beat)
}

/// Lower one finished arrangement gesture into arrangement primitive
/// commands. Every gesture maps to exactly one primitive (spec 9.1) except
/// `:delete-items`, which removes one scene change per selected id — each
/// removal is its own primitive and undo entry. Returns `Ok(vec![])` for
/// view-only actions this module does not own.
pub(crate) fn arrangement_action_song_commands(
    action: &Value,
    song: Option<&ProjectSong>,
    arrangement: Option<&ProjectArrangement>,
) -> Result<Vec<SongCommand>, String> {
    let map = action_map(action)?;
    let Some(kind) = action_type(map) else {
        return Err("action is missing :type".to_string());
    };
    match kind.as_str() {
        // Scene-lane span drag: the ghost's final start moves the scene
        // change the dragged row starts on.
        "finish-move-items" => {
            let row_id = require_row_id(map, "row-id")?;
            let start_beat = require_beat(map, "start")?;
            let song = song.ok_or_else(|| "no committed song".to_string())?;
            let from_beat = row_start_beat(song, row_id)?;
            Ok(vec![(
                "arrangement-scene-move",
                payload(vec![
                    ("from-beat", Value::Number(from_beat)),
                    ("to-beat", Value::Number(start_beat)),
                ]),
            )])
        }
        // Scene-lane end-edge resize: a scene span ends where the NEXT scene
        // change starts, so the gesture moves that change; resizing the last
        // span's end edge edits the song end instead. (Track CLIPS have real
        // ends now and resize themselves — lane spec 12.)
        "finish-resize-items" => {
            let row_id = require_row_id(map, "row-id")?;
            let end_beat = require_beat(map, "end")?;
            let song = song.ok_or_else(|| "no committed song".to_string())?;
            let arrangement = arrangement.ok_or_else(|| "no arrangement".to_string())?;
            let from_beat = row_start_beat(song, row_id)?;
            match next_scene_event_beat(arrangement, from_beat) {
                Some(next) => Ok(vec![(
                    "arrangement-scene-move",
                    payload(vec![
                        ("from-beat", Value::Number(next)),
                        ("to-beat", Value::Number(end_beat)),
                    ]),
                )]),
                None => Ok(vec![(
                    "song-set-end",
                    payload(vec![("end-beat", Value::Number(end_beat))]),
                )]),
            }
        }
        // Create (double-click draw or scene drop): insert a scene change
        // launching the chosen scene. A create beyond the committed song end
        // first extends the end to the gesture's :end (DAW convention:
        // dropping past the end grows the arrangement) — that gesture is two
        // primitives and therefore two undo entries.
        "finish-create-item" => {
            let start_beat = require_beat(map, "start")?;
            let scene = require_scene(map)?;
            let mut commands = Vec::new();
            if let Some(song) = song {
                if start_beat >= song.end_beat {
                    let end_beat = match map_field(map, "end") {
                        Some(Value::Number(end)) if end.is_finite() && end > start_beat => end,
                        _ => {
                            return Err(format!(
                                "create at beat {start_beat} is beyond the song end \
                                 {} and carries no :end to extend to",
                                song.end_beat
                            ));
                        }
                    };
                    commands.push((
                        "song-set-end",
                        payload(vec![("end-beat", Value::Number(end_beat))]),
                    ));
                }
            }
            commands.push((
                "arrangement-scene-insert",
                payload(vec![
                    ("beat", Value::Number(start_beat)),
                    ("scene", Value::Number(scene)),
                ]),
            ));
            Ok(commands)
        }
        // Scene-lane erase / delete: one removal primitive (and one undo
        // entry) per id. Removing a scene change merges its span into the
        // predecessor and can never touch a clip (lane spec 8).
        "delete-items" => {
            let Some(Value::List(ids)) = map_field(map, "ids") else {
                return Err("delete-items is missing :ids".to_string());
            };
            let song = song.ok_or_else(|| "no committed song".to_string())?;
            ids.iter()
                .map(|id| {
                    let id = match &*id.borrow() {
                        Value::Number(value)
                            if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
                        {
                            *value as u64
                        }
                        other => {
                            return Err(format!(
                                "delete-items id must be a row id, got {other:?}"
                            ));
                        }
                    };
                    let beat = row_start_beat(song, id)?;
                    Ok((
                        "arrangement-scene-remove",
                        payload(vec![("beat", Value::Number(beat))]),
                    ))
                })
                .collect()
        }
        // Track-lane clip edge drag: ONE clip resize (lane spec 12 — no more
        // "resize = move the next row"). The gesture carries the span it drew
        // on so the host command can resolve the clip it names.
        "clip-resize" => {
            let track = require_row_id(map, "track")?;
            let at_beat = require_beat(map, "at-beat")?;
            let at_end = require_beat(map, "at-end")?;
            let start_beat = require_beat(map, "start")?;
            let end_beat = require_beat(map, "end")?;
            Ok(vec![(
                "arrangement-clip-resize",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("at-beat", Value::Number(at_beat)),
                    ("at-end", Value::Number(at_end)),
                    ("start-beat", Value::Number(start_beat)),
                    ("end-beat", Value::Number(end_beat)),
                ]),
            )])
        }
        // Track-lane whole-clip drag: one rigid clip move (takes spec 7.4).
        "clip-move" => {
            let track = require_row_id(map, "track")?;
            let at_beat = require_beat(map, "at-beat")?;
            let at_end = require_beat(map, "at-end")?;
            let start_beat = require_beat(map, "start")?;
            Ok(vec![(
                "arrangement-clip-move",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("at-beat", Value::Number(at_beat)),
                    ("at-end", Value::Number(at_end)),
                    ("start-beat", Value::Number(start_beat)),
                ]),
            )])
        }
        // Track-lane Backspace: one clip delete. The lane rejoins the scene
        // backdrop over the deleted span (lane spec 6.2).
        "clip-delete" => {
            let track = require_row_id(map, "track")?;
            let at_beat = require_beat(map, "at-beat")?;
            let at_end = require_beat(map, "at-end")?;
            Ok(vec![(
                "arrangement-clip-delete",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("at-beat", Value::Number(at_beat)),
                    ("at-end", Value::Number(at_end)),
                ]),
            )])
        }
        // Content-length handle release: one song-set-end (spec 9.3).
        "finish-resize-content-length" => {
            let end_beat = require_beat(map, "length")?;
            Ok(vec![(
                "song-set-end",
                payload(vec![("end-beat", Value::Number(end_beat))]),
            )])
        }
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use sequencer::sequencer::{ArrClip, ClipId, ProjectSongRow, SceneEvent};

    fn value_map(fields: Vec<(&str, Value)>) -> Value {
        payload(fields)
    }

    fn value_list(items: Vec<Value>) -> Value {
        Value::List(items.into_iter().map(|item| Rc::new(RefCell::new(item))).collect())
    }

    /// The compiled song the scene-lane gestures address: three scene events
    /// at beats 0/16/32, end 48. Row ids are deliberately NOT positional so a
    /// translation that confused an id with an index would be caught.
    fn song() -> ProjectSong {
        let row = |id: u64, start_beat: f64, scene: usize| ProjectSongRow {
            id: SongRowId(id),
            start_beat,
            scene,
            overrides: Vec::new(),
        };
        ProjectSong {
            rows: vec![row(7, 0.0, 0), row(3, 16.0, 1), row(9, 32.0, 2)],
            end_beat: 48.0,
            loop_enabled: false,
            next_row_id: 10,
        }
    }

    /// The arrangement that song compiles from, plus one clip so the clip
    /// gestures have something to name.
    fn arrangement() -> ProjectArrangement {
        ProjectArrangement {
            scene_lane: vec![
                SceneEvent { start_beat: 0.0, scene: 0 },
                SceneEvent { start_beat: 16.0, scene: 1 },
                SceneEvent { start_beat: 32.0, scene: 2 },
            ],
            track_lanes: vec![vec![ArrClip::new(ClipId(0), 4.0, 12.0, Some(2))], Vec::new()],
            end_beat: 48.0,
            loop_enabled: false,
            next_clip_id: 1,
        }
    }

    fn lower(action: &Value) -> Result<Vec<SongCommand>, String> {
        arrangement_action_song_commands(action, Some(&song()), Some(&arrangement()))
    }

    fn payload_number(command: &SongCommand, key: &str) -> f64 {
        let Value::Map(map) = &command.1 else {
            panic!("payload must be a map");
        };
        match &*map[key].borrow() {
            Value::Number(value) => *value,
            other => panic!("payload :{key} must be a number, got {other:?}"),
        }
    }

    /// Scene-lane move: the dragged row names the scene change by its start
    /// beat, which is how the beat-addressed primitive takes it.
    #[test]
    fn move_gesture_lowers_to_one_scene_move() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-move-items".to_string())),
            ("row-id", Value::Number(3.0)),
            ("start", Value::Number(12.5)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1, "one gesture -> one primitive -> one undo entry");
        assert_eq!(commands[0].0, "arrangement-scene-move");
        assert_eq!(payload_number(&commands[0], "from-beat"), 16.0);
        assert_eq!(payload_number(&commands[0], "to-beat"), 12.5);
    }

    /// A scene span ends where the NEXT scene change starts, so its end-edge
    /// drag moves that change. (Track clips resize themselves now — see
    /// `clip_resize_lowers_to_one_clip_resize`.)
    #[test]
    fn scene_resize_gesture_moves_the_next_scene_change() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(7.0)),
            ("end", Value::Number(20.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-scene-move");
        // Row 7 starts at beat 0; the next scene change is the one at 16.
        assert_eq!(payload_number(&commands[0], "from-beat"), 16.0);
        assert_eq!(payload_number(&commands[0], "to-beat"), 20.0);
    }

    #[test]
    fn resizing_the_last_scene_span_edits_the_song_end() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(9.0)),
            ("end", Value::Number(40.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 40.0);
    }

    #[test]
    fn draw_gesture_inserts_a_scene_change_for_the_chosen_scene() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(24.0)),
            ("scene", Value::Number(2.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-scene-insert");
        assert_eq!(payload_number(&commands[0], "beat"), 24.0);
        assert_eq!(payload_number(&commands[0], "scene"), 2.0);
    }

    #[test]
    fn create_beyond_the_song_end_extends_the_end_first() {
        // Song ends at 48; a drop at beat 64 must extend before inserting.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(64.0)),
            ("end", Value::Number(80.0)),
            ("scene", Value::Number(1.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 80.0);
        assert_eq!(commands[1].0, "arrangement-scene-insert");
        assert_eq!(payload_number(&commands[1], "beat"), 64.0);

        // Beyond the end without an :end to extend to is an error, never a
        // silently-rejected insert.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(64.0)),
            ("scene", Value::Number(1.0)),
        ]);
        assert!(lower(&action).is_err());

        // With no committed song at all the insert stands alone; the
        // primitive itself reports there is nothing to insert into.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(0.0)),
            ("end", Value::Number(16.0)),
            ("scene", Value::Number(0.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, None, None).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-scene-insert");
    }

    #[test]
    fn delete_lowers_to_one_scene_removal_per_id() {
        let action = value_map(vec![
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                value_list(vec![Value::Number(3.0), Value::Number(9.0)]),
            ),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(commands
            .iter()
            .all(|command| command.0 == "arrangement-scene-remove"));
        assert_eq!(payload_number(&commands[0], "beat"), 16.0);
        assert_eq!(payload_number(&commands[1], "beat"), 32.0);
    }

    /// Lane spec 12: a clip edge drag is ONE clip resize, carrying the span
    /// the view drew on so the host command can resolve the stored clip.
    #[test]
    fn clip_resize_lowers_to_one_clip_resize() {
        let action = value_map(vec![
            ("type", Value::Keyword("clip-resize".to_string())),
            ("track", Value::Number(0.0)),
            ("at-beat", Value::Number(4.0)),
            ("at-end", Value::Number(12.0)),
            ("start", Value::Number(4.0)),
            ("end", Value::Number(20.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1, "one gesture -> one primitive");
        assert_eq!(commands[0].0, "arrangement-clip-resize");
        assert_eq!(payload_number(&commands[0], "track"), 0.0);
        assert_eq!(payload_number(&commands[0], "at-beat"), 4.0);
        assert_eq!(payload_number(&commands[0], "at-end"), 12.0);
        assert_eq!(payload_number(&commands[0], "start-beat"), 4.0);
        assert_eq!(payload_number(&commands[0], "end-beat"), 20.0);
    }

    #[test]
    fn clip_delete_and_move_lower_to_one_primitive_each() {
        let action = value_map(vec![
            ("type", Value::Keyword("clip-delete".to_string())),
            ("track", Value::Number(1.0)),
            ("at-beat", Value::Number(8.0)),
            ("at-end", Value::Number(16.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-clip-delete");
        assert_eq!(payload_number(&commands[0], "track"), 1.0);
        assert_eq!(payload_number(&commands[0], "at-beat"), 8.0);

        let action = value_map(vec![
            ("type", Value::Keyword("clip-move".to_string())),
            ("track", Value::Number(0.0)),
            ("at-beat", Value::Number(4.0)),
            ("at-end", Value::Number(12.0)),
            ("start", Value::Number(24.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-clip-move");
        assert_eq!(payload_number(&commands[0], "start-beat"), 24.0);
    }

    #[test]
    fn content_length_release_lowers_to_song_set_end() {
        let action = value_map(vec![
            (
                "type",
                Value::Keyword("finish-resize-content-length".to_string()),
            ),
            ("length", Value::Number(64.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 64.0);
    }

    #[test]
    fn malformed_and_unknown_actions_are_rejected_or_ignored() {
        // Unknown row id in a resize: rejected against the committed song, so
        // a stale gesture can never be mistaken for last-span semantics and
        // silently edit the song end.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(42.0)),
            ("end", Value::Number(20.0)),
        ]);
        assert!(lower(&action).is_err(), "unknown row id must not silently set-end");

        // Missing fields are errors.
        let action = value_map(vec![(
            "type",
            Value::Keyword("finish-move-items".to_string()),
        )]);
        assert!(lower(&action).is_err());

        // View-only actions produce no commands.
        let action = value_map(vec![("type", Value::Keyword("set-cursor".to_string()))]);
        assert!(lower(&action).unwrap().is_empty());
    }
}
