//! The graph demo scripts ported to the kinds (kind-bindings spec §13 stage
//! 8, eseq-0l17.64): their panels read and edit their `graph`, its
//! `graph-node`s and `graph-param`s, playback and the event streams
//! (§14.2k, §14.2s), through the shared `eseq.graph-kit`. Moved here from
//! the bare-runtime layout tests (`lisp_host::tests`, the graph
//! visualization and rack restore tests).

use super::packages_view::{binds, by_key, of_type};
use super::views::{assert_ported, distro, widget_keyed};
use super::*;
use eseqlisp::layout::LayoutNode;
use sequencer::graph::{
    GraphPayload, GraphVisualizationEdge, GraphVisualizationEvent, GraphVisualizationSnapshot,
    ProjectGraphEdgeParamOverride, ProjectGraphNodeIntrinsicOverride,
    ProjectGraphNodeParamOverride, ProjectGraphOverrides, ProjectGraphQuantizeOverride,
    ProjectGraphRouteOverride, ProjectGraphSeedFrom,
};
use sequencer::neural::NeuralMaxPolySelection;
use sequencer::sequencer::Timebase;

macro_rules! demo_source {
    ($file:literal) => {
        (
            concat!("scripts/sequencers/", $file),
            include_str!(concat!(
                "../../../../../../content/scripts/sequencers/",
                $file
            )),
        )
    };
}

/// The ported files' sources.
const PORTED: [(&str, &str); 8] = [
    demo_source!("graph-markov-8x8-demo.lisp"),
    demo_source!("graph-neural-16-cycle-demo.lisp"),
    demo_source!("graph-neural-16-demo.lisp"),
    demo_source!("graph-neural-8x8-demo.lisp"),
    demo_source!("graph-neural-8x8-reset-demo.lisp"),
    demo_source!("graph-neural-group-matrix-demo.lisp"),
    demo_source!("graph-neural-variable-reset-demo.lisp"),
    (
        "ui/graph-kit.lisp",
        include_str!("../../../../../../content/ui/graph-kit.lisp"),
    ),
];

#[test]
fn ported_graph_demos_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
}

/// A demo script: its file, its sequencer's name, its handle's prefix
/// (`<prefix>-name`), its buffer, its tab label and its widget keys' prefix.
struct Demo {
    file: &'static str,
    name: &'static str,
    prefix: &'static str,
    buffer: &'static str,
    tab: &'static str,
    key: &'static str,
}

const MARKOV: Demo = Demo {
    file: "graph-markov-8x8-demo.lisp",
    name: "markov-8x8-demo",
    prefix: "m8",
    buffer: "*markov-8x8*",
    tab: "Markov 8x8",
    key: "markov-8x8",
};
const CYCLE: Demo = Demo {
    file: "graph-neural-16-cycle-demo.lisp",
    name: "neural-16-cycle-demo",
    prefix: "g16c",
    buffer: "*16x16-cycle*",
    tab: "16x16 cyc",
    key: "graph-16",
};
const SIXTEEN: Demo = Demo {
    file: "graph-neural-16-demo.lisp",
    name: "neural-16-demo",
    prefix: "g16",
    buffer: "*16x16*",
    tab: "16x16",
    key: "graph-16",
};
const EIGHT: Demo = Demo {
    file: "graph-neural-8x8-demo.lisp",
    name: "neural-8x8-demo",
    prefix: "g8",
    buffer: "*8x8*",
    tab: "8x8",
    key: "graph-8x8",
};
const RESET: Demo = Demo {
    file: "graph-neural-8x8-reset-demo.lisp",
    name: "neural-8x8-reset-demo",
    prefix: "g8r",
    buffer: "*8x8-reset*",
    tab: "8x8 rst",
    key: "graph-8x8-reset",
};
const GROUPS: Demo = Demo {
    file: "graph-neural-group-matrix-demo.lisp",
    name: "neural-group-matrix-demo",
    prefix: "ggm",
    buffer: "*group-matrix*",
    tab: "grp mtx",
    key: "graph-group-matrix",
};
const VARIABLE: Demo = Demo {
    file: "graph-neural-variable-reset-demo.lisp",
    name: "neural-variable-reset-demo",
    prefix: "gvr",
    buffer: "*variable-reset*",
    tab: "var rst",
    key: "graph-variable-reset",
};

const DEMOS: [&Demo; 7] = [
    &MARKOV, &CYCLE, &SIXTEEN, &EIGHT, &RESET, &GROUPS, &VARIABLE,
];

/// What the tests evaluate with: the kinds and the kit the demos use.
const REFER_DEMO: &str = "(import eseq.kinds :refer (track tracks graph-of graph-param-named))
                          (import eseq.view-kit :refer (rgb-part))
                          (import eseq.graph-kit :refer (route-options weight-rows))";

impl Harness {
    fn demo_eval(&mut self, code: &str) -> Value {
        self.eval_with(REFER_DEMO, code)
    }

    /// Load `demo` as the script picker does; the panel renders once the
    /// host publishes the graph.
    fn demo_load(&mut self, demo: &Demo) {
        self.pkg_load(&format!("sequencers/{}", demo.file));
    }

    /// `demo`'s graph (as Lisp).
    fn demo_graph(demo: &Demo) -> String {
        format!("(graph-of {}-name)", demo.prefix)
    }

    /// `body` evaluated with `g` bound to `demo`'s graph.
    fn demo_with(&mut self, demo: &Demo, body: &str) -> Value {
        self.demo_eval(&format!(
            "(let ((g (graph-of {}-name))) {body})",
            demo.prefix
        ))
    }

    /// The instance `code` evaluates to.
    fn demo_instance(&mut self, code: &str) -> InstanceId {
        match self.demo_eval(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }

    /// `buffer`'s widget tree and revision, with the renders a cycle left
    /// pending for a buffer off screen applied.
    fn demo_tree(&mut self, buffer: &str) -> (Value, u64) {
        self.editor.refresh_runtime_side_effects();
        self.buffer_tree(buffer)
    }

    /// `buffer`'s panel laid out over a `cols` × `rows` viewport.
    fn demo_layout(&mut self, buffer: &str, cols: f32, rows: f32) -> std::sync::Arc<LayoutNode> {
        let (tree, _) = self.demo_tree(buffer);
        self.editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((cols, rows)))
            .unwrap_or_else(|| panic!("{buffer} lays out"))
    }

    /// Invoke widget `key`'s `prop` callback with `args`, then apply what it
    /// queued and render, as a frame does.
    fn demo_invoke(&mut self, layout: &LayoutNode, key: &str, prop: &str, args: Vec<Value>) {
        let callback = by_key(layout, key)
            .and_then(|node| node.props.get(prop))
            .cloned()
            .unwrap_or_else(|| panic!("{key} has {prop}"));
        self.editor
            .runtime_mut()
            .invoke(callback, args)
            .unwrap_or_else(|error| panic!("{key} {prop}: {error:?}"));
        self.drain();
        self.pkg_render();
    }

    /// The current scene's overrides of the graph whose sequencer id is `id`.
    fn demo_overrides(&self, id: u64) -> ProjectGraphOverrides {
        self.shared
            .state
            .current_graph_overrides()
            .into_iter()
            .find(|graph| graph.sequencer_id == id)
            .unwrap_or_else(|| panic!("overrides of {id}"))
    }

    /// Whether graph `name`, with its current overrides, fires past a seed
    /// on track 0 within a beat (the engine's own update, as the scheduler
    /// runs it).
    fn demo_propagates(&self, name: &str) -> bool {
        let state = &self.shared.state;
        let manifest = (state.published_sequencers().into_iter())
            .find(|published| published.name == name)
            .and_then(|published| published.graph)
            .expect("the graph's manifest");
        let overrides = (state.current_graph_overrides().into_iter())
            .find(|graph| graph.sequencer_id == manifest.id);
        let mut graph = manifest.materialize_with_overrides(overrides.as_ref());
        let payload = GraphPayload {
            note: 0.0,
            velocity: 1.0,
            duration_beats: 0.25,
        };
        assert_eq!(graph.seed(0, 0.0, payload), 1, "track 0 seeds one node");
        let tracks = self.app.tracks.len();
        let instruments = (0..tracks)
            .map(|_| sequencer::effects::EffectDescriptor::builtin_delay())
            .collect();
        let effects = (0..tracks)
            .map(|_| sequencer::effects::EffectDescriptor::default_full_chain())
            .collect();
        let mut scratch = sequencer::lisp_host::ScratchControlRuntime::new(
            state.clone(),
            effects,
            instruments,
            0,
            0,
        );
        let mut emissions = Vec::new();
        let mut start = 0.0_f64;
        while start < 1.0 {
            let end = (start + 0.021_f64).min(1.0);
            graph.process_block(
                start,
                end,
                0,
                48_000.0,
                manifest.max_poly,
                |eval| {
                    scratch
                        .invoke_graph_update(&manifest, eval)
                        .expect("the demo's update evaluates")
                },
                &mut emissions,
            );
            start = end;
        }
        !emissions.is_empty()
    }
}

