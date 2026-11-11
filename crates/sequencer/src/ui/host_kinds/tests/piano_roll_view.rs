//! The factory piano roll (`ui/piano-roll.lisp`) ported to the kinds
//! (kind-bindings spec §13 stage 8, eseq-0l17.16): the note grid over
//! `piano-roll.notes`, the clip panel over `piano-roll.clip`, the automation
//! lane over `piano-roll.steps` and the device params' and rack macros'
//! locks, and the view's own state.

use super::views::{assert_ported, distro, instance_bindings, widget_keyed, widgets_with_prop};
use super::*;
use sequencer::sequencer::{LaneSource, PatternId, ProjectArrangement, StepParam};

const REFER_PR: &str = "(import eseq.kinds :refer (track piano-roll device-param lock-param!))
     (def view eseq.piano-roll/piano-roll-view)
     (def lane-view eseq.piano-roll/lane-view)
     (def lane-of (i) (eseq.piano-roll/current-lane (track i)))
     (def points-of (i)
       (map (lambda (p) (list (get p :step) (get p :start) (get p :end) (get p :value) (get p :locked)))
            (get (lane-of i) :points)))
     (def act (kind step value) (eseq.piano-roll/automation-action kind step value))";

impl Harness {
    fn eval_pr(&mut self, code: &str) -> Value {
        self.eval_with(REFER_PR, code)
    }

    /// The factory DAW with the piano roll open on track `track`'s notes.
    fn open_piano_roll(&mut self, track: usize) {
        self.eval_pr(&format!(
            "(eseq.seq-panels/seq-open-piano-roll-bottom-for-track {track})"
        ));
        self.sync();
        self.show_all();
    }

    /// The piano roll's timeline widget.
    fn piano_roll_timeline(&self) -> HashMap<String, Value> {
        let (tree, _) = self.buffer_tree("*piano-roll*");
        let mut timelines = Vec::new();
        widgets_with_prop(&tree, "lanes", &mut timelines);
        timelines.pop().expect("the piano roll's timeline")
    }
}

#[test]
fn ported_piano_roll_uses_no_legacy_binding_forms() {
    assert_ported(&[(
        "ui/piano-roll.lisp",
        include_str!("../../../../../../content/ui/piano-roll.lisp"),
    )]);
}

/// The grid draws the notes by the ids the host's editor addresses, the
/// lanes from the pitch range, the track's color; the playhead binds, so
/// playback only repaints.
#[test]
fn the_note_grid_draws_the_notes_and_binds_the_playhead() {
    let mut h = distro();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0), (7.0, 2.0, 0.0)]);
    h.write_notes(0, 3, &[(-5.0, 0.5, 0.25)]);
    h.open_piano_roll(0);
    h.shared
        .piano_roll_selection
        .lock()
        .unwrap()
        .insert(piano_roll_item_id(0, 1));
    h.sync();
    h.show_all();
    let timeline = h.piano_roll_timeline();
    let item = |index: usize| items(&timeline["items"])[index].clone();
    let ids: Vec<Value> = (0..3).map(|index| get(&item(index), "id")).collect();
    let expected = [(0, 0), (0, 1), (3, 0)]
        .map(|(step, voice)| number(piano_roll_item_id(step, voice) as f64));
    assert_eq!(ids, expected);
    assert_eq!(get(&item(1), "lane"), number(48.0 - 7.0));
    assert_eq!(get(&item(1), "end"), number(2.0));
    assert_eq!(get(&item(2), "start"), number(3.25));
    assert_eq!(get(&item(2), "label"), s("G3 +0.25"));
    assert_eq!(get(&item(1), "selected"), Value::Bool(true));
    assert!(!timeline.contains_key("selection"), "the items carry it");
    let lanes = items(&timeline["lanes"]);
    assert_eq!(lanes.len(), 97, "pitch-max to pitch-min");
    assert_eq!(get(&lanes[48], "label"), s("C4"));
    assert_eq!(get(&lanes[0], "label"), s("C8"));
    assert_eq!(
        get(&lanes[49], "sidebar-bg"),
        Value::Keyword("white".into())
    );
    assert_eq!(
        get(&lanes[47], "sidebar-bg"),
        Value::Keyword("black".into())
    );
    assert_eq!(
        timeline["item-color"],
        h.eval_pr("(let ((t (track 0))) t.color)")
    );
    assert_eq!(timeline["content-length"], number(16.0));

    let (tree, revision) = h.buffer_tree("*piano-roll*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new(), "legacy bindings");
    let roll = h.singleton(PIANO_ROLL);
    assert!(bound.contains(&(roll, "playhead".to_string())), "{bound:?}");
    // Playback moves the playhead: a repaint, never a re-render.
    let transport = &h.shared.state.transport;
    transport.playing.store(true, Ordering::Relaxed);
    transport.track_playheads[0].store(5, Ordering::Relaxed);
    h.sync();
    h.show_all();
    assert_eq!(h.buffer_tree("*piano-roll*").1, revision);
    assert_eq!(h.eval_pr("piano-roll.playhead"), number(5.0));
}

