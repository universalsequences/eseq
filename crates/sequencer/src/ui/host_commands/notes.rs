//! The note setters (kind-bindings spec §14, stage 7e): `set-note`
//! (`note.pitch`, `start`, `length`, `velocity`, `selected`), `add-note`
//! (`add-note!`) and `delete-notes` (`delete-notes!`).
//!
//! A note is named by its track's stable `TrackId` and its note id, resolved
//! when the command lands through the host kinds' note ids
//! ([`NoteShared`]): a note that is gone, or one of another source than the
//! track's piano roll edits now (a clip pinned, a scene launched), is an
//! error that changes nothing. Setters act only where the note differs and
//! go through the piano roll's focus-aware history
//! (`app::edit::apply_recorded_focus_step_mutation`, the legacy piano
//! roll's, one undo entry each; undo restores), landing like the legacy
//! piano roll's actions ([`piano_roll_edit_landed`]). A moved note keeps its
//! id, written into the model with it (spec §14.2j: a note without a model
//! id takes its host one), so its handle follows it, through an undo too;
//! one moved onto another replaces it (the other's handle goes stale). A note keeps its velocity (its step's) when
//! it moves to a step holding no other note; chord notes share their step's.
//! The selection is the piano roll's (no history); an edit keeps the
//! selected notes selected where they land.
//!
//! Values follow the value rule (spec §14.2c): `pitch` an integer in −48–48,
//! `start` a finite number from 0 to below `piano-roll.focus-num-steps`,
//! `length` from 1/32 to 32 steps, `velocity` from 0 to 1, `selected` a
//! bool; anything else is an error. A full step takes no more notes.
//!
//! Script drags ([`super::ScriptEdit`]): while the pointer is down every
//! `pitch`, `start`, `length` and `velocity` `set!` (of any number of notes
//! of one source) joins ONE undo entry. A `set!` only records its target in
//! `GestureState::script_note_drag` (where each note was when the drag
//! started, and where it goes), tied to the drag's gesture id; once per
//! command batch ([`flush_note_drag`], also before any other command lands)
//! the frame puts the steps the drag touched back as it found them and
//! places every target there (`app::edit::note_drag_frame`), so a note
//! dragged across another replaces it only where it ends up; a note it lies
//! over stays registered (`note.hidden`) until the drag ends, and a `set!`
//! or delete of it is an error meanwhile. A frame that fails (a full step)
//! puts that frame's `set!`s back. A drag of another source (a scene
//! launched mid-drag) ends the open one first. Esc rolls the drag back
//! ([`cancel_note_drag`]): the steps, the notes' ids and the selection. With
//! the pointer up each `set!` is its own entry.

use super::step_history::{piano_roll_edit_landed, PianoRollLanding};
use super::track_settings::{command_track, SetValue};
use crate::host_kinds::{NoteKey, NoteSource};
use crate::*;
use sequencer::app::history::{GestureId, MergeKey};
use sequencer::sequencer::StepParam;
use std::collections::{BTreeMap, HashMap, HashSet};

pub(super) const COMMANDS: &[&str] = &["set-note", "add-note", "delete-notes"];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// A note as it is found: where it sits and its values, and its step's
/// velocity.
type Found = (NoteKey, PianoRollNote, f32);

/// One note an edit lifts from where it was and places anew; a new note
/// has no `from`, a deleted one no `to`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteEdit {
    /// Where it was when the edit (or the drag) started, its values and
    /// its step's velocity there.
    from: Option<Found>,
    /// Where it goes: its step and values.
    to: Option<(usize, PianoRollNote)>,
    /// Its step's velocity, when the edit sets it.
    velocity: Option<f32>,
}

/// A script note drag's targets (`GestureState::script_note_drag`, tied to
/// the drag's gesture id).
#[derive(Debug, Default)]
pub(crate) struct NoteDragTargets {
    /// The notes' source, and their ids and the selection as the drag found
    /// them (Esc puts both back).
    source: Option<NoteSource>,
    before_ids: HashMap<NoteKey, u64>,
    before_selection: HashSet<u64>,
    /// Each note the drag moves, by id (the order every frame places them
    /// in), and where the last frame put it.
    targets: BTreeMap<u64, NoteEdit>,
    placed: BTreeMap<u64, NoteKey>,
    /// The `set!`s since the last frame: each one's target before them
    /// (none: a new target), so a failed frame puts back only those. Not
    /// empty: a frame is pending (built at the end of the command batch,
    /// [`flush_note_drag`]).
    frame: Vec<(u64, Option<NoteEdit>)>,
}

