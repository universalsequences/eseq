//! Stage 7g-3: the native neural engine's networks and neurons (fields,
//! playback, the step-editing selection), their setters, identity and
//! observed gating.

use super::*;
use sequencer::neural::NeuralVisualizationSnapshot;

const REFER_NEURAL: &str = "(import eseq.kinds :refer (track tracks project network \
                            networks set-neural-weight!))";

impl Harness {
    fn eval_neural(&mut self, code: &str) -> Value {
        let source = format!("{REFER_NEURAL}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Evaluate `code` (a setter), apply what it queued and sync.
    fn neural_set(&mut self, code: &str) {
        self.eval_neural(code);
        self.drain();
        self.sync();
    }

    /// `code`'s command fails with `message` and records nothing.
    fn neural_rejects(&mut self, code: &str, message: &str) {
        self.rejects_in(REFER_NEURAL, code, message, true);
    }

    fn neural_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_neural(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    fn neural_undo(&mut self) {
        app::edit::undo(&mut self.app);
        self.sync();
    }

    fn neural_state<T>(&self, read: impl FnOnce(&NeuralState) -> T) -> T {
        read(&self.frame.host_kinds.networks)
    }

    /// A four-neuron network `name` in the current scene, its handle bound
    /// as `nw` and its neurons as `n0` … `n3`; returns its id.
    fn network(&mut self, name: &str) -> u64 {
        let created = self.eval_neural(&format!("(neural-create :name \"{name}\" :neurons 4)"));
        self.sync();
        let id = num(get(&created, "id")) as u64;
        self.eval_neural(&format!(
            "(def nw (first (filter (lambda (x) (= x.nid {id})) (networks))))
             (def n0 (nth nw.neurons 0)) (def n1 (nth nw.neurons 1))
             (def n2 (nth nw.neurons 2)) (def n3 (nth nw.neurons 3))"
        ));
        id
    }
}

/// The legacy `SEQ.neural-networks` entry of network `id`.
fn legacy_network(h: &Harness, id: u64) -> Value {
    let networks = build_neural_networks_value(&h.shared.state);
    let entry = items(&networks)
        .into_iter()
        .find(|network| num(get(network, "id")) as u64 == id);
    entry.expect("the legacy publisher lists the network")
}

/// A label the legacy publisher shows as a keyword.
fn keyword_label(value: Value) -> Value {
    match value {
        Value::Keyword(label) => Value::String(label),
        other => other,
    }
}

/// A visualization snapshot of network `id` (four neurons) running.
fn snapshot(id: u64, energy: f32) -> NeuralVisualizationSnapshot {
    let mut snapshot = NeuralVisualizationSnapshot {
        active: true,
        network_id: id,
        num_neurons: 4,
        ..Default::default()
    };
    snapshot.energy[0] = energy;
    snapshot.energy[1] = 9.0;
    snapshot.trigger_activity[0] = 0.5;
    snapshot.trigger_activity[1] = 2.0;
    snapshot.dampening[0][1] = 0.254;
    snapshot
}

#[test]
fn network_fields_read_like_the_legacy_neural_networks() {
    let mut h = Harness::new();
    let id = h.network("router");
    h.eval_neural(&format!(
        "(neural-set {id} :reset-bars 2 :energy-decay 0.9 :max-poly 3 :max-poly-selection :random)
         (neural-neuron {id} 2 :route 1 :resolution :8 :threshold 0.5 :delay 2 :quantize :4
                        :transpose 7 :dampening 0.3 :recovery 0.5)
         (neural-weight {id} :from 0 :to 2 :value 0.75)"
    ));
    h.sync();
    let legacy = legacy_network(&h, id);
    assert_eq!(h.eval_neural("(len (networks))"), number(1.0));
    assert_eq!(h.eval_neural("nw.index"), number(0.0));
    let network_fields = [
        ("nw.nid", "id"),
        ("nw.name", "name"),
        ("nw.enabled", "enabled"),
        ("nw.neuron-count", "num-neurons"),
        ("nw.reset-bars", "reset-bars"),
        ("nw.energy-decay", "energy-decay"),
        ("nw.max-poly", "max-poly"),
        ("nw.max-poly-selection", "max-poly-selection"),
        ("nw.weights", "weights"),
    ];
    for (kinds, key) in network_fields {
        assert_eq!(h.eval_neural(kinds), get(&legacy, key), "{kinds}");
    }
    assert_eq!(
        h.eval_neural("(nth nw.weights 0)"),
        h.eval_neural("(list 0 0 0.75 0)")
    );
    let neuron = items(&get(&legacy, "neurons"))[2].clone();
    let neuron_fields = [
        ("n2.index", "index"),
        ("n2.delay", "delay"),
        ("n2.threshold", "threshold"),
        ("n2.transpose", "transpose"),
        ("n2.dampening-amount", "dampening"),
        ("n2.dampening-recovery", "dampening-recovery"),
    ];
    for (kinds, key) in neuron_fields {
        assert_eq!(h.eval_neural(kinds), get(&neuron, key), "{kinds}");
    }
    // Clocks are labels (the legacy keywords), the route a track.
    assert_eq!(
        h.eval_neural("n2.resolution"),
        keyword_label(get(&neuron, "resolution"))
    );
    assert_eq!(
        h.eval_neural("n2.quantize"),
        keyword_label(get(&neuron, "quantize"))
    );
    assert_eq!(h.eval_neural("n2.quantize"), s("4"));
    assert_eq!(h.eval_neural("n1.quantize"), s("off"), "no quantize");
    assert_eq!(get(&neuron, "route"), number(1.0));
    assert_eq!(h.eval_neural("(= n2.route (track 1))"), Value::Bool(true));
    assert_eq!(h.eval_neural("n1.route"), Value::Nil);
    assert_eq!(h.eval_neural("(= n2.network nw)"), Value::Bool(true));
    assert_eq!(h.eval_neural("(len nw.neurons)"), number(4.0));
}

#[test]
fn network_setters_go_through_history_follow_the_value_rule_and_undo() {
    let mut h = Harness::new();
    let id = h.network("router");
    let describe = format!("(neural-describe {id})");
    let entries = h.app.history.undo_len();
    h.neural_set("(set! nw.reset-bars 2)");
    assert_eq!(h.eval_neural("nw.reset-bars"), number(2.0));
    assert_eq!(get(&h.eval_neural(&describe), "reset-bars"), number(2.0));
    assert_eq!(h.app.history.undo_len(), entries + 1, "one entry");
    // The current value is no edit.
    h.neural_set("(set! nw.reset-bars 2)");
    assert_eq!(h.app.history.undo_len(), entries + 1);
    let edits = [
        "(set! nw.name \"matrix\")",
        "(set! nw.enabled false)",
        "(set! nw.energy-decay 0.5)",
        "(set! nw.max-poly 4)",
        "(set! nw.max-poly-selection \"Markov\")",
        "(set-neural-weight! nw 0 1 0.5)",
        "(set! n1.route (track 1))",
        "(set! n1.resolution \"8t\")",
        "(set! n1.quantize \"16\")",
        "(set! n1.delay 3)",
        "(set! n1.threshold 0.25)",
        "(set! n1.transpose -3)",
        "(set! n1.dampening-amount 0.5)",
        "(set! n1.dampening-recovery 0.75)",
    ];
    for code in edits {
        h.neural_set(code);
    }
    assert_eq!(h.app.history.undo_len(), entries + 15, "one entry per set!");
    let network = h.eval_neural(&describe);
    assert_eq!(get(&network, "name"), s("matrix"));
    assert_eq!(get(&network, "enabled"), Value::Bool(false));
    assert_eq!(get(&network, "max-poly-selection"), s("markov"));
    assert_eq!(h.eval_neural("(nth (nth nw.weights 0) 1)"), number(0.5));
    let n1 = items(&get(&network, "neurons"))[1].clone();
    assert_eq!(get(&n1, "route"), number(1.0));
    assert_eq!(get(&n1, "resolution"), Value::Keyword("8T".to_string()));
    assert_eq!(get(&n1, "quantize"), Value::Keyword("16".to_string()));
    assert_eq!(
        h.eval_neural("(list n1.delay n1.threshold n1.transpose n1.dampening-amount)"),
        h.eval_neural("(list 3 0.25 -3 0.5)")
    );
    h.neural_set("(set! n1.quantize \"OFF\")");
    assert_eq!(h.eval_neural("n1.quantize"), s("off"));
    h.neural_set(
        "(set! nw.weights (list (list 1 0 0 0) (list 0 1 0 0) (list 0 0 1 0) (list 0 0 0 1)))",
    );
    assert_eq!(
        h.eval_neural("(nth nw.weights 2)"),
        h.eval_neural("(list 0 0 1 0)")
    );
    h.neural_set("(set! n1.route nil)");
    assert_eq!(h.eval_neural("n1.route"), Value::Nil);
    let edited = h.app.history.undo_len();
    assert_eq!(edited, entries + 18);

    // The value rule: where the natives clamp, the setters reject.
    h.neural_rejects("(set! n1.threshold -1)", "threshold takes a number from 0");
    h.neural_rejects(
        "(set! nw.energy-decay 1.5)",
        "energy-decay takes a number from 0 to 1",
    );
    h.neural_rejects("(set! nw.max-poly 0)", "max-poly takes an integer from 1");
    h.neural_rejects(
        "(set! nw.reset-bars 0.1)",
        "reset-bars takes a number from 0.25",
    );
    h.neural_rejects("(set! n1.dampening-amount 2)", "a number from 0 to 1");
    h.neural_rejects("(set! n1.resolution \"17\")", "resolution takes one of");
    h.neural_rejects("(set! n1.quantize \"never\")", "quantize takes one of");
    h.neural_rejects("(set! nw.max-poly-selection \"loud\")", "one of");
    h.neural_rejects("(set! nw.name \"  \")", "a non-empty name");
    h.neural_rejects(
        "(set-neural-weight! nw 0 4 1)",
        "to takes an integer from 0 to 3",
    );
    h.neural_rejects(
        "(set! nw.weights (list (list 1 0)))",
        "4 lists of 4 finite numbers",
    );
    // set! itself rejects a value of the wrong type.
    let typed = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER_NEURAL}\n(set! n1.delay 1.5)"));
    assert!(format!("{typed:?}").contains("is :int"), "{typed:?}");
    assert_eq!(h.app.history.undo_len(), edited);

    // Undo restores each field as it was.
    for _ in 0..18 {
        h.neural_undo();
    }
    assert_eq!(h.app.history.undo_len(), entries);
    let network = h.eval_neural(&describe);
    assert_eq!(get(&network, "name"), s("router"));
    assert_eq!(get(&network, "reset-bars"), number(4.0));
    assert_eq!(h.eval_neural("nw.enabled"), Value::Bool(true));
    assert_eq!(
        h.eval_neural("(nth nw.weights 0)"),
        h.eval_neural("(list 0 0 0 0)")
    );
    assert_eq!(
        h.eval_neural("(list n1.delay n1.threshold)"),
        h.eval_neural("(list 0 1)")
    );
    app::edit::redo(&mut h.app);
    h.sync();
    assert_eq!(h.eval_neural("nw.reset-bars"), number(2.0));

    // A drag's set!s on one field join one entry.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for v in [0.5, 0.75, 2.0] {
        h.neural_set(&format!("(set! n2.threshold {v})"));
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    assert_eq!(h.eval_neural("n2.threshold"), number(2.0));
    h.neural_undo();
    assert_eq!(
        h.eval_neural("n2.threshold"),
        number(1.0),
        "the value before the drag"
    );
}

#[test]
fn network_undo_restores_only_the_edited_field_in_its_scene() {
    let mut h = Harness::new();
    let id = h.network("router");
    h.neural_set("(set! n0.threshold 0.5)");
    // An unrecorded legacy write to another field of the network stays.
    h.eval_neural(&format!("(neural-neuron {id} 0 :transpose 5)"));
    h.sync();
    h.neural_undo();
    assert_eq!(
        h.eval_neural("(list n0.threshold n0.transpose)"),
        h.eval_neural("(list 1 5)")
    );

    // Undo lands in the scene the edit was made in.
    h.command("clone-pattern", Value::Nil);
    h.eval_neural("(host-command \"switch-pattern\" (dict :idx 1 :quantize \"off\"))");
    h.drain();
    h.sync();
    assert_eq!(h.app.state.current_scene_index(), 1);
    h.eval_neural(&format!(
        "(def nw (first (filter (lambda (x) (= x.nid {id})) (networks))))"
    ));
    h.neural_set("(set! nw.max-poly 7)");
    h.eval_neural("(host-command \"switch-pattern\" (dict :idx 0 :quantize \"off\"))");
    h.drain();
    h.sync();
    assert_eq!(h.eval_neural("nw.max-poly"), number(2.0), "scene 0's");
    h.neural_undo();
    let scenes = h.app.state.capture_project_scenes();
    let max_poly = |scene: usize| scenes.scenes[scene].neural_networks[0].max_poly;
    assert_eq!((max_poly(0), max_poly(1)), (2, 2), "scene 1's edit undone");
    app::edit::redo(&mut h.app);
    let scenes = h.app.state.capture_project_scenes();
    assert_eq!(scenes.scenes[1].neural_networks[0].max_poly, 7);
    assert_eq!(scenes.scenes[0].neural_networks[0].max_poly, 2);
}

#[test]
fn networks_keep_their_identity_and_stale_handles_error() {
    let mut h = Harness::new();
    let a = h.network("a");
    let (nw_a, n3_a) = (h.neural_instance("nw"), h.neural_instance("n3"));
    let b = h.network("b");
    let nw_b = h.neural_instance("nw");
    assert_ne!(nw_a, nw_b);
    assert_eq!(h.eval_neural("(len (networks))"), number(2.0));
    let project = h.singleton(PROJECT);
    assert_eq!(
        h.instances(h.rt().instance_field(project, "networks").unwrap()),
        vec![nw_a, nw_b]
    );
    // Deleting the first: its handles go stale, the second keeps its
    // instance and moves to index 0.
    h.eval_neural(&format!("(neural-delete {a})"));
    h.sync();
    assert!(!h.rt().instance_is_live(nw_a));
    assert!(!h.rt().instance_is_live(n3_a), "its neurons go with it");
    assert_eq!(h.neural_instance("(first (networks))"), nw_b);
    assert_eq!(h.eval_neural("nw.index"), number(0.0));
    // A setter resolves its network when it lands: one deleted after the
    // set! is an error that records nothing.
    let before = h.app.history.undo_len();
    h.editor.minibuffer = None;
    h.eval_neural("(set! nw.max-poly 5)");
    h.eval_neural(&format!("(neural-delete {b})"));
    h.drain();
    assert!(h.error().contains("the network is gone"), "{}", h.error());
    assert_eq!(h.app.history.undo_len(), before);
    h.sync();
    assert!(!h.rt().instance_is_live(nw_b));
    assert_eq!(h.eval_neural("(len (networks))"), number(0.0));
    // A project load replaces every network.
    let c = h.network("c");
    let nw_c = h.neural_instance("nw");
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(nw_c));
    assert_eq!(h.eval_neural("(len (networks))"), number(0.0));
    let _ = c;
}

#[test]
fn network_syncs_follow_their_key_and_live_fields_are_observed_gated() {
    let mut h = Harness::new();
    let id = h.network("router");
    let other = h.network("idle");
    h.eval_neural(&format!(
        "(def nw (first (networks))) (def n0 (nth nw.neurons 0)) (def n1 (nth nw.neurons 1))
         (def n2 (nth nw.neurons 2)) (def idle (nth (networks) 1)) (def i0 (nth idle.neurons 0))"
    ));
    let (syncs, pushes) = h.neural_state(|n| (n.syncs, n.pushes));
    // Idle ticks sync nothing; a step edit pushes no network.
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.neural_state(|n| n.syncs), syncs);
    h.eval_neural("(seq-set-track-step 0 3 true)");
    h.sync();
    assert_eq!(h.neural_state(|n| n.pushes), pushes, "no network moved");
    // A legacy edit pushes its network alone.
    h.eval_neural(&format!("(neural-neuron {id} 1 :delay 4)"));
    h.sync();
    assert_eq!(h.neural_state(|n| n.pushes), pushes + 1);
    assert_eq!(h.eval_neural("n1.delay"), number(4.0));

