//! The piano roll (spec §14, stage 7e): the `piano-roll` singleton (the
//! current track's edit focus, its pinned clip, loop window and playhead)
//! and its `note`s (the timeline's items and their selection).
//!
//! **Identity.** A chord note carries a model id ([`NoteId`], spec §14.2j)
//! beside its lanes (live `ChordData`, a pattern's `chord_snapshot`, the
//! step snapshots history restores; never saved: a project load gives fresh
//! ones), allocated when the note is created and carried by every edit that
//! moves or rewrites it: the note setters and script drags, the legacy piano
//! roll's moves, the step grid's step moves, a recording beside it, an undo
//! or redo. A copy beside its original (a paste, a doubled pattern) is
//! another note, a fresh id. The note instance is keyed (track instance id,
//! nid), the nid being the model id, so a held handle follows its note
//! through all of those, and an undo brings a replaced note back under its
//! own id (a new instance: its handle was dropped with it). A step's single
//! note held by its step parameters (a step turned on in the grid) has no
//! model id: the host gives it one by its [`NoteKey`] (step, transpose,
//! offset) from the model's allocator, keeps it while the note stays at its
//! key, and a setter writes it into the model with the note (from then on
//! the note is a chord note with that id); such a note's handle goes stale
//! when another path moves it, and on an undo or redo that changes the
//! notes. A repeat of a model id in one source (a writer that copied lanes
//! whole) is treated as id-less. The key table ([`NoteShared`]) is where
//! each id sits as of the last sync (and the setters' moves since). The
//! notes are those of the piano roll's source ([`NoteSource`]: the current
//! track's resolved focus, the effective pattern for a live focus); another
//! source (a track switch, a clip pinned, a scene launch, a project load)
//! replaces them all.
//!
//! **Feeds.** The focus fields are model fields behind [`FocusKey`] (the
//! source, the pinned clip, the committed song and scenes revisions, the
//! pattern epoch, the live length and the song structure generation),
//! compared every tick without allocating; they derive through
//! the `App`'s focus accessors. The notes are registered lazily, on the first
//! read of `piano-roll.notes` (the reader hook, or the tick once observed),
//! like a device's params; then the tick re-reads them
//! (`PianoRollLanes::step_rows_batch`, the legacy items' batch read with
//! the step parameters) only when their [`ContentKey`] moved (the scheduler
//! published the source's track again (a live focus: the published track
//! snapshot moved), the scenes or pool content revision, the pattern epoch
//! or the source moved, an undo or redo replayed: `App::history_replays`,
//! a focus step edit committed, rolled back or dragged:
//! `App::focus_step_edits`): an idle tick loads a few counters. The focus
//! steps (`focus_steps`) share the key and the read. The selection (`note.selected`) is the legacy
//! piano roll's, compared every tick while notes are registered.
//! `piano-roll.playhead` is live: the focus playhead
//! (`App::focus_playhead_step`) while observed; a cold read of a pinned
//! focus sees -1 while stopped, else the last one computed.

use super::*;
use sequencer::sequencer::{NoteId, PatternId, SequencerTrackSnapshot};

/// A note's place in its source: its step, and its transpose and offset as
/// the model holds them (bit-exact). A step holds one note per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NoteKey {
    pub(crate) step: usize,
    transpose: u32,
    delay: u32,
}

impl NoteKey {
    pub(crate) fn of(step: usize, note: &PianoRollNote) -> Self {
        // `+ 0.0` makes -0.0 the key of 0.0.
        Self {
            step,
            transpose: (note.transpose + 0.0).to_bits(),
            delay: (note.delay + 0.0).to_bits(),
        }
    }

    /// The voice of the note this key names among its step's `notes`.
    pub(crate) fn voice_in(&self, notes: &[PianoRollNote]) -> Option<usize> {
        (notes.iter()).position(|note| Self::of(self.step, note) == *self)
    }
}

