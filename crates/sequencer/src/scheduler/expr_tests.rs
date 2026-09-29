//! Expr cards on graph node patches, end to end (docs/expr-process-spec.md
//! §2, §2.1, §3, §10; bead eseq-waa9.10): the UI natives compile a body into
//! a hidden `expr#<hash>` class and rebind the slot, and the production
//! lookahead runs the node patch on the scheduler VM.

use super::{reconcile_graph_runtimes, schedule_playing_lookahead, LiveMidiFxTrackState, SchedulerLookaheadState};
use crate::graph::{
    ProjectGraphEdgeParamOverride, ProjectGraphNodeIntrinsicOverride, ProjectGraphOverrides,
    ProjectGraphSeedFrom,
};
use crate::lisp_host;
use crate::scheduled_event::{ScheduledEventKind, ScheduledEventQueue};
use crate::sequencer::{default_empty_effect_chain, SequencerState, StepParam, MAX_TRACKS};
use eseqlisp::vm::Value;
use eseqlisp::Runtime;
use std::sync::Arc;

const GRAPH: &str = "expr-graph";

fn run_with_scheduler_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("expr-process-harness".to_string())
        .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
        .spawn(f)
        .expect("spawn expr harness")
        .join()
        .expect("expr harness panicked")
}

/// Track 0 step 0 seeds node 0 with transpose 2; node 0 feeds node 1 (weight
/// 1), which routes to track 1. Node 1's patch is what the tests edit.
fn expr_graph_state() -> Arc<SequencerState> {
    expr_graph_state_seeded(&[0])
}

/// [`expr_graph_state`] with track 0 trigs (transpose 2) on `steps`, so node
/// 1 fires once per trig, one step later.
fn expr_graph_state_seeded(steps: &[usize]) -> Arc<SequencerState> {
    expr_graph_chain_state(2, steps)
}

/// A line of `nodes` nodes, each feeding the next (weight 1, one step), all
/// routed to track 1; track 0 trigs on `steps` seed node 0.
fn expr_graph_chain_state(nodes: usize, steps: &[usize]) -> Arc<SequencerState> {
    let state = Arc::new(SequencerState::new(
        2,
        (0..2).map(|_| default_empty_effect_chain()).collect(),
    ));
    state.toggle_play();
    for &step in steps {
        state.toggle_step_and_clear_plocks(0, step);
        state.set_step_param(0, step, StepParam::Transpose, 2.0);
    }
    let mut authoring = Runtime::new();
    let publish_state = Arc::clone(&state);
    authoring.register_native("def-sequencer", move |args, _ctx| {
        let published = lisp_host::published_sequencer_from_def_args(&args)?;
        let name = published.name.clone();
        publish_state.publish_sequencer(published);
        Ok(Value::String(name))
    });
    authoring
        .eval_str(
            &r#"
            (def-sequencer "expr-graph"
              :shape (line NODES)
              :energy-decay 1
              :reset-every 0
              :seed-on-reset 0
              :max-poly 8
              :max-poly-selection :deterministic
              :duration (steps 1)
              (def-node nrn
                :resolution :16
                :delay 1
                :quantize :16
                :route 1
                :seed-from 0
                :reduce :sum
                :params ((threshold :float 0 4 :default 0.5))
                :state ((energy :leak (per-step :energy-decay)))
                :update (if (>= (energy) (param :threshold))
                          (emit :note (in-note) :vel (in-vel))
                          nil))
              (edges
                :from nrn
                :to nrn
                :topology (all-to-all)
                :gather (edge :weight)
                :params ((weight :float -1 1 :default 0))))
            "#
            .replace("NODES", &nodes.to_string()),
        )
        .expect("publish graph");
    let published = state
        .published_sequencers()
        .into_iter()
        .find(|seq| seq.name == GRAPH)
        .expect("published graph");
    let manifest = published.graph.as_ref().expect("graph manifest");
    let edge_group = crate::graph::edge_set_group_id(&manifest.edge_sets[0]);
    let intrinsic = |instance: usize, seed: Option<ProjectGraphSeedFrom>| ProjectGraphNodeIntrinsicOverride {
        group: "nrn".to_string(),
        instance,
        resolution: None,
        delay_steps: None,
        quantize: None,
        route: None,
        seed_from: seed,
        seed_on_reset: None,
        duration: None,
        swing: None,
        neural_group: None,
        process_chain: None,
    };
    state
        .edit_current_graph_overrides(|graphs| {
            graphs.push(ProjectGraphOverrides {
                sequencer_id: published.id,
                sequencer_name: published.name.clone(),
                node_intrinsics: (0..nodes)
                    .map(|node| {
                        intrinsic(node, (node > 0).then(|| ProjectGraphSeedFrom::Tracks(Vec::new())))
                    })
                    .collect(),
                edge_params: (1..nodes)
                    .map(|to| ProjectGraphEdgeParamOverride {
                        group: edge_group.clone(),
                        from: to - 1,
                        to,
                        param: "weight".to_string(),
                        value: 1.0,
                    })
                    .collect(),
                ..Default::default()
            });
            Ok(())
        })
        .expect("install graph overrides");
    // The app's UI VM publishes the builtin process library when it loads it.
    let mut publisher = lisp_host::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
    publisher
        .eval(&lisp_host::load_process_library_source())
        .expect("publish builtin process library");
    state.publish_process_authoring(
        publisher.process_authoring_snapshot().to_published().expect("publishable"),
    );
    state
}

fn ui_runtime(state: &Arc<SequencerState>) -> Runtime {
    let mut ui = Runtime::new();
    lisp_host::register_graph_authoring_natives(&mut ui, Arc::clone(state));
    ui
}

fn eval(ui: &mut Runtime, source: &str) -> Value {
    ui.eval_str(source)
        .unwrap_or_else(|error| panic!("{source}: {error:?}"))
        .unwrap_or(Value::Nil)
}

fn add_slot(ui: &mut Runtime, class: &str) -> u64 {
    match eval(ui, &format!("(graph-node-process-add \"{GRAPH}\" 1 \"{class}\")")) {
        Value::Number(id) => id as u64,
        other => panic!("add {class}: {other:?}"),
    }
}

fn field(map: &Value, key: &str) -> Value {
    let Value::Map(map) = map else { panic!("expected a map, got {map:?}") };
    map.get(key).map(|value| value.borrow().clone()).unwrap_or(Value::Nil)
}

fn names(value: &Value) -> Vec<String> {
    let Value::List(items) = value else { return Vec::new() };
    items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::String(name) => name.clone(),
            other => panic!("expected a name, got {other:?}"),
        })
        .collect()
}

fn set_source(ui: &mut Runtime, id: u64, source: &str) -> Value {
    let quoted = format!("{source:?}");
    eval(ui, &format!("(graph-node-process-expr-set \"{GRAPH}\" 1 {id} {quoted})"))
}

fn node1_chain(state: &SequencerState) -> crate::process::TrackProcessChain {
    state
        .current_graph_overrides()
        .iter()
        .find(|graph| graph.sequencer_name == GRAPH)
        .and_then(|graph| graph.node_intrinsics.iter().find(|node| node.instance == 1))
        .and_then(|node| node.process_chain.clone())
        .unwrap_or_default()
}

/// One network trigger on track 1.
#[derive(Clone, Copy, Debug)]
struct Hit {
    sample: u64,
    neuron: usize,
    resolved: crate::accumulator::ResolvedStep,
}

/// Run one lookahead second on the scheduler VM (with the defs the worker
/// would sync: its scratch library merged with the UI-published snapshot,
/// which carries the compiled expr classes) and return node 1's emitted
/// transposes on track 1.
fn node1_transposes(state: &Arc<SequencerState>) -> Vec<f32> {
    track1_hits(state).into_iter().map(|hit| hit.resolved.transpose).collect()
}

/// The network triggers on track 1 over one lookahead second, in time order.
fn track1_hits(state: &Arc<SequencerState>) -> Vec<Hit> {
    track1_hit_passes(state, None).remove(0)
}

/// [`track1_hits`] for one pass, or, when `between` is given, two
/// consecutive lookahead seconds on the same scheduler (and process
/// runtime), with `between` run on the scheduler before the second.
fn track1_hit_passes(
    state: &Arc<SequencerState>,
    between: Option<&dyn Fn(&mut SchedulerLookaheadState)>,
) -> Vec<Vec<Hit>> {
    let snapshot = state.publish_scheduler_snapshot();
    let mut scheduler = SchedulerLookaheadState::new(48_000);
    let manifests = state
        .published_sequencers()
        .into_iter()
        .filter_map(|seq| seq.graph)
        .collect::<Vec<_>>();
    reconcile_graph_runtimes(
        manifests,
        &snapshot.graph_overrides,
        &[],
        &mut scheduler.graph_runtimes,
        &mut scheduler.graph_manifests,
        scheduler.clock.total_beats,
    );
    let mut scratch = lisp_host::scheduler_scratch_runtime_with_fallbacks(Arc::clone(state), 0, 0);
    scratch
        .eval(&lisp_host::load_midi_fx_library_source())
        .expect("load MIDI FX library");
    scratch
        .eval(&lisp_host::load_process_library_source())
        .expect("load builtin process library");
    scheduler.process_runtime.sync_authoring(
        crate::process::merge_authoring_snapshots(
            scratch.process_authoring_snapshot(),
            state.published_process_authoring().to_runtime(),
        ),
        0.0,
    );
    let mut scratch_runtime = Some(scratch);
    let queue = ScheduledEventQueue::<64>::new();
    let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
        std::array::from_fn(|_| LiveMidiFxTrackState::default());
    let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;
    let mut passes = Vec::new();
    let mut scheduled_until = 0;
    for pass in 0..if between.is_some() { 2 } else { 1 } {
        if pass > 0 {
            if let Some(between) = between {
                between(&mut scheduler);
            }
        }
        scheduled_until = schedule_playing_lookahead(
            &mut scheduler,
            state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            scheduled_until,
            48_000,
            48_000,
            12_000,
            samples_per_quarter,
            scheduled_until,
            false,
            false,
        )
        .scheduled_until_sample;
        let mut hits = Vec::new();
        while let Some(event) = queue.pop_owned() {
            if let ScheduledEventKind::NetworkTrigger { track: 1, resolved, source_neuron, .. } = event.kind {
                hits.push(Hit { sample: event.sample_time, neuron: source_neuron, resolved });
            }
        }
        hits.sort_by_key(|hit| hit.sample);
        passes.push(hits);
    }
    passes
}