/// The widgets of `layout` of type `widget`.
fn count(layout: &LayoutNode, widget: &str) -> usize {
    let mut found = Vec::new();
    of_type(layout, widget, &mut found);
    found.len()
}

/// Widget `key` of `layout`, laid out with a finite, non-empty rect.
fn measured<'a>(layout: &'a LayoutNode, key: &str) -> &'a LayoutNode {
    let node = by_key(layout, key).unwrap_or_else(|| panic!("{key} shows"));
    let rect = node.rect;
    assert!(
        rect.row.is_finite() && rect.col.is_finite() && rect.width > 0.0 && rect.height > 0.0,
        "{key}: {rect:?}"
    );
    node
}

fn prop(node: &LayoutNode, name: &str) -> Value {
    node.props.get(name).cloned().unwrap_or(Value::Nil)
}

fn strings(values: &[&str]) -> Value {
    list_value(values.iter().map(|value| s(value)))
}

/// Row `r`, column `c` of a matrix value.
fn cell(matrix: &Value, r: usize, c: usize) -> Value {
    items(&items(matrix)[r])[c].clone()
}

/// A bound prop's current value.
fn slot_value(value: &Value) -> f64 {
    match value {
        Value::ReactiveRef { slot, .. } => read_float_slot(slot),
        other => panic!("not a binding: {other:?}"),
    }
}

/// Show `buffer` alone in the active window, so its subtrees re-run in
/// place (an off-screen buffer's renders wait for it to show).
fn show_on_screen(h: &mut Harness, buffer: &str) {
    h.eval(&format!(
        "(set-layout (list :buf \"{buffer}\" :hide-status true))"
    ));
    let id = (h.editor.buffers.iter())
        .find(|item| item.name == buffer)
        .map(|item| item.id)
        .unwrap_or_else(|| panic!("{buffer}"));
    h.editor.set_active_buffer(id);
    h.editor.set_layout_viewport(240, 100);
    h.pkg_render();
}

/// The (full-buffer, subtree) re-runs a render after `edit` makes.
fn reruns(h: &mut Harness, edit: impl FnOnce(&mut Harness)) -> (u64, u64) {
    h.pkg_render();
    let before = h.rt().ui_work_counters();
    edit(h);
    h.pkg_render();
    let after = h.rt().ui_work_counters();
    (
        after.full_buffer_reruns - before.full_buffer_reruns,
        after.subtree_reruns - before.subtree_reruns,
    )
}

/// The tabs registered for script step sequencers, as (label, buffer,
/// sequencer).
fn registered_tabs(h: &Harness) -> Vec<(Value, Value, Value)> {
    let tabs = (h
        .rt()
        .state_value("eseq.seq-step-tabs/seq-registered-step-tabs"))
    .expect("the registered tabs");
    items(&tabs)
        .iter()
        .map(|tab| {
            let tab = items(tab);
            (tab[0].clone(), tab[1].clone(), tab[2].clone())
        })
        .collect()
}

/// A drum rack whose pads play `tracks` in order; returns its group id.
fn demo_rack(h: &mut Harness, tracks: &[usize]) -> u64 {
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    for (pad, track) in tracks.iter().enumerate() {
        h.app
            .assign_rack_pad_track_recorded(
                group,
                sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE + pad as i32,
                *track,
            )
            .expect("pad");
    }
    h.share_buses_and_groups();
    h.sync();
    group
}

fn rack_members(h: &Harness, group: u64) -> Vec<usize> {
    let group = h
        .app
        .groups
        .iter()
        .find(|g| g.id == group)
        .expect("the rack");
    group.members.clone()
}

/// What a rack-owned graph's route menu lists: each member as its track's
/// number and name, then Off.
fn member_options(h: &Harness, members: &[usize]) -> Value {
    let labels = members
        .iter()
        .map(|track| s(&format!("{} {}", track + 1, h.app.tracks[*track])))
        .chain([s("Off")]);
    list_value(labels)
}

