//! Stage 7d: the arrangement (`song`, `region`, `scene-span`, `clip`,
//! `cell`, `track.governed` / `track.latched`).

use super::*;
use sequencer::sequencer::{ClipId, LaneSource, PatternId};

const REFER_7D: &str =
    "(import eseq.kinds :refer (track tracks scenes banks song region transport \
                        launch-cell! select-region! clear-region! take-none take-governed \
                        take-latched))
                        (def track-clips (i) (let ((t (track i))) t.clips))
                        (def track-cells (i) (let ((t (track i))) t.cells))";

impl Harness {
    fn eval_7d(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7D}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Run `code`'s commands, then sync.
    fn run_7d(&mut self, code: &str) {
        self.eval_7d(code);
        self.drain();
        self.sync();
    }

    /// Run `code`'s commands; they must fail with `message` and change no
    /// history.
    fn rejects_7d(&mut self, code: &str, message: &str) {
        let undo = self.app.history.undo_len();
        self.editor.minibuffer = None;
        self.eval_7d(code);
        self.drain();
        let error = self.editor.minibuffer.clone().unwrap_or_default();
        assert!(error.contains(message), "{code}: {error}");
        assert_eq!(self.app.history.undo_len(), undo, "{code} changed nothing");
    }

    fn clip(&mut self, track: usize, start: f64, end: f64, pattern: u64) -> ClipId {
        let source = LaneSource::Pattern(PatternId(pattern));
        let id = self.app.arr_clip_create(track, start, end, source, 0.0);
        id.expect("clip")
    }

    /// The committed clip `id`'s (start, end, pattern).
    fn committed(&self, id: ClipId) -> Option<(f64, f64, Option<u64>)> {
        self.app.state.with_committed_arrangement(|arrangement| {
            let (_, clip) = arrangement?.find_clip(id)?;
            Some((clip.start_beat, clip.end_beat, clip.pattern_id))
        })
    }

    /// Publish the legacy song and cell fields as the reactive tick does.
    fn publish_legacy_song(&mut self) {
        let state = self.shared.state.clone();
        let count = self.app.tracks.len();
        let rt = self.editor.runtime_mut();
        sync_song_state(rt, &self.app, &mut SongFrameState::default(), true);
        sync_track_pattern_cell_state_fields(rt, &state, count);
    }

    fn seq(&self, field: &str) -> Value {
        self.rt()
            .reactive_field_value("SEQ", field)
            .unwrap_or_else(|| panic!("SEQ.{field}"))
            .clone()
    }

    /// A second pattern in track 0's pool: a cloned scene's cell.
    fn second_pattern(&mut self) -> u64 {
        self.command("clone-pattern", Value::Nil);
        self.sync();
        let cells = self.shared.state.track_pattern_cells(0);
        let pattern = cells.iter().map(|cell| cell.pattern_id.0).max().unwrap();
        assert!(pattern > 1, "clone-pattern adds a pattern: {cells:?}");
        pattern
    }
}

fn map_get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone()),
        other => panic!("not a map: {other:?}"),
    }
}

fn items(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        other => panic!("not a list: {other:?}"),
    }
}