/// Scroll, zoom, the cursor, the new-note length and the fit are the view's
/// own state (the legacy `defstate`s' behaviour, at the default lower-pane
/// height with the automation row under the grid).
#[test]
fn the_view_scrolls_zooms_and_fits_its_notes() {
    let mut h = distro();
    h.open_piano_roll(0);
    let field = |h: &mut Harness, name: &str| h.eval_pr(&format!("(get-view-field \"{name}\")"));
    h.eval_pr(
        "(def get-view-field (name)
           (match name \"start\" view.start \"duration\" view.duration
                       \"lane-scroll\" view.lane-scroll \"lane-height\" view.lane-height))",
    );
    // Nothing to fit: C4 centred over the visible lanes.
    h.eval_pr("(eseq.piano-roll/piano-roll-request-fit-for-track 0)");
    assert_eq!(field(&mut h, "lane-scroll"), number(44.0));
    let action = |h: &mut Harness, event: &str| {
        h.eval_pr(&format!(
            "(eseq.piano-roll/piano-roll-action (dict {event}))"
        ));
    };
    // A lane scroll stops at the last lanes that fill the grid.
    action(
        &mut h,
        ":type :scroll-view :lane-scroll 1000 :delta-lanes 0",
    );
    assert_eq!(field(&mut h, "lane-scroll"), number(88.0));
    // A created or resized note's length is the next new note's.
    action(&mut h, ":type :finish-create-item :start 20 :end 22.5");
    h.show_all();
    assert_eq!(h.piano_roll_timeline()["create-duration"], number(2.5));
    action(&mut h, ":type :resize-item-absolute :duration 3.25");
    action(&mut h, ":type :clear-selection :time 4.5");
    h.show_all();
    assert_eq!(h.piano_roll_timeline()["create-duration"], number(3.25));
    assert_eq!(h.eval_pr("view.cursor"), number(4.5));
    // A time zoom keeps the lane height; a scroll stops at the axis' end.
    h.eval_pr("(set! view.duration 8) (set! view.lane-height 1)");
    action(&mut h, ":type :zoom-view :anchor-time 4 :factor 2");
    assert_eq!(field(&mut h, "lane-height"), number(1.0));
    assert_eq!(field(&mut h, "duration"), number(4.0));
    h.eval_pr("(set! view.duration 8)");
    action(&mut h, ":type :scroll-view :delta-time 100");
    assert_eq!(field(&mut h, "start"), number(12.0));
    // Fit to notes on lanes 20 and 60, from step 2 to 16.
    h.write_notes(0, 2, &[(28.0, 2.0, 0.0)]);
    h.write_notes(0, 14, &[(-12.0, 2.0, 0.0)]);
    h.sync();
    h.eval_pr("(set! view.lane-height 0.5) (eseq.piano-roll/piano-roll-request-fit)");
    assert_eq!(field(&mut h, "start"), number(1.0));
    assert_eq!(field(&mut h, "duration"), number(16.0));
    assert_eq!(field(&mut h, "lane-scroll"), number(36.0));
    assert_eq!(h.eval_pr("view.fit"), Value::Nil, "applied");
}

/// The scroll, zoom and cursor bind (the timeline's and the lane's view
/// axis): a scroll or a cursor move repaints, never re-renders the buffer.
#[test]
fn scrolling_and_moving_the_cursor_only_repaint() {
    let mut h = distro();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.open_piano_roll(0);
    let (tree, revision) = h.buffer_tree("*piano-roll*");
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    let Value::Instance(view) = h.eval_pr("view") else {
        panic!("the view singleton")
    };
    for field in ["start", "duration", "cursor"] {
        assert!(
            bound.contains(&(view, field.to_string())),
            "{field}: {bound:?}"
        );
    }
    for event in [
        ":type :scroll-view :delta-time 4 :delta-lanes 0",
        ":type :set-cursor :time 3",
    ] {
        h.eval_pr(&format!(
            "(eseq.piano-roll/piano-roll-action (dict {event}))"
        ));
        h.sync();
        h.show_all();
        assert_eq!(h.buffer_tree("*piano-roll*").1, revision, "{event}");
    }
    assert_eq!(
        h.eval_pr("(list view.start view.cursor)"),
        h.eval_pr("(list 4 3)")
    );
}

/// A fit asked for another track waits until the piano roll shows that
/// track's notes (the host invokes the view after each note sync).
#[test]
fn a_fit_for_another_track_waits_for_its_notes() {
    let mut h = distro();
    h.open_piano_roll(0);
    h.write_notes(1, 12, &[(0.0, 1.0, 0.0)]);
    h.eval_pr("(eseq.piano-roll/piano-roll-request-fit-for-track 1)");
    assert_eq!(h.eval_pr("view.fit"), h.eval_pr("(track 1)"), "pending");
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_pr("view.fit"), Value::Nil, "applied");
    assert_eq!(
        h.eval_pr("(list view.start view.duration view.lane-scroll)"),
        h.eval_pr("(list 11 4 44)")
    );
}

