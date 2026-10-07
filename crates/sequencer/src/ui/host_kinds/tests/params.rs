//! Stage 7b: device params, their p-lock display and setters, and the step
//! p-lock render (`plocked`, `lock-kind`, `variant-color`).

use super::*;

/// The Filter's `cutoff` (Hz) and the sampler's `start` (a percent param:
/// stored 0–1, shown 0–100).
const CUTOFF: usize = 2;
const START: usize = 2;
/// The sampler's `loop` (an enum).
const LOOP: usize = 6;

impl Harness {
    /// Track 0 gets a Filter in effect slot 0 (through a recorded chain
    /// edit, which binds its instance id); a third track (2) gets a sampler
    /// instrument. Synced, nothing read yet.
    pub(super) fn with_devices() -> (Self, usize) {
        let mut h = Harness::new();
        let slot = h.add_effect(0, "Filter");
        h.app
            .graph_controller()
            .add_blank_sampler_track()
            .expect("sampler track");
        h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        h.sync();
        (h, slot)
    }

    pub(super) fn add_effect(&mut self, track: usize, name: &str) -> usize {
        let slot = self
            .app
            .apply_recorded_track_effect_chain_mutation(track, "Add effect", |app| {
                app.add_builtin_effect_sync(track, name)
            })
            .expect("add effect");
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        slot
    }

    fn params_of(&self, device: InstanceId) -> Vec<InstanceId> {
        self.rt()
            .keyed_children_of_kind(device, PARAM)
            .map(|(id, _)| id)
            .collect()
    }

