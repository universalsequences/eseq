//! Stage 7e: the piano roll (`piano-roll`, `note`) and the tracker's lock
//! cells (`param.step-locks`, `rack-macro.step-locks`).

use super::*;
use sequencer::sequencer::{LaneSource, PatternId, StepParam};

const REFER_7E: &str =
    "(import eseq.kinds :refer (track piano-roll add-note! delete-notes! pitch-min pitch-max))
     (def notes () piano-roll.notes)
     (def note-at (i) (nth piano-roll.notes i))";

impl Harness {
    fn eval_7e(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7E}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Run `code`'s commands, then sync.
    fn run_7e(&mut self, code: &str) {
        self.eval_7e(code);
        self.drain();
        self.sync();
    }

    /// Run `code`'s commands; they must fail with `message` and change no
    /// history and no note.
    fn rejects_7e(&mut self, code: &str, message: &str) {
        let undo = self.app.history.undo_len();
        let notes = self.all_notes(0);
        self.editor.minibuffer = None;
        self.eval_7e(code);
        self.drain();
        let error = self.editor.minibuffer.clone().unwrap_or_default();
        assert!(error.contains(message), "{code}: {error}");
        assert_eq!(self.app.history.undo_len(), undo, "{code} recorded nothing");
        assert_eq!(self.all_notes(0), notes, "{code} changed no note");
    }

    /// Write `notes` (transpose, duration, offset) on `step` of `track`'s
    /// live pattern and publish the track, as an edit does.
    fn write_notes(&mut self, track: usize, step: usize, notes: &[(f32, f32, f32)]) {
        let lanes = PianoRollLanes::live(&self.shared.state, track);
        let notes: Vec<PianoRollNote> = (notes.iter())
            .map(|&(transpose, duration, delay)| PianoRollNote {
                transpose,
                duration,
                delay,
            })
            .collect();
        lanes.set_note_entries(step, &notes);
        self.shared.state.publish_scheduler_track(track);
    }

    /// The notes on `step` of `track`'s live pattern.
    fn live_notes(&self, track: usize, step: usize) -> Vec<(f32, f32, f32)> {
        let lanes = PianoRollLanes::live(&self.shared.state, track);
        (lanes.note_entries(step).iter())
            .map(|note| (note.transpose, note.duration, note.delay))
            .collect()
    }

    /// Every (step, note) of `track`'s live pattern.
    fn all_notes(&self, track: usize) -> Vec<(usize, (f32, f32, f32))> {
        (0..16)
            .flat_map(|step| {
                let notes = self.live_notes(track, step);
                notes.into_iter().map(move |note| (step, note))
            })
            .collect()
    }

    fn step_velocity(&self, track: usize, step: usize) -> f32 {
        self.shared.state.pattern.step_data[track].get(step, StepParam::Velocity)
    }

    /// Publish the legacy piano roll fields as the reactive tick does.
    fn publish_legacy_piano_roll(&mut self) {
        let state = self.shared.state.clone();
        let selection = self.shared.piano_roll_selection.clone();
        let track = self.shared.current_track.load(Ordering::Relaxed);
        let rt = self.editor.runtime_mut();
        sync_piano_roll_state(rt, &self.app, &state, track, &selection);
        sync_piano_roll_playhead(rt, &self.app, track, 0);
    }

    fn seq_7e(&self, field: &str) -> Value {
        self.rt()
            .reactive_field_value("SEQ", field)
            .unwrap_or_else(|| panic!("SEQ.{field}"))
            .clone()
    }

    fn note_syncs(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().notes.syncs
    }

    fn note_instances(&self) -> usize {
        let tracks = [self.track_id(0), self.track_id(1)];
        (tracks.iter())
            .map(|track| self.rt().keyed_children_of_kind(*track, NOTE).count())
            .sum()
    }

    fn drag_frames(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().notes.drag_frames
    }

    fn live(&self, id: InstanceId) -> bool {
        self.rt().instance_is_live(id)
    }

    /// Esc, as the event loop handles it with a script note drag open.
    fn cancel_note_drag(&mut self) -> Option<Result<(), app::edit::EditError>> {
        let mut ctx = LoopCtx {
            sessions: &mut self.sessions,
            meters: &mut self.meters,
            frame: &mut self.frame,
            gesture: &mut self.gesture,
            track_names: &mut self.track_names,
            shared: &self.shared,
        };
        crate::host_commands::notes::cancel_note_drag(&mut self.app, &mut ctx)
    }

    fn switch_scene(&mut self, index: usize) {
        self.eval_7e(&format!(
            "(host-command \"switch-pattern\" (dict :idx {index} :quantize \"off\"))"
        ));
        self.drain();
        assert_eq!(self.app.state.current_scene_index(), index);
        self.sync();
    }