impl NoteDragTargets {
    fn joins(&self, source: &NoteSource) -> bool {
        (self.source).is_some_and(|known| known.same_notes(source))
    }
}

/// Every step `edits` lift a note from or place one on.
fn touched_steps(edits: &[NoteEdit]) -> Vec<usize> {
    (edits.iter())
        .flat_map(|edit| {
            let from = edit.from.map(|(key, _, _)| key.step);
            from.into_iter().chain(edit.to.map(|(step, _)| step))
        })
        .collect()
}

/// Lift every edit's note from where it was, then place each where it goes
/// (replacing a note already there), in order; set the velocities they ask
/// for (a note keeps its own on a step it has to itself). Returns where
/// each edit's note landed.
fn apply_note_edits(
    lanes: &PianoRollLanes,
    edits: &[NoteEdit],
) -> Result<Vec<Option<NoteKey>>, String> {
    let mut steps: BTreeMap<usize, Vec<PianoRollNote>> = BTreeMap::new();
    let notes_at = |steps: &mut BTreeMap<usize, Vec<PianoRollNote>>, step: usize| {
        if !steps.contains_key(&step) {
            steps.insert(step, lanes.note_entries(step));
        }
    };
    for (key, _, _) in edits.iter().filter_map(|edit| edit.from) {
        notes_at(&mut steps, key.step);
        let notes = steps.get_mut(&key.step).expect("just read");
        notes.retain(|note| NoteKey::of(key.step, note) != key);
    }
    let mut placed = Vec::with_capacity(edits.len());
    let mut velocities = Vec::new();
    for edit in edits {
        let Some((step, note)) = edit.to else {
            placed.push(None);
            continue;
        };
        let note = normalized_piano_roll_note(&note);
        let key = NoteKey::of(step, &note);
        notes_at(&mut steps, step);
        let notes = steps.get_mut(&step).expect("just read");
        let alone = notes.is_empty();
        notes.retain(|other| NoteKey::of(step, other) != key);
        if notes.len() >= piano_roll_step_capacity() {
            return Err(format!("step {} holds no more notes", step + 1));
        }
        notes.push(note);
        let moved_away = edit.from.is_some_and(|(from, _, _)| from.step != step);
        let carried = (alone && moved_away).then(|| edit.from.map(|(_, _, v)| v));
        if let Some(velocity) = edit.velocity.or(carried.flatten()) {
            velocities.push((step, velocity));
        }
        placed.push(Some(key));
    }
    for (step, notes) in &steps {
        lanes.set_note_entries(*step, notes);
    }
    for (step, velocity) in velocities {
        lanes.set_step_param(step, StepParam::Velocity, velocity);
    }
    Ok(placed)
}

/// The notes `selection` (item ids) selects, by key.
fn selected_keys(lanes: &PianoRollLanes, selection: &HashSet<u64>) -> Vec<NoteKey> {
    let mut steps: HashMap<usize, Vec<PianoRollNote>> = HashMap::new();
    (selection.iter())
        .filter_map(|id| {
            let (step, voice) = lanes.item_parts(*id)?;
            let notes = (steps.entry(step)).or_insert_with(|| lanes.note_entries(step));
            Some(NoteKey::of(step, notes.get(voice)?))
        })
        .collect()
}

/// Keep the selected notes selected where `moves` (key before → key after,
/// none: deleted) put them, and the rest where they now sit: the selection
/// is by item id (step, voice), and an edit shifts the voices beside the
/// notes it moves. Run with `keys` from [`selected_keys`] before the edit.
fn reselect(
    ctx: &LoopCtx<'_>,
    lanes: &PianoRollLanes,
    keys: Vec<NoteKey>,
    moves: &[(NoteKey, Option<NoteKey>)],
) {
    let mut steps: HashMap<usize, Vec<PianoRollNote>> = HashMap::new();
    let selection: HashSet<u64> = keys
        .into_iter()
        .filter_map(|key| match moves.iter().find(|(from, _)| *from == key) {
            Some((_, to)) => *to,
            None => Some(key),
        })
        .filter_map(|key| {
            let notes = (steps.entry(key.step)).or_insert_with(|| lanes.note_entries(key.step));
            Some(piano_roll_item_id(key.step, key.voice_in(notes)?))
        })
        .collect();
    *ctx.shared.piano_roll_selection.lock().unwrap() = selection;
}