/// Every demo keeps the handle `def-sequencer` returns and loads without
/// writing overrides; project-owned, its routes are the project's tracks;
/// under a rack owner (as a rack replays its scripts) it publishes a
/// rack-owned instance, its tab wears the rack's name, its routes are the
/// rack's members (one that joins later included), and a route edit
/// stores a member index.
#[test]
fn every_graph_demo_loads_project_owned_and_rack_owned() {
    for demo in DEMOS {
        let mut h = distro();
        h.pkg_tracks(4);
        h.demo_load(demo);
        let file = demo.file;
        let project_id = sequencer::lisp_host::graph_instance_id(demo.name, None);
        assert_eq!(
            h.eval(&format!("{}-name", demo.prefix)),
            number(project_id as f64),
            "{file}"
        );
        assert_eq!(h.eval("script-sequencer-name"), s(demo.name), "{file}");
        assert!(
            h.shared.state.current_graph_overrides().is_empty(),
            "{file}: loading writes no override"
        );
        let tabs = registered_tabs(&h);
        assert!(
            tabs.contains(&(s(demo.tab), s(demo.buffer), s(demo.name))),
            "{file}: its step tab: {tabs:?}"
        );
        let graph = Harness::demo_graph(demo);
        assert_eq!(
            h.demo_eval(&format!("(route-options {graph})")),
            strings(&["Track 1", "Track 2", "Track 3", "Track 4", "Off"]),
            "{file}: the project's tracks and Off"
        );

        let group = demo_rack(&mut h, &[2, 0]);
        let load = format!(r#"(load "@/scripts/sequencers/{file}")"#);
        sequencer::lisp_host::with_graph_owner_rack(Some(group), || h.eval(&load));
        h.assert_no_load_errors(file);
        h.drain();
        h.pkg_render();
        let rack_id = sequencer::lisp_host::graph_instance_id(demo.name, Some(group));
        assert_eq!(
            h.eval(&format!("{}-name", demo.prefix)),
            number(rack_id as f64),
            "{file}"
        );
        let rack_name = (h.app.groups.iter())
            .find(|g| g.id == group)
            .map(|g| g.name.clone())
            .expect("the rack");
        assert_eq!(h.eval("script-tab-label"), s(&rack_name), "{file}");
        assert!(
            (h.shared.state.published_sequencers().iter()).any(|published| published.id == rack_id
                && published
                    .graph
                    .as_ref()
                    .is_some_and(|graph| graph.owner_rack == Some(group))),
            "{file}: a rack-owned instance"
        );
        let members = rack_members(&h, group);
        assert_eq!(
            h.demo_eval(&format!("(route-options {graph})")),
            member_options(&h, &members),
            "{file}: the rack's members"
        );
        // A member that joins the rack later shows up.
        h.app
            .assign_rack_pad_track_recorded(
                group,
                sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE + 2,
                3,
            )
            .expect("pad");
        h.share_buses_and_groups();
        h.pkg_render();
        let members = rack_members(&h, group);
        assert_eq!(members.len(), 3, "{file}");
        assert_eq!(
            h.demo_eval(&format!("(route-options {graph})")),
            member_options(&h, &members),
            "{file}: a later member"
        );
        // The panel's route menu stores the member's index.
        let layout = h.demo_layout(demo.buffer, 200.0, 120.0);
        let label = items(&member_options(&h, &members))[1].clone();
        h.demo_invoke(
            &layout,
            &format!("{}-route-0", demo.key),
            "on-change",
            vec![label],
        );
        assert_eq!(
            h.demo_overrides(rack_id).node_intrinsics[0].route,
            Some(ProjectGraphRouteOverride::Track(1)),
            "{file}"
        );
    }
}

/// The 8x8 panel: its widgets, the keyboard over the tracks' active notes,
/// controls bound to their fields and edited through them (one undo entry
/// each), the explicit init's ring (which propagates a seed) and the weight
/// matrix's cell edits (zeroed, the net is silent).
#[test]
fn the_8x8_demo_reads_and_edits_its_graph() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.demo_load(&EIGHT);
    let layout = h.demo_layout("*8x8*", 120.0, 60.0);
    assert_eq!(
        count(&layout, "matrix"),
        4,
        "weights, triggers, energy, dampening"
    );
    assert_eq!(count(&layout, "event-view"), 2);
    assert_eq!(count(&layout, "piano-keyboard"), 1);
    assert_eq!(count(&layout, "number-picker"), 8 * 5 + 3);
    assert_eq!(count(&layout, "dropdown"), 8 * 3);
    for key in [
        "graph-8x8-weight-matrix",
        "graph-8x8-trigger-matrix",
        "graph-8x8-energy-matrix",
        "graph-8x8-dampening-matrix",
        "graph-8x8-event-view",
        "graph-8x8-track-event-view",
        "graph-8x8-piano",
        "graph-8x8-reset-bars",
        "graph-8x8-max-poly",
        "graph-8x8-piano-press-depth",
    ] {
        measured(&layout, key);
    }
    for n in 0..8 {
        for field in [
            "route",
            "delay",
            "transpose",
            "vel-decay",
            "dampening",
            "recovery",
            "resolution",
            "quantize",
        ] {
            measured(&layout, &format!("graph-8x8-{field}-{n}"));
        }
    }
    let options = items(&prop(measured(&layout, "graph-8x8-quantize-0"), "options"));
    for label in ["2T", "4T", "8T", "16T", "32T", "64T"] {
        assert!(options.contains(&s(label)), "quantize offers {label}");
    }
    let piano = measured(&layout, "graph-8x8-piano");
    assert_eq!(prop(piano, "key-count"), number(80.0));
    assert_eq!(
        prop(piano, "tracks"),
        list_value((0..8).map(|t| number(t as f64)))
    );
    assert_eq!(
        prop(piano, "overlap-mode"),
        Value::Keyword("loudest".into())
    );
    assert_eq!(
        prop(piano, "notes-by-track"),
        h.demo_eval("(map (lambda (t) t.active-notes) (tracks))")
    );
    assert!(h.rt().host_field_observed(h.track_id(0), "active-notes"));

    // Controls bind their fields.
    let (tree, revision) = h.demo_tree("*8x8*");
    let g = Harness::demo_graph(&EIGHT);
    let transpose = h.demo_instance(&format!(
        "(let ((g {g})) (graph-param-named (nth g.nodes 2) \"transpose\"))"
    ));
    let picker = widget_keyed(&tree, "graph-8x8-transpose-2").expect("transpose 2");
    assert!(binds(&picker, "value", transpose, "value"));
    let node3 = h.demo_instance(&format!("(let ((g {g})) (nth g.nodes 3))"));
    let delay = widget_keyed(&tree, "graph-8x8-delay-3").expect("delay 3");
    assert!(binds(&delay, "value", node3, "delay"));
    let graph = h.demo_instance(&g);
    let bars = widget_keyed(&tree, "graph-8x8-reset-bars").expect("reset bars");
    assert!(binds(&bars, "value", graph, "reset-bars"));
    // An edit from elsewhere (a native) repaints without a re-render.
    h.eval("(graph-param g8-name 2 :transpose -12)");
    h.drain();
    h.pkg_render();
    assert_eq!(h.demo_tree("*8x8*").1, revision, "the edit only repaints");
    assert_eq!(slot_value(&picker["value"]), -12.0);

    // The explicit init writes the ring, which carries a seed on.
    let id = sequencer::lisp_host::graph_instance_id("neural-8x8-demo", None);
    let entries = h.app.history.undo_len();
    h.eval("(script-init-fn)");
    h.drain();
    h.pkg_render();
    assert_eq!(
        h.app.history.undo_len(),
        entries + 1,
        "the init's writes: one entry"
    );
    let graph = h.demo_overrides(id);
    assert_eq!(graph.edge_params.len(), 64, "the whole matrix");
    assert!(graph.edge_params.iter().any(|edge| {
        (edge.from, edge.to, edge.param.as_str(), edge.value) == (0, 1, "weight", 1.0)
    }));
    assert!(graph.node_intrinsics.iter().any(|node| {
        node.instance == 0 && node.seed_from == Some(ProjectGraphSeedFrom::Tracks(vec![0]))
    }));
    assert!(
        h.demo_propagates("neural-8x8-demo"),
        "the ring propagates a seed"
    );
    let layout = h.demo_layout("*8x8*", 120.0, 60.0);
    let weights = prop(by_key(&layout, "graph-8x8-weight-matrix").unwrap(), "value");
    assert_eq!(
        cell(&weights, 0, 1),
        number(1.0),
        "the panel shows the ring"
    );

    // Each control edits its field: one entry each.
    let entries = h.app.history.undo_len();
    h.demo_invoke(
        &layout,
        "graph-8x8-transpose-2",
        "on-change",
        vec![number(7.0)],
    );
    h.demo_invoke(
        &layout,
        "graph-8x8-vel-decay-5",
        "on-change",
        vec![number(0.5)],
    );
    h.demo_invoke(&layout, "graph-8x8-resolution-3", "on-change", vec![s("8")]);
    h.demo_invoke(
        &layout,
        "graph-8x8-route-4",
        "on-change",
        vec![s("Track 3")],
    );
    assert_eq!(h.app.history.undo_len(), entries + 4);
    let graph = h.demo_overrides(id);
    let param = |instance: usize, name: &str| {
        (graph.node_params.iter())
            .find(|p| p.instance == instance && p.param == name)
            .map(|p| p.value)
    };
    assert_eq!(param(2, "transpose"), Some(7.0));
    assert_eq!(param(5, "vel-decay"), Some(0.5));
    let node = |instance: usize| {
        (graph.node_intrinsics.iter())
            .find(|n| n.instance == instance)
            .cloned()
            .unwrap_or_else(|| panic!("node {instance}"))
    };
    assert_eq!(node(3).resolution, Some(vec![Timebase::Eighth as u8]));
    assert_eq!(node(4).route, Some(ProjectGraphRouteOverride::Track(2)));

    // A cell edit sets one edge; zeroing every cell silences the net.
    h.demo_invoke(
        &layout,
        "graph-8x8-weight-matrix",
        "on-cell-change",
        vec![number(3.0), number(4.0), number(0.5)],
    );
    let graph = h.demo_overrides(id);
    assert_eq!(graph.edge_params.len(), 64);
    assert!(
        graph
            .edge_params
            .iter()
            .any(|edge| { (edge.from, edge.to, edge.value) == (3, 4, 0.5) })
    );
    for r in 0..8 {
        for c in 0..8 {
            h.demo_invoke(
                &layout,
                "graph-8x8-weight-matrix",
                "on-cell-change",
                vec![number(r as f64), number(c as f64), number(0.0)],
            );
        }
    }
    assert!(
        !h.demo_propagates("neural-8x8-demo"),
        "a zero matrix is silent"
    );
}