    fn instance(&mut self, code: &str) -> InstanceId {
        match self.eval_all(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    pub(super) fn filter_slot(&self, slot: usize) -> &sequencer::effects::EffectSlotState {
        &self.shared.state.pattern.effect_chains[0][slot]
    }

    pub(super) fn drain_and_sync(&mut self) {
        self.drain();
        self.sync();
    }

    /// Apply a p-lock edit as a knob does: through history, with the
    /// invalidation the knob queues.
    pub(super) fn lock_effect(&mut self, slot: usize, step: usize, param: usize, value: f32) {
        let command = app::AppCommand::SetEffectPlock {
            track: 0,
            step,
            slot_idx: slot,
            param_idx: param,
            value,
        };
        app::apply_command(&mut self.app, command);
        self.shared.ui_invalidations.push(UiInvalidation::TrackFx {
            track: 0,
            change: TrackFxInvalidation::Plock { slot, param },
        });
    }

    pub(super) fn error(&self) -> String {
        self.editor.minibuffer.clone().unwrap_or_default()
    }

    fn plock_scans(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().plock_scans
    }
}

fn close(value: Value, expected: f64) -> bool {
    (num(value) - expected).abs() < 1e-3
}

const DEVICES: &str = r#"(def t0 (track 0)) (def flt (first t0.devices))
    (def cutoff (device-param flt "cutoff"))
    (def t2 (track 2)) (def inst (first t2.devices))
    (def start (device-param inst "start")) (def loop-mode (device-param inst "loop"))
    (def enabled (device-param inst "enabled"))"#;

#[test]
fn param_fields_read_after_sync_for_instrument_and_effect_params() {
    let (mut h, slot) = Harness::with_devices();
    assert_eq!(slot, 0);
    h.eval_all(DEVICES);
    // The sampler instrument: every descriptor param, in order.
    let inst_desc = h.app.graph.instrument_descriptors[2].params.clone();
    assert_eq!(
        h.eval_all("(len inst.params)"),
        Value::Number(inst_desc.len() as f64)
    );
    assert_eq!(
        h.eval_all("start"),
        h.eval_all(&format!("(nth inst.params {START})"))
    );
    assert_eq!(h.eval_all("start.index"), Value::Number(START as f64));
    assert_eq!(h.eval_all("start.device"), h.eval_all("inst"));
    assert_eq!(h.eval_all("start.name"), s("start"));
    // Display units: percent params 0–100.
    assert_eq!(h.eval_all("start.min"), Value::Number(0.0));
    assert_eq!(h.eval_all("start.max"), Value::Number(100.0));
    assert_eq!(h.eval_all("start.unit"), s("%"));
    assert_eq!(h.eval_all("start.type"), s("continuous"));
    assert_eq!(h.eval_all("start.options"), h.eval_all("(list)"));
    h.shared.state.pattern.instrument_slots[2]
        .defaults
        .set(START, 0.25);
    assert!(close(h.eval_all("start.base"), 25.0));
    assert!(close(h.eval_all("start.value"), 25.0));
    assert_eq!(h.eval_all("start.locked"), Value::Bool(false));
    assert_eq!(h.eval_all("start.has-locks"), Value::Bool(false));
    assert_eq!(h.eval_all("start.text"), s(""));
    assert_eq!(h.eval_all("start.printing"), Value::Bool(false));
    // An enum: its labels, and the label its value selects.
    assert_eq!(h.eval_all("loop-mode.type"), s("enum"));
    assert_eq!(
        h.eval_all("loop-mode.options"),
        h.eval_all(r#"(list "one-shot" "gate" "loop" "ping-pong")"#)
    );
    let loop_value = h.shared.state.pattern.instrument_slots[2]
        .defaults
        .get(LOOP);
    let labels = ["one-shot", "gate", "loop", "ping-pong"];
    assert_eq!(
        h.eval_all("loop-mode.text"),
        s(labels[loop_value.round() as usize])
    );
    assert_eq!(
        h.eval_all("(list enabled.type enabled.text)"),
        h.eval_all(r#"(list "boolean" "on")"#)
    );
    assert_eq!(h.eval_all(r#"(device-param inst "no such")"#), Value::Nil);
    // The Filter effect.
    assert_eq!(h.eval_all("cutoff.index"), Value::Number(CUTOFF as f64));
    assert_eq!(h.eval_all("cutoff.device"), h.eval_all("flt"));
    assert_eq!(h.eval_all("cutoff.min"), Value::Number(20.0));
    assert_eq!(h.eval_all("cutoff.max"), Value::Number(20000.0));
    assert_eq!(h.eval_all("cutoff.default"), Value::Number(1000.0));
    assert_eq!(h.eval_all("cutoff.unit"), s("Hz"));
    let base = h.filter_slot(0).defaults.get(CUTOFF) as f64;
    assert!(close(h.eval_all("cutoff.base"), base));
    assert!(close(h.eval_all("cutoff.value"), base));
    assert_eq!(h.eval_all("inst.playhead"), Value::Number(0.0));
    assert_eq!(h.eval_all("flt.playhead"), Value::Number(0.0));
    // What each device is, and the stable ids the setters address.
    assert_eq!(h.eval_all("inst.type"), s("sampler"));
    assert_eq!(h.eval_all("flt.type"), s("Filter"));
    assert_eq!(h.eval_all("inst.did"), Value::Number(0.0));
    let track_id = h.app.track_registry.id_at(0).unwrap();
    let effect_id = h.app.device_registry.audio_effect_id(track_id, slot);
    let effect_id = effect_id.expect("a recorded add binds the instance id");
    assert_eq!(h.eval_all("flt.did"), Value::Number(effect_id.0 as f64));
    assert_eq!(h.eval_all("t0.tid"), Value::Number(track_id.0 as f64));
    // A value read matches the legacy panel field (Hz: no unit change).
    let step = displayed_plock_step(
        &h.shared.state,
        0,
        selected_plock_step(&h.shared.selected_steps),
    );
    let rt = h.editor.runtime_mut();
    sync_track_effect_param_value_field(rt, &h.app, 0, 0, CUTOFF, step);
    let legacy = rt
        .reactive_field_value(
            "SEQ",
            &track_effect_param_value_field(0, 0, CUTOFF, "cutoff"),
        )
        .cloned()
        .expect("legacy cutoff field");
    assert_eq!(legacy, h.eval_all("cutoff.value"));
}

#[test]
fn effect_and_instrument_params_speak_the_same_display_units() {
    let mut h = Harness::new();
    let slot = h.add_effect(0, "Chorus");
    h.sync();
    let desc = h.app.graph.effect_descriptors[0][slot].clone();
    let index = desc
        .params
        .iter()
        .position(|param| param.is_percent())
        .expect("the Chorus has a percent param");
    let pdesc = desc.params[index].clone();
    h.eval_all(&format!(
        "(def fx (first (let ((t (track 0))) t.devices))) (def p (nth fx.params {index}))"
    ));
    assert_eq!(h.eval_all("p.unit"), s("%"));
    // `percent` says the device stores it as a fraction (the legacy effect
    // commands' units); a param of another unit is not one.
    assert_eq!(h.eval_all("p.percent"), Value::Bool(true));
    let other = desc
        .params
        .iter()
        .position(|param| !param.is_percent())
        .expect("the Chorus has a non-percent param");
    assert_eq!(
        h.eval_all(&format!("(let ((q (nth fx.params {other}))) q.percent)")),
        Value::Bool(false)
    );
    assert!(close(h.eval_all("p.min"), f64::from(pdesc.min) * 100.0));
    assert!(close(h.eval_all("p.max"), f64::from(pdesc.max) * 100.0));
    assert!(close(
        h.eval_all("p.default"),
        f64::from(pdesc.default) * 100.0
    ));
    let chain = &h.shared.state.pattern.effect_chains[0][slot];
    let stored = f64::from(chain.defaults.get(index));
    assert!(close(h.eval_all("p.base"), stored * 100.0));
    assert!(close(h.eval_all("p.value"), stored * 100.0));
    // Set in display units too.
    let target = (pdesc.min + pdesc.max) / 2.0;
    h.eval_all(&format!("(set! p.base {})", target * 100.0));
    h.drain_and_sync();
    let chain = &h.shared.state.pattern.effect_chains[0][slot];
    assert!((chain.defaults.get(index) - target).abs() < 1e-5);
    assert!(close(h.eval_all("p.base"), f64::from(target) * 100.0));
}

#[test]
fn param_value_follows_the_selected_or_playing_steps_lock() {
    let (mut h, slot) = Harness::with_devices();
    let base = h.filter_slot(slot).defaults.get(CUTOFF) as f64;
    h.lock_effect(slot, 4, CUTOFF, 500.0);
    let pattern = &h.shared.state.pattern.patterns[0];
    pattern.set_step_active(4, true);
    pattern.set_step_active(6, true);
    h.eval_all(DEVICES);
    h.eval_all(
        "(def value #'cutoff.value) (def locked #'cutoff.locked) (def base #'cutoff.base)
         (def any #'cutoff.has-locks)",
    );
    h.sync();
    let check = |h: &mut Harness, value: f64, locked: bool, what: &str| {
        assert!((h.slot("value") - value).abs() < 1e-3, "{what}: value");
        assert_eq!(
            h.slot("locked"),
            f64::from(u8::from(locked)),
            "{what}: locked"
        );
        assert!((h.slot("base") - base).abs() < 1e-3, "{what}: base");
    };
    check(&mut h, base, false, "stopped, nothing selected");
    assert_eq!(h.slot("any"), 1.0, "some step locks it");
    // The selected step's lock; an off step holds the lock before it until
    // the next trigger.
    h.shared.selected_steps.lock().unwrap().insert(4);
    h.sync();
    check(&mut h, 500.0, true, "step 4 selected");
    *h.shared.selected_steps.lock().unwrap() = HashSet::from([5]);
    h.sync();
    check(&mut h, 500.0, true, "off step 5 holds step 4's lock");
    *h.shared.selected_steps.lock().unwrap() = HashSet::from([6]);
    h.sync();
    check(&mut h, base, false, "step 6 triggers without a lock");
    // Playing with no selection: the playing step's.
    h.shared.selected_steps.lock().unwrap().clear();
    let transport = &h.shared.state.transport;
    transport.playing.store(true, Ordering::Relaxed);
    transport.track_playheads[0].store(4, Ordering::Relaxed);
    h.sync();
    check(&mut h, 500.0, true, "playing step 4");
    h.shared.state.transport.track_playheads[0].store(7, Ordering::Relaxed);
    h.sync();
    check(&mut h, base, false, "playing step 7");
    h.shared.state.transport.track_playheads[0].store(4, Ordering::Relaxed);
    // Another track current: this one shows its base (like send.display).
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.sync();
    check(&mut h, base, false, "track 0 not current");
    // On the current track the legacy field shows the same.
    h.shared.current_track.store(0, Ordering::Relaxed);
    h.sync();
    check(&mut h, 500.0, true, "track 0 current again");
    let step = displayed_plock_step(
        &h.shared.state,
        0,
        selected_plock_step(&h.shared.selected_steps),
    );
    let rt = h.editor.runtime_mut();
    sync_track_effect_param_value_field(rt, &h.app, 0, 0, CUTOFF, step);
    let legacy = rt
        .reactive_field_value(
            "SEQ",
            &track_effect_param_value_field(0, 0, CUTOFF, "cutoff"),
        )
        .cloned();
    assert_eq!(legacy, Some(Value::Number(500.0)));
    // A clear (through the setter) empties has-locks.
    h.eval_all("(unlock-param! cutoff (list (nth t0.steps 4)))");
    h.drain_and_sync();
    assert_eq!(h.slot("any"), 0.0);
    check(&mut h, base, false, "lock cleared");
}

#[test]
fn param_setters_change_the_model_through_history() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    h.eval_all("(def s3 (nth t0.steps 3)) (def s7 (nth t0.steps 7))");
    let base = h.filter_slot(slot).defaults.get(CUTOFF);
    // The base, on a track that is not the current one too; each script
    // set! is an undo entry of its own (no pointer held).
    h.shared.current_track.store(1, Ordering::Relaxed);
    let before = h.app.history.undo_len();
    h.eval_all("(set! cutoff.base 1500) (set! cutoff.base 2000)");
    h.drain_and_sync();
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), 2000.0);
    assert!(close(h.eval_all("cutoff.base"), 2000.0));
    assert!(h.app.history.active_gesture().is_none(), "entries ended");
    assert_eq!(h.app.history.undo_len(), before + 2);
    // Setting the value it has changes nothing.
    h.eval_all("(set! cutoff.base 2000)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 2);
    // Clamped to the range; undo restores the base.
    h.eval_all("(set! cutoff.base 99999)");
    h.drain();
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), 20000.0);
    for expected in [2000.0, 1500.0, base] {
        app::edit::undo(&mut h.app);
        assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), expected);
    }
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert!(close(h.eval_all("cutoff.base"), base as f64));
    // Instrument params take display units.
    h.eval_all("(set! start.base 50)");
    h.drain_and_sync();
    let inst = &h.shared.state.pattern.instrument_slots[2];
    assert_eq!(inst.defaults.get(START), 0.5);
    assert!(close(h.eval_all("start.base"), 50.0));
    // P-locks: several steps, one undo entry; never the base.
    let before = h.app.history.undo_len();
    h.eval_all("(lock-param! cutoff (list s3 s7) 800)");
    h.drain_and_sync();
    let chain = h.filter_slot(slot);
    assert_eq!(chain.plocks.get(3, CUTOFF), Some(800.0));
    assert_eq!(chain.plocks.get(7, CUTOFF), Some(800.0));
    assert_eq!(chain.defaults.get(CUTOFF), base);
    assert_eq!(h.eval_all("cutoff.has-locks"), Value::Bool(true));
    let locked = h.app.history.undo_len();
    assert_eq!(locked, before + 1);
    h.eval_all("(lock-param! cutoff (list s3 s7) 800)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), locked, "already locked");
    h.eval_all("(unlock-param! cutoff (list s3))");
    h.drain_and_sync();
    assert_eq!(h.filter_slot(slot).plocks.get(3, CUTOFF), None);
    assert_eq!(h.filter_slot(slot).plocks.get(7, CUTOFF), Some(800.0));
    app::edit::undo(&mut h.app);
    assert_eq!(h.filter_slot(slot).plocks.get(3, CUTOFF), Some(800.0));
    app::edit::undo(&mut h.app);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.filter_slot(slot).plocks.get(7, CUTOFF), None);
    assert_eq!(h.eval_all("cutoff.has-locks"), Value::Bool(false));
    // Instrument locks too, in display units.
    h.eval_all("(lock-param! start (list (nth t2.steps 1)) 75)");
    h.drain();
    let inst = &h.shared.state.pattern.instrument_slots[2];
    assert_eq!(inst.plocks.get(1, START), Some(0.75));
    // value and locked are read-only.
    for (field, value) in [("value", "1"), ("locked", "true")] {
        let error = h
            .editor
            .runtime_mut()
            .eval_str(&format!("{REFER_ALL} (set! cutoff.{field} {value})"))
            .expect_err("read-only");
        assert!(
            format!("{error:?}").contains(&format!("param.{field} is read-only")),
            "{error:?}"
        );
    }
}

#[test]
fn param_setters_check_their_inputs() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    let base = h.filter_slot(slot).defaults.get(CUTOFF);
    // A non-finite value is an error and changes nothing.
    let track_id = h.app.track_registry.id_at(0).unwrap().0 as f64;
    let did = num(h.eval_all("flt.did"));
    let payload = |value: f64| {
        let entries = [
            ("track-id", Value::Number(track_id)),
            ("device", Value::Number(did)),
            ("param-idx", Value::Number(CUTOFF as f64)),
            ("value", Value::Number(value)),
        ];
        let map = entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect();
        Value::Map(map)
    };
    h.command("set-device-param", payload(f64::NAN));
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), base);
    assert!(h.error().contains("not finite"), "{}", h.error());
    h.editor.minibuffer = None;
    h.command("set-device-param", payload(f64::INFINITY));
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), base);
    assert!(h.error().contains("not finite"), "{}", h.error());
    // Boolean params take true/false.
    let enabled = h.app.graph.instrument_descriptors[2]
        .params
        .iter()
        .position(|param| param.name == "enabled")
        .unwrap();
    let inst_value = |h: &Harness, index: usize| {
        h.shared.state.pattern.instrument_slots[2]
            .defaults
            .get(index)
    };
    h.eval_all("(set! enabled.base false)");
    h.drain_and_sync();
    assert_eq!(inst_value(&h, enabled), 0.0);
    assert_eq!(h.eval_all("enabled.text"), s("off"));
    h.eval_all("(set! enabled.base true)");
    h.drain_and_sync();
    assert_eq!(inst_value(&h, enabled), 1.0);
    assert_eq!(h.eval_all("enabled.base"), Value::Number(1.0));
    // Enum values round to an option.
    h.eval_all("(set! loop-mode.base 1.6)");
    h.drain_and_sync();
    assert_eq!(inst_value(&h, LOOP), 2.0);
    assert_eq!(h.eval_all("loop-mode.text"), s("loop"));
    // Steps of another track are an error; nothing is locked.
    let before = h.app.history.undo_len();
    h.eval_all("(lock-param! cutoff (list (nth t0.steps 1) (nth t2.steps 2)) 900)");
    h.drain();
    let error = h.error();
    assert!(error.contains("steps of the param's track"), "{error}");
    assert_eq!(h.filter_slot(slot).plocks.get(1, CUTOFF), None);
    assert_eq!(h.app.history.undo_len(), before);
    h.editor.minibuffer = None;
    h.eval_all("(unlock-param! cutoff (list (nth t2.steps 2)))");
    h.drain();
    let error = h.error();
    assert!(error.contains("steps of the param's track"), "{error}");
}

