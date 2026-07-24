//! Arrangement-timeline gesture translation
//! (docs/arrangement-timeline-ui-spec.md 9): the one seam that lowers a
//! finished scene-lane gesture into song-mode editing primitives. Live drag
//! actions never reach this module — the Lisp view keeps them as ghost
//! preview state and forwards only the terminal action, augmented with the
//! ghost's final values. Each returned command is one validated, atomic,
//! one-undo-entry song primitive (song-mode-spec 5.6); this module never
//! mutates anything itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eseqlisp::vm::Value;

use sequencer::sequencer::{ProjectSong, SongRowId};

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

/// The row after `row_id` in start-beat order: `Ok(Some)` when a next row
/// exists, `Ok(None)` when `row_id` is the last row, `Err` when the id is
/// not in the song at all (a stale gesture must not silently edit the end).
fn next_row_id(song: &ProjectSong, row_id: u64) -> Result<Option<SongRowId>, String> {
    let index = song
        .rows
        .iter()
        .position(|row| row.id == SongRowId(row_id))
        .ok_or_else(|| format!("song has no row with id {row_id}"))?;
    Ok(song.rows.get(index + 1).map(|row| row.id))
}

/// Lower one finished arrangement gesture into song primitive commands.
/// Every gesture maps to exactly one primitive (spec 9.1) except
/// `:delete-items`, which removes one row per selected id — each removal is
/// its own primitive and undo entry. Returns `Ok(vec![])` for view-only
/// actions this module does not own.
pub(crate) fn arrangement_action_song_commands(
    action: &Value,
    song: Option<&ProjectSong>,
) -> Result<Vec<SongCommand>, String> {
    let map = action_map(action)?;
    let Some(kind) = action_type(map) else {
        return Err("action is missing :type".to_string());
    };
    match kind.as_str() {
        // Span drag: the ghost's final start moves the row itself.
        "finish-move-items" => {
            let row_id = require_row_id(map, "row-id")?;
            let start_beat = require_beat(map, "start")?;
            Ok(vec![(
                "song-row-move",
                payload(vec![
                    ("row-id", Value::Number(row_id as f64)),
                    ("start-beat", Value::Number(start_beat)),
                ]),
            )])
        }
        // End-edge resize: a span ends where the NEXT row starts, so the
        // gesture moves that row (spec 9.1); resizing the last row's end
        // edge edits the song end instead.
        "finish-resize-items" => {
            let row_id = require_row_id(map, "row-id")?;
            let end_beat = require_beat(map, "end")?;
            let song = song.ok_or_else(|| "no committed song".to_string())?;
            match next_row_id(song, row_id)? {
                Some(next) => Ok(vec![(
                    "song-row-move",
                    payload(vec![
                        ("row-id", Value::Number(next.0 as f64)),
                        ("start-beat", Value::Number(end_beat)),
                    ]),
                )]),
                None => Ok(vec![(
                    "song-set-end",
                    payload(vec![("end-beat", Value::Number(end_beat))]),
                )]),
            }
        }
        // Create (double-click draw or scene drop): insert a row launching
        // the chosen scene. A create beyond the committed song end first
        // extends the end to the gesture's :end (DAW convention: dropping
        // past the end grows the arrangement) — that gesture is two
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
                "song-row-insert",
                payload(vec![
                    ("start-beat", Value::Number(start_beat)),
                    ("scene", Value::Number(scene)),
                    ("overrides", Value::List(vec![])),
                ]),
            ));
            Ok(commands)
        }
        // Erase / delete: one removal primitive (and one undo entry) per id.
        "delete-items" => {
            let Some(Value::List(ids)) = map_field(map, "ids") else {
                return Err("delete-items is missing :ids".to_string());
            };
            ids.iter()
                .map(|id| {
                    let id = match &*id.borrow() {
                        Value::Number(value)
                            if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
                        {
                            *value
                        }
                        other => {
                            return Err(format!(
                                "delete-items id must be a row id, got {other:?}"
                            ));
                        }
                    };
                    Ok((
                        "song-row-remove",
                        payload(vec![("row-id", Value::Number(id))]),
                    ))
                })
                .collect()
        }
        // Track-clip surgery (delete / edge-resize on a track lane): one
        // atomic song-track-paint primitive per gesture. :pattern-id nil (or
        // absent) silences the region; a pool id paints it.
        "track-paint" => {
            let track = require_row_id(map, "track")?;
            let start_beat = require_beat(map, "start")?;
            let end_beat = require_beat(map, "end")?;
            let pattern_id = match map_field(map, "pattern-id") {
                None | Some(Value::Nil) => Value::Nil,
                Some(Value::Number(id))
                    if id.is_finite() && id >= 1.0 && id.fract() == 0.0 =>
                {
                    Value::Number(id)
                }
                other => {
                    return Err(format!(
                        "track-paint :pattern-id must be a pool id or nil, got {other:?}"
                    ));
                }
            };
            let mut fields = vec![
                ("track", Value::Number(track as f64)),
                ("start-beat", Value::Number(start_beat)),
                ("end-beat", Value::Number(end_beat)),
                ("pattern-id", pattern_id),
            ];
            // Optional clip anchor (takes spec 7.4): the grow gesture
            // forwards the existing clip's anchor so the extension
            // continues the loop instead of re-anchoring at the paint start.
            if let Some(Value::Number(anchor)) = map_field(map, "anchor-beat") {
                if anchor.is_finite() {
                    fields.push(("anchor-beat", Value::Number(anchor)));
                }
            }
            if let Some(Value::Number(offset)) = map_field(map, "anchor-offset-steps") {
                if offset.is_finite() && offset >= 0.0 {
                    fields.push(("anchor-offset-steps", Value::Number(offset)));
                }
            }
            Ok(vec![("song-track-paint", payload(fields))])
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

    use sequencer::sequencer::ProjectSongRow;

    fn value_map(fields: Vec<(&str, Value)>) -> Value {
        payload(fields)
    }

    fn value_list(items: Vec<Value>) -> Value {
        Value::List(items.into_iter().map(|item| Rc::new(RefCell::new(item))).collect())
    }

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

    fn payload_number(command: &SongCommand, key: &str) -> f64 {
        let Value::Map(map) = &command.1 else {
            panic!("payload must be a map");
        };
        match &*map[key].borrow() {
            Value::Number(value) => *value,
            other => panic!("payload :{key} must be a number, got {other:?}"),
        }
    }

    #[test]
    fn move_gesture_lowers_to_one_row_move() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-move-items".to_string())),
            ("row-id", Value::Number(3.0)),
            ("start", Value::Number(12.5)),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1, "one gesture -> one primitive -> one undo entry");
        assert_eq!(commands[0].0, "song-row-move");
        assert_eq!(payload_number(&commands[0], "row-id"), 3.0);
        assert_eq!(payload_number(&commands[0], "start-beat"), 12.5);
    }

    #[test]
    fn resize_gesture_moves_the_next_row() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(7.0)),
            ("end", Value::Number(20.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-row-move");
        // Row 7 is first in start order; the row after it is id 3.
        assert_eq!(payload_number(&commands[0], "row-id"), 3.0);
        assert_eq!(payload_number(&commands[0], "start-beat"), 20.0);
    }

    #[test]
    fn resizing_the_last_row_edits_the_song_end() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(9.0)),
            ("end", Value::Number(40.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 40.0);
    }

    #[test]
    fn draw_gesture_inserts_a_row_for_the_chosen_scene() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(24.0)),
            ("scene", Value::Number(2.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-row-insert");
        assert_eq!(payload_number(&commands[0], "start-beat"), 24.0);
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
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 80.0);
        assert_eq!(commands[1].0, "song-row-insert");
        assert_eq!(payload_number(&commands[1], "start-beat"), 64.0);

        // Beyond the end without an :end to extend to is an error, never a
        // silently-rejected insert.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(64.0)),
            ("scene", Value::Number(1.0)),
        ]);
        assert!(arrangement_action_song_commands(&action, Some(&song())).is_err());

        // With no committed song the insert itself creates one (default
        // end); no extension command is prepended.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("start", Value::Number(0.0)),
            ("end", Value::Number(16.0)),
            ("scene", Value::Number(0.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, None).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-row-insert");
    }

    #[test]
    fn delete_lowers_to_one_removal_per_id() {
        let action = value_map(vec![
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                value_list(vec![Value::Number(3.0), Value::Number(9.0)]),
            ),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|command| command.0 == "song-row-remove"));
        assert_eq!(payload_number(&commands[0], "row-id"), 3.0);
        assert_eq!(payload_number(&commands[1], "row-id"), 9.0);
    }

    #[test]
    fn track_paint_lowers_to_one_song_track_paint() {
        // Silence (delete / shrink): nil pattern-id.
        let action = value_map(vec![
            ("type", Value::Keyword("track-paint".to_string())),
            ("track", Value::Number(2.0)),
            ("start", Value::Number(8.0)),
            ("end", Value::Number(16.0)),
            ("pattern-id", Value::Nil),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1, "one gesture -> one primitive");
        assert_eq!(commands[0].0, "song-track-paint");
        assert_eq!(payload_number(&commands[0], "track"), 2.0);
        assert_eq!(payload_number(&commands[0], "start-beat"), 8.0);
        assert_eq!(payload_number(&commands[0], "end-beat"), 16.0);
        let Value::Map(map) = &commands[0].1 else { panic!() };
        assert!(matches!(&*map["pattern-id"].borrow(), Value::Nil));

        // Extend: the clip's pattern id rides through.
        let action = value_map(vec![
            ("type", Value::Keyword("track-paint".to_string())),
            ("track", Value::Number(0.0)),
            ("start", Value::Number(16.0)),
            ("end", Value::Number(24.0)),
            ("pattern-id", Value::Number(3.0)),
        ]);
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(payload_number(&commands[0], "pattern-id"), 3.0);

        // Malformed pattern id is an error, not a silent silence.
        let action = value_map(vec![
            ("type", Value::Keyword("track-paint".to_string())),
            ("track", Value::Number(0.0)),
            ("start", Value::Number(0.0)),
            ("end", Value::Number(4.0)),
            ("pattern-id", Value::Number(1.5)),
        ]);
        assert!(arrangement_action_song_commands(&action, Some(&song())).is_err());
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
        let commands = arrangement_action_song_commands(&action, Some(&song())).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "song-set-end");
        assert_eq!(payload_number(&commands[0], "end-beat"), 64.0);
    }

    #[test]
    fn malformed_and_unknown_actions_are_rejected_or_ignored() {
        // Unknown row id in a resize: rejected against the committed song.
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("row-id", Value::Number(42.0)),
            ("end", Value::Number(20.0)),
        ]);
        // Row 42 does not exist: treated as last-row semantics would be
        // wrong, so next_row_id yields None only for the true last row;
        // an unknown id must error out instead.
        let result = arrangement_action_song_commands(&action, Some(&song()));
        assert!(result.is_err(), "unknown row id must not silently set-end");

        // Missing fields are errors.
        let action = value_map(vec![(
            "type",
            Value::Keyword("finish-move-items".to_string()),
        )]);
        assert!(arrangement_action_song_commands(&action, Some(&song())).is_err());

        // View-only actions produce no commands.
        let action = value_map(vec![("type", Value::Keyword("set-cursor".to_string()))]);
        assert!(arrangement_action_song_commands(&action, Some(&song()))
            .unwrap()
            .is_empty());
    }
}
