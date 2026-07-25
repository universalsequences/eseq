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
    // Declarative authoring: `def-song` lowers to the arrangement (lane
    // spec 8), not to the row primitives.
    "arrangement-replace",
    "arrangement-clear",
    // Take lifecycle (takes spec 6.4) + the Phase C region→take harness.
    "song-take-delete",
    "song-region-to-take",
    // Transport authority (docs/song-mode-spec.md 12/13): routed through the
    // state machine in app/song_transport.rs.
    "song-transport-toggle-play",
    "song-transport-play",
    "song-use-arrangement",
    "song-capture-arm",
    "song-capture-cancel",
    "song-back-to-song",
    "song-back-to-song-track",
    "song-status",
    // Sound binding (takes spec 16): timeline clip selection is the explicit
    // binding gesture, plus the two explicit propagation gestures.
    "song-select-clip",
    "song-deselect-clip",
    // Region selection (docs/arrangement-region-editing-spec.md 4.1): pure
    // selection state — no song mutation, no undo entry, and legal while
    // song editing is locked.
    "song-set-region",
    "song-clear-region",
    "song-set-arr-cursor",
    // Region copy/paste/delete (region spec 5.2). Copy and paste need the
    // clipboard handle, so all three are applied in `handle` below where the
    // loop context is in scope, not in `run`/`run_transport`.
    "song-region-copy",
    "song-region-paste",
    "song-region-delete",
    "song-region-duplicate",
    "sound-push-to-pattern",
    "sound-apply-to-all-takes",
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
            override_from_numbers(track, pattern_id, 0.0)
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
            // Optional clip start offset in pattern steps (takes spec 6.2).
            let offset_steps = map_number(map, "offset-steps").unwrap_or(0.0);
            // Optional take source (takes spec 6.2): mutually exclusive with
            // a pattern id per validation 6.3.
            if let Some(take_id) = map_number(map, "take-id") {
                if !take_id.is_finite() || take_id < 0.0 || take_id.fract() != 0.0 {
                    return Err("override take-id must be a non-negative integer".to_string());
                }
                if pattern_id.is_some_and(|id| id != 0.0) {
                    return Err(
                        "an override cannot carry both :take-id and :pattern-id".to_string()
                    );
                }
                if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
                    return Err("override track must be a non-negative integer".to_string());
                }
                if !offset_steps.is_finite() || offset_steps < 0.0 {
                    return Err(
                        "override offset-steps must be a finite, non-negative number".to_string()
                    );
                }
                return Ok(ProjectSongTrackOverride::new_take(
                    track as usize,
                    take_id as u64,
                    offset_steps,
                ));
            }
            override_from_numbers(track, pattern_id, offset_steps)
        }
        _ => Err("override entries must be (track pattern-id) pairs or maps".to_string()),
    }
}