#[test]
fn script_param_edits_keep_their_own_undo_entries_beside_a_user_drag() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    let params = h.app.graph.effect_descriptors[0][slot].params.clone();
    let other = (0..params.len())
        .find(|index| *index != CUTOFF && !params[*index].is_boolean() && !params[*index].is_enum())
        .expect("another continuous param");
    let desc = params[other].clone();
    let base = h.filter_slot(slot).defaults.get(CUTOFF);
    let other_base = h.filter_slot(slot).defaults.get(other);
    let span = desc.max - desc.min;
    let dragged = desc.clamp(other_base + span * 0.25);
    let dragged = if dragged == other_base {
        desc.clamp(other_base - span * 0.25)
    } else {
        dragged
    };
    // The user drags another knob: a coalescing gesture stays open.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    let drag = |value| app::AppCommand::SetEffectParam {
        track: 0,
        slot_idx: slot,
        param_idx: other,
        value,
    };
    app::try_apply_command(&mut h.app, drag(dragged)).expect("drag");
    let gesture = h.app.history.active_gesture().map(|g| g.id);
    assert!(gesture.is_some(), "the drag's gesture");
    // A script set! lands mid-drag: an entry of its own, the drag untouched.
    h.eval_all("(set! cutoff.base 3000)");
    h.drain();
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), 3000.0);
    assert_eq!(h.app.history.active_gesture().map(|g| g.id), gesture);
    assert_eq!(h.app.history.undo_len(), before + 1, "the script's entry");
    app::try_apply_command(&mut h.app, drag((dragged + other_base) / 2.0)).expect("drag");
    assert_eq!(h.app.history.active_gesture().map(|g| g.id), gesture);
    // Release: the drag is one entry.
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 2);
    app::edit::undo(&mut h.app);
    assert_eq!(h.filter_slot(slot).defaults.get(other), other_base);
    app::edit::undo(&mut h.app);
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), base);
    // A drag view: set!s while the pointer is down join one entry, which
    // the release ends.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    h.eval_all("(set! cutoff.base 1200)");
    h.drain();
    h.eval_all("(set! cutoff.base 1300)");
    h.drain();
    assert!(h.app.history.active_gesture().is_some());
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 1);
    app::edit::undo(&mut h.app);
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), base);
}

