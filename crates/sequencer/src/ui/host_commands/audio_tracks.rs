//! Host commands for audio-track regions (docs/audio-tracks-spec.md §7,
//! §8.1). Each routes to one `App` method in `sequencer::app::audio_tracks`,
//! which edits the committed arrangement as one undo step; results land on
//! the status line like every arrangement edit (`song::song_edit_landed`).

use crate::*;

use sequencer::app::audio_tracks::{AudioRegionPatch, FadeHandle, Place};
use sequencer::sequencer::ClipId;

use super::song::{clip_ids, payload_map, require_number, require_whole};
use crate::history_commands::map_bool;

pub(super) const COMMANDS: &[&str] = &[
    "audio-regions-drop",
    "audio-region-move",
    "audio-regions-move",
    "audio-region-resize",
    "audio-region-split",
    "audio-region-duplicate",
    "audio-regions-delete",
    "audio-region-set",
    "audio-region-set-window",
    "audio-region-fade",
    "audio-region-fade-end",
    "audio-regions-paste",
    "audio-lock-select",
    "audio-track-set-input",
    "audio-track-set-monitor",
    "audio-record-offset",
];

type PayloadMap = HashMap<String, Rc<RefCell<Value>>>;

fn optional_track(map: &PayloadMap) -> Result<Option<usize>, String> {
    match map.get("track").map(|cell| cell.borrow().clone()) {
        None | Some(Value::Nil) => Ok(None),
        Some(_) => require_whole(map, "track").map(|track| Some(track as usize)),
    }
}

fn region_id(map: &PayloadMap) -> Result<ClipId, String> {
    require_whole(map, "id").map(ClipId)
}

