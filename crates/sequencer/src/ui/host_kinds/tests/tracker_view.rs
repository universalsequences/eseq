//! The alez.tracker package ported to the kinds (kind-bindings spec §13
//! stage 8, eseq-0l17.65): its cells over the tracks' steps, its columns
//! over the step params, the device params' and rack macros' locks and the
//! process lanes, its row lamps over `track.playhead-row` and its cursor, a
//! view singleton the cells compare in their shaders.

use super::views::{assert_ported, distro, instance_bindings, widget_keyed, widgets_with_prop};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eseqlisp::vm::format_lisp_value;
use sequencer::sequencer::{ProjectArrangement, StepParam};

const TRACKER: &str = include_str!("../../../../../../content/packages/alez.tracker/src/ui.lisp");

const REFER_TR: &str =
    "(import eseq.kinds :refer (track tracks selection device-param lock-param!))
     (def cursor alez.tracker.ui/tracker-cursor)
     (def view alez.tracker.ui/tracker-view)
     (def step-of (i n) (nth (let ((t (track i))) t.steps) n))
     (def cols-of (i) (alez.tracker.ui/track-columns (track i)))
     (def keys-of (i) (map (lambda (c) (get c :key)) (cols-of i)))
     (def col-of (i key) (first (filter (lambda (c) (= (get c :key) key)) (cols-of i))))
     (def value-of (i key row) (alez.tracker.ui/column-value (track i) (col-of i key) row))
     (def press (k) (alez.tracker.ui/handle-key k k))";

impl Harness {
    fn eval_tr(&mut self, code: &str) -> Value {
        self.eval_with(REFER_TR, code)
    }

    /// Apply what the view queued, sync and render.
    fn tr_render(&mut self) {
        self.drain();
        self.sync();
        self.show_all();
    }

    /// The tracker's widget tree and its revision.
    fn tracker_tree(&self) -> (Value, u64) {
        self.buffer_tree("*tracker*")
    }

    /// The tracker widget keyed `key`.
    fn tracker_widget(&self, key: &str) -> HashMap<String, Value> {
        let (tree, _) = self.tracker_tree();
        widget_keyed(&tree, key).unwrap_or_else(|| panic!("no {key}"))
    }

    /// The text of the tracker widget keyed `key` (or of its first label).
    fn tracker_text(&self, key: &str) -> Value {
        let widget = self.tracker_widget(key);
        let mut labels = Vec::new();
        widgets_with_prop(&Value::Map(cells(widget)), "text", &mut labels);
        labels
            .first()
            .map_or(Value::Nil, |label| label["text"].clone())
    }
}

fn cells(widget: HashMap<String, Value>) -> HashMap<String, Rc<RefCell<Value>>> {
    (widget.into_iter())
        .map(|(key, value)| (key, Rc::new(RefCell::new(value))))
        .collect()
}

/// The factory DAW with the process library and the tracker package
/// imported (its tab shown), rendered until the fields its first render
/// registered (the steps, the params) have arrived.
fn tracker() -> Harness {
    let mut h = distro();
    h.publish_library();
    h.eval("(import alez.tracker.ui)");
    h.tr_render();
    h.tr_render();
    h
}

/// The tracker's leaf widgets (cells, row numbers, track names), by key,
/// with the identity of their key cell: a re-run subtree's widgets are new,
/// a reused one's keep their cells.
fn leaves(tree: &Value, out: &mut HashMap<String, usize>) {
    let Value::Map(map) = tree else { return };
    if let Some(cell) = map.get("key") {
        if let Value::String(key) = &*cell.borrow() {
            let key = &key[key.find("tracker-").unwrap_or(0)..];
            let leaf = ["tracker-cell-", "tracker-gutter-", "tracker-head-"];
            if leaf.iter().any(|prefix| key.starts_with(prefix)) {
                out.insert(key.to_string(), Rc::as_ptr(cell) as usize);
            }
        }
    }
    if let Some(children) = map.get("children") {
        if let Value::List(children) = &*children.borrow() {
            for child in children {
                leaves(&child.borrow(), out);
            }
        }
    }
}