#[test]
fn an_enum_or_boolean_base_set_rebuilds_the_legacy_panel() {
    let (mut h, _) = Harness::with_devices();
    h.eval_all(DEVICES);
    let epochs = |h: &Harness| {
        (
            h.shared.fx_epoch.load(Ordering::Relaxed),
            h.shared.ui_epoch.load(Ordering::Relaxed),
        )
    };
    let before = epochs(&h);
    h.eval_all("(set! start.base 40)");
    h.drain();
    assert_eq!(epochs(&h), before, "a continuous param rebinds its readout");
    h.eval_all("(set! loop-mode.base 3)");
    h.drain();
    let after = epochs(&h);
    assert!(after.0 > before.0 && after.1 > before.1, "an enum rebuilds");
    h.eval_all("(set! enabled.base false)");
    h.drain();
    let last = epochs(&h);
    assert!(last.0 > after.0 && last.1 > after.1, "a boolean rebuilds");
}

#[test]
fn params_register_lazily_and_drop_with_their_device() {
    let (mut h, slot) = Harness::with_devices();
    let filter = h.instance("(first (let ((t (track 0))) t.devices))");
    let inst = h.instance("(first (let ((t (track 2))) t.devices))");
    assert!(h.params_of(filter).is_empty(), "nothing read d.params yet");
    h.sync();
    assert!(h.params_of(filter).is_empty());
    // A cold read registers them all, with their descriptor fields.
    h.eval_all("(def flt (first (let ((t (track 0))) t.devices)))");
    let count = h.app.graph.effect_descriptors[0][slot].params.len();
    assert_eq!(h.eval_all("(len flt.params)"), Value::Number(count as f64));
    let params = h.params_of(filter);
    assert_eq!(params.len(), count);
    let cutoff = h.instance_of(PARAM, &[filter, CUTOFF as u64]);
    assert_eq!(h.rt().instance_field(cutoff, "name"), Ok(s("cutoff")));
    // Only that device's.
    assert!(h.params_of(inst).is_empty());
    // A view reading t2's instrument params registers them.
    h.eval_all(
        r#"(def inst (first (let ((t (track 2))) t.devices)))
           (effect-buffer "*params*" (label (str (len inst.params))))"#,
    );
    h.show_all();
    assert!(h.rt().host_field_observed(inst, "params"));
    h.sync();
    let inst_params = h.params_of(inst).len();
    assert_eq!(
        inst_params,
        h.app.graph.instrument_descriptors[2].params.len()
    );
    assert_eq!(
        h.eval_all("(len inst.params)"),
        Value::Number(inst_params as f64)
    );
    // Deleting the effect drops the device and its params.
    h.app
        .graph_controller()
        .delete_custom_effect_slot(0, slot)
        .expect("delete");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert!(!h.rt().instance_is_live(filter));
    assert!(params.iter().all(|id| !h.rt().instance_is_live(*id)));
    assert_eq!(h.params_of(inst).len(), inst_params);
}