/// Where the piano roll's notes come from: a track (its position and
/// instance) and its resolved edit focus (with the effective pattern of a
/// live focus, so a scene launch is another source).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoteSource {
    pub(crate) track: usize,
    pub(crate) track_id: InstanceId,
    pub(crate) focus: PianoRollFocusSpec,
    pattern: Option<PatternId>,
}

impl NoteSource {
    pub(crate) fn resolve(app: &app::App, track: usize, track_id: InstanceId) -> Self {
        let focus = PianoRollFocusSpec::from_focus(app.track_edit_focus(track));
        let pattern = match focus {
            PianoRollFocusSpec::Live => app
                .state
                .with_project_scenes(|scenes| scenes.effective_pattern_id(track)),
            _ => None,
        };
        Self {
            track,
            track_id,
            focus,
            pattern,
        }
    }

    pub(crate) fn lanes(&self, state: &Arc<SequencerState>) -> PianoRollLanes {
        PianoRollLanes::new(state, self.track, self.focus)
    }

    /// Whether `other` holds the same notes (a track reorder moves only the
    /// position).
    pub(crate) fn same_notes(&self, other: &NoteSource) -> bool {
        (self.track_id, self.focus, self.pattern) == (other.track_id, other.focus, other.pattern)
    }
}

/// One listed note: its instance, id, key and voice, and its length and
/// velocity (as of the last sync).
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteRow {
    pub(crate) id: InstanceId,
    pub(crate) nid: u64,
    pub(crate) key: NoteKey,
    pub(crate) voice: usize,
    length: f32,
    velocity: f32,
}

impl NoteRow {
    /// What an undo or redo compares: where the note sits and its values.
    fn content(&self) -> (NoteKey, u32, u32) {
        (self.key, self.length.to_bits(), self.velocity.to_bits())
    }
}

/// The notes' share of [`KindsShared`] (the reader hook registers them; the
/// note setters move their ids).
#[derive(Default)]
pub(crate) struct NoteShared {
    /// `piano-roll.notes` was read or observed: the notes are registered and
    /// kept current.
    registered: bool,
    /// The tick has pushed `piano-roll.notes` since they were (re)registered.
    pushed: bool,
    /// The source the rows are of, and the one the tick resolved last (a
    /// cold read registers against it, the focus steps' too).
    pub(crate) source: Option<NoteSource>,
    pub(super) focus: Option<NoteSource>,
    /// Each note's id by its key, while it sits there, and the reverse
    /// (as of the last sync, and the setters' and drags' moves since).
    ids: HashMap<NoteKey, u64>,
    keys: HashMap<u64, NoteKey>,
    /// The ids the host gave notes with no model id (a step's single note
    /// held by its step parameters), as of the last sync.
    fallback: HashSet<u64>,
    /// The listed notes in order (step, then voice).
    pub(crate) rows: Vec<NoteRow>,
    /// The notes a script drag lies over: kept registered (unlisted,
    /// `hidden`) until it ends.
    pub(crate) held: HashSet<u64>,
    /// The focus playhead last computed (a cold read of a pinned focus).
    playhead: f64,
    /// Note re-reads and script drag frames, for tests.
    pub(crate) syncs: u64,
    pub(crate) drag_frames: u64,
}

impl NoteShared {
    /// The id of the note at `key`, if one is known there.
    pub(crate) fn nid_at(&self, key: &NoteKey) -> Option<u64> {
        self.ids.get(key).copied()
    }

    /// Where note `nid` sits, while it is known.
    pub(crate) fn key_of(&self, nid: u64) -> Option<NoteKey> {
        self.keys.get(&nid).copied()
    }

    /// Every known (key, id).
    pub(crate) fn ids(&self) -> HashMap<NoteKey, u64> {
        self.ids.clone()
    }

    /// Note `nid` now sits at `to`; a note known at `to` before is replaced
    /// (its handle goes stale at the next sync).
    pub(crate) fn rekey(&mut self, nid: u64, to: NoteKey) {
        self.move_notes(&[(nid, to)]);
    }

