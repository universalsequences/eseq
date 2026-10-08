//! Stage 7: bus, group, send, master, engine and the new track, step, transport and selection fields.

use super::*;

#[test]
fn new_kinds_and_fields_read_through_kinds_after_sync() {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.share_buses_and_groups();
    {
        let params = &h.shared.state.pattern.track_params[1];
        params.set_pan(-0.5);
    }
    h.shared.state.pattern.step_data[0].set(2, StepParam::Velocity, 0.42);
    h.shared.state.pattern.step_data[0].set(2, StepParam::Duration, 3.0);
    h.shared.state.pattern.patterns[0].set_step_active(2, true);
    h.shared.state.transport.bpm.store(133, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playhead
        .store(17, Ordering::Relaxed);
    h.shared.selected_tracks.lock().unwrap().extend([1, 0]);
    h.meters.cached_bus_peak_levels = vec![0.0; h.app.buses.len()];
    let fx_index = h.app.buses.iter().position(|bus| bus.id == fx).unwrap();
    h.meters.cached_bus_peak_levels[fx_index] = 0.375;
    h.meters.cached_peak_l_level = 0.25;
    h.meters.cached_peak_r_level = 0.5;
    h.meters.cached_cpu_load_bits = 12.5f32.to_bits();
    assert!(h.sync());
    // Buses: every bus of the App, in order, keyed by position.
    assert_eq!(
        h.eval_all("(len (buses))"),
        Value::Number(h.app.buses.len() as f64)
    );
    h.eval_all(&format!("(def fx (nth (buses) {fx_index}))"));
    assert_eq!(h.eval_all("fx.name"), s("FX"));
    assert_eq!(h.eval_all("fx.index"), Value::Number(fx_index as f64));
    let volume = h.app.buses[fx_index].volume as f64;
    assert_eq!(h.eval_all("fx.volume"), Value::Number(volume));
    assert_eq!(h.eval_all("fx.muted"), Value::Bool(false));
    assert_eq!(h.eval_all("fx.soloed"), Value::Bool(false));
    assert_eq!(h.eval_all("fx.peak"), Value::Number(0.375));
    // Groups.
    assert_eq!(h.eval_all("(len (groups))"), Value::Number(1.0));
    h.eval_all("(def g (first (groups)))");
    let group = h.app.groups[0].clone();
    assert_eq!(h.eval_all("g.gid"), Value::Number(group.id as f64));
    assert_eq!(h.eval_all("g.name"), s(&group.name));
    assert_eq!(h.eval_all("g.collapsed"), Value::Bool(group.collapsed));
    assert_eq!(h.eval_all("g.rack"), Value::Bool(false));
    assert_eq!(h.eval_all("(= g.tracks (tracks))"), Value::Bool(true));
    assert_eq!(h.eval_all("(first g.color)"), Value::Symbol("rgb".into()));
    let group_bus = h
        .app
        .buses
        .iter()
        .position(|bus| bus.id.0 == group.bus_id)
        .expect("group bus");
    assert_eq!(
        h.eval_all("g.bus"),
        h.eval_all(&format!("(nth (buses) {group_bus})"))
    );
    // Track extras.
    h.eval_all("(def t0 (track 0)) (def t1 (track 1))");
    assert_eq!(h.eval_all("t0.group"), h.eval_all("g"));
    assert_eq!(h.eval_all("t1.pan"), Value::Number(-0.5));
    assert_eq!(h.eval_all("t0.soloed"), Value::Bool(false));
    assert_eq!(h.eval_all("t0.collapsed"), Value::Bool(false));
    assert_eq!(h.eval_all("t0.playhead"), Value::Number(-1.0));
    let timebase = h.shared.state.pattern.track_params[0]
        .get_timebase()
        .label();
    assert_eq!(h.eval_all("t0.timebase"), s(timebase));
    let instrument = instrument_type_label(h.app.graph.track_instrument_types[0]);
    assert_eq!(h.eval_all("t0.instrument-type"), s(instrument));
    assert_eq!(h.eval_all("t0.rack"), Value::Bool(false));
    // Sends: one per bus but the main mix.
    let sends = h
        .app
        .buses
        .iter()
        .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
        .count();
    assert_eq!(h.eval_all("(len t0.sends)"), Value::Number(sends as f64));
    h.eval_all("(def fx-send (first (filter (lambda (s) (= s.bus fx)) t0.sends)))");
    assert_eq!(h.eval_all("fx-send.track"), h.eval_all("t0"));
    assert_eq!(h.eval_all("fx-send.amount"), Value::Number(0.0));
    // Steps: held and the parameters.
    h.eval_all("(def s2 (nth t0.steps 2)) (def s4 (nth t0.steps 4)) (def s6 (nth t0.steps 6))");
    let velocity = h.shared.state.pattern.step_data[0].get(2, StepParam::Velocity) as f64;
    assert_eq!(h.eval_all("s2.velocity"), Value::Number(velocity));
    assert_eq!(h.eval_all("s2.duration"), Value::Number(3.0));
    assert_eq!(
        h.eval_all("s2.held"),
        Value::Bool(true),
        "a sounding step is held too"
    );
    assert_eq!(h.eval_all("s4.held"), Value::Bool(true));
    assert_eq!(h.eval_all("s6.held"), Value::Bool(false));
    // Transport, master, engine, selection.
    assert_eq!(h.eval_all("transport.bpm"), Value::Number(133.0));
    assert_eq!(h.eval_all("transport.position"), Value::Number(17.0));
    assert_eq!(h.eval_all("transport.metronome"), Value::Bool(false));
    assert_eq!(h.eval_all("transport.roll-mode"), Value::Bool(false));
    assert_eq!(h.eval_all("transport.record-quantize"), s("1/16"));
    assert_eq!(h.eval_all("master.peak-l"), Value::Number(0.25));
    assert_eq!(h.eval_all("master.peak-r"), Value::Number(0.5));
    assert_eq!(h.eval_all("master.recording"), Value::Bool(false));
    assert_eq!(h.eval_all("engine.cpu-load"), Value::Number(12.5));
    let latency = h.shared.state.pdc_latency_seconds() as f64 * 1000.0;
    assert_eq!(h.eval_all("engine.latency-ms"), Value::Number(latency));
    assert_eq!(h.eval_all("selection.tracks"), h.eval_all("(tracks)"));
    // Playing: the track playhead follows.
    h.shared.state.transport.track_playheads[0].store(5, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    h.eval_all("(def ph #'t0.playhead)");
    h.sync();
    assert_eq!(h.slot("ph"), 5.0);
}

#[test]
fn new_set_paths_change_the_model() {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.share_buses_and_groups();
    h.sync();
    let fx_index = h.app.buses.iter().position(|bus| bus.id == fx).unwrap();
    h.eval_all(&format!(
        "(def t0 (track 0)) (def s3 (nth t0.steps 3)) (def fx (nth (buses) {fx_index}))
         (def g (first (groups)))
         (def fx-send (first (filter (lambda (s) (= s.bus fx)) t0.sends)))"
    ));
    // Track pan, solo, collapse.
    h.eval_all("(set! t0.pan -0.25) (set! t0.soloed true) (set! t0.soloed true)");
    h.eval_all("(set! t0.collapsed true)");
    h.drain();
    let tp = &h.shared.state.pattern.track_params[0];
    assert!((tp.get_pan() + 0.25).abs() < 1e-6);
    assert!(tp.is_solo(), "two absolute sets solo once");
    assert_eq!(
        h.shared.track_collapsed.lock().unwrap().first(),
        Some(&true)
    );
    h.sync();
    assert_eq!(h.eval_all("t0.soloed"), Value::Bool(true));
    assert_eq!(h.eval_all("t0.collapsed"), Value::Bool(true));
    h.eval_all("(set! t0.soloed false)");
    h.drain();
    assert!(!h.shared.state.pattern.track_params[0].is_solo());
    // A step parameter, on a track other than the current one too.
    h.eval_all("(set! s3.velocity 0.3) (set! s3.duration 2)");
    h.drain();
    let data = &h.shared.state.pattern.step_data[0];
    assert!((data.get(3, StepParam::Velocity) - 0.3).abs() < 1e-6);
    assert_eq!(data.get(3, StepParam::Duration), 2.0);
    assert!((num(h.eval_all("s3.velocity")) - 0.3).abs() < 1e-6);
    // Send amount (no step selected: the track's own level).
    h.eval_all("(set! fx-send.amount 0.6)");
    h.drain();
    let amount = h.shared.state.pattern.track_params[0]
        .sends()
        .iter()
        .find(|send| send.destination == fx)
        .map(|send| send.amount);
    assert!(
        amount.is_some_and(|amount| (amount - 0.6).abs() < 1e-6),
        "{amount:?}"
    );
    assert!((num(h.eval_all("fx-send.amount")) - 0.6).abs() < 1e-6);
    // Bus volume, mute and solo.
    h.eval_all(
        "(set! fx.volume 0.4) (set! fx.muted true) (set! fx.muted true) (set! fx.soloed true)",
    );
    h.drain();
    h.share_buses_and_groups();
    let bus = &h.app.buses[fx_index];
    assert!((bus.volume - 0.4).abs() < 1e-6);
    assert!(bus.mute && bus.solo);
    h.sync();
    assert_eq!(h.eval_all("fx.muted"), Value::Bool(true));
    assert_eq!(h.eval_all("fx.soloed"), Value::Bool(true));
    h.eval_all("(set! fx.muted false)");
    h.drain();
    assert!(!h.app.buses[fx_index].mute);
    // Group collapse lands in the shared groups the tick pulls from.
    h.eval_all("(set! g.collapsed true) (set! g.collapsed true)");
    assert!(h.shared.track_groups.lock().unwrap()[0].collapsed);
    h.app.groups = h.shared.track_groups.lock().unwrap().clone();
    h.sync();
    assert_eq!(h.eval_all("g.collapsed"), Value::Bool(true));
    // Transport.
    h.eval_all(
        "(set! transport.bpm 141) (set! transport.metronome true) (set! transport.metronome true)
         (set! transport.roll-mode true) (set! transport.record-quantize \"1/8\")",
    );
    h.drain();
    let transport = &h.shared.state.transport;
    assert_eq!(transport.bpm.load(Ordering::Relaxed), 141);
    assert!(transport.metronome_enabled.load(Ordering::Relaxed));
    assert!(transport.roll_mode.load(Ordering::Relaxed));
    assert_eq!(h.eval_all("transport.record-quantize"), s("1/8"));
    h.eval_all("(set! transport.roll-mode false)");
    h.drain();
    assert!(!h.shared.state.transport.roll_mode.load(Ordering::Relaxed));
    // Read-only stays read-only.
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_ALL} (set! master.peak-l 1)"))
        .expect_err("read-only");
    assert!(
        format!("{error:?}").contains("master.peak-l is read-only"),
        "{error:?}"
    );
}

#[test]
fn bus_group_and_send_identity_survives_add_remove_reorder_and_project_load() {
    let mut h = Harness::new();
    let a = h.add_bus("A");
    let b = h.add_bus("B");
    h.sync();
    let index = |h: &Harness, id| h.app.buses.iter().position(|bus| bus.id == id).unwrap() as u64;
    let bus_a = h.instance_of(BUS, &[index(&h, a)]);
    let bus_b = h.instance_of(BUS, &[index(&h, b)]);
    let t0 = h.track_id(0);
    let send_b = h.instance_of(SEND, &[t0, b.0]);
    // Reorder: the instances re-key, the ids stay.
    let (from, to) = (index(&h, b) as usize, index(&h, a) as usize);
    h.app.reorder_bus_recorded(from, to).expect("reorder");
    h.sync();
    assert_eq!(h.instance_of(BUS, &[index(&h, a)]), bus_a);
    assert_eq!(h.instance_of(BUS, &[index(&h, b)]), bus_b);
    assert_eq!(
        h.rt().instance_field(bus_b, "index"),
        Ok(Value::Number(index(&h, b) as f64))
    );
    // Delete A: its instance and the sends to it go; B's stay.
    let send_a = h.instance_of(SEND, &[t0, a.0]);
    h.app.delete_bus_recorded(a).expect("delete");
    h.sync();
    assert!(!h.rt().instance_is_live(bus_a));
    assert!(!h.rt().instance_is_live(send_a));
    assert!(h.rt().instance_is_live(send_b));
    assert_eq!(
        h.rt().instance_field(send_b, "bus"),
        Ok(Value::Instance(bus_b))
    );
    // A group appears and goes with its edits.
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.sync();
    let group = h.instance_of(GROUP, &[0]);
    assert_eq!(
        h.rt().instance_field(t0, "group"),
        Ok(Value::Instance(group))
    );
    let group_id = h.app.groups[0].id;
    h.app.ungroup_tracks_recorded(group_id).expect("ungroup");
    h.sync();
    assert!(!h.rt().instance_is_live(group));
    assert_eq!(h.rt().instance_field(t0, "group"), Ok(Value::Nil));
    // A project load replaces buses, groups and sends.
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(bus_b));
    assert!(!h.rt().instance_is_live(send_b));
    assert_eq!(
        h.eval_all("(len (buses))"),
        Value::Number(h.app.buses.len() as f64)
    );
}

#[test]
fn new_live_fields_are_computed_only_while_observed() {
    let mut h = Harness::new();
    h.add_bus("FX");
    h.sync();
    h.eval_all("(def t0 (track 0)) (len t0.steps) (def fx (nth (buses) (- (len (buses)) 1)))");
    h.meters.cached_bus_peak_levels = vec![0.5; h.app.buses.len()];
    h.meters.cached_peak_l_level = 0.5;
    for step in 0..6 {
        h.shared
            .state
            .transport
            .playhead
            .store(step, Ordering::Relaxed);
        h.shared.state.pattern.step_data[0].set(1, StepParam::Velocity, step as f32 / 10.0);
        h.sync();
    }
    for key in [
        f::STEP_VELOCITY,
        f::STEP_HELD,
        f::TRANSPORT_POSITION,
        f::BUS_PEAK,
        f::MASTER_PEAK_L,
        f::ENGINE_CPU_LOAD,
        f::SEND_AMOUNT,
        f::TRACK_PLAYHEAD,
    ] {
        assert_eq!(h.computed(key), 0, "{key:?} computed while unobserved");
    }
    assert!(!h.frame.host_kinds.wants_bus_peaks());
    assert!(!h.frame.host_kinds.wants_master_peaks());
    // Observing one step's velocity: that step follows, on change only.
    h.eval_all("(def s1 (nth t0.steps 1)) (def v1 #'s1.velocity)");
    let cold = h.computed(f::STEP_VELOCITY);
    assert_eq!(cold, 1, "the binding's seed asks the reader once");
    h.sync();
    h.shared.state.pattern.step_data[0].set(1, StepParam::Velocity, 0.9);
    h.sync();
    assert!((h.slot("v1") - 0.9).abs() < 1e-6);
    let after = h.computed(f::STEP_VELOCITY);
    h.sync();
    assert_eq!(h.computed(f::STEP_VELOCITY), after, "no change, no work");
    assert_eq!(h.computed(f::STEP_HELD), 0, "only the observed field");
    // Meters: observed peaks ask the tick to keep polling.
    h.eval_all("(def bp #'fx.peak) (def ml #'master.peak-l) (def pos #'transport.position)");
    h.sync();
    assert!(h.frame.host_kinds.wants_bus_peaks());
    assert!(h.frame.host_kinds.wants_master_peaks());
    assert_eq!(h.slot("bp"), 0.5);
    assert_eq!(h.slot("ml"), 0.5);
    h.shared
        .state
        .transport
        .playhead
        .store(40, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("pos"), 40.0);
    h.eval_all("(set! bp nil) (set! ml nil)");
    h.sync();
    h.sync();
    assert!(!h.frame.host_kinds.wants_bus_peaks());
    assert!(!h.frame.host_kinds.wants_master_peaks());
}

/// Two tracks, an "FX" bus, and per-track send instances bound as `s0`
/// (track 0, the current track) and `s1` (track 1).
fn send_harness() -> (Harness, sequencer::sequencer::BusId) {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    h.sync();
    let fx_index = h.app.buses.iter().position(|bus| bus.id == fx).unwrap();
    h.eval_all(&format!(
        "(def fx (nth (buses) {fx_index}))
         (def t0 (track 0)) (def t1 (track 1))
         (def s0 (first (filter (lambda (s) (= s.bus fx)) t0.sends)))
         (def s1 (first (filter (lambda (s) (= s.bus fx)) t1.sends)))"
    ));
    (h, fx)
}

fn base_send(h: &Harness, track: usize, bus: sequencer::sequencer::BusId) -> Option<f32> {
    h.shared.state.pattern.track_params[track].send_amount(bus)
}

fn send_lock(
    h: &Harness,
    track: usize,
    step: usize,
    bus: sequencer::sequencer::BusId,
) -> Option<f32> {
    h.shared.state.pattern.track_send_plocks[track].get(step, bus)
}

#[test]
fn send_amount_is_the_base_and_display_follows_the_lock_on_the_current_track() {
    let (mut h, fx) = send_harness();
    for track in 0..2 {
        h.shared.state.pattern.track_params[track].set_sends(vec![TrackSendSnapshot {
            destination: fx,
            amount: 0.25,
        }]);
        h.shared.state.pattern.track_send_plocks[track].set(2, fx, 0.75);
    }
    h.eval_all("(def d0 #'s0.display) (def l0 #'s0.locked)");
    h.sync();
    // Nothing selected and stopped: the base everywhere.
    assert_eq!(h.eval_all("s0.amount"), Value::Number(0.25));
    assert_eq!(h.slot("d0"), 0.25);
    assert_eq!(h.eval_all("s0.locked"), Value::Bool(false));
    // Selecting the locked step on the current track shows its lock.
    h.shared.selected_steps.lock().unwrap().insert(2);
    h.sync();
    assert_eq!(h.slot("d0"), 0.75);
    assert_eq!(h.eval_all("s0.locked"), Value::Bool(true));
    assert_eq!(
        h.eval_all("s0.amount"),
        Value::Number(0.25),
        "amount stays the base"
    );
    // Another track's send shows its own base (the selection is track 0's).
    assert_eq!(h.eval_all("s1.display"), Value::Number(0.25));
    assert_eq!(h.eval_all("s1.locked"), Value::Bool(false));
    // Deselecting restores the base.
    h.shared.selected_steps.lock().unwrap().clear();
    h.sync();
    assert_eq!(h.slot("d0"), 0.25);
    assert_eq!(h.eval_all("s0.locked"), Value::Bool(false));
}

#[test]
fn send_amount_set_never_p_locks_and_the_legacy_command_locks_only_the_current_track() {
    let (mut h, fx) = send_harness();
    h.shared.selected_steps.lock().unwrap().insert(3);
    // The kind setter: the base of either track, never a lock.
    h.eval_all("(set! s0.amount 0.4) (set! s1.amount 0.5)");
    h.drain();
    assert!(base_send(&h, 0, fx).is_some_and(|amount| (amount - 0.4).abs() < 1e-6));
    assert!(base_send(&h, 1, fx).is_some_and(|amount| (amount - 0.5).abs() < 1e-6));
    assert_eq!(send_lock(&h, 0, 3, fx), None);
    assert_eq!(send_lock(&h, 1, 3, fx), None);
    h.sync();
    assert!((num(h.eval_all("s1.amount")) - 0.5).abs() < 1e-6);
    // The mixer's `set-track-bus-send` on another track sets its base,
    // never a lock at the current track's selected step.
    let fx_index = h.app.buses.iter().position(|bus| bus.id == fx).unwrap();
    let send = |track: usize, amount: f64| {
        map_value([
            ("track", Value::Number(track as f64)),
            ("bus", Value::Number(fx_index as f64)),
            ("amount", Value::Number(amount)),
        ])
    };
    h.command("set-track-bus-send", send(1, 0.6));
    assert!(base_send(&h, 1, fx).is_some_and(|amount| (amount - 0.6).abs() < 1e-6));
    assert_eq!(send_lock(&h, 1, 3, fx), None);
    // On the current track it still locks the selected step.
    h.command("set-track-bus-send", send(0, 0.9));
    assert!(send_lock(&h, 0, 3, fx).is_some_and(|amount| (amount - 0.9).abs() < 1e-6));
    assert!(base_send(&h, 0, fx).is_some_and(|amount| (amount - 0.4).abs() < 1e-6));
}

#[test]
fn a_bus_reorder_between_set_and_apply_still_targets_the_set_bus() {
    let (mut h, fx) = send_harness();
    let other = h.add_bus("Other");
    h.sync();
    h.eval_all("(set! s0.amount 0.7)");
    // The command is queued; the buses swap places before it lands.
    let (from, to) = (
        h.app.buses.iter().position(|bus| bus.id == other).unwrap(),
        h.app.buses.iter().position(|bus| bus.id == fx).unwrap(),
    );
    h.app.reorder_bus_recorded(from, to).expect("reorder");
    h.share_buses_and_groups();
    h.drain();
    assert!(base_send(&h, 0, fx).is_some_and(|amount| (amount - 0.7).abs() < 1e-6));
    assert_eq!(base_send(&h, 0, other).unwrap_or(0.0), 0.0);
}

#[test]
fn buses_and_groups_keep_identity_while_a_loaded_registry_lags() {
    let mut h = Harness::new();
    h.sync();
    // A project load whose track list the registry has not caught up with
    // yet: the load replaces buses and groups once, then the track model
    // retries every tick without re-registering them.
    h.command("new-project", Value::Nil);
    h.add_bus("FX");
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.app.tracks.push("Pending".to_string());
    h.sync();
    let bus = h.instance_of(BUS, &[1]);
    let syncs = h.model_syncs();
    for _ in 0..3 {
        h.sync();
        assert!(h.rt().instance_is_live(bus), "the bus is not re-registered");
        assert_eq!(h.instance_of(BUS, &[1]), bus);
    }
    assert_eq!(
        h.model_syncs(),
        syncs + 3,
        "the model sync retries every tick"
    );
    h.app.tracks.pop();
    h.sync();
    assert_eq!(h.instance_of(BUS, &[1]), bus);
    let group = h.instance_of(GROUP, &[0]);
    let syncs = h.model_syncs();
    h.sync();
    assert_eq!(h.model_syncs(), syncs, "caught up: no more retries");
    assert_eq!(h.instance_of(BUS, &[1]), bus);
    assert_eq!(h.instance_of(GROUP, &[0]), group);
}

#[test]
fn non_distinct_bus_or_group_ids_retry_the_model_sync() {
    let mut h = Harness::new();
    h.add_bus("FX");
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.sync();
    let syncs = h.model_syncs();
    h.sync();
    assert_eq!(h.model_syncs(), syncs);
    // A duplicated group id: retried every tick until it is gone.
    let duplicate = h.app.groups[0].clone();
    h.app.groups.push(duplicate);
    h.sync();
    h.sync();
    assert_eq!(h.model_syncs(), syncs + 2);
    h.app.groups.pop();
    h.sync();
    h.sync();
    assert_eq!(h.model_syncs(), syncs + 3);
    // Likewise a duplicated bus id.
    let duplicate = h.app.buses[1].clone();
    h.app.buses.push(duplicate);
    h.sync();
    h.sync();
    assert_eq!(h.model_syncs(), syncs + 5);
    h.app.buses.pop();
    h.sync();
    h.sync();
    assert_eq!(h.model_syncs(), syncs + 6);
}

impl Harness {
    /// `code` (a setter) is refused under the value rule: an error naming
    /// `expected` reaches the status line (a native's) or the minibuffer
    /// (a host command's), and no undo entry is recorded.
    fn refuses_value(&mut self, code: &str, expected: &str) {
        self.editor.minibuffer = None;
        self.editor.runtime_mut().take_status_message();
        let before = self.app.history.undo_len();
        self.eval_all(code);
        let status = self.editor.runtime_mut().take_status_message();
        self.drain();
        let shown = status
            .or_else(|| self.editor.minibuffer.clone())
            .unwrap_or_default();
        assert!(shown.contains(expected), "{code}: {shown:?}");
        assert_eq!(
            self.app.history.undo_len(),
            before,
            "{code} recorded nothing"
        );
    }
}

/// eseq-0l17.38: the stage 1–7 number setters follow the value rule
/// (§14.2c): out of range is an error that changes nothing (no silent
/// clamping), the range's ends work, and the current value round-trips
/// with no undo entry.
#[test]
fn earlier_number_setters_follow_the_value_rule() {
    let mut h = Harness::new();
    let fx = h.add_bus("FX");
    h.share_buses_and_groups();
    h.sync();
    let fx_index = h.app.buses.iter().position(|bus| bus.id == fx).unwrap();
    h.eval_all(&format!(
        "(def t0 (track 0)) (def s3 (nth t0.steps 3)) (def fx (nth (buses) {fx_index}))
         (def fx-send (first (filter (lambda (s) (= s.bus fx)) t0.sends)))"
    ));
    h.eval_all(
        "(set! t0.volume 0.5) (set! t0.pan 0.25) (set! fx.volume 0.5) (set! fx-send.amount 0.5)
         (set! transport.bpm 130) (set! s3.velocity 0.5)",
    );
    h.drain();
    h.share_buses_and_groups();
    h.sync();
    let model = |h: &Harness| {
        let tp = &h.shared.state.pattern.track_params[0];
        let send = (tp.sends().iter())
            .find(|send| send.destination == fx)
            .map(|send| send.amount);
        (
            tp.get_volume(),
            tp.get_pan(),
            h.app.buses[fx_index].volume,
            send,
            h.shared.state.transport.bpm.load(Ordering::Relaxed),
            h.shared.state.pattern.step_data[0].get(3, StepParam::Velocity),
        )
    };
    let before = model(&h);
    assert_eq!(before, (0.5, 0.25, 0.5, Some(0.5), 130, 0.5));
    for (code, expected) in [
        (
            "(set! t0.volume 1.5)",
            "seq-set-track-volume: 1.5 is not a number from 0 to 1",
        ),
        ("(set! t0.volume -0.1)", "seq-set-track-volume"),
        (
            "(set! t0.pan 2)",
            "seq-set-track-pan: 2 is not a number from -1 to 1",
        ),
        ("(set! fx.volume 1.01)", "seq-set-bus-volume"),
        (
            "(set! fx-send.amount -1)",
            "set-track-send-base: -1 is not a number from 0 to 1",
        ),
        (
            "(set! transport.bpm 301)",
            "seq-set-bpm: 301 is not a number from 20 to 300",
        ),
        ("(set! transport.bpm 19)", "seq-set-bpm"),
        (
            "(set! s3.velocity 5)",
            "seq-set-track-step-param :velocity: 5 is not a number from 0 to 1",
        ),
        ("(set! s3.transpose 49)", "from -48 to 48"),
    ] {
        h.refuses_value(code, expected);
        h.share_buses_and_groups();
        assert_eq!(model(&h), before, "{code} changed nothing");
    }
    // The current values round-trip and record nothing.
    let undo = h.app.history.undo_len();
    h.eval_all(
        "(set! t0.volume t0.volume) (set! t0.pan t0.pan) (set! fx.volume fx.volume)
         (set! fx-send.amount fx-send.amount) (set! transport.bpm transport.bpm)
         (set! s3.velocity s3.velocity)",
    );
    h.drain();
    h.share_buses_and_groups();
    assert_eq!(model(&h), before);
    assert_eq!(
        h.app.history.undo_len(),
        undo,
        "a round trip records nothing"
    );
    // The ends of each range are values.
    h.eval_all(
        "(set! t0.volume 1) (set! t0.pan -1) (set! fx.volume 0) (set! fx-send.amount 1)
         (set! transport.bpm 300) (set! s3.velocity 0)",
    );
    h.drain();
    h.share_buses_and_groups();
    assert_eq!(model(&h), (1.0, -1.0, 0.0, Some(1.0), 300, 0.0));
}

/// A value no meter read returns: a cache still holding it was not polled.
const UNPOLLED: f64 = 9.0;

/// Fill every meter cache with [`UNPOLLED`].
fn seed_meters(h: &mut Harness) {
    let (tracks, buses) = (h.app.tracks.len(), h.app.buses.len());
    let meters = &mut h.meters;
    meters.cached_peak_l_level = UNPOLLED;
    meters.cached_peak_r_level = UNPOLLED;
    meters.cached_track_peak_levels = vec![UNPOLLED; tracks];
    meters.cached_bus_peak_levels = vec![UNPOLLED; buses];
    meters.cached_modulator_phases = vec![UNPOLLED; tracks];
    meters.cached_modulator_levels = vec![UNPOLLED; tracks];
    meters.cached_mod_port_levels.track_outputs = vec![UNPOLLED; tracks];
}

/// Which meter caches the tick rewrote since [`seed_meters`].
fn polled_meters(h: &Harness) -> MeterDemand {
    let meters = &h.meters;
    MeterDemand {
        master: meters.cached_peak_l_level != UNPOLLED,
        tracks: meters.cached_track_peak_levels[0] != UNPOLLED,
        buses: meters.cached_bus_peak_levels[0] != UNPOLLED,
        modulators: meters.cached_modulator_phases[0] != UNPOLLED,
        mod_levels: meters.cached_mod_port_levels.track_outputs[0] != UNPOLLED,
    }
}

/// Make the meter cadence due (`true`) or never due within the test.
fn meter_cadence_due(h: &mut Harness, due: bool) {
    h.meters.last_meter_poll_at = if due {
        Instant::now() - METER_POLL_INTERVAL * 2
    } else {
        Instant::now() + Duration::from_secs(3600)
    };
}

/// The tick reads a meter cache only while a kind field observes it
/// (docs/kind-bindings-spec.md D3, eseq-0l17.79): with nothing observed it
/// polls nothing, due or not; a newly observed meter samples at once, then
/// at the meter cadence; a released one stops.
#[test]
fn the_tick_polls_each_meter_only_while_a_kind_field_observes_it() {
    let mut h = Harness::new();
    h.add_bus("FX");
    h.app
        .graph_controller()
        .add_modulator_track()
        .expect("modulator track");
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.tick();
    h.tick();
    let none = MeterDemand::default();
    // Nothing observed: no meter is read, though the cadence is due.
    seed_meters(&mut h);
    meter_cadence_due(&mut h, true);
    h.tick();
    h.tick();
    assert_eq!(polled_meters(&h), none, "nothing observed, nothing polled");
    assert_eq!(h.frame.prev_meter_demand, none);
    // One track's peak: its cache samples once observed (the sync that
    // first pushes the field sets the demand the next tick polls on),
    // without waiting for the cadence.
    h.eval_all(
        "(def t0 (track 0)) (def fx (nth (buses) (- (len (buses)) 1))) \
         (def lfo (let ((t (track 2))) (first t.devices))) (def p0 #'t0.peak)",
    );
    seed_meters(&mut h);
    meter_cadence_due(&mut h, false);
    h.tick();
    h.tick();
    let tracks = MeterDemand { tracks: true, ..none };
    assert_eq!(polled_meters(&h), tracks, "a newly observed meter polls at once");
    // Then at the cadence only.
    seed_meters(&mut h);
    h.tick();
    assert_eq!(polled_meters(&h), none, "the cadence is not due");
    meter_cadence_due(&mut h, true);
    h.tick();
    assert_eq!(polled_meters(&h), tracks, "the cadence polls the observed meter");
    // Every meter observed.
    h.eval_all(
        "(def bp #'fx.peak) (def ml #'master.peak-l) (def mo #'t0.mod-out-level) \
         (def lp #'lfo.modulator-phase)",
    );
    seed_meters(&mut h);
    meter_cadence_due(&mut h, false);
    h.tick();
    h.tick();
    let all = MeterDemand {
        master: true,
        tracks: true,
        buses: true,
        modulators: true,
        mod_levels: true,
    };
    assert_eq!(h.frame.prev_meter_demand, all);
    assert_eq!(
        polled_meters(&h),
        MeterDemand { tracks: false, ..all },
        "the newly observed ones poll at once"
    );
    // Released: polling stops, and the released peaks and levels fall to
    // silence (a reopened meter never flashes an old peak).
    seed_meters(&mut h);
    h.eval_all("(set! p0 nil) (set! bp nil) (set! ml nil) (set! mo nil) (set! lp nil)");
    h.tick();
    h.tick();
    assert_eq!(h.frame.prev_meter_demand, none);
    assert_eq!(h.meters.cached_peak_l_level, 0.0);
    assert!(h.meters.cached_track_peak_levels.iter().all(|level| *level == 0.0));
    assert!(h.meters.cached_bus_peak_levels.iter().all(|level| *level == 0.0));
    assert!(h.meters.cached_mod_port_levels.track_outputs.iter().all(|level| *level == 0.0));
    seed_meters(&mut h);
    meter_cadence_due(&mut h, true);
    h.tick();
    assert_eq!(polled_meters(&h), none, "released, nothing polled");
}