#[test]
fn a_reorder_keeps_device_and_param_identity_and_a_replacement_renews_them() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all("(def flt (first (let ((t (track 0))) t.devices))) (def cutoff (nth flt.params 2))");
    let filter = h.instance("flt");
    let cutoff = h.instance("cutoff");
    assert_eq!(h.rt().instance_field(cutoff, "name"), Ok(s("cutoff")));
    // Another effect after it: the Filter's device and params stay.
    let second = h.add_effect(0, "Reverb");
    h.sync();
    assert!(h.rt().instance_is_live(cutoff));
    assert_eq!(h.eval_all("flt.slot"), Value::Number(slot as f64));
    // Reorder: the Reverb moves before the Filter. The Filter's device and
    // params keep their instances; only slot moves.
    h.app
        .move_effect_slot_between_tracks_recorded(0, second, 0, Some(slot))
        .expect("move");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.app.graph.effect_descriptors[0][slot].name, "Reverb");
    let filter_slot = h.app.graph.effect_descriptors[0]
        .iter()
        .position(|desc| desc.name == "Filter")
        .unwrap();
    assert_ne!(filter_slot, slot);
    assert!(h.rt().instance_is_live(filter));
    assert!(h.rt().instance_is_live(cutoff));
    assert_eq!(h.eval_all("flt.name"), s("Filter"));
    assert_eq!(h.eval_all("flt.slot"), Value::Number(filter_slot as f64));
    assert_eq!(h.eval_all("cutoff.name"), s("cutoff"));
    // The setter follows the device to its new slot.
    h.eval_all("(set! cutoff.base 4321)");
    h.drain_and_sync();
    let chain = &h.shared.state.pattern.effect_chains[0][filter_slot];
    assert_eq!(chain.defaults.get(CUTOFF), 4321.0);
    // Replacing the effect in its slot (the replacement adopts the instance
    // id): the device stays, its params are new and the old handles stale.
    let old_params = h.params_of(filter);
    let did = h.eval_all("flt.did");
    h.app
        .apply_recorded_track_effect_chain_mutation(0, "Replace effect", |app| {
            app.graph_controller()
                .delete_custom_effect_slot(0, filter_slot)?;
            app.insert_builtin_effect_before_slot_sync(0, filter_slot, "Chorus")
        })
        .expect("replace");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_all("flt.did"), did, "the replacement adopts the id");
    assert!(h.rt().instance_is_live(filter));
    assert_eq!(h.eval_all("flt.name"), s("Chorus"));
    assert!(old_params.iter().all(|id| !h.rt().instance_is_live(*id)));
    let chorus = h.app.graph.effect_descriptors[0][filter_slot]
        .params
        .clone();
    assert_eq!(h.params_of(filter).len(), chorus.len());
    assert_eq!(
        h.eval_all("(let ((p (first flt.params))) p.name)"),
        s(&chorus[0].name)
    );
    // A project load replaces the track, its devices and their params.
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(filter));
}