    /// Each note of `moves` now sits at its key (all lifted first, so notes
    /// may trade places); a note known at one of those keys before and not
    /// moved is replaced (its handle goes stale at the next sync).
    pub(crate) fn move_notes(&mut self, moves: &[(u64, NoteKey)]) {
        for (nid, _) in moves {
            if let Some(from) = self.keys.remove(nid) {
                self.ids.remove(&from);
            }
        }
        for (nid, to) in moves {
            if let Some(other) = self.ids.insert(*to, *nid) {
                self.keys.remove(&other);
            }
            self.keys.insert(*nid, *to);
        }
    }

    /// Notes `nids` are gone.
    pub(crate) fn forget(&mut self, nids: &[u64]) {
        for nid in nids {
            if let Some(key) = self.keys.remove(nid) {
                self.ids.remove(&key);
            }
        }
    }

    /// The ids during a script drag: `ids` as the drag computed them, and
    /// the notes it lies over.
    pub(crate) fn set_drag_ids(&mut self, ids: HashMap<NoteKey, u64>, held: HashSet<u64>) {
        self.keys = ids.iter().map(|(key, nid)| (*nid, *key)).collect();
        self.ids = ids;
        self.held = held;
    }

    /// The id of the note at `key` with model id `model` (`0`: none), not
    /// one of `used` (the ids this listing gave already): its model id; else
    /// (none, or a copy's repeat of one) the host id this key's id-less note
    /// had; else a fresh one, from the model's allocator (a setter writes it
    /// into the model with the note). A host id given is added to `given`.
    fn id_for(
        &mut self,
        key: NoteKey,
        model: NoteId,
        used: &mut HashSet<u64>,
        given: &mut HashSet<u64>,
    ) -> u64 {
        let model = u64::from(model);
        let nid = if model != 0 && used.insert(model) {
            model
        } else {
            let known = (self.ids.get(&key).copied())
                .filter(|nid| self.fallback.contains(nid) && used.insert(*nid));
            let nid = known.unwrap_or_else(|| {
                let nid = u64::from(sequencer::sequencer::new_note_id());
                used.insert(nid);
                nid
            });
            given.insert(nid);
            nid
        };
        if let Some(other) = self.ids.insert(key, nid).filter(|other| *other != nid) {
            self.keys.remove(&other);
        }
        if let Some(from) = self.keys.insert(nid, key).filter(|from| *from != key) {
            if self.ids.get(&from) == Some(&nid) {
                self.ids.remove(&from);
            }
        }
        nid
    }

    /// Forget every note's id but `kept`'s.
    fn keep_only(&mut self, kept: &HashSet<u64>) {
        self.ids.retain(|_, nid| kept.contains(nid));
        self.keys.retain(|nid, _| kept.contains(nid));
    }

    /// Forget every note (another source); returns the source they were
    /// of (its note instances are to be dropped).
    fn reset(&mut self) -> Option<NoteSource> {
        crate::piano_roll::clear_implicit_note_ids();
        self.ids.clear();
        self.keys.clear();
        self.fallback.clear();
        self.rows.clear();
        self.held.clear();
        self.pushed = false;
        self.source.take()
    }
}