    fn instance_7e(&mut self, code: &str) -> InstanceId {
        match self.eval_7e(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }
}

/// What a delete of a dropped note reports.
const STALE: &str = "delete-notes: the note is gone";

fn entry(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone()),
        other => panic!("not a map: {other:?}"),
    }
}

fn list(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        other => panic!("not a list: {other:?}"),
    }
}

fn near(value: Value, expected: f64) -> bool {
    (num(value) - expected).abs() < 1e-5
}

#[test]
fn piano_roll_fields_read_after_sync_and_match_the_legacy_fields() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0), (7.0, 2.0, 0.0)]);
    h.write_notes(0, 3, &[(-5.0, 0.5, 0.25)]);
    h.sync();
    h.sync();
    assert_eq!(h.note_instances(), 0, "nothing read the notes yet");
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(3.0));
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1)) (def c (note-at 2))");
    assert_eq!(h.eval_7e("piano-roll.track"), h.eval_7e("(track 0)"));
    assert_eq!(h.eval_7e("a.track"), h.eval_7e("(track 0)"));
    assert_eq!(h.eval_7e("piano-roll.focus-kind"), s("live"));
    assert_eq!(h.eval_7e("piano-roll.clip-kind"), s("none"));
    assert_eq!(h.eval_7e("piano-roll.clip"), Value::Nil);
    assert_eq!(h.eval_7e("piano-roll.focus-num-steps"), Value::Number(16.0));
    assert_eq!(h.eval_7e("piano-roll.window-marker"), Value::Number(-1.0));
    assert_eq!(
        h.eval_7e("(len piano-roll.window-span)"),
        Value::Number(0.0)
    );
    assert_eq!(h.eval_7e("piano-roll.window-repeat"), Value::Number(0.0));
    assert_eq!(h.eval_7e("piano-roll.playhead"), Value::Number(0.0));
    assert_eq!(
        h.eval_7e("(list a.pitch b.pitch c.pitch)"),
        h.eval_7e("(list 0 7 -5)")
    );
    assert_eq!(h.eval_7e("c.start"), Value::Number(3.25));
    assert_eq!(h.eval_7e("b.length"), Value::Number(2.0));
    assert_eq!(h.eval_7e("c.velocity"), Value::Number(1.0));
    assert_eq!(h.eval_7e("c.label"), s("G3 +0.25"));
    assert_eq!(
        h.eval_7e("(list pitch-min pitch-max)"),
        h.eval_7e("(list -48 48)")
    );
    assert_eq!(PIANO_ROLL_MIN_TRANSPOSE, -48);
    assert_eq!(PIANO_ROLL_MAX_TRANSPOSE, 48);
    // Legacy parity: the items (id order = step, then voice), selection and
    // the focus fields.
    h.shared
        .piano_roll_selection
        .lock()
        .unwrap()
        .insert(piano_roll_item_id(0, 1));
    h.sync();
    h.publish_legacy_piano_roll();
    let items = list(h.seq_7e("piano-roll-items"));
    assert_eq!(items.len(), 3);
    for (index, item) in items.iter().enumerate() {
        let mut field = |code: &str| h.eval_7e(&format!("(let ((n (note-at {index}))) {code})"));
        assert_eq!(field("n.start"), entry(item, "start"));
        let end = num(field("(+ n.start n.length)"));
        assert!(near(entry(item, "end"), end), "{index}");
        assert_eq!(field("(- pitch-max n.pitch)"), entry(item, "lane"));
        assert_eq!(field("n.label"), entry(item, "label"));
        assert_eq!(field("n.selected"), entry(item, "selected"));
    }
    assert_eq!(h.eval_7e("b.selected"), Value::Bool(true));
    assert_eq!(h.eval_7e("piano-roll.focus-label"), h.seq_7e("focus-label"));
    assert_eq!(
        h.eval_7e("piano-roll.focus-num-steps"),
        h.seq_7e("focus-num-steps")
    );
    assert_eq!(h.seq_7e("focus-kind"), Value::Keyword("live".to_string()));
    assert_eq!(h.seq_7e("focus-clip-start"), Value::Nil);
    // A pinned clip: its pattern is not the effective one (a cloned scene
    // plays a copy), so the piano roll edits the pool pattern.
    h.command("clone-pattern", Value::Nil);
    h.sync();
    let source = LaneSource::Pattern(PatternId(1));
    let clip = h
        .app
        .arr_clip_create(0, 4.0, 6.0, source, 2.0)
        .expect("clip");
    h.app.set_arrangement_view_visible(true);
    h.app
        .select_song_clip_span(0, clip, Some((4.0, 6.0)))
        .expect("select");
    h.sync();
    assert_eq!(h.eval_7e("piano-roll.focus-kind"), s("pattern"));
    assert_eq!(h.eval_7e("piano-roll.clip-kind"), s("pattern"));
    let first_clip = "(first (let ((t (track 0))) t.clips))";
    assert_eq!(h.eval_7e("piano-roll.clip"), h.eval_7e(first_clip));
    assert_eq!(h.eval_7e("piano-roll.clip.offset"), Value::Number(2.0));
    h.publish_legacy_piano_roll();
    assert_eq!(h.eval_7e("piano-roll.focus-label"), h.seq_7e("focus-label"));
    assert_eq!(
        h.eval_7e("piano-roll.window-marker"),
        h.seq_7e("focus-window-marker")
    );
    assert_eq!(
        h.eval_7e("piano-roll.window-repeat"),
        h.seq_7e("focus-window-repeat")
    );
    let span = h.seq_7e("focus-window-span");
    assert_eq!(h.eval_7e("piano-roll.window-span"), span);
    assert_eq!(
        h.eval_7e("piano-roll.clip.start"),
        h.seq_7e("focus-clip-start")
    );
    assert_eq!(h.eval_7e("piano-roll.playhead"), Value::Number(-1.0));
    // The pool pattern's notes, as the legacy items list them.
    let items = list(h.seq_7e("piano-roll-items"));
    assert_eq!(
        h.eval_7e("(len (notes))"),
        Value::Number(items.len() as f64)
    );
    // Another source: the live pattern's handles went stale.
    let Value::Instance(a) = h.eval_7e("a") else {
        panic!("a")
    };
    assert!(!h.rt().instance_is_live(a));
}