fn assert_node1_transposes(state: &Arc<SequencerState>, expected: f32, why: &str) {
    let hits = node1_transposes(state);
    assert!(!hits.is_empty(), "node 1 fires: {why}");
    assert!(
        hits.iter().all(|transpose| (*transpose - expected).abs() < 1e-6),
        "{why}: expected transpose {expected}, got {hits:?}"
    );
}

#[test]
fn expr_set_source_compiles_a_hidden_class_whose_wire_drives_the_payload() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        assert_node1_transposes(&state, 2.0, "no patch: the seed's transpose");

        let expr = add_slot(&mut ui, "expr");
        let transpose = add_slot(&mut ui, "neural-transpose");
        // An empty expr card passes nothing.
        assert_node1_transposes(&state, 2.0, "empty expr + unwired transpose");

        let result = set_source(&mut ui, expr, "(* x rate)");
        assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
        assert_eq!(names(&field(&result, "inlets")), vec!["x", "rate"]);
        let Value::String(class) = field(&result, "class") else { panic!("class: {result:?}") };
        assert!(crate::process::is_expr_process_class(&class), "{class}");

        // Rebound in place: same id, same position, source stored, the
        // hashed class hidden from the picker and labelled `expr`.
        let chain = node1_chain(&state);
        assert_eq!(chain.slots[0].instance_id.0, expr);
        assert_eq!(chain.slots[0].class_name, class);
        assert_eq!(chain.slots[0].expr_source.as_deref(), Some("(* x rate)"));
        let classes = eval(&mut ui, "(graph-node-process-classes)");
        let Value::List(classes) = classes else { panic!("classes") };
        let listed: Vec<String> = classes
            .iter()
            .map(|entry| match field(&entry.borrow(), "class") {
                Value::String(name) => name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(listed.iter().any(|name| name == "expr"), "plain expr is offered");
        assert!(!listed.iter().any(|name| crate::process::is_expr_process_class(name)));
        let read = eval(&mut ui, &format!("(first (graph-node-process-chain \"{GRAPH}\" 1))"));
        assert_eq!(field(&read, "label"), Value::String("expr".to_string()));
        assert_eq!(field(&read, "expr-source"), Value::String("(* x rate)".to_string()));
        assert_eq!(field(&read, "error"), Value::Nil);
        assert_eq!(
            eval(&mut ui, &format!("(graph-node-process-expr-source \"{GRAPH}\" 1 {expr})")),
            Value::String("(* x rate)".to_string())
        );

        // x = 2, rate = 3, wire -> transpose.amount: +6 on the payload.
        eval(&mut ui, &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {expr} :x 2)"));
        eval(&mut ui, &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {expr} :rate 3)"));
        eval(
            &mut ui,
            &format!("(graph-node-process-wire \"{GRAPH}\" 1 {expr} \"wire\" {transpose} \"amount\")"),
        );
        assert_node1_transposes(&state, 8.0, "2 + (* 2 3) through the wire");

        // Identical source elsewhere shares the class.
        let twin = add_slot(&mut ui, "expr");
        // (eseqlisp strings have no escapes, so whitespace-only here; the
        // comment/newline normalization is pinned in the unit tests.)
        let twin_result = set_source(&mut ui, twin, "  (*   x   rate)  ");
        assert_eq!(field(&twin_result, "class"), Value::String(class.clone()));
    });
}

#[test]
fn expr_recommit_reconciles_inlets_by_name_and_keeps_surviving_cables() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let source = add_slot(&mut ui, "expr");
        let shaper = add_slot(&mut ui, "expr");
        assert_eq!(field(&set_source(&mut ui, source, "3"), "ok"), Value::Bool(true));
        assert_eq!(field(&set_source(&mut ui, shaper, "(* x k)"), "ok"), Value::Bool(true));
        eval(&mut ui, &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {shaper} :k 2)"));
        eval(
            &mut ui,
            &format!("(graph-node-process-wire \"{GRAPH}\" 1 {source} \"wire\" {shaper} \"x\")"),
        );
        eval(&mut ui, &format!("(graph-node-process-map \"{GRAPH}\" 1 {shaper} \"out\" :transpose)"));
        assert_node1_transposes(&state, 8.0, "2 + 3 * 2 mapped onto transpose");

        // `x` survives: its cable follows the slot onto the new class; `k`
        // is gone with its value.
        let result = set_source(&mut ui, shaper, "(+ x 1)");
        assert_eq!(field(&result, "ok"), Value::Bool(true));
        assert_eq!(names(&field(&result, "inlets")), vec!["x"]);
        assert_eq!(names(&field(&result, "removed")), vec!["k"]);
        let chain = node1_chain(&state);
        let shaper_slot = chain.slots.iter().find(|slot| slot.instance_id.0 == shaper).unwrap();
        assert!(!shaper_slot.inlets.contains_key("k"), "removed inlet drops its value");
        let Some(Some(crate::process::ParamTarget::ProcessInlet { process, inlet, .. })) =
            chain.slots[0].bindings.get("wire")
        else {
            panic!("the cable into x survives: {:?}", chain.slots[0].bindings);
        };
        assert_eq!((process.as_str(), inlet.as_str()), (shaper_slot.class_name.as_str(), "x"));
        assert!(
            matches!(shaper_slot.bindings.get("out"), Some(Some(crate::process::ParamTarget::StepParam { .. }))),
            "the card's own mapping survives the rebind"
        );
        assert_node1_transposes(&state, 6.0, "2 + (3 + 1) through the kept cable");

        // Renaming x drops the cable into it.
        let result = set_source(&mut ui, shaper, "(+ y 1)");
        assert_eq!(names(&field(&result, "removed")), vec!["x"]);
        let chain = node1_chain(&state);
        assert!(chain.slots[0].bindings.get("wire").is_none(), "cable into x dropped");
        assert_node1_transposes(&state, 3.0, "2 + (0 + 1): y starts at 0");
    });
}

#[test]
fn expr_bad_source_keeps_the_previous_class_running() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let expr = add_slot(&mut ui, "expr");
        set_source(&mut ui, expr, "(+ a 5)");
        eval(&mut ui, &format!("(graph-node-process-map \"{GRAPH}\" 1 {expr} \"out\" :transpose)"));
        let before = node1_chain(&state);
        assert_node1_transposes(&state, 7.0, "2 + 5");

        for bad in ["(sinn a)", "(+ a", "(let ((neuron 1)) (neuron 2))"] {
            let result = set_source(&mut ui, expr, bad);
            assert_eq!(field(&result, "ok"), Value::Bool(false), "{bad}: {result:?}");
            assert!(matches!(field(&result, "error"), Value::String(_)), "{bad}: {result:?}");
            assert_eq!(names(&field(&result, "inlets")), vec!["a"], "old inlets reported");
            assert_eq!(node1_chain(&state), before, "{bad}: slot untouched");
        }
        let result = set_source(&mut ui, expr, "(sinn a)");
        assert_eq!(
            field(&result, "span"),
            Value::List(vec![
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(1.0))),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Number(5.0))),
            ]),
            "span of the unknown function"
        );
        assert_node1_transposes(&state, 7.0, "old class still running");
    });
}

#[test]
fn expr_runtime_error_and_step_budget_bypass_only_that_card() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let expr = add_slot(&mut ui, "expr");
        let transpose = add_slot(&mut ui, "neural-transpose");
        eval(&mut ui, &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {transpose} :amount 7)"));
        eval(&mut ui, &format!("(graph-node-process-map \"{GRAPH}\" 1 {expr} \"out\" :transpose)"));

        for (body, why) in [
            ("(+ a (list 1))", "type error"),
            ("((lambda (f) (f f 1)) (lambda (g n) (g g (+ n a))))", "runaway recursion hits the step budget"),
        ] {
            let result = set_source(&mut ui, expr, body);
            assert_eq!(field(&result, "ok"), Value::Bool(true), "{body} compiles: {result:?}");
            // The erroring card writes nothing; the next card still runs.
            assert_node1_transposes(&state, 9.0, why);
            let error = state.process_run_error(expr);
            assert!(error.is_some(), "{why}: error published for the card");
            if body.contains("lambda") {
                assert!(error.unwrap().contains("StepBudgetExceeded"), "{why}");
            }
            let read = eval(&mut ui, &format!("(first (graph-node-process-chain \"{GRAPH}\" 1))"));
            assert!(matches!(field(&read, "error"), Value::String(_)), "{why}: chain read error dot");
        }

        // A clean body clears the error on its next run.
        set_source(&mut ui, expr, "(+ a 1)");
        assert_node1_transposes(&state, 10.0, "2 + 1 + 7");
        assert_eq!(state.process_run_error(expr), None);
    });
}