/// Bring the note instances in line with `source`'s notes (see the module
/// docs): register the new ones, push every field (each compared with its
/// cell), drop the gone ones but those a script drag lies over while
/// `keep_held` (`hidden`). After an undo or redo (`replayed`) that changed
/// the notes, every note without a model id gets a fresh id. `steps` is
/// the source's ([`source_rows`]). Returns the listed instances and whether
/// anything changed.
pub(super) fn sync_notes<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    source: NoteSource,
    steps: &[StepRow],
    keep_held: bool,
    replayed: bool,
) -> (Vec<InstanceId>, bool) {
    let mut changed = false;
    let replaced = {
        let notes = &mut shared.borrow_mut().notes;
        let same = notes.source.is_some_and(|known| known.same_notes(&source));
        let replaced = if same { None } else { notes.reset() };
        // A track reorder moves only the position.
        notes.source = Some(source);
        replaced
    };
    if let Some(old) = replaced {
        changed |= reconcile_children(store, old.track_id, NOTE, &[], |_, _, _| {}).1;
    }
    // The listed notes: a repeat of a key gets no instance.
    let mut seen = HashSet::new();
    let listed: Vec<(NoteKey, usize, PianoRollNote, f32)> = (steps.iter().enumerate())
        .flat_map(|(step, (notes, values))| {
            let velocity = values[StepParam::Velocity.index()];
            let notes = notes.iter().copied().enumerate();
            notes.map(move |(voice, note)| (NoteKey::of(step, &note), voice, note, velocity))
        })
        .filter(|(key, ..)| seen.insert(*key))
        .collect();
    let (nids, held) = {
        let notes = &mut shared.borrow_mut().notes;
        let content = |(key, _, note, velocity): &(NoteKey, usize, PianoRollNote, f32)| {
            (*key, note.duration.to_bits(), velocity.to_bits())
        };
        if replayed && !(notes.rows.iter().map(NoteRow::content)).eq(listed.iter().map(content)) {
            // The model's ids came back with the notes; the host's did not.
            let fallback = std::mem::take(&mut notes.fallback);
            notes.ids.retain(|_, nid| !fallback.contains(nid));
            notes.keys.retain(|nid, _| !fallback.contains(nid));
        }
        if !keep_held {
            notes.held.clear();
        }
        let (mut used, mut given) = (HashSet::new(), HashSet::new());
        let nids: Vec<u64> = (listed.iter())
            .map(|(key, _, note, _)| notes.id_for(*key, note.id, &mut used, &mut given))
            .collect();
        // The lanes' readers report these notes with their host ids, so a
        // writer that rewrites their step stores them in the model.
        let implicit = (listed.iter().zip(&nids))
            .filter(|(_, nid)| given.contains(nid))
            .filter_map(|((key, _, note, _), nid)| {
                Some((key.step, *note, NoteId::try_from(*nid).ok()?))
            });
        crate::piano_roll::set_implicit_note_ids(source.track, source.focus, implicit);
        notes.fallback = given;
        let held: Vec<u64> = (notes.held.iter())
            .filter(|nid| !notes.keys.contains_key(nid))
            .copied()
            .collect();
        (nids, held)
    };
    let wanted: Vec<u64> = nids.iter().chain(&held).copied().collect();
    let (ids, registered) =
        reconcile_children(store, source.track_id, NOTE, &wanted, |store, id, nid| {
            put(
                store,
                shared,
                id,
                f::NOTE_TRACK,
                Value::Instance(source.track_id),
            );
            put(store, shared, id, f::NOTE_NID, number(nid as f64));
        });
    changed |= registered;
    let mut rows = Vec::with_capacity(listed.len());
    {
        let selection = sources.piano_roll_selection.lock().unwrap();
        for ((key, voice, note, velocity), (nid, id)) in listed.iter().zip(nids.iter().zip(&ids)) {
            let Some(id) = *id else {
                continue;
            };
            let item = piano_roll_item_id(key.step, *voice);
            let selected = selection.contains(&item);
            let fields = [
                (f::NOTE_PITCH, number(note.transpose.round())),
                (f::NOTE_START, number(key.step as f32 + note.delay)),
                (f::NOTE_LENGTH, number(note.duration)),
                (f::NOTE_VELOCITY, number(*velocity)),
                (f::NOTE_SELECTED, Value::Bool(selected)),
                (f::NOTE_LABEL, Value::String(piano_roll_note_label(note))),
                (f::NOTE_HIDDEN, Value::Bool(false)),
                (f::NOTE_ITEM, number(item as f64)),
            ];
            for (field, value) in fields {
                changed |= put(store, shared, id, field, value);
            }
            rows.push(NoteRow {
                id,
                nid: *nid,
                key: *key,
                voice: *voice,
                length: note.duration,
                velocity: *velocity,
            });
        }
    }
    for id in ids[nids.len()..].iter().flatten() {
        changed |= put(store, shared, *id, f::NOTE_HIDDEN, Value::Bool(true));
    }
    let notes = &mut shared.borrow_mut().notes;
    // A gone note's key names no note any more: a later note there gets a
    // fresh id, never the old handle.
    notes.keep_only(&wanted.iter().copied().collect());
    notes.rows = rows;
    notes.syncs += 1;
    let listed = notes.rows.iter().map(|row| row.id).collect();
    (listed, changed)
}

