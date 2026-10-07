//! The factory packages and demo scripts ported to the kinds (kind-bindings
//! spec §13 stage 8, eseq-0l17.20): alez.jaki's panel over its generator's
//! marks, the event-view demos over `transport.track-events`, the band demo
//! over its tracks' processes and the neural track router over its
//! `network` and `neuron`s.

use super::views::{assert_ported, instance_bindings, widget_keyed, widgets_with_prop};
use super::*;
use eseqlisp::layout::LayoutNode;
use sequencer::neural::NeuralMaxPolySelection;

/// The ported files' sources.
const PORTED: [(&str, &str); 5] = [
    (
        "packages/alez.jaki/src/kind.lisp",
        include_str!("../../../../../../content/packages/alez.jaki/src/kind.lisp"),
    ),
    (
        "scripts/processes/process-ui-control-demo.lisp",
        include_str!("../../../../../../content/scripts/processes/process-ui-control-demo.lisp"),
    ),
    (
        "scripts/sequencers/band-coupling-matrix-demo.lisp",
        include_str!("../../../../../../content/scripts/sequencers/band-coupling-matrix-demo.lisp"),
    ),
    (
        "scripts/sequencers/jaki-builder-demo.lisp",
        include_str!("../../../../../../content/scripts/sequencers/jaki-builder-demo.lisp"),
    ),
    (
        "scripts/sequencers/neural-8x8-track-router.lisp",
        include_str!("../../../../../../content/scripts/sequencers/neural-8x8-track-router.lisp"),
    ),
];

#[test]
fn ported_packages_and_demos_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

impl Harness {
    /// `count` tracks in all (blank samplers added after the project's two).
    fn pkg_tracks(&mut self, count: usize) {
        while self.app.tracks.len() < count {
            self.app
                .graph_controller()
                .add_blank_sampler_track()
                .expect("sampler track");
        }
        self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.sync();
    }

