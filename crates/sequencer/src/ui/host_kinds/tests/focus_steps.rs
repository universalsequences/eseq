//! Stage 7e-2: the piano roll's focus steps (`piano-roll.steps`,
//! `focus-step`) and the automation lane as a view over them.

use super::*;
use sequencer::sequencer::{LaneSource, PatternId, StepParam};

const REFER_7E2: &str = "(import eseq.kinds :refer (track piano-roll device-param lock-param! \
                         add-note! focus-step-params focus-step-value set-focus-step!))
     (def fs-at (i) (nth piano-roll.steps i))
     (def note-at (i) (nth piano-roll.notes i))";

impl Harness {
    fn eval_7e2(&mut self, code: &str) -> Value {
        let source = format!("{REFER_7E2}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Run `code`'s commands, then sync.
    fn run_7e2(&mut self, code: &str) {
        self.eval_7e2(code);
        self.drain();
        self.sync();
    }

    fn rejects_7e2(&mut self, code: &str, message: &str) {
        self.rejects_in(REFER_7E2, code, message, true);
    }

    /// Set `param` of `step` on `track`'s live pattern and publish the
    /// track, as an edit does.
    fn write_param(&mut self, track: usize, step: usize, param: StepParam, value: f32) {
        PianoRollLanes::live(&self.shared.state, track).set_step_param(step, param, value);
        self.shared.state.publish_scheduler_track(track);
    }

    fn live_param(&self, track: usize, step: usize, param: StepParam) -> f32 {
        self.shared.state.pattern.step_data[track].get(step, param)
    }

    fn focus_step_syncs(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().focus_steps.syncs
    }

    fn focus_steps_of(&self, track: u64) -> usize {
        let track = self.track_id(track);
        self.rt().keyed_children_of_kind(track, FOCUS_STEP).count()
    }

    /// Pin a clip of pool pattern 1 (a copy of track 0's pattern the
    /// current scene does not play), so the piano roll edits the pool.
    fn pin_pool_clip(&mut self) {
        let source = LaneSource::Pattern(PatternId(1));
        let clip = (self.app)
            .arr_clip_create(0, 4.0, 6.0, source, 2.0)
            .expect("clip");
        self.app.set_arrangement_view_visible(true);
        (self.app)
            .select_song_clip_span(0, clip, Some((4.0, 6.0)))
            .expect("select");
        self.sync();
    }
}

fn pool_lanes(h: &Harness) -> PianoRollLanes {
    PianoRollLanes::new(&h.shared.state, 0, PianoRollFocusSpec::Pool(PatternId(1)))
}

/// `(step start end value locked)` of each legacy lane point.
fn legacy_points(lane: &Value) -> Vec<Value> {
    let fields = ["step", "start", "end", "value", "locked"];
    (items(&get(lane, "points")).iter())
        .map(|point| list_value(fields.map(|field| get(point, field))))
        .collect()
}

#[test]
fn focus_steps_show_the_sources_steps_as_the_legacy_lane_did() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0), (7.0, 2.0, 0.0)]);
    h.write_notes(0, 3, &[(-5.0, 0.5, 0.25)]);
    h.write_param(0, 3, StepParam::Velocity, 0.5);
    h.sync();
    h.sync();
    assert_eq!(h.focus_steps_of(0), 0, "nothing read the steps yet");
    assert_eq!(h.eval_7e2("(len piano-roll.steps)"), Value::Number(16.0));
    h.sync();
    assert_eq!(h.focus_steps_of(0), 16);
    let row = |h: &mut Harness, i: usize| {
        h.eval_7e2(&format!(
            "(let ((fs (fs-at {i}))) (list fs.index fs.active fs.start fs.end fs.velocity))"
        ))
    };
    assert_eq!(row(&mut h, 0), h.eval_7e2("(list 0 true 0 2 1)"));
    assert_eq!(row(&mut h, 1), h.eval_7e2("(list 1 false 1 1 1)"));
    assert_eq!(row(&mut h, 3), h.eval_7e2("(list 3 true 3.25 3.75 0.5)"));
    assert_eq!(
        h.eval_7e2("(let ((fs (fs-at 0))) fs.track)"),
        h.eval_7e2("(track 0)")
    );
    assert_eq!(
        h.eval_7e2("(let ((fs (fs-at 3))) (focus-step-value fs \"velocity\"))"),
        Value::Number(0.5)
    );
    // Legacy parity: a step param's lane points are the active steps' span
    // and value.
    let state = h.shared.state.clone();
    let lanes = PianoRollLanes::live(&state, 0);
    for (param, key) in [
        (StepParam::Velocity, "velocity"),
        (StepParam::Delay, "delay"),
    ] {
        let lane = build_piano_roll_automation_value(
            &h.app,
            &state,
            &lanes,
            &PianoRollAutomationTarget::StepParam(param).key(),
        );
        let points = h.eval_7e2(&format!(
            "(map (lambda (fs) (list fs.index fs.start fs.end (focus-step-value fs \"{key}\") true))
                  (filter (lambda (fs) fs.active) piano-roll.steps))"
        ));
        assert_eq!(list_value(legacy_points(&lane)), points, "{key}");
    }
    // The picker's step params: the host's, in the legacy picker's order,
    // each one a focus-step field focus-step-value reads.
    let params = items(&h.eval_7e2("(focus-step-params)"));
    let legacy = items(&build_piano_roll_automation_params_value(&h.app, &state, 0));
    assert_eq!(params.len(), StepParam::VISIBLE.len());
    for ((row, param), legacy) in params.iter().zip(StepParam::VISIBLE).zip(&legacy) {
        assert_eq!(get(row, "label"), get(legacy, "label"));
        let Value::String(name) = get(row, "name") else {
            panic!("{row:?}");
        };
        assert_eq!(focus_step_param(&name), Some(param));
        assert_eq!(num(get(row, "max")) as f32, param.max(), "{name}");
        let read = format!("(let ((fs (fs-at 3))) (focus-step-value fs \"{name}\"))");
        let value = h.live_param(0, 3, param);
        assert_eq!(h.eval_7e2(&read), Value::Number(f64::from(value)), "{name}");
    }
    assert_eq!(focus_step_param("vel"), None, "field names only");
}

