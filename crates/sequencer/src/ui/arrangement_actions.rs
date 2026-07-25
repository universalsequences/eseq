//! Arrangement-timeline gesture translation
//! (docs/arrangement-timeline-ui-spec.md 9): the one seam that lowers a
//! finished timeline gesture into arrangement editing primitives
//! (docs/arrangement-lane-model-spec.md 8). Live drag actions never reach
//! this module — the Lisp view keeps them as ghost preview state and forwards
//! only the terminal action, augmented with the ghost's final values. Each
//! returned command is one validated, atomic, one-undo-entry primitive; this
//! module never mutates anything itself.
//!
//! Gestures address REAL model objects (lane spec 12, phase 5): a scene-lane
//! item is a scene EVENT, named by its start beat, and a track-lane item is a
//! stored CLIP, named by its `clip-id`. Nothing here resolves a span into an
//! object any more, so a gesture can never land on the wrong one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eseqlisp::vm::Value;

use sequencer::sequencer::{ProjectArrangement, ProjectSong};

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

/// A non-negative integer id field (`:clip-id`, `:track`).
fn require_id(
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

/// Validate that a scene-lane gesture named a scene event that really
/// exists. `Err` when it does not — a stale gesture must never silently edit
/// a different part of the timeline.
fn require_scene_event(arrangement: &ProjectArrangement, beat: f64) -> Result<f64, String> {
    arrangement
        .scene_lane
        .iter()
        .find(|event| event.start_beat == beat)
        .map(|event| event.start_beat)
        .ok_or_else(|| format!("the arrangement has no scene change at beat {beat}"))
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
        // event the dragged span belongs to. The span IS the event, so the
        // gesture carries its start beat directly.
        "finish-move-items" => {
            let from_beat = require_beat(map, "from-beat")?;
            let start_beat = require_beat(map, "start")?;
            let arrangement = arrangement.ok_or_else(|| "no arrangement".to_string())?;
            let from_beat = require_scene_event(arrangement, from_beat)?;
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
            let from_beat = require_beat(map, "from-beat")?;
            let end_beat = require_beat(map, "end")?;
            let arrangement = arrangement.ok_or_else(|| "no arrangement".to_string())?;
            let from_beat = require_scene_event(arrangement, from_beat)?;
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
        // entry) per id. The ids ARE the scene events' start beats, so a span
        // that exists only because some track's clip edge landed there is no
        // longer even representable. Removing a scene change merges its span
        // into the predecessor and can never touch a clip (lane spec 8).
        "delete-items" => {
            let Some(Value::List(ids)) = map_field(map, "ids") else {
                return Err("delete-items is missing :ids".to_string());
            };
            let arrangement = arrangement.ok_or_else(|| "no arrangement".to_string())?;
            ids.iter()
                .map(|id| {
                    let beat = match &*id.borrow() {
                        Value::Number(value) if value.is_finite() => *value,
                        other => {
                            return Err(format!(
                                "delete-items id must be a scene-event beat, got {other:?}"
                            ));
                        }
                    };
                    let beat = require_scene_event(arrangement, beat)?;
                    Ok((
                        "arrangement-scene-remove",
                        payload(vec![("beat", Value::Number(beat))]),
                    ))
                })
                .collect()
        }
        // Track-lane clip edge drag: ONE clip resize (lane spec 12 — no more
        // "resize = move the next row"), naming the stored clip by id.
        "clip-resize" => {
            let track = require_id(map, "track")?;
            let clip_id = require_id(map, "clip-id")?;
            let start_beat = require_beat(map, "start")?;
            let end_beat = require_beat(map, "end")?;
            Ok(vec![(
                "arrangement-clip-resize",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("start-beat", Value::Number(start_beat)),
                    ("end-beat", Value::Number(end_beat)),
                ]),
            )])
        }
        // Track-lane whole-clip drag: one rigid clip move (takes spec 7.4).
        "clip-move" => {
            let track = require_id(map, "track")?;
            let clip_id = require_id(map, "clip-id")?;
            let start_beat = require_beat(map, "start")?;
            Ok(vec![(
                "arrangement-clip-move",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("start-beat", Value::Number(start_beat)),
                ]),
            )])
        }
        // Track-lane Backspace: one clip delete. The lane rejoins the scene
        // backdrop over the deleted span (lane spec 6.2).
        "clip-delete" => {
            let track = require_id(map, "track")?;
            let clip_id = require_id(map, "clip-id")?;
            Ok(vec![(
                "arrangement-clip-delete",
                payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("clip-id", Value::Number(clip_id as f64)),
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

    use sequencer::sequencer::{ArrClip, ClipId, ProjectSongRow, SceneEvent, SongRowId};

    fn value_map(fields: Vec<(&str, Value)>) -> Value {
        payload(fields)
    }

    fn value_list(items: Vec<Value>) -> Value {
        Value::List(items.into_iter().map(|item| Rc::new(RefCell::new(item))).collect())
    }

    /// The compiled song, used only for the song-end checks now that scene
    /// gestures address the arrangement's own events.
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

    /// Scene events at 0/16/32 plus one clip, whose id is deliberately not
    /// its lane index so a translation confusing the two would be caught.
    fn arrangement() -> ProjectArrangement {
        ProjectArrangement {
            scene_lane: vec![
                SceneEvent { start_beat: 0.0, scene: 0 },
                SceneEvent { start_beat: 16.0, scene: 1 },
                SceneEvent { start_beat: 32.0, scene: 2 },
            ],
            track_lanes: vec![vec![ArrClip::new(ClipId(5), 4.0, 12.0, Some(2))], Vec::new()],
            end_beat: 48.0,
            loop_enabled: false,
            next_clip_id: 6,
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

    /// Scene-lane move: the dragged span names its scene event by start beat,
    /// which is exactly what the beat-addressed primitive takes.
    #[test]
    fn move_gesture_lowers_to_one_scene_move() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-move-items".to_string())),
            ("from-beat", Value::Number(16.0)),
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
            ("from-beat", Value::Number(0.0)),
            ("end", Value::Number(20.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-scene-move");
        // The dragged span starts at 0; the next scene change is at 16.
        assert_eq!(payload_number(&commands[0], "from-beat"), 16.0);
        assert_eq!(payload_number(&commands[0], "to-beat"), 20.0);
    }

    #[test]
    fn resizing_the_last_scene_span_edits_the_song_end() {
        let action = value_map(vec![
            ("type", Value::Keyword("finish-resize-items".to_string())),
            ("from-beat", Value::Number(32.0)),
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
                value_list(vec![Value::Number(16.0), Value::Number(32.0)]),
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

    /// A beat with no scene event on it is not addressable at all now — the
    /// spans the view draws are exactly the events, so this can only be a
    /// stale gesture, and it must be refused rather than snapped somewhere.
    #[test]
    fn scene_gestures_reject_a_beat_with_no_scene_event() {
        for kind in ["finish-move-items", "finish-resize-items"] {
            let action = value_map(vec![
                ("type", Value::Keyword(kind.to_string())),
                ("from-beat", Value::Number(24.0)),
                ("start", Value::Number(8.0)),
                ("end", Value::Number(28.0)),
            ]);
            let error = lower(&action).expect_err("no scene change at beat 24");
            assert!(error.contains("no scene change at beat 24"), "{error}");
        }
        let action = value_map(vec![
            ("type", Value::Keyword("delete-items".to_string())),
            ("ids", value_list(vec![Value::Number(24.0)])),
        ]);
        assert!(lower(&action).is_err());
    }

    /// Lane spec 12: a clip edge drag is ONE clip resize, naming the stored
    /// clip by its id.
    #[test]
    fn clip_resize_lowers_to_one_clip_resize() {
        let action = value_map(vec![
            ("type", Value::Keyword("clip-resize".to_string())),
            ("track", Value::Number(0.0)),
            ("clip-id", Value::Number(5.0)),
            ("start", Value::Number(4.0)),
            ("end", Value::Number(20.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1, "one gesture -> one primitive");
        assert_eq!(commands[0].0, "arrangement-clip-resize");
        assert_eq!(payload_number(&commands[0], "track"), 0.0);
        assert_eq!(payload_number(&commands[0], "clip-id"), 5.0);
        assert_eq!(payload_number(&commands[0], "start-beat"), 4.0);
        assert_eq!(payload_number(&commands[0], "end-beat"), 20.0);
    }

    #[test]
    fn clip_delete_and_move_lower_to_one_primitive_each() {
        let action = value_map(vec![
            ("type", Value::Keyword("clip-delete".to_string())),
            ("track", Value::Number(1.0)),
            ("clip-id", Value::Number(5.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-clip-delete");
        assert_eq!(payload_number(&commands[0], "track"), 1.0);
        assert_eq!(payload_number(&commands[0], "clip-id"), 5.0);

        let action = value_map(vec![
            ("type", Value::Keyword("clip-move".to_string())),
            ("track", Value::Number(0.0)),
            ("clip-id", Value::Number(5.0)),
            ("start", Value::Number(24.0)),
        ]);
        let commands = lower(&action).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "arrangement-clip-move");
        assert_eq!(payload_number(&commands[0], "clip-id"), 5.0);
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
        // Missing fields are errors.
        let action = value_map(vec![(
            "type",
            Value::Keyword("finish-move-items".to_string()),
        )]);
        assert!(lower(&action).is_err());

        // A clip gesture with no id is an error, never a positional guess.
        let action = value_map(vec![
            ("type", Value::Keyword("clip-delete".to_string())),
            ("track", Value::Number(0.0)),
        ]);
        assert!(lower(&action).is_err());

        // View-only actions produce no commands.
        let action = value_map(vec![("type", Value::Keyword("set-cursor".to_string()))]);
        assert!(lower(&action).unwrap().is_empty());
    }
}