#[test]
fn expr_sources_round_trip_and_recompile_on_load() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let source = add_slot(&mut ui, "expr");
        let shaper = add_slot(&mut ui, "expr");
        set_source(&mut ui, source, "4");
        set_source(&mut ui, shaper, "(- x 1)");
        eval(
            &mut ui,
            &format!("(graph-node-process-wire \"{GRAPH}\" 1 {source} \"wire\" {shaper} \"x\")"),
        );
        eval(&mut ui, &format!("(graph-node-process-map \"{GRAPH}\" 1 {shaper} \"out\" :transpose)"));
        assert_node1_transposes(&state, 5.0, "2 + (4 - 1)");

        // Save: the override serializes the body, not just the hash.
        let saved = serde_json::to_string(&state.current_graph_overrides()).expect("serialize");
        assert!(saved.contains("\"expr_source\":\"(- x 1)\""), "{saved}");
        // Stale hashes in the file are re-derived from the body on load.
        let class = node1_chain(&state).slots[1].class_name.clone();
        let saved = saved.replace(&class, "expr#000000000000");
        let mut loaded: Vec<ProjectGraphOverrides> = serde_json::from_str(&saved).expect("deserialize");
        for node in &mut loaded[0].node_intrinsics {
            if let Some(chain) = node.process_chain.as_mut() {
                lisp_host::rederive_expr_class_names_in_chain(chain);
            }
        }

        // Load into a fresh session: nothing compiled until the sync.
        let fresh = expr_graph_state();
        fresh
            .edit_current_graph_overrides(|graphs| {
                *graphs = loaded;
                Ok(())
            })
            .expect("install loaded overrides");
        assert!(!fresh.has_expr_process_def(&class));
        assert!(lisp_host::sync_expr_process_classes(&fresh).is_empty());
        assert!(fresh.has_expr_process_def(&class));
        assert_eq!(node1_chain(&fresh), node1_chain(&state), "slots, wires and bodies survive");
        assert_node1_transposes(&fresh, 5.0, "recompiled after load");
    });
}

// ---- eseq-waa9.12: `$` context variables and direct writes (spec §4, §6) ----

/// Trigs on track 0 steps 0, 2, 4, 6: node 1 fires four times a second, at
/// steps 1, 3, 5, 7.
const FOUR_TRIGS: &[usize] = &[0, 2, 4, 6];
/// 120 bpm at 48 kHz.
const SAMPLES_PER_BEAT: f64 = 24_000.0;

fn node_eval(ui: &mut Runtime, node: usize, verb: &str, rest: &str) -> Value {
    eval(ui, &format!("({verb} \"{GRAPH}\" {node} {rest})"))
}

fn add_slot_on(ui: &mut Runtime, node: usize, class: &str) -> u64 {
    match node_eval(ui, node, "graph-node-process-add", &format!("\"{class}\"")) {
        Value::Number(id) => id as u64,
        other => panic!("add {class}: {other:?}"),
    }
}

/// Add an expr card to `node` with `source`, asserting the commit succeeds.
fn expr_card_on(ui: &mut Runtime, node: usize, source: &str) -> u64 {
    let id = add_slot_on(ui, node, "expr");
    let result = node_eval(ui, node, "graph-node-process-expr-set", &format!("{id} {source:?}"));
    assert_eq!(field(&result, "ok"), Value::Bool(true), "{source}: {result:?}");
    id
}

fn map_out_to_transpose(ui: &mut Runtime, node: usize, id: u64) {
    node_eval(ui, node, "graph-node-process-map", &format!("{id} \"out\" :transpose"));
}

fn node1_hits(state: &Arc<SequencerState>) -> Vec<Hit> {
    track1_hits(state).into_iter().filter(|hit| hit.neuron == 1).collect()
}

fn transposes(hits: &[Hit]) -> Vec<f32> {
    hits.iter().map(|hit| hit.resolved.transpose).collect()
}

#[test]
fn expr_context_payload_vars_read_the_payload_as_earlier_slots_left_it() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let base = node1_hits(&state);
        assert_eq!(base.len(), 1, "one fire: {base:?}");
        let base = base[0].resolved;

        // $note after a neural-transpose slot: 2 + 5 = 7, mapped back onto
        // transpose (an add) doubles it.
        let transpose = add_slot_on(&mut ui, 1, "neural-transpose");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{transpose} :amount 5"));
        let note = expr_card_on(&mut ui, 1, "$note");
        map_out_to_transpose(&mut ui, 1, note);
        assert_eq!(transposes(&node1_hits(&state)), vec![14.0], "$note sees the earlier +5");

        // $vel / $dur read the payload; vel!/dur! write it, and a later
        // card's $vel sees the earlier card's write.
        let reader = expr_card_on(&mut ui, 1, "(xpose! (- (* $vel 10) $note))");
        let hit = node1_hits(&state)[0].resolved;
        assert!(
            (hit.transpose - (14.0 + base.velocity * 10.0 - 14.0)).abs() < 1e-4,
            "$vel {} / $note 14: {hit:?}",
            base.velocity
        );
        node_eval(&mut ui, 1, "graph-node-process-remove", &format!("{reader}"));
        let writer = expr_card_on(&mut ui, 1, "(do (vel! 0.25) (dur! (* $dur 2)))");
        let reader = expr_card_on(&mut ui, 1, "(xpose! (* $vel 8))");
        let hit = node1_hits(&state)[0].resolved;
        assert!((hit.velocity - 0.25).abs() < 1e-6, "vel! sets velocity: {hit:?}");
        assert!((hit.duration - base.duration * 2.0).abs() < 1e-6, "dur! from $dur: {hit:?}");
        assert!((hit.transpose - 16.0).abs() < 1e-6, "$vel after vel!: 14 + 0.25 * 8: {hit:?}");
        for id in [writer, reader, note, transpose] {
            node_eval(&mut ui, 1, "graph-node-process-remove", &format!("{id}"));
        }

        // $delay: what earlier slots added to the propagation delay.
        let delay = add_slot_on(&mut ui, 1, "neural-delay");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{delay} :amount 3"));
        expr_card_on(&mut ui, 1, "(xpose! $delay)");
        assert_eq!(transposes(&node1_hits(&state)), vec![5.0], "2 + $delay 3");
    });
}

#[test]
fn expr_context_transport_vars_track_the_fire_beat() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        // Four sixteenths per beat: $beat * 4 is the fire's step, and in
        // the first bar so is $phase * 16.
        let card = expr_card_on(&mut ui, 1, "(* $beat 4)");
        map_out_to_transpose(&mut ui, 1, card);
        let hits = node1_hits(&state);
        assert_eq!(hits.len(), 4, "{hits:?}");
        for hit in &hits {
            let step = (hit.sample as f64 / SAMPLES_PER_BEAT * 4.0).round() as f32;
            assert_eq!(hit.resolved.transpose, 2.0 + step, "$beat at sample {}", hit.sample);
        }
        node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!("{card} \"(* $phase 16)\""));
        assert_eq!(transposes(&node1_hits(&state)), transposes(&hits), "$phase in bar 1");
    });
}

#[test]
fn expr_n_and_prev_count_fires_and_clear_on_reset() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "$n");
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(transposes(&node1_hits(&state)), vec![2.0, 3.0, 4.0, 5.0], "$n is 0-based");

        node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!("{card} \"(+ $prev 1)\""));
        assert_eq!(transposes(&node1_hits(&state)), vec![3.0, 4.0, 5.0, 6.0], "$prev chain counts up");

        // reset! on the second fire: the next fire is the first after a
        // reset ($reset 1), $n and $prev start over.
        node_eval(
            &mut ui,
            1,
            "graph-node-process-expr-set",
            &format!("{card} \"(do (if (= $n 1) (reset!)) (+ $prev 1 (* 10 $reset)))\""),
        );
        assert_eq!(
            transposes(&node1_hits(&state)),
            // The graph's first fire counts as after a reset. Fire 0 sends
            // 0 + 1 + 10 = 11, fire 1 (n = 1) 11 + 1 and asks for a reset;
            // fire 2 starts over (without the reset it would send 23).
            vec![13.0, 14.0, 13.0, 14.0],
            "reset! clears $n and $prev; $reset marks the first fire after"
        );
    });
}

#[test]
fn expr_veto_mutes_from_a_condition_and_the_write_sends_nothing() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let before = track1_hits(&state);
        assert_eq!(before.iter().filter(|hit| hit.neuron == 1).count(), 4, "{before:?}");

        // On node 1: mute from the third fire on.
        let card = expr_card_on(&mut ui, 1, "(if (> $n 1) (veto!))");
        assert_eq!(node1_hits(&state).len(), 2, "fires 0 and 1 play");
        // The body's value is the write's nil: nothing on the wire.
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(transposes(&node1_hits(&state)), vec![2.0, 2.0], "writes send nothing");
    });
}

#[test]
fn expr_veto_still_scatters_to_the_next_node() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_chain_state(3, FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let count = |hits: &[Hit], neuron: usize| hits.iter().filter(|hit| hit.neuron == neuron).count();
        let before = track1_hits(&state);
        assert_eq!((count(&before, 1), count(&before, 2)), (4, 4), "{before:?}");
        // Node 1 muted: node 2, fed by node 1's scatter, fires as before.
        expr_card_on(&mut ui, 1, "(veto!)");
        let hits = track1_hits(&state);
        assert_eq!((count(&hits, 1), count(&hits, 2)), (0, 4), "{hits:?}");
    });
}