#[test]
fn a_device_params_lane_is_a_view_over_focus_steps_and_its_locks() {
    let (mut h, slot) = Harness::with_devices();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.write_notes(0, 4, &[(0.0, 2.0, 0.5)]);
    h.write_notes(0, 9, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e2(
        r#"(def t0 (track 0)) (def flt (first t0.devices))
           (def cutoff (device-param flt "cutoff"))
           (lock-param! cutoff (list (nth t0.steps 4) (nth t0.steps 6)) 800)
           ;; The lane's points: a locked step's lock, else an active
           ;; step's base (gray), as the legacy lane drew them.
           (def lane-points (p)
             (filter (lambda (x) x)
               (map (lambda (fs)
                      (let ((lock (first (filter (lambda (row) (= (first row) fs.index))
                                                 p.step-locks))))
                        (if lock
                          (list fs.index fs.start fs.end (nth lock 1) true)
                          (if fs.active (list fs.index fs.start fs.end p.base false) nil))))
                    piano-roll.steps)))"#,
    );
    h.drain();
    h.sync();
    assert_eq!(h.eval_7e2("cutoff.has-locks"), Value::Bool(true));
    let state = h.shared.state.clone();
    let lanes = PianoRollLanes::live(&state, 0);
    let target = PianoRollAutomationTarget::Effect {
        slot_idx: slot,
        param_idx: 2,
    };
    let lane = build_piano_roll_automation_value(&h.app, &state, &lanes, &target.key());
    let points = legacy_points(&lane);
    assert_eq!(points.len(), 4, "three active steps and an off-step lock");
    assert_eq!(list_value(points), h.eval_7e2("(lane-points cutoff)"));
}

#[test]
fn focus_step_setters_edit_the_source_through_history() {
    let mut h = Harness::new();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0), (4.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e2("(def a (fs-at 2)) (def b (fs-at 5))");
    h.sync();
    let base = h.app.history.undo_len();
    h.run_7e2("(set! a.velocity 0.25)");
    assert_eq!(h.live_param(0, 2, StepParam::Velocity), 0.25);
    assert_eq!(h.eval_7e2("a.velocity"), Value::Number(0.25));
    // A transpose moves the step's chord with it.
    h.run_7e2("(set-focus-step! a \"transpose\" 2)");
    assert_eq!(h.live_notes(0, 2), vec![(2.0, 1.0, 0.0), (6.0, 1.0, 0.0)]);
    // A step holding no note takes its params too.
    h.run_7e2("(set! b.retrig 3)");
    assert_eq!(h.live_param(0, 5, StepParam::Retrig), 3.0);
    assert_eq!(h.app.history.undo_len(), base + 3, "an undo entry per edit");
    assert!(h.app.history.active_gesture().is_none());
    // The current value changes nothing.
    h.run_7e2("(set! a.velocity 0.25) (set! b.retrig b.retrig)");
    assert_eq!(h.app.history.undo_len(), base + 3);
    // The value rule: out of range, an unknown field or a step past the
    // source's length is an error that changes nothing.
    h.rejects_7e2("(set! a.velocity 2)", "velocity takes a number from 0 to 1");
    h.rejects_7e2("(set! a.sync 8)", "sync takes a number from 0 to 7");
    h.rejects_7e2(
        "(set-focus-step! a \"speed\" 1)",
        "set-focus-step: a focus step has no settable field speed",
    );
    h.rejects_7e2(
        "(host-command \"set-focus-step\"
           (dict :track-id a.track.tid :index 16 :field \"pan\" :value 0))",
        "index takes an integer from 0 to 15",
    );
    // A param no focus step holds fails at once (a native's failure).
    h.editor.runtime_mut().take_status_message();
    h.eval_7e2("(focus-step-value a \"vel\")");
    let status = h
        .editor
        .runtime_mut()
        .take_status_message()
        .unwrap_or_default();
    assert!(status.contains("a focus step has no param vel"), "{status}");
    // Undo restores, newest first.
    h.undo();
    assert_eq!(h.live_param(0, 5, StepParam::Retrig), 0.0);
    h.undo();
    assert_eq!(h.live_notes(0, 2), vec![(0.0, 1.0, 0.0), (4.0, 1.0, 0.0)]);
    h.undo();
    assert_eq!(h.live_param(0, 2, StepParam::Velocity), 1.0);
    h.sync();
    assert_eq!(h.eval_7e2("a.velocity"), Value::Number(1.0));
    assert_eq!(h.eval_7e2("b.retrig"), Value::Number(0.0));
}

#[test]
fn a_pinned_sources_steps_are_the_pool_patterns() {
    let mut h = Harness::new();
    h.command("clone-pattern", Value::Nil);
    h.sync();
    let pool = pool_lanes(&h);
    pool.set_note_entries(
        1,
        &[PianoRollNote {
            transpose: 3.0,
            duration: 1.0,
            delay: 0.0,
        }],
    );
    pool.set_step_param(1, StepParam::Pan, -0.5);
    h.eval_7e2("(def a (fs-at 1))");
    h.sync();
    assert_eq!(
        h.eval_7e2("(list a.active a.pan)"),
        h.eval_7e2("(list false 0)")
    );
    let a = h.eval_7e2("a");
    h.pin_pool_clip();
    assert_eq!(h.eval_7e2("piano-roll.focus-kind"), s("pattern"));
    // Positional: the same instance, the pool step's values.
    assert_eq!(h.eval_7e2("(fs-at 1)"), a);
    assert_eq!(
        h.eval_7e2("(list a.active a.pan)"),
        h.eval_7e2("(list true -0.5)")
    );
    let base = h.app.history.undo_len();
    h.run_7e2("(set! a.pan 0.75)");
    assert_eq!(pool_lanes(&h).step_param(1, StepParam::Pan), 0.75);
    assert_eq!(
        h.live_param(0, 1, StepParam::Pan),
        0.0,
        "the live pattern is not the pool's"
    );
    assert_eq!(h.eval_7e2("a.pan"), Value::Number(0.75));
    assert_eq!(h.app.history.undo_len(), base + 1);
    h.undo();
    h.sync();
    assert_eq!(pool_lanes(&h).step_param(1, StepParam::Pan), -0.5);
    assert_eq!(h.eval_7e2("a.pan"), Value::Number(-0.5));
}

#[test]
fn a_script_drag_of_focus_steps_is_one_entry() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e2("(def a (fs-at 0)) (def b (fs-at 1))");
    h.sync();
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7e2("(set! a.velocity 0.5)");
    h.run_7e2("(set! a.velocity 0.25) (set! b.pan 0.5) (set! b.delay 0.5)");
    assert_eq!(h.live_param(0, 0, StepParam::Velocity), 0.25);
    assert_eq!(h.live_param(0, 1, StepParam::Pan), 0.5);
    assert_eq!(
        h.eval_7e2("(list a.velocity b.pan)"),
        h.eval_7e2("(list 0.25 0.5)")
    );
    // A rejected set! changes nothing and keeps the drag open.
    h.rejects_7e2("(set! a.velocity 3)", "velocity takes a number from 0 to 1");
    assert!(h.app.history.active_gesture().is_some());
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(
        h.app.history.undo_len(),
        undo + 1,
        "the whole drag is one entry"
    );
    h.undo();
    assert_eq!(h.live_param(0, 0, StepParam::Velocity), 1.0);
    assert_eq!(h.live_param(0, 1, StepParam::Pan), 0.0);
    assert_eq!(h.live_param(0, 1, StepParam::Delay), 0.0);
    // Esc rolls a drag back, recording nothing.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7e2("(set! a.velocity 0.5) (set! b.pan -1)");
    assert_eq!(app::edit::cancel_active_gesture(&mut h.app), Ok(true));
    h.gesture.pointer_down = false;
    h.gesture.script_param_gesture = None;
    assert_eq!(h.app.history.undo_len(), undo);
    assert_eq!(h.live_param(0, 0, StepParam::Velocity), 1.0);
    assert_eq!(h.live_param(0, 1, StepParam::Pan), 0.0);
    h.sync();
    assert_eq!(
        h.eval_7e2("(list a.velocity b.pan)"),
        h.eval_7e2("(list 1 0)")
    );
}

#[test]
fn focus_steps_follow_the_piano_rolls_track_and_cost_nothing_while_idle() {
    let mut h = Harness::new();
    h.write_notes(0, 0, &[(0.0, 1.0, 0.0)]);
    h.sync();
    assert_eq!(h.focus_step_syncs(), 0, "steps register on the first read");
    h.eval_7e2("(def a (fs-at 0))");
    h.sync();
    let syncs = h.focus_step_syncs();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.focus_step_syncs(), syncs, "idle ticks re-read nothing");
    // A mixer edit moves no content counter.
    h.shared.state.pattern.track_params[0].set_volume(0.3);
    h.sync();
    assert_eq!(h.focus_step_syncs(), syncs);
    // A step edit re-reads them once.
    h.write_param(0, 0, StepParam::Pan, 0.25);
    h.sync();
    h.sync();
    assert_eq!(h.focus_step_syncs(), syncs + 1);
    assert_eq!(h.eval_7e2("a.pan"), Value::Number(0.25));
    // A shorter source drops the steps past it.
    h.shared.state.pattern.track_params[0].set_num_steps(8);
    h.sync();
    assert_eq!(h.eval_7e2("(len piano-roll.steps)"), Value::Number(8.0));
    assert_eq!(h.focus_steps_of(0), 8);
    // Another track's piano roll: the first track's steps go.
    let Value::Instance(a) = h.eval_7e2("a") else {
        panic!("a")
    };
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    assert!(!h.rt().instance_is_live(a));
    assert_eq!(h.focus_steps_of(0), 0);
    assert_eq!(h.focus_steps_of(1), 16);
    assert_eq!(
        h.eval_7e2("(let ((fs (fs-at 0))) fs.track)"),
        h.eval_7e2("(track 1)")
    );
}