#[test]
fn note_setters_change_the_model_through_history_with_undo() {
    let mut h = Harness::new();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 5, &[(3.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    h.sync();
    let a = h.instance_7e("a");
    let base = h.app.history.undo_len();
    h.run_7e("(set! a.pitch 4)");
    assert_eq!(h.live_notes(0, 2), vec![(4.0, 1.0, 0.0)]);
    assert_eq!(h.eval_7e("a.pitch"), Value::Number(4.0));
    assert_eq!(h.eval_7e("a.label"), s("E4"));
    h.run_7e("(set! a.start 6.5)");
    assert_eq!(h.live_notes(0, 2), vec![]);
    assert_eq!(h.live_notes(0, 6), vec![(4.0, 1.0, 0.5)]);
    assert_eq!(h.eval_7e("a.start"), Value::Number(6.5));
    // The handle followed its note (now after b).
    assert_eq!(h.instance_7e("(note-at 1)"), a);
    h.run_7e("(set! a.length 3)");
    assert_eq!(h.live_notes(0, 6), vec![(4.0, 3.0, 0.5)]);
    h.run_7e("(set! a.velocity 0.5)");
    assert_eq!(h.step_velocity(0, 6), 0.5);
    assert_eq!(h.eval_7e("a.velocity"), Value::Number(0.5));
    assert_eq!(h.app.history.undo_len(), base + 4, "an undo entry per edit");
    assert!(h.app.history.active_gesture().is_none());
    // The value a note has changes nothing.
    h.run_7e("(set! a.pitch 4) (set! a.start 6.5) (set! a.length 3) (set! a.velocity 0.5)");
    assert_eq!(h.app.history.undo_len(), base + 4);
    // A note alone on its new step keeps its velocity.
    h.run_7e("(set! a.start 9)");
    assert_eq!(h.step_velocity(0, 9), 0.5);
    // Selection: no history.
    h.run_7e("(set! b.selected true)");
    let selected = h.shared.piano_roll_selection.lock().unwrap().clone();
    assert_eq!(selected, HashSet::from([piano_roll_item_id(5, 0)]));
    assert_eq!(h.eval_7e("b.selected"), Value::Bool(true));
    // A selected note stays selected where it moves.
    h.run_7e("(set! b.start 1)");
    let selected = h.shared.piano_roll_selection.lock().unwrap().clone();
    assert_eq!(selected, HashSet::from([piano_roll_item_id(1, 0)]));
    assert_eq!(h.eval_7e("b.selected"), Value::Bool(true));
    h.run_7e("(set! b.selected false)");
    assert!(h.shared.piano_roll_selection.lock().unwrap().is_empty());
    assert_eq!(h.app.history.undo_len(), base + 6);
    // Adding and deleting notes: one entry each.
    h.run_7e("(add-note! 12.5 -12 2 :velocity 0.25)");
    assert_eq!(h.live_notes(0, 12), vec![(-12.0, 2.0, 0.5)]);
    assert_eq!(h.step_velocity(0, 12), 0.25);
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(3.0));
    h.run_7e("(delete-notes! (list b (note-at 2)))");
    assert_eq!(h.all_notes(0), vec![(9, (4.0, 3.0, 0.0))]);
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(1.0));
    let Value::Instance(b) = h.eval_7e("b") else {
        panic!("b")
    };
    assert!(
        !h.rt().instance_is_live(b),
        "a deleted note's handle goes stale"
    );
    assert_eq!(h.app.history.undo_len(), base + 8);
    // Undo restores, newest first.
    app::edit::undo(&mut h.app);
    assert_eq!(h.live_notes(0, 1), vec![(3.0, 1.0, 0.0)]);
    while h.app.history.undo_len() > base {
        app::edit::undo(&mut h.app);
    }
    assert_eq!(
        h.all_notes(0),
        vec![(2, (0.0, 1.0, 0.0)), (5, (3.0, 1.0, 0.0))]
    );
    h.sync();
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(2.0));
    assert_eq!(
        h.eval_7e("(let ((n (note-at 0))) n.start)"),
        Value::Number(2.0)
    );
}