/// The piano roll's notes source for `track` now, with the track's instance.
fn track_source(app: &app::App, ctx: &LoopCtx<'_>, map: &Payload) -> Result<NoteSource, String> {
    let track = command_track(app, map)?;
    let tid = app.track_registry.id_at(track).ok_or("the track is gone")?;
    let instance = (ctx.frame.host_kinds.track_instance(tid.0)).ok_or("the track is gone")?;
    Ok(NoteSource::resolve(app, track, instance))
}

/// Note `nid` of `source`: where it sits, its values and velocity now, and
/// its voice there (an error when it is gone, of another source, or hidden
/// under a script drag's note).
fn resolve_note(
    app: &app::App,
    ctx: &LoopCtx<'_>,
    source: &NoteSource,
    nid: u64,
) -> Result<(Found, usize), String> {
    let gone = || "the note is gone".to_string();
    let key = {
        let shared = ctx.frame.host_kinds.shared.borrow();
        let notes = &shared.notes;
        let known = notes.source.is_some_and(|known| known.same_notes(source));
        if !known {
            return Err(gone());
        }
        match notes.key_of(nid) {
            Some(key) => key,
            None if notes.held.contains(&nid) && app.active_note_drag().is_some() => {
                return Err("the note is hidden under a dragged note until the drag ends".into())
            }
            None => return Err(gone()),
        }
    };
    let lanes = source.lanes(&app.state);
    let entries = lanes.note_entries(key.step);
    let voice = key.voice_in(&entries).ok_or_else(gone)?;
    let velocity = lanes.step_param(key.step, StepParam::Velocity);
    Ok(((key, entries[voice], velocity), voice))
}

/// `field`'s new value applied to a note going to `(step, note)` with
/// `velocity` (the value rule: see the module docs).
fn edited(
    lanes: &PianoRollLanes,
    field: &str,
    value: &SetValue<'_>,
    (mut step, mut note): (usize, PianoRollNote),
    mut velocity: Option<f32>,
) -> Result<((usize, PianoRollNote), Option<f32>), String> {
    match field {
        "pitch" => {
            let (min, max) = (
                PIANO_ROLL_MIN_TRANSPOSE as i64,
                PIANO_ROLL_MAX_TRANSPOSE as i64,
            );
            note.transpose = value.signed(min, max)? as f32;
        }
        "start" => {
            let num_steps = lanes.num_steps();
            let start = value.number(0.0, num_steps as f64)?;
            if start >= num_steps as f64 {
                return value.fail(&format!("a step from 0 to below {num_steps}"));
            }
            (step, note.delay) = piano_roll_time_to_step_delay(start, num_steps);
        }
        "length" => {
            let max = StepParam::Duration.max() as f64;
            note.duration = value.number(PIANO_ROLL_MIN_DURATION as f64, max)? as f32;
        }
        "velocity" => velocity = Some(value.number(0.0, 1.0)? as f32),
        other => return Err(format!("unknown note field '{other}'")),
    }
    Ok(((step, note), velocity))
}

/// Apply `edits` to `source` as one undo entry (`label`), keeping the
/// selection on the notes; returns where each landed.
fn apply_one_shot(
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
    source: &NoteSource,
    edits: &[NoteEdit],
    label: &'static str,
) -> Result<Vec<Option<NoteKey>>, String> {
    let lanes = source.lanes(&app.state);
    let selection = ctx.shared.piano_roll_selection.lock().unwrap().clone();
    let keys = selected_keys(&lanes, &selection);
    let steps = touched_steps(edits);
    let focus = app.track_edit_focus(source.track);
    let script = super::ScriptEdit::begin(app, ctx);
    let mut placed = Vec::new();
    let outcome = script.apply_with(app, |app| {
        app::edit::apply_recorded_focus_step_mutation(app, focus, &steps, label, |app| {
            let lanes = source.lanes(&app.state);
            placed = apply_note_edits(&lanes, edits).map_err(app::edit::EditError::ReplayFailed)?;
            Ok(())
        })
    });
    let changed = matches!(outcome, Ok(app::edit::EditOutcome::Applied(_)));
    script.end(app, ctx, false, changed);
    outcome.map_err(|error| format!("{error:?}"))?;
    let moves: Vec<(NoteKey, Option<NoteKey>)> = (edits.iter().zip(&placed))
        .filter_map(|(edit, placed)| Some((edit.from?.0, *placed)))
        .collect();
    reselect(ctx, &source.lanes(&app.state), keys, &moves);
    if changed {
        piano_roll_edit_landed(ctx, source.track, PianoRollLanding::Recorded);
    }
    Ok(placed)
}