fn override_from_numbers(
    track: f64,
    pattern_id: Option<f64>,
    offset_steps: f64,
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
    if !offset_steps.is_finite() || offset_steps < 0.0 {
        return Err("override offset-steps must be a finite, non-negative number".to_string());
    }
    Ok(ProjectSongTrackOverride {
        track: track as usize,
        pattern_id,
        take_id: None,
        offset_steps,
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
            // Optional clip anchor (takes spec 7.4): the grow gesture passes
            // the existing clip's anchor so the extension continues the loop
            // instead of re-starting it at the paint start.
            let anchor_beat = map_number(map, "anchor-beat").unwrap_or(start_beat);
            let anchor_offset_steps = map_number(map, "anchor-offset-steps").unwrap_or(0.0);
            app.song_track_paint_anchored(
                track as usize,
                start_beat,
                end_beat,
                pattern_id,
                anchor_beat,
                anchor_offset_steps,
            )?;
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
        "arrangement-replace" => {
            let map = payload_map(payload)?;
            let rows = parse_rows(map)?;
            let row_count = rows.len();
            let end_beat = require_number(map, "end-beat")?;
            let loop_enabled = map_bool(map, "loop");
            let name = map_string(map, "name");
            app.arr_replace_rows(rows, end_beat, loop_enabled)?;
            Ok(match name {
                Some(name) => format!(
                    "Committed song \"{name}\": {row_count} row(s), end beat {end_beat}"
                ),
                None => format!("Replaced song: {row_count} row(s), end beat {end_beat}"),
            })
        }
        "arrangement-clear" => {
            app.arr_clear()?;
            Ok("Cleared song".to_string())
        }
        "song-take-delete" => {
            let map = payload_map(payload)?;
            let track = require_number(map, "track")?;
            let take_id = require_number(map, "take-id")?;
            if track < 0.0 || track.fract() != 0.0 || take_id < 0.0 || take_id.fract() != 0.0 {
                return Err("track and take-id must be non-negative integers".to_string());
            }
            app.song_take_delete(track as usize, take_id as u64)?;
            Ok(format!(
                "Deleted take {} on track {}",
                take_id as u64,
                track as usize + 1
            ))
        }
        "song-region-to-take" => {
            let map = payload_map(payload)?;
            let track = require_number(map, "track")?;
            if track < 0.0 || track.fract() != 0.0 {
                return Err("track must be a non-negative integer".to_string());
            }
            let start_beat = require_number(map, "start-beat")?;
            let end_beat = require_number(map, "end-beat")?;
            let take_id = app.song_region_to_take(track as usize, start_beat, end_beat)?;
            Ok(format!(
                "Converted track {} beats {start_beat}-{end_beat} into take {}",
                track as usize + 1,
                take_id.0
            ))
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
        // Back to Song (takes spec 10): clear the manual-override latch so
        // the song resumes launch authority with anchored phase.
        "song-back-to-song" => app.back_to_song().map(Some),
        // Per-track Back to Song (takes spec 10 UX): one lane returns to the
        // song's launch authority; other latched lanes stay manual.
        "song-back-to-song-track" => {
            let map = payload_map(payload)?;
            let track = map_usize(map, "track").ok_or("missing or invalid :track")?;
            app.back_to_song_track(track).map(Some)
        }
        "song-status" => Ok(Some(song_status_summary(app))),
        // Selecting a clip re-binds the track's device panel, monitor sound
        // and record-clone template in one move (takes spec 16.2/16.6), so
        // it lives with the transport commands: it changes what is sounding.
        "song-select-clip" => {
            let map = payload_map(payload)?;
            let track = map_usize(map, "track").ok_or("missing or invalid :track")?;
            let row_id = require_row_id(map)?;
            // The timeline sends the MERGED clip's span alongside the row id
            // so the selection is also a one-clip region (region spec 4.1,
            // amended): selecting a clip lights its body and gives
            // copy/delete a target. Absent span = clear the region.
            let span = match (map_number(map, "start"), map_number(map, "end")) {
                (Some(start), Some(end)) if start.is_finite() && end.is_finite() => {
                    Some((start, end))
                }
                _ => None,
            };
            app.select_song_clip_span(track, row_id, span)?;
            Ok(app.track_binding_label(track).map(|label| format!("Bound: {label}")))
        }
        "song-deselect-clip" => {
            app.set_song_clip_selection(None);
            Ok(None)
        }
        // Region selection (region spec 4.1). It rides with the transport
        // commands because setting it releases the sound binding, i.e. it
        // changes what the device panel and monitor are pointed at.
        "song-set-region" => {
            let map = payload_map(payload)?;
            let track_a = map_usize(map, "track-a").ok_or("missing or invalid :track-a")?;
            let track_b = map_usize(map, "track-b").ok_or("missing or invalid :track-b")?;
            let start = require_number(map, "start")?;
            let end = require_number(map, "end")?;
            if !start.is_finite() || !end.is_finite() {
                return Err("region bounds must be finite".to_string());
            }
            app.set_song_region(app::song_region::SongRegionSelection::new(
                track_a, track_b, start, end,
            ));
            Ok(None)
        }
        "song-clear-region" => {
            app.clear_song_region();
            Ok(None)
        }
        // Arrangement edit-cursor mirror (region spec 5.3): the paste target
        // for the Rust-side Cmd-V seam. Pure state, no undo entry.
        "song-set-arr-cursor" => {
            let map = payload_map(payload)?;
            let beat = require_number(map, "time")?;
            let track = map_number(map, "track").unwrap_or(-1.0);
            app.set_arrangement_cursor(beat, track as isize);
            Ok(None)
        }
        "sound-push-to-pattern" => {
            let map = payload_map(payload)?;
            let track = map_usize(map, "track").ok_or("missing or invalid :track")?;
            app.push_bound_sound_to_pattern(track).map(Some)
        }
        "sound-apply-to-all-takes" => {
            let map = payload_map(payload)?;
            let track = map_usize(map, "track").ok_or("missing or invalid :track")?;
            app.apply_bound_sound_to_all_takes(track).map(Some)
        }
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
    // Region clipboard commands need the loop context's clipboard handle and
    // are unavailable headlessly, like the transport commands.
    if !COMMANDS.contains(&name)
        || TRANSPORT_COMMANDS.contains(&name)
        || REGION_CLIPBOARD_COMMANDS.contains(&name)
    {
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
    "song-back-to-song",
    "song-back-to-song-track",
    "song-status",
    "song-select-clip",
    "song-deselect-clip",
    "song-set-region",
    "song-clear-region",
    "song-set-arr-cursor",
    "sound-push-to-pattern",
    "sound-apply-to-all-takes",
];

/// Region clipboard commands (region spec 5.2/5.3). They live here rather
/// than in `run` because copy and paste need the shared clipboard handle —
/// the same reason the piano-roll clipboard commands sit in the loop-context
/// layer. Each mutating one is a single primitive call, so one undo entry.
fn run_region_clipboard(
    name: &str,
    payload: &Value,
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
) -> Result<Option<String>, String> {
    let clipboard = ctx.shared.arrangement_clipboard.clone();
    match name {
        "song-region-copy" => {
            let copied = app.song_region_copy()?;
            let spans = copied.span_count();
            let tracks = copied.tracks.len();
            *clipboard.lock().unwrap() = Some(copied);
            Ok(Some(format!(
                "Copied {spans} clip{} across {tracks} track{}",
                if spans == 1 { "" } else { "s" },
                if tracks == 1 { "" } else { "s" },
            )))
        }
        "song-region-paste" => {
            let stored = clipboard.lock().unwrap().clone();
            let Some(stored) = stored else {
                return Err("The arrangement clipboard is empty".to_string());
            };
            // The widget's :paste-items carries its own time; the keyboard
            // seam falls back to the mirrored arrangement cursor.
            let dest = match payload {
                Value::Map(map) => map_number(map, "time"),
                _ => None,
            }
            .unwrap_or(app.arrangement_cursor_beat);
            app.song_region_paste(&stored, dest).map(Some)
        }
        "song-region-delete" => app.song_region_delete().map(Some),
        // Duplicate = copy + ripple insert after the region (Ableton's
        // Duplicate Time). It reads the region itself, so no clipboard.
        "song-region-duplicate" => app.song_region_duplicate().map(Some),
        _ => Err(format!("unknown region clipboard command: {name}")),
    }
}

const REGION_CLIPBOARD_COMMANDS: &[&str] = &[
    "song-region-copy",
    "song-region-paste",
    "song-region-delete",
    "song-region-duplicate",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    if REGION_CLIPBOARD_COMMANDS.contains(&name) {
        match run_region_clipboard(name, &payload, app, ctx) {
            Ok(Some(status)) => {
                app.song_edit_error = None;
                editor.handle_host_event(HostEvent::Status(status));
            }
            Ok(None) => {}
            Err(error) => {
                app.song_edit_error = Some(error.clone());
                editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}")));
            }
        }
        return;
    }
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
        Ok(status) => {
            // A successful edit clears the latched rejection so the
            // arrangement banner disappears.
            app.song_edit_error = None;
            editor.handle_host_event(HostEvent::Status(status));
        }
        Err(error) => {
            // Latch the rejection for SEQ.song-edit-error: the step tile
            // hides the status line, so the arrangement view surfaces it.
            app.song_edit_error = Some(error.clone());
            editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}")));
        }
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
            ProjectSongTrackOverride::new(1, Some(3))
        );
        assert_eq!(
            parse_override(&value_map(vec![
                ("track", Value::Number(2.0)),
                ("pattern-id", Value::Number(5.0)),
            ]))
            .unwrap(),
            ProjectSongTrackOverride::new(2, Some(5))
        );
        assert!(parse_override(&pair(-1.0, 3.0)).is_err());
        // Pattern id 0 and nil both mean explicit-empty (the track plays
        // nothing for the row).
        assert_eq!(
            parse_override(&pair(1.0, 0.0)).unwrap(),
            ProjectSongTrackOverride::new(1, None)
        );
        assert_eq!(
            parse_override(&Value::List(vec![
                Rc::new(RefCell::new(Value::Number(1.0))),
                Rc::new(RefCell::new(Value::Nil)),
            ]))
            .unwrap(),
            ProjectSongTrackOverride::new(1, None)
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
            vec![ProjectSongTrackOverride::new(0, Some(3))]
        );

        let missing_scene = value_map(vec![("start-beat", Value::Number(0.0))]);
        assert!(parse_row_spec(&missing_scene).is_err());
    }
}
