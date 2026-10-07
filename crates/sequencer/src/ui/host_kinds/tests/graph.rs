//! Stage 7g: graph sequencers (graphs, nodes, edges, params, playback,
//! active notes), their setters, identity and observed gating; 7g-2: a
//! node's process patch (`n.processes`) and its setters; 7g-4: the event
//! streams (`graph.events`, `node-events`, deltas and group traces,
//! `transport.track-events`).

use super::*;
use eseqlisp::widget_render::event_view::ROW_FIELDS;
use sequencer::graph::{
    GraphDeltaEntry, GraphDeltaKey, GraphSoundingNote, GraphVisualizationEdge,
    GraphVisualizationEvent, GraphVisualizationSnapshot,
};
use sequencer::sequencer::TrackOutputEvent;

const REFER_GRAPH: &str = "(import eseq.kinds :refer (track tracks project graphs graph-of \
                           graph-param-named graph-edge-to set-group-gain! set-group-coupling! \
                           gate-generator! graph-timebase-options graph-quantize-options \
                           graph-max-poly-selection-options process-library \
                           set-process-enabled! set-inlet! move-process! add-process! \
                           remove-process! bind-port! add-fanout! unbind-port! clear-port! \
                           remove-fanout!))";

const NEURAL: &str = "alez/neural:neural";