/// A drag frame: drag `id` (of `source`) is rebuilt from where it started
/// with all its targets (`app::edit::note_drag_frame`: one restore, one rebuild, one
/// scheduler publish). A frame that fails puts this frame's `set!`s back
/// and rebuilds with the earlier targets.
fn drag_frame(
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
    id: GestureId,
    source: NoteSource,
    targets: &mut NoteDragTargets,
) -> Result<(), String> {
    let selection = ctx.shared.piano_roll_selection.lock().unwrap().clone();
    let keys = selected_keys(&source.lanes(&app.state), &selection);
    let focus = app.track_edit_focus(source.track);
    let rebuild = |app: &mut app::App, targets: &NoteDragTargets| {
        let edits: Vec<NoteEdit> = targets.targets.values().copied().collect();
        let mut placed = Vec::new();
        app::edit::note_drag_frame(app, id, focus, &touched_steps(&edits), |app| {
            placed = apply_note_edits(&source.lanes(&app.state), &edits)?;
            Ok(())
        })?;
        Ok::<_, String>(placed)
    };
    let (placed, result) = match rebuild(app, targets) {
        Ok(placed) => (placed, Ok(())),
        Err(error) => {
            for (nid, earlier) in targets.frame.drain(..).rev() {
                match earlier {
                    Some(edit) => targets.targets.insert(nid, edit),
                    None => targets.targets.remove(&nid),
                };
            }
            (rebuild(app, targets)?, Err(error))
        }
    };
    // The ids as the drag leaves them: the notes it moves where they land,
    // the others where the drag found them but under a moved note (held).
    let moved: HashSet<u64> = targets.targets.keys().copied().collect();
    let froms: HashSet<NoteKey> = (targets.targets.values())
        .filter_map(|edit| edit.from.map(|(key, _, _)| key))
        .collect();
    let mut ids: HashMap<NoteKey, u64> = (targets.before_ids.iter())
        .filter(|(key, nid)| !froms.contains(key) && !moved.contains(nid))
        .map(|(key, nid)| (*key, *nid))
        .collect();
    let mut placed_keys = BTreeMap::new();
    for (nid, key) in targets.targets.keys().zip(&placed) {
        if let Some(key) = key {
            ids.insert(*key, *nid);
            placed_keys.insert(*nid, *key);
        }
    }
    let held: HashSet<u64> = (targets.before_ids.iter())
        .filter(|(key, nid)| !moved.contains(nid) && ids.get(key) != Some(nid))
        .map(|(_, nid)| *nid)
        .collect();
    let moves: Vec<(NoteKey, Option<NoteKey>)> = (targets.targets.iter())
        .filter_map(|(nid, edit)| {
            let last = (targets.placed.get(nid).copied()).or(edit.from.map(|from| from.0))?;
            Some((last, placed_keys.get(nid).copied()))
        })
        .collect();
    reselect(ctx, &source.lanes(&app.state), keys, &moves);
    let notes = &mut ctx.frame.host_kinds.shared.borrow_mut().notes;
    notes.set_drag_ids(ids, held);
    notes.drag_frames += 1;
    targets.placed = placed_keys;
    result
}

/// Build the pending script note drag frame, if any: once per command
/// batch (the event loop runs it after the batch, before the host kinds
/// sync), and before any other command lands (`dispatch_custom_host_command`).
pub(crate) fn flush_note_drag(app: &mut app::App, editor: &mut Editor, ctx: &mut LoopCtx<'_>) {
    let Some((id, targets)) = ctx.gesture.script_note_drag.as_mut() else {
        return;
    };
    let Some(source) = targets.source.filter(|_| !targets.frame.is_empty()) else {
        return;
    };
    let (id, mut targets) = (*id, std::mem::take(targets));
    let result = drag_frame(app, ctx, id, source, &mut targets);
    targets.frame.clear();
    ctx.gesture.script_note_drag = Some((id, targets));
    // A drag view: its later `set!`s join this drag until release.
    ctx.gesture.script_param_gesture = app.active_note_drag();
    match result {
        Ok(()) => piano_roll_edit_landed(ctx, source.track, PianoRollLanding::Frame),
        Err(message) => editor.handle_host_event(HostEvent::Error(format!("set-note: {message}"))),
    }
}