#[test]
fn expr_delay_write_moves_the_downstream_fire() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_chain_state(3, &[0]);
        let mut ui = ui_runtime(&state);
        let node2 = |state: &Arc<SequencerState>| -> Vec<Hit> {
            track1_hits(state).into_iter().filter(|hit| hit.neuron == 2).collect()
        };
        let before = node2(&state);
        assert_eq!(before.len(), 1, "{:?}", track1_hits(&state));
        // Node 1's card pushes its propagation delay two steps: node 2
        // fires two sixteenths (12000 samples) later.
        expr_card_on(&mut ui, 1, "(delay! 2)");
        let after = node2(&state);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0].sample, before[0].sample + 12_000, "{before:?} -> {after:?}");
        assert_eq!(after[0].resolved.transpose, 2.0, "delay! leaves the note alone");
    });
}

#[test]
fn expr_xpose_write_and_value_combine_on_one_card() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(do (xpose! 3) 4)");
        assert_eq!(transposes(&node1_hits(&state)), vec![5.0], "xpose! adds; the value is unmapped");
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(transposes(&node1_hits(&state)), vec![9.0], "2 + 3 + mapped 4");
    });
}

#[test]
fn expr_inlets_may_shadow_scheduler_functions() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        // `vel` is a scheduler native (ratchet event read): as an argument it
        // is an inlet for this body, and the body runs.
        let card = expr_card_on(&mut ui, 1, "(* vel 2)");
        map_out_to_transpose(&mut ui, 1, card);
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{card} :vel 3"));
        assert_eq!(transposes(&node1_hits(&state)), vec![8.0], "2 + 3 * 2");
        assert_eq!(state.process_run_error(card), None);

        // `in` is the natural preset name; the generated inlet binding reads
        // through the global `in` outside the body, so shadowing is safe.
        let result = node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!("{card} \"(* in k)\""));
        assert_eq!(names(&field(&result, "inlets")), vec!["in", "k"]);
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{card} :in 3"));
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{card} :k 4"));
        assert_eq!(transposes(&node1_hits(&state)), vec![14.0], "2 + 3 * 4");
        assert_eq!(state.process_run_error(card), None);

        // Used both ways: refused at commit, the old class keeps running.
        let result = node_eval(
            &mut ui,
            1,
            "graph-node-process-expr-set",
            &format!("{card} \"(+ (vel x) vel)\""),
        );
        assert_eq!(field(&result, "ok"), Value::Bool(false), "{result:?}");
        let Value::String(message) = field(&result, "error") else { panic!("{result:?}") };
        assert!(message.contains("`vel`") && message.contains("rename the inlet"), "{message}");
        assert_eq!(transposes(&node1_hits(&state)), vec![14.0]);
    });
}

#[test]
fn expr_context_completions_list_every_context_variable_and_write() {
    let state = expr_graph_state();
    let mut ui = ui_runtime(&state);
    let Value::List(items) = eval(&mut ui, "(expr-context-completions)") else { panic!("list") };
    let labels: Vec<String> = items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::List(parts) => match &*parts[0].borrow() {
                Value::String(label) => label.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        })
        .collect();
    for name in lisp_host::EXPR_CONTEXT_VARS.iter().chain(&[
        "veto!", "delay!", "xpose!", "vel!", "dur!", "reset!", "state", "prev", "delta", "integ", "sh", "slew",
        "every", "count",
    ]) {
        assert!(labels.iter().any(|label| label == name), "{name} in {labels:?}");
    }
}

// ---- eseq-waa9.13: state and stateful helpers (spec §5, §5.1, §9) ----

/// The spec §9 Galois LFSR, stepped the way the expr body steps it.
fn lfsr_step(s: u32, taps: u32) -> u32 {
    (s >> 1) ^ if s & 1 == 1 { taps } else { 0 }
}

/// The first `n` register values after the seed.
fn lfsr_sequence(n: usize) -> Vec<u32> {
    let mut s = 0xACE1;
    (0..n)
        .map(|_| {
            s = lfsr_step(s, 0xB400);
            s
        })
        .collect()
}

/// Commit `source` on an existing card, asserting success.
fn recommit(ui: &mut Runtime, id: u64, source: &str) {
    let result = node_eval(ui, 1, "graph-node-process-expr-set", &format!("{id} {source:?}"));
    assert_eq!(field(&result, "ok"), Value::Bool(true), "{source}: {result:?}");
}

/// Node 1's transposes minus the seed's 2: what the mapped card sent.
fn sent(state: &Arc<SequencerState>) -> Vec<f32> {
    transposes(&node1_hits(state)).into_iter().map(|t| t - 2.0).collect()
}

#[test]
fn expr_spec_lfsr_steps_across_fires_and_restarts_after_a_reset() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        // The §9 body verbatim; a later card reads the delay it wrote.
        // (Single line: eseqlisp strings have no escapes; the multi-line
        // form is pinned by the unit tests.)
        let lfsr = add_slot_on(&mut ui, 1, "expr");
        let result = node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!(
            "{lfsr} \"(state s 0xACE1) (set! s (bit-xor (shr s 1) (if (= (bit-and s 1) 1) taps 0))) (delay! (* grain (bit-and s 7)))\""
        ));
        assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
        assert_eq!(names(&field(&result, "inlets")), vec!["taps", "grain"]);
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{lfsr} :taps {}", 0xB400));
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{lfsr} :grain 1"));
        expr_card_on(&mut ui, 1, "(xpose! $delay)");
        let expected: Vec<f32> = lfsr_sequence(4).iter().map(|s| (s & 7) as f32).collect();
        assert_eq!(expected, vec![0.0, 0.0, 4.0, 6.0], "the Rust register");
        assert_eq!(sent(&state), expected, "the body steps the register once per fire");
        assert_eq!(state.process_run_error(lfsr), None);

        // A graph reset after fire 1: fire 2 starts over from 0xACE1.
        let resetter = expr_card_on(&mut ui, 1, "(if (= $n 1) (reset!))");
        assert_eq!(sent(&state), vec![expected[0], expected[1], expected[0], expected[1]]);
        node_eval(&mut ui, 1, "graph-node-process-remove", &format!("{resetter}"));

        // The register itself on the wire (16 bits of state, 4 sent).
        let wire = expr_card_on(
            &mut ui,
            1,
            "(state s 0xACE1) (set! s (bit-xor (shr s 1) (if (= (bit-and s 1) 1) 0xB400 0))) (bit-and s 15)",
        );
        node_eval(&mut ui, 1, "graph-node-process-remove", &format!("{lfsr}"));
        map_out_to_transpose(&mut ui, 1, wire);
        let expected: Vec<f32> = lfsr_sequence(4).iter().map(|s| (s & 15) as f32).collect();
        assert_eq!(sent(&state), expected);
    });
}

#[test]
fn expr_state_set_reaches_the_cell_inside_let_and_lambda() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(state c 0) (let ((k 1)) (set! c (+ c k))) c");
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(sent(&state), vec![1.0, 2.0, 3.0, 4.0], "set! inside let");
        recommit(&mut ui, card, "(state c 0) ((lambda (k) (if (> k 0) (set! c (+ c k)))) 2) c");
        assert_eq!(sent(&state), vec![2.0, 4.0, 6.0, 8.0], "set! inside a lambda and an if");
        // State is independent of the output and of $prev.
        recommit(&mut ui, card, "(state c 10) (set! c (- c 1)) (+ $prev (* 0 c) 1)");
        assert_eq!(sent(&state), vec![1.0, 2.0, 3.0, 4.0]);
        // A bypassed (failed) run stores nothing: c sticks at 1.
        recommit(&mut ui, card, "(state c 0) (set! c (+ c 1)) (if (= c 2) (list 1) c)");
        assert_eq!(sent(&state), vec![1.0, 0.0, 0.0, 0.0]);
        assert!(state.process_run_error(card).is_some());
    });
}

#[test]
fn expr_cards_with_the_same_source_keep_independent_state() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let source = "(state c 0) (set! c (+ c k)) c";
        let a = expr_card_on(&mut ui, 1, source);
        let b = expr_card_on(&mut ui, 1, source);
        let chain = node1_chain(&state);
        assert_eq!(chain.slots[0].class_name, chain.slots[1].class_name, "one hashed class");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{a} :k 1"));
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{b} :k 10"));
        map_out_to_transpose(&mut ui, 1, a);
        map_out_to_transpose(&mut ui, 1, b);
        // a: 1 2 3 4, b: 10 20 30 40 (a shared cell would give 1+11, …).
        assert_eq!(sent(&state), vec![11.0, 22.0, 33.0, 44.0]);
    });
}