#[test]
fn arrangement_fields_read_after_sync_and_match_the_legacy_fields() {
    let mut h = Harness::new();
    let a = h.clip(0, 0.0, 4.0, 1);
    let b = h.clip(0, 8.0, 12.0, 1);
    h.clip(1, 4.0, 8.0, 1);
    h.app.arr_set_loop(true).expect("loop");
    h.app.set_arrangement_cursor(6.0, -1);
    h.sync();
    h.eval_7d("(def t0 (track 0)) (def t1 (track 1)) (def c (first t0.clips))");
    assert_eq!(h.eval_7d("(len t0.clips)"), Value::Number(2.0));
    assert_eq!(h.eval_7d("(len t1.clips)"), Value::Number(1.0));
    assert_eq!(h.eval_7d("c.cid"), Value::Number(a.0 as f64));
    assert_eq!(
        h.eval_7d("(let ((x (nth t0.clips 1))) x.cid)"),
        Value::Number(b.0 as f64)
    );
    assert_eq!(h.eval_7d("c.track"), h.eval_7d("t0"));
    assert_eq!(h.eval_7d("c.start"), Value::Number(0.0));
    assert_eq!(h.eval_7d("c.end"), Value::Number(4.0));
    assert_eq!(h.eval_7d("c.take"), Value::Number(-1.0));
    assert_eq!(h.eval_7d("c.offset"), Value::Number(0.0));
    assert_eq!(h.eval_7d("c.cell.pid"), Value::Number(1.0));
    assert_eq!(h.eval_7d("c.cell"), h.eval_7d("(first t0.cells)"));
    // Song fields.
    assert_eq!(h.eval_7d("song.exists"), Value::Bool(true));
    assert_eq!(h.eval_7d("song.loop"), Value::Bool(true));
    assert_eq!(h.eval_7d("song.mode"), s("stopped"));
    assert_eq!(h.eval_7d("song.recording-kind"), s(""));
    assert_eq!(h.eval_7d("song.cursor"), Value::Number(6.0));
    assert_eq!(h.eval_7d("song.position"), Value::Number(0.0));
    assert_eq!(h.eval_7d("song.edit-error"), s(""));
    assert_eq!(h.eval_7d("song.region"), Value::Nil);
    assert_eq!(h.eval_7d("song.bound-clip"), Value::Nil);
    assert_eq!(h.eval_7d("song.manual-latch"), Value::Bool(false));
    assert_eq!(h.eval_7d("t0.governed"), h.eval_7d("take-none"));
    assert_eq!(h.eval_7d("t0.latched"), Value::Bool(false));
    // Legacy parity.
    h.publish_legacy_song();
    assert_eq!(h.seq("song-end-beat"), h.eval_7d("song.end"));
    assert_eq!(h.seq("song-loop-enabled"), h.eval_7d("song.loop"));
    assert_eq!(h.seq("song-exists"), h.eval_7d("song.exists"));
    assert_eq!(h.seq("song-mode"), h.eval_7d("song.mode"));
    assert_eq!(h.seq("song-position-beats"), h.eval_7d("song.position"));
    assert_eq!(
        h.seq("song-recording-kind"),
        h.eval_7d("song.recording-kind")
    );
    assert_eq!(h.seq("song-scene-latched"), h.eval_7d("song.scene-latched"));
    let governed = items(h.seq("song-track-governed"));
    let latched = items(h.seq("song-track-latched"));
    for track in 0..2 {
        h.eval_7d(&format!("(def tx (track {track}))"));
        assert_eq!(governed[track], h.eval_7d("tx.governed"));
        assert_eq!(latched[track], h.eval_7d("tx.latched"));
    }
    // The lanes: ids, spans and source previews.
    let lanes = items(h.seq("song-lanes"));
    let events = items(h.seq("song-lane-events"));
    for (track, lane) in lanes.into_iter().enumerate() {
        let previews = items(events[track].clone());
        for (index, legacy) in items(lane).into_iter().enumerate() {
            h.eval_7d(&format!("(def cx (nth (track-clips {track}) {index}))"));
            let clip = "cx";
            assert_eq!(
                map_get(&legacy, "clip-id"),
                h.eval_7d(&format!("{clip}.cid"))
            );
            let start = h.eval_7d(&format!("{clip}.start"));
            assert_eq!(map_get(&legacy, "start-beat"), start);
            assert_eq!(
                map_get(&legacy, "end-beat"),
                h.eval_7d(&format!("{clip}.end"))
            );
            let pattern = map_get(&legacy, "pattern-id");
            let preview = previews
                .iter()
                .find(|preview| map_get(preview, "pattern-id") == pattern)
                .expect("the clip's preview");
            let fields = [
                ("num-steps", "num-steps"),
                ("length-beats", "length"),
                ("events", "events"),
            ];
            for (legacy_key, field) in fields {
                let value = h.eval_7d(&format!("{clip}.{field}"));
                assert_eq!(map_get(preview, legacy_key), value, "{field}");
            }
        }
    }
    let spans = items(h.seq("scene-spans"));
    assert_eq!(
        h.eval_7d("(len song.spans)"),
        Value::Number(spans.len() as f64)
    );
    for (index, span) in spans.iter().enumerate() {
        h.eval_7d(&format!("(def sx (nth song.spans {index}))"));
        let ours = "sx";
        assert_eq!(
            h.eval_7d(&format!("{ours}.index")),
            Value::Number(index as f64)
        );
        assert_eq!(
            map_get(span, "start-beat"),
            h.eval_7d(&format!("{ours}.start"))
        );
        assert_eq!(map_get(span, "end-beat"), h.eval_7d(&format!("{ours}.end")));
        let scene = h.eval_7d(&format!("{ours}.scene.index"));
        assert_eq!(map_get(span, "scene"), scene);
    }
    // Cells.
    let cells = h.shared.state.track_pattern_cells(0);
    assert_eq!(
        h.eval_7d("(len t0.cells)"),
        Value::Number(cells.len() as f64)
    );
    h.eval_7d("(def cl (first t0.cells))");
    assert_eq!(h.eval_7d("cl.track"), h.eval_7d("t0"));
    let model = &cells[0];
    assert_eq!(
        h.seq("track-pattern-cell-active-0-1"),
        h.eval_7d("cl.active")
    );
    assert_eq!(
        h.eval_7d("cl.assigned"),
        Value::Bool(model.assigned_to_current_scene)
    );
    assert_eq!(h.eval_7d("cl.override"), Value::Bool(model.overridden));
    assert_eq!(h.eval_7d("cl.selected"), Value::Bool(false));
    assert_eq!(h.eval_7d("cl.active"), Value::Bool(true));
    assert_eq!(h.eval_7d("cl.queued"), Value::Bool(false));
    assert_eq!(h.eval_7d("cl.banks"), h.eval_7d("(list (first (banks)))"));
    // The region, the bound clip and the edit error follow the legacy
    // commands.
    let region = map_value([
        ("track-a", Value::Number(1.0)),
        ("track-b", Value::Number(0.0)),
        ("start", Value::Number(2.0)),
        ("end", Value::Number(6.0)),
    ]);
    h.command("song-set-region", region);
    h.sync();
    assert_eq!(h.eval_7d("song.region"), h.eval_7d("region"));
    assert_eq!(h.eval_7d("region.tracks"), h.eval_7d("(list t0 t1)"));
    assert_eq!(h.eval_7d("region.start"), Value::Number(2.0));
    assert_eq!(h.eval_7d("region.end"), Value::Number(6.0));
    assert_eq!(h.eval_7d("region.scene-lane"), Value::Bool(false));
    let select = map_value([
        ("track", Value::Number(0.0)),
        ("clip-id", Value::Number(b.0 as f64)),
        ("start", Value::Number(8.0)),
        ("end", Value::Number(12.0)),
    ]);
    h.command("song-select-clip", select);
    h.sync();
    assert_eq!(h.eval_7d("song.bound-clip"), h.eval_7d("(nth t0.clips 1)"));
    h.command(
        "arrangement-clip-delete",
        map_value([("clip-id", Value::Number(999.0))]),
    );
    h.sync();
    h.publish_legacy_song();
    assert_eq!(h.seq("song-edit-error"), h.eval_7d("song.edit-error"));
    assert_ne!(h.eval_7d("song.edit-error"), s(""));
    assert_eq!(h.seq("song-region"), {
        let region = [0.0, 0.0, 8.0, 12.0].map(Value::Number);
        list_value(region.into_iter().chain([Value::Bool(false)]))
    });
    // Nothing changed: a second sync pushes nothing.
    h.sync();
    assert!(!h.sync());
}