#[test]
fn note_setters_take_their_ranges_and_reject_the_rest() {
    let mut h = Harness::new();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 3, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    h.sync();
    h.rejects_7e("(set! a.pitch 49)", "an integer from -48 to 48");
    h.rejects_7e("(set! a.start -1)", "a number from 0");
    h.rejects_7e("(set! a.start 16)", "a step from 0 to below 16");
    h.rejects_7e("(set! a.length 0)", "a number from 0.03125 to 32");
    h.rejects_7e("(set! a.velocity 2)", "a number from 0 to 1");
    h.rejects_7e("(add-note! 1 0 0)", "a number from 0.03125 to 32");
    // Wrong types fail at set!.
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_7E}\n(set! a.pitch \"x\")"))
        .expect_err("type");
    assert!(format!("{error:?}").contains(":int"), "{error:?}");
    // A note gone when the command lands is an error, never another note.
    h.eval_7e("(set! b.pitch 1)");
    h.write_notes(0, 3, &[(5.0, 1.0, 0.0)]);
    h.rejects_7e("nil", "the note is gone");
    assert_eq!(h.live_notes(0, 3), vec![(5.0, 1.0, 0.0)]);
    h.write_notes(0, 3, &[]);
    h.sync();
    // A stale handle's set! is a silent no-op (a dropped instance takes no
    // write, spec §4 "stale self"); its delete is an error.
    h.rejects_7e("(set! b.pitch 1)", "");
    assert_eq!(h.editor.minibuffer, None, "no error, no command");
    h.rejects_7e("(delete-notes! (list a b))", STALE);
    // A moved note replaces the one it lands on.
    h.write_notes(0, 7, &[(0.0, 2.0, 0.0)]);
    h.sync();
    h.eval_7e("(def c (note-at 1))");
    h.run_7e("(set! a.start 7)");
    assert_eq!(h.live_notes(0, 7), vec![(0.0, 1.0, 0.0)]);
    let Value::Instance(c) = h.eval_7e("c") else {
        panic!("c")
    };
    assert!(!h.rt().instance_is_live(c));
    assert_eq!(h.eval_7e("a.start"), Value::Number(7.0));
}

#[test]
fn a_script_drag_of_notes_is_one_entry_rebuilt_from_where_it_started() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 2.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    h.sync();
    let (a, b) = (h.instance_7e("a"), h.instance_7e("b"));
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    // A lands on B: B is replaced while A lies on it, ...
    h.run_7e("(set! a.start 4)");
    assert_eq!(h.live_notes(0, 0), vec![]);
    assert_eq!(h.live_notes(0, 4), vec![(0.0, 2.0, 0.0)]);
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(1.0));
    assert!(h.rt().instance_is_live(b), "held while A lies on it");
    // ... and back, with its handle, once A moves on.
    h.run_7e("(set! a.start 8)");
    assert_eq!(h.live_notes(0, 4), vec![(0.0, 1.0, 0.0)]);
    assert_eq!(h.live_notes(0, 8), vec![(0.0, 2.0, 0.0)]);
    assert_eq!(h.instance_7e("(note-at 0)"), b);
    assert_eq!(h.instance_7e("(note-at 1)"), a);
    // Other fields and other notes join the same entry.
    h.run_7e("(set! a.pitch 5) (set! b.length 3) (set! a.velocity 0.25)");
    assert_eq!(h.live_notes(0, 8), vec![(5.0, 2.0, 0.0)]);
    assert_eq!(h.live_notes(0, 4), vec![(0.0, 3.0, 0.0)]);
    assert_eq!(h.step_velocity(0, 8), 0.25);
    // A rejected frame changes nothing and keeps the targets.
    h.rejects_7e("(set! a.start 99)", "a number from 0");
    assert_eq!(h.live_notes(0, 8), vec![(5.0, 2.0, 0.0)]);
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    assert_eq!(
        h.app.history.undo_len(),
        undo + 1,
        "the whole drag is one entry"
    );
    assert_eq!(h.eval_7e("a.pitch"), Value::Number(5.0));
    assert_eq!(h.eval_7e("b.length"), Value::Number(3.0));
    // Undo restores the notes before the drag.
    app::edit::undo(&mut h.app);
    assert_eq!(
        h.all_notes(0),
        vec![(0, (0.0, 2.0, 0.0)), (4, (0.0, 1.0, 0.0))]
    );
    // A drag that ends on another note replaces it once.
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7e("(set! a.start 2) (set! a.start 4)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    assert_eq!(h.all_notes(0), vec![(4, (0.0, 2.0, 0.0))]);
    let Value::Instance(b) = h.eval_7e("b") else {
        panic!("b")
    };
    assert!(!h.rt().instance_is_live(b), "the note it ended on is gone");
    assert_eq!(h.app.history.undo_len(), undo + 1);
}