/// The lane shows a step param's values at the active steps, or a device
/// param's locks and its base; the picker lists the params that carry a
/// lock; an edit writes the step param or the lock.
#[test]
fn the_automation_lane_shows_and_writes_step_params_and_device_locks() {
    let mut h = distro();
    h.add_effect(0, "Filter");
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 2.0, 0.5)]);
    h.open_piano_roll(0);
    assert_eq!(h.eval_pr("(get (lane-of 0) :label)"), s("Velocity"));
    assert_eq!(
        h.eval_pr("(points-of 0)"),
        h.eval_pr("(list (list 0 0 1 1 true) (list 4 4.5 6.5 1 true))")
    );
    // A step param edit: the source's step, one undo entry; a clear is the
    // default again.
    let before = h.app.history.undo_len();
    h.eval_pr("(act :set 4 0.25)");
    assert_eq!(
        h.eval_pr("lane-view.edit-value"),
        number(0.25),
        "the readout"
    );
    h.drain();
    h.sync();
    let velocity = |h: &Harness| h.shared.state.pattern.step_data[0].get(4, StepParam::Velocity);
    assert_eq!(velocity(&h), 0.25);
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.eval_pr("(act :finish 4 0.25)");
    assert_eq!(h.eval_pr("lane-view.edit-value"), Value::Nil);
    h.eval_pr("(act :clear 4 0)");
    h.drain();
    h.sync();
    assert_eq!(velocity(&h), 1.0);

    // A locked device param joins the picker, labelled by its device.
    h.eval_pr(
        r#"(def flt (first (filter (lambda (d) (= d.type "Filter")) (let ((t (track 0))) t.devices))))
           (def cutoff (device-param flt "cutoff"))
           (lock-param! cutoff (list (nth (let ((t (track 0))) t.steps) 6)) 800)"#,
    );
    h.drain();
    h.sync();
    let labels =
        h.eval_pr("(map (lambda (o) (get o :label)) (eseq.piano-roll/lane-options (track 0)))");
    assert!(items(&labels).contains(&s("Filter cutoff")), "{labels:?}");
    h.eval_pr(r#"(eseq.piano-roll/select-lane (track 0) "Filter cutoff")"#);
    assert_eq!(h.eval_pr("lane-view.param"), h.eval_pr("cutoff"));
    assert_eq!(h.eval_pr("(get (lane-of 0) :label)"), s("Filter cutoff"));
    // The base (gray) at the active steps, the lock (an off-step one too).
    assert_eq!(
        h.eval_pr("(points-of 0)"),
        h.eval_pr("(list (list 0 0 1 cutoff.base false) (list 4 4.5 6.5 cutoff.base false) (list 6 6 6 800 true))")
    );
    // An edit locks the step; a clear unlocks it.
    h.eval_pr("(act :set 0 1200)");
    h.drain();
    h.sync();
    assert_eq!(
        h.eval_pr("cutoff.step-locks"),
        h.eval_pr("(list (list 0 1200) (list 6 800))")
    );
    h.eval_pr("(act :clear 6 0)");
    h.drain();
    h.sync();
    assert_eq!(
        h.eval_pr("cutoff.step-locks"),
        h.eval_pr("(list (list 0 1200))")
    );
    // On another track the param is not the lane: the step param shows.
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_pr("(get (lane-of 1) :label)"), s("Velocity"));
}