#[test]
fn take_constants_name_the_legacy_take_lane_states() {
    let mut h = Harness::new();
    // `song_take_lane_states`: 0 not a take lane, 1 take-governed, 2 a take
    // lane latched away.
    for (name, state) in [("take-none", 0), ("take-governed", 1), ("take-latched", 2)] {
        assert_eq!(h.eval_7d(name), Value::Number(state as f64), "{name}");
    }
}

#[test]
fn arrangement_setters_change_the_model_through_history_with_undo() {
    let mut h = Harness::new();
    let a = h.clip(0, 0.0, 4.0, 1);
    h.sync();
    h.eval_7d("(def t0 (track 0)) (def c (first t0.clips))");
    let base = h.app.history.undo_len();
    let undo = base;
    h.run_7d("(set! c.start 8)");
    assert_eq!(h.committed(a), Some((8.0, 12.0, Some(1))));
    assert_eq!(h.eval_7d("c.start"), Value::Number(8.0));
    assert_eq!(h.eval_7d("c.end"), Value::Number(12.0));
    h.run_7d("(set! c.end 16)");
    assert_eq!(h.committed(a), Some((8.0, 16.0, Some(1))));
    h.run_7d("(set! song.loop true) (set! song.end 32)");
    assert!(h
        .app
        .state
        .with_committed_song(|song| song.unwrap().loop_enabled));
    assert_eq!(h.eval_7d("song.end"), Value::Number(32.0));
    assert_eq!(h.app.history.undo_len() - undo, 4, "an undo entry per edit");
    // A setter given the current value changes nothing.
    h.run_7d("(set! c.start 8) (set! c.end 16) (set! song.loop true) (set! song.end 32)");
    assert_eq!(h.app.history.undo_len() - undo, 4);
    // Another pattern of the track.
    let pattern = h.second_pattern();
    h.eval_7d(&format!(
        "(def other (first (filter (lambda (x) (= x.pid {pattern})) t0.cells)))"
    ));
    // Launching a cell makes it the scene's cell on its track (the cloned
    // scene plays the new pattern).
    assert_eq!(h.eval_7d("other.assigned"), Value::Bool(true));
    h.eval_7d("(def first-cell c.cell)");
    assert_eq!(h.eval_7d("first-cell.assigned"), Value::Bool(false));
    h.run_7d("(launch-cell! first-cell)");
    assert_eq!(h.eval_7d("first-cell.assigned"), Value::Bool(true));
    assert_eq!(h.eval_7d("first-cell.active"), Value::Bool(true));
    assert_eq!(h.eval_7d("other.assigned"), Value::Bool(false));
    let undo = h.app.history.undo_len();
    h.run_7d("(set! c.cell other)");
    assert_eq!(h.committed(a), Some((8.0, 16.0, Some(pattern))));
    assert_eq!(h.eval_7d("c.cell"), h.eval_7d("other"));
    assert_eq!(h.app.history.undo_len(), undo + 1);
    // Undo restores, newest first.
    app::edit::undo(&mut h.app);
    assert_eq!(h.committed(a), Some((8.0, 16.0, Some(1))));
    while h.app.history.undo_len() > base {
        app::edit::undo(&mut h.app);
    }
    assert_eq!(h.committed(a), Some((0.0, 4.0, Some(1))));
    h.sync();
    assert_eq!(h.eval_7d("c.start"), Value::Number(0.0));
    assert_eq!(h.eval_7d("song.loop"), Value::Bool(false));
    // A drag view: start set!s while the pointer is down join one entry.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for start in [1, 2, 3] {
        h.run_7d(&format!("(set! c.start {start})"));
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.committed(a), Some((3.0, 7.0, Some(1))));
    assert_eq!(h.app.history.undo_len(), undo + 1, "the drag is one entry");
    // A discrete edit never stays open.
    h.gesture.pointer_down = true;
    h.run_7d("(set! song.loop true)");
    assert!(h.app.history.active_gesture().is_none());
    h.gesture.pointer_down = false;
    assert_eq!(h.app.history.undo_len(), undo + 2);
    // A user's fader drag stays one entry around a script edit in it.
    h.gesture.pointer_down = true;
    let fader = |value| app::AppCommand::SetTrackVolume { track: 1, value };
    app::try_apply_command(&mut h.app, fader(0.3)).expect("drag");
    let drag = h.app.history.active_gesture().map(|gesture| gesture.id);
    h.run_7d("(set! c.end 9)");
    assert_eq!(h.committed(a), Some((3.0, 9.0, Some(1))));
    assert_eq!(h.app.history.active_gesture().map(|g| g.id), drag);
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), undo + 4);
    // Selection state: no history.
    let undo = h.app.history.undo_len();
    h.run_7d("(set! song.cursor 5) (set! song.bound-clip c)");
    assert_eq!(h.app.arrangement_cursor_beat, 5.0);
    assert_eq!(h.eval_7d("song.cursor"), Value::Number(5.0));
    let selected = h.app.song_clip_selection.map(|selection| selection.clip_id);
    assert_eq!(selected, Some(a));
    assert_eq!(h.eval_7d("song.bound-clip"), h.eval_7d("c"));
    // The clip's span is its one-clip region.
    assert_eq!(h.eval_7d("song.region.start"), Value::Number(3.0));
    // A free region releases the clip binding.
    h.run_7d("(select-region! (track 1) t0 1 2)");
    assert_eq!(h.eval_7d("song.bound-clip"), Value::Nil);
    assert_eq!(h.eval_7d("region.tracks"), h.eval_7d("(list t0 (track 1))"));
    assert_eq!(h.eval_7d("region.end"), Value::Number(2.0));
    h.run_7d("(set! song.bound-clip c) (set! song.bound-clip nil) (clear-region!)");
    assert_eq!(h.eval_7d("song.region"), Value::Nil);
    assert_eq!(h.eval_7d("song.bound-clip"), Value::Nil);
    assert_eq!(h.app.history.undo_len(), undo);
}

