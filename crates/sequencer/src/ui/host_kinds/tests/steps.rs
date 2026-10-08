//! Steps: lazy registration, selection and gestures.

use super::*;

#[test]
fn steps_register_only_when_read_and_drop_when_the_track_shrinks() {
    let mut h = Harness::new();
    h.sync();
    let t0 = h.track_id(0);
    assert!(
        h.steps_of(t0).is_empty(),
        "no step instances until t.steps is read"
    );
    h.sync();
    assert!(h.steps_of(t0).is_empty());
    h.shared.state.pattern.track_params[0].set_num_steps(16);
    h.eval("(def t0 (track 0)) (len t0.steps)");
    assert_eq!(h.steps_of(t0).len(), 16);
    assert!(
        h.steps_of(h.track_id(1)).is_empty(),
        "only the track that was read"
    );
    let step12 = h.rt().keyed_instance(STEP, &[t0, 12]).expect("step 12");
    // Shrinking drops the steps past the end, even with no reader.
    h.shared.state.pattern.track_params[0].set_num_steps(8);
    h.sync();
    assert_eq!(h.steps_of(t0).len(), 8);
    assert!(!h.rt().instance_is_live(step12));
    // A reader of t.steps sees the list follow the length.
    h.eval(r#"(effect-buffer "*steps*" (label (str (len t0.steps))))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    assert!(h.rt().host_field_observed(t0, "steps"));
    h.shared.state.pattern.track_params[0].set_num_steps(12);
    assert!(h.sync());
    assert_eq!(h.steps_of(t0).len(), 12);
    assert_eq!(h.eval("(len t0.steps)"), Value::Number(12.0));
}

#[test]
fn step_selected_follows_a_rack_wide_selection() {
    let mut h = Harness::new();
    h.sync();
    h.shared.state.pattern.track_params[1].set_num_steps(4);
    h.eval("(def t1 (track 1)) (def s2 (nth t1.steps 2)) (def sel2 #'s2.selected)");
    h.shared.selected_steps.lock().unwrap().extend([2, 6]);
    h.sync();
    assert_eq!(h.slot("sel2"), 0.0, "track 1 is not the current track");
    // Rack-wide Cmd+A: the selection covers every listed track.
    *h.shared.active_delete_target.lock().unwrap() =
        Some(ActiveDeleteTarget::TrackSteps { tracks: vec![0, 1] });
    h.shared
        .active_delete_target_version
        .fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("sel2"), 1.0);
    assert_eq!(
        h.eval("(let ((t (track 0)) (s (nth t.steps 6))) s.selected)"),
        Value::Bool(true)
    );
    // Clipped to the track's length: track 1 has no step 6.
    assert_eq!(h.eval("(len t1.steps)"), Value::Number(4.0));
    // Disarming the target drops the other tracks again.
    *h.shared.active_delete_target.lock().unwrap() = None;
    h.shared
        .active_delete_target_version
        .fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("sel2"), 0.0);
}

#[test]
fn step_gestures_on_a_step_instance_select_its_track_and_toggle_the_step() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(import eseq.step-grid-interactions)");
    // Press and release an empty step of track 1 while track 0 is selected.
    h.eval(
        "(let ((t1 (nth (tracks) 1)))
           (let ((s3 (nth t1.steps 3)))
             (do (eseq.step-grid-interactions/down s3 (dict))
                 (eseq.step-grid-interactions/up s3 (dict))
                 nil)))",
    );
    h.drain();
    h.sync();
    assert_eq!(
        h.shared.current_track.load(Ordering::Relaxed),
        1,
        "track 1 selected"
    );
    assert!(
        h.shared.state.pattern.patterns[1].is_active(3),
        "the step turned on"
    );
    assert!(!h.shared.state.pattern.patterns[0].is_active(3));
    // A drag reaching another track's step does nothing there.
    h.eval(
        "(let ((t0 (first (tracks))) (t1 (nth (tracks) 1)))
           (do (eseq.step-grid-interactions/down (nth t1.steps 5) (dict))
               (eseq.step-grid-interactions/drag (nth t0.steps 6) (dict))
               (eseq.step-grid-interactions/up (nth t0.steps 6) (dict))
               nil))",
    );
    h.drain();
    assert!(!h.shared.state.pattern.patterns[0].is_active(6));
}

