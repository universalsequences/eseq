//! Stage 7b-2: devices beyond the track chain (MIDI effects, bus effects,
//! drum rack slots and their effects), their params and setters.

use super::*;

/// transpose-range's params: `min` (default -12, -96–96) and `max`.
const MIN: usize = 0;
/// The Filter's `cutoff` (Hz).
const CUTOFF: usize = 2;
/// The sampler's `start` (a percent param: stored 0–1, shown 0–100).
const START: usize = 2;

impl Harness {
    fn add_midi_fx(&mut self, track: usize, name: &str) -> usize {
        let slot = self
            .app
            .apply_recorded_track_midi_fx_chain_mutation(track, "Add MIDI FX", |app| {
                app.add_midi_fx_to_track_sync(track, name)
            })
            .expect("add MIDI FX");
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        slot
    }

    /// A bus ("FX") with a Filter, through a recorded chain edit (which
    /// binds its instance id); returns the bus position and effect slot.
    fn bus_with_filter(&mut self) -> (usize, usize) {
        let id = self.add_bus("FX");
        let bus = self.app.buses.iter().position(|bus| bus.id == id).unwrap();
        let slot = self
            .app
            .apply_recorded_bus_effect_chain_mutation(bus, "Add bus effect", |app| {
                app.add_builtin_bus_effect_sync(bus, "Filter")
            })
            .expect("add bus effect");
        self.share_buses_and_groups();
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        (bus, slot)
    }