#[test]
fn arrangement_setters_take_beats_flags_and_cells_and_reject_the_rest() {
    let mut h = Harness::new();
    h.clip(0, 4.0, 8.0, 1);
    h.clip(1, 0.0, 4.0, 1);
    h.sync();
    h.eval_7d("(def c (first (track-clips 0))) (def c1 (first (track-clips 1)))");
    h.rejects_7d("(set! c.start -1)", "at least 0");
    h.rejects_7d("(set! c.end 4)", "after the clip's start");
    h.rejects_7d(
        "(set! c.cell (first (track-cells 1)))",
        "a cell of the clip's track",
    );
    h.rejects_7d("(set! c.cell nil)", "a cell of the clip's track");
    h.rejects_7d("(set! song.manual-latch true)", "false");
    h.rejects_7d("(let ((t (track 0))) (set! t.latched true))", "false");
    h.rejects_7d("(set! song.cursor -2)", "at least 0");
    // The model refuses too, without an entry: an end before a clip's end.
    h.rejects_7d("(set! song.end 2)", "Cannot shorten");
    h.sync();
    let error = h.eval_7d("song.edit-error");
    assert!(
        matches!(&error, Value::String(e) if e.contains("Cannot shorten")),
        "{error:?}"
    );
    // Wrong types fail at set!.
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_7D}\n(set! song.loop 1)"))
        .expect_err("type");
    assert!(format!("{error:?}").contains(":bool"), "{error:?}");
}