/// A load keeps the overrides a project saved and the panel shows them;
/// it shows each scene's overrides as the scene plays.
#[test]
fn the_8x8_demo_shows_saved_overrides_and_follows_the_scene() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    let id = sequencer::lisp_host::graph_instance_id("neural-8x8-demo", None);
    let intrinsic = |instance: usize| ProjectGraphNodeIntrinsicOverride::empty("nrn", instance);
    let saved = ProjectGraphOverrides {
        sequencer_id: id,
        sequencer_name: "neural-8x8-demo".to_string(),
        owner_rack: None,
        node_intrinsics: vec![
            ProjectGraphNodeIntrinsicOverride {
                seed_from: Some(ProjectGraphSeedFrom::Tracks(vec![0])),
                ..intrinsic(0)
            },
            ProjectGraphNodeIntrinsicOverride {
                delay_steps: Some(6),
                ..intrinsic(3)
            },
            ProjectGraphNodeIntrinsicOverride {
                route: Some(ProjectGraphRouteOverride::Track(0)),
                ..intrinsic(4)
            },
        ],
        node_params: vec![ProjectGraphNodeParamOverride {
            group: "nrn".to_string(),
            instance: 2,
            param: "transpose".to_string(),
            value: -12.0,
        }],
        edge_params: vec![ProjectGraphEdgeParamOverride {
            group: "nrn->nrn".to_string(),
            from: 0,
            to: 1,
            param: "weight".to_string(),
            value: 0.25,
        }],
        reset_every_beats: None,
        max_poly: None,
        max_poly_selection: None,
        node_count: None,
        group_gain: None,
        group_coupling: None,
        group_trace_decay: None,
        group_coupling_scale: None,
        group_excite_floor: None,
    };
    h.shared
        .state
        .edit_current_graph_overrides(|graphs| {
            *graphs = vec![saved.clone()];
            Ok(())
        })
        .unwrap();
    h.demo_load(&EIGHT);
    assert_eq!(h.shared.state.current_graph_overrides(), vec![saved]);
    let shows = |h: &mut Harness, transpose: f64, delay: f64, weight: f64| {
        let (tree, _) = h.demo_tree("*8x8*");
        let picker = widget_keyed(&tree, "graph-8x8-transpose-2").expect("transpose 2");
        assert_eq!(slot_value(&picker["value"]), transpose);
        let picker = widget_keyed(&tree, "graph-8x8-delay-3").expect("delay 3");
        assert_eq!(slot_value(&picker["value"]), delay);
        let route = widget_keyed(&tree, "graph-8x8-route-4").expect("route 4");
        assert_eq!(route["value"], s("Track 1"));
        let weights = widget_keyed(&tree, "graph-8x8-weight-matrix").expect("weights");
        assert_eq!(cell(&weights["value"], 0, 1), number(weight));
    };
    shows(&mut h, -12.0, 6.0, 0.25);

    // A second scene with its own overrides: each scene shows its own.
    h.command("clone-pattern", Value::Nil);
    h.drain();
    assert_eq!(h.app.state.current_scene_index(), 1);
    h.eval("(graph-param g8-name 2 :transpose 5)");
    h.eval("(graph-node g8-name 3 :delay 2)");
    h.eval("(graph-edge g8-name :from 0 :to 1 :weight 0.75)");
    h.pkg_render();
    shows(&mut h, 5.0, 2.0, 0.75);
    let switch = |h: &mut Harness, index: usize| {
        h.eval(&format!(
            "(host-command \"switch-pattern\" (dict :idx {index} :quantize \"off\"))"
        ));
        h.drain();
        assert_eq!(h.app.state.current_scene_index(), index);
        h.pkg_render();
    };
    switch(&mut h, 0);
    shows(&mut h, -12.0, 6.0, 0.25);
    switch(&mut h, 1);
    shows(&mut h, 5.0, 2.0, 0.75);
}

/// The reset fork's batch controls (global transpose, dur x, delay x and
/// res/q x) edit every node; its reset toggles edit their node's param.
#[test]
fn the_8x8_reset_demo_batch_controls_edit_every_node() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.demo_load(&RESET);
    let layout = h.demo_layout("*8x8-reset*", 120.0, 60.0);
    assert_eq!(
        count(&layout, "matrix"),
        4,
        "weights, triggers, energy, dampening"
    );
    assert_eq!(count(&layout, "event-view"), 1);
    assert_eq!(count(&layout, "number-picker"), 8 * 5 + 4);
    assert_eq!(count(&layout, "toggle"), 8 * 2);
    assert_eq!(count(&layout, "dropdown"), 8 * 3 + 2);
    for key in [
        "graph-8x8-reset-global-transpose",
        "graph-8x8-reset-dur-factor",
        "graph-8x8-reset-delay-factor",
        "graph-8x8-reset-timebase-factor",
        "graph-8x8-reset-weight-matrix",
        "graph-8x8-reset-event-view",
    ] {
        measured(&layout, key);
    }
    for n in 0..8 {
        for field in ["transpose-reset", "vel-reset", "resolution", "quantize"] {
            measured(&layout, &format!("graph-8x8-reset-{field}-{n}"));
        }
    }
    let manifest = (h.shared.state.published_sequencers().into_iter())
        .find_map(|published| published.graph.filter(|graph| graph.name == RESET.name))
        .expect("the manifest");
    for (param, default) in [
        ("global-transpose", 0.0),
        ("transpose-reset", 0.0),
        ("dur-factor", 1.0),
        ("vel-reset", 0.0),
    ] {
        assert_eq!(manifest.node.param_default(param), Some(default), "{param}");
    }
    // A toggle binds its node's param.
    let (tree, _) = h.demo_tree("*8x8-reset*");
    let g = Harness::demo_graph(&RESET);
    let reset = h.demo_instance(&format!(
        "(let ((g {g})) (graph-param-named (nth g.nodes 2) \"transpose-reset\"))"
    ));
    let toggle = widget_keyed(&tree, "graph-8x8-reset-transpose-reset-2").expect("toggle");
    assert!(binds(&toggle, "value", reset, "value"));

    for (key, value) in [
        ("graph-8x8-reset-global-transpose", number(12.0)),
        ("graph-8x8-reset-dur-factor", number(2.0)),
        ("graph-8x8-reset-transpose-reset-2", Value::Bool(true)),
        ("graph-8x8-reset-vel-reset-3", Value::Bool(true)),
        ("graph-8x8-reset-delay-factor", s("2")),
        ("graph-8x8-reset-timebase-factor", s("2")),
    ] {
        h.demo_invoke(&layout, key, "on-change", vec![value]);
    }
    let id = sequencer::lisp_host::graph_instance_id(RESET.name, None);
    let graph = h.demo_overrides(id);
    let every = |name: &str, value: f64| {
        (graph.node_params.iter())
            .filter(|p| p.param == name && p.value == value)
            .count()
    };
    assert_eq!(every("global-transpose", 12.0), 8, "every node's");
    assert_eq!(every("dur-factor", 2.0), 8, "every node's");
    assert!(
        graph
            .node_params
            .iter()
            .any(|p| { (p.instance, p.param.as_str(), p.value) == (2, "transpose-reset", 1.0) })
    );
    assert!(
        graph
            .node_params
            .iter()
            .any(|p| { (p.instance, p.param.as_str(), p.value) == (3, "vel-reset", 1.0) })
    );
    let thirty_second = Timebase::ThirtySecond as u8;
    assert_eq!(
        (graph.node_intrinsics.iter())
            .filter(|node| {
                node.delay_steps == Some(2)
                    && node.resolution == Some(vec![thirty_second])
                    && node.quantize
                        == Some(ProjectGraphQuantizeOverride::Timebase(vec![thirty_second]))
            })
            .count(),
        8,
        "delay x and res/q x edit every node"
    );
    // The factor pickers are one-shot: they show 1 again.
    let layout = h.demo_layout("*8x8-reset*", 120.0, 60.0);
    for key in [
        "graph-8x8-reset-delay-factor",
        "graph-8x8-reset-timebase-factor",
    ] {
        assert_eq!(
            prop(by_key(&layout, key).unwrap(), "value"),
            s("1"),
            "{key}"
        );
    }
    // res/q x moves a triplet resolution within the triplets and keeps Prh.
    h.demo_with(
        &RESET,
        "(let ((a (nth g.nodes 1)) (b (nth g.nodes 2))) (set! a.resolution \"8T\") (set! b.resolution \"Prh\"))",
    );
    h.drain();
    h.pkg_render();
    h.demo_invoke(
        &layout,
        "graph-8x8-reset-timebase-factor",
        "on-change",
        vec![s("1/2")],
    );
    h.pkg_render();
    assert_eq!(
        h.demo_with(
            &RESET,
            "(map (lambda (i) (let ((n (nth g.nodes i))) n.resolution)) (range 0 3))"
        ),
        strings(&["16", "4T", "Prh"])
    );
}

/// eseq-0l17.53: each batch control of the reset demo (global transpose,
/// dur x, delay x, res/q x) writes every node through the legacy natives as
/// one undo entry, and undo restores every node.
#[test]
fn the_8x8_reset_demo_batch_edits_are_one_undo_entry_each() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.demo_load(&RESET);
    let layout = h.demo_layout("*8x8-reset*", 120.0, 60.0);
    let id = sequencer::lisp_host::graph_instance_id(RESET.name, None);
    // The overrides as sets (undo may reorder their entries; none yet).
    let fields = |h: &Harness| {
        let graph = (h.shared.state.current_graph_overrides().into_iter())
            .find(|graph| graph.sequencer_id == id)
            .unwrap_or_default();
        let mut params: Vec<String> = (graph.node_params.iter())
            .map(|p| format!("{} {} {}", p.instance, p.param, p.value))
            .collect();
        let mut nodes: Vec<String> = (graph.node_intrinsics.iter())
            .map(|n| {
                format!(
                    "{} {:?} {:?} {:?}",
                    n.instance, n.delay_steps, n.resolution, n.quantize
                )
            })
            .collect();
        params.sort();
        nodes.sort();
        (params, nodes)
    };
    for (key, value) in [
        ("graph-8x8-reset-global-transpose", number(12.0)),
        ("graph-8x8-reset-dur-factor", number(2.0)),
        ("graph-8x8-reset-delay-factor", s("2")),
        ("graph-8x8-reset-timebase-factor", s("2")),
    ] {
        let before = fields(&h);
        let entries = h.app.history.undo_len();
        h.demo_invoke(&layout, key, "on-change", vec![value]);
        assert_eq!(h.app.history.undo_len(), entries + 1, "{key}: one entry");
        let edited = fields(&h);
        assert_ne!(edited, before, "{key} edits");
        app::edit::undo(&mut h.app);
        assert_eq!(fields(&h), before, "{key}: undo restores every node");
        app::edit::redo(&mut h.app);
        assert_eq!(fields(&h), edited, "{key}: redo");
    }
}