/// Push `note.selected` of every listed note from `selection` (the legacy
/// piano roll's item ids).
fn push_selected(pusher: &mut Pusher<'_>, selection: &HashSet<u64>) {
    let rows = pusher.shared.borrow().notes.rows.clone();
    for row in rows {
        let selected = selection.contains(&piano_roll_item_id(row.key.step, row.voice));
        pusher.push(row.id, f::NOTE_SELECTED, Value::Bool(selected));
    }
}

/// `source`'s steps: each one's notes and parameters (one batch read, which
/// the notes and the focus steps share).
pub(super) fn source_rows(sources: &KindsHandles, source: NoteSource) -> Vec<StepRow> {
    let lanes = source.lanes(&sources.state);
    lanes.step_rows_batch(lanes.num_steps())
}

/// A cold read of `piano-roll.notes` (the reader hook): registers the notes
/// the first time; afterwards the model field answers (`None`) once the tick
/// pushed it.
pub(super) fn cold_piano_roll_notes<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
) -> Option<Value> {
    let source = {
        let shared = shared.borrow();
        if shared.skip.contains(&f::PIANO_ROLL_NOTES) {
            return None;
        }
        let notes = &shared.notes;
        if notes.registered {
            if notes.pushed {
                return None;
            }
            return Some(instance_list(notes.rows.iter().map(|row| row.id)));
        }
        notes.focus?
    };
    shared.borrow_mut().notes.registered = true;
    let rows = source_rows(sources, source);
    let (listed, _) = sync_notes(store, sources, shared, source, &rows, false, false);
    shared.borrow_mut().count(f::PIANO_ROLL_NOTES);
    Some(instance_list(listed))
}

/// `piano-roll.playhead`: a live focus's playhead step (the raw track
/// playhead, as `App::focus_playhead_step` passes it), else the
/// pinned focus's as last computed by the tick; -1 without a track.
pub(super) fn piano_roll_playhead(sources: &KindsHandles, shared: &RefCell<KindsShared>) -> f64 {
    let notes = &shared.borrow().notes;
    match notes.focus {
        Some(source) if source.focus == PianoRollFocusSpec::Live => {
            let playheads = &sources.state.transport.track_playheads;
            (playheads.get(source.track)).map_or(-1.0, |head| head.load(Ordering::Relaxed) as f64)
        }
        Some(_) => notes.playhead,
        None => -1.0,
    }
}

/// What the focus fields were last pushed under (compared every tick).
#[derive(Clone, Copy, PartialEq)]
struct FocusKey {
    roll: InstanceId,
    source: Option<NoteSource>,
    clip: Option<u64>,
    clip_kind: Option<&'static str>,
    song: u64,
    scenes: u64,
    epoch: u64,
    num_steps: usize,
    structure: u64,
}

/// What the source's notes (and its focus steps) were last read under:
/// re-read when it moves.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ContentKey {
    source: NoteSource,
    scenes: u64,
    pool: u64,
    epoch: u64,
    num_steps: usize,
    /// A live focus's source track published again
    /// ([`PianoRollState::publishes`]); 0 for a pinned one.
    publishes: u64,
    /// `App::history_replays` (an undo or redo).
    replays: u64,
    /// `App::focus_step_edits`: a setter's, a script drag's or a rolled
    /// back edit (a pinned source's pool writes move no other counter).
    edits: u64,
}