    /// Load a factory script as the script picker does, then sync and render.
    fn pkg_load(&mut self, script: &str) {
        self.eval(&format!(r#"(load "@/scripts/{script}")"#));
        self.drain();
        self.sync();
        self.show_all();
    }

    /// The transport plays with the audio clock at `sample` (stopped: None).
    fn pkg_play_at(&mut self, sample: Option<u64>) {
        let state = &self.shared.state;
        state
            .transport
            .playing
            .store(sample.is_some(), Ordering::Relaxed);
        state.set_audio_rendered_sample(sample.unwrap_or(0));
    }

    fn pkg_render(&mut self) {
        self.sync();
        self.show_all();
    }
}

/// Whether `widget`'s `prop` is a binding of `field` on instance `id`.
fn binds(widget: &HashMap<String, Value>, prop: &str, id: InstanceId, field: &str) -> bool {
    let mut bound = Vec::new();
    instance_bindings(&widget[prop], &mut bound, &mut Vec::new());
    bound == [(id, field.to_string())]
}

#[test]
fn the_jaki_panel_reads_its_generators_marks() {
    let mut h = Harness::new();
    h.eval("(import alez.jaki.kind)");
    let before = crate::host_commands::instances::instance_ids(&h.app);
    let mut create = HashMap::new();
    create.insert(
        "kind".to_string(),
        Rc::new(RefCell::new(s("alez/jaki:jaki"))),
    );
    h.command("instance-create", Value::Map(create));
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created.difference(&before).next().expect("a jaki");
    h.pkg_render();
    let buffer = "*jaki · jaki 1*";
    // Row 0 sounds (it routes track 1): before its route slot's first stamp
    // the slot lights nothing.
    let (tree, _) = h.buffer_tree(buffer);
    let slot = widget_keyed(&tree, "jaki-mods-0").expect("row 0's slot");
    assert_eq!(slot["lit"], number(0.0));

    // The first stamps register the marks; the panel then binds row 0's
    // slot to its route slot's mark and the hit strip reads the tick's.
    let state = h.shared.state.clone();
    state.push_generator_mark(id, "0", 100, 5.0);
    state.push_generator_mark(id, "", 100, 1.0);
    h.pkg_play_at(Some(200));
    h.pkg_render();
    h.pkg_render();
    let mark = |h: &mut Harness, key: &str| match h.eval(&format!(
        r#"(import eseq.kinds :refer (generator-of generator-mark-named))
           (generator-mark-named (generator-of (instance-ref {id})) "{key}")"#
    )) {
        Value::Instance(mark) => mark,
        other => panic!("mark {key:?}: {other:?}"),
    };
    let route_mark = mark(&mut h, "0");
    let (tree, revision) = h.buffer_tree(buffer);
    let slot = widget_keyed(&tree, "jaki-mods-0").expect("row 0's slot");
    assert!(
        binds(&slot, "lit", route_mark, "value"),
        "{:?}",
        slot["lit"]
    );
    let tick_mark = mark(&mut h, "");
    assert!(h.rt().host_field_observed(tick_mark, "value"));
    let hit = widget_keyed(&tree, "jaki-hit-0").expect("the hit strip's first cell");
    assert_eq!(
        hit["background-color"],
        Value::Keyword("process-lane-accent".into()),
        "tick 0 sounds: its cell lights"
    );

    // A later hit under the same key repaints the slot and re-renders
    // nothing else.
    state.push_generator_mark(id, "0", 300, 9.0);
    h.pkg_play_at(Some(400));
    h.pkg_render();
    assert_eq!(h.buffer_tree(buffer).1, revision, "the hit only repaints");
    let Value::ReactiveRef { slot, .. } = &slot["lit"] else {
        panic!("a binding");
    };
    assert_eq!(read_float_slot(slot), 9.0);
    h.pkg_play_at(None);
}

#[test]
fn the_event_view_demos_bind_the_tracks_output_events() {
    for (script, buffer) in [
        ("processes/process-ui-control-demo.lisp", "*process-ui*"),
        ("sequencers/band-coupling-matrix-demo.lisp", "*band-matrix*"),
    ] {
        let mut h = Harness::new();
        h.pkg_tracks(4);
        h.pkg_load(script);
        let (tree, _) = h.buffer_tree(buffer);
        let mut views = Vec::new();
        widgets_with_prop(&tree, "current-beat", &mut views);
        let view = views
            .pop()
            .unwrap_or_else(|| panic!("{script}: its event view"));
        let transport = h.singleton(TRANSPORT);
        assert!(
            binds(&view, "current-beat", transport, "track-events-beat"),
            "{script}: the beat binds"
        );
        assert_eq!(view["events"], h.eval("transport.track-events"), "{script}");
        assert_eq!(
            view["color-palette"],
            h.eval("(map (lambda (t) t.color) (tracks))"),
            "{script}: the tracks' colors"
        );
        assert!(
            h.rt().host_field_observed(transport, "track-events"),
            "{script}: the history is computed while the view shows"
        );
    }
}

#[test]
fn the_band_demo_shows_its_tracks_inlets() {
    let mut h = Harness::new();
    h.pkg_tracks(4);
    h.pkg_load("sequencers/band-coupling-matrix-demo.lisp");
    // An inlet reads back from the track's chain (here a default project
    // lane's), anything the chain lacks from the panel's own edits.
    assert_eq!(
        h.eval(r#"(band-inlet (band-process 2 "lane-rand") "hi" -1)"#),
        number(12.0)
    );
    assert_eq!(
        h.eval(r#"(band-inlet (band-process 2 "lane-rand") "none" -1)"#),
        number(-1.0)
    );
    assert_eq!(h.eval(r#"(band-process 7 "band-ear")"#), Value::Nil);
    assert_eq!(h.eval(r#"(band-inlet nil "a0" -1)"#), number(-1.0));
    h.eval("(band-set-cell 0 2 0.9) (band-set-lag 1 4) (band-set-coupling 1.5)");
    h.pkg_render();
    let (tree, _) = h.buffer_tree("*band-matrix*");
    let matrix = widget_keyed(&tree, "band-cell-matrix").expect("the matrix");
    assert_eq!(items(&items(&matrix["value"])[0])[2], number(0.9));
    let lag = widget_keyed(&tree, "band-lag-1").expect("track 2's lag");
    assert_eq!(lag["value"], number(4.0));
    let coupling = widget_keyed(&tree, "band-coupling").expect("coupling");
    assert_eq!(coupling["value"], number(1.5));
}

// ── the neural track router ──────────────────────────────────────────────

const ROUTER: &str = "sequencers/neural-8x8-track-router.lisp";

/// The router's network in the engine's model.
fn router_network(h: &Harness) -> sequencer::neural::ProjectNeuralNetwork {
    let networks = h.shared.state.current_neural_networks();
    assert_eq!(networks.len(), 1, "one network");
    networks[0].clone()
}

fn by_key<'a>(node: &'a LayoutNode, key: &str) -> Option<&'a LayoutNode> {
    if node.stable_key.as_deref() == Some(key) {
        return Some(node);
    }
    node.children.iter().find_map(|child| by_key(child, key))
}

fn of_type<'a>(node: &'a LayoutNode, widget: &str, out: &mut Vec<&'a LayoutNode>) {
    if node.widget_type == widget {
        out.push(node);
    }
    for child in &node.children {
        of_type(child, widget, out);
    }
}

impl Harness {
    /// The router's panel laid out over an 80 × 18 viewport.
    fn router_layout(&mut self) -> std::sync::Arc<LayoutNode> {
        let (tree, _) = self.buffer_tree("*matrix*");
        self.editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 18.0)))
            .expect("the router lays out")
    }
}

#[test]
fn the_router_script_is_idempotent_and_routes_tracks() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.pkg_load(ROUTER);
    h.pkg_load(ROUTER);
    let network = router_network(&h);
    assert_eq!(network.name, "8x8-track-router2");
    assert_eq!(network.num_neurons, 8);
    assert!(network.enabled);
    assert_eq!(network.reset_interval_bars, 4.0);
    assert_eq!(network.energy_decay, 0.994);
    assert_eq!(network.max_poly, 2);
    assert_eq!(
        network.max_poly_selection,
        NeuralMaxPolySelection::Deterministic
    );
    let ring: Vec<Vec<f32>> = (0..8)
        .map(|from| (0..8).map(|to| f32::from(to == (from + 1) % 8)).collect())
        .collect();
    assert_eq!(network.weights, ring);
    let routes: Vec<_> = network.neurons.iter().map(|neuron| neuron.route).collect();
    assert_eq!(routes, (0..8).map(Some).collect::<Vec<_>>());
    for neuron in &network.neurons {
        assert_eq!(neuron.delay_steps, 1);
        assert!(neuron.quantize_timebase().is_none());
        assert_eq!(neuron.transpose, 0.0);
        assert_eq!(neuron.threshold, 1.0);
        assert_eq!(neuron.dampening_amount, 0.0);
        assert!((neuron.dampening_recovery - 0.98).abs() < f32::EPSILON);
    }
    assert!(!h.shared.state.pattern.neural_reset_patterns[0].is_active(0));
}

#[test]
fn the_router_reuses_its_named_network() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.pkg_load(ROUTER);
    let id = router_network(&h).id;
    h.eval(&format!(
        "(neural-weight {id} :from 0 :to 1 :value 0.25)
         (neural-set {id} :reset-bars 2 :energy-decay 0.5 :max-poly 4 :max-poly-selection :random)
         (neural-neuron {id} 1 :route 7 :threshold 1.5 :delay 4 :quantize :4 :transpose -7
           :dampening 0.25 :recovery 0.75)"
    ));
    h.pkg_load(ROUTER);
    let network = router_network(&h);
    assert_eq!(network.id, id);
    assert_eq!(network.reset_interval_bars, 2.0);
    assert_eq!(network.max_poly_selection, NeuralMaxPolySelection::Random);
    assert_eq!(network.weights[0][1], 0.25);
    assert_eq!(network.neurons[1].route, Some(7));
    assert_eq!(network.neurons[1].threshold, 1.5);
    assert_eq!(network.neurons[1].transpose, -7.0);
}