    // Playback: nothing computed while unobserved.
    h.shared.state.set_neural_visualization(snapshot(id, 0.5));
    for _ in 0..3 {
        h.sync();
    }
    for key in [f::NEURON_ENERGY, f::NEURON_SELECTED, f::NETWORK_ACTIVE] {
        assert_eq!(h.computed(key), 0, "{key:?}");
    }
    // A cold read asks the host (the legacy transforms: energy clamped to
    // 0-4 and rounded, triggers clamped, dampening rounded, by target).
    assert_eq!(
        h.eval_neural("(list n0.energy n1.energy n0.trigger n1.trigger)"),
        h.eval_neural("(list 0.5 4 0.5 1)")
    );
    assert_eq!(
        h.eval_neural("n0.dampening"),
        h.eval_neural("(list 0 0.25 0 0)")
    );
    let legacy_energy = items(&build_neural_energy_matrix_value(&h.shared.state));
    let legacy_dampening = items(&build_neural_dampening_matrix_value(&h.shared.state));
    for neuron in 0..4 {
        let read = |h: &mut Harness, field: &str| {
            h.eval_neural(&format!("(let ((n (nth nw.neurons {neuron}))) n.{field})"))
        };
        assert_eq!(read(&mut h, "energy"), items(&legacy_energy[neuron])[0]);
        assert_eq!(read(&mut h, "dampening"), legacy_dampening[neuron]);
    }
    assert_eq!(
        h.eval_neural("(list nw.active idle.active i0.energy (len i0.dampening))"),
        h.eval_neural("(list true false 0 4)"),
        "zeros unless the engine runs the network"
    );