#[test]
fn clips_and_cells_keep_identity_across_edits_reorders_and_project_load() {
    let mut h = Harness::new();
    let a = h.clip(1, 0.0, 4.0, 1);
    h.sync();
    h.eval_7d("(def t1 (track 1)) (def c (first t1.clips)) (def cl (first t1.cells))");
    let clip = h.eval_7d("c");
    let cell = h.eval_7d("cl");
    // Moving or resizing keeps the instance.
    h.run_7d("(set! c.start 4)");
    assert_eq!(h.eval_7d("(first t1.clips)"), clip);
    // Another clip and another pattern add instances; the others stay.
    let b = h.clip(1, 12.0, 16.0, 1);
    h.second_pattern();
    assert_eq!(h.eval_7d("(len t1.clips)"), Value::Number(2.0));
    assert_eq!(h.eval_7d("(first t1.clips)"), clip);
    let second = h.eval_7d("(let ((x (nth t1.clips 1))) x.cid)");
    assert_eq!(second, Value::Number(b.0 as f64));
    assert_eq!(h.eval_7d("(first t1.cells)"), cell);
    assert_eq!(h.eval_7d("(len t1.cells)"), Value::Number(2.0));
    // Deleting the track in front moves track 1 to 0: its clips and cells
    // move with it; undo moves them back.
    h.app.delete_track_recorded(0).expect("delete");
    h.sync();
    assert_eq!(h.eval_7d("t1.index"), Value::Number(0.0));
    assert_eq!(h.eval_7d("(first (track-clips 0))"), clip);
    assert_eq!(h.eval_7d("(first (track-cells 0))"), cell);
    assert_eq!(h.eval_7d("c.track"), h.eval_7d("t1"));
    // A setter lands on the clip by id after the reorder.
    h.run_7d("(set! c.end 10)");
    assert_eq!(h.committed(a), Some((4.0, 10.0, Some(1))));
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    h.sync();
    assert_eq!(h.eval_7d("t1.index"), Value::Number(1.0));
    assert_eq!(h.eval_7d("(first (track-clips 1))"), clip);
    assert_eq!(h.eval_7d("(first (track-cells 1))"), cell);
    assert_eq!(h.eval_7d("c.end"), Value::Number(8.0));
    // A deleted clip's instance goes stale.
    let Value::Instance(clip_id) = clip else {
        panic!("{clip:?}")
    };
    h.app.arr_clip_delete(a).expect("delete");
    h.sync();
    assert!(!h.rt().instance_is_live(clip_id));
    assert_eq!(h.eval_7d("(len t1.clips)"), Value::Number(1.0));
    // A project load replaces them all.
    let Value::Instance(cell_id) = cell else {
        panic!("{cell:?}")
    };
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(cell_id));
    assert_eq!(h.eval_7d("(len (track-clips 1))"), Value::Number(0.0));
    assert_eq!(h.eval_7d("(len (track-cells 1))"), Value::Number(1.0));
    assert_ne!(
        h.eval_7d("(first (track-cells 1))"),
        Value::Instance(cell_id)
    );
}

#[test]
fn scene_spans_are_positional_and_follow_the_scene_lane() {
    let mut h = Harness::new();
    h.second_pattern();
    h.app.arr_scene_event_insert(0.0, 0).expect("scene change");
    h.app.arr_scene_event_insert(8.0, 1).expect("scene change");
    h.sync();
    assert_eq!(h.eval_7d("(len song.spans)"), Value::Number(2.0));
    h.eval_7d("(def s0 (first song.spans)) (def s1 (nth song.spans 1))");
    assert_eq!(h.eval_7d("s1.start"), Value::Number(8.0));
    assert_eq!(h.eval_7d("s1.end"), h.eval_7d("song.end"));
    assert_eq!(h.eval_7d("s1.scene"), h.eval_7d("(nth (scenes) 1)"));
    assert_eq!(h.eval_7d("s0.end"), Value::Number(8.0));
    // A change in between: positions keep their instances, values move.
    h.app.arr_scene_event_insert(4.0, 1).expect("scene change");
    h.sync();
    assert_eq!(h.eval_7d("(len song.spans)"), Value::Number(3.0));
    assert_eq!(h.eval_7d("(nth song.spans 1)"), h.eval_7d("s1"));
    assert_eq!(h.eval_7d("s1.start"), Value::Number(4.0));
    h.eval_7d("(def s2 (nth song.spans 2))");
    app::edit::undo(&mut h.app);
    h.sync();
    assert_eq!(h.eval_7d("(len song.spans)"), Value::Number(2.0));
    assert_eq!(h.eval_7d("s1.start"), Value::Number(8.0));
    assert_eq!(
        h.eval_7d("s2.start"),
        Value::Number(0.0),
        "a dropped span is stale"
    );
}