impl Harness {
    fn eval_graph(&mut self, code: &str) -> Value {
        let source = format!("{REFER_GRAPH}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Create a `neural` instance (alez.neural; its :on-create writes the
    /// ring), sync, and bind `self` to it in Lisp as `name`; returns its id.
    fn neural(&mut self, name: &str) -> u64 {
        if self.rt().instance_kind_schema(NEURAL).is_none() {
            self.eval("(import alez.neural.variable-reset)");
        }
        let id = self.create_instance(NEURAL);
        self.eval_graph(&format!("(def {name} (instance-ref {id}))"));
        id
    }

    /// Create an instance of `kind` (project-owned) and sync; returns its id.
    fn create_instance(&mut self, kind: &str) -> u64 {
        let before = crate::host_commands::instances::instance_ids(&self.app);
        self.command("instance-create", map_value([("kind", s(kind))]));
        let created = crate::host_commands::instances::instance_ids(&self.app);
        let id = *created
            .difference(&before)
            .next()
            .expect("an instance was created");
        self.sync();
        id
    }

    fn graph_drain(&mut self) {
        self.drain();
        self.sync();
    }

    fn graph_undo(&mut self) {
        app::edit::undo(&mut self.app);
        self.sync();
    }

    fn graph_redo(&mut self) {
        app::edit::redo(&mut self.app);
        self.sync();
    }

    /// `code`'s command fails with `message` and records nothing.
    fn graph_rejects(&mut self, code: &str, message: &str) {
        let before = self.app.history.undo_len();
        self.editor.minibuffer = None;
        self.eval_graph(code);
        self.graph_drain();
        let error = self.editor.minibuffer.clone().unwrap_or_default();
        assert!(error.contains(message), "{code}: {error}");
        assert_eq!(self.app.history.undo_len(), before, "{code}: no entry");
    }

    fn graph_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_graph(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    fn graph_shared<T>(&self, read: impl FnOnce(&GraphShared) -> T) -> T {
        read(&self.frame.host_kinds.shared.borrow().graphs)
    }

    /// The same expression through the kinds and through the legacy native.
    fn same(&mut self, kinds: &str, legacy: &str) {
        let (a, b) = (self.eval_graph(kinds), self.eval_graph(legacy));
        assert_eq!(a, b, "{kinds} vs {legacy}");
    }
}

fn map_value<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

#[test]
fn graph_fields_read_like_the_legacy_graph_reads() {
    let mut h = Harness::new();
    let id = h.neural("nn");
    h.eval_graph(
        "(def g (graph-of nn)) (def n0 (nth g.nodes 0)) (def n1 (nth g.nodes 1))
         (def thr (lambda (n) (graph-param-named n \"threshold\")))",
    );
    assert_eq!(h.eval_graph("g.gid"), number(id as f64));
    assert_eq!(h.eval_graph("(= g (graph-of nn.id))"), Value::Bool(true));
    assert_eq!(h.eval_graph("(len (graphs))"), number(1.0));
    assert_eq!(h.eval_graph("g.owner"), Value::Nil);
    assert_eq!(
        h.eval_graph("(list g.variable g.min-nodes g.max-nodes)"),
        h.eval_graph("(list true 1 16)")
    );
    h.same("g.node-count", "(graph-config-value nn :node-count)");
    h.same("(len g.nodes)", "(graph-config-value nn :node-count)");
    h.same("g.reset-bars", "(graph-config-value nn :reset-bars)");
    h.same("g.max-poly", "(graph-config-value nn :max-poly)");
    h.same(
        "g.max-poly-selection",
        "(graph-config-value nn :max-poly-selection)",
    );
    h.same(
        "g.group-trace-decay",
        "(graph-config-value nn :group-trace-decay)",
    );
    h.same(
        "(nth g.group-gain 6)",
        "(graph-config-value nn :group-gain-1-2)",
    );
    h.same(
        "(nth g.group-coupling 6)",
        "(graph-config-value nn :group-coupling-1-2)",
    );
    for field in ["delay", "resolution", "quantize", "seed-on-reset", "group"] {
        h.same(
            &format!("n1.{field}"),
            &format!("(graph-node-value nn 1 :{field})"),
        );
    }
    h.same(
        "(if n0.seed-route 1 0)",
        "(graph-node-value nn 0 :seed-route)",
    );
    h.same(
        "(if n1.seed-route 1 0)",
        "(graph-node-value nn 1 :seed-route)",
    );
    // The ring default (:on-create): node 0 seeds from its route, track 0.
    assert_eq!(h.eval_graph("n0.seed-route"), Value::Bool(true));
    assert_eq!(h.eval_graph("(= n0.route (track 0))"), Value::Bool(true));
    assert_eq!(
        h.eval_graph("(= (first n0.seeds) (track 0))"),
        Value::Bool(true)
    );
    assert_eq!(
        h.eval_graph("(list n0.generator n0.restart)"),
        h.eval_graph("(list -1 false)")
    );
    h.same(
        "n1.resolution-cycle",
        "(list (graph-node-value nn 1 :resolution))",
    );
    // Params and edges register on first read.
    assert!(h.graph_shared(|g| g.node_params.is_empty() && g.node_edges.is_empty()));
    h.same(
        "(let ((p (thr n1))) p.value)",
        "(graph-param-value nn 1 :threshold)",
    );
    assert_eq!(
        h.eval_graph("(map (lambda (p) p.name) n1.params)"),
        h.eval_graph(
            "(list \"threshold\" \"global-transpose\" \"transpose\" \"transpose-reset\" \
             \"dur-factor\" \"vel-decay\" \"vel-reset\" \"dampening\" \"recovery\")"
        )
    );
    assert_eq!(
        h.eval_graph(
            "(let ((p (thr n1))) (list p.type p.min p.max p.default (= p.node n1) p.edge))"
        ),
        h.eval_graph("(list \"float\" 0 4 0.55 true nil)")
    );
    assert_eq!(
        h.eval_graph("(len n0.edges)"),
        h.eval_graph("(len g.nodes)")
    );
    h.eval_graph("(def e01 (graph-edge-to n0 1)) (def w01 (graph-param-named e01 \"weight\"))");
    assert_eq!(
        h.eval_graph("(list (= e01.from n0) (= e01.to n1))"),
        h.eval_graph("(list true true)")
    );
    h.same("w01.value", "(graph-edge-value nn 0 1 :weight)");
    assert_eq!(h.eval_graph("w01.value"), number(1.0), "the ring");
    h.same(
        "(let ((p (graph-param-named (graph-edge-to n1 0) \"weight\"))) p.value)",
        "(graph-edge-value nn 1 0 :weight)",
    );
    assert_eq!(
        h.eval_graph("(list (= w01.edge e01) w01.node)"),
        h.eval_graph("(list true nil)")
    );

    // Legacy writes flow into the kinds at the next sync.
    h.eval_graph(
        "(graph-node nn 1 :delay 3 :resolution \"8\" :quantize \"off\" :group 2 :route 1 :seed-from (list 0 1))
         (graph-param nn 1 :threshold 1.25)
         (graph-edge nn :from 0 :to 1 :weight 0.25)
         (graph-config nn :max-poly 6)
         (graph-config nn :group-gain-1-2 1.5)",
    );
    h.sync();
    for field in ["delay", "resolution", "quantize", "group"] {
        h.same(
            &format!("n1.{field}"),
            &format!("(graph-node-value nn 1 :{field})"),
        );
    }
    assert_eq!(h.eval_graph("n1.delay"), number(3.0));
    assert_eq!(h.eval_graph("(= n1.route (track 1))"), Value::Bool(true));
    assert_eq!(
        h.eval_graph("(map (lambda (t) t.index) n1.seeds)"),
        h.eval_graph("(list 0 1)")
    );
    assert_eq!(h.eval_graph("n1.seed-route"), Value::Bool(false));
    h.same(
        "(let ((p (thr n1))) p.value)",
        "(graph-param-value nn 1 :threshold)",
    );
    h.same("w01.value", "(graph-edge-value nn 0 1 :weight)");
    assert_eq!(h.eval_graph("w01.value"), number(0.25));
    h.same("g.max-poly", "(graph-config-value nn :max-poly)");
    assert_eq!(h.eval_graph("(nth g.group-gain 6)"), number(1.5));
    h.same(
        "(nth g.group-gain 6)",
        "(graph-config-value nn :group-gain-1-2)",
    );
    // A generator route.
    h.eval_graph("(graph-node nn 2 :route (list :restart 7))");
    h.sync();
    assert_eq!(
        h.eval_graph("(let ((n (nth g.nodes 2))) (list n.route n.generator n.restart))"),
        h.eval_graph("(list nil 7 true)")
    );
    // The option constants are the host's.
    let strings = |value: Value| -> Vec<String> {
        match value {
            Value::List(items) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::String(text) => text.clone(),
                    other => panic!("{other:?}"),
                })
                .collect(),
            other => panic!("{other:?}"),
        }
    };
    let timebases: Vec<String> = sequencer::sequencer::Timebase::LABELS
        .map(String::from)
        .to_vec();
    assert_eq!(strings(h.eval_graph("graph-timebase-options")), timebases);
    let quantize: Vec<String> = std::iter::once("off".to_string())
        .chain(timebases.iter().cloned())
        .collect();
    assert_eq!(strings(h.eval_graph("graph-quantize-options")), quantize);
    let selections: Vec<String> = sequencer::neural::NeuralMaxPolySelection::ALL
        .map(|selection| selection.as_str().to_string())
        .to_vec();
    assert_eq!(
        strings(h.eval_graph("graph-max-poly-selection-options")),
        selections
    );
    assert_eq!(
        h.eval_graph("(list (len g.group-gain) (len g.group-coupling))"),
        h.eval_graph(&format!(
            "(list {0} {0})",
            sequencer::graph::NEURAL_GROUP_CELLS
        ))
    );
}

#[test]
fn graph_setters_go_through_history_follow_the_value_rule_and_undo() {
    let mut h = Harness::new();
    h.neural("nn");
    h.eval_graph(
        "(def g (graph-of nn)) (def n1 (nth g.nodes 1))
         (def thr (graph-param-named n1 \"threshold\"))
         (def w12 (graph-param-named (graph-edge-to n1 2) \"weight\"))",
    );
    let entries = h.app.history.undo_len();
    let set = |h: &mut Harness, code: &str| {
        h.eval_graph(code);
        h.graph_drain();
    };
    set(&mut h, "(set! n1.delay 5)");
    assert_eq!(
        h.eval_graph("(list n1.delay (graph-node-value nn 1 :delay))"),
        h.eval_graph("(list 5 5)")
    );
    assert_eq!(h.app.history.undo_len(), entries + 1, "one entry");
    // The current value is no edit.
    set(&mut h, "(set! n1.delay 5)");
    assert_eq!(h.app.history.undo_len(), entries + 1);
    set(&mut h, "(set! n1.resolution \"8t\")");
    assert_eq!(h.eval_graph("(graph-node-value nn 1 :resolution)"), s("8T"));
    set(&mut h, "(set! n1.quantize-cycle (list \"16\" \"4\"))");
    assert_eq!(
        h.eval_graph("(graph-node-value nn 1 :quantize-cycle)"),
        s("16 4")
    );
    set(&mut h, "(set! n1.route (track 1))");
    assert_eq!(h.eval_graph("(graph-node-value nn 1 :route)"), number(1.0));
    set(&mut h, "(set! n1.seeds (list (track 1) (track 0)))");
    assert_eq!(
        h.eval_graph("(graph-node-value nn 1 :seed-from)"),
        h.eval_graph("(list 0 1)")
    );
    set(&mut h, "(set! n1.seed-route true)");
    assert_eq!(h.eval_graph("n1.seed-route"), Value::Bool(true));
    set(&mut h, "(set! n1.group 3)");
    set(&mut h, "(set! n1.seed-on-reset 0.5)");
    set(&mut h, "(set! thr.value 2.5)");
    assert_eq!(
        h.eval_graph("(graph-param-value nn 1 :threshold)"),
        number(2.5)
    );
    set(&mut h, "(set! w12.value -0.5)");
    assert_eq!(
        h.eval_graph("(graph-edge-value nn 1 2 :weight)"),
        number(-0.5)
    );
    set(&mut h, "(set! g.max-poly-selection \"Markov\")");
    assert_eq!(
        h.eval_graph("(graph-config-value nn :max-poly-selection)"),
        s("markov")
    );
    set(&mut h, "(set! g.reset-bars 2)");
    assert_eq!(
        h.eval_graph("(graph-config-value nn :reset-bars)"),
        number(2.0)
    );
    set(&mut h, "(set-group-coupling! g 1 2 -1.5)");
    assert_eq!(
        h.eval_graph("(graph-config-value nn :group-coupling-1-2)"),
        number(-1.5)
    );
    assert_eq!(h.eval_graph("(nth g.group-coupling 6)"), number(-1.5));
    set(&mut h, "(set! g.node-count 4)");
    assert_eq!(h.eval_graph("(len g.nodes)"), number(4.0));
    let edited = h.app.history.undo_len();
    assert_eq!(edited, entries + 14, "one entry per set!");

    // The value rule: anything else is an error that records nothing.
    h.graph_rejects("(set! n1.delay -1)", "delay takes an integer");
    // set! itself rejects a value of the wrong type.
    let typed = h.editor.runtime_mut().eval_str("(set! n1.delay 1.5)");
    assert!(format!("{typed:?}").contains("is :int"), "{typed:?}");
    h.graph_rejects("(set! n1.resolution \"17\")", "resolution takes one of");
    h.graph_rejects("(set! n1.quantize-cycle (list \"off\" \"4\"))", "off alone");
    h.graph_rejects("(set! thr.value 9)", "a number from 0 to 4");
    h.graph_rejects("(set! g.node-count 17)", "an integer from 1 to 16");
    h.graph_rejects("(set! g.max-poly-selection \"loud\")", "one of");
    h.graph_rejects("(set-group-gain! g 4 0 1)", "row takes");
    h.graph_rejects(
        "(gate-generator! n1 999)",
        "a generator instance of the graph's owner",
    );

    // Undo restores each override as it was, the rest of the graph alone.
    h.graph_undo();
    assert_eq!(h.eval_graph("(len g.nodes)"), number(8.0));
    for _ in 0..13 {
        h.graph_undo();
    }
    assert_eq!(h.app.history.undo_len(), entries);
    h.same("n1.delay", "(graph-node-value nn 1 :delay)");
    assert_eq!(h.eval_graph("n1.delay"), number(1.0));
    assert_eq!(
        h.eval_graph("(graph-param-value nn 1 :threshold)"),
        number(0.55)
    );
    assert_eq!(
        h.eval_graph("(graph-edge-value nn 1 2 :weight)"),
        number(1.0),
        "the ring"
    );
    assert_eq!(
        h.eval_graph("(graph-config-value nn :max-poly-selection)"),
        s("propagation")
    );
    h.graph_redo();
    assert_eq!(h.eval_graph("n1.delay"), number(5.0));

    // A drag's set!s on one field join one entry.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for v in [1.0, 1.5, 2.0] {
        set(&mut h, &format!("(set! thr.value {v})"));
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    assert_eq!(h.eval_graph("thr.value"), number(2.0));
    h.graph_undo();
    assert_eq!(
        h.eval_graph("thr.value"),
        number(0.55),
        "the value before the drag"
    );
}

#[test]
fn graphs_keep_their_identity_across_node_counts_instances_and_project_loads() {
    let mut h = Harness::new();
    h.neural("a");
    h.eval_graph(
        "(def ga (graph-of a)) (def a0 (nth ga.nodes 0)) (def a5 (nth ga.nodes 5))
         (def a0-edges a0.edges) (def a05 (graph-edge-to a0 5)) (def a01 (graph-edge-to a0 1))
         (def a0-params a0.params)",
    );
    let (ga, a0, a5, a01) = (
        h.graph_instance("ga"),
        h.graph_instance("a0"),
        h.graph_instance("a5"),
        h.graph_instance("a01"),
    );
    let a05 = h.graph_instance("a05");
    // A node-count change drops the last nodes (and the edges into them);
    // the others keep their instances.
    h.eval_graph("(set! ga.node-count 4)");
    h.graph_drain();
    assert!(!h.rt().instance_is_live(a5), "a dropped node goes stale");
    assert!(!h.rt().instance_is_live(a05), "and the edges into it");
    assert!(h.rt().instance_is_live(a0) && h.rt().instance_is_live(a01));
    assert_eq!(h.graph_instance("(nth ga.nodes 0)"), a0);
    assert_eq!(h.eval_graph("(len a0.edges)"), number(4.0));
    h.eval_graph("(set! ga.node-count 8)");
    h.graph_drain();
    assert_ne!(
        h.graph_instance("(nth ga.nodes 5)"),
        a5,
        "a fresh node, not the old handle"
    );
    assert_eq!(h.eval_graph("(len a0.edges)"), number(8.0));
    // A second instance: its own graph; deleting the first drops its graph.
    let b = h.neural("b");
    h.eval_graph("(def gb (graph-of b))");
    let gb = h.graph_instance("gb");
    assert_ne!(gb, ga);
    assert_eq!(h.eval_graph("(len (graphs))"), number(2.0));
    let a_id = num(h.eval_graph("a.id")) as u64;
    h.command("instance-delete", map_value([("id", number(a_id as f64))]));
    h.sync();
    assert!(
        !h.rt().instance_is_live(ga),
        "the deleted instance's graph goes stale"
    );
    assert!(!h.rt().instance_is_live(a0));
    assert_eq!(
        h.graph_instance("(first (graphs))"),
        gb,
        "the other graph is kept"
    );
    assert_eq!(h.eval_graph("gb.index"), number(0.0));
    // Undo brings the instance back: a fresh graph, not the stale handle.
    app::edit::undo(&mut h.app);
    crate::host_commands::instances::sync_instances_to_editor(&h.app, &mut h.editor);
    h.sync();
    assert_eq!(h.eval_graph("(len (graphs))"), number(2.0));
    assert!(!h.rt().instance_is_live(ga));
    assert_eq!(h.graph_instance("(graph-of b)"), gb);
    // A project load replaces every graph.
    h.command("new-project", Value::Nil);
    h.sync();
    assert!(!h.rt().instance_is_live(gb));
    assert_eq!(h.eval_graph("(len (graphs))"), number(0.0));
    assert!(h.graph_shared(|g| g.sources.is_empty()));
    let _ = b;
}

#[test]
fn graph_syncs_follow_their_key_and_live_fields_are_observed_gated() {
    let mut h = Harness::new();
    h.neural("nn");
    h.eval_graph("(def g (graph-of nn)) (def n1 (nth g.nodes 1)) (def t0 (track 0))");
    let (syncs, derives) = h.graph_shared(|g| (g.syncs, g.derives));
    // Idle ticks sync nothing; a UI epoch or a step edit re-derives no graph.
    for _ in 0..3 {
        h.sync();
    }
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.eval_graph("(seq-set-track-step 0 3 true)");
    h.sync();
    assert_eq!(h.graph_shared(|g| g.derives), derives, "no graph moved");
    assert!(h.graph_shared(|g| g.syncs) <= syncs + 1);
    // An override edit re-derives its graph once.
    h.eval_graph("(graph-node nn 1 :delay 2)");
    h.sync();
    h.sync();
    assert_eq!(h.graph_shared(|g| g.derives), derives + 1);

    // Playback: nothing computed while unobserved.
    let gid = num(h.eval_graph("g.gid")) as u64;
    let snapshot = |beat: f64, energy: f64| GraphVisualizationSnapshot {
        id: gid,
        name: "nn".to_string(),
        active: true,
        current_beat: beat,
        num_nodes: 2,
        energy: vec![energy, 9.0],
        trigger_activity: vec![0.5, 2.0],
        edges: vec![GraphVisualizationEdge {
            from: 0,
            to: 1,
            weight: 1.0,
            dampening: 0.25,
            delay_steps: 0,
            distribution: sequencer::graph::EdgeDistribution::BroadcastWeighted,
        }],
        node_sounding: vec![
            Vec::new(),
            vec![
                GraphSoundingNote {
                    note: 7.0,
                    velocity: 0.5,
                    start_sample: 0,
                    end_sample: 100,
                },
                GraphSoundingNote {
                    note: 9.0,
                    velocity: 0.75,
                    start_sample: 200,
                    end_sample: 300,
                },
            ],
        ],
        ..Default::default()
    };
    h.shared
        .state
        .set_graph_visualizations(vec![snapshot(1.0, 0.5)]);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.computed(f::GRAPH_ENERGY), 0);
    assert_eq!(h.computed(f::GRAPH_NODE_SOUNDING), 0);
    assert_eq!(h.computed(f::TRACK_ACTIVE_NOTES), 0);
    // A cold read asks the host (the legacy display transforms: energy and
    // triggers clamped, dampening by from row and to column).
    assert_eq!(h.eval_graph("g.energy"), h.eval_graph("(list 0.5 4)"));
    assert_eq!(h.eval_graph("g.triggers"), h.eval_graph("(list 0.5 1)"));
    assert_eq!(
        h.eval_graph("g.dampening"),
        h.eval_graph("(list (list 0 0.25) (list 0 0))")
    );
    assert_eq!(
        h.eval_graph("(list g.active g.beat)"),
        h.eval_graph("(list true 1)")
    );
    assert_eq!(
        h.eval_graph("n1.sounding"),
        h.eval_graph("(list)"),
        "stopped"
    );
    // Observed: pushed when the snapshot moves.
    h.eval_graph(
        r#"(effect-buffer "*graph*" (label (str (len g.energy) g.beat (len n1.sounding) (len t0.active-notes))))"#,
    );
    h.editor.runtime_mut().run_reactive_cycle();
    let g = h.graph_instance("g");
    assert!(h.rt().host_field_observed(g, "energy"));
    h.sync();
    let energy = h.computed(f::GRAPH_ENERGY);
    assert!(energy > 0);
    h.shared
        .state
        .set_graph_visualizations(vec![snapshot(2.0, 1.25)]);
    h.sync();
    assert_eq!(
        h.eval_graph("(list g.beat (first g.energy))"),
        h.eval_graph("(list 2 1.25)")
    );
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    h.shared.state.set_audio_rendered_sample(50);
    h.sync();
    assert_eq!(
        h.eval_graph("n1.sounding"),
        h.eval_graph("(list (list 7 0.5))")
    );
    h.shared.state.set_audio_rendered_sample(250);
    h.sync();
    assert_eq!(
        h.eval_graph("n1.sounding"),
        h.eval_graph("(list (list 9 0.75))")
    );
    assert!(h.computed(f::GRAPH_NODE_SOUNDING) > 0);
    assert!(h.computed(f::TRACK_ACTIVE_NOTES) > 0);
    assert_eq!(h.eval_graph("t0.active-notes"), h.eval_graph("(list)"));
    h.shared
        .state
        .transport
        .playing
        .store(false, Ordering::Relaxed);
}

#[test]
fn a_rack_owned_graph_routes_through_the_racks_members() {
    let mut h = Harness::new();
    let first = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    h.app
        .assign_rack_pad_track_recorded(group, first, 1)
        .expect("pad");
    h.app
        .assign_rack_pad_track_recorded(group, first + 2, 0)
        .expect("pad");
    h.share_buses_and_groups();
    h.sync();
    let members = h
        .app
        .groups
        .iter()
        .find(|g| g.id == group)
        .unwrap()
        .members
        .clone();
    // A view-less kind (the neural package's view reads modules the bare
    // root lacks for a rack-owned instance).
    h.eval(
        "(def-kind tiny
           :sequencer (:shape (line :default 3 :min 1 :max 4)
             (def-node nrn :resolution :16 :route 0
               :params ((threshold :float 0 4 :default 0.5))
               :state ((energy))
               :update (if (>= (energy) (param :threshold)) (emit :note 0) nil))
             (edges :from nrn :to nrn :topology (all-to-all)
               :params ((weight :float -1 1 :default 0)))))",
    );
    let payload = map_value([
        ("kind", s("scratch:tiny")),
        ("group-id", number(group as f64)),
    ]);
    h.command("instance-create", payload);
    h.sync();
    h.eval_graph("(def g (first (graphs))) (def n2 (nth g.nodes 2))");
    let owner = h.graph_instance("g.owner");
    assert_eq!(h.rt().instance_kind(owner), Some(GROUP));
    assert_eq!(h.eval_graph("g.owner.gid"), number(group as f64));
    // Routes are member indices: the kinds show the member's track.
    h.eval_graph("(graph-node g.gid 2 :route 1)");
    h.sync();
    let track = members[1] as f64;
    assert_eq!(h.eval_graph("n2.route.index"), number(track));
    assert_eq!(
        h.eval_graph("(graph-node-value g.gid 2 :route)"),
        number(1.0)
    );
    // A set! names a track; the host stores its member index.
    h.eval_graph(&format!("(set! n2.route (track {}))", members[0]));
    h.graph_drain();
    assert_eq!(
        h.eval_graph("(graph-node-value g.gid 2 :route)"),
        number(0.0)
    );
    assert_eq!(h.eval_graph("n2.route.index"), number(members[0] as f64));
    let outside = (0..h.app.tracks.len()).find(|track| !members.contains(track));
    if let Some(outside) = outside {
        h.graph_rejects(
            &format!("(set! n2.route (track {outside}))"),
            "not a member of the graph's rack",
        );
    }
    h.eval_graph("(set! n2.route nil)");
    h.graph_drain();
    assert_eq!(h.eval_graph("n2.route"), Value::Nil);
}

#[test]
fn graph_undo_restores_only_the_edited_field() {
    let mut h = Harness::new();
    h.neural("nn");
    h.eval_graph("(def g (graph-of nn)) (def n1 (nth g.nodes 1))");
    let set = |h: &mut Harness, code: &str| {
        h.eval_graph(code);
        h.graph_drain();
    };
    let legacy = |h: &mut Harness, code: &str| {
        h.eval_graph(code);
        h.sync();
    };
    let delay = h.eval_graph("n1.delay");
    let entries = h.app.history.undo_len();
    // A node field: undo leaves a later legacy write to another field.
    set(&mut h, "(set! n1.delay 7)");
    legacy(&mut h, "(graph-node nn 1 :route 2)");
    h.graph_undo();
    assert_eq!(h.eval_graph("n1.delay"), delay);
    assert_eq!(
        h.eval_graph("(graph-node-value nn 1 :route)"),
        number(2.0),
        "the route is kept"
    );
    h.graph_redo();
    assert_eq!(
        h.eval_graph("(list n1.delay (graph-node-value nn 1 :route))"),
        h.eval_graph("(list 7 2)")
    );
    // A config field.
    let poly = h.eval_graph("g.max-poly");
    set(&mut h, "(set! g.max-poly 6)");
    legacy(&mut h, "(graph-config nn :reset-bars 3)");
    h.graph_undo();
    assert_eq!(h.eval_graph("g.max-poly"), poly);
    assert_eq!(h.eval_graph("g.reset-bars"), number(3.0), "reset-bars kept");
    h.graph_redo();
    assert_eq!(
        h.eval_graph("(list g.max-poly g.reset-bars)"),
        h.eval_graph("(list 6 3)")
    );
    // A group matrix cell: the matrix's other cells are kept.
    set(&mut h, "(set-group-gain! g 0 1 1.5)");
    legacy(&mut h, "(graph-config nn :group-gain-2-3 0.5)");
    h.graph_undo();
    assert_eq!(
        h.eval_graph("(list (nth g.group-gain 1) (nth g.group-gain (+ (* 2 4) 3)))"),
        h.eval_graph("(list 1 0.5)")
    );
    h.graph_redo();
    assert_eq!(
        h.eval_graph("(list (graph-config-value nn :group-gain-0-1) (graph-config-value nn :group-gain-2-3))"),
        h.eval_graph("(list 1.5 0.5)")
    );
    assert_eq!(h.app.history.undo_len(), entries + 3);
    // An undone first cell edit leaves no matrix override behind.
    set(&mut h, "(set-group-coupling! g 3 3 1)");
    h.graph_undo();
    let overrides = h.app.state.current_graph_overrides();
    assert!(overrides.iter().all(|graph| graph.group_coupling.is_none()));

    // A drag over two fields records one entry per field.
    let (delay, energy) = (h.eval_graph("n1.delay"), h.eval_graph("n1.seed-on-reset"));
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    set(&mut h, "(set! n1.delay 2)");
    set(&mut h, "(set! n1.delay 3)");
    set(&mut h, "(set! n1.seed-on-reset 0.25)");
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 2, "one entry per field");
    h.graph_undo();
    assert_eq!(
        h.eval_graph("(list n1.delay n1.seed-on-reset)"),
        h.eval_graph(&format!("(list 3 {})", num(energy)))
    );
    h.graph_undo();
    assert_eq!(h.eval_graph("n1.delay"), delay);
}

#[test]
fn gate_generator_takes_a_generator_of_the_graphs_owner() {
    let mut h = Harness::new();
    h.neural("nn");
    h.eval("(def-kind gen :generator (:resolution :16 :tick nil))");
    let gen = h.create_instance("scratch:gen");
    h.eval_graph("(def g (graph-of nn)) (def n1 (nth g.nodes 1))");
    h.graph_rejects(
        "(gate-generator! n1 nn.id)",
        "a generator instance of the graph's owner",
    );
    let other = h.create_instance(NEURAL);
    h.graph_rejects(
        &format!("(gate-generator! n1 {other})"),
        "a generator instance of the graph's owner",
    );
    h.eval_graph(&format!("(gate-generator! n1 {gen} :restart true)"));
    h.graph_drain();
    assert_eq!(
        h.eval_graph("(list n1.generator n1.restart n1.route)"),
        h.eval_graph(&format!("(list {gen} true nil)"))
    );
}

#[test]
fn graph_params_keep_their_handles_by_name_across_re_evaluation() {
    let mut h = Harness::new();
    let sequencer = |params: &str| {
        format!(
            r#"(def-sequencer "pp"
                 :shape (line 2)
                 :energy-decay 1
                 :reset-every 0
                 :seed-on-reset 0
                 :max-poly 4
                 (def-node nrn
                   :resolution :16
                   :delay 1
                   :quantize :16
                   :route 0
                   :seed-from ()
                   :reduce :sum
                   :params ({params})
                   :state ((energy :leak (per-step :energy-decay)))
                   :update (>= (node-state self :energy) (node-param self :alpha)))
                 (edges :from nrn :to nrn :topology (all-to-all) :gather (edge :weight)
                   :params ((weight :float -1 1 :default 1))))"#
        )
    };
    h.eval(&sequencer(
        "(alpha :float 0 4 :default 0.5) (beta :float 0 1 :default 0.25) (gamma :int 0 8 :default 2)",
    ));
    h.sync();
    h.eval_graph(
        r#"(def g (first (filter (lambda (g) (= g.name "pp")) (graphs))))
           (def n0 (nth g.nodes 0))
           (def pa (graph-param-named n0 "alpha"))
           (def pb (graph-param-named n0 "beta"))
           (def pc (graph-param-named n0 "gamma"))"#,
    );
    let (g, pa, pb, pc) = (
        h.graph_instance("g"),
        h.graph_instance("pa"),
        h.graph_instance("pb"),
        h.graph_instance("pc"),
    );
    // Reordered, beta renamed to delta.
    h.eval(&sequencer(
        "(gamma :int 0 8 :default 2) (alpha :float 0 4 :default 0.5) (delta :float 0 1 :default 0.75)",
    ));
    h.sync();
    assert_eq!(
        h.graph_instance(r#"(first (filter (lambda (g) (= g.name "pp")) (graphs)))"#),
        g
    );
    assert!(h.rt().instance_is_live(pa) && h.rt().instance_is_live(pc));
    assert!(!h.rt().instance_is_live(pb), "a renamed param goes stale");
    assert_eq!(
        h.eval_graph("(list pa.name pa.index pc.name pc.index)"),
        h.eval_graph(r#"(list "alpha" 1 "gamma" 0)"#)
    );
    assert_eq!(
        h.eval_graph("(map (lambda (p) p.name) n0.params)"),
        h.eval_graph(r#"(list "gamma" "alpha" "delta")"#)
    );
    assert_eq!(h.graph_instance("(nth n0.params 1)"), pa);
}

/// Node 2 of `nn`'s patch built through the legacy natives (rand wired into
/// cmp's `a`, fanned out into mask's `prob`; node 1 one prob slot), and
/// the kinds' handles on them.
const NODE_PATCH: &str = r#"(def rand-id (graph-node-process-add nn 2 "lane-rand"))
    (def cmp-id (graph-node-process-add nn 2 "lane-cmp"))
    (def mask-id (graph-node-process-add nn 2 "prob-mask"))
    (graph-node-process-wire nn 2 rand-id :wire cmp-id :a)
    (graph-node-process-fanout-add nn 2 rand-id :wire mask-id :prob)
    (def prob-id (graph-node-process-add nn 1 "lane-prob"))"#;

const NODE_HANDLES: &str = r#"(def g (graph-of nn)) (def n1 (nth g.nodes 1)) (def n2 (nth g.nodes 2))
    (def part (lambda (xs name) (first (filter (lambda (x) (= x.name name)) xs))))
    (def rnd (nth n2.processes 0)) (def cmp (nth n2.processes 1)) (def mask (nth n2.processes 2))
    (def lo (part rnd.inlets "lo")) (def roll (part rnd.inlets "roll"))
    (def out (part rnd.ports "out")) (def wire (part rnd.ports "wire"))"#;

impl Harness {
    /// A `neural` instance `nn` with the library published and
    /// [`NODE_PATCH`] built.
    fn node_patch() -> Self {
        let mut h = Self::new();
        h.publish_library();
        h.neural("nn");
        h.eval_graph(NODE_PATCH);
        // The natives' history entries land with the host commands.
        h.graph_drain();
        h
    }

    /// Node `node`'s chain as the legacy native reads it.
    fn node_chain(&mut self, node: usize) -> Vec<Value> {
        items(&self.eval_graph(&format!("(graph-node-process-chain nn {node})")))
    }
}

#[test]
fn node_processes_read_like_the_legacy_node_patch() {
    let mut h = Harness::node_patch();
    // Nothing reads a node's processes: none is registered.
    let registered = |h: &Harness| h.frame.host_kinds.shared.borrow().lanes.nodes.len();
    assert_eq!(registered(&h), 0);
    h.eval_graph(NODE_HANDLES);
    h.sync();
    assert_eq!(registered(&h), 1, "only the node read");
    let legacy = h.node_chain(2);
    assert_eq!(
        h.eval_graph("(len n2.processes)"),
        number(legacy.len() as f64)
    );
    for (index, slot) in legacy.iter().enumerate() {
        let p = format!("(nth n2.processes {index})");
        assert_eq!(
            h.eval_graph(&format!(
                "(let ((p {p})) (list p.proc-id p.class-name p.name p.enabled p.expr p.known \
                                      p.promoted-expr p.as-expr-reason p.index (= p.node n2) p.track \
                                      (len p.lanes)))"
            )),
            list_value([
                get(slot, "instance-id"),
                get(slot, "class"),
                get(slot, "label"),
                get(slot, "enabled"),
                get(slot, "expr"),
                get(slot, "known"),
                get(slot, "promoted-expr"),
                get(slot, "as-expr-reason"),
                number(index as f64),
                Value::Bool(true),
                Value::Nil,
                number(0.0),
            ]),
            "{p}"
        );
        // Every inlet is a scalar on a node (the class's lane inlets too).
        let defs = items(&get(slot, "inlet-defs"));
        let inlets = h.eval_graph(&format!(
            "(let ((p {p})) (map (lambda (i) (list i.name i.type i.value i.options)) p.inlets))"
        ));
        let expected = defs.iter().map(|def| {
            let options = match get(def, "options") {
                Value::Nil => list_value([]),
                options => options,
            };
            list_value([
                get(def, "name"),
                get(def, "kind"),
                get(def, "value"),
                options,
            ])
        });
        assert_eq!(inlets, list_value(expected), "{p}.inlets");
    }
    assert_eq!(
        h.eval_graph("(map (lambda (i) i.name) rnd.inlets)"),
        h.eval_graph(r#"(list "roll" "lo" "hi" "whole" "hold")"#)
    );
    // The wiring: the primary wire and the fan-out cable resolve to the
    // node's processes; the in ports are the legacy lane patch's.
    assert_eq!(
        h.eval_graph(
            "(let ((fo (first wire.fanout)))
               (list (= wire.target-process cmp) wire.target-inlet wire.connectable out.mappable \
                     (len wire.fanout) (= fo.target-process mask) fo.target-inlet))"
        ),
        h.eval_graph(r#"(list true "a" true true 1 true "prob")"#)
    );
    let in_ports = items(&h.eval_graph("(graph-node-lane-patch nn 2)"))
        .iter()
        .map(|entry| {
            list_value(
                items(&get(entry, "in-ports"))
                    .iter()
                    .map(|p| get(p, "name")),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        h.eval_graph("(map (lambda (p) p.in-ports) n2.processes)"),
        list_value(in_ports)
    );
    assert_eq!(
        h.eval_graph("(map (lambda (c) c.name) rnd.cells)"),
        h.eval_graph(r#"(list "held")"#)
    );
    // A node's process belongs to no track's chain.
    assert_eq!(
        h.eval_graph("(let ((t (track 0))) (len (filter (lambda (p) (= p rnd)) t.processes)))"),
        number(0.0)
    );

    // Legacy edits flow into the kinds at the next sync, on the same
    // instances.
    let rnd = h.graph_instance("rnd");
    h.eval_graph("(graph-node-process-enable nn 2 rand-id false)");
    h.eval_graph("(graph-node-process-inlet nn 2 rand-id :lo 3)");
    h.eval_graph("(graph-node-process-move nn 2 mask-id -2)");
    h.sync();
    assert_eq!(
        h.eval_graph("(list rnd.enabled lo.value rnd.index mask.index)"),
        h.eval_graph("(list false 3 1 0)")
    );
    assert_eq!(h.graph_instance("(nth n2.processes 1)"), rnd);
    // Removing a slot drops its instance and the cables into it.
    let mask = h.graph_instance("mask");
    h.eval_graph("(graph-node-process-remove nn 2 mask-id)");
    h.sync();
    assert!(!h.rt().instance_is_live(mask));
    assert_eq!(h.eval_graph("(len wire.fanout)"), number(0.0));

    // The latest run error of a slot reads under its own id (the node
    // runner's).
    let rand_id = num(h.eval_graph("rand-id")) as u64;
    let errors = std::collections::BTreeMap::from([(rand_id, "boom".to_string())]);
    h.shared.state.publish_process_run_errors(errors);
    h.sync();
    assert_eq!(h.eval_graph("rnd.error"), s("boom"));
}

#[test]
fn node_process_setters_record_the_node_chain_and_follow_the_value_rule() {
    let mut h = Harness::node_patch();
    h.eval_graph(NODE_HANDLES);
    h.sync();
    let entries = h.app.history.undo_len();
    let set = |h: &mut Harness, code: &str| {
        h.eval_graph(code);
        h.graph_drain();
    };
    set(&mut h, "(set! rnd.enabled false)");
    assert_eq!(get(&h.node_chain(2)[0], "enabled"), Value::Bool(false));
    assert_eq!(h.app.history.undo_len(), entries + 1, "one entry");
    // The current value is no edit.
    set(&mut h, "(set-process-enabled! rnd false)");
    assert_eq!(h.app.history.undo_len(), entries + 1);
    set(&mut h, "(set! lo.value -5)");
    // A gate takes a bool too (set! itself takes the field's :number).
    set(&mut h, "(set-inlet! roll false)");
    assert_eq!(
        h.eval_graph("(list lo.value roll.value)"),
        h.eval_graph("(list -5 0)")
    );
    set(&mut h, "(bind-port! out \"Vel\")");
    assert_eq!(h.eval_graph("out.target-step-param"), s("velocity"));
    assert_eq!(
        get(&items(&get(&h.node_chain(2)[0], "ports"))[0], "mapped-to"),
        s("velocity")
    );
    // `delay` is a payload field, not a step param.
    set(&mut h, "(bind-port! out \"delay\")");
    assert_eq!(h.eval_graph("out.target-step-param"), s("delay"));
    assert_eq!(
        get(&items(&get(&h.node_chain(2)[0], "ports"))[0], "mapped-to"),
        s("delay")
    );
    set(&mut h, "(bind-port! wire (part mask.inlets \"prob\"))");
    assert_eq!(
        h.eval_graph("(list (= wire.target-process mask) wire.target-inlet)"),
        h.eval_graph(r#"(list true "prob")"#)
    );
    set(&mut h, "(add-fanout! wire (part cmp.inlets \"a\"))");
    assert_eq!(h.eval_graph("(len wire.fanout)"), number(2.0));
    set(&mut h, "(let ((fo (nth wire.fanout 1))) (set! fo.hi 7))");
    assert_eq!(
        h.eval_graph("(let ((fo (nth wire.fanout 1))) fo.hi)"),
        number(7.0)
    );
    set(&mut h, "(remove-fanout! (first wire.fanout))");
    assert_eq!(
        h.eval_graph(
            "(let ((fo (first wire.fanout))) (list (len wire.fanout) (= fo.target-process cmp)))"
        ),
        h.eval_graph("(list 1 true)")
    );
    set(&mut h, "(unbind-port! wire)");
    assert_eq!(
        h.eval_graph("(list wire.disconnected wire.target-process)"),
        h.eval_graph("(list true nil)")
    );
    set(&mut h, "(clear-port! wire)");
    assert_eq!(h.eval_graph("wire.disconnected"), Value::Bool(false));
    // Structure: add (a fresh node slot id), move, remove.
    set(
        &mut h,
        r#"(add-process! n2 (first (filter (lambda (c) (= c.name "lane-acc")) process-library.classes)))"#,
    );
    h.eval_graph("(def acc (nth n2.processes 3))");
    assert_eq!(h.eval_graph("acc.class-name"), s("lane-acc"));
    assert!(
        num(h.eval_graph("acc.proc-id")) >= (1u64 << 45) as f64,
        "the node band"
    );
    let acc = h.graph_instance("acc");
    set(&mut h, "(move-process! acc rnd)");
    assert_eq!(
        h.eval_graph("(list acc.index rnd.index)"),
        h.eval_graph("(list 0 1)")
    );
    assert_eq!(
        h.graph_instance("(first n2.processes)"),
        acc,
        "a move keeps the instance"
    );
    set(&mut h, "(move-process! acc nil)");
    assert_eq!(h.eval_graph("acc.index"), number(3.0));
    set(&mut h, "(remove-process! acc)");
    assert!(!h.rt().instance_is_live(acc));
    let edited = h.app.history.undo_len();
    assert_eq!(edited, entries + 15, "one entry per edit");

    // The value rule, and a node's own scope: errors that record nothing.
    h.graph_rejects("(set! lo.value 200)", "a number from -128 to 128");
    h.graph_rejects("(set! roll.value 2)", "from 0 to 1");
    h.graph_rejects("(bind-port! out \"retrig\")", "a fire payload field");
    h.graph_rejects("(set-inlet! lo 1 :all true)", ":all edits a project lane");
    h.eval_graph(r#"(def prob (first n1.processes)) (def prob-in (part prob.inlets "prob"))"#);
    h.graph_rejects("(bind-port! wire prob-in)", "a port targets its own node");
    h.graph_rejects(
        "(move-process! rnd prob)",
        "a process moves within its own node",
    );
    h.graph_rejects(
        "(let ((t (track 0)) (p (first t.processes))) (bind-port! wire (first p.lanes)))",
        "a port targets its own node",
    );
    assert_eq!(h.app.history.undo_len(), edited);

    // Undo restores the node's chain one entry at a time.
    h.graph_undo();
    assert_eq!(h.eval_graph("(len n2.processes)"), number(4.0));
    for _ in 0..14 {
        h.graph_undo();
    }
    assert_eq!(h.app.history.undo_len(), entries);
    assert_eq!(
        h.eval_graph("(list rnd.enabled lo.value (= wire.target-process cmp) (len wire.fanout))"),
        h.eval_graph("(list true 0 true 1)")
    );
    h.graph_redo();
    assert_eq!(h.eval_graph("rnd.enabled"), Value::Bool(false));

    // An inlet drag's set!s join one entry, as the legacy picker's.
    let before = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for v in [1.0, 2.0, 3.0] {
        set(&mut h, &format!("(set! lo.value {v})"));
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), before + 1, "one drag entry");
    h.graph_undo();
    assert_eq!(
        h.eval_graph("lo.value"),
        number(0.0),
        "the value before the drag"
    );
}

#[test]
fn node_processes_sync_with_their_node_and_go_with_it() {
    let mut h = Harness::node_patch();
    h.eval_graph(NODE_HANDLES);
    h.sync();
    let process_syncs = |h: &Harness| h.frame.host_kinds.shared.borrow().lanes.process_syncs;
    let synced = process_syncs(&h);
    // Idle ticks, an edit to another node's chain, a param edit: no node's
    // processes re-derive.
    for _ in 0..3 {
        h.sync();
    }
    h.eval_graph("(graph-node-process-enable nn 1 prob-id false)");
    h.eval_graph("(graph-param nn 2 :threshold 1.5)");
    h.sync();
    assert_eq!(process_syncs(&h), synced, "node 2's chain did not move");
    // Its own chain moving re-derives it once.
    h.eval_graph("(graph-node-process-enable nn 2 cmp-id false)");
    h.sync();
    h.sync();
    assert_eq!(process_syncs(&h), synced + 3);
    let rnd = h.graph_instance("rnd");
    // A node-count change dropping the node drops its processes.
    h.eval_graph("(set! g.node-count 2)");
    h.graph_drain();
    assert!(
        !h.rt().instance_is_live(rnd),
        "the node's processes go with it"
    );
    assert!(h.frame.host_kinds.shared.borrow().lanes.nodes.is_empty());
}

#[test]
fn node_process_errors_and_scopes_are_live_observed_gated_and_drop_with_their_slot() {
    let mut h = Harness::node_patch();
    h.eval_graph(NODE_HANDLES);
    h.eval_graph("(def held (first rnd.cells)) (def hit (first cmp.cells))");
    h.sync();
    // A node slot runs under its own id (the node runner's runtime id).
    let rand_id = num(h.eval_graph("rand-id")) as u64;
    let cmp_id = num(h.eval_graph("cmp-id")) as u64;
    let scopes = |rand: Vec<f32>, hit: Vec<f32>| {
        HashMap::from([
            (rand_id, HashMap::from([("held".to_string(), rand)])),
            (cmp_id, HashMap::from([("hit".to_string(), hit)])),
        ])
    };
    let state = h.shared.state.clone();
    state.publish_process_scope_values(scopes(vec![1.0, 4.0], vec![0.0]));
    state.publish_process_run_errors(std::collections::BTreeMap::from([(
        cmp_id,
        "cmp failed".to_string(),
    )]));
    // Unobserved: the tick computes nothing; a cold read asks the host.
    h.sync();
    assert_eq!(h.computed(f::STATE_CELL_VALUES), 0);
    assert_eq!(h.computed(f::PROCESS_ERROR), 0);
    assert_eq!(h.eval_graph("held.values"), h.eval_graph("(list 1 4)"));
    assert_eq!(h.eval_graph("cmp.error"), s("cmp failed"));
    assert_eq!(h.eval_graph("rnd.error"), s(""));
    let cold = (
        h.computed(f::STATE_CELL_VALUES),
        h.computed(f::PROCESS_ERROR),
    );
    h.sync();
    assert_eq!(
        (
            h.computed(f::STATE_CELL_VALUES),
            h.computed(f::PROCESS_ERROR)
        ),
        cold,
        "unobserved: the tick reads nothing"
    );

    // Observed: pushed when the scheduler's scopes or run errors move, not
    // every tick.
    h.eval_graph(r#"(effect-buffer "*node-bay*" (label (str (len held.values) rnd.error)))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    let held = h.graph_instance("held");
    assert!(h.rt().host_field_observed(held, "values"));
    h.sync();
    let observed = (
        h.computed(f::STATE_CELL_VALUES),
        h.computed(f::PROCESS_ERROR),
    );
    h.sync();
    h.sync();
    assert_eq!(
        (
            h.computed(f::STATE_CELL_VALUES),
            h.computed(f::PROCESS_ERROR)
        ),
        observed,
        "nothing moved"
    );
    state.publish_process_scope_values(scopes(vec![1.0, 4.0, 7.0], vec![1.0]));
    h.sync();
    assert!(h.computed(f::STATE_CELL_VALUES) > observed.0);
    assert_eq!(h.eval_graph("held.values"), h.eval_graph("(list 1 4 7)"));
    // hit is not observed: the loop left it alone, a read asks again.
    assert_eq!(h.eval_graph("hit.values"), h.eval_graph("(list 1)"));
    state.publish_process_run_errors(std::collections::BTreeMap::from([(
        rand_id,
        "rand failed".to_string(),
    )]));
    h.sync();
    assert_eq!(h.eval_graph("rnd.error"), s("rand failed"));
    state.publish_process_run_errors(std::collections::BTreeMap::new());
    h.sync();
    assert_eq!(h.eval_graph("rnd.error"), s(""), "a clean run clears it");

    // A removed slot's process and cells go stale, and the loop reads
    // nothing more for them.
    let rnd = h.graph_instance("rnd");
    h.eval_graph("(graph-node-process-remove nn 2 rand-id)");
    h.graph_drain();
    assert!(!h.rt().instance_is_live(rnd) && !h.rt().instance_is_live(held));
    let gone = (
        h.computed(f::STATE_CELL_VALUES),
        h.computed(f::PROCESS_ERROR),
    );
    state.publish_process_scope_values(scopes(vec![9.0], vec![2.0]));
    state.publish_process_run_errors(std::collections::BTreeMap::from([(
        rand_id,
        "late".to_string(),
    )]));
    h.sync();
    assert_eq!(
        (
            h.computed(f::STATE_CELL_VALUES),
            h.computed(f::PROCESS_ERROR)
        ),
        gone,
        "no live field of a removed slot is computed"
    );
    // So does a dropped node's.
    let hit = h.graph_instance("hit");
    h.eval_graph("(set! g.node-count 2)");
    h.graph_drain();
    assert!(!h.rt().instance_is_live(hit));
}

/// A legacy event map (`event-history`, `node-events`)
/// as the kinds' positional row: [`ROW_FIELDS`] in order, nil as -1.
fn legacy_event_row(event: &Value) -> Value {
    list_value(ROW_FIELDS.iter().map(|field| match get(event, field) {
        Value::Nil => number(-1.0),
        value => value,
    }))
}

fn legacy_event_rows(events: &Value) -> Value {
    list_value(items(events).iter().map(legacy_event_row))
}

/// A legacy column matrix (`((a) (b) …)`) as a flat list.
fn legacy_column(column: &Value) -> Value {
    list_value(items(column).iter().map(|row| items(row)[0].clone()))
}

/// The first row of `id`'s list `field` as pushed (its identity shows
/// whether a tick pushed the list again).
fn first_row(h: &Harness, id: InstanceId, field: &str) -> Rc<RefCell<Value>> {
    match h.rt().instance_field(id, field) {
        Ok(Value::List(rows)) => rows[0].clone(),
        other => panic!("{field}: {other:?}"),
    }
}

/// Node `node`'s event at `beat` (track 3 for an odd node, none for an
/// even one), its transpose and velocity past the display transforms.
fn graph_event(node: usize, beat: f64) -> GraphVisualizationEvent {
    GraphVisualizationEvent {
        node_index: node,
        track: (node % 2 == 1).then_some(3),
        sample_time: (beat * 1000.0) as u64,
        beat,
        transpose: -7.256,
        velocity: 1.5,
    }
}

#[test]
fn graph_event_streams_read_like_the_legacy_visualization_and_skip_an_unchanged_stamp() {
    let mut h = Harness::new();
    h.neural("nn");
    h.eval_graph("(def g (graph-of nn))");
    let gid = num(h.eval_graph("g.gid")) as u64;
    let snapshot = |stamp: u64, history: &[GraphVisualizationEvent]| GraphVisualizationSnapshot {
        id: gid,
        name: "nn".to_string(),
        active: true,
        current_beat: 4.0,
        num_nodes: 2,
        node_events: vec![None, history.last().copied()],
        event_history: history.to_vec(),
        history_stamp: stamp,
        node_events_stamp: stamp,
        deltas: vec![
            GraphDeltaEntry {
                key: GraphDeltaKey::EdgeParam {
                    from: 0,
                    to: 1,
                    param: "weight".to_string(),
                },
                delta: -0.25,
            },
            GraphDeltaEntry {
                key: GraphDeltaKey::NodeParam {
                    node: 1,
                    param: "threshold".to_string(),
                },
                delta: 2.0,
            },
            GraphDeltaEntry {
                key: GraphDeltaKey::NodeDelay { node: 1 },
                delta: -0.5,
            },
        ],
        group_activity: vec![0.75, 0.0, 0.0, 0.0],
        group_suppression: vec![0.0, -0.375, 0.0, 0.0],
        ..Default::default()
    };
    let history = [graph_event(0, 1.0), graph_event(1, 2.0)];
    let state = h.shared.state.clone();
    state.set_graph_visualizations(vec![snapshot(7, &history)]);
    for _ in 0..3 {
        h.sync();
    }
    let streams = [
        f::GRAPH_EVENTS,
        f::GRAPH_NODE_EVENTS,
        f::GRAPH_DELTAS,
        f::GRAPH_NODE_DELTAS,
        f::GRAPH_GROUP_ACTIVITY,
        f::GRAPH_GROUP_SUPPRESSION,
    ];
    for key in streams {
        assert_eq!(h.computed(key), 0, "{key:?} unobserved");
    }

    // Cold reads: the legacy `SEQ.graph-visualizations` entry, as rows.
    let legacy = items(&build_graph_visualizations_value(&state))[0].clone();
    assert_eq!(
        h.eval_graph("g.events"),
        legacy_event_rows(&get(&legacy, "event-history"))
    );
    let node_events = items(&get(&legacy, "node-events"));
    let node_rows = list_value(node_events.iter().map(|event| match event {
        Value::Nil => list_value(std::iter::empty()),
        event => legacy_event_row(event),
    }));
    assert_eq!(h.eval_graph("g.node-events"), node_rows);
    // The legacy `events` are the nodes' that show one.
    assert_eq!(
        h.eval_graph("(filter (lambda (row) (> (len row) 0)) g.node-events)"),
        legacy_event_rows(&get(&legacy, "events"))
    );
    assert_eq!(h.eval_graph("g.deltas"), get(&legacy, "delta-matrix"));
    assert_eq!(
        h.eval_graph("g.node-deltas"),
        legacy_column(&get(&legacy, "node-delta-column"))
    );
    assert_eq!(
        h.eval_graph("g.group-activity"),
        legacy_column(&get(&legacy, "group-activity-matrix"))
    );
    assert_eq!(
        h.eval_graph("g.group-suppression"),
        legacy_column(&get(&legacy, "group-suppression-matrix"))
    );
    // Rows: no track is -1; a node's latest shows the display transforms.
    let row = |cells: [f64; 5]| list_value(cells.map(number));
    assert_eq!(
        h.eval_graph("(list (first g.events) (nth g.node-events 1))"),
        list_value([
            row([0.0, -1.0, 1.0, -7.256_f32 as f64, 1.5]),
            row([1.0, 3.0, 2.0, -7.26, 1.0]),
        ])
    );

    // Observed: pushed, then an idle tick reads the stamp alone.
    h.eval_graph(
        r#"(effect-buffer "*events*" (label (str (len g.events) (len g.node-events) (len g.deltas) (len g.node-deltas) (len g.group-activity) (len g.group-suppression))))"#,
    );
    h.editor.runtime_mut().run_reactive_cycle();
    h.sync();
    let g = h.graph_instance("g");
    for key in streams {
        assert!(h.computed(key) > 0, "{key:?} observed");
    }
    let pushed = first_row(&h, g, "events");
    // Same stamp, other history: not read (the stamp says nothing moved).
    state.set_graph_visualizations(vec![snapshot(7, &[graph_event(1, 9.0)])]);
    for _ in 0..3 {
        h.sync();
    }
    assert!(
        Rc::ptr_eq(&pushed, &first_row(&h, g, "events")),
        "no push while idle"
    );
    assert_eq!(h.eval_graph("(len g.events)"), number(2.0));
    // Playing: a new stamp pushes the new history and latest events.
    let played = [
        graph_event(0, 1.0),
        graph_event(1, 2.0),
        graph_event(0, 3.0),
    ];
    state.set_graph_visualizations(vec![snapshot(8, &played)]);
    h.sync();
    let legacy = items(&build_graph_visualizations_value(&state))[0].clone();
    assert_eq!(
        h.eval_graph("g.events"),
        legacy_event_rows(&get(&legacy, "event-history"))
    );
    assert_eq!(
        h.eval_graph("(first (nth g.node-events 1))"),
        number(0.0),
        "node 1 shows the latest event"
    );
    // The history dropping its oldest row.
    let shifted = [
        graph_event(1, 2.0),
        graph_event(0, 3.0),
        graph_event(1, 4.0),
    ];
    state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        history_stamp: 30,
        event_history: shifted.to_vec(),
        ..snapshot(8, &played)
    }]);
    h.sync();
    assert_eq!(h.eval_graph("(nth (nth g.events 2) 2)"), number(4.0));
    // A node event expiring re-pushes node-events alone.
    let nodes_pushed = first_row(&h, g, "node-events");
    let events_pushed = first_row(&h, g, "events");
    state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        history_stamp: 30,
        event_history: shifted.to_vec(),
        node_events: vec![None, None],
        node_events_stamp: 31,
        ..snapshot(8, &played)
    }]);
    h.sync();
    assert!(!Rc::ptr_eq(&nodes_pushed, &first_row(&h, g, "node-events")));
    assert_eq!(
        h.eval_graph("g.node-events"),
        h.eval_graph("(list (list) (list))")
    );
    assert!(
        Rc::ptr_eq(&events_pushed, &first_row(&h, g, "events")),
        "events not re-pushed"
    );
    // A reset clears them.
    state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        node_events: vec![None, None],
        ..snapshot(9, &[])
    }]);
    h.sync();
    assert_eq!(
        h.eval_graph("(list g.events g.node-events)"),
        h.eval_graph("(list (list) (list (list) (list)))")
    );
    // Before the scheduler runs the graph: none, sized to its nodes.
    state.set_graph_visualizations(Vec::new());
    h.sync();
    assert_eq!(
        h.eval_graph("(list (len g.events) (len g.node-events) g.group-activity)"),
        h.eval_graph("(list 0 (len g.nodes) (list 0 0 0 0))")
    );
}

#[test]
fn transport_track_events_read_the_track_output_and_skip_an_unchanged_revision() {
    let mut h = Harness::new();
    let state = h.shared.state.clone();
    let event = |track: usize, beat: f64| TrackOutputEvent {
        track,
        sample_time: (beat * 1000.0) as u64,
        beat,
        transpose: 7.0,
        velocity: 0.5,
        ..Default::default()
    };
    state.set_track_output_current_beat(2.0);
    state.append_track_output_events([event(1, 1.5)]);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(h.computed(f::TRANSPORT_TRACK_EVENTS), 0, "unobserved");
    // Cold reads: the tracks' output, as rows (no node: -1).
    assert_eq!(
        h.eval_all("transport.track-events"),
        h.eval_all("(list (list -1 1 1.5 7 0.5))")
    );
    assert_eq!(h.eval_all("transport.track-events-beat"), number(2.0));

    h.eval_all(
        r#"(effect-buffer "*track-events*" (label (str (len transport.track-events) transport.track-events-beat)))"#,
    );
    h.editor.runtime_mut().run_reactive_cycle();
    h.sync();
    let transport = h.singleton(TRANSPORT);
    let pushed = first_row(&h, transport, "track-events");
    // Idle: the revision alone is read; an empty append moves nothing.
    let revision = state.track_output_events_revision();
    state.append_track_output_events([]);
    assert_eq!(state.track_output_events_revision(), revision);
    for _ in 0..3 {
        h.sync();
    }
    assert!(Rc::ptr_eq(
        &pushed,
        &first_row(&h, transport, "track-events")
    ));
    // Playing: an append and the scheduler's beat push.
    state.append_track_output_events([event(0, 3.0)]);
    state.set_track_output_current_beat(3.5);
    h.sync();
    assert_eq!(
        h.eval_all("transport.track-events"),
        h.eval_all("(list (list -1 1 1.5 7 0.5) (list -1 0 3 7 0.5))")
    );
    assert_eq!(h.eval_all("transport.track-events-beat"), number(3.5));
    state.clear_track_output_events();
    h.sync();
    assert_eq!(h.eval_all("transport.track-events"), h.eval_all("(list)"));
}

#[test]
fn row_history_builds_only_new_rows() {
    let cells = |history: &RowHistory| match history.value() {
        Value::List(cells) => cells,
        other => panic!("{other:?}"),
    };
    let row = |beat: f64| [0.0, -1.0, beat, 0.0, 1.0];
    let mut history = RowHistory::default();
    history.update(|rows| rows.extend([row(1.0), row(2.0)]));
    let first = cells(&history);
    // Grown at the end: the kept rows keep their cells.
    history.update(|rows| rows.extend([row(1.0), row(2.0), row(3.0)]));
    let grown = cells(&history);
    assert!(Rc::ptr_eq(&first[0], &grown[0]) && Rc::ptr_eq(&first[1], &grown[1]));
    // Dropped from the front: the rest keep theirs.
    history.update(|rows| rows.extend([row(2.0), row(3.0), row(4.0)]));
    let shifted = cells(&history);
    assert!(Rc::ptr_eq(&grown[1], &shifted[0]) && Rc::ptr_eq(&grown[2], &shifted[1]));
    assert_eq!(
        history.value(),
        list_value([2.0, 3.0, 4.0].map(|beat| numbers(&row(beat))))
    );
    // Reset and refilled: nothing kept.
    history.update(|rows| rows.push(row(9.0)));
    assert!(!shifted
        .iter()
        .any(|cell| Rc::ptr_eq(cell, &cells(&history)[0])));
    history.update(|_| ());
    assert_eq!(history.value(), list_value(std::iter::empty()));
}