    // Observed: pushed when the snapshot moves.
    h.eval_neural(
        r#"(effect-buffer "*neural*" (label (str n0.energy nw.active n2.selected (len n0.dampening))))"#,
    );
    h.editor.runtime_mut().run_reactive_cycle();
    let n0 = h.neural_instance("n0");
    assert!(h.rt().host_field_observed(n0, "energy"));
    h.sync();
    let computed = h.computed(f::NEURON_ENERGY);
    assert!(computed > 0);
    h.shared.state.set_neural_visualization(snapshot(id, 1.25));
    h.sync();
    assert_eq!(h.eval_neural("n0.energy"), number(1.25));
    // The engine moves to the other network: this one reads zeros.
    h.shared
        .state
        .set_neural_visualization(snapshot(other, 2.0));
    h.sync();
    assert_eq!(
        h.eval_neural("(list n0.energy nw.active)"),
        h.eval_neural("(list 0 false)")
    );

    // The step-editing selection, as the legacy natives see it.
    h.eval_neural(&format!("(neural-select-neuron {id} 2)"));
    h.sync();
    assert_eq!(h.eval_neural("n2.selected"), Value::Bool(true));
    h.eval_neural("(set! n1.selected true)");
    h.sync();
    assert_eq!(
        h.eval_neural(&format!(
            "(list n1.selected n2.selected (neural-neuron-selected? {id} 1) (neural-neuron-selected? {id} 2))"
        )),
        h.eval_neural("(list true false true false)"),
        "selecting one selects it alone"
    );
    h.eval_neural("(set! n1.selected false)");
    h.sync();
    assert_eq!(h.eval_neural("n1.selected"), Value::Bool(false));
    assert_eq!(
        h.eval_neural("(neural-selected-neurons)"),
        h.eval_neural("(list)")
    );
    assert!(h.computed(f::NEURON_SELECTED) > 0);
    h.editor.runtime_mut().take_status_message();
    h.eval_neural(&format!("(neural-set-neuron-selected {id} 9 true)"));
    let status = h
        .editor
        .runtime_mut()
        .take_status_message()
        .unwrap_or_default();
    assert!(status.contains("neuron index out of range"), "{status}");
}