/// The variable-count graph's panel lays out its active nodes (8, grown to
/// 16, shrunk to 12, grown back with the dormant nodes' overrides intact),
/// lights the row of the weight column pressed, colors each row by its
/// route and edits seeds, the threshold (every node up to the capacity) and
/// the max-poly selection.
#[test]
fn the_variable_reset_demo_follows_its_node_count() {
    fn active_layout(layout: &LayoutNode, count: usize) {
        for key in [
            "graph-variable-reset-node-count",
            "graph-variable-reset-max-poly-selection",
            "graph-variable-reset-threshold",
            "graph-variable-reset-route-color-0",
            "graph-variable-reset-seed-route-0",
            "graph-variable-reset-reset-seed-0",
            "graph-variable-reset-trigger-matrix",
            "graph-variable-reset-energy-matrix",
            "graph-variable-reset-weight-matrix",
            "graph-variable-reset-dampening-matrix",
            "graph-variable-reset-row-0",
        ] {
            measured(layout, key);
        }
        let weight = measured(layout, "graph-variable-reset-weight-matrix");
        assert_eq!(prop(weight, "rows"), number(count as f64));
        assert_eq!(prop(weight, "cols"), number(count as f64));
        assert!(weight.props.contains_key("on-cell-press"));
        assert!(weight.props.contains_key("on-cell-release"));
        let trigger = measured(layout, "graph-variable-reset-trigger-matrix");
        assert_eq!(prop(trigger, "rows"), number(count as f64));
        assert_eq!(prop(trigger, "cols"), number(1.0));
        let height = count as f64 + count.saturating_sub(1) as f64 * 0.2;
        for key in ["trigger-matrix", "energy-matrix", "weight-matrix"] {
            let matrix = measured(layout, &format!("graph-variable-reset-{key}"));
            let Value::Number(h) = prop(matrix, "height") else {
                panic!("{key} height")
            };
            assert!((h - height).abs() < 1e-9, "{key}: {h} vs {height}");
        }
        let last = count - 1;
        for field in ["transpose", "route-color", "seed-route", "reset-seed"] {
            measured(layout, &format!("graph-variable-reset-{field}-{last}"));
            assert!(
                by_key(layout, &format!("graph-variable-reset-{field}-{count}")).is_none(),
                "no row {count}"
            );
        }
    }
    let mut h = Harness::new();
    h.pkg_tracks(16);
    h.demo_load(&VARIABLE);
    let manifest = (h.shared.state.published_sequencers().into_iter())
        .find_map(|published| published.graph.filter(|graph| graph.name == VARIABLE.name))
        .expect("the manifest");
    assert_eq!(manifest.shape.num_nodes(), 8);
    assert_eq!(manifest.shape.capacity_num_nodes(), 16);
    let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
    active_layout(&layout, 8);

    // Pressing a weight cell lights its destination's row until released,
    // re-running only the rows' highlight boxes (their controls reused).
    show_on_screen(&mut h, "*variable-reset*");
    let press = reruns(&mut h, |h| {
        h.demo_invoke(
            &layout,
            "graph-variable-reset-weight-matrix",
            "on-cell-press",
            vec![number(2.0), number(3.0)],
        )
    });
    assert_eq!(press, (0, 8), "the eight rows' highlight boxes");
    let lit = |h: &mut Harness, row: usize| {
        let (tree, _) = h.demo_tree("*variable-reset*");
        let row = widget_keyed(&tree, &format!("graph-variable-reset-row-{row}")).expect("row");
        assert_eq!(
            row["selected-background-color"],
            Value::Keyword("mixer-strip-selected-bg".into())
        );
        row["selected"].clone()
    };
    assert_eq!(lit(&mut h, 3), Value::Bool(true));
    assert_eq!(lit(&mut h, 2), Value::Bool(false));
    assert_eq!(h.eval("gvr-view.selected-neuron"), number(3.0));
    h.demo_invoke(
        &layout,
        "graph-variable-reset-weight-matrix",
        "on-cell-release",
        vec![number(2.0), number(3.0)],
    );
    assert_eq!(lit(&mut h, 3), Value::Bool(false));
    assert_eq!(h.eval("gvr-view.selected-neuron"), number(-1.0));

    // A row's strip shows its route's color, the off grey without one.
    let strip = |h: &mut Harness, n: usize| {
        let (tree, _) = h.demo_tree("*variable-reset*");
        let strip =
            widget_keyed(&tree, &format!("graph-variable-reset-route-color-{n}")).expect("strip");
        list_value(["active", "track-r", "track-g", "track-b"].map(|p| strip[p].clone()))
    };
    let color = |h: &mut Harness, track: usize| {
        h.demo_eval(&format!(
            "(let ((c (track {track}))) (list 1 (rgb-part c.color 0) (rgb-part c.color 1) (rgb-part c.color 2)))"
        ))
    };
    let track_0 = color(&mut h, 0);
    assert_eq!(strip(&mut h, 0), track_0);
    h.demo_invoke(
        &layout,
        "graph-variable-reset-route-4",
        "on-change",
        vec![s("Track 3")],
    );
    let track_2 = color(&mut h, 2);
    assert_eq!(strip(&mut h, 4), track_2);
    h.demo_invoke(
        &layout,
        "graph-variable-reset-route-4",
        "on-change",
        vec![s("Off")],
    );
    assert_eq!(
        strip(&mut h, 4),
        list_value([number(0.0), number(0.20), number(0.21), number(0.23)])
    );

    // Seeds, the threshold and the max-poly selection.
    let node = |h: &mut Harness, n: usize, field: &str| {
        h.demo_with(
            &VARIABLE,
            &format!("(let ((n (nth g.nodes {n}))) n.{field})"),
        )
    };
    assert_eq!(node(&mut h, 0, "seed-route"), Value::Bool(false));
    assert_eq!(node(&mut h, 0, "seed-on-reset"), number(0.0));
    h.demo_invoke(
        &layout,
        "graph-variable-reset-seed-route-1",
        "on-change",
        vec![Value::Bool(true)],
    );
    assert_eq!(node(&mut h, 1, "seed-route"), Value::Bool(true));
    h.demo_invoke(
        &layout,
        "graph-variable-reset-seed-route-1",
        "on-change",
        vec![Value::Bool(false)],
    );
    assert_eq!(node(&mut h, 1, "seed-route"), Value::Bool(false));
    h.demo_invoke(
        &layout,
        "graph-variable-reset-reset-seed-7",
        "on-change",
        vec![Value::Bool(true)],
    );
    assert_eq!(node(&mut h, 7, "seed-on-reset"), number(1.0));
    h.demo_invoke(
        &layout,
        "graph-variable-reset-threshold",
        "on-change",
        vec![number(0.8)],
    );
    h.demo_invoke(
        &layout,
        "graph-variable-reset-max-poly-selection",
        "on-change",
        vec![s("random")],
    );
    assert_eq!(h.demo_with(&VARIABLE, "g.max-poly-selection"), s("random"));

    // Grown to the capacity: every node carries the threshold.
    h.demo_invoke(
        &layout,
        "graph-variable-reset-node-count",
        "on-change",
        vec![number(16.0)],
    );
    let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
    active_layout(&layout, 16);
    let param = |h: &mut Harness, n: usize, name: &str| {
        h.demo_with(
            &VARIABLE,
            &format!("(let ((p (graph-param-named (nth g.nodes {n}) \"{name}\"))) p.value)"),
        )
    };
    assert_eq!(param(&mut h, 14, "threshold"), number(0.8));
    h.eval("(graph-param gvr-name 14 :transpose 7)");
    h.eval("(graph-edge gvr-name :from 14 :to 3 :weight 0.5)");

    // Shrunk, the dormant nodes keep their overrides.
    h.demo_invoke(
        &layout,
        "graph-variable-reset-node-count",
        "on-change",
        vec![number(12.0)],
    );
    let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
    active_layout(&layout, 12);
    let id = sequencer::lisp_host::graph_instance_id(VARIABLE.name, None);
    let graph = h.demo_overrides(id);
    assert_eq!(graph.node_count, Some(12));
    assert_eq!(
        graph.max_poly_selection,
        Some(NeuralMaxPolySelection::Random)
    );
    assert!(
        graph
            .node_params
            .iter()
            .any(|p| { (p.instance, p.param.as_str(), p.value) == (14, "threshold", 0.8) })
    );
    assert!(
        graph
            .node_params
            .iter()
            .any(|p| { (p.instance, p.param.as_str(), p.value) == (14, "transpose", 7.0) })
    );
    assert!(
        graph
            .edge_params
            .iter()
            .any(|e| (e.from, e.to, e.value) == (14, 3, 0.5))
    );
    assert!(
        graph
            .node_intrinsics
            .iter()
            .any(|node| { node.instance == 7 && node.seed_on_reset == Some(1.0) })
    );
    let shrunk = manifest.runtime_config_with_overrides(Some(&graph));
    assert_eq!(shrunk.nodes.len(), 12);
    assert!(shrunk.nodes[7].trigger_on_reset);
    assert!(shrunk.edges.iter().all(|e| e.from < 12 && e.to < 12));

    h.demo_invoke(
        &layout,
        "graph-variable-reset-node-count",
        "on-change",
        vec![number(16.0)],
    );
    let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
    active_layout(&layout, 16);
    assert_eq!(param(&mut h, 14, "transpose"), number(7.0));
    let weights = prop(
        by_key(&layout, "graph-variable-reset-weight-matrix").unwrap(),
        "value",
    );
    assert_eq!(cell(&weights, 14, 3), number(0.5));
}