#[test]
fn a_focus_step_edit_moves_its_notes_with_their_handles() {
    let mut h = Harness::new();
    h.write_notes(0, 2, &[(0.0, 1.0, 0.0), (4.0, 1.0, 0.0)]);
    h.write_notes(0, 5, &[(1.0, 1.0, 0.0)]);
    h.sync();
    h.eval_7e2("(def a (fs-at 2)) (def b (fs-at 5)) (def x (note-at 0)) (def y (note-at 1))");
    h.eval_7e2("(def z (note-at 2))");
    h.sync();
    let (x, y, z) = (h.eval_7e2("x"), h.eval_7e2("y"), h.eval_7e2("z"));
    // A transpose of 4 puts x where y was: both keep their handles.
    h.run_7e2("(set! a.transpose (+ a.transpose 4))");
    assert_eq!(h.live_notes(0, 2), vec![(4.0, 1.0, 0.0), (8.0, 1.0, 0.0)]);
    assert_eq!(
        (h.eval_7e2("(note-at 0)"), h.eval_7e2("(note-at 1)")),
        (x, y)
    );
    assert_eq!(
        h.eval_7e2("(list x.pitch y.pitch)"),
        h.eval_7e2("(list 4 8)")
    );
    // A step's own transpose moves a note it holds alone too.
    h.run_7e2("(set! b.transpose (- b.transpose 3))");
    assert_eq!(h.live_notes(0, 5), vec![(-2.0, 1.0, 0.0)]);
    assert_eq!(h.eval_7e2("(note-at 2)"), z);
    assert_eq!(h.eval_7e2("z.pitch"), Value::Number(-2.0));
}

