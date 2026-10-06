//! The arrangement kinds' setters (kind-bindings spec §14, stage 7d):
//! `set-song` (`song.cursor`, `end`, `loop`, `manual-latch`, `bound-clip`
//! and `track.latched`), `set-clip` (`clip.start`, `end`, `cell`) and
//! `set-song-region` (`select-region!`, `clear-region!`). `cell.selected`
//! sets the delete target directly (`seq-set-delete-target`).
//!
//! Clips are named by their stable clip id, tracks by their stable
//! `TrackId` and cells by (track id, pattern id), resolved when the command
//! lands, so an edit or a reorder in between cannot retarget them; a gone
//! clip, track or pattern is an error. Setters act only where the model
//! differs. Arrangement edits go through the primitives the timeline's
//! commands use (`App::arr_*`, one arrangement history entry each, which
//! undo restores) and land like them ([`super::song::song_edit_landed`]: a
//! rejection, the setter's own included, is latched in `song.edit-error`;
//! a success resyncs the piano roll).
//!
//! Script drags ([`super::ScriptEdit`]): while the pointer is down, every
//! `clip.start`, `clip.end` and `song.end` `set!` (of any number of clips)
//! joins ONE undo entry under [`DRAG_KEY`]. The drag's targets accumulate
//! in `GestureState::script_arrangement_drag` and every frame rebuilds the
//! arrangement from where the gesture started with all of them
//! (`App::arr_script_drag`), so a clip dragged across another only
//! occludes it where it ends up, as a timeline drag. With the pointer up
//! each `set!` is a one-shot edit and its own entry (`arr_clip_move`,
//! `arr_clip_resize`, which grows a take past its end, `arr_set_end`).
//!
//! The cursor, the latches, the bound clip and the region are selection
//! and transport state, without history, as for the legacy commands.
//! Values follow the value rule ([`SetValue`]): beats are finite numbers of
//! at least 0, flags bools, ids non-negative integers; anything else is an
//! error that changes nothing.

use super::track_settings::{command_track, value_id, SetValue};
use crate::*;
use sequencer::app::arr_edit::ArrangementDragTargets;
use sequencer::app::history::MergeKey;
use sequencer::app::song_region::SongRegionSelection;
use sequencer::sequencer::{ArrClip, ClipId, LaneSource, PatternId, TrackId};
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["set-song", "set-clip", "set-song-region"];

/// The merge key every script drag's arrangement edits share.
const DRAG_KEY: &str = "kinds-arrangement";

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// One arrangement edit a setter asks for.
enum SongEdit {
    Loop(bool),
    End(f64),
    Move(ClipId, f64),
    Resize(ClipId, f64, f64),
    Source(ClipId, PatternId),
}

impl SongEdit {
    /// A value a drag moves: joins the script's drag while the pointer is
    /// down.
    fn continuous(&self) -> bool {
        matches!(self, Self::End(_) | Self::Move(..) | Self::Resize(..))
    }

    fn clip(&self) -> Option<ClipId> {
        match *self {
            Self::Move(clip, _) | Self::Resize(clip, ..) | Self::Source(clip, _) => Some(clip),
            Self::Loop(_) | Self::End(_) => None,
        }
    }

    /// Record a continuous edit among a drag's targets.
    fn target(&self, targets: &mut ArrangementDragTargets) {
        match *self {
            Self::End(end) => targets.end = Some(end),
            Self::Move(clip, start) => targets.clips.entry(clip.0).or_default().start = Some(start),
            Self::Resize(clip, _, end) => targets.clips.entry(clip.0).or_default().end = Some(end),
            Self::Loop(_) | Self::Source(..) => {}
        }
    }

    /// The one-shot edit: its own history entry.
    fn apply(self, app: &mut app::App) -> Result<(), String> {
        match self {
            Self::Loop(on) => app.arr_set_loop(on),
            Self::End(end) => app.arr_set_end(end),
            Self::Move(clip, start) => app.arr_clip_move(clip, start),
            Self::Resize(clip, start, end) => app.arr_clip_resize(clip, start, end),
            Self::Source(clip, pattern) => {
                app.arr_clip_set_source(clip, LaneSource::Pattern(pattern))
            }
        }
    }
}