/// The group matrices show the engine's inert defaults and set one cell
/// per edit.
#[test]
fn the_group_matrix_demo_edits_group_cells() {
    let mut h = Harness::new();
    h.pkg_tracks(4);
    h.demo_load(&GROUPS);
    let layout = h.demo_layout("*group-matrix*", 160.0, 70.0);
    for key in [
        "graph-group-matrix-weight-matrix",
        "graph-group-matrix-group-gain-matrix",
        "graph-group-matrix-group-coupling-matrix",
        "graph-group-matrix-group-activity-matrix",
        "graph-group-matrix-group-suppression-matrix",
        "graph-group-matrix-trace-decay",
        "graph-group-matrix-coupling-scale",
        "graph-group-matrix-excite-floor",
    ] {
        measured(&layout, key);
    }
    let gain = prop(
        by_key(&layout, "graph-group-matrix-group-gain-matrix").unwrap(),
        "value",
    );
    let coupling = prop(
        by_key(&layout, "graph-group-matrix-group-coupling-matrix").unwrap(),
        "value",
    );
    assert_eq!(cell(&gain, 0, 1), number(1.0), "G: inert at 1");
    assert_eq!(cell(&coupling, 0, 1), number(0.0), "H: inert at 0");
    let (tree, _) = h.demo_tree("*group-matrix*");
    let graph = h.demo_instance(&Harness::demo_graph(&GROUPS));
    let decay = widget_keyed(&tree, "graph-group-matrix-trace-decay").expect("trace decay");
    assert!(binds(&decay, "value", graph, "group-trace-decay"));

    h.demo_invoke(
        &layout,
        "graph-group-matrix-group-gain-matrix",
        "on-cell-change",
        vec![number(0.0), number(1.0), number(0.25)],
    );
    h.demo_invoke(
        &layout,
        "graph-group-matrix-group-coupling-matrix",
        "on-cell-change",
        vec![number(1.0), number(0.0), number(-1.5)],
    );
    let id = sequencer::lisp_host::graph_instance_id(GROUPS.name, None);
    let overrides = h.demo_overrides(id);
    let k = sequencer::graph::NEURAL_GROUP_MAX as usize;
    assert_eq!(overrides.group_gain.as_ref().expect("gain")[1], 0.25);
    assert_eq!(
        overrides.group_coupling.as_ref().expect("coupling")[k],
        -1.5
    );
    let layout = h.demo_layout("*group-matrix*", 160.0, 70.0);
    let gain = prop(
        by_key(&layout, "graph-group-matrix-group-gain-matrix").unwrap(),
        "value",
    );
    let coupling = prop(
        by_key(&layout, "graph-group-matrix-group-coupling-matrix").unwrap(),
        "value",
    );
    assert_eq!(cell(&gain, 0, 1), number(0.25));
    assert_eq!(cell(&coupling, 1, 0), number(-1.5));
}

/// The Markov panel and its explicit init (a weighted-choice matrix, node
/// delays, a seed).
#[test]
fn the_markov_demo_loads_its_matrix_and_node_delays() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.demo_load(&MARKOV);
    let manifest = (h.shared.state.published_sequencers().into_iter())
        .find_map(|published| published.graph.filter(|graph| graph.name == MARKOV.name))
        .expect("the manifest");
    assert_eq!(manifest.shape.num_nodes(), 8);
    assert_eq!(
        manifest.edge_sets[0].distribution,
        sequencer::graph::EdgeDistribution::WeightedChoice
    );
    let layout = h.demo_layout("*markov-8x8*", 70.0, 70.0);
    assert_eq!(count(&layout, "matrix"), 3, "triggers, energy, weights");
    for key in [
        "markov-8x8-trigger-matrix",
        "markov-8x8-energy-matrix",
        "markov-8x8-weight-matrix",
    ] {
        measured(&layout, key);
    }
    assert_eq!(count(&layout, "number-picker"), 8 * 3 + 1);
    assert_eq!(count(&layout, "dropdown"), 8 * 3);

    h.eval("(m8-init-defaults)");
    h.pkg_render();
    let graph = h.demo_overrides(sequencer::lisp_host::graph_instance_id(MARKOV.name, None));
    assert_eq!(graph.edge_params.len(), 64, "the weight matrix");
    assert!(graph.edge_params.iter().any(|edge| {
        (edge.from, edge.to, edge.param.as_str(), edge.value) == (0, 1, "weight", 0.65)
    }));
    assert!(graph.node_intrinsics.iter().any(|node| {
        node.instance == 0 && node.seed_from == Some(ProjectGraphSeedFrom::Tracks(vec![0]))
    }));
    assert!(
        graph
            .node_intrinsics
            .iter()
            .any(|node| node.instance == 4 && node.delay_steps == Some(3))
    );
    let layout = h.demo_layout("*markov-8x8*", 70.0, 70.0);
    let weights = prop(
        by_key(&layout, "markov-8x8-weight-matrix").unwrap(),
        "value",
    );
    assert_eq!(cell(&weights, 0, 1), number(0.65));
    let delay = by_key(&layout, "markov-8x8-delay-4").expect("delay 4");
    assert_eq!(slot_value(&prop(delay, "value")), 3.0);
}