/// The panel reads the network's fields: an edit from elsewhere shows
/// without the panel writing anything back.
#[test]
fn the_router_panel_follows_the_network() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.pkg_load(ROUTER);
    let id = router_network(&h).id;
    let history = h.app.history.undo_len();
    h.shared
        .state
        .edit_current_neural_networks(|networks| {
            let network = &mut networks[0];
            network.reset_interval_bars = 3.0;
            network.max_poly_selection = NeuralMaxPolySelection::Random;
            network.weights[0][1] = 0.75;
            network.neurons[0].threshold = 1.75;
            network.neurons[1].route = Some(6);
            network.neurons[1].delay_steps = 5;
            network.neurons[1].quantize = Some(sequencer::sequencer::Timebase::Eighth as u8);
            Ok(())
        })
        .unwrap();
    h.pkg_render();
    let layout = h.router_layout();
    let mut pickers = Vec::new();
    of_type(&layout, "number-picker", &mut pickers);
    let value = |node: &LayoutNode| node.props.get("value").cloned().unwrap_or(Value::Nil);
    assert_eq!(value(pickers[0]), number(3.0), "bars");
    assert_eq!(
        value(pickers[3]),
        number(1.75),
        "the threshold shows the first neuron's"
    );
    let row = by_key(&layout, "neural-router-row-2").expect("neuron 2's row");
    let mut dropdowns = Vec::new();
    of_type(row, "dropdown", &mut dropdowns);
    assert_eq!(value(dropdowns[0]), s("Track 7"));
    assert_eq!(value(dropdowns[1]), s("8"));
    let mut matrices = Vec::new();
    of_type(&layout, "matrix", &mut matrices);
    let weights = matrices
        .iter()
        .find(|m| m.props.get("on-change").is_some())
        .expect("the weight matrix");
    assert_eq!(items(&items(&value(weights))[0])[1], number(0.75));
    assert_eq!(h.app.history.undo_len(), history, "nothing written back");
    assert_eq!(router_network(&h).id, id);
}