/// The leaf widgets a render after `edit` rebuilt, sorted.
fn rebuilt(h: &mut Harness, edit: impl FnOnce(&mut Harness)) -> Vec<String> {
    h.tr_render();
    let (mut before, mut after) = (HashMap::new(), HashMap::new());
    leaves(&h.tracker_tree().0, &mut before);
    edit(h);
    h.tr_render();
    leaves(&h.tracker_tree().0, &mut after);
    let mut keys: Vec<String> = (after.into_iter())
        .filter(|(key, cell)| before.get(key) != Some(cell))
        .map(|(key, _)| key)
        .collect();
    keys.sort();
    keys
}

/// The instance fields a prop binds, as (instance, field).
fn bound(prop: &Value) -> Vec<(InstanceId, String)> {
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(prop, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    bound
}

#[test]
fn ported_tracker_uses_no_legacy_binding_forms() {
    assert_ported(&[("packages/alez.tracker/src/ui.lisp", TRACKER)]);
    assert!(!TRACKER.contains("eseq.bindings"), "no SEQV channels");
}

/// Importing the module installs and selects the Tracker tab; its cells read
/// the tracks' steps (a shorter track repeats as ghost rows); hide drops it.
#[test]
fn importing_the_tracker_installs_its_tab_and_draws_the_steps() {
    let mut h = tracker();
    let tabs = format_lisp_value(&h.eval("(eseq.seq-step-tabs/seq-main-step-tabs)"));
    assert!(
        tabs.contains("Tracker") && tabs.contains("*tracker*"),
        "tabs: {tabs}"
    );
    assert_eq!(
        h.eval("(eseq.seq-step-tabs/seq-visible-main-panel-buffer)"),
        s("*tracker*")
    );
    h.shared.state.pattern.track_params[1].set_num_steps(4);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.eval_tr(
        "(let ((s (step-of 1 1)))
           (do (set! s.active true) (set! s.transpose -13) (set! s.velocity 0.5)))",
    );
    h.tr_render();
    assert_eq!(h.eval_tr("(alez.tracker.ui/pattern-rows)"), number(16.0));
    assert_eq!(h.tracker_text("tracker-cell-1-1-0"), s("B-2"));
    assert_eq!(h.tracker_text("tracker-cell-1-1-1"), s("3F"));
    assert_eq!(h.tracker_text("tracker-cell-0-1-0"), s("---"));
    // Row 5 of the 4-step track is a ghost of step 1, drawn on the track's
    // tint.
    assert_eq!(h.tracker_text("tracker-cell-1-5-0"), s("B-2"));
    assert_eq!(
        h.tracker_widget("tracker-row-1-5")["background-color"],
        h.eval_tr("(eseq.view-kit/color-rgba (let ((t (track 1))) t.color) 0.06)")
    );
    assert_eq!(h.tracker_text("tracker-gutter-15"), s("15"));
    assert_eq!(
        h.tracker_text("tracker-head-0"),
        h.eval_tr("(let ((t (track 0))) t.name)")
    );
    assert_eq!(h.tracker_text("tracker-sub-note-0"), s("Note"));
    assert_eq!(h.tracker_text("tracker-sub-vol-0"), s("Vol"));
    let names = h.eval_tr(
        "(list (alez.tracker.ui/note-name 0) (alez.tracker.ui/note-name -13)
               (alez.tracker.ui/hex2 127))",
    );
    assert_eq!(format_lisp_value(&names), r#"("C-4" "B-2" "7F")"#);

    h.eval("(alez.tracker.ui/hide)");
    let tabs = format_lisp_value(&h.eval("(eseq.seq-step-tabs/seq-main-step-tabs)"));
    assert!(!tabs.contains("*tracker*"), "hide unregisters the tab");
}