#[test]
fn arrangement_live_fields_are_computed_only_while_observed() {
    let mut h = Harness::new();
    h.second_pattern();
    h.clip(0, 0.0, 4.0, 1);
    h.sync();
    let keys = [
        f::SONG_POSITION,
        f::SONG_MANUAL_LATCH,
        f::SONG_SCENE_LATCHED,
        f::TRACK_LATCHED,
        f::CELL_QUEUED,
        f::CELL_SELECTED,
    ];
    for _ in 0..4 {
        h.shared.state.latch_song_manual_override([0]);
        h.sync();
        h.shared.state.clear_song_manual_latch();
        h.sync();
    }
    for key in keys {
        assert_eq!(h.computed(key), 0, "{key:?} computed while unobserved");
    }
    // A cold read answers without observing.
    h.shared.state.latch_song_manual_override([0]);
    assert_eq!(
        h.eval_7d("(let ((t (track 0))) t.latched)"),
        Value::Bool(true)
    );
    assert_eq!(h.eval_7d("song.manual-latch"), Value::Bool(true));
    h.shared.state.clear_song_manual_latch();
    // One observed cell of several: one computation per tick.
    h.eval_7d("(def c0 (first (track-cells 0))) (def sel #'c0.selected) (def pos #'song.position)");
    h.sync();
    let (cells, positions) = (h.computed(f::CELL_SELECTED), h.computed(f::SONG_POSITION));
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.computed(f::CELL_SELECTED), cells + 3);
    assert_eq!(h.computed(f::SONG_POSITION), positions + 3);
    assert_eq!(h.computed(f::CELL_QUEUED), 0);
    // Selecting the cell as the delete target reaches the binding.
    h.run_7d("(set! c0.selected true)");
    assert_eq!(h.slot("sel"), 1.0);
    h.run_7d("(set! c0.selected false)");
    assert_eq!(h.slot("sel"), 0.0);
    h.eval_7d("(set! sel nil) (set! pos nil)");
    h.sync();
    h.sync();
    let (cells, positions) = (h.computed(f::CELL_SELECTED), h.computed(f::SONG_POSITION));
    h.sync();
    assert_eq!(h.computed(f::CELL_SELECTED), cells);
    assert_eq!(h.computed(f::SONG_POSITION), positions);
}

#[test]
fn a_script_drag_over_several_clips_is_one_entry_and_occludes_only_where_it_ends() {
    let mut h = Harness::new();
    let a = h.clip(0, 0.0, 4.0, 1);
    let b = h.clip(0, 8.0, 12.0, 1);
    let other = h.clip(1, 0.0, 4.0, 1);
    h.sync();
    h.eval_7d("(def a (first (track-clips 0))) (def b (nth (track-clips 0) 1))");
    h.eval_7d("(def o (first (track-clips 1)))");
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    // A crosses B: B is truncated while A lies on it, restored once A moves
    // past it.
    h.run_7d("(set! a.start 6)");
    assert_eq!(
        h.committed(b),
        Some((10.0, 12.0, Some(1))),
        "occluded under A"
    );
    h.run_7d("(set! a.start 8)");
    assert_eq!(h.committed(b), None, "B lies under A");
    h.run_7d("(set! a.start 16)");
    assert_eq!(h.committed(a), Some((16.0, 20.0, Some(1))));
    assert_eq!(
        h.committed(b),
        Some((8.0, 12.0, Some(1))),
        "B is intact again"
    );
    // Another clip, its start and end in one frame, and the song end join
    // the same entry.
    h.run_7d("(set! o.start 2) (set! o.end 8) (set! song.end 40)");
    assert_eq!(h.committed(other), Some((2.0, 8.0, Some(1))));
    assert_eq!(
        h.committed(a),
        Some((16.0, 20.0, Some(1))),
        "A's target holds"
    );
    assert_eq!(h.eval_7d("song.end"), Value::Number(40.0));
    // A rejected frame changes nothing and keeps the targets.
    h.rejects_7d("(set! song.end 4)", "Cannot shorten");
    assert_eq!(h.committed(a), Some((16.0, 20.0, Some(1))));
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(
        h.app.history.undo_len(),
        undo + 1,
        "the whole drag is one entry"
    );
    // Only the final overlap occludes: A ends on B's tail.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7d("(set! a.start 2) (set! a.start 10)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.committed(a), Some((10.0, 14.0, Some(1))));
    assert_eq!(h.committed(b), Some((8.0, 10.0, Some(1))));
    assert_eq!(h.app.history.undo_len(), undo + 1);
    // Undo restores the arrangement before the drag.
    app::edit::undo(&mut h.app);
    assert_eq!(h.committed(a), Some((16.0, 20.0, Some(1))));
    assert_eq!(h.committed(b), Some((8.0, 12.0, Some(1))));
    app::edit::undo(&mut h.app);
    assert_eq!(h.committed(a), Some((0.0, 4.0, Some(1))));
    assert_eq!(h.committed(other), Some((0.0, 4.0, Some(1))));
}