/// A rack macro's lane locks the rack track's steps.
#[test]
fn the_automation_lane_locks_a_rack_macro() {
    let mut h = distro();
    h.rack_track();
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.sync();
    h.open_piano_roll(2);
    h.eval_pr(
        "(def rm (first (let ((d (first (let ((t (track 2))) t.devices)))) d.macros)))
         (set! lane-view.macro rm)",
    );
    assert_eq!(h.eval_pr("(get (lane-of 2) :max)"), number(1.0));
    h.eval_pr("(act :set 3 0.25)");
    h.drain();
    h.sync();
    let lock =
        |h: &Harness| h.shared.state.live_rack_track_snapshot(2).unwrap().macros[0].plocks[3];
    assert_eq!(lock(&h), Some(0.25));
    assert_eq!(h.eval_pr("rm.has-locks"), Value::Bool(true));
    let label = h.eval_pr("(str \"rack \" rm.name)");
    assert_eq!(h.eval_pr("(get (lane-of 2) :label)"), label, "listed now");
    h.eval_pr("(act :clear 3 0)");
    h.drain();
    h.sync();
    assert_eq!(lock(&h), None);
}

/// A pinned take's lane writes the take's chunk patterns, never the live
/// pattern; a drag of it is one undo entry.
#[test]
fn the_lane_edits_a_pinned_take_in_one_entry() {
    let mut h = distro();
    h.app
        .arr_replace(ProjectArrangement::new(2, 128.0))
        .unwrap();
    h.app.set_arrangement_view_visible(true);
    let (take, clip) = h.app.arr_empty_take_clip_create(0, 0.0, 128.0).unwrap();
    h.app
        .select_song_clip_span(0, clip, Some((0.0, 128.0)))
        .expect("select");
    h.open_piano_roll(0);
    assert_eq!(h.eval_pr("piano-roll.focus-kind"), s("take"));
    let chunks = h.shared.state.track_take(0, take).unwrap().chunks;
    let before = StepParam::Velocity.default_value();
    let velocity = |h: &Harness, pattern| {
        h.shared
            .state
            .with_pool_pattern(0, pattern, |data| {
                data.step_data[0][StepParam::Velocity.index()]
            })
            .unwrap()
    };
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [0.5, 0.25] {
        // Step 256: the take's second chunk.
        h.eval_pr(&format!("(act :set 256 {value})"));
        h.drain();
        h.sync();
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(velocity(&h, chunks[0]), before);
    assert_eq!(velocity(&h, chunks[1]), 0.25);
    assert_eq!(
        h.shared.state.pattern.step_data[0].get(0, StepParam::Velocity),
        before
    );
    assert_eq!(h.app.history.undo_len(), undo + 1, "one entry");
    h.undo();
    assert_eq!(velocity(&h, chunks[1]), before);
}

/// The entry mode: from arrangement clip gestures the piano roll shows the
/// selected clip, or that none is; from anywhere else the current track.
#[test]
fn the_arrangement_entry_shows_the_selected_clip_or_none() {
    let mut h = distro();
    h.eval_pr("(eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track 0)");
    h.sync();
    h.show_all();
    assert_eq!(
        h.eval_pr("(eseq.piano-roll/piano-roll-arrangement-mode?)"),
        Value::Bool(true)
    );
    let (tree, _) = h.buffer_tree("*piano-roll*");
    assert!(
        widget_keyed(&tree, "no-clip-selected").is_some(),
        "the empty state"
    );
    h.eval_pr("(eseq.seq-panels/seq-open-piano-roll-bottom-for-track 0)");
    h.show_all();
    assert_eq!(
        h.eval_pr("(eseq.piano-roll/piano-roll-arrangement-mode?)"),
        Value::Bool(false)
    );
    h.piano_roll_timeline();
}

/// A pinned clip: the clip panel shows its Start, End and Offset and the
/// grid its loop window.
#[test]
fn the_clip_panel_shows_the_pinned_clip() {
    let mut h = distro();
    h.command("clone-pattern", Value::Nil);
    h.sync();
    let source = LaneSource::Pattern(PatternId(1));
    let clip = (h.app)
        .arr_clip_create(0, 4.0, 6.0, source, 2.0)
        .expect("clip");
    h.app.set_arrangement_view_visible(true);
    (h.app)
        .select_song_clip_span(0, clip, Some((4.0, 6.0)))
        .expect("select");
    h.open_piano_roll(0);
    let (tree, _) = h.buffer_tree("*piano-roll*");
    let value_of = |suffix: &str| {
        let picker = widget_keyed(&tree, suffix).unwrap_or_else(|| panic!("{suffix}"));
        picker["value"].clone()
    };
    assert_eq!(value_of("panel-start"), h.eval_pr("piano-roll.clip.start"));
    assert_eq!(value_of("panel-end"), h.eval_pr("piano-roll.clip.end"));
    assert_eq!(value_of("panel-offset"), number(2.0));
    let timeline = h.piano_roll_timeline();
    assert_eq!(timeline["band-slide"], Value::Bool(true));
    assert_eq!(
        timeline["window-marker"],
        h.eval_pr("piano-roll.window-marker")
    );
}

/// A fit hands the timeline a lane to center in its real height (the pane
/// is ratio-sized and remembered, so the Lisp cannot know it); the first
/// lane scroll or lane zoom adopts the scroll the timeline showed.
#[test]
fn a_fit_centers_in_the_rendered_grid_until_the_lanes_scroll() {
    let mut h = distro();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.open_piano_roll(0);
    h.eval_pr("(eseq.piano-roll/piano-roll-request-fit-for-track 0)");
    let center = h.eval_pr("view.lane-center");
    assert!(matches!(center, Value::Number(_)), "{center:?}");
    h.show_all();
    assert_eq!(h.piano_roll_timeline()["lane-center"], center);
    // A purely horizontal scroll keeps the centering.
    h.eval_pr("(eseq.piano-roll/piano-roll-action (dict :type :scroll-view :delta-time 2 :delta-lanes 0))");
    assert_eq!(h.eval_pr("view.lane-center"), center);
    // A lane zoom measures from the scroll the timeline showed.
    h.eval_pr(
        "(eseq.piano-roll/piano-roll-action
           (dict :type :zoom-lanes :anchor-lane 50 :factor 1 :lane-scroll 40))",
    );
    assert_eq!(h.eval_pr("view.lane-center"), Value::Nil);
    assert_eq!(h.eval_pr("view.lane-scroll"), number(40.0));
}