/// `track.playhead-row`: which repeat of a shorter track plays, on a grid
/// as tall as the longest pattern. A row's lamp binds it; the gutter's lamp
/// and, while playing, the scroll bind the current track's
/// (`selection.playhead-row`): playback only repaints.
#[test]
fn a_rows_lamp_lights_the_repeat_the_playhead_plays() {
    let mut h = tracker();
    h.shared.state.pattern.track_params[0].set_num_steps(4);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.tr_render();
    let row = |h: &mut Harness| h.eval_tr("(let ((t (track 0))) t.playhead-row)");
    assert_eq!(row(&mut h), number(-1.0), "stopped");
    // A 4-step track under a 16-row grid on its step 2 at the transport's
    // sixteenth 38: 38 mod 16 = 6, and 6 mod 4 = 2, so row 6 lights.
    let transport = &h.shared.state.transport;
    transport.playing.store(true, Ordering::Relaxed);
    transport.track_playheads[0].store(2, Ordering::Relaxed);
    transport.playhead.store(38, Ordering::Relaxed);
    h.tr_render();
    assert_eq!(row(&mut h), number(6.0));
    let track0 = (h.track_id(0), "playhead-row".to_string());
    let lamp = h.tracker_widget("tracker-row-0-6");
    assert_eq!(lamp["background"], s("tracker-row-lamp"));
    assert_eq!(lamp["row"], number(6.0));
    let slot = &lamp["shader-state-track.playhead-row"];
    assert_eq!(bound(slot), vec![track0.clone()]);
    let current = (
        h.singleton("eseq.kinds:selection"),
        "playhead-row".to_string(),
    );
    assert_eq!(
        h.eval_tr("selection.playhead-row"),
        number(6.0),
        "the current track's"
    );
    let gutter = h.tracker_widget("tracker-gutter-6");
    assert_eq!(
        bound(&gutter["playhead-row"]),
        vec![current.clone()],
        "the gutter follows the current track"
    );
    let scroll = h.tracker_widget("tracker-scroll");
    assert_eq!(
        bound(&scroll["center-row"]),
        vec![current],
        "playing: the scroll follows the current track's row"
    );
    // A clock that disagrees with the track's own step lights the real row.
    let revision = h.tracker_tree().1;
    h.shared
        .state
        .transport
        .playhead
        .store(37, Ordering::Relaxed);
    h.tr_render();
    assert_eq!(row(&mut h), number(2.0));
    let Value::ReactiveRef { slot, .. } = slot else {
        panic!("a binding");
    };
    assert_eq!(read_float_slot(slot), 2.0);
    assert_eq!(h.tracker_tree().1, revision, "playback only repaints");

    h.shared
        .state
        .transport
        .playing
        .store(false, Ordering::Relaxed);
    h.tr_render();
    assert_eq!(row(&mut h), number(-1.0));
    let cursor = h.singleton("alez.tracker.ui:tracker-cursor");
    assert_eq!(
        bound(&h.tracker_widget("tracker-scroll")["center-row"]),
        vec![(cursor, "row".to_string())],
        "stopped: the scroll follows the cursor row"
    );
}