/// The 16-node panel: every node's controls, the timing controls on every
/// node, the reset params on theirs, and the explicit init's ring.
#[test]
fn the_16_demo_edits_every_node_and_writes_its_ring() {
    let mut h = Harness::new();
    h.pkg_tracks(16);
    h.demo_load(&SIXTEEN);
    let layout = h.demo_layout("*16x16*", 90.0, 70.0);
    assert_eq!(
        count(&layout, "matrix"),
        4,
        "weights, triggers, energy, dampening"
    );
    assert_eq!(count(&layout, "event-view"), 1);
    for key in [
        "graph-16-trigger-matrix",
        "graph-16-energy-matrix",
        "graph-16-weight-matrix",
        "graph-16-dampening-matrix",
        "graph-16-event-view",
    ] {
        measured(&layout, key);
    }
    for (key, name, value) in [
        ("graph-16-trigger-matrix", "height", 24.0),
        ("graph-16-energy-matrix", "height", 24.0),
        ("graph-16-weight-matrix", "width", 52.0),
        ("graph-16-weight-matrix", "height", 24.0),
    ] {
        assert_eq!(
            prop(by_key(&layout, key).unwrap(), name),
            number(value),
            "{key} {name}"
        );
    }
    assert_eq!(count(&layout, "number-picker"), 16 * 8 + 4);
    assert_eq!(count(&layout, "dropdown"), 16 * 3);
    for n in 0..16 {
        for field in [
            "route",
            "delay",
            "transpose",
            "transpose-reset",
            "vel-decay",
            "vel-reset",
            "state-reset",
            "dampening",
            "recovery",
            "resolution",
            "quantize",
        ] {
            measured(&layout, &format!("graph-16-{field}-{n}"));
        }
    }
    for (key, value) in [
        ("graph-16-transpose-reset-5", 1.0),
        ("graph-16-vel-reset-6", 1.0),
        ("graph-16-state-reset-7", 1.0),
        ("graph-16-dur-factor", 2.0),
        ("graph-16-swing", 64.0),
    ] {
        h.demo_invoke(&layout, key, "on-change", vec![number(value)]);
    }
    let id = sequencer::lisp_host::graph_instance_id(SIXTEEN.name, None);
    let graph = h.demo_overrides(id);
    let has = |instance: usize, name: &str, value: f64| {
        graph
            .node_params
            .iter()
            .any(|p| (p.instance, p.param.as_str(), p.value) == (instance, name, value))
    };
    assert!(has(5, "transpose-reset", 1.0));
    assert!(has(6, "vel-reset", 1.0));
    assert!(has(7, "state-reset", 1.0));
    assert!(
        (0..16).all(|n| has(n, "dur-factor", 2.0)),
        "every node's dur x"
    );
    assert!((0..16).all(|n| has(n, "swing", 64.0)), "every node's swing");
    let layout = h.demo_layout("*16x16*", 90.0, 70.0);
    let swing = by_key(&layout, "graph-16-swing").expect("swing");
    assert_eq!(slot_value(&prop(swing, "value")), 64.0, "shows node 0's");

    h.eval("(g16-init-ring-defaults)");
    let graph = h.demo_overrides(id);
    assert_eq!(graph.edge_params.len(), 16 * 16);
    for (from, to) in [(0, 1), (15, 0)] {
        assert!(graph.edge_params.iter().any(|edge| {
            (edge.from, edge.to, edge.param.as_str(), edge.value) == (from, to, "weight", 1.0)
        }));
    }
    assert!(graph.node_intrinsics.iter().any(|node| {
        node.instance == 0 && node.seed_from == Some(ProjectGraphSeedFrom::Tracks(vec![0]))
    }));
}

/// The cycle fields round-trip a node's resolution and quantize cycles: a
/// field keeps the text typed into it (junk tokens and spaces included)
/// while its scene plays and sets the node's cycle to the labels it names.
#[test]
fn the_16_cycle_demo_round_trips_resolution_and_quantize_cycles() {
    let mut h = Harness::new();
    h.pkg_tracks(16);
    h.demo_load(&CYCLE);
    let layout = h.demo_layout("*16x16-cycle*", 120.0, 70.0);
    measured(&layout, "graph-16c-event-view");
    assert_eq!(count(&layout, "event-view"), 1);
    assert_eq!(count(&layout, "text-input"), 16 * 2);
    for n in 0..16 {
        measured(&layout, &format!("graph-16c-resolution-{n}"));
        measured(&layout, &format!("graph-16c-quantize-{n}"));
    }
    assert!(h.shared.state.current_graph_overrides().is_empty());

    h.eval("(script-init-fn)");
    h.pkg_render();
    let cycle = |h: &mut Harness, n: usize, field: &str| {
        h.demo_with(
            &CYCLE,
            &format!("(let ((n (nth g.nodes {n}))) n.{field}-cycle)"),
        )
    };
    assert_eq!(
        cycle(&mut h, 0, "resolution"),
        strings(&["16", "16", "16", "16", "16", "4"])
    );
    assert_eq!(cycle(&mut h, 1, "resolution"), strings(&["16", "8", "16"]));
    let id = sequencer::lisp_host::graph_instance_id(CYCLE.name, None);
    let node0 = (h.demo_overrides(id).node_intrinsics.into_iter())
        .find(|node| node.instance == 0)
        .expect("node 0");
    assert_eq!(
        node0.resolution,
        Some(vec![4, 4, 4, 4, 4, 2]),
        "timebase indices"
    );
    let field = |h: &mut Harness, key: &str| {
        let (tree, _) = h.demo_tree("*16x16-cycle*");
        widget_keyed(&tree, key).expect(key)["value"].clone()
    };
    assert_eq!(
        field(&mut h, "graph-16c-resolution-0"),
        s("16 16 16 16 16 4")
    );

    // Typing: the field keeps the text, the node takes its labels.
    let layout = h.demo_layout("*16x16-cycle*", 120.0, 70.0);
    h.demo_invoke(
        &layout,
        "graph-16c-resolution-2",
        "on-change",
        vec![s("16  4 garbage 8")],
    );
    assert_eq!(cycle(&mut h, 2, "resolution"), strings(&["16", "4", "8"]));
    assert_eq!(
        field(&mut h, "graph-16c-resolution-2"),
        s("16  4 garbage 8")
    );
    h.demo_invoke(
        &layout,
        "graph-16c-quantize-4",
        "on-change",
        vec![s("16 8 16")],
    );
    assert_eq!(cycle(&mut h, 4, "quantize"), strings(&["16", "8", "16"]));
    h.demo_invoke(&layout, "graph-16c-quantize-4", "on-change", vec![s("off")]);
    assert_eq!(cycle(&mut h, 4, "quantize"), strings(&["off"]));
    // Any case, and the graph-* natives' words.
    h.demo_invoke(
        &layout,
        "graph-16c-quantize-4",
        "on-change",
        vec![s("16t sixteenth PRH quarter-triplet")],
    );
    assert_eq!(
        cycle(&mut h, 4, "quantize"),
        strings(&["16T", "16", "Prh", "4T"])
    );
    // A resolution naming no label keeps its text and the node's cycle.
    h.demo_invoke(&layout, "graph-16c-resolution-2", "on-change", vec![s("P")]);
    assert_eq!(cycle(&mut h, 2, "resolution"), strings(&["16", "4", "8"]));
    assert_eq!(field(&mut h, "graph-16c-resolution-2"), s("P"));

    // An undo shows the cycle it restores, not the text typed.
    h.demo_invoke(
        &layout,
        "graph-16c-resolution-3",
        "on-change",
        vec![s("8  4")],
    );
    assert_eq!(field(&mut h, "graph-16c-resolution-3"), s("8  4"));
    h.undo();
    h.pkg_render();
    assert_eq!(cycle(&mut h, 3, "resolution"), strings(&["16"]));
    assert_eq!(field(&mut h, "graph-16c-resolution-3"), s("16"));

    // A keystroke re-runs the cycle fields, never the rows.
    show_on_screen(&mut h, "*16x16-cycle*");
    let typed = reruns(&mut h, |h| {
        h.demo_invoke(
            &layout,
            "graph-16c-resolution-5",
            "on-change",
            vec![s("8 ")],
        )
    });
    assert_eq!(
        typed,
        (0, 33),
        "the 32 cycle fields (the typed text), then the edited one (its cycle)"
    );

    // Another scene shows its own cycles.
    h.command("clone-pattern", Value::Nil);
    h.drain();
    h.eval("(graph-node g16c-name 2 :resolution \"8 8\")");
    h.pkg_render();
    assert_eq!(field(&mut h, "graph-16c-resolution-2"), s("8 8"));
}