#[test]
fn param_and_plock_render_fields_cost_nothing_while_unobserved() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    h.eval_all("(def s4 (nth t0.steps 4)) (len inst.params)");
    let keys = [
        f::PARAM_VALUE,
        f::PARAM_BASE,
        f::PARAM_LOCKED,
        f::PARAM_HAS_LOCKS,
        f::PARAM_TEXT,
        f::PARAM_PRINTING,
        f::STEP_PLOCKED,
        f::STEP_LOCK_KIND,
        f::STEP_VARIANT_COLOR,
        f::DEVICE_PLAYHEAD,
        f::DEVICE_PARAMS,
    ];
    let before: Vec<u64> = keys.iter().map(|key| h.computed(*key)).collect();
    let scans = h.plock_scans();
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    for step in 0..8 {
        h.shared.state.transport.track_playheads[0].store(step, Ordering::Relaxed);
        h.filter_slot(slot)
            .defaults
            .set(CUTOFF, 1000.0 + step as f32);
        h.lock_effect(slot, step as usize, CUTOFF, 400.0);
        h.sync();
    }
    let after: Vec<u64> = keys.iter().map(|key| h.computed(*key)).collect();
    assert_eq!(before, after, "nothing observed, nothing computed");
    assert_eq!(h.plock_scans(), scans);
    // Observing one param's value computes that field only, and a tick
    // asks only that param (of the many registered) what it observes.
    let flt = h.instance("flt");
    let inst = h.instance("inst");
    assert!(h.params_of(flt).len() + h.params_of(inst).len() > 20);
    h.eval_all("(def value #'cutoff.value)");
    h.sync();
    let seeded = h.computed(f::PARAM_VALUE);
    let queries = h.frame.host_kinds.shared.borrow().param_queries;
    for _ in 0..5 {
        h.sync();
    }
    assert_eq!(h.computed(f::PARAM_VALUE), seeded + 5);
    let asked = h.frame.host_kinds.shared.borrow().param_queries - queries;
    assert_eq!(asked, 5, "one observed param, one query per tick");
    assert_eq!(h.computed(f::PARAM_BASE), after[1]);
    // The step render is recomputed only when a p-lock edit lands.
    h.eval_all("(def plocked #'s4.plocked)");
    h.sync();
    assert_eq!(h.slot("plocked"), 1.0);
    let render = h.computed(f::STEP_PLOCKED);
    let scans = h.plock_scans();
    h.shared.ui_invalidations.push(UiInvalidation::TrackFx {
        track: 0,
        change: TrackFxInvalidation::Param {
            slot,
            param: CUTOFF,
        },
    });
    h.sync();
    h.sync();
    assert_eq!(
        h.computed(f::STEP_PLOCKED),
        render,
        "a base edit moves no lock"
    );
    assert_eq!(h.plock_scans(), scans);
    h.eval_all("(unlock-param! cutoff (list s4))");
    h.drain_and_sync();
    assert_eq!(h.slot("plocked"), 0.0, "a p-lock edit moves the render");
    assert_eq!(h.plock_scans(), scans + 1);
    // Dropping the bindings stops the work again.
    h.eval_all("(set! value nil) (set! plocked nil)");
    let value = h.computed(f::PARAM_VALUE);
    let render = h.computed(f::STEP_PLOCKED);
    h.shared.ui_invalidations.push(UiInvalidation::ProjectState);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::PARAM_VALUE), value);
    assert_eq!(h.computed(f::STEP_PLOCKED), render);
}