#[test]
fn notes_keep_identity_across_edits_and_go_on_a_new_source_or_project() {
    let mut h = Harness::new();
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.write_notes(1, 1, &[(0.0, 1.0, 0.0), (4.0, 1.0, 0.0)]);
    h.write_notes(1, 6, &[(2.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1)) (def c (note-at 2))");
    h.sync();
    let (a, b, c) = (h.instance_7e("a"), h.instance_7e("b"), h.instance_7e("c"));
    // A legacy piano roll delete of b: b goes, a and c keep their handles.
    let id = piano_roll_item_id(1, 1);
    h.run_7e(&format!(
        "(host-command \"piano-roll-history-action\"
           (dict :track 1 :action (dict :type :delete-items :ids (list {id}))))"
    ));
    assert_eq!(h.live_notes(1, 1), vec![(0.0, 1.0, 0.0)]);
    assert!(!h.rt().instance_is_live(b));
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    assert_eq!(h.instance_7e("(note-at 1)"), c);
    // A note recreated where b was is another note.
    h.write_notes(1, 1, &[(0.0, 1.0, 0.0), (4.0, 1.0, 0.0)]);
    h.sync();
    assert_ne!(h.instance_7e("(note-at 1)"), b);
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    // Deleting the track in front moves the track: its notes stay theirs.
    let t1 = h.track_id(1);
    h.app.delete_track_recorded(0).expect("delete");
    h.shared.current_track.store(0, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_7e("piano-roll.track"), Value::Instance(t1));
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    assert_eq!(h.instance_7e("(note-at 2)"), c);
    // A setter lands on its note after the reorder.
    h.run_7e("(set! a.pitch 1)");
    assert_eq!(h.live_notes(0, 1)[0].0, 1.0);
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    // Another track's notes replace them.
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    h.shared.current_track.store(0, Ordering::Relaxed);
    h.sync();
    assert!(!h.rt().instance_is_live(a));
    assert!(!h.rt().instance_is_live(c));
    // A project load replaces everything.
    h.write_notes(0, 3, &[(1.0, 1.0, 0.0)]);
    h.sync();
    let d = h.instance_7e("(note-at 0)");
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(d));
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(0.0));
}

#[test]
fn piano_roll_feeds_cost_nothing_while_idle_or_unobserved() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.note_syncs(), 0, "notes register on the first read");
    assert_eq!(h.computed(f::PIANO_ROLL_PLAYHEAD), 0);
    let focus = h.frame.host_kinds.piano_roll.focus_syncs;
    h.sync();
    assert_eq!(
        h.frame.host_kinds.piano_roll.focus_syncs, focus,
        "an idle tick"
    );
    h.eval_7e("(def a (note-at 0))");
    h.sync();
    let syncs = h.note_syncs();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.note_syncs(), syncs, "idle ticks re-read no notes");
    // A mixer edit moves no note counter.
    h.shared.state.pattern.track_params[0].set_volume(0.3);
    h.sync();
    assert_eq!(h.note_syncs(), syncs);
    // A selection change pushes selected, re-reading nothing.
    h.shared
        .piano_roll_selection
        .lock()
        .unwrap()
        .insert(piano_roll_item_id(0, 0));
    h.sync();
    assert_eq!(h.note_syncs(), syncs);
    assert_eq!(h.eval_7e("a.selected"), Value::Bool(true));
    // A note edit re-reads them once.
    h.write_notes(0, 2, &[(1.0, 1.0, 0.0)]);
    h.sync();
    h.sync();
    assert_eq!(h.note_syncs(), syncs + 1);
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(2.0));
    // The playhead: computed per tick only while observed.
    h.shared.state.transport.track_playheads[0].store(5, Ordering::Relaxed);
    assert_eq!(h.eval_7e("piano-roll.playhead"), Value::Number(5.0));
    let cold = h.computed(f::PIANO_ROLL_PLAYHEAD);
    h.sync();
    assert_eq!(h.computed(f::PIANO_ROLL_PLAYHEAD), cold);
    h.eval_7e("(def ph #'piano-roll.playhead)");
    h.sync();
    let observed = h.computed(f::PIANO_ROLL_PLAYHEAD);
    h.shared.state.transport.track_playheads[0].store(6, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("ph"), 6.0);
    assert_eq!(h.computed(f::PIANO_ROLL_PLAYHEAD), observed + 1);
    h.eval_7e("(set! ph nil)");
    h.sync();
    h.sync();
    let released = h.computed(f::PIANO_ROLL_PLAYHEAD);
    h.sync();
    assert_eq!(h.computed(f::PIANO_ROLL_PLAYHEAD), released);
}