#[test]
fn expr_stateful_helpers_run_per_call_site_and_reset() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "0");
        map_out_to_transpose(&mut ui, 1, card);
        // x per fire: 5 1 4 2.
        let x = "(nth '(5 1 4 2) $n)";
        for (body, expected) in [
            (format!("(prev {x})"), vec![0.0, 5.0, 1.0, 4.0]),
            (format!("(delta {x})"), vec![5.0, -4.0, 3.0, -2.0]),
            (format!("(integ {x})"), vec![5.0, 6.0, 10.0, 12.0]),
            (format!("(sh (= (bit-and $n 1) 0) {x})"), vec![5.0, 5.0, 4.0, 4.0]),
            (format!("(slew {x} 0.5)"), vec![2.5, 1.75, 2.875, 2.4375]),
            // nil on the off fires sends nothing.
            (format!("(every 2 {x})"), vec![5.0, 0.0, 4.0, 0.0]),
            ("(count 3)".to_string(), vec![0.0, 1.0, 2.0, 0.0]),
            // Two call sites, two histories: -(prev x).
            (format!("(- (prev {x}) (prev (* 2 {x})))"), vec![0.0, -5.0, -1.0, -4.0]),
            // Helpers reset with the rest of the card's state.
            ("(do (if (= $n 1) (reset!)) (integ 1))".to_string(), vec![1.0, 2.0, 1.0, 2.0]),
        ] {
            recommit(&mut ui, card, &body);
            assert_eq!(sent(&state), expected, "{body}");
            assert_eq!(state.process_run_error(card), None, "{body}");
        }
        // A helper inside a lambda is refused at commit; the old class runs on.
        let result = node_eval(
            &mut ui,
            1,
            "graph-node-process-expr-set",
            &format!("{card} \"((lambda (v) (prev v)) 1)\""),
        );
        assert_eq!(field(&result, "ok"), Value::Bool(false), "{result:?}");
        assert_eq!(sent(&state), vec![1.0, 2.0, 1.0, 2.0]);
    });
}

#[test]
fn expr_locals_may_shadow_globals_they_do_not_call() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(let ((vel 2)) (* vel 3))");
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(sent(&state), vec![6.0], "a let local named like a scheduler native");
        recommit(&mut ui, card, "((lambda (count) (+ count 1)) 2)");
        assert_eq!(sent(&state), vec![3.0], "a lambda parameter named like a global");
        recommit(&mut ui, card, "(state neuron 4) (set! neuron (+ neuron 1)) neuron");
        assert_eq!(sent(&state), vec![5.0], "a state name named like a global");
        for bad in ["(let ((vel 2)) (vel 1))", "(state s 0) (let ((s 1)) s)", "(state s 0) (state s 1) s"] {
            let result = node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!("{card} {bad:?}"));
            assert_eq!(field(&result, "ok"), Value::Bool(false), "{bad}: {result:?}");
            assert!(!matches!(field(&result, "span"), Value::Nil), "{bad}: span");
        }
    });
}

#[test]
fn expr_state_is_per_slot_across_wires_and_identical_sources() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let counter = "(state c 0) (set! c (+ c 1)) c";
        // A counter wired into an accumulator with its own `c`, then a
        // second card with the counter's exact source; all three mutate
        // their `c` in the same fire.
        let a = expr_card_on(&mut ui, 1, counter);
        let b = expr_card_on(&mut ui, 1, "(state c 10) (set! c (+ c x)) c");
        let twin = expr_card_on(&mut ui, 1, counter);
        node_eval(&mut ui, 1, "graph-node-process-wire", &format!("{a} \"wire\" {b} \"x\""));
        map_out_to_transpose(&mut ui, 1, b);
        map_out_to_transpose(&mut ui, 1, twin);
        // Per fire: a 1 2 3 4 into b (11 13 16 20), twin 1 2 3 4. A shared
        // or leaked cell would break the sums. (Kept under the transpose
        // clamp.)
        assert_eq!(sent(&state), vec![12.0, 15.0, 19.0, 24.0]);
        for id in [a, b, twin] {
            assert_eq!(state.process_run_error(id), None);
        }
    });
}

#[test]
fn expr_state_edge_cases() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(state c) (set! c (+ c 1)) c");
        map_out_to_transpose(&mut ui, 1, card);
        assert_eq!(sent(&state), vec![1.0, 2.0, 3.0, 4.0], "(state c) starts at 0");
        recommit(&mut ui, card, "(state c -0x10) (set! c (+ c 1)) c");
        assert_eq!(sent(&state), vec![-15.0, -14.0, -13.0, -12.0], "signed hex init");
        // A lambda that writes the cell, called twice per fire.
        recommit(&mut ui, card, "(state c 0) (let ((bump (lambda () (set! c (+ c 1))))) (bump) (bump)) c");
        assert_eq!(sent(&state), vec![2.0, 4.0, 6.0, 8.0]);
        // A write inside a lambda before a failure is not stored either.
        recommit(
            &mut ui,
            card,
            "(state c 0) ((lambda () (set! c (+ c 1)) (if (= c 2) (list 1) c)))",
        );
        assert_eq!(sent(&state), vec![1.0, 0.0, 0.0, 0.0]);
        // `set!` with a missing or extra value is a commit error.
        for bad in ["(state c 0) (set! c) c", "(state c 0) (set! c 1 2) c"] {
            let result = node_eval(&mut ui, 1, "graph-node-process-expr-set", &format!("{card} {bad:?}"));
            assert_eq!(field(&result, "ok"), Value::Bool(false), "{bad}: {result:?}");
        }
    });
}

#[test]
fn expr_state_restarts_on_stop_play_and_carries_on_without_it() {
    run_with_scheduler_stack(|| {
        // Trigs on every even step: node 1 fires four times in each of two
        // consecutive lookahead seconds, with no graph reset between them.
        let state = expr_graph_state_seeded(&[0, 2, 4, 6, 8, 10, 12, 14]);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(state c 0) (set! c (+ c 1)) (+ c (integ 2))");
        map_out_to_transpose(&mut ui, 1, card);
        let sends = |passes: Vec<Vec<Hit>>| -> Vec<Vec<f32>> {
            passes
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .filter(|hit| hit.neuron == 1)
                        .map(|hit| hit.resolved.transpose - 2.0)
                        .collect()
                })
                .collect()
        };
        let first = vec![3.0, 6.0, 9.0, 12.0];
        let carried = sends(track1_hit_passes(&state, Some(&|_: &mut SchedulerLookaheadState| {})));
        assert_eq!(carried[0], first);
        assert_eq!(carried[1], vec![15.0, 18.0, 21.0, 24.0], "cells carry on without a reset");
        // What the worker does on stop → play: clear the step process state.
        // The state and helper cells restart at their inits.
        let restarted = sends(track1_hit_passes(
            &state,
            Some(&|scheduler: &mut SchedulerLookaheadState| {
                scheduler.process_runtime.reset_step_process_states()
            }),
        ));
        assert_eq!(restarted, vec![first.clone(), first]);
    });
}

// ---- eseq-waa9.15: threading, constants, shaping helpers (spec §6.1) ----

#[test]
fn expr_shaping_helpers_threading_and_constants_run_on_the_scheduler_vm() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "0");
        map_out_to_transpose(&mut ui, 1, card);
        for (body, expected) in [
            ("(euclid 3 8 $n)", vec![1.0, 0.0, 0.0, 1.0]),
            ("(-> $n (* 0.25) tri (scale 0 1 0 8))", vec![0.0, 4.0, 8.0, 4.0]),
            ("(->> 0.25 (* $n) sqr)", vec![1.0, 1.0, 0.0, 0.0]),
            ("(-> $n (fold 0 2))", vec![0.0, 1.0, 2.0, 1.0]),
            ("(-> $n (wrap 0 3))", vec![0.0, 1.0, 2.0, 0.0]),
            ("(clip $n 1 2)", vec![1.0, 1.0, 2.0, 2.0]),
            ("(quant (* $n 0.6) 1)", vec![0.0, 1.0, 1.0, 2.0]),
            ("(-> $n (* 0.25) sine (scale 0 1 0 2) round)", vec![1.0, 2.0, 1.0, 0.0]),
            ("(-> $n (* 0.5) bipolar unipolar (* 2))", vec![0.0, 1.0, 2.0, 3.0]),
            ("(round (+ tau pi))", vec![9.0, 9.0, 9.0, 9.0]),
            // A stateful helper written through ->.
            ("(-> $n prev)", vec![0.0, 0.0, 1.0, 2.0]),
            ("(-> $n (* 2) delta)", vec![0.0, 2.0, 2.0, 2.0]),
        ] {
            recommit(&mut ui, card, body);
            assert_eq!(sent(&state), expected, "{body}");
            assert_eq!(state.process_run_error(card), None, "{body}");
        }
        // A non-number argument fails the run (bypass): nothing is sent.
        recommit(&mut ui, card, "(quant (list 1) 1)");
        assert_eq!(sent(&state), vec![0.0; 4]);
        assert!(state.process_run_error(card).is_some());
    });
}

#[test]
fn expr_choose_picks_among_its_arguments_from_the_per_fire_rng() {
    run_with_scheduler_stack(|| {
        let steps: Vec<usize> = (0..7).collect();
        let state = expr_graph_state_seeded(&steps);
        let mut ui = ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(choose 1 2 3 4 5 6 7 8)");
        map_out_to_transpose(&mut ui, 1, card);
        let picks = sent(&state);
        assert!(picks.len() >= 6, "{picks:?}");
        assert!(picks.iter().all(|pick| (1.0..=8.0).contains(pick) && pick.fract() == 0.0), "{picks:?}");
        assert!(picks.windows(2).any(|pair| pair[0] != pair[1]), "fires differ: {picks:?}");
        assert_eq!(state.process_run_error(card), None);
        // The per-fire seed makes the choices reproducible.
        assert_eq!(sent(&state), picks);
        // Non-number values are chosen as they are (a nil choice sends nothing).
        recommit(&mut ui, card, "(choose 5)");
        assert!(sent(&state).iter().all(|pick| *pick == 5.0));
    });
}