/// Perf probe (ignored; run in release with `--run-ignored only
/// --no-capture`): a note dragged through the piano roll's real path — the
/// Lisp action, the queued `piano-roll-gesture-update`, the production
/// reactive tick and the re-render — per frame, split by frame kind: a
/// repeat of the last position, a move within the note's step, and a move
/// that crosses into the next step.
#[test]
#[ignore]
fn piano_roll_note_drag_frame_cost_probe() {
    use std::time::Instant;
    let mut h = distro();
    for track in 0..2 {
        for step in (0..16).step_by(2) {
            h.write_notes(track, step, &[(step as f32 % 7.0, 1.0, 0.0)]);
        }
    }
    h.sync();
    h.open_piano_roll(0);
    let note = h.eval_pr("(let ((n (first piano-roll.notes))) (list n.item (- eseq.kinds/pitch-max n.pitch) n.start))");
    let Value::List(parts) = note else { panic!("{note:?}") };
    let parts: Vec<f64> = parts.iter().map(|p| num(p.borrow().clone())).collect();
    let (id, lane, start) = (parts[0], parts[1], parts[2]);
    let frame = |h: &mut Harness, at: f64| {
        let t0 = Instant::now();
        h.eval_pr(&format!(
            "(eseq.piano-roll/piano-roll-action (dict :type :move-items-absolute :ids (list {id}) :anchor-id {id} :start {at} :lane {lane}))"
        ));
        let t1 = Instant::now();
        h.drain();
        let t2 = Instant::now();
        h.tick();
        let t3 = Instant::now();
        h.show_all();
        let t4 = Instant::now();
        [t1 - t0, t2 - t1, t3 - t2, t4 - t3]
    };
    let mut buckets: HashMap<&str, Vec<[Duration; 4]>> = HashMap::new();
    for _round in 0..3 {
        for step in 0..8 {
            let base = start + step as f64;
            for (kind, offset) in [("cross", 0.0), ("repeat", 0.0), ("within", 0.25), ("within", 0.5), ("repeat", 0.5)] {
                let cost = frame(&mut h, base + offset);
                buckets.entry(kind).or_default().push(cost);
            }
        }
        let cost = frame(&mut h, start);
        buckets.entry("cross").or_default().push(cost);
        h.eval_pr("(eseq.piano-roll/piano-roll-action (dict :type :finish-move-items))");
        h.drain();
        h.tick();
        h.show_all();
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    for (kind, costs) in &buckets {
        let n = costs.len() as f64;
        let mean = |i: usize| costs.iter().map(|c| ms(c[i])).sum::<f64>() / n;
        let total: Vec<f64> = costs.iter().map(|c| c.iter().map(|d| ms(*d)).sum()).collect();
        let mut sorted = total.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "[pr-drag] {kind:7} n={:3} median={:7.2}ms max={:7.2}ms | lisp={:6.2} drain={:6.2} tick={:6.2} render={:6.2}",
            costs.len(),
            sorted[sorted.len() / 2],
            sorted[sorted.len() - 1],
            mean(0), mean(1), mean(2), mean(3),
        );
    }
}

