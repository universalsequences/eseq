//! Host commands wrapping the song-mode editing primitives
//! (docs/song-mode-spec.md 5.6/12). Each command routes to one `App` method
//! in `sequencer::app::song_edit`, which validates, applies atomically, and
//! commits exactly one undo entry; success and failure surface on the status
//! line.

use crate::*;

use sequencer::sequencer::{
    ClipId, LaneSource, PatternId, ProjectSongTrackOverride, TakeId,
};
use sequencer::app::song_edit::SongRowSpec;

pub(super) const COMMANDS: &[&str] = &[
    // Arrangement editing primitives (lane spec 8). Scene-lane ops address
    // scene changes by beat; clip ops address a clip by id, or by the
    // (track, beat) a timeline gesture drew on.
    "arrangement-scene-insert",
    "arrangement-scene-move",
    "arrangement-scene-set",
    "arrangement-scene-remove",
    "arrangement-clip-create",
    "arrangement-clip-delete",
    "arrangement-clip-move",
    "arrangement-clip-resize",
    "arrangement-clip-split",
    "arrangement-clip-set-source",
    "song-set-end",
    "song-set-loop",
    // Row path: the declarative/capture commit surface only (lane spec 9 —
    // capture still stages rows).
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

fn require_track(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<usize, String> {
    let track = require_number(map, "track")?;
    if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
        return Err("track must be a non-negative integer".to_string());
    }
    Ok(track as usize)
}

/// A clip source from `{pattern-id, take-id}`: a positive pool id, a take id,
/// or nil/0/absent for an explicit-empty clip (deliberate silence that still
/// occludes the scene backdrop, lane spec 6.2).
fn parse_source(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<LaneSource, String> {
    if let Some(take_id) = map_number(map, "take-id") {
        if !take_id.is_finite() || take_id < 0.0 || take_id.fract() != 0.0 {
            return Err("take-id must be a non-negative integer".to_string());
        }
        return Ok(LaneSource::Take(TakeId(take_id as u64)));
    }
    match map.get("pattern-id").map(|cell| cell.borrow().clone()) {
        None | Some(Value::Nil) => Ok(LaneSource::Empty),
        Some(Value::Number(id)) if id == 0.0 => Ok(LaneSource::Empty),
        Some(Value::Number(id)) if id.is_finite() && id >= 1.0 && id.fract() == 0.0 => {
            Ok(LaneSource::Pattern(PatternId(id as u64)))
        }
        _ => Err("pattern-id must be a positive integer, or 0/nil for silence".to_string()),
    }
}

/// Address a clip by its stable id. The lane read surface publishes stored
/// clip ids (lane spec 12), so every gesture names the object it edits —
/// there is no positional fallback to be stale about.
fn resolve_clip(map: &HashMap<String, Rc<RefCell<Value>>>) -> Result<ClipId, String> {
    let clip_id = require_number(map, "clip-id")?;
    if !clip_id.is_finite() || clip_id < 0.0 || clip_id.fract() != 0.0 {
        return Err("clip-id must be a non-negative integer".to_string());
    }
    Ok(ClipId(clip_id as u64))
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
        // --- scene lane (lane spec 8) --------------------------------
        "arrangement-scene-insert" => {
            let map = payload_map(payload)?;
            let beat = require_number(map, "beat")?;
            let scene = require_scene(map)?;
            app.arr_scene_event_insert(beat, scene)?;
            Ok(format!(
                "Inserted scene {} at beat {beat}",
                scene + 1
            ))
        }
        "arrangement-scene-move" => {
            let map = payload_map(payload)?;
            let from_beat = require_number(map, "from-beat")?;
            let to_beat = require_number(map, "to-beat")?;
            app.arr_scene_event_move(from_beat, to_beat)?;
            Ok(format!("Moved the scene change to beat {to_beat}"))
        }
        "arrangement-scene-set" => {
            let map = payload_map(payload)?;
            let beat = require_number(map, "beat")?;
            let scene = require_scene(map)?;
            app.arr_scene_event_set(beat, scene)?;
            Ok(format!(
                "Set the scene change at beat {beat} to scene {}",
                scene + 1
            ))
        }
        "arrangement-scene-remove" => {
            let map = payload_map(payload)?;
            let beat = require_number(map, "beat")?;
            app.arr_scene_event_remove(beat)?;
            Ok(format!("Removed the scene change at beat {beat}"))
        }
        // --- track lanes (lane spec 8) -------------------------------
        "arrangement-clip-create" => {
            let map = payload_map(payload)?;
            let track = require_track(map)?;
            let start_beat = require_number(map, "start-beat")?;
            let end_beat = require_number(map, "end-beat")?;
            let source = parse_source(map)?;
            let offset_steps = map_number(map, "offset-steps").unwrap_or(0.0);
            app.arr_clip_create(track, start_beat, end_beat, source, offset_steps)?;
            Ok(format!(
                "Created a clip on track {} over beats {start_beat}-{end_beat}",
                track + 1
            ))
        }
        "arrangement-clip-delete" => {
            let map = payload_map(payload)?;
            let clip_id = resolve_clip(map)?;
            app.arr_clip_delete(clip_id)?;
            Ok(format!("Deleted clip {}", clip_id.0))
        }
        "arrangement-clip-move" => {
            let map = payload_map(payload)?;
            let clip_id = resolve_clip(map)?;
            let start_beat = require_number(map, "start-beat")?;
            app.arr_clip_move(clip_id, start_beat)?;
            Ok(format!("Moved clip {} to beat {start_beat}", clip_id.0))
        }
        "arrangement-clip-resize" => {
            let map = payload_map(payload)?;
            let clip_id = resolve_clip(map)?;
            let start_beat = require_number(map, "start-beat")?;
            let end_beat = require_number(map, "end-beat")?;
            app.arr_clip_resize(clip_id, start_beat, end_beat)?;
            Ok(format!(
                "Resized clip {} to beats {start_beat}-{end_beat}",
                clip_id.0
            ))
        }
        "arrangement-clip-split" => {
            let map = payload_map(payload)?;
            let clip_id = resolve_clip(map)?;
            let beat = require_number(map, "beat")?;
            let right = app.arr_clip_split(clip_id, beat)?;
            Ok(format!(
                "Split clip {} at beat {beat} (new clip {})",
                clip_id.0, right.0
            ))
        }
        "arrangement-clip-set-source" => {
            let map = payload_map(payload)?;
            let clip_id = resolve_clip(map)?;
            let source = parse_source(map)?;
            app.arr_clip_set_source(clip_id, source)?;
            Ok(format!("Set clip {} source", clip_id.0))
        }
        "song-set-end" => {
            let map = payload_map(payload)?;
            let end_beat = require_number(map, "end-beat")?;
            app.arr_set_end(end_beat)?;
            Ok(format!("Set song end to beat {end_beat}"))
        }
        "song-set-loop" => {
            let map = payload_map(payload)?;
            let enabled = map_bool(map, "enabled");
            app.arr_set_loop(enabled)?;
            Ok(format!(
                "Song loop {}",
                if enabled { "enabled" } else { "disabled" }
            ))
        }
        // `seq-song-replace` / `seq-song-clear` keep their names (spec 5,
        // non-goal: renaming the natives) but lower to the arrangement like
        // every other authoring path.
        "song-replace" => {
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
        "song-clear" => {
            app.arr_clear()?;
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
            let clip_id = resolve_clip(map)?;
            // The timeline sends the clip's drawn span alongside its id so
            // the selection is also a one-clip region (region spec 4.1,
            // amended): selecting a clip lights its body and gives
            // copy/delete a target. Absent span = clear the region.
            let span = match (map_number(map, "start"), map_number(map, "end")) {
                (Some(start), Some(end)) if start.is_finite() && end.is_finite() => {
                    Some((start, end))
                }
                _ => None,
            };
            app.select_song_clip_span(track, clip_id, span)?;
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
            // `:scene-lane` marks a marquee swept in the SCENE lane (region
            // spec 4.2, lane spec 8): the same rectangle, but copy/paste/
            // delete carry the scene EVENTS inside it as well as the clips.
            let scene_lane = map_bool(map, "scene-lane");
            app.set_song_region(app::song_region::SongRegionSelection::new_in_lane(
                track_a, track_b, start, end, scene_lane,
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