#[test]
fn cold_step_render_reads_scan_once() {
    let (mut h, slot) = Harness::with_devices();
    h.lock_effect(slot, 3, CUTOFF, 700.0);
    h.eval_all("(def t0 (track 0)) (len t0.steps)");
    let scans = h.plock_scans();
    let rows = h.eval_all("(map (lambda (s) (list s.plocked s.lock-kind)) t0.steps)");
    assert_eq!(h.plock_scans(), scans + 1, "one scan for every step");
    h.eval_all("(map (lambda (s) s.variant-color) t0.steps)");
    assert_eq!(h.plock_scans(), scans + 1);
    let Value::List(rows) = rows else {
        panic!("a list: {rows:?}");
    };
    let render = plock_variant_step_render_values(&h.shared.state, 0);
    let kind3 = render[3].kind;
    assert_ne!(kind3, 0);
    assert_eq!(
        *rows[3].borrow(),
        h.eval_all(&format!("(list true {kind3})"))
    );
    assert_eq!(*rows[2].borrow(), h.eval_all("(list false lock-none)"));
    assert_eq!(
        h.eval_all("(list lock-none lock-seq lock-variant)"),
        h.eval_all("(list 0 1 2)")
    );
    // A p-lock edit moves the cache.
    h.lock_effect(slot, 5, CUTOFF, 900.0);
    assert_eq!(
        h.eval_all("(let ((s (nth t0.steps 5))) s.plocked)"),
        Value::Bool(true)
    );
    assert_eq!(h.plock_scans(), scans + 2);
}