#[test]
fn expr_completions_list_shaping_helpers_constants_and_threading() {
    let state = expr_graph_state();
    let mut ui = ui_runtime(&state);
    let Value::List(items) = eval(&mut ui, "(expr-context-completions)") else { panic!("list") };
    let labels: Vec<String> = items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::List(parts) => match &*parts[0].borrow() {
                Value::String(label) => label.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        })
        .collect();
    for name in [
        "quant", "fold", "wrap", "scale", "clip", "euclid", "choose", "sine", "tri", "saw", "sqr", "unipolar",
        "bipolar", "pi", "tau", "->", "->>",
    ] {
        assert!(labels.iter().any(|label| label == name), "{name} in {labels:?}");
    }
}

// ---- eseq-waa9.17: promote to My processes + edit as expr (spec §8) ----

/// The spec §9 LFSR as the lfsr preset holds it (multi-line: eseqlisp
/// strings take raw newlines).
const LFSR_BODY: &str = "; 16-bit Galois LFSR on the delay
(state s 0xACE1)
(set! s (bit-xor (shr s 1)
                 (if (= (bit-and s 1) 1) taps 0)))
(delay! (* grain (bit-and s 7)))";

/// A temp My processes package for one test. Promote writes files: a test
/// must never write into the real ~/.eseq.d. The override is thread-local
/// and lasts while the returned guard lives (declared after the dir, so it
/// drops first and never points at a deleted dir).
struct TempMyProcesses {
    _guard: lisp_host::MyProcessesDirOverrideGuard,
    dir: tempfile::TempDir,
}

