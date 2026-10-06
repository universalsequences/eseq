//! Stage 7c: process lanes (processes, lanes, inlets, ports, fan-out
//! entries, state cells, the library), their setters, identity and
//! observed gating.

use super::*;

const REFER_LANES: &str =
    "(import eseq.kinds :refer (track tracks process-library selection project \
                           set-process-enabled! set-inlet! set-lane-steps! move-process! \
                           add-process! remove-process! bind-port! add-fanout! unbind-port! \
                           clear-port! remove-fanout!))";

/// Track 0's default lanes, by name, and some of their parts.
const LANES: &str = r#"(def t0 (track 0)) (def t1 (track 1))
    (def by-name (lambda (t name) (first (filter (lambda (p) (= p.name name)) t.processes))))
    (def part (lambda (xs name) (first (filter (lambda (x) (= x.name name)) xs))))
    (def prob (by-name t0 "prob")) (def rnd (by-name t0 "rand")) (def tacc (by-name t0 "tacc"))
    (def grab (by-name t0 "grab")) (def reset (by-name t0 "reset")) (def cmpa (by-name t0 "cmp A"))
    (def cnt (by-name t0 "count"))
    (def prob-lane (first prob.lanes)) (def lo (part rnd.inlets "lo"))
    (def which (part grab.inlets "value")) (def tacc-reset (part tacc.inlets "reset"))
    (def out (part rnd.ports "out")) (def wire (part rnd.ports "wire"))
    (def reset-wire (part reset.ports "wire"))"#;

impl Harness {
    /// A fresh project with the process library published (the event loop
    /// republishes it after a project switch; the harness has no UI
    /// authoring registry to do so).
    fn with_library() -> Self {
        let mut h = Self::new();
        h.publish_library();
        h
    }

    pub(super) fn publish_library(&mut self) {
        let library = sequencer::lisp_host::load_process_library_source();
        self.editor
            .runtime_mut()
            .eval_str(&library)
            .expect("builtin process library");
    }

    fn eval_lanes(&mut self, code: &str) -> Value {
        let source = format!("{REFER_LANES}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    fn lanes_list(&mut self, code: &str) -> Vec<Value> {
        items(&self.eval_lanes(code))
    }

    fn lane_syncs(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().lanes.syncs
    }

    fn lane_undo(&mut self) {
        self.undo();
        self.sync();
    }

    fn lane_drain(&mut self) {
        self.drain();
        self.sync();
    }

    /// `code`'s command fails with `message` and records nothing.
    fn lane_rejects(&mut self, code: &str, message: &str) {
        let before = self.app.history.undo_len();
        self.editor.minibuffer = None;
        self.eval_lanes(code);
        self.lane_drain();
        let error = self.editor.minibuffer.clone().unwrap_or_default();
        assert!(error.contains(message), "{code}: {error}");
        assert_eq!(self.app.history.undo_len(), before, "{code}: no entry");
    }

    /// Track `track`'s composed slot named `name`.
    fn slot_named(&self, track: usize, name: &str) -> sequencer::process::TrackProcessSlot {
        let chain = self
            .shared
            .state
            .composed_track_process_chain(track)
            .unwrap();
        chain
            .slots
            .into_iter()
            .find(|slot| slot.instance_name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no slot {name}"))
    }

    fn lane_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_lanes(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }
}

/// A legacy optional string (nil) as the kinds show it ("").
fn or_empty(value: Value) -> Value {
    match value {
        Value::Nil => s(""),
        other => other,
    }
}

fn list(values: impl IntoIterator<Item = Value>) -> Value {
    list_value(values)
}

fn fields_of(entry: &Value, keys: &[&str]) -> Vec<Value> {
    keys.iter().map(|key| get(entry, key)).collect()
}

#[test]
fn lanes_processes_and_the_library_read_like_the_legacy_fields() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    let state = h.shared.state.clone();
    // Lanes (SEQ.track-process-lanes, SEQ.track-process-lane-values).
    let legacy = items(&build_process_lanes_value(&state, 0));
    assert!(legacy.len() >= 14, "the default lanes: {}", legacy.len());
    let values = items(&items(&build_all_track_process_lane_values(&state, 2))[0]);
    let lanes = h.lanes_list(
        "(map (lambda (l) (list l.position l.inlet l.label l.short-label l.type l.min l.max \
                              l.default l.decimals l.forked l.process.proc-id l.process.index \
                              (= l.track t0) l.values))
              t0.lanes)",
    );
    assert_eq!(lanes.len(), legacy.len());
    for ((lane, entry), values) in lanes.iter().zip(&legacy).zip(&values) {
        let mut expected = fields_of(
            entry,
            &[
                "lane-index",
                "inlet",
                "label",
                "short-label",
                "kind",
                "min",
                "max",
                "default",
                "decimals",
                "forked",
                "instance-id",
                "slot-index",
            ],
        );
        expected.push(Value::Bool(true));
        expected.push(values.clone());
        assert_eq!(*lane, list(expected), "lane {entry:?}");
    }
    // The current track's lanes (SEQ.process-lanes) are selection.track's.
    assert_eq!(
        h.eval_lanes("(map (lambda (l) l.label) selection.track.lanes)"),
        list(legacy.iter().map(|entry| get(entry, "label")))
    );