#[test]
fn param_step_locks_list_the_patterns_locks_while_observed() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(
        r#"(def t0 (track 0)) (def flt (first t0.devices))
           (def cutoff (device-param flt "cutoff"))
           (def s3 (nth t0.steps 3)) (def s7 (nth t0.steps 7))"#,
    );
    assert_eq!(h.eval_all("(len cutoff.step-locks)"), Value::Number(0.0));
    h.eval_all("(lock-param! cutoff (list s3 s7) 800)");
    h.drain();
    h.sync();
    let locks = h.eval_all("cutoff.step-locks");
    assert_eq!(locks, h.eval_all("(list (list 3 800) (list 7 800))"));
    // Parity with the tracker's legacy cells: the column's values.
    let state = h.shared.state.clone();
    let rows = list(build_tracker_rows_value(&h.app, &state));
    let cell = |step: usize| list(list(rows[step].clone())[0].clone());
    assert_eq!(cell(3).last().cloned(), Some(Value::Number(800.0)));
    assert_eq!(cell(4).last().cloned(), Some(Value::Nil));
    assert_eq!(h.filter_slot(slot).plocks.get(3, 2), Some(800.0));
    // Computed only while observed, then when the track's p-locks moved.
    let cold = h.computed(f::PARAM_STEP_LOCKS);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::PARAM_STEP_LOCKS), cold);
    h.eval_all(r#"(effect-buffer "*observe*" (label (str cutoff.step-locks)))"#);
    h.show_all();
    h.sync();
    let observed = h.computed(f::PARAM_STEP_LOCKS);
    h.sync();
    assert_eq!(
        h.computed(f::PARAM_STEP_LOCKS),
        observed,
        "its p-lock key did not move"
    );
    h.lock_effect(slot, 9, 2, 1200.0);
    h.sync();
    let moved = h.computed(f::PARAM_STEP_LOCKS);
    assert!(moved > observed, "recomputed when the p-locks moved");
    assert_eq!(
        h.eval_all("cutoff.step-locks"),
        h.eval_all("(list (list 3 800) (list 7 800) (list 9 1200))")
    );
    // The re-rendered observer settles; idle ticks compute nothing.
    h.sync();
    let settled = h.computed(f::PARAM_STEP_LOCKS);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::PARAM_STEP_LOCKS), settled);
}

#[test]
fn rack_macro_step_locks_list_the_patterns_locks() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_7e("(def rk (let ((t (track 2))) (first t.devices))) (def rm (nth rk.macros 1))");
    assert_eq!(h.eval_7e("(len rm.step-locks)"), Value::Number(0.0));
    for (step, value) in [(3, 0.25), (5, 0.5)] {
        let command = app::AppCommand::SetRackMacroPlockMulti {
            track: 2,
            steps: vec![step],
            macro_idx: 1,
            value,
        };
        app::apply_command(&mut h.app, command);
    }
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_7e("rm.step-locks"),
        h.eval_7e("(list (list 3 0.25) (list 5 0.5))")
    );
    assert_eq!(h.eval_7e("rm.has-locks"), Value::Bool(true));
}