/// Playback repaints only the panel's playback views: the firing history
/// and the trigger, energy and dampening matrices re-run their subtrees,
/// the beat is bound, the controls stay; the tracks' notes re-run only the
/// keyboard. The views follow the node count.
#[test]
fn graph_playback_reruns_only_the_playback_views() {
    let mut h = Harness::new();
    h.pkg_tracks(8);
    h.demo_load(&VARIABLE);
    show_on_screen(&mut h, "*variable-reset*");
    let id = sequencer::lisp_host::graph_instance_id(VARIABLE.name, None);
    let state = h.shared.state.clone();
    for count in [8, 3] {
        if count != 8 {
            h.demo_with(&VARIABLE, &format!("(set! g.node-count {count})"));
            h.drain();
            h.pkg_render();
        }
        for beat in [1.0, 2.0] {
            let event = GraphVisualizationEvent {
                node_index: 1,
                track: Some(0),
                sample_time: (beat * 24_000.0) as u64,
                beat,
                transpose: 7.0,
                velocity: 0.75,
            };
            let mut energy = vec![0.0; count];
            energy[1] = beat;
            let mut triggers = vec![0.0; count];
            triggers[1] = (beat * 0.25) as f32;
            state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
                id,
                name: VARIABLE.name.to_string(),
                active: true,
                current_beat: beat,
                num_nodes: count,
                energy,
                trigger_activity: triggers,
                event_history: vec![event],
                history_stamp: beat as u64 + count as u64 * 10,
                edges: vec![GraphVisualizationEdge {
                    from: 0,
                    to: 1,
                    weight: 1.0,
                    dampening: beat * 0.25,
                    delay_steps: 1,
                    distribution: Default::default(),
                }],
                ..Default::default()
            }]);
            let before = h.rt().ui_work_counters();
            h.pkg_render();
            let after = h.rt().ui_work_counters();
            assert_eq!(
                after.full_buffer_reruns, before.full_buffer_reruns,
                "playback never re-runs the panel"
            );
            assert_eq!(
                after.subtree_reruns - before.subtree_reruns,
                4,
                "the history and the three playback matrices"
            );
            let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
            let events = measured(&layout, "graph-variable-reset-event-view");
            assert_eq!(slot_value(&prop(events, "current-beat")), beat);
            assert_eq!(prop(events, "y-max"), number((count - 1) as f64));
            assert_eq!(
                prop(events, "events"),
                list_value([list_value([1.0, 0.0, beat, 7.0, 0.75].map(number))])
            );
            for (key, field) in [
                ("trigger-matrix", "(map (lambda (x) (list x)) g.triggers)"),
                ("energy-matrix", "(map (lambda (x) (list x)) g.energy)"),
                ("dampening-matrix", "g.dampening"),
            ] {
                let matrix = measured(&layout, &format!("graph-variable-reset-{key}"));
                assert_eq!(prop(matrix, "rows"), number(count as f64));
                assert_eq!(
                    prop(matrix, "value"),
                    h.demo_with(&VARIABLE, field),
                    "{key}"
                );
            }
            assert_eq!(
                cell(
                    &prop(
                        by_key(&layout, "graph-variable-reset-energy-matrix").unwrap(),
                        "value"
                    ),
                    1,
                    0
                ),
                number(beat)
            );

            state.replace_live_notes(0, [(60 + beat as u8, 0.75)]);
            let before = h.rt().ui_work_counters();
            h.pkg_render();
            let after = h.rt().ui_work_counters();
            assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns);
            assert_eq!(
                after.subtree_reruns - before.subtree_reruns,
                1,
                "only the keyboard"
            );
            let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
            let notes = prop(
                measured(&layout, "graph-variable-reset-piano"),
                "notes-by-track",
            );
            assert_eq!(
                items(&items(&notes)[0])
                    .iter()
                    .map(|note| items(note)[0].clone())
                    .collect::<Vec<_>>(),
                vec![number(60.0 + beat)]
            );
        }
    }
    // Stopped and cleared: no history, no notes.
    state.set_graph_visualizations(Vec::new());
    state.replace_live_notes(0, []);
    h.pkg_render();
    let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
    assert_eq!(
        prop(
            measured(&layout, "graph-variable-reset-event-view"),
            "events"
        ),
        list_value([])
    );
    let notes = prop(
        measured(&layout, "graph-variable-reset-piano"),
        "notes-by-track",
    );
    assert!(items(&notes).iter().all(|track| items(track).is_empty()));
}

// ── rack-owned restore ───────────────────────────────────────────────────

const RACK_SOURCE: &str = "(load \"@/scripts/sequencers/graph-neural-variable-reset-demo.lisp\")";

/// The factory DAW with a drum rack over `tracks` that owns the variable
/// graph demo (as a saved project lists it); returns the rack's group id.
fn rack_graph_project(tracks: usize, members: &[usize]) -> (Harness, u64) {
    let mut h = distro();
    h.pkg_tracks(tracks);
    let group = demo_rack(&mut h, members);
    let rack = (h.app.groups.iter_mut())
        .find(|g| g.id == group)
        .and_then(|g| g.rack.as_mut())
        .expect("the rack");
    rack.sequencers
        .push(sequencer::project::ProjectRackSequencer {
            sequencer_id: sequencer::lisp_host::graph_instance_id(VARIABLE.name, Some(group)),
            sequencer_name: VARIABLE.name.to_string(),
            source: RACK_SOURCE.to_string(),
        });
    h.app.state.set_rack_memberships(h.app.rack_memberships());
    h.share_buses_and_groups();
    h.pkg_render();
    (h, group)
}

/// The tabs of the tile showing `buffer`, as (label, buffer).
fn tile_tabs(editor: &Editor, buffer: &str) -> Vec<(String, String)> {
    let index = (editor.buffers.iter())
        .position(|item| item.name == buffer)
        .unwrap_or_else(|| panic!("{buffer}"));
    let leaf = (editor.tile_root.find_leaf_by_buffer_idx(index))
        .unwrap_or_else(|| panic!("a tile shows {buffer}"));
    leaf.tabs
        .iter()
        .map(|tab| {
            (
                tab.label.clone(),
                editor.buffers[tab.buffer_idx].name.clone(),
            )
        })
        .collect()
}

/// Reopening a project (an empty scratch, unrelated scratch source, empty
/// again) replays its rack's graph script into one usable tab each time.
#[test]
fn empty_scratch_restores_the_rack_graph_tab_and_controls() {
    let (mut h, group) = rack_graph_project(2, &[0]);
    assert!(h.app.state.scratch_source().is_empty());
    for scratch in ["", "(def restored-project-marker 1)", ""] {
        crate::state_values::clear_project_script_tabs(&mut h.editor).unwrap();
        h.app.state.set_scratch_source(scratch);
        crate::state_values::evaluate_project_scratch_on_ui_runtime(&mut h.editor, &h.app)
            .expect("restore the rack's scripts");
        h.drain();
        h.pkg_render();
        let published = h.app.state.published_sequencers();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].graph.as_ref().unwrap().owner_rack, Some(group));
        let tabs = tile_tabs(&h.editor, "*sequencer*");
        assert_eq!(tabs.len(), 2, "the step tab and the restored graph tab");
        assert_eq!(tabs[1].1, "*variable-reset*");
        // Clicking the tab shows the panel in the sequencer's window.
        assert!(h.editor.switch_active_tile_to_buffer_named("*sequencer*"));
        h.eval("(eseq.seq-step-tabs/seq-select-main-step-tab-by-index 2)");
        h.editor.runtime_mut().run_reactive_cycle();
        h.editor.refresh_runtime_side_effects();
        assert_eq!(h.editor.active_buffer().name, "*variable-reset*");
        let layout = h.demo_layout("*variable-reset*", 140.0, 70.0);
        measured(&layout, "graph-variable-reset-weight-matrix");
    }
}

/// A rack-owned graph's rows stay whole while the rack's members change
/// under it (members leaving, a track joining), on a legacy rack and a clip
/// rack.
#[test]
fn rack_member_churn_keeps_the_graph_rows() {
    for clips in [false, true] {
        let (mut h, group) = rack_graph_project(6, &[0, 1, 2, 3]);
        crate::state_values::evaluate_project_scratch_on_ui_runtime(&mut h.editor, &h.app)
            .expect("load the rack's script");
        h.drain();
        h.pkg_render();
        if clips {
            h.app
                .convert_rack_to_clips_recorded(group)
                .expect("convert to clips");
        }
        // Author the graph as the panel does: 16 active nodes, each routed
        // to a member.
        let name = VARIABLE.name;
        sequencer::lisp_host::with_graph_owner_rack(Some(group), || {
            h.eval(&format!("(graph-config \"{name}\" :node-count 16)"));
            for node in 0..16 {
                h.eval(&format!(
                    "(graph-node \"{name}\" {node} :route {})",
                    node % 4
                ));
            }
        });
        let whole = |h: &mut Harness, label: &str| {
            h.share_buses_and_groups();
            h.pkg_render();
            assert_eq!(
                h.demo_with(&VARIABLE, "(len g.nodes)"),
                number(16.0),
                "{label}"
            );
            assert_eq!(
                h.demo_with(&VARIABLE, "(map (lambda (n) n.seed-route) g.nodes)"),
                list_value((0..16).map(|_| Value::Bool(false))),
                "{label}"
            );
            let layout = h.demo_layout("*variable-reset*", 140.0, 90.0);
            measured(&layout, "graph-variable-reset-seed-route-15");
        };
        whole(&mut h, "before");
        h.app
            .remove_track_from_group_recorded(1)
            .expect("member 1 leaves");
        h.app
            .remove_track_from_group_recorded(2)
            .expect("member 2 leaves");
        whole(&mut h, "after two members left");
        h.app
            .attach_track_to_group(4, group, None)
            .expect("a track joins");
        h.app.publish_rack_choke_runtime();
        whole(&mut h, "after a track joined");
    }
}