/// The panel's controls line up with the matrices' rows; a row's number
/// selects its neuron, which the row's bound selection lights; editing a
/// row's control sets the neuron's field.
#[test]
fn the_router_rows_align_select_and_edit() {
    let mut h = Harness::new();
    h.pkg_tracks(16);
    h.pkg_load(ROUTER);
    let layout = h.router_layout();
    for (key, text, width) in [
        ("neural-router-column-label-route", "route", 7.68),
        ("neural-router-column-label-delay", "delay", 5.04),
        ("neural-router-column-label-quantize", "quant", 5.76),
        ("neural-router-column-label-transpose", "transp", 5.04),
        ("neural-router-column-label-dampening", "damp", 5.04),
        ("neural-router-column-label-recovery", "recov", 5.04),
    ] {
        let label = by_key(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_eq!(label.props.get("text"), Some(&s(text)), "{key}");
        assert!(
            (label.rect.width - width).abs() <= 0.05,
            "{key}: {:?}",
            label.rect
        );
    }
    let mut matrices = Vec::new();
    of_type(&layout, "matrix", &mut matrices);
    assert_eq!(matrices.len(), 4, "triggers, energy, weights, dampening");
    matrices.sort_by(|a, b| a.rect.col.total_cmp(&b.rect.col));
    let weights = matrices[2];
    for matrix in &matrices {
        assert!(
            (matrix.rect.row - weights.rect.row).abs() <= 0.05
                && (matrix.rect.height - weights.rect.height).abs() <= 0.05,
            "{:?} aligns with {:?}",
            matrix.rect,
            weights.rect
        );
    }
    let mut pickers = Vec::new();
    of_type(&layout, "number-picker", &mut pickers);
    assert_eq!(pickers.len(), 36, "four global pickers, four per neuron");
    let mut row_pickers: Vec<_> = pickers
        .into_iter()
        .filter(|picker| {
            let center = picker.rect.row + picker.rect.height * 0.5;
            center >= weights.rect.row && center <= weights.rect.row + weights.rect.height
        })
        .collect();
    assert_eq!(row_pickers.len(), 32);
    row_pickers.sort_by(|a, b| {
        a.rect
            .row
            .total_cmp(&b.rect.row)
            .then(a.rect.col.total_cmp(&b.rect.col))
    });
    let pitch = weights.rect.height / 8.0;
    for (index, picker) in row_pickers.iter().enumerate() {
        let expected = weights.rect.row + pitch * ((index / 4) as f32 + 0.5);
        let center = picker.rect.row + picker.rect.height * 0.5;
        assert!(
            (center - expected).abs() <= 0.05,
            "picker {index}: {center} vs {expected}"
        );
    }
    let mut dropdowns = Vec::new();
    of_type(&layout, "dropdown", &mut dropdowns);
    assert_eq!(
        dropdowns.len(),
        17,
        "max-poly selection, route and quantize per neuron"
    );

    // The row's selection is the neuron's, bound.
    let (tree, revision) = h.buffer_tree("*matrix*");
    let row = widget_keyed(&tree, "neural-router-row-3").expect("row 3");
    let n2 = h.eval("(let ((nw (router-network))) (nth nw.neurons 2))");
    let Value::Instance(n2) = n2 else {
        panic!("neuron 2: {n2:?}");
    };
    assert!(binds(&row, "selected", n2, "selected"));
    let label = by_key(&layout, "neural-router-row-label-3").expect("row 3's number");
    let click = label.props.get("on-click").cloned().expect("on-click");
    h.editor
        .runtime_mut()
        .invoke(click, vec![Value::Bool(true)])
        .expect("select neuron 2");
    let id = router_network(&h).id;
    assert_eq!(
        h.eval(&format!("(neural-neuron-selected? {id} 2)")),
        Value::Bool(true)
    );
    h.pkg_render();
    assert_eq!(
        h.buffer_tree("*matrix*").1,
        revision,
        "selecting only repaints"
    );
    // The background clears the selection.
    let clear = layout
        .props
        .get("on-click")
        .cloned()
        .expect("the panel's click");
    h.editor
        .runtime_mut()
        .invoke(clear, vec![Value::Bool(true)])
        .expect("clear the selection");
    assert_eq!(h.eval("(neural-selected-neurons)"), Value::List(Vec::new()));

    // The route dropdown lists every track; picking one routes the neuron.
    let route = row_dropdown(&layout, "neural-router-row-1", 0);
    let options = items(route.props.get("options").expect("options"));
    assert_eq!(options.len(), 17, "16 tracks and Off");
    assert_eq!(options[15], s("Track 16"));
    let pick = route.props.get("on-change").cloned().expect("on-change");
    h.editor
        .runtime_mut()
        .invoke(pick, vec![s("Track 16")])
        .expect("route neuron 0");
    h.drain();
    h.sync();
    assert_eq!(router_network(&h).neurons[0].route, Some(15));
    let delay = row_picker(&layout, "neural-router-row-1", 0);
    let set = delay.props.get("on-change").cloned().expect("on-change");
    h.editor
        .runtime_mut()
        .invoke(set, vec![number(6.0)])
        .expect("delay neuron 0");
    h.drain();
    h.sync();
    assert_eq!(router_network(&h).neurons[0].delay_steps, 6);
}

/// The panel's threshold sets every neuron's as one edit (a drag one entry,
/// undone whole); without its network the panel offers to create it, never
/// calling a native itself.
#[test]
fn the_router_sets_thresholds_once_and_offers_to_create() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.pkg_load(ROUTER);

    // The threshold: a drag over every neuron is one entry.
    let layout = h.router_layout();
    let mut pickers = Vec::new();
    of_type(&layout, "number-picker", &mut pickers);
    let threshold = pickers[3].props["on-change"].clone();
    let entries = h.app.history.undo_len();
    h.gesture.pointer_down = true;
    for v in [0.5, 0.75, 1.25] {
        h.editor
            .runtime_mut()
            .invoke(threshold.clone(), vec![number(v)])
            .expect("set the threshold");
        h.drain();
        h.sync();
    }
    h.gesture.pointer_down = false;
    app::edit::finish_active_gesture(&mut h.app);
    assert_eq!(h.app.history.undo_len(), entries + 1, "one drag entry");
    let thresholds = |h: &Harness| -> Vec<f32> {
        let network = router_network(h);
        network.neurons.iter().map(|n| n.threshold).collect()
    };
    assert_eq!(thresholds(&h), vec![1.25; 8]);
    app::edit::undo(&mut h.app);
    h.sync();
    assert_eq!(thresholds(&h), vec![1.0; 8], "undone whole");

    // Without its network: a message and a Create button, which makes it.
    let id = router_network(&h).id;
    h.eval(&format!("(neural-delete {id})"));
    h.pkg_render();
    assert!(h.shared.state.current_neural_networks().is_empty());
    let (tree, _) = h.buffer_tree("*matrix*");
    let create = widget_keyed(&tree, "neural-router-create").expect("the Create button");
    h.editor
        .runtime_mut()
        .invoke(create["on-click"].clone(), vec![Value::Bool(true)])
        .expect("create the network");
    h.pkg_render();
    assert_eq!(router_network(&h).num_neurons, 8);
    let (tree, _) = h.buffer_tree("*matrix*");
    assert!(widget_keyed(&tree, "neural-router-create").is_none());
    assert!(widget_keyed(&tree, "neural-router-row-1").is_some());
}

fn row_dropdown<'a>(layout: &'a LayoutNode, row: &str, index: usize) -> &'a LayoutNode {
    let row = by_key(layout, row).unwrap_or_else(|| panic!("{row}"));
    let mut dropdowns = Vec::new();
    of_type(row, "dropdown", &mut dropdowns);
    dropdowns.sort_by(|a, b| a.rect.col.total_cmp(&b.rect.col));
    dropdowns[index]
}

fn row_picker<'a>(layout: &'a LayoutNode, row: &str, index: usize) -> &'a LayoutNode {
    let row = by_key(layout, row).unwrap_or_else(|| panic!("{row}"));
    let mut pickers = Vec::new();
    of_type(row, "number-picker", &mut pickers);
    pickers.sort_by(|a, b| a.rect.col.total_cmp(&b.rect.col));
    pickers[index]
}