impl TempMyProcesses {
    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

fn temp_my_processes() -> TempMyProcesses {
    let dir = tempfile::tempdir().expect("temp dir");
    let guard =
        lisp_host::set_my_processes_package_dir_override(Some(dir.path().join("packages/user.processes")));
    TempMyProcesses { _guard: guard, dir }
}

/// A UI runtime like the app's: the process authoring natives (def-process
/// publishes), the builtin library as the package layer, the graph natives.
fn promote_ui_runtime(
    state: &Arc<SequencerState>,
) -> (Runtime, lisp_host::PublishedProcessAuthoringNatives) {
    let mut ui = Runtime::new();
    let authoring = lisp_host::register_published_process_authoring_natives(
        &mut ui,
        Arc::clone(state),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    ui.eval_str(&lisp_host::load_process_library_source()).expect("builtin library");
    for line in lisp_host::load_my_processes_source().lines() {
        ui.eval_str(line).expect("My processes module");
    }
    authoring.mark_package_defs();
    lisp_host::register_graph_authoring_natives(&mut ui, Arc::clone(state));
    (ui, authoring)
}

fn string_field(map: &Value, key: &str) -> String {
    match field(map, key) {
        Value::String(text) => text,
        other => panic!("{key}: expected a string, got {other:?} in {map:?}"),
    }
}

/// Promote card `id` on node 1 as `name` the way the UI does: write the
/// module, load it, rebind the slot. Returns the class.
fn promote(ui: &mut Runtime, id: u64, name: &str) -> String {
    let result = node_eval(ui, 1, "graph-node-process-promote", &format!("{id} \"{name}\""));
    assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
    let path = string_field(&result, "path");
    let class = string_field(&result, "class");
    // `load` returns the module's last value (the def-process class name),
    // or an error string starting "load:".
    let loaded = eval(ui, &format!("(load \"{path}\")"));
    assert_eq!(loaded, Value::String(class.clone()), "load {path}");
    let rebind = node_eval(ui, 1, "graph-node-process-rebind-class", &format!("{id} \"{class}\""));
    assert_eq!(field(&rebind, "ok"), Value::Bool(true), "{rebind:?}");
    class
}

fn slot_of(state: &SequencerState, id: u64) -> crate::process::TrackProcessSlot {
    node1_chain(state).slots.into_iter().find(|slot| slot.instance_id.0 == id).expect("slot")
}

#[test]
fn promote_lfsr_card_writes_the_module_and_runs_identically() {
    run_with_scheduler_stack(|| {
        let dir = temp_my_processes();
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let (mut ui, authoring) = promote_ui_runtime(&state);
        let lfsr = add_slot_on(&mut ui, 1, "expr");
        let result = eval(&mut ui, &format!("(graph-node-process-expr-set \"{GRAPH}\" 1 {lfsr} \"{LFSR_BODY}\")"));
        assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{lfsr} :taps {}", 0xB400));
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{lfsr} :grain 1"));
        expr_card_on(&mut ui, 1, "(xpose! $delay)");
        let before = sent(&state);
        assert_eq!(before, vec![0.0, 0.0, 4.0, 6.0], "the expr card");

        let class = promote(&mut ui, lfsr, "my-lfsr");
        assert_eq!(class, "user.processes.my-lfsr/my-lfsr");
        // The module: the package manifest, the module header, the body verbatim.
        let package = dir.path().join("packages/user.processes");
        assert!(package.join("manifest.json").is_file());
        let text = std::fs::read_to_string(package.join("src/my-lfsr.lisp")).unwrap();
        assert!(text.contains("(module user.processes.my-lfsr)"), "{text}");
        assert!(text.contains(":doc \"16-bit Galois LFSR on the delay\""), "{text}");
        assert!(text.contains("(taps :float -1000000 1000000 :default 46080 :lane true)"), "{text}");
        assert!(text.contains(&format!(":expr \"{LFSR_BODY}\")")), "{text}");

        // The card is rebound in place: same id and position, values kept,
        // no longer an expr card.
        let slot = slot_of(&state, lfsr);
        assert_eq!(slot.class_name, class);
        assert_eq!(slot.expr_source, None);
        assert!(!slot.is_expr_card());
        assert_eq!(node1_chain(&state).slots[0].instance_id.0, lfsr);
        assert_eq!(slot.inlets.get("taps").map(|v| v.to_value()), Some(Value::Number(46080.0)));
        let def = state
            .published_process_authoring()
            .defs
            .into_iter()
            .find(|def| def.name == class)
            .expect("the promoted class is published");
        assert_eq!(def.expr_source.as_deref(), Some(LFSR_BODY));
        assert_eq!(sent(&state), before, "the promoted class runs like the card");
        assert_eq!(state.process_run_error(lfsr), None);

        // The chain read and the add menu see a My processes card.
        let chain = node_eval(&mut ui, 1, "graph-node-process-chain", "");
        let Value::List(slots) = chain else { panic!("chain list") };
        let entry = slots[0].borrow().clone();
        assert_eq!(string_field(&entry, "label"), "my-lfsr");
        assert_eq!(field(&entry, "expr"), Value::Bool(false));
        assert_eq!(field(&entry, "promoted-expr"), Value::Bool(true));
        assert_eq!(field(&entry, "as-expr-reason"), Value::Nil);
        let Value::List(classes) = eval(&mut ui, "(graph-node-process-classes)") else { panic!("classes") };
        let row = classes
            .iter()
            .map(|row| row.borrow().clone())
            .find(|row| string_field(row, "class") == class)
            .expect("the add menu offers the promoted class");
        assert_eq!(string_field(&row, "label"), "my-lfsr");
        assert!(
            def.source_path.as_deref().is_some_and(lisp_host::is_my_processes_source),
            "its source file lies in the My processes package: {:?}",
            def.source_path
        );

        // A project switch keeps it (a library class, like builtin.lisp's).
        authoring.reset_project_authored();
        assert!(state.published_process_authoring().defs.iter().any(|def| def.name == class));

        // Name checks: taken (by name or label), illegal, reserved.
        // The user's own class of that name is an update, not a clash.
        let check = eval(&mut ui, &format!("(graph-node-process-promote-check \"{GRAPH}\" \"my-lfsr\")"));
        assert_eq!((field(&check, "ok"), field(&check, "update")), (Value::Bool(true), Value::Bool(true)), "{check:?}");
        for (name, why) in [
            ("delay", "already exists"),
            ("Bad Name", "not a legal name"),
            ("expr", "reserved"),
        ] {
            let check = eval(&mut ui, &format!("(graph-node-process-promote-check \"{GRAPH}\" \"{name}\")"));
            assert_eq!(field(&check, "ok"), Value::Bool(false), "{name}");
            assert!(string_field(&check, "error").contains(why), "{name}: {check:?}");
        }
        let check = eval(&mut ui, &format!("(graph-node-process-promote-check \"{GRAPH}\" \"wobble\")"));
        assert_eq!(field(&check, "ok"), Value::Bool(true), "{check:?}");
        // Only an expr card promotes.
        let refused = node_eval(&mut ui, 1, "graph-node-process-promote", &format!("{lfsr} \"again\""));
        assert!(string_field(&refused, "error").contains("not an expr card"), "{refused:?}");
    });
}

#[test]
fn promote_keeps_cables_and_values_and_edit_as_expr_round_trips() {
    run_with_scheduler_stack(|| {
        let _dir = temp_my_processes();
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let (mut ui, _authoring) = promote_ui_runtime(&state);
        // The bounce preset with k cabled from another card, plus a helper.
        let source = expr_card_on(&mut ui, 1, "3");
        let bounce = expr_card_on(&mut ui, 1, "(+ (* k (pow decay $n)) (* 0 (prev k)))");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{bounce} :decay 0.5"));
        node_eval(&mut ui, 1, "graph-node-process-wire", &format!("{source} \"wire\" {bounce} \"k\""));
        map_out_to_transpose(&mut ui, 1, bounce);
        let before = sent(&state);
        assert_eq!(before, vec![3.0, 1.5, 0.75, 0.375]);

        let class = promote(&mut ui, bounce, "bouncy");
        let chain = node1_chain(&state);
        let Some(Some(crate::process::ParamTarget::ProcessInlet { process, inlet, instance_id })) =
            chain.slots[0].bindings.get("wire")
        else {
            panic!("the cable into k survives: {:?}", chain.slots[0].bindings);
        };
        assert_eq!((process.as_str(), inlet.as_str(), instance_id.map(|id| id.0)), (class.as_str(), "k", Some(bounce)));
        assert_eq!(slot_of(&state, bounce).inlets.get("decay").map(|v| v.to_value()), Some(Value::Number(0.5)));
        assert_eq!(sent(&state), before, "promoted: same output, cable and mapping kept");

        // Edit as expr: the slot becomes an expr card holding the body.
        let result = node_eval(&mut ui, 1, "graph-node-process-edit-as-expr", &format!("{bounce}"));
        assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
        let slot = slot_of(&state, bounce);
        assert!(slot.is_expr_card());
        assert!(crate::process::is_expr_process_class(&slot.class_name));
        assert_eq!(slot.expr_source.as_deref(), Some("(+ (* k (pow decay $n)) (* 0 (prev k)))"));
        let chain = node1_chain(&state);
        assert!(
            matches!(chain.slots[0].bindings.get("wire"),
                Some(Some(crate::process::ParamTarget::ProcessInlet { process, .. })) if *process == slot.class_name),
            "the cable follows the slot back"
        );
        assert_eq!(sent(&state), before, "back as an expr card: same output");

        // A class that was not promoted refuses, saying why.
        let transpose = add_slot_on(&mut ui, 1, "neural-transpose");
        let refused = node_eval(&mut ui, 1, "graph-node-process-edit-as-expr", &format!("{transpose}"));
        assert_eq!(field(&refused, "ok"), Value::Bool(false));
        assert!(string_field(&refused, "error").contains("not promoted from an expr card"), "{refused:?}");
        let Value::List(slots) = node_eval(&mut ui, 1, "graph-node-process-chain", "") else { panic!() };
        let entry = slots.last().unwrap().borrow().clone();
        assert!(string_field(&entry, "as-expr-reason").contains("not promoted"), "{entry:?}");
    });
}

#[test]
fn promoted_class_loads_on_restart_and_a_missing_package_leaves_the_card_inert() {
    run_with_scheduler_stack(|| {
        let dir = temp_my_processes();
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let (mut ui, _authoring) = promote_ui_runtime(&state);
        let card = expr_card_on(&mut ui, 1, "(+ 1 $n)");
        map_out_to_transpose(&mut ui, 1, card);
        let before = sent(&state);
        assert_eq!(before, vec![1.0, 2.0, 3.0, 4.0]);
        let class = promote(&mut ui, card, "counter");
        drop(ui);

        // A restart: a fresh UI runtime loads My processes at startup.
        let (_ui, _authoring) = promote_ui_runtime(&state);
        assert!(state.published_process_authoring().defs.iter().any(|def| def.name == class));
        assert_eq!(sent(&state), before, "the project's slot runs after a restart");

        // Without the package, the slot keeps its class and does nothing,
        // like any process whose package is missing.
        std::fs::remove_dir_all(dir.path().join("packages")).unwrap();
        let (mut ui, _authoring) = promote_ui_runtime(&state);
        assert!(!state.published_process_authoring().defs.iter().any(|def| def.name == class));
        assert_eq!(slot_of(&state, card).class_name, class);
        assert!(sent(&state).is_empty() || sent(&state).iter().all(|v| *v == 0.0), "inert");
        let Value::List(slots) = node_eval(&mut ui, 1, "graph-node-process-chain", "") else { panic!() };
        let entry = slots[0].borrow().clone();
        assert_eq!(field(&entry, "known"), Value::Bool(false));
        assert!(string_field(&entry, "as-expr-reason").contains("not loaded"), "{entry:?}");
    });
}

/// Promote -> as expr -> change the body -> promote under the same name
/// updates the user's own class (spec §8): the name check says `:update`,
/// the write needs the replace confirmation, the file is rewritten and
/// reloaded (one class, not two), the edited card is rebound, and another
/// card of the class picks up the new body — with an inlet removed (and a
/// cable still pointing at it) and one added — without a run error. A body
/// with `"` in it round-trips exactly. Builtin names stay refused.
#[test]
fn promote_again_updates_my_own_class_for_every_card_using_it() {
    run_with_scheduler_stack(|| {
        let dir = temp_my_processes();
        let state = expr_graph_state_seeded(FOUR_TRIGS);
        let (mut ui, _authoring) = promote_ui_runtime(&state);
        let a = expr_card_on(&mut ui, 1, "(* k (pow decay $n))");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{a} :k 4"));
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{a} :decay 0.5"));
        map_out_to_transpose(&mut ui, 1, a);
        let class = promote(&mut ui, a, "bouncy");
        // A second card of the class, its decay cabled from the first.
        let b = add_slot_on(&mut ui, 1, &class);
        map_out_to_transpose(&mut ui, 1, b);
        node_eval(&mut ui, 1, "graph-node-process-wire", &format!("{a} \"wire\" {b} \"decay\""));
        assert!(sent(&state).iter().all(|v| v.is_finite()));

        // As expr remembers where the card came from.
        let reopened = node_eval(&mut ui, 1, "graph-node-process-edit-as-expr", &format!("{a}"));
        assert_eq!(field(&reopened, "ok"), Value::Bool(true), "{reopened:?}");
        assert_eq!(string_field(&reopened, "origin"), "bouncy");
        // New body: `decay` goes, `gain` comes, and a comment with quotes.
        let body = "; the \"gain\" card\n(* k gain)";
        let result = eval(
            &mut ui,
            &format!(
                "(graph-node-process-expr-set \"{GRAPH}\" 1 {a} (str \"; the \" (string-from-char-code 34) \"gain\" (string-from-char-code 34) \" card\n(* k gain)\"))"
            ),
        );
        assert_eq!(field(&result, "ok"), Value::Bool(true), "{result:?}");
        node_eval(&mut ui, 1, "graph-node-process-inlet", &format!("{a} :gain 2"));

        let check = eval(&mut ui, &format!("(graph-node-process-promote-check \"{GRAPH}\" \"bouncy\")"));
        assert_eq!((field(&check, "ok"), field(&check, "update")), (Value::Bool(true), Value::Bool(true)), "{check:?}");
        let check = eval(&mut ui, &format!("(graph-node-process-promote-check \"{GRAPH}\" \"fresh\")"));
        assert_eq!(field(&check, "update"), Value::Bool(false), "{check:?}");
        // Unconfirmed: refused, nothing written. Builtins: never replaced.
        let path = dir.path().join("packages/user.processes/src/bouncy.lisp");
        let v1 = std::fs::read_to_string(&path).unwrap();
        let refused = node_eval(&mut ui, 1, "graph-node-process-promote", &format!("{a} \"bouncy\""));
        assert!(string_field(&refused, "error").contains("confirm to update"), "{refused:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), v1);
        for taken in ["delay", "neural-delay", "transpose"] {
            let refused = node_eval(&mut ui, 1, "graph-node-process-promote", &format!("{a} \"{taken}\" true"));
            assert!(string_field(&refused, "error").contains("already exists"), "{taken}: {refused:?}");
        }

        let written = node_eval(&mut ui, 1, "graph-node-process-promote", &format!("{a} \"bouncy\" true"));
        assert_eq!((field(&written, "ok"), field(&written, "update")), (Value::Bool(true), Value::Bool(true)), "{written:?}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("(gain :float -1000000 1000000 :default 2 :lane true)"), "{text}");
        assert!(!text.contains("(decay "), "{text}");
        assert!(text.contains("(string-from-char-code 34)"), "{text}");
        assert!(text.contains(":doc \"the 'gain' card\""), "{text}");
        assert_eq!(eval(&mut ui, &format!("(load \"{}\")", path.display())), Value::String(class.clone()), "reload");
        let rebind = node_eval(&mut ui, 1, "graph-node-process-rebind-class", &format!("{a} \"{class}\""));
        assert_eq!(field(&rebind, "ok"), Value::Bool(true), "{rebind:?}");

        let defs: Vec<_> =
            state.published_process_authoring().defs.into_iter().filter(|def| def.name == class).collect();
        assert_eq!(defs.len(), 1, "a reload replaces the class");
        assert_eq!(defs[0].expr_source.as_deref(), Some(body), "the quoted body round-trips exactly");
        assert_eq!(
            defs[0].inlets.iter().map(|inlet| inlet.name.as_str()).collect::<Vec<_>>(),
            vec!["k", "gain"]
        );
        assert_eq!(slot_of(&state, a).class_name, class);
        assert_eq!(slot_of(&state, b).class_name, class);
        // Both cards run the new body: 4 * 2 each (b takes the new defaults).
        assert_eq!(sent(&state), vec![16.0, 16.0, 16.0, 16.0]);
        assert_eq!(state.process_run_error(a), None);
        assert_eq!(state.process_run_error(b), None);
        // The chain read of the other card lists the new inlets.
        let Value::List(slots) = node_eval(&mut ui, 1, "graph-node-process-chain", "") else { panic!() };
        let entry = slots[1].borrow().clone();
        assert_eq!(field(&entry, "known"), Value::Bool(true));
        assert_eq!(field(&entry, "promoted-expr"), Value::Bool(true));
        // Restart: the updated file loads the same way.
        drop(ui);
        let (_ui, _authoring) = promote_ui_runtime(&state);
        assert_eq!(sent(&state), vec![16.0, 16.0, 16.0, 16.0]);
    });
}

// ---- eseq-waa9.23: node process-chain edits are undoable ----

/// The chains (before, after) the last `graph-node-process-*` edit queued
/// for the host's history, draining the queue.
fn last_history_chains(
    ui: &mut eseqlisp::Editor,
) -> (Option<crate::process::TrackProcessChain>, Option<crate::process::TrackProcessChain>) {
    let payload = ui
        .drain_host_commands()
        .into_iter()
        .filter_map(|command| match command {
            eseqlisp::HostCommand::Custom { name, payload }
                if name == lisp_host::GRAPH_NODE_PROCESS_HISTORY_COMMAND =>
            {
                Some(payload)
            }
            _ => None,
        })
        .last()
        .expect("the edit queued a history record");
    let chain = |key: &str| match field(&payload, key) {
        Value::String(json) => Some(serde_json::from_str(&json).expect("chain json")),
        Value::Nil => None,
        other => panic!("{key}: {other:?}"),
    };
    (chain("before"), chain("after"))
}

/// Undoing an expr commit restores the previous body AND the scheduler runs
/// the previous hashed class again, also in a session that never compiled
/// it (the restore registers classes from the bodies on the slots).
#[test]
fn expr_commit_undo_restores_the_previous_class_on_the_scheduler() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut editor = eseqlisp::Editor::new(ui_runtime(&state), eseqlisp::EditorConfig::default());
        let expr = add_slot(editor.runtime_mut(), "expr");
        let transpose = add_slot(editor.runtime_mut(), "neural-transpose");
        set_source(editor.runtime_mut(), expr, "(* x rate)");
        eval(editor.runtime_mut(), &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {expr} :x 2)"));
        eval(editor.runtime_mut(), &format!("(graph-node-process-inlet \"{GRAPH}\" 1 {expr} :rate 3)"));
        eval(
            editor.runtime_mut(),
            &format!("(graph-node-process-wire \"{GRAPH}\" 1 {expr} \"wire\" {transpose} \"amount\")"),
        );
        assert_node1_transposes(&state, 8.0, "2 + (* 2 3)");
        let old_class = node1_chain(&state).slots[0].class_name.clone();
        editor.drain_host_commands();

        set_source(editor.runtime_mut(), expr, "(+ x rate)");
        assert_node1_transposes(&state, 7.0, "2 + (+ 2 3)");
        let (before, after) = last_history_chains(&mut editor);
        assert_eq!(after.as_ref(), Some(&node1_chain(&state)));
        let before = before.expect("a chain before the commit");
        assert_eq!(before.slots[0].class_name, old_class);
        assert_eq!(before.slots[0].expr_source.as_deref(), Some("(* x rate)"));

        let scene = state.current_scene_id().expect("a scene");
        let id = state.published_sequencers()[0].id;
        lisp_host::restore_graph_node_process_chain(&state, scene, id, 1, Some(before.clone()))
            .expect("undo");
        assert_eq!(node1_chain(&state), before, "the previous body, class and wire");
        assert_node1_transposes(&state, 8.0, "undo: the previous class runs again");
        lisp_host::restore_graph_node_process_chain(&state, scene, id, 1, after.clone()).expect("redo");
        assert_node1_transposes(&state, 7.0, "redo: the new class runs");

        // A session that never compiled the old body (the undo entry
        // outlived a reload of the class table): the restore compiles it.
        let fresh = expr_graph_state();
        assert!(!fresh.has_expr_process_def(&old_class));
        let scene = fresh.current_scene_id().expect("a scene");
        let id = fresh.published_sequencers()[0].id;
        lisp_host::restore_graph_node_process_chain(&fresh, scene, id, 1, Some(before)).expect("restore");
        assert!(fresh.has_expr_process_def(&old_class), "registered from the slot's body");
        assert_node1_transposes(&fresh, 8.0, "the restored class runs");
    });
}

/// Undo writes back into the scene the edit was made in, even after the
/// transport moved to another scene.
#[test]
fn node_process_restore_targets_the_recorded_scene() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut editor = eseqlisp::Editor::new(ui_runtime(&state), eseqlisp::EditorConfig::default());
        add_slot(editor.runtime_mut(), "neural-transpose");
        let (before, _) = last_history_chains(&mut editor);
        assert_eq!(before, None, "the node had no chain");
        let scene = state.current_scene_id().expect("a scene");
        let id = state.published_sequencers()[0].id;
        let missing = crate::sequencer::SceneId(scene.0 + 1_000_000);
        assert!(
            lisp_host::restore_graph_node_process_chain(&state, missing, id, 1, None).is_err(),
            "a scene that no longer exists fails the replay"
        );
        assert_eq!(node1_chain(&state).slots.len(), 1, "a failed replay changes nothing");
        lisp_host::restore_graph_node_process_chain(&state, scene, id, 1, None).expect("undo add");
        assert!(node1_chain(&state).slots.is_empty());
    });
}