/// The keys edit the step under the cursor (a ghost row its real step), one
/// undo entry each; the cursor is a view singleton the cells compare in
/// their shaders, so moving it only repaints.
#[test]
fn the_keys_edit_the_step_under_the_cursor_and_the_cursor_only_repaints() {
    let mut h = tracker();
    let cursor = h.singleton("alez.tracker.ui:tracker-cursor");
    let cell = h.tracker_widget("tracker-cell-0-2-0");
    assert_eq!(cell["background"], s("tracker-cursor-lamp"));
    let at = ["cell-track", "cell-row", "cell-col"].map(|prop| cell[prop].clone());
    assert_eq!(at, [number(0.0), number(2.0), number(0.0)]);
    for field in ["track", "row", "col"] {
        let prop = &cell[&format!("cursor-{field}")];
        assert_eq!(bound(prop), vec![(cursor, field.to_string())]);
    }

    // A real key reaches the mode's handler.
    let id = (h.editor.buffers.iter())
        .find(|buffer| buffer.name == "*tracker*")
        .expect("tracker buffer")
        .id;
    h.editor.set_active_buffer(id);
    // The first key after the buffer is activated re-renders it once.
    h.editor
        .handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    h.tr_render();
    assert_eq!(
        h.eval_tr("cursor.row"),
        number(15.0),
        "UP wraps to the last row"
    );
    let revision = h.tracker_tree().1;
    h.editor
        .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    h.editor
        .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    h.tr_render();
    assert_eq!(h.eval_tr("cursor.row"), number(1.0));
    assert_eq!(h.tracker_tree().1, revision, "a cursor move only repaints");

    // RET toggles the step, a note key sets its transpose (the step is
    // already on) and moves on by step-advance; a note typed into an empty
    // step turns it on in the same entry.
    let entries = h.app.history.undo_len();
    h.eval_tr("(press \"RET\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("(let ((s (step-of 0 1))) s.active)"),
        Value::Bool(true)
    );
    h.eval_tr("(press \"x\") (press \"t\")");
    h.tr_render();
    let state = &h.shared.state;
    assert_eq!(
        state.pattern.step_data[0].get(1, StepParam::Transpose),
        18.0
    );
    assert_eq!(h.app.history.undo_len(), entries + 2, "one entry each");
    assert_eq!(
        h.eval_tr("(list cursor.row view.octave)"),
        h.eval_tr("(list 2 5)")
    );
    assert_eq!(h.tracker_text("tracker-cell-0-1-0"), s("F#5"));
    h.eval_tr("(alez.tracker.ui/select-cell 0 6 0) (press \"a\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("(let ((s (step-of 0 6))) (list s.active s.transpose))"),
        h.eval_tr("(list true 12)")
    );
    assert_eq!(h.app.history.undo_len(), entries + 3, "one entry");
    h.eval_tr("(alez.tracker.ui/select-cell 0 1 0)");

    // Vol takes two hex digits, 00-7F as the cells show it: the first waits
    // in the entry chip.
    h.eval_tr("(alez.tracker.ui/select-cell 0 1 1) (press \"4\")");
    assert_eq!(h.eval_tr("view.entry"), number(4.0));
    h.tr_render();
    assert_eq!(h.tracker_text("tracker-chip-entry"), s("4_"));
    h.eval_tr("(press \"0\")");
    h.tr_render();
    assert_eq!(h.eval_tr("view.entry"), s(""));
    let velocity = h.shared.state.pattern.step_data[0].get(1, StepParam::Velocity);
    assert!((velocity - 64.0 / 127.0).abs() < 1e-5, "{velocity}");
    assert_eq!(h.tracker_text("tracker-cell-0-1-1"), s("40"));
    h.eval_tr("(press \"F\") (press \"F\")");
    h.tr_render();
    let velocity = h.shared.state.pattern.step_data[0].get(1, StepParam::Velocity);
    assert_eq!(velocity, 1.0, "past 7F saturates");
    // A pending digit drops on a move, by key or by click.
    h.eval_tr("(press \"4\") (press \"LEFT\")");
    assert_eq!(h.eval_tr("view.entry"), s(""));
    h.eval_tr(
        "(alez.tracker.ui/select-cell 0 1 1) (press \"4\") (alez.tracker.ui/select-cell 0 2 1)",
    );
    assert_eq!(h.eval_tr("view.entry"), s(""));
    // A key first puts a cursor left off the grid (a track or a column gone
    // under it) back on it, so its lamp shows the cell the key edits.
    h.eval_tr("(set! cursor.track 40) (set! cursor.col 9) (press \"DOWN\")");
    assert_eq!(
        h.eval_tr("(list cursor.track cursor.row cursor.col)"),
        h.eval_tr("(list (- (len (tracks)) 1) 3 1)")
    );

    // A ghost row edits the step it mirrors; BS clears it.
    h.shared.state.pattern.track_params[0].set_num_steps(4);
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.tr_render();
    h.eval_tr("(alez.tracker.ui/select-cell 0 9 0) (press \"BS\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("(let ((s (step-of 0 1))) s.active)"),
        Value::Bool(false)
    );
    // The host's current track follows the cursor.
    h.eval_tr("(alez.tracker.ui/select-cell 1 0 0)");
    h.tr_render();
    assert_eq!(
        h.eval_tr("(= selection.track (track 1))"),
        Value::Bool(true)
    );
}