/// The piano roll sync's state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct PianoRollState {
    focus: Option<FocusKey>,
    notes: Option<ContentKey>,
    /// What the focus steps were last read under.
    pub(super) steps: Option<ContentKey>,
    /// The scheduler snapshot version last looked at, and the source
    /// track's published snapshot then; how many times that snapshot was
    /// another one (a live focus's content moved).
    snapshot: Option<(u64, Option<Arc<SequencerTrackSnapshot>>)>,
    publishes: u64,
    /// The selection last pushed.
    selection: Option<HashSet<u64>>,
    /// `App::history_replays` as the notes were last read.
    replays: u64,
    /// Focus field pushes, for tests.
    pub(crate) focus_syncs: u64,
}

impl PianoRollState {
    /// Sync everything at the next tick (a schema change).
    pub(super) fn invalidate(&mut self) {
        self.focus = None;
        self.notes = None;
        self.steps = None;
        self.selection = None;
    }
}

/// The fields a piano roll's observed mask covers: its live fields (bit
/// `i` is `PIANO_ROLL_LIVE.keys[i]`), then `notes` ([`NOTES_BIT`]) and
/// `steps` ([`STEPS_BIT`]).
static PIANO_ROLL_OBSERVED: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut names = PIANO_ROLL_LIVE.names.clone();
    names.extend([f::PIANO_ROLL_NOTES.1, f::PIANO_ROLL_STEPS.1]);
    names
});
static NOTES_BIT: LazyLock<ObservedMask> = LazyLock::new(|| {
    assert!(
        PIANO_ROLL_LIVE.keys.len() + 2 <= MAX_OBSERVED_FIELDS,
        "piano-roll live fields + notes + steps exceed the {MAX_OBSERVED_FIELDS}-bit observed \
         mask: widen ObservedMask"
    );
    1 << PIANO_ROLL_LIVE.keys.len()
});
static STEPS_BIT: LazyLock<ObservedMask> = LazyLock::new(|| *NOTES_BIT << 1);

impl HostKinds {
    /// The piano roll: its focus fields when [`FocusKey`] moved, its notes
    /// and focus steps once registered when their [`ContentKey`] moved, the
    /// selection when it changed, the playhead while observed. Runs after
    /// the song sync (`piano-roll.clip` is a clip instance).
    pub(super) fn sync_piano_roll(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let Some(roll) = pusher.singleton(PIANO_ROLL) else {
            return;
        };
        let (sources, shared) = (pusher.sources, pusher.shared);
        let current = sources.current_track.load(Ordering::Relaxed);
        let track_id = self.track_ids.get(current).copied().flatten();
        let source = (track_id.filter(|_| sources.track_exists(current)))
            .map(|id| NoteSource::resolve(app, current, id));
        let stale = {
            let mut shared = shared.borrow_mut();
            shared.notes.focus = source;
            let first = shared.notes.rows.first();
            first.is_some_and(|row| !pusher.rt.instance_is_live(row.id))
        };
        if stale {
            // A hot reload dropped the note instances.
            shared.borrow_mut().notes.reset();
            self.piano_roll.notes = None;
        }
        if shared.borrow_mut().focus_steps.drop_if_stale(pusher.rt) {
            self.piano_roll.steps = None;
        }
        self.sync_piano_roll_focus(pusher, app, roll, source);
        let mask = pusher.rt.host_fields_observed(roll, &PIANO_ROLL_OBSERVED);
        if mask & *NOTES_BIT != 0 {
            shared.borrow_mut().notes.registered = true;
        }
        if mask & *STEPS_BIT != 0 {
            shared.borrow_mut().focus_steps.register();
        }
        let (notes, steps) = {
            let shared = shared.borrow();
            (shared.notes.registered, shared.focus_steps.registered())
        };
        if let Some(source) = source.filter(|_| notes || steps) {
            let key = self.content_key(pusher.sources, app, source);
            let notes_due = notes && self.notes_due(shared, app, source, key);
            let steps_due = steps && self.steps_due(shared, key);
            // One read of the source for both.
            let rows = (notes_due || steps_due).then(|| source_rows(sources, source));
            if notes {
                let rows = rows.as_deref().filter(|_| notes_due);
                self.sync_piano_roll_notes(pusher, app, roll, source, key, rows);
            }
            if let Some(rows) = rows.as_deref().filter(|_| steps_due) {
                self.sync_piano_roll_steps(pusher, roll, source, key, rows);
            }
        }
        let playhead = PIANO_ROLL_LIVE.bit(f::PIANO_ROLL_PLAYHEAD);
        let pinned = source.filter(|source| source.focus != PianoRollFocusSpec::Live);
        if mask & playhead == 0 && pinned.is_some() && !sources.state.is_playing() {
            // Stopped, a pinned source shows no playhead (an atomic load:
            // a cold read sees -1, not a stale step).
            shared.borrow_mut().notes.playhead = -1.0;
        }
        if mask & playhead != 0 {
            let playheads = &sources.state.transport.track_playheads;
            let pinned = pinned.and_then(|source| Some((source, playheads.get(source.track)?)));
            if let Some((source, head)) = pinned {
                let head = head.load(Ordering::Relaxed) as usize;
                let step = app.focus_playhead_step(source.track, head);
                shared.borrow_mut().notes.playhead = step.unwrap_or(-1.0);
            }
            let value = piano_roll_playhead(sources, shared);
            pusher.push_computed(roll, f::PIANO_ROLL_PLAYHEAD, number(value));
        }
    }