/// A drag frame: `edit` joins the targets the open drag gesture has set
/// (none when it is not [`DRAG_KEY`]'s or not the one they were set in),
/// and the arrangement is rebuilt from the gesture's start with all of them.
fn apply_drag(app: &mut app::App, ctx: &mut LoopCtx<'_>, edit: &SongEdit) -> Result<(), String> {
    let key = MergeKey::new(DRAG_KEY);
    let slot = &mut ctx.gesture.script_arrangement_drag;
    let mut targets = (super::open_drag_targets(app, slot, &key).cloned()).unwrap_or_default();
    edit.target(&mut targets);
    app.arr_script_drag(key, &targets)?;
    ctx.gesture.script_arrangement_drag = app
        .history
        .active_gesture()
        .map(|gesture| (gesture.id, targets));
    Ok(())
}

/// Apply `edit` as a script edit and land it like the legacy song commands.
fn apply_song_edit(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    name: &str,
    edit: SongEdit,
) {
    let continuous = edit.continuous();
    let clip = edit.clip();
    let script = super::ScriptEdit::begin(app, ctx);
    let result = if script.drags(ctx, continuous) {
        apply_drag(app, ctx, &edit)
    } else {
        script.apply_with(app, |app| edit.apply(app))
    };
    if let (Ok(()), Some(clip)) = (&result, clip) {
        // The selected clip's one-clip region follows it, as for
        // `arrangement-clip-move`.
        app.refresh_song_region_for_clip(clip);
    }
    script.end(app, ctx, continuous, result.is_ok());
    super::song::song_edit_landed(app, editor, ctx, name, result.map(|()| None));
}

/// The committed clip `id` names now, with its lane.
fn committed_clip(app: &app::App, id: ClipId) -> Result<(usize, ArrClip), String> {
    app.state
        .with_committed_arrangement(|arrangement| {
            let (track, clip) = arrangement?.find_clip(id)?;
            Some((track, *clip))
        })
        .ok_or_else(|| "the clip is gone".to_string())
}

/// The pattern `map`'s `:pattern-id` names in `track`'s pool.
fn pool_pattern(app: &app::App, track: usize, map: &Payload) -> Result<PatternId, String> {
    let pattern = SetValue::of(map, "pattern-id", "pattern-id").id("a pattern id")?;
    let pattern = PatternId(pattern);
    let pooled = app.state.with_project_scenes(|scenes| {
        scenes
            .track_pools
            .get(track)
            .is_some_and(|pool| pool.get(pattern).is_some())
    });
    pooled
        .then_some(pattern)
        .ok_or_else(|| "the pattern is gone".to_string())
}

/// `set-song`: `{:field :value}` (`:track-id` for `latched`, `:clip-id` for
/// `bound-clip`). Returns the edit to apply, if any; the rest applies here.
fn set_song(
    app: &mut app::App,
    map: &Payload,
) -> Result<(Option<SongEdit>, Option<String>), String> {
    let field = map_string(map, "field").ok_or("needs a :field")?;
    let value = SetValue::of(map, "value", &field);
    let committed = app.state.with_committed_song(|song| {
        song.map_or((0.0, false), |song| (song.end_beat, song.loop_enabled))
    });
    let status = match field.as_str() {
        "loop" => {
            let on = value.flag()?;
            return Ok(((on != committed.1).then_some(SongEdit::Loop(on)), None));
        }
        "end" => {
            let end = value.from(0.0)?;
            return Ok(((end != committed.0).then_some(SongEdit::End(end)), None));
        }
        "cursor" => {
            let beat = value.from(0.0)?;
            if beat != app.arrangement_cursor_beat {
                app.set_arrangement_cursor(beat, app.arrangement_cursor_track);
            }
            None
        }
        "manual-latch" => {
            if value.flag()? {
                return value.fail("false (a launch latches; false is Back to Song)");
            }
            song_manual_latch(&app.state)
                .then(|| app.back_to_song())
                .transpose()?
        }
        "latched" => {
            let track = command_track(app, map)?;
            if value.flag()? {
                return value.fail("false (a launch latches; false hands the lane back)");
            }
            song_lane_latched(app.state.song_manual_latch_mask(), track)
                .then(|| app.back_to_song_track(track))
                .transpose()?
        }
        "bound-clip" => {
            let clip = SetValue::of(map, "clip-id", "bound-clip").id_or_nil("a clip or nil")?;
            match clip.map(ClipId) {
                None => {
                    app.set_song_clip_selection(None);
                }
                Some(id) => {
                    let (track, clip) = committed_clip(app, id)?;
                    let selected = app.song_clip_selection.map(|selection| selection.clip_id);
                    if selected != Some(id) {
                        let span = Some((clip.start_beat, clip.end_beat));
                        app.select_song_clip_span(track, id, span)?;
                    }
                }
            }
            None
        }
        other => return Err(format!("unknown song field '{other}'")),
    };
    Ok((None, status))
}