    /// Track 2 becomes a drum rack holding one sampler slot.
    pub(super) fn rack_track(&mut self) {
        self.app
            .graph_controller()
            .add_blank_sampler_track()
            .expect("sampler track");
        self.app
            .group_track_to_instrument_rack_recorded(2)
            .expect("group to rack");
        self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn rack_slot(&self) -> sequencer::sequencer::RackSlotSnapshot {
        self.rack_slot_at(0)
    }

    fn rack_slot_at(&self, slot: usize) -> sequencer::sequencer::RackSlotSnapshot {
        self.shared
            .state
            .live_rack_track_snapshot(2)
            .expect("rack")
            .slots[slot]
            .clone()
    }

    fn device_syncs(&self) -> u64 {
        self.frame.host_kinds.devices.syncs
    }

    fn midi_value(&self, track: usize, slot: usize, param: usize) -> f32 {
        self.shared.state.pattern.midi_fx_slots[track][slot]
            .defaults
            .get(param)
    }

    pub(super) fn fails(&mut self, code: &str, expected: &str) {
        self.editor.minibuffer = None;
        self.eval_all(code);
        self.drain();
        assert!(self.error().contains(expected), "{code}: {}", self.error());
    }
}

fn legacy(h: &Harness, field: &str) -> Option<Value> {
    h.rt().reactive_field_value("SEQ", field).cloned()
}

#[test]
fn midi_devices_read_after_sync_and_match_the_legacy_fields() {
    let mut h = Harness::new();
    assert_eq!(h.add_midi_fx(0, "transpose-range"), 0);
    assert_eq!(h.add_midi_fx(0, "arp"), 1);
    h.sync();
    h.eval_all(
        r#"(def t0 (track 0)) (def md (first t0.midi-devices)) (def arp (nth t0.midi-devices 1))
           (def mn (device-param md "min"))"#,
    );
    assert_eq!(h.eval_all("(len t0.midi-devices)"), Value::Number(2.0));
    assert_eq!(
        h.eval_all("(list md.role md.slot md.name md.type md.enabled arp.slot)"),
        h.eval_all(r#"(list "midi-fx" 0 "transpose-range" "transpose-range" true 1)"#)
    );
    assert_eq!(h.eval_all("md.track"), h.eval_all("t0"));
    assert_eq!(
        h.eval_all("(list md.bus md.container md.voices md.devices)"),
        h.eval_all("(list nil nil 0 (list))")
    );
    let track_id = h.app.track_registry.id_at(0).unwrap();
    let id = h
        .app
        .device_registry
        .midi_effect_id(track_id, 0)
        .expect("bound by the add");
    assert_eq!(h.eval_all("md.did"), Value::Number(id.0 as f64));
    // The chain is the instrument and effects only.
    assert_eq!(
        h.eval_all("(len (filter (lambda (d) (= d.role \"midi-fx\")) t0.devices))"),
        Value::Number(0.0)
    );
    // Params: the descriptor's, in display units.
    assert_eq!(
        h.eval_all("(list mn.index mn.name mn.min mn.max mn.default mn.type)"),
        h.eval_all(r#"(list 0 "min" -96 96 -12 "continuous")"#)
    );
    assert_eq!(h.eval_all("mn.device"), h.eval_all("md"));
    h.shared.state.pattern.midi_fx_slots[0][0]
        .defaults
        .set(MIN, -5.0);
    assert_eq!(
        h.eval_all("(list mn.base mn.value mn.locked)"),
        h.eval_all("(list -5 -5 false)")
    );
    // The legacy value field shows the same.
    sync_midi_fx_param_value_field(h.editor.runtime_mut(), &h.shared.state, 0, 0, MIN, None);
    let field = midi_fx_param_value_field(0, 0, MIN, "min");
    assert_eq!(legacy(&h, &field), Some(h.eval_all("mn.value")));
    // A lock at the selected step shows on the current track, as the
    // legacy field does.
    h.eval_all("(lock-param! mn (list (nth t0.steps 3)) 7)");
    h.drain_and_sync();
    h.shared.state.pattern.patterns[0].set_step_active(3, true);
    h.shared.selected_steps.lock().unwrap().insert(3);
    assert_eq!(
        h.eval_all("(list mn.value mn.locked mn.has-locks mn.base)"),
        h.eval_all("(list 7 true true -5)")
    );
    sync_midi_fx_param_value_field(h.editor.runtime_mut(), &h.shared.state, 0, 0, MIN, Some(3));
    assert_eq!(legacy(&h, &field), Some(Value::Number(7.0)));
}

#[test]
fn midi_device_params_set_through_history_with_undo() {
    let mut h = Harness::new();
    h.add_midi_fx(0, "transpose-range");
    h.sync();
    h.eval_all(
        r#"(def t0 (track 0)) (def md (first t0.midi-devices)) (def mn (device-param md "min"))"#,
    );
    let before = h.app.history.undo_len();
    h.eval_all("(set! mn.base -24)");
    h.drain_and_sync();
    assert_eq!(h.midi_value(0, 0, MIN), -24.0);
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert!(h.app.history.active_gesture().is_none(), "its own entry");
    assert_eq!(h.eval_all("mn.base"), Value::Number(-24.0));
    // Clamped; the same value again is no edit.
    h.eval_all("(set! mn.base -500)");
    h.drain();
    assert_eq!(h.midi_value(0, 0, MIN), -96.0);
    h.eval_all("(set! mn.base -96)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 2);
    app::edit::undo(&mut h.app);
    assert_eq!(h.midi_value(0, 0, MIN), -24.0);
    app::edit::undo(&mut h.app);
    assert_eq!(h.midi_value(0, 0, MIN), -12.0);
    // Locks: one entry for all steps, undone as one; cleared through history.
    h.eval_all("(lock-param! mn (list (nth t0.steps 1) (nth t0.steps 5)) 4)");
    h.drain();
    let slot = &h.shared.state.pattern.midi_fx_slots[0][0];
    assert_eq!(
        (slot.plocks.get(1, MIN), slot.plocks.get(5, MIN)),
        (Some(4.0), Some(4.0))
    );
    assert_eq!(slot.defaults.get(MIN), -12.0, "never the base");
    h.eval_all("(unlock-param! mn (list (nth t0.steps 1)))");
    h.drain();
    assert_eq!(
        h.shared.state.pattern.midi_fx_slots[0][0]
            .plocks
            .get(1, MIN),
        None
    );
    app::edit::undo(&mut h.app);
    assert_eq!(
        h.shared.state.pattern.midi_fx_slots[0][0]
            .plocks
            .get(1, MIN),
        Some(4.0)
    );
    app::edit::undo(&mut h.app);
    assert_eq!(
        h.shared.state.pattern.midi_fx_slots[0][0]
            .plocks
            .get(5, MIN),
        None
    );
    // Another track's step is an error.
    h.fails(
        "(let ((t1 (track 1))) (lock-param! mn (list (nth t1.steps 0)) 3))",
        "steps must be steps",
    );
}

#[test]
fn bus_devices_read_after_sync_and_set_through_history() {
    let mut h = Harness::new();
    let (bus, slot) = h.bus_with_filter();
    h.sync();
    h.eval_all(&format!(
        r#"(def b (nth (buses) {bus})) (def bd (first b.devices)) (def bc (device-param bd "cutoff"))"#
    ));
    assert_eq!(
        h.eval_all("(list (len b.devices) bd.role bd.slot bd.name bd.type bd.enabled)"),
        h.eval_all(&format!(
            r#"(list 1 "bus-effect" {slot} "Filter" "Filter" true)"#
        ))
    );
    assert_eq!(
        h.eval_all("(list bd.bus bd.track)"),
        h.eval_all("(list b nil)")
    );
    let bus_id = h.app.buses[bus].id;
    let id = h
        .app
        .device_registry
        .bus_audio_effect_id(bus_id, slot)
        .expect("bound");
    assert_eq!(h.eval_all("bd.did"), Value::Number(id.0 as f64));
    let base = h.app.buses[bus].effect_slots[slot].defaults[CUTOFF];
    assert_eq!(
        h.eval_all("(list bc.name bc.unit bc.base bc.value bc.locked bc.has-locks)"),
        h.eval_all(&format!(
            r#"(list "cutoff" "Hz" {base} {base} false false)"#
        ))
    );
    // The legacy value field shows the same.
    sync_bus_effect_param_value_field(h.editor.runtime_mut(), &h.app, bus, slot, CUTOFF);
    let field = bus_effect_param_value_field(bus, slot, CUTOFF, "cutoff");
    assert_eq!(legacy(&h, &field), Some(h.eval_all("bc.value")));
    // The base: its own undo entry, the shared bus copy follows.
    let before = h.app.history.undo_len();
    h.eval_all("(set! bc.base 1500)");
    h.drain_and_sync();
    assert_eq!(h.app.buses[bus].effect_slots[slot].defaults[CUTOFF], 1500.0);
    assert_eq!(
        h.shared.bus_state.lock().unwrap()[bus].effect_slots[slot].defaults[CUTOFF],
        1500.0
    );
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert_eq!(
        h.eval_all("(list bc.base bc.value)"),
        h.eval_all("(list 1500 1500)")
    );
    assert_eq!(legacy(&h, &field), Some(Value::Number(1500.0)));
    app::edit::undo(&mut h.app);
    assert_eq!(h.app.buses[bus].effect_slots[slot].defaults[CUTOFF], base);
    // A drag view's set!s while the pointer is down join one entry.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in [600, 700, 800] {
        h.eval_all(&format!("(set! bc.base {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    h.eval_all("(set! bc.base 850)");
    h.drain();
    assert_eq!(h.app.buses[bus].effect_slots[slot].defaults[CUTOFF], 850.0);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    app::edit::undo(&mut h.app);
    assert_eq!(h.app.buses[bus].effect_slots[slot].defaults[CUTOFF], base);
    // An enum or boolean param rebuilds the panels (as the bus knob does).
    let desc = h.app.buses[bus].effect_descriptors[slot].clone();
    let choice = desc
        .params
        .iter()
        .position(|param| param.is_enum() || param.is_boolean())
        .expect("the Filter has an enum or a boolean");
    let epoch = h.shared.fx_epoch.load(Ordering::Relaxed);
    let current = h.app.buses[bus].effect_slots[slot].defaults[choice];
    let next = if current >= desc.params[choice].max {
        desc.params[choice].min
    } else {
        current + 1.0
    };
    h.eval_all(&format!(
        "(let ((p (nth bd.params {choice}))) (set! p.base {next}))"
    ));
    h.drain();
    assert_eq!(h.app.buses[bus].effect_slots[slot].defaults[choice], next);
    assert!(
        h.shared.fx_epoch.load(Ordering::Relaxed) > epoch,
        "panel rebuilt"
    );
    // Bus effects take no p-locks.
    h.fails("(lock-param! bc (list) 300)", "bus effects take no p-locks");
    h.fails("(unlock-param! bc (list))", "bus effects take no p-locks");
}

#[test]
fn rack_slot_devices_hold_the_slot_instrument_its_voices_and_effects() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all(
        r#"(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
           (def start (device-param rs "start"))"#,
    );
    assert_eq!(h.eval_all("rk.type"), s("rack"));
    let slot = h.rack_slot();
    assert_eq!(
        h.eval_all("(list (len rk.devices) rs.role rs.slot rs.type rs.enabled rs.voices)"),
        h.eval_all(&format!(
            r#"(list 1 "rack-slot" 0 "sampler" true {})"#,
            slot.max_polyphony
        ))
    );
    assert_eq!(
        h.eval_all("(list rs.container rs.track rs.bus)"),
        h.eval_all("(list rk t2 nil)")
    );
    let sampler = sequencer::effects::EffectDescriptor::builtin_sampler();
    assert_eq!(
        h.eval_all("(len rs.params)"),
        Value::Number(sampler.params.len() as f64)
    );
    let stored = slot.instrument_slot.defaults[START];
    assert_eq!(
        h.eval_all("(list start.unit start.max)"),
        h.eval_all(r#"(list "%" 100)"#)
    );
    assert!((num(h.eval_all("start.base")) - f64::from(stored) * 100.0).abs() < 1e-3);
    // The legacy rack panel field shows the same.
    sync_rack_slot_instrument_param_value_field(h.editor.runtime_mut(), &h.app, 2, 0, START, None);
    let field = rack_slot_instrument_param_value_field(2, 0, START, "start");
    let shown = num(legacy(&h, &field).expect("legacy rack field"));
    assert!((shown - num(h.eval_all("start.value"))).abs() < 1e-3);
    // The base through history (display units), undone.
    let before = h.app.history.undo_len();
    h.eval_all("(set! start.base 50)");
    h.drain_and_sync();
    assert_eq!(h.rack_slot().instrument_slot.defaults[START], 0.5);
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert!((num(h.eval_all("start.value")) - 50.0).abs() < 1e-3);
    app::edit::undo(&mut h.app);
    assert_eq!(h.rack_slot().instrument_slot.defaults[START], stored);
    // A lock on the rack track's steps; clearing has no command yet.
    h.eval_all("(lock-param! start (list (nth t2.steps 2)) 25)");
    h.drain();
    let locked = h.rack_slot().instrument_slot.plocks[2][START];
    assert_eq!(locked, Some(0.25));
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    assert_eq!(
        h.eval_all("(list start.value start.locked start.has-locks)"),
        h.eval_all("(list 25 true true)")
    );
    h.fails(
        "(unlock-param! start (list (nth t2.steps 2)))",
        "no clear command",
    );
    // Voices: through history, the value rule, undo.
    let before = h.app.history.undo_len();
    h.eval_all("(set! rs.voices 3)");
    h.drain_and_sync();
    assert_eq!(h.rack_slot().max_polyphony, 3);
    assert_eq!(h.eval_all("rs.voices"), Value::Number(3.0));
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.fails("(set! rs.voices 0)", "voices takes");
    h.fails("(set! rs.voices 17)", "voices takes");
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_ALL} (set! rs.voices 2.5)"))
        .expect_err("set! checks the declared type");
    assert!(format!("{error:?}").contains("is :int"), "{error:?}");
    h.fails("(set! rk.voices 2)", "has no voices");
    assert_eq!(h.rack_slot().max_polyphony, 3);
    app::edit::undo(&mut h.app);
    assert_eq!(h.rack_slot().max_polyphony, slot.max_polyphony);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("rs.voices"),
        Value::Number(slot.max_polyphony as f64)
    );
    // An effect on the slot: a device the slot holds, with params.
    let effect = h
        .app
        .apply_recorded_rack_effect_chain_mutation(2, 0, "Add rack effect", |app| {
            app.add_builtin_rack_slot_effect_sync(2, 0, "Filter")
        })
        .expect("add rack effect");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("rs"),
        h.eval_all("(first rk.devices)"),
        "the slot device stays"
    );
    h.eval_all(r#"(def re (first rs.devices)) (def rc (device-param re "cutoff"))"#);
    assert_eq!(
        h.eval_all("(list re.role re.slot re.name re.container re.track)"),
        h.eval_all(&format!(r#"(list "rack-effect" {effect} "Filter" rs t2)"#))
    );
    h.eval_all("(set! rc.base 900)");
    h.drain_and_sync();
    let rack_effect = h.rack_slot().effect_slots[effect].defaults[CUTOFF];
    assert_eq!(rack_effect, 900.0);
    assert_eq!(h.eval_all("rc.value"), Value::Number(900.0));
    h.eval_all("(lock-param! rc (list (nth t2.steps 4)) 400)");
    h.drain();
    assert_eq!(
        h.rack_slot().effect_slots[effect].plocks[4][CUTOFF],
        Some(400.0)
    );
    h.eval_all("(unlock-param! rc (list (nth t2.steps 4)))");
    h.drain();
    assert_eq!(h.rack_slot().effect_slots[effect].plocks[4][CUTOFF], None);
}

#[test]
fn device_delete_targets_follow_and_set_the_active_target() {
    let mut h = Harness::new();
    h.rack_track();
    h.add_effect(0, "Filter");
    let (bus, slot) = h.bus_with_filter();
    h.sync();
    h.eval_all(&format!(
        r#"(def t0 (track 0)) (def flt (first t0.devices)) (def t2 (track 2))
           (def rk (first t2.devices)) (def rs (first rk.devices))
           (def b (nth (buses) {bus})) (def bd (first b.devices))
           (def rs-target #'rs.delete-target)"#
    ));
    assert_eq!(h.computed(f::DEVICE_DELETE_TARGET), 1, "the #' seed only");
    h.sync();
    let observed = h.computed(f::DEVICE_DELETE_TARGET);
    // A rack slot.
    h.eval_all("(set! rs.delete-target true)");
    h.drain_and_sync();
    let target = h.shared.active_delete_target.lock().unwrap().clone();
    assert_eq!(
        target,
        Some(ActiveDeleteTarget::RackSlot { track: 2, slot: 0 })
    );
    assert_eq!(h.slot("rs-target"), 1.0);
    assert!(
        h.computed(f::DEVICE_DELETE_TARGET) > observed,
        "observed: computed per tick"
    );
    // A bus effect; a chain effect of the current track.
    h.eval_all("(set! bd.delete-target true)");
    h.drain_and_sync();
    let target = h.shared.active_delete_target.lock().unwrap().clone();
    assert_eq!(
        target,
        Some(ActiveDeleteTarget::FxEffect {
            chain: FxDeleteChain::Bus,
            bus: Some(bus),
            slot
        })
    );
    assert_eq!(h.slot("rs-target"), 0.0);
    assert_eq!(
        h.eval_all("(list bd.delete-target rs.delete-target)"),
        h.eval_all("(list true false)")
    );
    h.eval_all("(set! flt.delete-target true)");
    h.drain_and_sync();
    assert_eq!(
        h.eval_all("(list flt.delete-target bd.delete-target)"),
        h.eval_all("(list true false)")
    );
    // false clears the target only while it names the device.
    h.eval_all("(set! bd.delete-target false)");
    h.drain_and_sync();
    assert_eq!(h.eval_all("flt.delete-target"), Value::Bool(true));
    h.eval_all("(set! flt.delete-target false)");
    h.drain_and_sync();
    assert_eq!(*h.shared.active_delete_target.lock().unwrap(), None);
    // An instrument never; another track's chain effect only when current.
    h.fails("(set! rk.delete-target true)", "no delete target");
    h.shared.current_track.store(1, Ordering::Relaxed);
    h.fails("(set! flt.delete-target true)", "only on the current track");
    assert_eq!(h.eval_all("flt.delete-target"), Value::Bool(false));
}

#[test]
fn devices_keep_their_instances_across_reorders_and_bindings_and_go_with_a_load() {
    let mut h = Harness::new();
    h.add_midi_fx(0, "transpose-range");
    h.add_midi_fx(0, "arp");
    h.sync();
    h.eval_all(
        r#"(def t0 (track 0)) (def tr (first t0.midi-devices)) (def arp (nth t0.midi-devices 1))
           (def mn (device-param tr "min"))"#,
    );
    // A reorder keeps both instances and their params; only slot moves.
    h.app
        .apply_recorded_track_midi_fx_chain_mutation(0, "Move MIDI FX", |app| {
            app.move_midi_fx_slot_sync(0, 1, Some(0))
        })
        .expect("move");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_all("(first t0.midi-devices)"), h.eval_all("arp"));
    assert_eq!(h.eval_all("(nth t0.midi-devices 1)"), h.eval_all("tr"));
    assert_eq!(
        h.eval_all("(list arp.slot tr.slot mn.name)"),
        h.eval_all(r#"(list 0 1 "min")"#)
    );
    assert_eq!(h.eval_all("(device-param tr \"min\")"), h.eval_all("mn"));
    // A setter addressed by did lands on the moved device.
    h.eval_all("(set! mn.base 3)");
    h.drain();
    assert_eq!(h.midi_value(0, 1, MIN), 3.0);
    // Another effect in a device's place (same identity, another
    // descriptor) replaces its params: old handles go stale.
    let track_id = h.app.track_registry.id_at(0).unwrap();
    h.shared.state.pattern.track_params[0].set_midi_fx_chain(vec!["arp".into(), "arp".into()]);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("(nth t0.midi-devices 1)"),
        h.eval_all("tr"),
        "the device stays"
    );
    let Value::Instance(old_param) = h.eval_all("mn") else {
        panic!("mn is an instance");
    };
    assert!(!h.rt().instance_is_live(old_param));
    assert_eq!(h.eval_all("tr.name"), s("arp"));
    // A slot whose id is not bound yet gets one later: the instance is
    // re-keyed, not replaced.
    h.shared.state.pattern.track_params[1].set_midi_fx_chain(vec!["arp".into()]);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_all("(def t1 (track 1)) (def late (first t1.midi-devices))");
    let unbound = num(h.eval_all("late.did"));
    assert!(unbound >= UNBOUND_EFFECT_DID as f64, "{unbound}");
    h.app
        .apply_recorded_track_midi_fx_chain_mutation(1, "Bind", |_| Ok(()))
        .expect("capture binds the chain");
    let id = h
        .app
        .device_registry
        .midi_effect_id(h.app.track_registry.id_at(1).unwrap(), 0);
    let id = id.expect("bound");
    let syncs = h.device_syncs();
    h.sync();
    assert_eq!(
        h.device_syncs(),
        syncs + 1,
        "a binding moves the registry generation"
    );
    assert_eq!(h.eval_all("(first t1.midi-devices)"), h.eval_all("late"));
    assert_eq!(h.eval_all("late.did"), Value::Number(id.0 as f64));
    // Deleting a MIDI effect drops its device.
    let _ = track_id;
    h.app
        .apply_recorded_track_midi_fx_chain_mutation(0, "Delete MIDI FX", |app| {
            app.delete_midi_fx_slot(0, 0)
        })
        .expect("delete");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_all("(len t0.midi-devices)"), Value::Number(1.0));
    assert_eq!(h.eval_all("(first t0.midi-devices)"), h.eval_all("tr"));
    // A project load replaces every device.
    h.command("new-project", Value::Nil);
    h.sync();
    assert_eq!(
        h.eval_all("(list tr.name late.name)"),
        h.eval_all(r#"(list "" "")"#)
    );
    assert_eq!(
        h.eval_all("(let ((t (track 0))) (len t.midi-devices))"),
        Value::Number(0.0)
    );
}

#[test]
fn device_syncs_run_only_when_their_key_moves() {
    let mut h = Harness::new();
    h.add_midi_fx(0, "transpose-range");
    let slot = h.add_effect(0, "Filter");
    h.sync();
    // A continuous param drag (here: set!s) moves no device key.
    h.eval_all(
        r#"(def t0 (track 0)) (def flt (first t0.devices)) (def md (first t0.midi-devices))
           (def cutoff (device-param flt "cutoff")) (def mn (device-param md "min"))"#,
    );
    h.sync();
    let syncs = h.device_syncs();
    // Params of these devices are computed only while observed.
    h.sync();
    assert_eq!(h.computed(f::PARAM_VALUE), 0);
    h.eval_all("(def shown #'mn.value)");
    let seeded = h.computed(f::PARAM_VALUE);
    h.sync();
    h.sync();
    assert_eq!(
        h.computed(f::PARAM_VALUE),
        seeded + 2,
        "once per tick while observed"
    );
    for value in [800, 900, 1000] {
        h.eval_all(&format!(
            "(set! cutoff.base {value}) (set! mn.base {})",
            value / 100
        ));
        h.drain_and_sync();
    }
    assert_eq!(h.filter_slot(slot).defaults.get(CUTOFF), 1000.0);
    assert_eq!(h.midi_value(0, 0, MIN), 10.0);
    assert_eq!(h.device_syncs(), syncs, "no device sync for a value edit");
    // A chain edit does.
    h.add_midi_fx(0, "arp");
    h.sync();
    assert_eq!(h.device_syncs(), syncs + 1);
    assert_eq!(h.eval_all("(len t0.midi-devices)"), Value::Number(2.0));
}

/// MIDI effects set straight into track 1's chain: no identities bound.
fn unbound_midi_chain(h: &mut Harness, names: &[&str]) {
    let chain: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    for (slot, name) in names.iter().enumerate() {
        let desc = sequencer::lisp_host::load_midi_fx_descriptor(name).expect("descriptor");
        h.shared.state.pattern.midi_fx_slots[1][slot].apply_descriptor(&desc, 0);
    }
    h.shared.state.pattern.track_params[1].set_midi_fx_chain(chain);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
}

fn did(h: &mut Harness, device: &str) -> u64 {
    num(h.eval_all(&format!("{device}.did"))) as u64
}

#[test]
fn reordering_unbound_devices_keeps_each_handle_on_its_own_device() {
    let mut h = Harness::new();
    // MIDI effects (transpose-range, arp), reordered to (arp, transpose-range)
    // by the edit that binds their identities.
    unbound_midi_chain(&mut h, &["transpose-range", "arp"]);
    h.sync();
    h.eval_all(
        r#"(def t1 (track 1)) (def tr (first t1.midi-devices)) (def arp (nth t1.midi-devices 1))
           (def mn (device-param tr "min"))"#,
    );
    assert!(did(&mut h, "tr") >= UNBOUND_EFFECT_DID && did(&mut h, "arp") >= UNBOUND_EFFECT_DID);
    h.app
        .apply_recorded_track_midi_fx_chain_mutation(1, "Move MIDI FX", |app| {
            app.move_midi_fx_slot_sync(1, 1, Some(0))
        })
        .expect("move");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("(list tr.name tr.slot arp.name arp.slot mn.name)"),
        h.eval_all(r#"(list "transpose-range" 1 "arp" 0 "min")"#)
    );
    assert_eq!(h.eval_all("(first t1.midi-devices)"), h.eval_all("arp"));
    assert_eq!(h.eval_all("(device-param tr \"min\")"), h.eval_all("mn"));
    let track_id = h.app.track_registry.id_at(1).unwrap();
    let registry = &h.app.device_registry;
    let bound = registry.midi_effect_id(track_id, 1).expect("bound").0;
    assert_eq!(did(&mut h, "tr"), bound);

    // Track chain effects (Filter, OTT) on track 0, the same way.
    let first = h.app.add_builtin_effect_sync(0, "Filter").expect("filter");
    let second = h.app.add_builtin_effect_sync(0, "OTT").expect("ott");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_all("(def t0 (track 0)) (def flt (first t0.devices)) (def ott (nth t0.devices 1))");
    assert_eq!(
        h.eval_all("(list flt.name ott.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    assert!(did(&mut h, "flt") >= UNBOUND_EFFECT_DID);
    h.app
        .apply_recorded_track_effect_chain_mutation(0, "Move effect", |app| {
            app.move_effect_slot_sync(0, second, Some(first))
        })
        .expect("move");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_all("(first t0.devices)"), h.eval_all("ott"));
    assert_eq!(h.eval_all("(nth t0.devices 1)"), h.eval_all("flt"));
    assert_eq!(
        h.eval_all("(list flt.name ott.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    assert!(did(&mut h, "flt") < UNBOUND_EFFECT_DID, "bound");

    // A bus's effects.
    let id = h.add_bus("FX");
    let bus = h.app.buses.iter().position(|bus| bus.id == id).unwrap();
    let first = h
        .app
        .add_builtin_bus_effect_sync(bus, "Filter")
        .expect("filter");
    let second = h.app.add_builtin_bus_effect_sync(bus, "OTT").expect("ott");
    h.share_buses_and_groups();
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_all(&format!(
        "(def b (nth (buses) {bus})) (def bf (first b.devices)) (def bo (nth b.devices 1))"
    ));
    assert_eq!(
        h.eval_all("(list bf.name bo.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    let unbound = did(&mut h, "bf") >= UNBOUND_EFFECT_DID;
    h.app
        .apply_recorded_bus_effect_chain_mutation(bus, "Move bus effect", |app| {
            app.move_bus_effect_slot_sync(bus, second, Some(first))
        })
        .expect("move");
    h.share_buses_and_groups();
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.eval_all("(first b.devices)"), h.eval_all("bo"));
    assert_eq!(h.eval_all("(nth b.devices 1)"), h.eval_all("bf"));
    assert_eq!(
        h.eval_all("(list bf.name bo.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    assert!(unbound || did(&mut h, "bf") < UNBOUND_EFFECT_DID);

    // A rack slot's effects (and the slot, bound on the way).
    h.rack_track();
    let first = h
        .app
        .add_builtin_rack_slot_effect_sync(2, 0, "Filter")
        .expect("filter");
    let second = h
        .app
        .add_builtin_rack_slot_effect_sync(2, 0, "OTT")
        .expect("ott");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_all(
        r#"(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
           (def rf (first rs.devices)) (def ro (nth rs.devices 1))"#,
    );
    assert_eq!(
        h.eval_all("(list rf.name ro.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    assert!(did(&mut h, "rf") >= UNBOUND_EFFECT_DID);
    h.app
        .apply_recorded_rack_effect_chain_mutation(2, 0, "Move rack effect", |app| {
            app.move_rack_slot_effect_slot_sync(2, 0, second, first)
        })
        .expect("move");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("(first rk.devices)"),
        h.eval_all("rs"),
        "the slot stays"
    );
    assert_eq!(h.eval_all("(first rs.devices)"), h.eval_all("ro"));
    assert_eq!(h.eval_all("(nth rs.devices 1)"), h.eval_all("rf"));
    assert_eq!(
        h.eval_all("(list rf.name ro.name)"),
        h.eval_all(r#"(list "Filter" "OTT")"#)
    );
    assert!(did(&mut h, "rf") < UNBOUND_EFFECT_DID, "bound");
    assert!(did(&mut h, "rs") < UNBOUND_EFFECT_DID, "bound");
}

#[test]
fn several_edits_of_one_unbound_device_in_one_eval_all_land() {
    let mut h = Harness::new();
    unbound_midi_chain(&mut h, &["transpose-range"]);
    h.sync();
    h.eval_all(
        r#"(def t1 (track 1)) (def tr (first t1.midi-devices))
           (def mn (device-param tr "min")) (def mx (device-param tr "max"))"#,
    );
    assert!(did(&mut h, "tr") >= UNBOUND_EFFECT_DID);
    // The first edit binds the device's identity; the second still names
    // it by its placeholder.
    h.editor.minibuffer = None;
    h.eval_all("(set! mn.base -20) (set! mx.base 20)");
    h.drain();
    assert_eq!(h.error(), "");
    let slot = &h.shared.state.pattern.midi_fx_slots[1][0];
    assert_eq!((slot.defaults.get(0), slot.defaults.get(1)), (-20.0, 20.0));
    let track_id = h.app.track_registry.id_at(1).unwrap();
    let bound = h.app.device_registry.midi_effect_id(track_id, 0);
    assert!(bound.is_some(), "the first edit bound the device");
    // A drag keeps landing until the next sync re-keys the device.
    h.gesture.pointer_down = true;
    for value in [-30, -31, -32] {
        h.eval_all(&format!("(set! mn.base {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    assert_eq!(h.midi_value(1, 0, 0), -32.0);
    h.sync();
    assert!(did(&mut h, "tr") < UNBOUND_EFFECT_DID, "re-keyed");
    assert_eq!(h.eval_all("(first t1.midi-devices)"), h.eval_all("tr"));
}

#[test]
fn a_bind_alone_re_keys_a_chain_device() {
    let mut h = Harness::new();
    h.app.add_builtin_effect_sync(0, "Filter").expect("filter");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_all("(def t0 (track 0)) (def flt (first t0.devices))");
    assert!(did(&mut h, "flt") >= UNBOUND_EFFECT_DID);
    // A capture binds the chain; no epoch moves, and nothing is recorded.
    let revision = h.app.history.current_revision();
    h.app
        .apply_recorded_track_effect_chain_mutation(0, "Bind", |_| Ok(()))
        .expect("capture binds the chain");
    assert_eq!(
        h.app.history.current_revision(),
        revision,
        "no history entry"
    );
    h.sync();
    let track_id = h.app.track_registry.id_at(0).unwrap();
    let slot = sequencer::effects::BUILTIN_SLOT_COUNT;
    let id = h
        .app
        .device_registry
        .audio_effect_id(track_id, slot)
        .expect("bound");
    assert_eq!(did(&mut h, "flt"), id.0);
    assert_eq!(h.eval_all("(first t0.devices)"), h.eval_all("flt"));
}

#[test]
fn value_edits_read_no_file_and_do_no_device_work() {
    let mut h = Harness::new();
    h.add_midi_fx(0, "transpose-range");
    h.rack_track();
    h.sync();
    h.eval_all(
        r#"(def t0 (track 0)) (def md (first t0.midi-devices)) (def mn (device-param md "min"))
           (def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
           (def start (device-param rs "start"))"#,
    );
    h.sync();
    // The history command a MIDI effect edit lands through clamps against
    // the effect's descriptor itself (`AppCommand::SetMidiFxParam`, shared
    // with the legacy knob): what it reads is the baseline, and the setter
    // adds nothing to it.
    let reads = sequencer::lisp_host::midi_fx_source_reads();
    let command = app::AppCommand::SetMidiFxParam {
        track: 0,
        slot_idx: 0,
        param_idx: MIN,
        value: 0.0,
    };
    app::apply_command(&mut h.app, command);
    let per_command = sequencer::lisp_host::midi_fx_source_reads() - reads;
    let reads = sequencer::lisp_host::midi_fx_source_reads();
    let loads = h.frame.host_kinds.devices.midi_fx_loads;
    let (syncs, racks) = (h.device_syncs(), h.frame.host_kinds.devices.racks_synced);
    for value in [1, 2, 3] {
        h.eval_all(&format!("(set! mn.base {value})"));
        h.drain_and_sync();
    }
    assert_eq!(
        sequencer::lisp_host::midi_fx_source_reads() - reads,
        3 * per_command,
        "the setter reads no file"
    );
    assert_eq!(h.device_syncs(), syncs, "no device work");
    // A rack knob drag: no file read at all.
    let reads = sequencer::lisp_host::midi_fx_source_reads();
    let revision = h.shared.state.pattern.rack_tracks.revision();
    h.gesture.pointer_down = true;
    for value in [10, 20, 30] {
        h.eval_all(&format!("(set! start.base {value})"));
        h.drain_and_sync();
    }
    h.gesture.pointer_down = false;
    assert_eq!(h.midi_value(0, 0, MIN), 3.0);
    assert!((h.rack_slot().instrument_slot.defaults[START] - 0.3).abs() < 1e-6);
    assert_eq!(
        sequencer::lisp_host::midi_fx_source_reads(),
        reads,
        "no file read"
    );
    assert_eq!(h.frame.host_kinds.devices.midi_fx_loads, loads);
    assert!(
        h.shared.state.pattern.rack_tracks.revision() > revision,
        "the rack moved"
    );
    assert_eq!(
        h.frame.host_kinds.devices.racks_synced, racks,
        "no rack re-synced"
    );
    assert_eq!(h.device_syncs(), syncs, "no device work");
    // A layout edit (the slot's voices) re-syncs the rack.
    h.eval_all("(set! rs.voices 2)");
    h.drain_and_sync();
    assert_eq!(h.frame.host_kinds.devices.racks_synced, racks + 1);
    assert_eq!(h.eval_all("rs.voices"), Value::Number(2.0));
    // The library loads again only once it changed.
    h.add_midi_fx(0, "arp");
    h.sync();
    assert_eq!(h.frame.host_kinds.devices.midi_fx_loads, loads);
}

impl Harness {
    /// Rack track 2 (see [`Self::rack_track`]) gains a second, blank
    /// sampler slot (through the recorded slot add, which binds its id).
    fn second_rack_slot(&mut self) {
        let buffer = sequencer::instruments::sampler::create_silent_buffer(self.app.graph.lg.0)
            .expect("buffer");
        self.app
            .apply_recorded_rack_slot_add(2, "Add rack sample", |app| {
                app.graph_controller()
                    .add_sampler_slot_to_rack_buffer(2, buffer, 44_100, "Second")
            })
            .expect("second slot");
        self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    }

    fn strip_computed(&self) -> u64 {
        strip_keys().map(|key| self.computed(key)).sum()
    }
}

#[test]
fn rack_slot_strip_locks_list_the_controls_some_step_locks() {
    // eseq-0l17.61: the rack panel's p-lock presence dot (the legacy
    // SEQ.track-plock-any rack slot rows), any step of the pattern, the
    // track current or not.
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
         (def s2 (nth t2.steps 2)) (def s5 (nth t2.steps 5))",
    );
    assert_eq!(h.eval_all("rs.strip-locks"), list_value([]));
    h.eval_all("(lock-strip! rs \"muted\" (list s5) true)");
    h.eval_all("(lock-strip! rs \"gain\" (list s2) 0.5)");
    h.drain();
    h.sync();
    // In RackSlotParam order, by the slot dicts' control names.
    assert_eq!(
        h.eval_all("rs.strip-locks"),
        h.eval_all("(list \"gain\" \"mute\")")
    );
    assert_eq!(
        h.eval_all("rk.strip-locks"),
        list_value([]),
        "any other device: none"
    );
    h.eval_all("(unlock-strip! rs \"gain\" (list s2))");
    h.drain();
    h.sync();
    assert_eq!(h.eval_all("rs.strip-locks"), h.eval_all("(list \"mute\")"));
    // Observed, it is cached under the track's p-lock key: an idle tick
    // neither scans the pattern nor takes the rack lock.
    h.eval_all(r#"(effect-buffer "*locks*" (label (str rs.strip-locks)))"#);
    h.show_all();
    h.sync();
    let scans = h.frame.host_kinds.shared.borrow().panel_scans;
    let locks = h.frame.host_kinds.devices.strip_locks;
    let computed = h.computed(f::DEVICE_STRIP_LOCKS);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.computed(f::DEVICE_STRIP_LOCKS), computed + 3, "observed");
    assert_eq!(h.frame.host_kinds.shared.borrow().panel_scans, scans);
    assert_eq!(h.frame.host_kinds.devices.strip_locks, locks);
    h.eval_all("(lock-strip! rs \"gain\" (list s5) 0.25)");
    h.drain();
    h.sync();
    assert_eq!(
        h.eval_all("rs.strip-locks"),
        h.eval_all("(list \"gain\" \"mute\")")
    );
}

#[test]
fn rack_slot_strip_controls_read_their_base_display_and_lock_state() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
         (def s2 (nth t2.steps 2)) (def s5 (nth t2.steps 5))",
    );
    let slot = h.rack_slot();
    assert_eq!(
        h.eval_all(
            "(list rs.gain rs.gain-display rs.gain-locked rs.pan rs.pan-display rs.pan-locked
                   rs.muted rs.muted-display rs.soloed rs.soloed-locked rs.choke)"
        ),
        h.eval_all(&format!(
            "(list {g} {g} false {p} {p} false false false false false 0)",
            g = slot.gain,
            p = slot.pan
        ))
    );
    // Any other device reads 0 / false.
    assert_eq!(
        h.eval_all("(list rk.gain rk.gain-display rk.pan rk.muted rk.soloed-locked rk.choke)"),
        h.eval_all("(list 0 0 0 false false 0)")
    );
    // Locks (one undo entry each); the base stays.
    let before = h.app.history.undo_len();
    h.eval_all("(lock-strip! rs \"gain\" (list s2 s5) 0.5)");
    h.eval_all("(lock-strip! rs \"muted\" (list s2) true)");
    h.eval_all("(lock-strip! rs \"pan\" (list s5) -0.25)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 3);
    let locks = h.rack_slot().param_plocks;
    assert_eq!(locks.get(2, RackSlotParam::Gain), Some(0.5));
    assert_eq!(locks.get(5, RackSlotParam::Gain), Some(0.5));
    assert_eq!(locks.get(2, RackSlotParam::Mute), Some(1.0));
    assert_eq!(locks.get(5, RackSlotParam::Pan), Some(-0.25));
    // Nothing displayed (the rack is not the current track): the base.
    h.sync();
    assert_eq!(
        h.eval_all("(list rs.gain-display rs.gain-locked rs.muted-display rs.muted-locked)"),
        h.eval_all(&format!("(list {} false false false)", slot.gain))
    );
    // The current track's selected step shows its locks.
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    assert_eq!(
        h.eval_all(
            "(list rs.gain rs.gain-display rs.gain-locked rs.muted rs.muted-display
                   rs.muted-locked rs.pan-display rs.pan-locked)"
        ),
        h.eval_all(&format!(
            "(list {} 0.5 true false true true {} false)",
            slot.gain, slot.pan
        ))
    );
    // The legacy rack strip field shows the same value.
    for (param, field) in [
        (RackSlotParam::Gain, "rs.gain-display"),
        (RackSlotParam::Mute, "rs.muted-display"),
    ] {
        sync_rack_slot_control_value_field(h.editor.runtime_mut(), &h.app, 2, 0, param, Some(2));
        assert_eq!(
            legacy(&h, &rack_slot_value_field(2, 0, param)),
            Some(h.eval_all(field)),
            "{field}"
        );
    }
    // While playing with no selection: the playing step's.
    h.shared.selected_steps.lock().unwrap().clear();
    let transport = &h.shared.state.transport;
    transport.track_playheads[2].store(5, Ordering::Relaxed);
    h.set_playing(true);
    h.sync();
    assert_eq!(
        h.eval_all("(list rs.gain-display rs.pan-display rs.pan-locked rs.muted-display)"),
        h.eval_all("(list 0.5 -0.25 true false)")
    );
    h.set_playing(false);
    // Locks already holding the value are left alone; clearing is one
    // entry, and only steps holding a lock count.
    let before = h.app.history.undo_len();
    h.eval_all("(lock-strip! rs \"gain\" (list s2 s5) 0.5)");
    h.eval_all("(unlock-strip! rs \"pan\" (list s2))");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before, "nothing to do");
    h.eval_all("(unlock-strip! rs \"gain\" (list s2 s5))");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert_eq!(h.rack_slot().param_plocks.get(2, RackSlotParam::Gain), None);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    assert_eq!(
        h.eval_all("(list rs.gain-display rs.gain-locked)"),
        h.eval_all(&format!("(list {} false)", slot.gain))
    );
    app::edit::undo(&mut h.app);
    assert_eq!(
        h.rack_slot().param_plocks.get(2, RackSlotParam::Gain),
        Some(0.5)
    );
    // The value rule: the field's own range, a bool for the flags; locks of
    // another track's steps, of an unlockable field, of another device.
    h.fails(
        "(lock-strip! rs \"gain\" (list s2) 3)",
        "a number from 0 to 2",
    );
    h.fails(
        "(lock-strip! rs \"pan\" (list s2) -2)",
        "a number from -1 to 1",
    );
    h.fails("(lock-strip! rs \"muted\" (list s2) 1)", "true or false");
    h.fails("(lock-strip! rs \"choke\" (list s2) 1)", "takes no p-locks");
    h.fails(
        "(let ((t0 (track 0))) (lock-strip! rs \"gain\" (list (nth t0.steps 2)) 1))",
        "steps of the device's track",
    );
    h.fails("(lock-strip! rk \"gain\" (list s2) 1)", "no strip controls");
    assert_eq!(
        h.rack_slot().param_plocks.get(2, RackSlotParam::Gain),
        Some(0.5)
    );
}

#[test]
fn rack_slot_strip_setters_follow_the_value_rule_through_history() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    let slot = h.rack_slot();
    // Each a history entry of its own; undone.
    let before = h.app.history.undo_len();
    h.eval_all("(set! rs.gain 1.5)");
    h.drain();
    h.eval_all("(set! rs.pan -0.5)");
    h.drain();
    h.eval_all("(toggle! rs.muted)");
    h.drain();
    h.eval_all("(toggle! rs.soloed)");
    h.drain();
    h.eval_all("(set! rs.choke 3)");
    h.drain();
    h.eval_all("(set! rs.enabled false)");
    h.drain_and_sync();
    let edited = h.rack_slot();
    assert_eq!(
        (edited.gain, edited.pan, edited.mute, edited.solo),
        (1.5, -0.5, true, true)
    );
    assert_eq!((edited.choke_group, edited.enabled), (Some(3), false));
    assert_eq!(h.app.history.undo_len(), before + 6);
    assert_eq!(
        h.eval_all("(list rs.gain rs.pan rs.muted rs.soloed rs.choke rs.enabled)"),
        h.eval_all("(list 1.5 -0.5 true true 3 false)")
    );
    // The current value is a no-op (no entry).
    h.eval_all("(set! rs.gain 1.5) (set! rs.muted true) (set! rs.choke 3) (set! rs.enabled false)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 6);
    // Choke 0 is none.
    h.eval_all("(set! rs.choke 0)");
    h.drain();
    assert_eq!(h.rack_slot().choke_group, None);
    app::edit::undo(&mut h.app);
    for _ in 0..6 {
        app::edit::undo(&mut h.app);
    }
    let undone = h.rack_slot();
    assert_eq!(
        (undone.gain, undone.pan, undone.mute, undone.solo),
        (slot.gain, slot.pan, slot.mute, slot.solo)
    );
    assert_eq!(
        (undone.choke_group, undone.enabled),
        (slot.choke_group, slot.enabled)
    );
    // Out of range, wrong type, another device: errors that change nothing.
    h.fails("(set! rs.gain 2.5)", "a number from 0 to 2");
    h.fails("(set! rs.gain -0.1)", "a number from 0 to 2");
    h.fails("(set! rs.pan 1.5)", "a number from -1 to 1");
    h.fails("(set! rs.choke 17)", "an integer from 0 to 16");
    for code in [
        "(set! rs.choke 2.5)",
        "(set! rs.muted 1)",
        "(set! rs.gain \"1\")",
    ] {
        let error = h
            .editor
            .runtime_mut()
            .eval_str(&format!("{REFER_ALL} {code}"))
            .expect_err("set! checks the declared type");
        assert!(format!("{error:?}").contains("is :"), "{code}: {error:?}");
    }
    h.fails("(set! rk.gain 1)", "has no gain");
    h.fails("(set! rk.muted true)", "has no muted");
    h.fails("(set! rk.enabled false)", "enabled is not settable");
    assert_eq!(h.rack_slot().gain, slot.gain);
    // A drag view's gain set!s while the pointer is down join one entry; a
    // flag set while it is down is an entry of its own.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in ["0.2", "0.3", "0.4"] {
        h.eval_all(&format!("(set! rs.gain {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    h.eval_all("(set! rs.gain 0.45)");
    h.drain();
    assert_eq!(h.rack_slot().gain, 0.45);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    h.gesture.pointer_down = true;
    h.eval_all("(set! rs.muted true)");
    h.drain();
    h.eval_all("(set! rs.muted false)");
    h.drain();
    h.gesture.pointer_down = false;
    assert_eq!(h.app.history.undo_len(), before + 3, "a flag each");
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    assert_eq!(h.rack_slot().gain, slot.gain);
}

#[test]
fn rack_slot_strip_base_reads_round_trip_even_out_of_range() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    // A gain stored past the value rule's range (as a loaded project may
    // hold) reads back unclamped.
    {
        let racks = &h.shared.state.pattern.rack_tracks;
        let mut racks = racks.lock().unwrap();
        racks[2].as_mut().expect("rack").slots[0].gain = 2.5;
    }
    h.sync();
    assert_eq!(h.eval_all("rs.gain"), Value::Number(2.5));
    // Setting the value read back is a no-op: no error, no undo entry.
    let before = h.app.history.undo_len();
    h.editor.minibuffer = None;
    h.eval_all("(set! rs.gain rs.gain)");
    h.drain();
    assert_eq!(h.error(), "", "the current value is no error");
    assert_eq!(h.app.history.undo_len(), before);
    assert_eq!(h.rack_slot().gain, 2.5);
    // Any other value still follows the value rule.
    h.fails("(set! rs.gain 2.4)", "a number from 0 to 2");
    assert_eq!(h.rack_slot().gain, 2.5);
}

#[test]
fn rack_slot_strip_fields_are_computed_only_while_observed() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    h.sync();
    let (computed, locks) = (h.strip_computed(), h.frame.host_kinds.devices.strip_locks);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(
        h.strip_computed(),
        computed,
        "nothing observes a strip field"
    );
    assert_eq!(h.frame.host_kinds.devices.strip_locks, locks);
    // Two held bindings: computed per tick under one rack lock.
    h.eval_all("(def g #'rs.gain-display) (def m #'rs.muted-locked) (def c #'rk.choke)");
    h.sync();
    let (computed, locks) = (h.strip_computed(), h.frame.host_kinds.devices.strip_locks);
    // Unchanged values are compared with their cells, so they push nothing.
    assert!(!h.sync(), "unchanged strip values push nothing");
    assert_eq!(
        h.strip_computed(),
        computed + 3,
        "the three observed fields"
    );
    assert_eq!(
        h.frame.host_kinds.devices.strip_locks,
        locks + 1,
        "one rack lock per tick"
    );
    // A drag repaints the binding without device work.
    let syncs = h.device_syncs();
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.eval_all("(set! rs.gain 0.25)");
    h.drain_and_sync();
    assert_eq!(h.slot("g"), 0.25);
    assert_eq!(h.device_syncs(), syncs, "no device work");
    h.eval_all("(lock-strip! rs \"muted\" (list (nth t2.steps 1)) true)");
    h.drain();
    h.shared.selected_steps.lock().unwrap().insert(1);
    h.sync();
    assert_eq!(h.slot("m"), 1.0);
}

#[test]
fn rack_slot_strip_handles_follow_their_slot_and_go_stale_with_it() {
    let mut h = Harness::new();
    h.rack_track();
    h.second_rack_slot();
    h.sync();
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices))
         (def first-slot (nth rk.devices 0)) (def second-slot (nth rk.devices 1))",
    );
    h.eval_all("(set! second-slot.gain 0.75) (set! second-slot.choke 2)");
    h.drain_and_sync();
    assert_eq!(h.rack_slot_at(1).gain, 0.75);
    let first = h.eval_all("(list first-slot)");
    let first = h.instances(first)[0];
    // Delete the first slot: the second's handle follows it to position 0.
    h.eval_all("(host-command \"delete-rack-slot\" (dict :track 2 :slot 0))");
    h.drain_and_sync();
    let rack = h.shared.state.live_rack_track_snapshot(2).expect("rack");
    assert_eq!(rack.slots.len(), 1);
    assert!(!h.rt().instance_is_live(first), "the deleted slot is stale");
    assert_eq!(
        h.eval_all("(list second-slot.slot second-slot.gain second-slot.choke)"),
        h.eval_all("(list 0 0.75 2)")
    );
    h.eval_all("(set! second-slot.pan 0.5)");
    h.drain();
    assert_eq!(h.rack_slot_at(0).pan, 0.5, "the handle edits its own slot");
    // A stale handle's setter changes nothing.
    let before = h.app.history.undo_len();
    h.editor.minibuffer = None;
    let _ = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_ALL} (set! first-slot.gain 1.25)"));
    h.drain();
    assert_eq!(h.app.history.undo_len(), before);
    assert_eq!(h.rack_slot_at(0).gain, 0.75);
    // Undoing the pan and the delete brings the first slot back under its
    // own identity (a new instance; the old handle stays stale).
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(
        h.eval_all("(list (len rk.devices) (nth rk.devices 1) second-slot.slot)"),
        h.eval_all("(list 2 second-slot 1)")
    );
    assert_eq!(
        h.eval_all("(let ((d (first rk.devices))) d.did)"),
        Value::Number(1.0)
    );
}

#[test]
fn rack_slot_base_note_and_voices_read_their_base_display_and_lock_state() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
         (def s2 (nth t2.steps 2)) (def s5 (nth t2.steps 5))",
    );
    let slot = h.rack_slot();
    let voices = slot.max_polyphony;
    assert_eq!(
        h.eval_all(
            "(list rs.base-note rs.base-note-display rs.base-note-locked rs.voices-display)"
        ),
        h.eval_all(&format!(
            "(list {b} {b} false {voices})",
            b = slot.instrument_base_note_offset
        ))
    );
    // Locks (one undo entry each); the bases stay.
    let before = h.app.history.undo_len();
    h.eval_all("(lock-strip! rs \"base-note\" (list s2 s5) 7)");
    h.eval_all("(lock-strip! rs \"voices\" (list s2) 3)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 2);
    let locks = h.rack_slot().param_plocks;
    assert_eq!(locks.get(2, RackSlotParam::BaseNote), Some(7.0));
    assert_eq!(locks.get(5, RackSlotParam::BaseNote), Some(7.0));
    assert_eq!(locks.get(2, RackSlotParam::MaxPolyphony), Some(3.0));
    // Nothing displayed (the rack is not the current track): the base.
    h.sync();
    assert_eq!(
        h.eval_all("(list rs.base-note-display rs.base-note-locked rs.voices-display)"),
        h.eval_all(&format!(
            "(list {} false {voices})",
            slot.instrument_base_note_offset
        ))
    );
    // The current track's selected step shows its locks.
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    assert_eq!(
        h.eval_all(
            "(list rs.base-note rs.base-note-display rs.base-note-locked rs.voices rs.voices-display)"
        ),
        h.eval_all(&format!(
            "(list {} 7 true {voices} 3)",
            slot.instrument_base_note_offset
        ))
    );
    // The legacy rack slot value fields show the same values.
    for (param, field) in [
        (RackSlotParam::BaseNote, "rs.base-note-display"),
        (RackSlotParam::MaxPolyphony, "rs.voices-display"),
    ] {
        sync_rack_slot_control_value_field(h.editor.runtime_mut(), &h.app, 2, 0, param, Some(2));
        assert_eq!(
            legacy(&h, &rack_slot_value_field(2, 0, param)),
            Some(h.eval_all(field)),
            "{field}"
        );
    }
    // Any other device: its own value, never locked (a track instrument's
    // base note; 0 else).
    let offsets = &h.shared.state.pattern.instrument_base_note_offsets;
    offsets[2].store(5.0f32.to_bits(), Ordering::Relaxed);
    assert_eq!(
        h.eval_all(
            "(list rk.base-note rk.base-note-display rk.base-note-locked rk.voices-display)"
        ),
        h.eval_all("(list 5 5 false 0)")
    );
    assert_eq!(
        rs_base_note(&mut h),
        slot.instrument_base_note_offset as f64
    );
    // Clearing is one entry; only steps holding a lock count.
    let before = h.app.history.undo_len();
    h.eval_all("(unlock-strip! rs \"voices\" (list s5))");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before, "nothing to clear");
    h.eval_all("(unlock-strip! rs \"base-note\" (list s2 s5))");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 1);
    h.sync();
    assert_eq!(
        h.eval_all("(list rs.base-note-display rs.base-note-locked rs.voices-display)"),
        h.eval_all(&format!(
            "(list {} false 3)",
            slot.instrument_base_note_offset
        ))
    );
    app::edit::undo(&mut h.app);
    assert_eq!(
        h.rack_slot().param_plocks.get(2, RackSlotParam::BaseNote),
        Some(7.0)
    );
    // The value rule: each field's own range.
    h.fails(
        "(lock-strip! rs \"base-note\" (list s2) 60)",
        "a number from -48 to 48",
    );
    h.fails(
        "(lock-strip! rs \"voices\" (list s2) 0)",
        "an integer from 1 to 16",
    );
    h.fails(
        "(lock-strip! rs \"voices\" (list s2) 2.5)",
        "an integer from 1 to 16",
    );
    h.fails(
        "(lock-strip! rk \"base-note\" (list s2) 1)",
        "no strip controls",
    );
    assert_eq!(
        h.rack_slot()
            .param_plocks
            .get(2, RackSlotParam::MaxPolyphony),
        Some(3.0)
    );
}

/// `rs.base-note` (rack slot 0 of track 2).
fn rs_base_note(h: &mut Harness) -> f64 {
    match h.eval_all("rs.base-note") {
        Value::Number(note) => note,
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn rack_slot_base_note_and_voices_setters_follow_the_value_rule_through_history() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    let slot = h.rack_slot();
    let offset = |h: &Harness| {
        let offsets = &h.shared.state.pattern.instrument_base_note_offsets;
        f32::from_bits(offsets[2].load(Ordering::Relaxed))
    };
    // A rack slot's base note: its own, through history; the track
    // instrument's is untouched.
    let before = h.app.history.undo_len();
    h.eval_all("(set! rs.base-note 12)");
    h.drain_and_sync();
    assert_eq!(h.rack_slot().instrument_base_note_offset, 12.0);
    assert_eq!(offset(&h), 0.0);
    assert_eq!(h.app.history.undo_len(), before + 1);
    assert_eq!(rs_base_note(&mut h), 12.0);
    // The legacy value field is repainted (rack_slot_strip_applied).
    let field = rack_slot_value_field(2, 0, RackSlotParam::BaseNote);
    assert_eq!(legacy(&h, &field), Some(Value::Number(12.0)));
    // The current value is a no-op.
    h.eval_all("(set! rs.base-note 12)");
    h.drain();
    assert_eq!(h.app.history.undo_len(), before + 1);
    // The track instrument's base note keeps its own path.
    h.eval_all("(set! rk.base-note -5)");
    h.drain_and_sync();
    assert_eq!(offset(&h), -5.0);
    assert_eq!(h.rack_slot().instrument_base_note_offset, 12.0);
    assert_eq!(h.app.history.undo_len(), before + 2);
    // Out of range, wrong type: errors that change nothing.
    h.fails("(set! rs.base-note 49)", "a number from -48 to 48");
    h.fails("(set! rs.voices 17)", "an integer from 1 to 16");
    assert_eq!(h.rack_slot().instrument_base_note_offset, 12.0);
    // A drag's base note and voices set!s join one entry each.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for value in ["13", "14", "15"] {
        h.eval_all(&format!("(set! rs.base-note {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    h.eval_all("(set! rs.base-note 16)");
    h.drain();
    assert_eq!(h.rack_slot().instrument_base_note_offset, 16.0);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    h.gesture.pointer_down = true;
    for value in ["2", "3", "4"] {
        h.eval_all(&format!("(set! rs.voices {value})"));
        h.drain();
    }
    h.gesture.pointer_down = false;
    h.eval_all("(set! rs.voices 5)");
    h.drain();
    assert_eq!(h.rack_slot().max_polyphony, 5);
    assert_eq!(h.app.history.undo_len(), before + 2, "one drag entry");
    app::edit::undo(&mut h.app);
    app::edit::undo(&mut h.app);
    assert_eq!(h.rack_slot().max_polyphony, slot.max_polyphony);
    assert_eq!(h.rack_slot().instrument_base_note_offset, 12.0);
}

#[test]
fn rack_slot_base_note_fields_are_computed_only_while_observed() {
    let mut h = Harness::new();
    h.rack_track();
    h.sync();
    h.eval_all("(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))");
    h.sync();
    let (computed, locks) = (h.strip_computed(), h.frame.host_kinds.devices.strip_locks);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.strip_computed(), computed, "nothing observes them");
    assert_eq!(h.frame.host_kinds.devices.strip_locks, locks);
    // A track instrument's base note alone takes no rack lock.
    h.eval_all("(def n #'rk.base-note)");
    h.sync();
    let (computed, locks) = (h.strip_computed(), h.frame.host_kinds.devices.strip_locks);
    h.sync();
    assert_eq!(h.strip_computed(), computed + 1);
    assert_eq!(
        h.frame.host_kinds.devices.strip_locks, locks,
        "no rack lock"
    );
    // The rack slot's: computed per tick under one rack lock.
    h.eval_all("(def d #'rs.base-note-display) (def v #'rs.voices-display)");
    h.sync();
    let (computed, locks) = (h.strip_computed(), h.frame.host_kinds.devices.strip_locks);
    assert!(!h.sync(), "unchanged values push nothing");
    assert_eq!(
        h.strip_computed(),
        computed + 3,
        "the three observed fields"
    );
    assert_eq!(h.frame.host_kinds.devices.strip_locks, locks + 1);
    // A base note edit repaints the binding without device work (a voices
    // edit moves the rack's layout fingerprint).
    let syncs = h.device_syncs();
    h.eval_all("(set! rs.base-note -3)");
    h.drain_and_sync();
    assert_eq!(h.slot("d"), -3.0);
    assert_eq!(h.device_syncs(), syncs, "no device work");
    h.eval_all("(set! rs.voices 4)");
    h.drain_and_sync();
    assert_eq!(h.slot("v"), 4.0);
}

#[test]
fn the_held_rack_controls_and_step_pickers_show_the_print_latch() {
    // eseq-0l17.61: while a print holds them, the step panel's pickers (the
    // edit step's fields), a rack macro's knob and a rack slot's strip show
    // the value being printed, not the one they snap back to between the
    // steps it lands on (the legacy print display fields).
    let mut h = Harness::new();
    h.rack_track();
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.sync();
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices))
         (def rm (nth rk.macros 1)) (def s0 (nth t2.steps 0)) (def s1 (nth t2.steps 1))
         (def gain #'rs.gain-display) (def knob #'rm.value)
         (def vel #'s0.velocity) (def vel1 #'s1.velocity)",
    );
    h.sync();
    let names = ["gain", "knob", "vel", "vel1"];
    let shown = |h: &mut Harness| names.map(|name| h.slot(name) as f32);
    let rest = shown(&mut h);
    {
        let mut print = h.shared.step_print.lock().unwrap();
        let gain = sequencer::sequencer::RackSlotParam::Gain;
        let strip = PrintTarget::RackSlotParam {
            slot_idx: 0,
            param: gain,
        };
        print.latch(2, strip, 0.3);
        print.latch(2, PrintTarget::RackMacro { macro_idx: 1 }, 0.8);
        print.latch(2, StepParam::Velocity, 0.2);
    }
    h.sync();
    assert_eq!(shown(&mut h), rest, "only while playing and recording");
    h.set_playing(true);
    h.shared.recording.store(true, Ordering::Relaxed);
    h.sync();
    // The edit step (the cursor's: step 0) alone.
    assert_eq!(shown(&mut h), [0.3, 0.8, 0.2, rest[3]]);
    assert_ne!(rest[..3], [0.3, 0.8, 0.2]);
    h.shared.step_print.lock().unwrap().disarm();
    h.sync();
    assert_eq!(shown(&mut h), rest);
}