/// A track's columns: the step params an active step holds off their
/// default and its locked device params, then the columns added from the
/// picker; each reads and writes its own way, one undo entry per edit.
#[test]
fn a_tracks_columns_are_its_step_params_locks_and_added_targets() {
    let mut h = tracker();
    h.add_effect(0, "Filter");
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 2.0, 0.0)]);
    let data = &h.shared.state.pattern.step_data[0];
    // Velocity and transpose are the Note/Vol cells, however far they stray.
    data.set(0, StepParam::Velocity, 0.5);
    data.set(0, StepParam::Transpose, 7.0);
    h.shared.state.publish_scheduler_track(0);
    h.tr_render();
    h.eval_tr(
        r#"(def flt (first (filter (lambda (d) (= d.type "Filter")) (let ((t (track 0))) t.devices))))
           (def cutoff (device-param flt "cutoff"))
           (lock-param! cutoff (list (step-of 0 4)) 800)"#,
    );
    h.tr_render();
    let cutoff = h.eval_tr(r#"(str "param:effect:" flt.did ":" cutoff.index)"#);
    let cutoff = format_lisp_value(&cutoff);
    assert_eq!(
        h.eval_tr("(keys-of 0)"),
        h.eval_tr(&format!("(list \"step:duration\" {cutoff})"))
    );
    assert_eq!(h.eval_tr("(value-of 0 \"step:duration\" 4)"), number(2.0));
    assert_eq!(h.eval_tr("(value-of 0 \"step:duration\" 0)"), number(1.0));
    assert_eq!(
        h.eval_tr("(value-of 0 \"step:duration\" 1)"),
        Value::Nil,
        "inactive"
    );
    assert_eq!(
        h.eval_tr(&format!("(value-of 0 {cutoff} 4)")),
        number(800.0)
    );
    assert_eq!(h.eval_tr(&format!("(value-of 0 {cutoff} 0)")), Value::Nil);
    assert_eq!(h.tracker_text("tracker-sub-col-0-0"), s("Drt"));
    assert_eq!(h.tracker_text("tracker-sub-col-0-1"), s("ctf"));
    assert_eq!(h.tracker_text("tracker-cell-0-4-2"), s("2"));
    assert_eq!(h.tracker_text("tracker-cell-0-0-3"), s(".."));

    // The picker: one submenu per device (and the lanes); toggling adds a
    // target, toggling again hides it, as it hides a locked column.
    h.eval_tr("(alez.tracker.ui/open-column-menu (track 0) (dict :at (dict :col 10 :row 4)))");
    h.tr_render();
    let (tree, _) = h.tracker_tree();
    for group in ["Step", "FX 1 · Filter", "lanes"] {
        let key = format!("tracker-pick-0-{group}-group");
        assert!(widget_keyed(&tree, &key).is_some(), "{key}");
    }
    let item = widget_keyed(&tree, "tracker-pick-0-Step-step:pan").expect("pan item");
    assert_eq!(item["checked"], Value::Bool(false));
    let entries = h.app.history.undo_len();
    h.eval_tr(&format!(
        "(alez.tracker.ui/toggle-column (track 0) \"step:pan\")
         (alez.tracker.ui/toggle-column (track 0) {cutoff})"
    ));
    h.tr_render();
    assert_eq!(
        h.eval_tr("(keys-of 0)"),
        h.eval_tr("(list \"step:duration\" \"step:pan\")")
    );
    assert_eq!(h.tracker_text("tracker-sub-col-0-1"), s("Pan"));
    assert_eq!(
        h.app.history.undo_len(),
        entries,
        "view state records nothing"
    );
    h.eval_tr(&format!(
        "(alez.tracker.ui/toggle-column (track 0) \"step:pan\")
         (alez.tracker.ui/toggle-column (track 0) {cutoff})"
    ));
    assert_eq!(
        h.eval_tr("(keys-of 0)"),
        h.eval_tr(&format!("(list \"step:duration\" {cutoff})"))
    );
    // Closed, the picker builds no item.
    h.eval_tr("(let ((m alez.tracker.ui/tracker-menu)) (set! m.open false))");
    h.tr_render();
    let (tree, _) = h.tracker_tree();
    assert!(widget_keyed(&tree, "tracker-pick-0-Step-group").is_none());

    // Typing: the duration takes two hex digits over its range (the focus
    // step setter), the cutoff locks the step; BS clears the lock.
    h.eval_tr("(alez.tracker.ui/select-cell 0 4 2) (press \"8\") (press \"0\")");
    h.tr_render();
    let duration = h.shared.state.pattern.step_data[0].get(4, StepParam::Duration);
    let (lo, hi) = (StepParam::Duration.min(), StepParam::Duration.max());
    let typed = lo + (hi - lo) * 128.0 / 255.0;
    assert!((duration - typed).abs() < 1e-3, "{duration}");
    assert_eq!(h.app.history.undo_len(), entries + 1);
    h.eval_tr("(alez.tracker.ui/select-cell 0 0 3) (press \"F\") (press \"F\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("cutoff.step-locks"),
        h.eval_tr("(list (list 0 cutoff.max) (list 4 800))")
    );
    h.eval_tr("(alez.tracker.ui/select-cell 0 4 3) (press \"BS\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("cutoff.step-locks"),
        h.eval_tr("(list (list 0 cutoff.max))")
    );
    assert_eq!(h.app.history.undo_len(), entries + 3);
    // A nudge on an unlocked step starts from the param's base.
    h.eval_tr("(press \"=\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr("(nth (nth cutoff.step-locks 1) 1)"),
        h.eval_tr("(+ cutoff.base (/ (- cutoff.max cutoff.min) 32))")
    );

    // A collapsed track folds its columns into the badge.
    h.eval_tr("(alez.tracker.ui/toggle-collapse (track 0))");
    h.tr_render();
    assert_eq!(h.tracker_text("tracker-collapse-0"), s("2"));
    let (tree, _) = h.tracker_tree();
    assert!(widget_keyed(&tree, "tracker-cell-0-0-2").is_none());
}