#[test]
fn every_neuron_field_name_is_settable() {
    let mut h = Harness::new();
    let id = h.network("router");
    let tid = h.eval_neural("(let ((t (track 1))) t.tid)");
    let values = |name: &str| match name {
        "route" => Value::Nil,
        "resolution" => s("8"),
        "quantize" => s("4"),
        "delay" => number(3.0),
        "transpose" => number(2.0),
        _ => number(0.5),
    };
    for name in sequencer::neural::NeuronField::NAMES {
        let before = h.app.history.undo_len();
        h.editor.minibuffer = None;
        h.command(
            "set-neural",
            map_value([
                ("network-id", number(id as f64)),
                ("field", s(name)),
                ("value", values(name)),
                ("neuron", number(1.0)),
                ("track-id", tid.clone()),
            ]),
        );
        app::edit::finish_active_gesture(&mut h.app);
        assert_eq!(h.error(), "", "{name}");
        assert_eq!(
            h.app.history.undo_len(),
            before + 1,
            "{name} records an entry"
        );
    }
}

#[test]
fn network_ids_are_never_reused() {
    let mut h = Harness::new();
    let a = h.network("a");
    let b = h.network("b");
    let (nw_b, n0_b) = (h.neural_instance("nw"), h.neural_instance("n0"));
    h.neural_set("(set! n0.threshold 0.5)");
    // Delete the highest id and create another before a sync: a fresh id.
    h.eval_neural(&format!("(neural-delete {b})"));
    let created = h.eval_neural("(neural-create :name \"c\" :neurons 4)");
    let c = num(get(&created, "id")) as u64;
    assert!(c != a && c != b, "{c} reuses an id");
    h.sync();
    assert!(
        !h.rt().instance_is_live(nw_b),
        "the deleted network's handle goes stale"
    );
    assert!(!h.rt().instance_is_live(n0_b));
    // Undo of the deleted network's edit fails; the new network is untouched.
    let replay = app::edit::undo(&mut h.app);
    assert!(
        format!("{replay:?}").contains(&format!("neural network {b} no longer exists")),
        "{replay:?}"
    );
    let threshold = |h: &Harness| {
        let networks = h.app.state.current_neural_networks();
        let network = networks.iter().find(|network| network.id == c).unwrap();
        network.neurons[0].threshold
    };
    assert_eq!(threshold(&h), 1.0);
    // And again after deleting the newest.
    h.eval_neural(&format!("(neural-delete {c})"));
    let again = h.eval_neural("(neural-create :name \"d\" :neurons 4)");
    assert!(num(get(&again, "id")) as u64 > c);
}