    // Processes (SEQ.track-process-slots, SEQ.process-slots).
    let slots = items(&build_process_slots_value(&state, 0));
    let processes = h.lanes_list(
        "(map (lambda (p) (list p.index p.proc-id p.class-name p.enabled p.project p.default-lane \
                              p.instance-name p.doc p.source-path p.target p.class.name))
              t0.processes)",
    );
    assert_eq!(processes.len(), slots.len());
    for (process, slot) in processes.iter().zip(&slots) {
        let mut expected = fields_of(
            slot,
            &[
                "slot-index",
                "instance-id",
                "class",
                "enabled",
                "project",
                "default-lane",
            ],
        );
        expected.push(or_empty(get(slot, "instance-name")));
        expected.extend(fields_of(slot, &["doc", "source-path", "target", "class"]));
        assert_eq!(*process, list(expected), "process {slot:?}");
    }
    // Inlets and ports, each in the slot's order.
    for (index, slot) in slots.iter().enumerate() {
        let inlets = h.lanes_list(&format!(
            "(let ((p (nth t0.processes {index})))
               (map (lambda (i) (list i.name i.type i.options i.value i.default i.min i.max
                                      i.decimals i.doc (= i.process p)))
                    p.inlets))"
        ));
        let legacy_inlets = items(&get(slot, "inlets"));
        assert_eq!(inlets.len(), legacy_inlets.len());
        for (inlet, entry) in inlets.iter().zip(&legacy_inlets) {
            let mut expected = fields_of(
                entry,
                &[
                    "name", "kind", "options", "value", "default", "min", "max", "decimals", "doc",
                ],
            );
            expected.push(Value::Bool(true));
            assert_eq!(*inlet, list(expected), "inlet {entry:?}");
        }
        let ports = h.lanes_list(&format!(
            "(map (lambda (pt) (list pt.name pt.label pt.hint pt.target pt.status pt.manual
                                     (or pt.manual pt.disconnected) pt.mappable pt.connectable
                                     pt.bindable pt.target-kind pt.target-step-param
                                     (map (lambda (fo) (list fo.index fo.target fo.lo fo.hi))
                                          pt.fanout)))
                  (let ((p (nth t0.processes {index}))) p.ports))"
        ));
        let legacy_ports = items(&get(slot, "ports"));
        assert_eq!(ports.len(), legacy_ports.len());
        for (port, entry) in ports.iter().zip(&legacy_ports) {
            let mut expected = fields_of(
                entry,
                &[
                    "name",
                    "label",
                    "hint",
                    "target",
                    "status",
                    "manual",
                    "clearable",
                    "mappable",
                    "connectable",
                    "bindable",
                    "target-kind",
                ],
            );
            expected.push(or_empty(get(entry, "target-step-param")));
            let fanout = items(&get(entry, "fanout"))
                .into_iter()
                .map(|fo| list(fields_of(&fo, &["index", "target", "lo", "hi"])));
            expected.push(list(fanout));
            assert_eq!(*port, list(expected), "port {entry:?}");
        }
    }
    // The patchbay (SEQ.track-lane-patch): in ports, and every reader of a
    // connectable port as the process it wires into.
    let patch = items(&build_track_lane_patch_value(&state, 0));
    assert_eq!(patch.len(), slots.len());
    for (index, entry) in patch.iter().enumerate() {
        let in_ports = items(&get(entry, "in-ports"))
            .iter()
            .map(|port| get(port, "name"))
            .collect::<Vec<_>>();
        assert_eq!(
            h.eval_lanes(&format!(
                "(let ((p (nth t0.processes {index}))) p.in-ports)"
            )),
            list(in_ports)
        );
        for out in items(&get(entry, "out-ports")) {
            let Value::String(name) = get(&out, "name") else {
                panic!("a port name")
            };
            let wired = h.lanes_list(&format!(
                "(let ((pt (part (let ((p (nth t0.processes {index}))) p.ports) \"{name}\")))
                   (append (if pt.target-process
                             (list (list pt.target-process.index pt.target-inlet))
                             '())
                           (map (lambda (fo) (list fo.target-process.index fo.target-inlet))
                                (filter (lambda (fo) fo.target-process) pt.fanout))))"
            ));
            let expected: Vec<Value> = items(&get(&out, "readers"))
                .iter()
                .map(|reader| list(fields_of(reader, &["slot-index", "inlet"])))
                .collect();
            assert_eq!(wired, expected, "readers of {name} on slot {index}");
        }
    }
    // reset's wire drives tacc, and through its fan-out acc A and acc B.
    assert_eq!(
        h.eval_lanes(
            "(list reset-wire.target-process.name \
                   (map (lambda (fo) fo.target-process.name) reset-wire.fanout))"
        ),
        h.eval_lanes(r#"(list "tacc" (list "acc A" "acc B"))"#)
    );
    // State cells: the class's, by name.
    assert_eq!(
        h.eval_lanes("(map (lambda (c) c.name) rnd.cells)"),
        h.eval_lanes(r#"(list "held")"#)
    );
    // The library (SEQ.process-library).
    let library = items(&build_process_library_value(&state));
    let classes = h.lanes_list(
        "(map (lambda (c) (list c.index c.name c.doc c.source-path c.target c.lane-count c.ports))
              process-library.classes)",
    );
    assert_eq!(classes.len(), library.len());
    for (index, (class, entry)) in classes.iter().zip(&library).enumerate() {
        let ports = items(&get(entry, "ports"))
            .iter()
            .map(|port| get(port, "name"))
            .collect::<Vec<_>>();
        let mut expected = vec![number(index as f64)];
        expected.extend(fields_of(
            entry,
            &["name", "doc", "source-path", "target", "lane-count"],
        ));
        expected.push(list(ports));
        assert_eq!(*class, list(expected));
    }
    // Every track has its own instances of the project lanes.
    assert_eq!(
        h.eval_lanes(
            "(list (len t1.processes) (= (first t1.processes) prob) \
                   (let ((p (first t1.processes))) (= p.proc-id prob.proc-id)))"
        ),
        h.eval_lanes(&format!("(list {} false true)", slots.len()))
    );
}