#[test]
fn step_held_matches_the_duration_spans() {
    let mut h = Harness::new();
    h.sync();
    // `s3.held` bound (the tick's diff), every step read by value (the
    // reader hook).
    h.eval_all("(def t0 (track 0)) (def s3 (nth t0.steps 3)) (def h3 #'s3.held)");
    let num_steps = h.shared.state.pattern.track_params[0]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        seed >> 16
    };
    for _round in 0..8 {
        for step in 0..num_steps {
            let active = next() % 3 == 0;
            h.shared.state.pattern.patterns[0].set_step_active(step, active);
            let duration = (next() % 40) as f32 / 8.0;
            h.shared.state.pattern.step_data[0].set(step, StepParam::Duration, duration);
        }
        h.sync();
        let state = h.shared.state.clone();
        let covered = |step| Value::Bool(track_step_duration_covered(&state, 0, step));
        let kinds = h.eval_all("(map (lambda (s) s.held) t0.steps)");
        let Value::List(kinds) = kinds else {
            panic!("held is a list: {kinds:?}");
        };
        assert_eq!(kinds.len(), num_steps);
        let bound = h.slot("h3") != 0.0;
        assert_eq!(covered(3), Value::Bool(bound), "the bound step");
        for step in 0..num_steps {
            assert_eq!(*kinds[step].borrow(), covered(step), "step {step}");
        }
    }
}

/// A *step* panel param edit (`set-step-param-history`) moves the step's
/// fields at the next sync, with no `ui_epoch` bump (the targeted path;
/// ported from the legacy `SEQ.{transposes,velocities,durations}` and
/// `track-duration-spans` checks, eseq-0l17.78).
#[test]
fn a_step_panel_param_edit_moves_the_step_fields_without_a_ui_epoch_bump() {
    let mut h = Harness::new();
    h.shared.state.pattern.track_params[0].set_num_steps(16);
    h.shared.state.pattern.patterns[0].set_step_active(5, true);
    h.sync();
    h.eval_all(
        "(def t0 (track 0)) (def s5 (nth t0.steps 5)) (def s8 (nth t0.steps 8)) \
         (def held8 #'s8.held)",
    );
    h.sync();
    let epoch = h.shared.ui_epoch.load(Ordering::Relaxed);
    let edit = |h: &mut Harness, param: &str, value: f64| {
        let payload = crate::values::map_value(vec![
            ("track", Value::Number(0.0)),
            ("param", Value::Keyword(param.to_string())),
            ("value", Value::Number(value)),
            (
                "steps",
                Value::List(vec![Rc::new(RefCell::new(Value::Number(5.0)))]),
            ),
        ]);
        h.command("set-step-param-history", payload);
        h.sync();
    };
    edit(&mut h, "transpose", 7.0);
    assert_eq!(h.eval_all("s5.transpose"), Value::Number(7.0));
    edit(&mut h, "velocity", 0.25);
    assert_eq!(h.eval_all("s5.velocity"), Value::Number(0.25));
    edit(&mut h, "duration", 4.0);
    assert_eq!(h.eval_all("s5.duration"), Value::Number(4.0));
    assert_eq!(h.slot("held8"), 1.0, "a 4-step duration holds step 8");
    edit(&mut h, "duration", 1.0);
    assert_eq!(h.slot("held8"), 0.0, "shortened, the span is released");
    assert_eq!(
        h.shared.ui_epoch.load(Ordering::Relaxed),
        epoch,
        "step-param edits stay on the targeted path"
    );
}