/// Esc on an open script note drag: its steps go back as it found them
/// (nothing recorded), and the notes' ids and the selection with them.
/// `None` when the active gesture is not a script note drag.
pub(crate) fn cancel_note_drag(
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
) -> Option<Result<(), app::edit::EditError>> {
    let open = app.active_note_drag()?;
    if ctx.gesture.script_note_drag.as_ref()?.0 != open {
        return None;
    }
    let (_, targets) = ctx.gesture.script_note_drag.take()?;
    let result = app::edit::cancel_active_gesture(app).map(|_| ());
    let notes = &mut ctx.frame.host_kinds.shared.borrow_mut().notes;
    notes.set_drag_ids(targets.before_ids, HashSet::new());
    *ctx.shared.piano_roll_selection.lock().unwrap() = targets.before_selection;
    ctx.gesture.script_param_gesture = None;
    if let Some(source) = targets.source {
        ctx.shared.ui_invalidations.push(UiInvalidation::PianoRoll {
            track: source.track,
            change: PianoRollInvalidation::Items,
        });
    }
    Some(result)
}

/// `set-note`: `{:track-id :nid :field :value}`.
fn set_note(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    map: &Payload,
) -> Result<(), String> {
    let source = track_source(app, ctx, map)?;
    let nid = SetValue::of(map, "nid", "nid").id("a note id")?;
    let (field, value) = SetValue::field(map)?;
    let script = super::ScriptEdit::begin(app, ctx);
    let drags = field != "selected" && script.drags(ctx, true);
    // The script drag this `set!` joins: a pending frame's, or the open
    // drag's, while it is this source's.
    let key = MergeKey::new(app::edit::NOTE_DRAG_KEY);
    let joins = drags && {
        let pending = (ctx.gesture.script_note_drag.as_ref())
            .is_some_and(|(_, targets)| !targets.frame.is_empty());
        let slot = &mut ctx.gesture.script_note_drag;
        let targets = match pending {
            true => slot.as_mut().map(|(_, targets)| targets),
            false => super::open_drag_targets(app, slot, &key),
        };
        targets.is_some_and(|targets| targets.joins(&source))
    };
    if !joins {
        flush_note_drag(app, editor, ctx);
        if drags && app.active_note_drag().is_some() {
            // An open drag of another source (a scene launched, a clip
            // pinned): it ends here, with every write it made.
            app::edit::finish_active_gesture(app);
        }
    }
    if field == "selected" {
        let on = value.flag()?;
        let ((key, _, _), voice) = resolve_note(app, ctx, &source, nid)?;
        let item = piano_roll_item_id(key.step, voice);
        let mut selection = ctx.shared.piano_roll_selection.lock().unwrap();
        let changed = match on {
            true => selection.insert(item),
            false => selection.remove(&item),
        };
        if changed {
            ctx.shared.ui_invalidations.push(UiInvalidation::PianoRoll {
                track: source.track,
                change: PianoRollInvalidation::Selection,
            });
        }
        return Ok(());
    }
    let lanes = source.lanes(&app.state);
    // Where the note goes so far: a drag's target, else where it sits.
    let earlier = (ctx.gesture.script_note_drag.as_ref())
        .filter(|_| joins)
        .and_then(|(_, targets)| targets.targets.get(&nid).copied());
    let (from, to, velocity) = match earlier {
        Some(NoteEdit {
            from: Some(from),
            to: Some(to),
            velocity,
        }) => (from, to, velocity),
        _ => {
            let ((key, note, velocity), _) = resolve_note(app, ctx, &source, nid)?;
            ((key, note, velocity), (key.step, note), None)
        }
    };
    let (mut to, velocity) = edited(&lanes, &field, &value, to, velocity)?;
    // The note keeps its id in the model wherever it lands (a note with no
    // model id takes its host one).
    to.1.id = sequencer::sequencer::NoteId::try_from(nid).map_err(|_| "the note is gone")?;
    let edit = NoteEdit {
        from: Some(from),
        to: Some(to),
        velocity,
    };
    if drags {
        if !joins {
            let selection = ctx.shared.piano_roll_selection.lock().unwrap().clone();
            let targets = NoteDragTargets {
                source: Some(source),
                before_ids: ctx.frame.host_kinds.shared.borrow().notes.ids(),
                before_selection: selection,
                ..NoteDragTargets::default()
            };
            ctx.gesture.script_note_drag = Some((app::edit::next_gesture_id(), targets));
        }
        // Built once per frame ([`flush_note_drag`]).
        let (_, targets) = ctx.gesture.script_note_drag.as_mut().expect("just joined");
        let earlier = targets.targets.insert(nid, edit);
        if !targets.frame.iter().any(|(other, _)| *other == nid) {
            targets.frame.push((nid, earlier));
        }
        return Ok(());
    }
    let (key, note, current) = from;
    let target = normalized_piano_roll_note(&to.1);
    let unchanged = NoteKey::of(to.0, &target) == key
        && target.duration == note.duration
        && velocity.is_none_or(|velocity| velocity == current);
    if unchanged {
        return Ok(());
    }
    let placed = apply_one_shot(app, ctx, &source, &[edit], "Edit note")?;
    if let Some(Some(key)) = placed.first() {
        let notes = &mut ctx.frame.host_kinds.shared.borrow_mut().notes;
        notes.rekey(nid, *key);
    }
    Ok(())
}