#[test]
fn process_setters_go_through_history_and_follow_the_value_rule() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    let entries = h.app.history.undo_len();

    // enabled: this track's fork; again is a no-op; undo restores.
    h.eval_lanes("(set! prob.enabled false)");
    h.lane_drain();
    assert!(!h.slot_named(0, "prob").enabled);
    assert!(
        h.slot_named(1, "prob").enabled,
        "track 1 keeps the shared slot"
    );
    assert_eq!(h.eval_lanes("prob.enabled"), Value::Bool(false));
    assert_eq!(h.app.history.undo_len(), entries + 1);
    h.eval_lanes("(set! prob.enabled prob.enabled)");
    h.lane_drain();
    assert_eq!(
        h.app.history.undo_len(),
        entries + 1,
        "the same value: no entry"
    );
    h.lane_undo();
    assert!(h.slot_named(0, "prob").enabled);
    assert_eq!(h.eval_lanes("prob.enabled"), Value::Bool(true));
    // :all sets the shared slot, for every track.
    h.eval_lanes("(set-process-enabled! prob false :all true)");
    h.lane_drain();
    assert!(!h.slot_named(0, "prob").enabled && !h.slot_named(1, "prob").enabled);
    assert_eq!(
        h.eval_lanes("(list prob.enabled (let ((p (first t1.processes))) p.enabled))"),
        h.eval_lanes("(list false false)")
    );
    h.lane_undo();
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_LANES} {LANES} (set! prob.enabled 1)"))
        .expect_err("a bool field");
    assert!(format!("{error:?}").contains(":bool"), "{error:?}");

    // An inlet: in its declared range; the current value round-trips.
    h.eval_lanes("(set! lo.value 3)");
    h.lane_drain();
    let lo = |h: &Harness| h.slot_named(0, "rand").inlets.get("lo").cloned();
    assert_eq!(
        lo(&h),
        Some(sequencer::process::ProcessLiteral::Number(3.0))
    );
    assert_eq!(h.eval_lanes("lo.value"), number(3.0));
    let before = h.app.history.undo_len();
    h.eval_lanes("(set! lo.value lo.value)");
    h.lane_drain();
    assert_eq!(h.app.history.undo_len(), before);
    h.lane_rejects("(set! lo.value 500)", "a number from -128 to 128");
    // An enum inlet takes an option index; a gate a bool or 0/1.
    h.eval_lanes("(set! which.value 3)");
    h.lane_drain();
    assert_eq!(h.eval_lanes("which.value"), number(3.0));
    assert_eq!(
        h.eval_lanes("which.options"),
        h.eval_lanes(r#"(list "note" "vel" "dur" "note+b")"#)
    );
    h.lane_rejects("(set! which.value 4)", "an integer from 0 to 3");
    h.lane_rejects("(set! which.value 1.5)", "an integer from 0 to 3");
    h.eval_lanes("(set-inlet! tacc-reset true)");
    h.lane_drain();
    assert_eq!(h.eval_lanes("tacc-reset.value"), number(1.0));
    // :all writes the shared slot.
    h.eval_lanes("(set-inlet! lo -5 :all true)");
    h.lane_drain();
    let shared_lo = h.slot_named(1, "rand").inlets.get("lo").cloned();
    assert_eq!(
        shared_lo,
        Some(sequencer::process::ProcessLiteral::Number(-5.0))
    );

    // Lane steps: the differing steps, one entry; undo restores.
    let before = h.app.history.undo_len();
    h.eval_lanes("(set-lane-steps! prob-lane (list (nth t0.steps 0) (nth t0.steps 2)) 0.25)");
    h.lane_drain();
    assert_eq!(h.app.history.undo_len(), before + 1);
    let values = h.lanes_list(
        "(list (nth prob-lane.values 0) (nth prob-lane.values 1) (nth prob-lane.values 2))",
    );
    // Step 1 keeps reading the lane's default (1): a lane write pads the
    // steps before it with the default, not 0 (eseq-gk0h).
    assert_eq!(values, vec![number(0.25), number(1.0), number(0.25)]);
    assert_eq!(h.eval_lanes("prob-lane.forked"), Value::Bool(true));
    let legacy = items(&items(&build_all_track_process_lane_values(&h.shared.state, 2))[0]);
    assert_eq!(h.eval_lanes("prob-lane.values"), legacy[0]);
    h.eval_lanes("(set-lane-steps! prob-lane (list (nth t0.steps 0)) 0.25)");
    h.lane_drain();
    assert_eq!(
        h.app.history.undo_len(),
        before + 1,
        "already 0.25: no entry"
    );
    h.lane_rejects(
        "(set-lane-steps! prob-lane (list (nth t0.steps 0)) 2)",
        "a number from 0 to 1",
    );
    h.lane_rejects(
        "(set-lane-steps! prob-lane (list (nth t1.steps 0)) 0.5)",
        "steps of the lane's track",
    );
    h.lane_undo();
    assert_eq!(h.eval_lanes("(nth prob-lane.values 0)"), number(1.0));

    // A fan-out entry's range.
    let lo_was = h.eval_lanes("(let ((fo (first reset-wire.fanout))) fo.lo)");
    h.eval_lanes("(let ((fo (first reset-wire.fanout))) (set! fo.lo -2))");
    h.lane_drain();
    assert_eq!(h.slot_named(0, "reset").fanout["wire"][0].lo, -2.0);
    assert_eq!(
        h.eval_lanes("(let ((fo (first reset-wire.fanout))) fo.lo)"),
        number(-2.0)
    );
    h.lane_undo();
    assert_eq!(
        h.eval_lanes("(let ((fo (first reset-wire.fanout))) fo.lo)"),
        lo_was
    );

    // Ports: a step param, then clear, then disconnect.
    h.eval_lanes(r#"(bind-port! out "velocity")"#);
    h.lane_drain();
    assert_eq!(
        h.eval_lanes("(list out.status out.manual out.target-step-param)"),
        h.eval_lanes(r#"(list "bound" true "velocity")"#)
    );
    let before = h.app.history.undo_len();
    h.eval_lanes(r#"(bind-port! out "velocity")"#);
    h.lane_drain();
    assert_eq!(h.app.history.undo_len(), before, "bound already: no entry");
    h.eval_lanes("(clear-port! out)");
    h.lane_drain();
    assert_eq!(
        h.eval_lanes("(list out.manual out.target-step-param)"),
        h.eval_lanes(r#"(list false "")"#)
    );
    h.eval_lanes("(unbind-port! out)");
    h.lane_drain();
    assert_eq!(
        h.eval_lanes("(list out.disconnected out.status)"),
        h.eval_lanes(r#"(list true "unbound")"#)
    );
    // A wire into another lane, then a fan-out into a third.
    h.eval_lanes("(bind-port! wire (first cmpa.lanes))");
    h.lane_drain();
    assert_eq!(
        h.eval_lanes("(list (= wire.target-process cmpa) wire.target-inlet)"),
        h.eval_lanes(r#"(list true "a")"#)
    );
    h.eval_lanes("(add-fanout! wire (first cnt.lanes))");
    h.lane_drain();
    assert_eq!(
        h.eval_lanes(
            "(map (lambda (fo) (list (= fo.target-process cnt) fo.target-inlet)) wire.fanout)"
        ),
        h.eval_lanes(r#"(list (list true "step"))"#)
    );
    h.eval_lanes("(remove-fanout! (first wire.fanout))");
    h.lane_drain();
    assert_eq!(h.eval_lanes("(len wire.fanout)"), number(0.0));
    h.lane_rejects(
        "(bind-port! wire (first rnd.lanes))",
        "cannot wire into itself",
    );
    h.lane_rejects(r#"(bind-port! wire "velocity")"#, "port 'wire' takes");
}

#[test]
fn processes_keep_their_identity_across_moves_adds_removes_and_project_loads() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    let rand_id = h.lane_instance("rnd");
    // A move keeps the instance (its index moves) and its parts.
    h.eval_lanes("(def rnd-lo lo) (move-process! rnd prob)");
    h.lane_drain();
    assert_eq!(h.lane_instance("(first t0.processes)"), rand_id);
    assert_eq!(
        h.eval_lanes("(list rnd.index prob.index)"),
        h.eval_lanes("(list 0 1)")
    );
    assert_eq!(
        h.eval_lanes(r#"(= (part rnd.inlets "lo") rnd-lo)"#),
        Value::Bool(true)
    );
    h.lane_undo();
    assert_eq!(h.eval_lanes("rnd.index"), number(2.0));
    // An added lane (the track's own) comes last; removing it drops it.
    let count = num(h.eval_lanes("(len t0.processes)"));
    h.eval_lanes(
        r#"(add-process! t0 (first (filter (lambda (c) (= c.name "lane-grab"))
                                            process-library.classes)))"#,
    );
    h.lane_drain();
    h.eval_lanes("(def added (nth t0.processes (- (len t0.processes) 1)))");
    assert_eq!(h.eval_lanes("(len t0.processes)"), number(count + 1.0));
    assert_eq!(
        h.eval_lanes("(list added.roster added.project added.name added.class.name)"),
        h.eval_lanes(r#"(list true false "grab 2" "lane-grab")"#)
    );
    assert_eq!(
        h.eval_lanes("(len t1.processes)"),
        number(count),
        "track 1 is untouched"
    );
    // A wire across layers is refused.
    h.lane_rejects("(bind-port! wire (first added.lanes))", "within its layer");
    h.eval_lanes("(remove-process! added)");
    h.lane_drain();
    assert_eq!(h.eval_lanes("(len t0.processes)"), number(count));
    assert_eq!(
        h.eval_lanes("added.name"),
        s(""),
        "the removed process is stale"
    );
    // A track added in front re-keys the tracks; their processes stay.
    h.app.graph_controller().add_empty_track().expect("add");
    h.app
        .graph_controller()
        .move_appended_track_to(0)
        .expect("move");
    h.sync();
    assert_eq!(h.eval_lanes("t0.index"), number(1.0));
    assert_eq!(h.lane_instance("(nth t0.processes 2)"), rand_id);
    assert_eq!(h.eval_lanes("rnd.track"), h.eval_lanes("t0"));
    // A project load replaces them.
    h.command("new-project", Value::Nil);
    h.publish_library();
    h.sync();
    assert!(!h.rt().instance_is_live(rand_id));
    assert_eq!(h.eval_lanes("rnd.name"), s(""));
    h.eval_lanes(LANES);
    h.sync();
    assert!(h.lane_instance("rnd") != rand_id);
    assert_eq!(h.eval_lanes("rnd.name"), s("rand"));
}

#[test]
fn lanes_register_lazily_and_sync_only_what_moved() {
    let mut h = Harness::with_library();
    h.sync();
    // Nothing reads a track's processes: none is registered.
    assert_eq!(h.lane_syncs(), 0);
    assert!(h.frame.host_kinds.shared.borrow().lanes.tracks.is_empty());
    h.eval_lanes(LANES);
    h.sync();
    assert_eq!(
        h.frame.host_kinds.shared.borrow().lanes.tracks.len(),
        1,
        "only the track read"
    );
    // Idle ticks sync nothing; nor does a UI epoch (a selection, a panel).
    let syncs = h.lane_syncs();
    for _ in 0..3 {
        h.sync();
    }
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.lane_syncs(), syncs);
    // A lane edit syncs its track.
    h.eval_lanes("(len t1.processes)");
    h.sync();
    let syncs = h.lane_syncs();
    h.eval_lanes("(set-lane-steps! prob-lane (list (nth t0.steps 1)) 0.5)");
    h.lane_drain();
    assert_eq!(h.lane_syncs(), syncs + 1, "track 0 only");
    assert_eq!(h.eval_lanes("(nth prob-lane.values 1)"), number(0.5));

    // Live fields: computed only while observed, and only when the
    // scheduler's scopes or run errors moved.
    h.sync();
    assert_eq!(h.computed(f::STATE_CELL_VALUES), 0);
    assert_eq!(h.computed(f::PROCESS_ERROR), 0);
    h.eval_lanes("(def held (first rnd.cells))");
    let rand = h.slot_named(0, "rand");
    let runtime = sequencer::process::track_process_slot_runtime_id(&rand, 0).0;
    let scope = |values: Vec<f32>| {
        HashMap::from([(runtime, HashMap::from([("held".to_string(), values)]))])
    };
    h.shared
        .state
        .publish_process_scope_values(scope(vec![1.0, 4.0]));
    // A cold read asks the host.
    assert_eq!(h.eval_lanes("held.values"), h.eval_lanes("(list 1 4)"));
    assert_eq!(h.computed(f::STATE_CELL_VALUES), 1);
    h.sync();
    assert_eq!(
        h.computed(f::STATE_CELL_VALUES),
        1,
        "unobserved: the tick reads nothing"
    );
    // Observed: pushed when the scopes move, not every tick.
    h.eval_lanes(r#"(effect-buffer "*scope*" (label (str (len held.values) rnd.error)))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    let held = h.lane_instance("held");
    assert!(h.rt().host_field_observed(held, "values"));
    h.sync();
    let observed = h.computed(f::STATE_CELL_VALUES);
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::STATE_CELL_VALUES), observed, "no scope moved");
    h.shared
        .state
        .publish_process_scope_values(scope(vec![1.0, 4.0, 7.0]));
    h.sync();
    assert!(h.computed(f::STATE_CELL_VALUES) > observed);
    assert_eq!(h.eval_lanes("held.values"), h.eval_lanes("(list 1 4 7)"));
    let errors = std::collections::BTreeMap::from([(runtime, "boom".to_string())]);
    h.shared.state.publish_process_run_errors(errors);
    h.sync();
    assert_eq!(h.eval_lanes("rnd.error"), s("boom"));
}

#[test]
fn a_port_binds_a_param_of_its_own_track_by_the_devices_stable_ids() {
    let (mut h, slot) = Harness::with_devices();
    h.publish_library();
    h.eval_lanes(LANES);
    h.sync();
    h.eval_lanes(
        r#"(def flt (first t0.devices)) (def cutoff (part flt.params "cutoff"))
           (def t2 (track 2)) (def inst (first t2.devices)) (def other (first inst.params))"#,
    );
    h.eval_lanes("(bind-port! out cutoff)");
    h.lane_drain();
    let rand = h.slot_named(0, "rand");
    match rand.bindings.get("out") {
        Some(Some(sequencer::process::ParamTarget::EffectParam {
            slot: bound, param, ..
        })) => assert_eq!((*bound, param.as_str()), (slot, "cutoff")),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        h.eval_lanes("(list out.status out.manual)"),
        h.eval_lanes(r#"(list "bound" true)"#)
    );
    h.lane_rejects("(bind-port! out other)", "its own track's params only");
}

#[test]
fn process_parts_carry_their_key_index_and_step_params_are_canonical() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    // Each part's `index` is its place in its process's list.
    for list in ["rnd.inlets", "rnd.ports", "rnd.cells", "prob.lanes"] {
        let indexes = h.eval_lanes(&format!("(map (lambda (x) x.index) {list})"));
        let count = items(&indexes).len();
        assert!(count > 0, "{list}");
        let expected = list_value((0..count).map(|index| number(index as f64)));
        assert_eq!(indexes, expected, "{list}");
    }
    // The writable step params, by their canonical names.
    let options = h.eval_lanes("project.step-param-options");
    let names: Vec<&str> = crate::param_words::step_param_target_names().collect();
    assert_eq!(options, list_value(names.iter().map(|name| s(name))));
    assert!(names.contains(&"velocity") && names.contains(&"retrig-rate"));
    // Any spelling the scheduler accepts binds its canonical name.
    h.eval_lanes(r#"(bind-port! out "Vel")"#);
    h.lane_drain();
    assert_eq!(h.eval_lanes("out.target-step-param"), s("velocity"));
    match h.slot_named(0, "rand").bindings.get("out") {
        Some(Some(sequencer::process::ParamTarget::StepParam { param })) => {
            assert_eq!(param, "velocity")
        }
        other => panic!("{other:?}"),
    }
    h.lane_rejects(
        r#"(bind-port! out "bogus")"#,
        "one of project.step-param-options",
    );
}

#[test]
fn targets_and_moves_stay_on_their_track_and_layer() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    h.eval_lanes("(def cmpa1 (by-name t1 \"cmp A\")) (def prob1 (by-name t1 \"prob\"))");
    // Another track's lane or process is no target, nor a place to move to.
    h.lane_rejects(
        "(bind-port! wire (first cmpa1.lanes))",
        "a port targets its own track",
    );
    h.lane_rejects(
        "(add-fanout! wire (first cmpa1.lanes))",
        "a port targets its own track",
    );
    h.lane_rejects(
        "(move-process! rnd prob1)",
        "a process moves within its own track",
    );
    // A process moves within its layer only.
    h.eval_lanes(
        r#"(add-process! t0 (first (filter (lambda (c) (= c.name "lane-grab"))
                                            process-library.classes)))"#,
    );
    h.lane_drain();
    h.eval_lanes("(def added (nth t0.processes (- (len t0.processes) 1)))");
    h.lane_rejects("(move-process! added prob)", "moves within its layer");
    h.lane_rejects("(move-process! rnd added)", "moves within its layer");
    // :all edits a project lane only.
    h.lane_rejects(
        "(set-process-enabled! added false :all true)",
        ":all edits a project lane",
    );
}

#[test]
fn shared_edits_record_only_changes_and_reach_every_registered_track() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.eval_lanes("(def rnd1 (by-name t1 \"rand\")) (def lo1 (part rnd1.inlets \"lo\"))");
    h.sync();
    assert_eq!(h.frame.host_kinds.shared.borrow().lanes.tracks.len(), 2);
    // An :all edit lands on every track, track 1 included (registered, but
    // its process generation did not move).
    let entries = h.app.history.undo_len();
    h.eval_lanes("(set-inlet! lo -7 :all true)");
    h.lane_drain();
    assert_eq!(h.app.history.undo_len(), entries + 1);
    assert_eq!(h.eval_lanes("lo1.value"), number(-7.0));
    // Again: nothing changes, nothing is recorded.
    h.eval_lanes("(set-inlet! lo -7 :all true)");
    h.lane_drain();
    assert_eq!(
        h.app.history.undo_len(),
        entries + 1,
        "the same inlet: no entry"
    );
    h.eval_lanes("(set-process-enabled! prob false :all true)");
    h.lane_drain();
    let entries = h.app.history.undo_len();
    h.eval_lanes("(set-process-enabled! prob false :all true)");
    h.eval_lanes("(unbind-port! out :all true) (unbind-port! out :all true)");
    h.lane_drain();
    assert_eq!(
        h.app.history.undo_len(),
        entries + 1,
        "only the first unbind changed anything"
    );
    h.eval_lanes(r#"(bind-port! out "speed" :all true)"#);
    h.lane_drain();
    let entries = h.app.history.undo_len();
    h.eval_lanes(r#"(bind-port! out "speed" :all true)"#);
    h.lane_drain();
    assert_eq!(h.app.history.undo_len(), entries, "bound already: no entry");
    assert_eq!(
        h.eval_lanes("(let ((pt (part rnd1.ports \"out\"))) pt.target-step-param)"),
        s("speed")
    );
}

#[test]
fn a_shared_fanout_edit_addresses_the_shared_list() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    let shared_fanout = |h: &Harness| {
        let chain = h.shared.state.project_process_chain();
        let reset = chain
            .slots
            .into_iter()
            .find(|slot| slot.instance_name.as_deref() == Some("reset"))
            .unwrap();
        reset.fanout.get("wire").cloned().unwrap_or_default()
    };
    assert_eq!(shared_fanout(&h).len(), 2);
    // Track 0 forks the list down to one entry.
    h.eval_lanes("(remove-fanout! (first reset-wire.fanout))");
    h.lane_drain();
    assert_eq!(h.eval_lanes("(len reset-wire.fanout)"), number(1.0));
    assert_eq!(shared_fanout(&h).len(), 2);
    // A shared edit's index is the shared list's: entry 1 exists there.
    h.eval_lanes(
        r#"(host-command "edit-process"
             (dict :track-id t0.tid :proc-id reset.proc-id :port "wire" :index 1
                   :op "fanout-lo" :value -3 :all true))"#,
    );
    h.lane_drain();
    let shared = shared_fanout(&h);
    assert_eq!(shared.len(), 2);
    assert_eq!(shared[1].lo, -3.0);
    // It drops the track's fork: track 0 hears the shared list again.
    assert_eq!(h.eval_lanes("(len reset-wire.fanout)"), number(2.0));
    h.eval_lanes("(remove-fanout! (first reset-wire.fanout))");
    h.lane_drain();
    h.eval_lanes(
        r#"(host-command "edit-process"
             (dict :track-id t0.tid :proc-id reset.proc-id :port "wire" :index 1
                   :op "remove-fanout" :all true))"#,
    );
    h.lane_drain();
    assert_eq!(shared_fanout(&h).len(), 1);
    h.lane_rejects(
        r#"(host-command "edit-process"
             (dict :track-id t0.tid :proc-id reset.proc-id :port "wire" :index 1
                   :op "remove-fanout" :all true))"#,
        "the fan-out entry is gone",
    );
}