#[test]
fn undo_and_redo_make_note_handles_stale() {
    let mut h = Harness::new();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 7, &[(0.0, 2.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def c (note-at 1))");
    h.sync();
    let (a, c) = (h.instance_7e("a"), h.instance_7e("c"));
    h.run_7e("(set! a.start 7)");
    assert!(!h.live(c), "a replaced c");
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    // Undo puts c back at a's key: a goes stale, never showing c's data.
    app::edit::undo(&mut h.app);
    h.sync();
    assert!(!h.live(a), "undo made a stale");
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(2.0));
    let restored = h.instance_7e("(note-at 1)");
    assert_ne!(restored, a);
    assert_eq!(
        h.eval_7e("(let ((n (note-at 1))) n.length)"),
        Value::Number(2.0)
    );
    // Redo: every handle of the source is fresh again.
    let before = h.instance_7e("(note-at 0)");
    app::edit::redo(&mut h.app);
    h.sync();
    assert!(!h.live(before) && !h.live(restored));
    assert_eq!(h.all_notes(0), vec![(7, (0.0, 1.0, 0.0))]);
    assert_eq!(
        h.eval_7e("(let ((n (note-at 0))) n.start)"),
        Value::Number(7.0)
    );
    // An undo that changes no note keeps the handles.
    let kept = h.instance_7e("(note-at 0)");
    h.shared.state.pattern.track_params[0].set_volume(0.3);
    app::edit::undo(&mut h.app);
    app::edit::redo(&mut h.app);
    h.sync();
    assert!(h.live(kept), "the notes are as they were");
    // A drag ending on another note, undone and redone.
    app::edit::undo(&mut h.app);
    h.sync();
    h.eval_7e("(def x (note-at 0)) (def y (note-at 1))");
    let (x, y) = (h.instance_7e("x"), h.instance_7e("y"));
    h.gesture.pointer_down = true;
    h.run_7e("(set! x.start 4)");
    h.run_7e("(set! x.start 7)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    assert!(!h.live(y));
    assert_eq!(h.instance_7e("(note-at 0)"), x);
    app::edit::undo(&mut h.app);
    h.sync();
    assert!(!h.live(x), "undo made the dragged note stale");
    assert_eq!(
        h.all_notes(0),
        vec![(2, (0.0, 1.0, 0.0)), (7, (0.0, 2.0, 0.0))]
    );
    let undone = h.instance_7e("(note-at 1)");
    assert_eq!(
        h.eval_7e("(let ((n (note-at 1))) n.length)"),
        Value::Number(2.0)
    );
    app::edit::redo(&mut h.app);
    h.sync();
    assert!(!h.live(undone));
    assert_eq!(h.all_notes(0), vec![(7, (0.0, 1.0, 0.0))]);
    h.rejects_7e("(delete-notes! (list x))", STALE);
}

#[test]
fn a_covered_note_is_hidden_and_refuses_edits_until_the_drag_ends() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 2.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    h.sync();
    assert_eq!(h.eval_7e("b.hidden"), Value::Bool(false));
    h.gesture.pointer_down = true;
    h.run_7e("(set! a.start 4)");
    assert_eq!(h.eval_7e("b.hidden"), Value::Bool(true));
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(1.0));
    let hidden = "the note is hidden under a dragged note until the drag ends";
    h.rejects_7e("(set! b.pitch 3)", hidden);
    h.gesture.pointer_down = false;
    h.rejects_7e("(delete-notes! (list b))", hidden);
    h.gesture.pointer_down = true;
    // Uncovered, it is listed again.
    h.run_7e("(set! a.start 9)");
    assert_eq!(h.eval_7e("b.hidden"), Value::Bool(false));
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(2.0));
    // Covered when the drag ends: it is gone.
    h.run_7e("(set! a.start 4)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    let b = h.instance_7e("b");
    assert!(!h.live(b));
}

#[test]
fn esc_rolls_a_script_note_drag_back_with_its_ids_and_selection() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 2.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0)) (def b (note-at 1))");
    h.run_7e("(set! a.selected true)");
    let (a, b) = (h.instance_7e("a"), h.instance_7e("b"));
    let undo = h.app.history.undo_len();
    assert!(h.cancel_note_drag().is_none(), "no drag open");
    h.gesture.pointer_down = true;
    h.run_7e("(set! a.start 4) (set! a.pitch 2)");
    assert_eq!(
        h.all_notes(0),
        vec![(4, (0.0, 1.0, 0.0)), (4, (2.0, 2.0, 0.0))]
    );
    h.run_7e("(set! a.start 4) (set! a.pitch 0)");
    assert_eq!(h.eval_7e("b.hidden"), Value::Bool(true));
    let selected = h.shared.piano_roll_selection.lock().unwrap().clone();
    assert_eq!(selected, HashSet::from([piano_roll_item_id(4, 0)]));
    // Esc: the steps, ids and selection as the drag found them.
    assert!(matches!(h.cancel_note_drag(), Some(Ok(()))));
    h.gesture.pointer_down = false;
    assert!(h.app.history.active_gesture().is_none());
    assert!(h.gesture.script_note_drag.is_none());
    assert_eq!(h.app.history.undo_len(), undo, "nothing recorded");
    assert_eq!(
        h.all_notes(0),
        vec![(0, (0.0, 2.0, 0.0)), (4, (0.0, 1.0, 0.0))]
    );
    let selected = h.shared.piano_roll_selection.lock().unwrap().clone();
    assert_eq!(selected, HashSet::from([piano_roll_item_id(0, 0)]));
    let invalidations = h.shared.ui_invalidations.drain();
    assert!(invalidations.contains(&UiInvalidation::PianoRoll {
        track: 0,
        change: PianoRollInvalidation::Items,
    }));
    h.sync();
    assert_eq!(h.instance_7e("(note-at 0)"), a);
    assert_eq!(h.instance_7e("(note-at 1)"), b);
    assert_eq!(
        h.eval_7e("(list a.start b.hidden a.selected)"),
        h.eval_7e("(list 0 false true)")
    );
    // The handles still edit their notes.
    h.run_7e("(set! b.pitch 5)");
    assert_eq!(h.live_notes(0, 4), vec![(5.0, 1.0, 0.0)]);
}