#[test]
fn a_one_shot_end_past_a_take_grows_it_and_a_drag_clamps() {
    let mut h = Harness::new();
    h.clip(0, 0.0, 4.0, 1);
    let take = h.app.song_region_to_take(0, 0.0, 4.0).expect("take");
    let (id, end) = h
        .app
        .state
        .with_committed_arrangement(|arrangement| {
            let clip = arrangement?.track_lanes[0]
                .iter()
                .find(|clip| clip.take_id == Some(take.0))
                .copied()?;
            Some((clip.id, clip.end_beat))
        })
        .expect("take clip");
    assert_eq!(end, 4.0);
    h.sync();
    h.eval_7d("(def c (first (track-clips 0)))");
    assert_eq!(h.eval_7d("c.take"), Value::Number(take.0 as f64));
    // While the pointer is down the end clamps to the take's.
    h.gesture.pointer_down = true;
    h.run_7d("(set! c.end 12)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.committed(id).map(|clip| clip.1), Some(4.0));
    // A one-shot set! asks for the length, as the timeline's resize.
    let undo = h.app.history.undo_len();
    h.run_7d("(set! c.end 12)");
    assert_eq!(h.committed(id).map(|clip| clip.1), Some(12.0));
    assert_eq!(h.app.history.undo_len(), undo + 1);
    let steps = h
        .app
        .state
        .track_take(0, take)
        .expect("take")
        .total_len_steps;
    assert!(steps >= 48, "the take grew: {steps}");
}

#[test]
fn arrangement_setter_rejections_latch_the_edit_error_and_stale_ids_fail() {
    let mut h = Harness::new();
    let a = h.clip(0, 4.0, 8.0, 1);
    h.sync();
    h.eval_7d("(def c (first (track-clips 0)))");
    // The setter's own rejections latch like the model's.
    for (code, message) in [
        ("(set! c.start -1)", "at least 0"),
        ("(set! c.end 2)", "after the clip's start"),
        ("(set! song.end -3)", "at least 0"),
    ] {
        h.app.song_edit_error = None;
        h.rejects_7d(code, message);
        h.sync();
        let error = h.eval_7d("song.edit-error");
        assert!(
            matches!(&error, Value::String(e) if e.contains(message)),
            "{code}: {error:?}"
        );
    }
    // Selection state errors are reported, not latched.
    h.app.song_edit_error = None;
    h.rejects_7d("(set! song.cursor -1)", "at least 0");
    assert_eq!(h.app.song_edit_error, None);
    // A pattern id that is not in the track's pool.
    h.app.song_edit_error = None;
    let track = h.eval_7d("(let ((t (track 0))) t.tid)");
    h.command(
        "set-clip",
        map_value([
            ("clip-id", Value::Number(a.0 as f64)),
            ("field", s("cell")),
            ("track-id", track),
            ("pattern-id", Value::Number(999.0)),
        ]),
    );
    assert_eq!(
        h.app.song_edit_error.as_deref(),
        Some("the pattern is gone")
    );
    assert_eq!(h.committed(a), Some((4.0, 8.0, Some(1))));
}

#[test]
fn regions_take_the_scene_lane_and_clear_the_singleton() {
    let mut h = Harness::new();
    h.sync();
    h.eval_7d("(def t0 (track 0)) (def t1 (track 1))");
    h.run_7d("(select-region! t0 t1 2 6 :scene-lane true)");
    assert!(h.app.song_region_selection.expect("region").scene_lane);
    assert_eq!(h.eval_7d("region.scene-lane"), Value::Bool(true));
    assert_eq!(h.eval_7d("song.region"), h.eval_7d("region"));
    h.run_7d("(select-region! t0 t1 2 6)");
    assert_eq!(h.eval_7d("region.scene-lane"), Value::Bool(false));
    h.rejects_7d(
        "(select-region! t0 t1 2 6 :scene-lane 1)",
        "scene-lane takes",
    );
    h.run_7d("(clear-region!)");
    assert_eq!(h.eval_7d("song.region"), Value::Nil);
    assert_eq!(h.eval_7d("region.tracks"), list_value(Vec::<Value>::new()));
    assert_eq!(h.eval_7d("region.start"), Value::Number(0.0));
    assert_eq!(h.eval_7d("region.end"), Value::Number(0.0));
    assert_eq!(h.eval_7d("region.scene-lane"), Value::Bool(false));
}

#[test]
fn clip_dots_follow_palette_colors_and_are_gray_without_one() {
    let mut h = Harness::new();
    h.clip(0, 0.0, 4.0, 1);
    h.sync();
    h.eval_7d("(def c (first (track-clips 0)))");
    assert_eq!(
        h.eval_7d("c.dot"),
        Value::Bool(true),
        "pattern 1 has a patch"
    );
    let patch = h
        .app
        .state
        .with_project_scenes(|scenes| scenes.track_pools[0].refs(PatternId(1)))
        .expect("refs")
        .patch;
    let set_color = |h: &mut Harness, color: Option<u8>| {
        h.app.state.with_scenes_mut(|scenes| {
            let meta = scenes.track_pools[0].sounds.patch_meta.get_mut(&patch);
            meta.expect("meta").color = color;
        });
        h.sync();
    };
    h.sync(); // after the first import's schema load
    let syncs = h.frame.host_kinds.song.structure_syncs;
    set_color(&mut h, None);
    let gray = eseqlisp::widget_render::timeline::SOUND_DOT_GRAY;
    let gray = rgb3([gray.r, gray.g, gray.b]);
    assert_eq!(h.eval_7d("c.dot-color"), gray, "gray without a color");
    set_color(&mut h, Some(3));
    let (_, themed) = sound_palette_rgb(Some(3)).expect("palette color");
    assert_eq!(h.eval_7d("c.dot-color"), rgb3(themed), "the palette color");
    assert_eq!(
        h.frame.host_kinds.song.structure_syncs, syncs,
        "a color change rebuilds no clips"
    );
    // The legacy join agrees.
    let sounds = h.app.song_clip_sounds();
    assert_eq!(sounds[0][0].2, Some(3));
}

#[test]
fn cells_are_addressed_by_stable_ids_across_a_track_reorder() {
    let mut h = Harness::new();
    h.second_pattern();
    h.sync();
    h.eval_7d("(def t1 (track 1)) (def cl (first t1.cells))");
    let pid = num(h.eval_7d("cl.pid")) as u64;
    // The set! selects at once (a UI-thread delete target); the launch
    // queues, and the track in front goes before it lands.
    h.eval_7d("(set! cl.selected true) (launch-cell! cl)");
    let target = h.shared.active_delete_target.lock().unwrap().clone();
    assert!(
        track_pattern_cell_selected(target.as_ref(), 1, pid),
        "selected before the launch lands: {target:?}"
    );
    h.app.delete_track_recorded(0).expect("delete");
    h.drain();
    h.sync();
    assert_eq!(h.eval_7d("t1.index"), Value::Number(0.0));
    assert_eq!(h.eval_7d("cl.assigned"), Value::Bool(true), "launched");
    // Deselecting is synchronous too.
    h.eval_7d("(set! cl.selected true)");
    h.eval_7d("(set! cl.selected false)");
    assert!(h.shared.active_delete_target.lock().unwrap().is_none());
}

#[test]
fn a_knob_drag_rebuilds_no_clips_or_cells() {
    let mut h = Harness::new();
    h.clip(0, 0.0, 4.0, 1);
    h.sync();
    let syncs = h.frame.host_kinds.song.structure_syncs;
    let models = h.model_syncs();
    h.gesture.pointer_down = true;
    for value in [0.2, 0.4, 0.6] {
        let fader = app::AppCommand::SetTrackVolume { track: 1, value };
        app::try_apply_command(&mut h.app, fader).expect("drag");
        h.sync();
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    h.sync();
    assert!(h.model_syncs() > models, "the drag moved the model");
    assert_eq!(h.frame.host_kinds.song.structure_syncs, syncs);
    // A song edit does.
    h.app.arr_set_loop(true).expect("loop");
    h.sync();
    assert_eq!(h.frame.host_kinds.song.structure_syncs, syncs + 1);
}