    /// The focus fields, when [`FocusKey`] moved.
    fn sync_piano_roll_focus(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        roll: InstanceId,
        source: Option<NoteSource>,
    ) {
        let state = &app.state;
        let track = source.map(|source| source.track);
        let clip_kind = track.and_then(|track| app.focus_clip_source_kind(track));
        let clip = clip_kind
            .and(app.song_clip_selection)
            .map(|selection| selection.clip_id.0);
        let key = FocusKey {
            roll,
            source,
            clip,
            clip_kind,
            song: state.committed_song_revision(),
            scenes: state.project_scenes_revision(),
            epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            num_steps: track.map_or(0, |track| pusher.sources.num_steps(track)),
            structure: self.song.generation(),
        };
        let state = &mut self.piano_roll;
        if !moved(&mut state.focus, key, &mut state.focus_syncs) {
            return;
        }
        let focus = source.map_or(PianoRollFocusSpec::Live, |source| source.focus);
        let clip_id = (source.zip(clip))
            .and_then(|(source, clip)| pusher.rt.keyed_instance(CLIP, &[source.track_id, clip]));
        let (label, num_steps, (marker, span, repeat)) = match track {
            Some(track) => (
                app.focus_label(track).unwrap_or_default(),
                app.focus_num_steps(track),
                piano_roll_window(app, track),
            ),
            None => (String::new(), 0, (-1.0, None, 0.0)),
        };
        let track_id = source.map(|source| source.track_id);
        pusher.push(roll, f::PIANO_ROLL_TRACK, instance_or_nil(track_id));
        pusher.push(roll, f::PIANO_ROLL_FOCUS_KIND, text(focus.kind_name()));
        pusher.push(
            roll,
            f::PIANO_ROLL_CLIP_KIND,
            text(clip_kind.unwrap_or("none")),
        );
        pusher.push(roll, f::PIANO_ROLL_CLIP, instance_or_nil(clip_id));
        pusher.push(roll, f::PIANO_ROLL_FOCUS_LABEL, Value::String(label));
        pusher.push(
            roll,
            f::PIANO_ROLL_FOCUS_NUM_STEPS,
            number(num_steps as f64),
        );
        // The loop window: sentinel-shaped as the legacy fields.
        let span = match span {
            Some((start, end)) => list_value([number(start), number(end)]),
            None => list_value(Vec::<Value>::new()),
        };
        pusher.push(roll, f::PIANO_ROLL_WINDOW_MARKER, number(marker));
        pusher.push(roll, f::PIANO_ROLL_WINDOW_SPAN, span);
        pusher.push(roll, f::PIANO_ROLL_WINDOW_REPEAT, number(repeat));
    }