/// The history payload carries the whole chain: every slot field survives
/// the JSON round trip (expr body, class, inlets incl. non-dyadic floats,
/// wires, fan-out ranges, map targets, disabled flag, unbound ports, names,
/// lanes), and restoring it reproduces the chain exactly.
#[test]
fn node_process_history_json_round_trips_a_rich_chain() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut editor = eseqlisp::Editor::new(ui_runtime(&state), eseqlisp::EditorConfig::default());
        let ui = editor.runtime_mut();
        let expr = expr_card_on(ui, 1, "(* x rate)");
        let transpose = add_slot_on(ui, 1, "neural-transpose");
        let spare = add_slot_on(ui, 1, "neural-transpose");
        node_eval(ui, 1, "graph-node-process-inlet", &format!("{expr} :x 0.1"));
        node_eval(ui, 1, "graph-node-process-inlet", &format!("{expr} :rate 0.3333333333333333"));
        node_eval(ui, 1, "graph-node-process-wire", &format!("{expr} \"wire\" {transpose} \"amount\""));
        node_eval(ui, 1, "graph-node-process-fanout-add", &format!("{expr} \"wire\" {spare} \"amount\""));
        map_out_to_transpose(ui, 1, spare);
        node_eval(ui, 1, "graph-node-process-enable", &format!("{transpose} false"));
        let (_, after) = last_history_chains(&mut editor);
        let live = node1_chain(&state);
        assert_eq!(after.as_ref(), Some(&live), "the payload's after is the live chain");
        assert!(live.slots.iter().any(|slot| !slot.enabled));
        assert!(live.slots.iter().any(|slot| !slot.fanout.is_empty()));
        assert!(live.slots.iter().any(|slot| slot.expr_source.is_some()));

        // Fields the natives above do not reach, set directly.
        let mut rich = live.clone();
        rich.slots[1].instance_name = Some("named".to_string());
        rich.slots[1].unbound_ports.insert("out".to_string());
        rich.slots[1].lanes.insert(
            "amount".to_string(),
            crate::process::ProcessLane { values: vec![0.1, -1.0e-7, 3.4e38] },
        );
        rich.slots[1].bindings.insert("wire".to_string(), None);
        rich.slots[2].project_layer = true;
        let json = serde_json::to_string(&rich).expect("serialize");
        let back: crate::process::TrackProcessChain = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, rich, "JSON round trip is lossless");

        let scene = state.current_scene_id().expect("a scene");
        let id = state.published_sequencers()[0].id;
        lisp_host::restore_graph_node_process_chain(&state, scene, id, 1, Some(back.clone()))
            .expect("restore");
        assert_eq!(node1_chain(&state), back, "the restore writes the chain verbatim");
    });
}

/// Undoing a delete brings the slot's id back; a card added afterwards
/// mints a fresh id in the node band that collides with none of them.
#[test]
fn node_process_undo_delete_then_add_mints_a_fresh_id() {
    run_with_scheduler_stack(|| {
        let state = expr_graph_state();
        let mut editor = eseqlisp::Editor::new(ui_runtime(&state), eseqlisp::EditorConfig::default());
        let a = add_slot_on(editor.runtime_mut(), 1, "neural-transpose");
        let b = add_slot_on(editor.runtime_mut(), 1, "neural-transpose");
        editor.drain_host_commands();
        node_eval(editor.runtime_mut(), 1, "graph-node-process-remove", &format!("{b}"));
        let (before, after) = last_history_chains(&mut editor);
        assert_eq!(after.as_ref().map(|chain| chain.slots.len()), Some(1));
        let scene = state.current_scene_id().expect("a scene");
        let id = state.published_sequencers()[0].id;
        lisp_host::restore_graph_node_process_chain(&state, scene, id, 1, before).expect("undo");
        let restored: Vec<u64> = node1_chain(&state).slots.iter().map(|slot| slot.instance_id.0).collect();
        assert_eq!(restored, vec![a, b]);
        let c = add_slot_on(editor.runtime_mut(), 1, "neural-transpose");
        assert!(c != a && c != b, "fresh id {c} vs {a}, {b}");
        assert!(c >= 1 << 45 && c > b, "node band, above every minted id");
        let ids: std::collections::BTreeSet<u64> =
            node1_chain(&state).slots.iter().map(|slot| slot.instance_id.0).collect();
        assert_eq!(ids.len(), 3);
    });
}