#[test]
fn a_drag_whose_source_changes_ends_with_every_write_it_made() {
    let mut h = Harness::new();
    // Scene 1 plays a copy of track 0's pattern; scene 0 the original.
    h.command("clone-pattern", Value::Nil);
    h.switch_scene(0);
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0))");
    h.sync();
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7e("(set! a.pitch 3)");
    assert_eq!(h.live_notes(0, 0), vec![(3.0, 1.0, 0.0)]);
    // A scene launch mid-drag: another source.
    h.switch_scene(1);
    let a = h.instance_7e("a");
    assert!(!h.live(a), "another source");
    h.write_notes(0, 2, &[(1.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def b (note-at 0))");
    h.run_7e("(set! b.pitch 6)");
    assert_eq!(h.live_notes(0, 2), vec![(6.0, 1.0, 0.0)]);
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 2, "a drag per source");
    app::edit::undo(&mut h.app);
    assert_eq!(h.live_notes(0, 2), vec![(1.0, 1.0, 0.0)]);
    app::edit::undo(&mut h.app);
    h.switch_scene(0);
    assert_eq!(
        h.live_notes(0, 0),
        vec![(0.0, 1.0, 0.0)],
        "the first drag undone"
    );
    app::edit::redo(&mut h.app);
    assert_eq!(h.live_notes(0, 0), vec![(3.0, 1.0, 0.0)], "and redone");
}

#[test]
fn a_script_drag_rebuilds_once_per_frame_however_many_notes_it_moves() {
    let mut h = Harness::new();
    for step in 0..12 {
        h.write_notes(0, step, &[(step as f32, 1.0, 0.0)]);
    }
    h.sync();
    assert_eq!(h.eval_7e("(len (notes))"), Value::Number(12.0));
    h.sync();
    let undo = h.app.history.undo_len();
    let frames = h.drag_frames();
    h.gesture.pointer_down = true;
    h.run_7e("(map (lambda (n) (set! n.start (+ n.start 0.5))) (notes))");
    assert_eq!(h.drag_frames(), frames + 1, "one rebuild for twelve set!s");
    h.run_7e("(map (lambda (n) (set! n.pitch (+ n.pitch 12))) (notes))");
    assert_eq!(h.drag_frames(), frames + 2);
    let expected: Vec<(usize, (f32, f32, f32))> = (0..12)
        .map(|step| (step, (step as f32 + 12.0, 1.0, 0.5)))
        .collect();
    assert_eq!(h.all_notes(0), expected);
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 1);
}

#[test]
fn note_keys_are_bit_exact_but_for_negative_zero() {
    let note = |transpose: f32, delay: f32| PianoRollNote {
        transpose,
        duration: 1.0,
        delay,
    };
    assert_eq!(
        NoteKey::of(3, &note(-0.0, -0.0)),
        NoteKey::of(3, &note(0.0, 0.0))
    );
    assert_ne!(
        NoteKey::of(3, &note(0.0, 0.25)),
        NoteKey::of(3, &note(0.0, 0.5))
    );
}

#[test]
fn delete_notes_takes_the_notes_of_one_track() {
    let mut h = Harness::new();
    h.write_notes(0, 1, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e("(def a (note-at 0))");
    h.sync();
    let t1 = h.eval_7e("(let ((t (track 1))) t.tid)");
    h.rejects_7e(
        &format!(
            "(host-command \"delete-notes\"
               (dict :track-id a.track.tid :track-ids (list a.track.tid {t1}) :nids (list a.nid)))",
            t1 = num(t1)
        ),
        "the notes are of more than one track",
    );
    h.run_7e("(delete-notes! (list a))");
    assert_eq!(h.all_notes(0), vec![]);
}