#[test]
fn has_locks_counts_only_the_steps_of_the_pattern() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    let num_steps = h.shared.state.pattern.track_params[0].get_num_steps();
    assert!(num_steps < 40);
    h.lock_effect(slot, 40, CUTOFF, 600.0);
    h.eval_all("(def any #'cutoff.has-locks)");
    h.sync();
    assert_eq!(h.slot("any"), 0.0, "past the pattern's end");
    h.shared.state.pattern.track_params[0].set_num_steps(48);
    h.shared.ui_invalidations.push(UiInvalidation::TrackParam {
        track: 0,
        change: TrackParamInvalidation::NumSteps,
    });
    h.sync();
    assert_eq!(h.slot("any"), 1.0);
    h.eval_all("(set! any nil)");
    assert_eq!(h.eval_all("cutoff.has-locks"), Value::Bool(true));
}

#[test]
fn step_plock_render_and_send_lock_flags_match_the_legacy_fields() {
    let (mut h, slot) = Harness::with_devices();
    let fx = h.add_bus("FX");
    h.sync();
    for step in [2, 9] {
        h.lock_effect(slot, step, CUTOFF, 300.0 + step as f32);
    }
    h.shared.state.pattern.track_send_plocks[0].set(5, fx, 0.5);
    h.shared.ui_invalidations.push(UiInvalidation::ProjectState);
    h.eval_all(&format!(
        "(def t0 (track 0))
         (def fx-send (first (filter (lambda (s) (= s.bus.bid {})) t0.sends)))
         (def s9 (nth t0.steps 9)) (def kind9 #'s9.lock-kind)",
        fx.0
    ));
    h.sync();
    let num_steps = h.shared.state.pattern.track_params[0]
        .get_num_steps()
        .min(MAX_STEPS);
    let render = plock_variant_step_render_values(&h.shared.state, 0);
    let mask = track_step_plock_mask(&h.shared.state, 0, &h.app.graph.effect_descriptors);
    let rows =
        h.eval_all("(map (lambda (s) (list s.plocked s.lock-kind s.variant-color)) t0.steps)");
    let Value::List(rows) = rows else {
        panic!("a list: {rows:?}");
    };
    assert_eq!(rows.len(), num_steps);
    for (step, row) in rows.iter().enumerate() {
        let expected = h.eval_all(&format!(
            "(list {} {} (rgb {} {} {}))",
            mask[step / 64] & (1 << (step % 64)) != 0,
            render[step].kind,
            f64::from(render[step].color[0]),
            f64::from(render[step].color[1]),
            f64::from(render[step].color[2]),
        ));
        assert_eq!(*row.borrow(), expected, "step {step}");
    }
    for step in [2, 5, 9] {
        assert!(mask[step / 64] & (1 << (step % 64)) != 0, "step {step}");
    }
    assert_eq!(h.slot("kind9"), f64::from(render[9].kind));
    // Sends: a lock somewhere, and whether the shown level is one.
    assert_eq!(h.eval_all("fx-send.has-locks"), Value::Bool(true));
    assert_eq!(h.eval_all("fx-send.locked"), Value::Bool(false));
    h.shared.selected_steps.lock().unwrap().insert(5);
    assert_eq!(h.eval_all("fx-send.locked"), Value::Bool(true));
    // `has-locks` is cached under the track's p-lock key: an edit moves it,
    // as the send p-lock commands' invalidation does.
    h.shared.state.pattern.track_send_plocks[0].clear(5, fx);
    h.shared.ui_invalidations.push(UiInvalidation::TrackParam {
        track: 0,
        change: TrackParamInvalidation::BusSends,
    });
    assert_eq!(h.eval_all("fx-send.has-locks"), Value::Bool(false));
}

#[test]
fn param_printing_follows_the_print_latch_while_recording() {
    let (mut h, slot) = Harness::with_devices();
    h.eval_all(DEVICES);
    h.eval_all("(def printing #'cutoff.printing)");
    h.shared.step_print.lock().unwrap().latch(
        0,
        PrintTarget::Effect {
            slot_idx: slot,
            param_idx: CUTOFF,
        },
        800.0,
    );
    h.sync();
    assert_eq!(h.slot("printing"), 0.0, "only while playing and recording");
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    h.shared.recording.store(true, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("printing"), 1.0);
    h.shared.step_print.lock().unwrap().disarm();
    h.sync();
    assert_eq!(h.slot("printing"), 0.0);
}