#[test]
fn a_lane_drag_rebuilds_one_process_and_forked_follows_the_overrides() {
    let mut h = Harness::with_library();
    h.sync();
    h.eval_lanes(LANES);
    h.sync();
    // The first write forks the project lane (a full sync), the next ones
    // re-derive that process's lanes alone.
    h.eval_lanes("(set-lane-steps! prob-lane (list (nth t0.steps 0)) 0.5)");
    h.lane_drain();
    assert_eq!(h.eval_lanes("prob-lane.forked"), Value::Bool(true));
    let process_syncs = |h: &Harness| h.frame.host_kinds.shared.borrow().lanes.process_syncs;
    let before = process_syncs(&h);
    h.eval_lanes("(set-lane-steps! prob-lane (list (nth t0.steps 3)) 0.75)");
    h.lane_drain();
    assert_eq!(process_syncs(&h), before + 1, "only prob's lanes");
    assert_eq!(h.eval_lanes("(nth prob-lane.values 3)"), number(0.75));

    // An override holding the shared lane's own values composes the same
    // chain; clearing it still clears `forked`.
    let Value::String(inlet) = h.eval_lanes("prob-lane.inlet") else {
        panic!("a lane inlet")
    };
    let state = h.shared.state.clone();
    let mut project = state.project_process_chain();
    let prob = (project.slots.iter_mut())
        .find(|slot| slot.instance_name.as_deref() == Some("prob"))
        .unwrap();
    let lane = sequencer::process::ProcessLane {
        values: vec![1.0; 4],
    };
    prob.lanes.insert(inlet.clone(), lane.clone());
    let (id, identity) = (
        prob.instance_id,
        sequencer::process::project_slot_identity_id(prob),
    );
    assert!(state.set_project_process_chain(project));
    {
        let mut overrides = state.pattern.project_process_lane_overrides.lock().unwrap();
        let own = overrides[0].entry(identity).or_default();
        own.lanes.insert(inlet.clone(), lane);
    }
    h.shared
        .ui_invalidations
        .push(UiInvalidation::ProcessLaneValues { track: 0 });
    h.sync();
    assert_eq!(h.eval_lanes("prob-lane.forked"), Value::Bool(true));
    assert!(state.clear_project_process_lane_override(0, id, &inlet));
    h.shared
        .ui_invalidations
        .push(UiInvalidation::ProcessChain { track: 0 });
    h.sync();
    assert_eq!(h.eval_lanes("prob-lane.forked"), Value::Bool(false));
}