/// A process lane column reads the lane's values and sets them with
/// set-lane-steps!; a retrig (one-digit) column takes its digit as is.
#[test]
fn a_lane_column_reads_and_writes_the_lane() {
    let mut h = tracker();
    h.eval_tr("(def lane (first (let ((t (track 0))) t.lanes)))");
    let key = format_lisp_value(&h.eval_tr("(alez.tracker.ui/lane-key lane)"));
    h.eval_tr(&format!(
        "(alez.tracker.ui/toggle-column (track 0) {key})
         (alez.tracker.ui/toggle-column (track 0) \"step:retrig\")"
    ));
    h.tr_render();
    assert_eq!(
        h.eval_tr(&format!("(value-of 0 {key} 3)")),
        h.eval_tr("(nth lane.values 3)")
    );
    // Two hex digits over the lane's range.
    let entries = h.app.history.undo_len();
    h.eval_tr("(alez.tracker.ui/select-cell 0 3 2) (press \"0\") (press \"0\")");
    h.tr_render();
    assert_eq!(
        h.eval_tr(&format!("(value-of 0 {key} 3)")),
        h.eval_tr("lane.min")
    );
    assert_eq!(h.app.history.undo_len(), entries + 1);
    h.write_notes(0, 3, &[(0.0, 1.0, 0.0)]);
    h.tr_render();
    h.eval_tr("(alez.tracker.ui/select-cell 0 3 3) (press \"3\")");
    h.tr_render();
    let retrig = h.shared.state.pattern.step_data[0].get(3, StepParam::Retrig);
    assert_eq!(retrig, 3.0);
    // Off its default now, the retrig is one of the track's own columns,
    // ahead of the added lane.
    assert_eq!(
        h.eval_tr("(keys-of 0)"),
        h.eval_tr(&format!("(list \"step:retrig\" {key})"))
    );
    assert_eq!(h.tracker_text("tracker-cell-0-3-2"), s("3"));
}

/// A rack macro with a lock is a column, titled by its name (a rename shows).
#[test]
fn a_locked_rack_macro_is_a_column_named_by_the_macro() {
    let mut h = tracker();
    h.rack_track();
    h.tr_render();
    h.eval_tr(
        "(def rm (first (let ((d (first (let ((t (track 2))) t.devices)))) d.macros)))
         (eseq.kinds/lock-rack-macro! rm (list (step-of 2 3)) 0.25)",
    );
    h.tr_render();
    assert_eq!(h.eval_tr("(keys-of 2)"), h.eval_tr("(list \"macro:0\")"));
    assert_eq!(h.eval_tr("(value-of 2 \"macro:0\" 3)"), number(0.25));
    assert_eq!(h.tracker_text("tracker-cell-2-3-2"), s("3F"));
    h.eval_tr("(set! rm.name \"Tone Body\")");
    h.tr_render();
    assert_eq!(h.tracker_text("tracker-sub-col-2-0"), s("Tn.Bdy"));
}

/// Column headers spell a label compactly (the legacy host's rule).
#[test]
fn compact_labels_are_short_and_systematic() {
    let mut h = tracker();
    for (label, short) in [
        ("voicing.character", "vcn.chr"),
        ("body.damping", "bdy.dmp"),
        ("stick.hardness", "stc.hrd"),
        ("output.color", "otp.clr"),
        ("contact.touch", "cnt.tch"),
        ("lp_freq", "lp.frq"),
        ("cutoff", "ctf"),
        ("resonance", "rsn"),
        ("mix", "mix"),
        ("Duration", "Drt"),
        ("Retrig Rate", "Rtr.Rt"),
        ("__dgen_mod_active__body.bell", "~bdy.bll"),
        ("mod body.bell slot 1 amt", "bdy.bll~1"),
        ("a.b.c.d", "c.d"),
        ("", ""),
    ] {
        let code = format!("(alez.tracker.ui/compact-label \"{label}\")");
        assert_eq!(h.eval_tr(&code), s(short), "{label}");
    }
}

/// A step param column writes the live step its cell shows
/// (`set-step-param!`), never the piano roll's edit focus: with a take
/// pinned on the track, the take keeps its steps.
#[test]
fn a_step_column_writes_the_live_step_while_a_take_is_pinned() {
    let mut h = tracker();
    h.app
        .arr_replace(ProjectArrangement::new(2, 128.0))
        .unwrap();
    h.app.set_arrangement_view_visible(true);
    let (take, clip) = h.app.arr_empty_take_clip_create(0, 0.0, 128.0).unwrap();
    h.app
        .select_song_clip_span(0, clip, Some((0.0, 128.0)))
        .expect("select");
    assert!(matches!(
        h.app.track_edit_focus(0),
        sequencer::app::focus::EditFocus::Take { .. }
    ));
    h.eval_tr(
        "(let ((s (step-of 0 2))) (set! s.active true))
         (alez.tracker.ui/toggle-column (track 0) \"step:retrig\")",
    );
    h.tr_render();
    let chunks = h.shared.state.track_take(0, take).unwrap().chunks;
    let retrig = |h: &Harness, pattern| {
        h.shared
            .state
            .with_pool_pattern(0, pattern, |data| {
                data.step_data[2][StepParam::Retrig.index()]
            })
            .unwrap()
    };
    let before = retrig(&h, chunks[0]);
    let entries = h.app.history.undo_len();
    h.eval_tr("(alez.tracker.ui/select-cell 0 2 2) (press \"3\")");
    h.tr_render();
    assert_eq!(
        h.shared.state.pattern.step_data[0].get(2, StepParam::Retrig),
        3.0
    );
    assert_eq!(h.tracker_text("tracker-cell-0-2-2"), s("3"));
    assert_eq!(retrig(&h, chunks[0]), before, "the take keeps its step");
    assert_eq!(h.app.history.undo_len(), entries + 1, "one entry");
    // The value rule: out of range is an error that changes nothing.
    h.rejects_in(
        REFER_TR,
        "(eseq.kinds/set-step-param! (step-of 0 2) \"retrig\" -1)",
        "retrig takes",
        true,
    );
    assert_eq!(
        h.shared.state.pattern.step_data[0].get(2, StepParam::Retrig),
        3.0
    );
}

/// The view reads no step: a toggle re-renders that track's cells on that
/// row only (the columns come from t.step-params-in-use, which a step at its
/// defaults leaves alone), a lock edit that track's cells; a cursor move to
/// another track re-renders the two track headers (their highlight) and
/// nothing else (the scroll and the row numbers bind the current track's
/// row).
#[test]
fn edits_and_cursor_moves_re_render_only_what_they_change() {
    let mut h = tracker();
    let in_use = |h: &mut Harness| h.eval_tr("(let ((t (track 1))) t.step-params-in-use)");
    assert_eq!(in_use(&mut h), h.eval_tr("(list)"));
    let cells = |track: usize, row: usize| -> Vec<String> {
        (0..2)
            .map(|col| format!("tracker-cell-{track}-{row}-{col}"))
            .collect()
    };
    let toggle = rebuilt(&mut h, |h| {
        h.eval_tr("(let ((s (step-of 1 5))) (set! s.active true))");
    });
    assert_eq!(toggle, cells(1, 5), "the track's cells on row 5");
    assert_eq!(h.tracker_text("tracker-cell-1-5-0"), s("C-4"));
    // A step param off its default adds its column: the view re-runs.
    let retrig = rebuilt(&mut h, |h| {
        h.eval_tr("(let ((s (step-of 1 5))) (set! s.retrig 2))");
    });
    assert_eq!(in_use(&mut h), h.eval_tr("(list \"retrig\")"));
    assert!(retrig.contains(&"tracker-head-1".to_string()), "{retrig:?}");
    // Another step at its defaults leaves the columns alone.
    let toggle = rebuilt(&mut h, |h| {
        h.eval_tr("(let ((s (step-of 1 7))) (set! s.active true))");
    });
    let mut row = cells(1, 7);
    row.push("tracker-cell-1-7-2".to_string());
    assert_eq!(toggle, row);
    // A lock on a param that holds one already: that track's cells.
    h.add_effect(1, "Filter");
    h.tr_render();
    h.eval_tr(
        r#"(def flt (first (filter (lambda (d) (= d.type "Filter")) (let ((t (track 1))) t.devices))))
           (def cutoff (device-param flt "cutoff"))
           (lock-param! cutoff (list (step-of 1 5)) 800)"#,
    );
    h.tr_render();
    let lock = rebuilt(&mut h, |h| {
        h.eval_tr("(lock-param! cutoff (list (step-of 1 7)) 900)");
    });
    assert!(
        lock.iter().all(|key| key.starts_with("tracker-cell-1-")),
        "{lock:?}"
    );
    assert!(lock.contains(&"tracker-cell-1-7-3".to_string()), "{lock:?}");

    // A cross-track cursor move.
    h.eval_tr("(alez.tracker.ui/select-cell 0 3 0)");
    let moved = rebuilt(&mut h, |h| {
        h.eval_tr("(alez.tracker.ui/select-cell 1 3 0)");
    });
    assert_eq!(
        h.eval_tr("(= selection.track (track 1))"),
        Value::Bool(true)
    );
    assert_eq!(
        moved,
        ["tracker-head-0", "tracker-head-1"],
        "the two headers"
    );
}