    /// What `source`'s content is read under now: the counters, and
    /// whether the scheduler published its track again (a live focus's
    /// notes and steps move then; compared by snapshot, once per version).
    fn content_key(
        &mut self,
        sources: &KindsHandles,
        app: &app::App,
        source: NoteSource,
    ) -> ContentKey {
        let state = &sources.state;
        let version = state.scheduler_snapshot_version();
        let roll_state = &mut self.piano_roll;
        if !matches!(&roll_state.snapshot, Some((seen, _)) if *seen == version) {
            let latest = state.latest_scheduler_snapshot();
            let track = latest.tracks.get(source.track).cloned();
            let before = (roll_state.snapshot.as_ref()).and_then(|(_, track)| track.as_ref());
            let moved = match (before, &track) {
                (Some(before), Some(now)) => !Arc::ptr_eq(before, now),
                (None, None) => false,
                _ => true,
            };
            roll_state.publishes += u64::from(moved);
            roll_state.snapshot = Some((version, track));
        }
        let live = source.focus == PianoRollFocusSpec::Live;
        ContentKey {
            source,
            scenes: state.project_scenes_revision(),
            pool: state.pool_content_revision(),
            epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            num_steps: sources.num_steps(source.track),
            publishes: if live { roll_state.publishes } else { 0 },
            replays: app.history_replays,
            edits: app.focus_step_edits,
        }
    }

    /// Whether the registered notes are to be re-read: their [`ContentKey`]
    /// moved, they were never pushed, or a script drag ended over some.
    fn notes_due(
        &self,
        shared: &RefCell<KindsShared>,
        app: &app::App,
        source: NoteSource,
        key: ContentKey,
    ) -> bool {
        let notes = &shared.borrow().notes;
        let dragging = app.active_note_drag().is_some();
        self.piano_roll.notes != Some(key)
            || !notes.pushed
            || notes.source != Some(source)
            || (!dragging && !notes.held.is_empty())
    }

    /// The registered notes, re-read from `rows` (the source's steps) when
    /// they are due ([`Self::notes_due`]); the selection when it changed.
    fn sync_piano_roll_notes(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        roll: InstanceId,
        source: NoteSource,
        key: ContentKey,
        rows: Option<&[StepRow]>,
    ) {
        let (sources, shared) = (pusher.sources, pusher.shared);
        // Compared in place: an idle tick copies nothing.
        let selected = {
            let selection = sources.piano_roll_selection.lock().unwrap();
            (rows.is_some() || self.piano_roll.selection.as_ref() != Some(&*selection))
                .then(|| selection.clone())
        };
        if let Some(rows) = rows {
            let dragging = app.active_note_drag().is_some();
            let replayed = self.piano_roll.replays != app.history_replays;
            let (listed, changed) = sync_notes(
                &mut *pusher.rt,
                sources,
                shared,
                source,
                rows,
                dragging,
                replayed,
            );
            pusher.changed |= changed;
            self.piano_roll.replays = app.history_replays;
            pusher.push(roll, f::PIANO_ROLL_NOTES, instance_list(listed));
            shared.borrow_mut().notes.pushed = true;
            self.piano_roll.notes = Some(key);
        } else if let Some(selection) = &selected {
            push_selected(pusher, selection);
        }
        if selected.is_some() {
            self.piano_roll.selection = selected;
        }
    }
}

impl HostKinds {
    /// The track instance of stable track id `tid` (the note setters'
    /// source check).
    pub(crate) fn track_instance(&self, tid: u64) -> Option<InstanceId> {
        self.tracks.get(&tid).copied()
    }
}