#[test]
fn a_pinned_sources_notes_and_steps_follow_each_others_edits() {
    let mut h = Harness::new();
    h.command("clone-pattern", Value::Nil);
    h.sync();
    let note = PianoRollNote {
        transpose: 3.0,
        duration: 1.0,
        delay: 0.0,
    };
    pool_lanes(&h).set_note_entries(1, &[note]);
    h.pin_pool_clip();
    h.eval_7e2("(def a (fs-at 1)) (def c (fs-at 6)) (def n (note-at 0))");
    h.sync();
    let n = h.eval_7e2("n");
    // A focus step's transpose shows in its note, whose handle follows it.
    h.run_7e2("(set! a.transpose (+ a.transpose 2))");
    assert_eq!(h.eval_7e2("(note-at 0)"), n);
    assert_eq!(h.eval_7e2("n.pitch"), Value::Number(5.0));
    // A note added shows in the steps.
    assert_eq!(h.eval_7e2("c.active"), Value::Bool(false));
    h.run_7e2("(add-note! 6 0 1)");
    assert_eq!(h.eval_7e2("c.active"), Value::Bool(true));
    // Esc on a drag of the pinned steps: the pool and the fields go back.
    let undo = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.run_7e2("(set! a.pan 0.5) (set! c.velocity 0.25)");
    assert_eq!(
        h.eval_7e2("(list a.pan c.velocity)"),
        h.eval_7e2("(list 0.5 0.25)")
    );
    assert_eq!(app::edit::cancel_active_gesture(&mut h.app), Ok(true));
    h.gesture.pointer_down = false;
    h.gesture.script_param_gesture = None;
    h.sync();
    assert_eq!(h.app.history.undo_len(), undo);
    assert_eq!(pool_lanes(&h).step_param(1, StepParam::Pan), 0.0);
    assert_eq!(
        h.eval_7e2("(list a.pan c.velocity)"),
        h.eval_7e2("(list 0 1)")
    );
}