#[test]
fn live_fields_follow_a_runtime_id_that_moves_with_the_track() {
    let mut h = Harness::with_library();
    h.sync();
    // Only track 1's processes are registered (no other track's instances
    // change below).
    h.eval_lanes(
        r#"(def t1 (track 1))
           (def rnd1 (first (filter (lambda (p) (= p.name "rand")) t1.processes)))
           (def held1 (first rnd1.cells))"#,
    );
    h.sync();
    let rand = h.slot_named(1, "rand");
    assert!(
        rand.project_layer,
        "a project lane: its runtime id follows its track's position"
    );
    let at = |track| sequencer::process::track_process_slot_runtime_id(&rand, track).0;
    // Both ids hold a run error and a scope before the move, so nothing the
    // scheduler publishes moves when the track does.
    let errors = std::collections::BTreeMap::from([
        (at(1), "at 1".to_string()),
        (at(0), "at 0".to_string()),
    ]);
    h.shared.state.publish_process_run_errors(errors);
    let scope = |values: Vec<f32>| HashMap::from([("held".to_string(), values)]);
    let scopes = HashMap::from([(at(1), scope(vec![1.0])), (at(0), scope(vec![2.0, 3.0]))]);
    h.shared.state.publish_process_scope_values(scopes);
    h.eval_lanes(r#"(effect-buffer "*live*" (label (str rnd1.error (len held1.values))))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    h.sync();
    assert_eq!(
        h.eval_lanes("(list rnd1.error held1.values)"),
        h.eval_lanes(r#"(list "at 1" (list 1))"#)
    );
    let (rnd1, held1) = (h.lane_instance("rnd1"), h.lane_instance("held1"));
    // Deleting track 0 moves track 1 to position 0: the same instances,
    // read under the new runtime id.
    h.app.delete_track_recorded(0).expect("delete track 0");
    h.sync();
    assert!(h.rt().instance_is_live(rnd1) && h.rt().instance_is_live(held1));
    assert_eq!(
        h.eval_lanes("(list rnd1.error held1.values)"),
        h.eval_lanes(r#"(list "at 0" (list 2 3))"#)
    );
}