/// `add-note`: `{:track-id :start :pitch :length :velocity}` (velocity nil:
/// the step's). A note where one sits replaces it.
fn add_note(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let source = track_source(app, ctx, map)?;
    let lanes = source.lanes(&app.state);
    let blank = PianoRollNote {
        transpose: 0.0,
        duration: 1.0,
        delay: 0.0,
        id: 0,
    };
    let mut to = (0, blank);
    for field in ["start", "pitch", "length"] {
        (to, _) = edited(&lanes, field, &SetValue::of(map, field, field), to, None)?;
    }
    let velocity = match SetValue::of(map, "velocity", "velocity") {
        value if matches!(value.value(), Value::Nil) => None,
        value => Some(value.number(0.0, 1.0)? as f32),
    };
    let edit = NoteEdit {
        from: None,
        to: Some(to),
        velocity,
    };
    let placed = apply_one_shot(app, ctx, &source, &[edit], "Add note")?;
    // A note it landed on is replaced: that one's handle goes stale.
    if let Some(Some(key)) = placed.first() {
        let notes = &mut ctx.frame.host_kinds.shared.borrow_mut().notes;
        let known = notes.source.is_some_and(|known| known.same_notes(&source));
        if let Some(nid) = notes.nid_at(key).filter(|_| known) {
            notes.forget(&[nid]);
        }
    }
    Ok(())
}

/// `delete-notes`: `{:track-id :track-ids :nids}` (`track-ids` each note's
/// track, all `track-id`; nil: a stale note), one undo entry; any note gone
/// is an error that deletes none.
fn delete_notes(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let list = |key: &'static str, what: &'static str| match map.get(key) {
        Some(cell) => match &*cell.borrow() {
            Value::List(items) => (items.iter())
                .map(|item| SetValue::new(key, item.borrow().clone()).id_or_nil(what))
                .collect::<Result<Vec<Option<u64>>, _>>(),
            _ => Err(format!("needs :{key}, a list of {what}")),
        },
        None => Ok(Vec::new()),
    };
    let tids = list("track-ids", "track ids")?;
    if tids.contains(&None) {
        return Err("the note is gone".to_string());
    }
    let track = map_usize(map, "track-id").map(|tid| tid as u64);
    if tids.iter().any(|tid| *tid != track) {
        return Err("the notes are of more than one track".to_string());
    }
    let source = track_source(app, ctx, map)?;
    let nids = (list("nids", "note ids")?.into_iter())
        .map(|nid| nid.ok_or("the note is gone"))
        .collect::<Result<Vec<u64>, _>>()?;
    let mut edits = Vec::new();
    for nid in &nids {
        let (from, _) = resolve_note(app, ctx, &source, *nid)?;
        edits.push(NoteEdit {
            from: Some(from),
            to: None,
            velocity: None,
        });
    }
    if edits.is_empty() {
        return Ok(());
    }
    apply_one_shot(app, ctx, &source, &edits, "Delete notes")?;
    ctx.frame.host_kinds.shared.borrow_mut().notes.forget(&nids);
    Ok(())
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match (name, &payload) {
        ("set-note", Value::Map(map)) => set_note(app, editor, ctx, map),
        ("add-note", Value::Map(map)) => add_note(app, ctx, map),
        ("delete-notes", Value::Map(map)) => delete_notes(app, ctx, map),
        _ => Err("the payload is not a dict".to_string()),
    };
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