/// `set-clip`: `{:clip-id :field :value}` (`:track-id :pattern-id` for
/// `cell`). Returns the edit, if the clip differs.
fn set_clip(app: &app::App, map: &Payload) -> Result<Option<SongEdit>, String> {
    let id = ClipId(SetValue::of(map, "clip-id", "clip-id").id("a clip id")?);
    let (track, clip) = committed_clip(app, id)?;
    let field = map_string(map, "field").ok_or("needs a :field")?;
    let value = SetValue::of(map, "value", &field);
    Ok(match field.as_str() {
        "start" => {
            let start = value.from(0.0)?;
            (start != clip.start_beat).then_some(SongEdit::Move(id, start))
        }
        "end" => {
            let end = value.from(0.0)?;
            if end <= clip.start_beat {
                return value.fail(&format!(
                    "a beat after the clip's start ({})",
                    clip.start_beat
                ));
            }
            (end != clip.end_beat).then_some(SongEdit::Resize(id, clip.start_beat, end))
        }
        "cell" => {
            if command_track(app, map).ok() != Some(track) {
                return Err("cell takes a cell of the clip's track".to_string());
            }
            let pattern = pool_pattern(app, track, map)?;
            (clip.pattern_id != Some(pattern.0)).then_some(SongEdit::Source(id, pattern))
        }
        other => return Err(format!("unknown clip field '{other}'")),
    })
}

/// `set-song-region`: `{:track-ids (a b) :start :end :scene-lane}`, or nil
/// to clear.
fn set_region(app: &mut app::App, payload: &Value) -> Result<(), String> {
    let Value::Map(map) = payload else {
        app.clear_song_region();
        return Ok(());
    };
    let tracks = match map.get("track-ids").map(|cell| cell.borrow().clone()) {
        Some(Value::List(ids)) if ids.len() == 2 => ids
            .iter()
            .map(|id| {
                let id = value_id(&id.borrow()).ok_or("track-ids takes two track ids")?;
                live_track_index(app, TrackId(id)).ok_or("the track is gone")
            })
            .collect::<Result<Vec<usize>, _>>()?,
        _ => return Err("needs :track-ids, two track ids".to_string()),
    };
    let start = SetValue::of(map, "start", "region start").from(0.0)?;
    let end = SetValue::of(map, "end", "region end").from(0.0)?;
    // Optional: nil (or absent) is false.
    let scene_lane = SetValue::of(map, "scene-lane", "scene-lane").flag_or(false)?;
    let region = SongRegionSelection::new_in_lane(tracks[0], tracks[1], start, end, scene_lane);
    app.set_song_region(region);
    Ok(())
}

/// Whether `name`'s rejections are arrangement edit errors, latched in
/// `song.edit-error` like the model's: `set-clip`, `song.loop`, `song.end`.
fn latches_errors(name: &str, payload: &Value) -> bool {
    match (name, payload) {
        ("set-clip", _) => true,
        ("set-song", Value::Map(map)) => {
            matches!(map_string(map, "field").as_deref(), Some("loop" | "end"))
        }
        _ => false,
    }
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match (name, &payload) {
        ("set-song-region", payload) => set_region(app, payload).map(|()| (None, None)),
        ("set-song", Value::Map(map)) => set_song(app, map),
        ("set-clip", Value::Map(map)) => set_clip(app, map).map(|edit| (edit, None)),
        _ => Err("the payload is not a dict".to_string()),
    };
    match result {
        Ok((Some(edit), _)) => apply_song_edit(app, editor, ctx, name, edit),
        Ok((None, Some(status))) => editor.handle_host_event(HostEvent::Status(status)),
        Ok((None, None)) => {}
        Err(message) if latches_errors(name, &payload) => {
            super::song::song_edit_landed(app, editor, ctx, name, Err(message));
        }
        Err(message) => editor.handle_host_event(HostEvent::Error(format!("{name}: {message}"))),
    }
}