/// `:paths` (a list) or `:path` (one), as file paths.
fn drop_paths(map: &PayloadMap) -> Result<Vec<PathBuf>, String> {
    let text = |value: &Value| match value {
        Value::String(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };
    if let Some(Value::List(paths)) = map.get("paths").map(|cell| cell.borrow().clone()) {
        return Ok(paths.iter().filter_map(|path| text(&path.borrow())).collect());
    }
    map.get("path")
        .and_then(|path| text(&path.borrow()))
        .map(|path| vec![path])
        .ok_or_else(|| "Drop a sample file, not a folder".to_string())
}

fn optional_f32(map: &PayloadMap, key: &str) -> Option<f32> {
    map_number(map, key).map(|value| value as f32)
}

/// `:key` as a bool when present (`false` and absent differ).
fn optional_bool(map: &PayloadMap, key: &str) -> Option<bool> {
    match map.get(key).map(|cell| cell.borrow().clone()) {
        Some(Value::Bool(value)) => Some(value),
        _ => None,
    }
}

fn run(name: &str, payload: &Value, app: &mut app::App) -> Result<String, String> {
    let map = payload_map(payload)?;
    match name {
        "audio-regions-drop" => {
            let track = optional_track(map)?.ok_or_else(|| "missing :track".to_string())?;
            // No beat (a drop on the mixer strip): the arrangement cursor.
            let beat = map_number(map, "beat").unwrap_or(app.arrangement_cursor_beat);
            let paths = drop_paths(map)?;
            let ids = app.audio_regions_drop(track, beat, &paths)?;
            Ok(match ids.len() {
                1 => "Added audio region".to_string(),
                n => format!("Added {n} audio regions"),
            })
        }
        "audio-track-set-input" => {
            let track = require_whole(map, "track")? as usize;
            let device = match map.get("device").map(|cell| cell.borrow().clone()) {
                Some(Value::String(device)) if !device.is_empty() => Some(device),
                _ => None,
            };
            let Some(device) = device else {
                app.set_audio_track_input(track, None)?;
                return Ok(format!("Track {}: no input", track + 1));
            };
            let left = require_whole(map, "left")? as u16;
            let right = match map.get("right").map(|cell| cell.borrow().clone()) {
                None | Some(Value::Nil) => None,
                Some(_) => Some(require_whole(map, "right")? as u16),
            };
            app.set_audio_track_input(track, Some((device.clone(), left, right)))?;
            Ok(format!("Track {} records from {device}", track + 1))
        }
        "audio-track-set-monitor" => {
            let track = require_whole(map, "track")? as usize;
            let mode = match map.get("mode").map(|cell| cell.borrow().clone()) {
                Some(Value::String(mode)) => sequencer::audio_input::MonitorMode::parse(&mode),
                _ => None,
            }
            .ok_or_else(|| "expected :mode \"off\", \"auto\" or \"in\"".to_string())?;
            app.set_audio_track_monitor(track, mode)?;
            Ok(format!("Track {} monitor: {}", track + 1, mode.name()))
        }
        "audio-record-offset" => {
            app.set_audio_record_offset_ms(require_number(map, "ms")? as f32);
            Ok(String::new())
        }
        "audio-region-move" => {
            let id = region_id(map)?;
            let start = require_number(map, "start")?;
            let track = optional_track(map)?;
            if map_bool(map, "copy") {
                app.audio_region_place(id, start, track, Place::Copy)?;
                Ok("Copied audio region".to_string())
            } else {
                app.audio_region_place(id, start, track, Place::Move)?;
                Ok("Moved audio region".to_string())
            }
        }
        "audio-regions-move" => {
            let ids = clip_ids(map)?;
            app.audio_regions_move(&ids, require_number(map, "delta")?)?;
            Ok(format!("Moved {} audio region(s)", ids.len()))
        }
        "audio-region-resize" => {
            let id = region_id(map)?;
            app.audio_region_resize(id, require_number(map, "start")?, require_number(map, "end")?)?;
            Ok("Resized audio region".to_string())
        }
        "audio-region-split" => {
            let id = region_id(map)?;
            app.audio_region_split(id, require_number(map, "beat")?)?;
            Ok("Split audio region".to_string())
        }
        "audio-region-duplicate" => {
            app.audio_region_duplicate(region_id(map)?)?;
            Ok("Duplicated audio region".to_string())
        }
        "audio-regions-delete" => {
            let ids = clip_ids(map)?;
            app.audio_regions_delete(&ids)?;
            Ok(match ids.len() {
                1 => "Deleted audio region".to_string(),
                n => format!("Deleted {n} audio regions"),
            })
        }
        "audio-region-set" => {
            let id = region_id(map)?;
            if let Some(enabled) = optional_bool(map, "loop") {
                app.audio_region_set_loop(id, enabled)?;
            }
            let patch = AudioRegionPatch {
                gain_db: optional_f32(map, "gain"),
                fade_in_seconds: optional_f32(map, "fade-in"),
                fade_out_seconds: optional_f32(map, "fade-out"),
                muted: optional_bool(map, "muted"),
                transpose: optional_f32(map, "pitch"),
            };
            if patch != AudioRegionPatch::default() {
                app.audio_region_update(id, patch, map_bool(map, "live"))?;
            }
            Ok("Audio region updated".to_string())
        }
        "audio-regions-paste" => {
            let ids = clip_ids(map)?;
            let pasted = app.audio_regions_paste(&ids, require_number(map, "beat")?, optional_track(map)?)?;
            Ok(format!("Pasted {} audio region(s)", pasted.len()))
        }
        "audio-region-set-window" => {
            let id = region_id(map)?;
            app.audio_region_set_window(
                id,
                require_number(map, "offset")?,
                require_number(map, "length")?,
                map_bool(map, "live"),
            )?;
            Ok("Audio clip window set".to_string())
        }
        "audio-region-fade" => {
            let id = region_id(map)?;
            let handle = match map.get("handle").map(|cell| cell.borrow().clone()) {
                Some(Value::Keyword(name)) | Some(Value::String(name)) => FadeHandle::parse(&name),
                _ => None,
            }
            .ok_or_else(|| "audio-region-fade needs a :handle".to_string())?;
            app.audio_region_fade(id, handle, require_number(map, "value")?, map_bool(map, "live"))?;
            Ok("Audio fade set".to_string())
        }
        // A released fade handle: the next drag of the same region's fades is
        // its own undo step.
        "audio-region-fade-end" => {
            sequencer::app::edit::finish_active_gesture(app);
            Ok("Audio fade set".to_string())
        }
        "audio-lock-select" => {
            let ids = clip_ids(map)?;
            app.set_audio_lock_selection(optional_track(map)?.map(|track| (track, ids)));
            Ok("Selected regions for locking".to_string())
        }
        _ => Err(format!("unknown audio command {name}")),
    }
}

/// Apply an audio-region command outside the event loop (capture setup):
/// `None` when `name` is not one.
pub(crate) fn apply_audio_track_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Option<Result<String, String>> {
    if !COMMANDS.contains(&name) {
        return None;
    }
    let result = run(name, payload, app);
    app.sync_audio_track_playback();
    Some(result)
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = run(name, &payload, app);
    if name == "audio-lock-select" {
        // Mirrors the arrangement's clip selection: no status line, no song
        // edit; the knobs re-read the locks they show.
        match result {
            Ok(_) => {
                ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}"))),
        }
        return;
    }
    if name == "audio-record-offset" {
        // A setting, not a song edit: nothing for the status line.
        if let Err(error) = result {
            editor.handle_host_event(HostEvent::Error(format!("{name} failed: {error}")));
        }
        return;
    }
    let result = result.map(Some);
    // Publish at once rather than on the next frame, so playback started
    // right after an edit already hears it.
    app.sync_audio_track_playback();
    super::song::song_edit_landed(app, editor, ctx, name, result);
}