/// The lane footer's toggle hides and shows the automation lane; the
/// footer (toggle and picker) stays either way.
#[test]
fn the_lane_footer_toggle_hides_and_shows_the_lane() {
    let mut h = distro();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.open_piano_roll(0);
    let present = |h: &Harness, key: &str| {
        let (tree, _) = h.buffer_tree("*piano-roll*");
        widget_keyed(&tree, key).is_some()
    };
    assert!(present(&h, "automation-lane"));
    assert!(present(&h, "automation-lane-toggle"));
    assert!(present(&h, "automation-param"));
    h.eval_pr("(set! lane-view.shown false)");
    h.show_all();
    assert!(!present(&h, "automation-lane"), "hidden");
    assert!(present(&h, "automation-lane-toggle"));
    assert!(present(&h, "automation-param"));
    h.eval_pr("(set! lane-view.shown true)");
    h.show_all();
    assert!(present(&h, "automation-lane"), "shown again");
}

/// A note drag frame, through the real path (the action, the queued
/// `piano-roll-gesture-update`, the tick), bumps neither epoch, yet the
/// piano roll and the step grid both show the moved note; the release
/// bumps them once.
#[test]
fn a_note_drag_frame_bumps_no_epoch_but_every_view_follows() {
    let mut h = distro();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.open_piano_roll(0);
    let parts = h.eval_pr(
        "(let ((n (first piano-roll.notes))) (list n.item (- eseq.kinds/pitch-max n.pitch)))",
    );
    let Value::List(parts) = parts else { panic!("{parts:?}") };
    let parts: Vec<f64> = parts.iter().map(|p| num(p.borrow().clone())).collect();
    let (id, lane) = (parts[0], parts[1]);
    let step_active = |h: &mut Harness, step: usize| {
        h.eval_pr(&format!("(let ((s (nth (track 0).steps {step}))) s.active)"))
    };
    assert_eq!(step_active(&mut h, 2), Value::Bool(true));
    let epochs = |h: &Harness| {
        (
            h.shared.ui_epoch.load(Ordering::Relaxed),
            h.shared.fx_epoch.load(Ordering::Relaxed),
        )
    };
    let before = epochs(&h);
    h.eval_pr(&format!(
        "(eseq.piano-roll/piano-roll-action (dict :type :move-items-absolute :ids (list {id}) :anchor-id {id} :start 5 :lane {lane}))"
    ));
    h.drain();
    assert_eq!(epochs(&h), before, "a drag frame bumps no epoch");
    h.tick();
    h.show_all();
    assert_eq!(h.eval_pr("(map |n| n.start piano-roll.notes)"), h.eval_pr("(list 5)"));
    assert_eq!(step_active(&mut h, 5), Value::Bool(true), "the step grid follows");
    assert_eq!(step_active(&mut h, 2), Value::Bool(false));
    h.eval_pr("(eseq.piano-roll/piano-roll-action (dict :type :finish-move-items))");
    h.drain();
    let after = epochs(&h);
    assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1), "the release bumps once");
}
