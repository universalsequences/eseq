//! alez.neural's `neural` panel ported to the kinds (kind-bindings spec §13
//! stage 8, eseq-0l17.67): the instance's graph, its nodes, params and
//! processes (§14.2k, §14.2m, §14.2n, §14.2s), its expanded node editor (the
//! sequencer's node bay over `n.processes`), the *processes* dock and the
//! expr cards. Moved here from the bare-runtime tests (the package's graph
//! visualization, node notes, processes dock and expr card tests).

use super::packages_view::binds;
use super::views::{assert_ported, distro, widget_keyed};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use eseqlisp::layout::LayoutNode;
use sequencer::graph::{
    GraphSoundingNote, GraphVisualizationEdge, GraphVisualizationEvent, GraphVisualizationSnapshot,
};

const VARIABLE_RESET: &str =
    include_str!("../../../../../../content/packages/alez.neural/src/variable-reset.lisp");

#[test]
fn the_neural_panel_uses_no_legacy_binding_forms() {
    assert_ported(&[(
        "packages/alez.neural/src/variable-reset.lisp",
        VARIABLE_RESET,
    )]);
    let code = VARIABLE_RESET
        .lines()
        .filter(|line| !line.trim_start().starts_with(';'))
        .collect::<Vec<_>>()
        .join("\n");
    for native in [
        "graph-node-value",
        "graph-param-value",
        "graph-edge-value",
        "graph-node-process-chain",
        "graph-node-lane-patch",
        "graph-node-process-classes",
        "graph-route-tracks",
        "lane-patch-run-error",
        "process-scope-cells-for",
    ] {
        assert!(!code.contains(native), "the panel reads {native}");
    }
}

const NEURAL_KIND: &str = "alez/neural:neural";
const PANEL_REFER: &str =
    "(import eseq.kinds :refer (graph-of graph-param-named graph-edge-to tracks
                                                       process-library bind-port! remove-process!))
                           (import eseq.view-kit :refer (named))";

/// A `neural` instance's panel in the factory DAW, shown alone in the
/// window; `nn` is the instance, `(node k)` its graph's node k and `(proc
/// id)` node 1's process `id` in Lisp.
struct Panel {
    h: Harness,
    graph: u64,
    bay: String,
}

impl Panel {
    /// The factory DAW (the process library published) with one `neural`
    /// instance, its panel showing.
    fn new() -> Self {
        let mut h = distro();
        h.publish_library();
        h.eval("(import alez.neural.variable-reset)");
        let graph = create(&mut h, NEURAL_KIND, None);
        let bay = sequencer::lisp_host::instance_view_buffer_name("neural", "neural 1");
        let mut panel = Self { h, graph, bay };
        panel.eval(&format!(
            "(def nn (instance-ref {graph}))
             (def node (lambda (k) (let ((g (graph-of nn))) (nth g.nodes k))))
             (def proc (lambda (id) (let ((n (node 1))) (first (filter (lambda (p) (= p.proc-id id)) n.processes)))))"
        ));
        let bay = panel.bay.clone();
        panel.show(&bay);
        panel
    }

    /// `new` with node 1's editor open.
    fn open() -> Self {
        let mut panel = Self::new();
        panel.expand(1);
        panel
    }

    /// Apply what Lisp queued, sync and render until the kinds settle (a
    /// render's cold reads register what the next sync pushes).
    fn render(&mut self) {
        for _ in 0..2 {
            // The editor's own side effects (a layout, a buffer switch)
            // first: the drain takes every queued command.
            self.h.editor.runtime_mut().run_reactive_cycle();
            self.h.editor.refresh_runtime_side_effects();
            self.h.drain();
            self.h.sync();
            self.h.show_all();
        }
    }

    fn eval(&mut self, code: &str) -> Value {
        let value = self.h.eval_with(PANEL_REFER, code);
        self.render();
        value
    }

    fn expand(&mut self, node: i64) {
        self.eval(&format!(
            "(alez.neural.variable-reset/gvr-expand-node nn {node})"
        ));
    }

    /// Add a process of `class` to node 1 through the native (it answers
    /// with the new id at once; the kinds list it at the next sync).
    fn add(&mut self, class: &str) -> u64 {
        match self.eval(&format!("(graph-node-process-add nn 1 \"{class}\")")) {
            Value::Number(id) => id as u64,
            other => panic!("add {class}: {other:?}"),
        }
    }

    /// Wire node 1's process `source`'s `port` into process `target`'s
    /// `inlet`, as the bay's cable drag does (`bind-port!`).
    fn wire(&mut self, source: u64, port: &str, target: u64, inlet: &str) {
        self.eval(&format!(
            "(let ((a (proc {source})) (b (proc {target})))
               (bind-port! (named a.ports \"{port}\") (named b.inlets \"{inlet}\")))"
        ));
    }

    /// Remove node 1's process `id`, as the card's delete does.
    fn remove(&mut self, id: u64) {
        self.eval(&format!("(remove-process! (proc {id}))"));
    }

    /// Select card `id` in node 1's bay, as a click on the card does.
    fn select(&mut self, id: u64) {
        self.eval(&format!(
            "(eseq.sequencer/lane-patch-select-lane (eseq.sequencer/lane-patch-node-namespace nn 1) {id})"
        ));
    }

    /// Field `field` of each of node 1's processes, in fire order.
    fn chain(&mut self, field: &str) -> Vec<Value> {
        items(&self.eval(&format!(
            "(let ((n (node 1))) (map (lambda (p) p.{field}) n.processes))"
        )))
    }

    fn classes(&mut self) -> Vec<String> {
        self.chain("class-name")
            .into_iter()
            .map(|class| match class {
                Value::String(class) => class,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn ids(&mut self) -> Vec<u64> {
        self.chain("proc-id")
            .into_iter()
            .map(|id| num(id) as u64)
            .collect()
    }

    /// Inlet `name` of node 1's process `id` (nil when gone).
    fn inlet(&mut self, id: u64, name: &str) -> Value {
        self.eval(&format!(
            "(let ((p (proc {id})) (i (when p (named p.inlets \"{name}\")))) (when i i.value))"
        ))
    }

    /// Where process `id`'s port `port` is wired, as (proc-id inlet), or nil.
    fn wired_to(&mut self, id: u64, port: &str) -> Value {
        self.eval(&format!(
            "(let ((p (proc {id})) (pt (when p (named p.ports \"{port}\"))))
               (when (and pt pt.target-process) (list pt.target-process.proc-id pt.target-inlet)))"
        ))
    }

    fn mapped(&mut self, id: u64, port: &str) -> Value {
        self.eval(&format!(
            "(let ((p (proc {id})) (pt (named p.ports \"{port}\"))) pt.target-step-param)"
        ))
    }

    fn activate(&mut self, name: &str) {
        let id = (self.h.editor.buffers.iter())
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .id;
        self.h.editor.set_active_buffer(id);
        self.render();
    }

    /// `name` alone in the window (replaces the tile tree), active.
    fn show(&mut self, name: &str) {
        self.eval(&format!(
            "(set-layout (list :buf \"{name}\" :hide-status true))"
        ));
        self.h.editor.set_layout_viewport(240, 100);
        self.activate(name);
    }

    fn layout_of(&mut self, name: &str) -> Arc<LayoutNode> {
        self.show(name);
        self.h.editor.widget_layout().expect("layout")
    }

    fn has(&mut self, buffer: &str, key: &str) -> bool {
        let layout = self.layout_of(buffer);
        keyed(&layout, key).is_some()
    }

    /// A real click (down and up) at the middle of `buffer`'s widget `key`.
    fn click_in(&mut self, buffer: &str, key: &str) {
        let layout = self.layout_of(buffer);
        let node = measured(&layout, key).clone();
        let (col, row) = (
            node.rect.col + node.rect.width * 0.5,
            node.rect.row + node.rect.height * 0.5,
        );
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            self.h.editor.handle_mouse_precise(
                MouseEvent {
                    kind,
                    column: col.floor() as u16,
                    row: row.floor() as u16,
                    modifiers: KeyModifiers::NONE,
                },
                0,
                0,
                240,
                100,
                col,
                row,
            );
        }
        self.render();
    }

    /// Call `buffer`'s widget `key`'s `:on-change` with `value`.
    fn change_in(&mut self, buffer: &str, key: &str, value: Value) {
        let layout = self.layout_of(buffer);
        let node = measured(&layout, key).clone();
        let on_change = node.props.get("on-change").cloned().expect("on-change");
        (self.h.editor.runtime_mut())
            .invoke(on_change, vec![value])
            .expect("on-change");
        self.render();
    }

    fn showing(&mut self) -> bool {
        self.eval("(eseq.processes-buffer/showing?)") == Value::Bool(true)
    }

    /// The instance's tab in the main tile: the app's layout, then a click
    /// on the tab (the host registered it when it created the instance).
    fn show_tab(&mut self) {
        self.relayout();
        self.click_tab("neural 1");
    }

    /// The Seq tab in the main tile (creating the instance opened its own).
    fn show_seq_tab(&mut self) {
        self.eval("(set! eseq.seq-step-tabs/step-panel-buffer \"*sequencer*\")");
        self.relayout();
    }

    /// The real layout (sidebar, main, right column) as the app lays it out.
    fn relayout(&mut self) {
        self.eval("(eseq.seq-layout/refresh-current-layout)");
    }

    /// The main area's layout spec: the main tile and the right column.
    fn column_spec(&mut self) -> String {
        let value = self.eval("(eseq.seq-layout/step-and-track-panel-layout-spec)");
        eseqlisp::vm::format_lisp_value(&value)
    }

    /// Buffer names of the tiles on screen, with each leaf's height in a
    /// 240x100 window.
    fn tiles(&self) -> Vec<(String, f32)> {
        let editor = &self.h.editor;
        let root = &editor.tile_root;
        let area = eseqlisp::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 240.0,
            height: 100.0,
        };
        root.compute_rects(area, 0.0, 1.0, 1.0)
            .into_iter()
            .filter_map(|(id, rect)| root.find_leaf(id).map(|leaf| (leaf, rect)))
            .map(|(leaf, rect)| (editor.buffers[leaf.buffer_idx].name.clone(), rect.height))
            .collect()
    }

    fn visible(&self) -> Vec<String> {
        self.tiles().into_iter().map(|(name, _)| name).collect()
    }

    /// The tiles on screen: the browser always; in the right column either
    /// the dock (with `code` under it, or not) or *step* over *track*.
    fn assert_column(&self, docked: bool, code: Option<&str>, when: &str) {
        let visible = self.visible();
        let shows = |name: &str| visible.iter().any(|shown| shown == name);
        assert!(
            shows("*samples*"),
            "{when}: the browser never moves: {visible:?}"
        );
        assert_eq!(shows("*processes*"), docked, "{when}: {visible:?}");
        assert_eq!(shows("*step*"), !docked, "{when}: {visible:?}");
        assert_eq!(shows("*track*"), !docked, "{when}: {visible:?}");
        let code_tiles = (visible.iter())
            .filter(|shown| shown.starts_with("*expr"))
            .count();
        match code {
            Some(name) => assert!(
                shows(name) && code_tiles == 1,
                "{when}: code tile {name}: {visible:?}"
            ),
            None => assert_eq!(code_tiles, 0, "{when}: no code tile: {visible:?}"),
        }
    }

    /// Click the main tile's buffer tab labelled `label` with the mouse.
    fn click_tab(&mut self, label: &str) {
        self.h.editor.update_tile_rects(240, 100);
        let editor = &self.h.editor;
        let (col, row) = (editor.tile_rects().iter())
            .find_map(|(id, rect)| {
                let leaf = editor.tile_root.find_leaf(*id)?;
                let index = leaf.tabs.iter().position(|tab| tab.label == label)?;
                let tab = eseqlisp::tile::tile_tab_layouts(*rect, &leaf.tabs, leaf.selected_tab)
                    .into_iter()
                    .find(|tab| tab.index == index)?;
                Some((
                    tab.rect.col + tab.rect.width * 0.5,
                    tab.rect.row + tab.rect.height * 0.5,
                ))
            })
            .unwrap_or_else(|| panic!("no tab {label} on screen"));
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            self.h.editor.handle_tiled_mouse_precise(
                MouseEvent {
                    kind,
                    column: col.floor() as u16,
                    row: row.floor() as u16,
                    modifiers: KeyModifiers::NONE,
                },
                col,
                row,
                0,
            );
            self.render();
        }
    }

    /// Replace the active buffer's text the way typing does.
    fn type_body(&mut self, body: &str) {
        let buffer = self.h.editor.active_buffer_mut();
        buffer.set_text(body);
        buffer.dirty = true;
    }

    fn commit_chord(&mut self) {
        for _ in 0..2 {
            (self.h.editor).handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        }
        self.render();
    }

    /// Type `(+ $be` into the active buffer: whether completion offers
    /// `$beat`. Restores the text.
    fn offers_beat(&mut self) -> bool {
        let editor = &mut self.h.editor;
        let before = editor.active_buffer().text();
        let dirty = editor.active_buffer().dirty;
        editor.active_buffer_mut().set_text("(+ $b");
        editor.active_buffer_mut().cursor = (0, 5);
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        let offered = (editor.completion_state())
            .is_some_and(|completion| completion.items.iter().any(|item| item.label == "$beat"));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let buffer = editor.active_buffer_mut();
        buffer.set_text(&before);
        buffer.dirty = dirty;
        offered
    }

    fn source(&mut self, id: u64) -> Value {
        self.eval(&format!("(graph-node-process-expr-source nn 1 {id})"))
    }

    fn set_body(&mut self, id: u64, body: &str) {
        let ok = self.eval(&format!(
            "(get (graph-node-process-expr-set nn 1 {id} \"{body}\") :ok)"
        ));
        assert_eq!(ok, Value::Bool(true), "{body}");
    }

    /// Re-evaluate `file` (relative to content/) through the hot-reload
    /// pipeline, with `from` replaced by `to` in its source.
    fn reload(&mut self, file: &str, from: &str, to: &str) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let path = root.join(file).canonicalize().expect(file);
        let text = std::fs::read_to_string(&path).expect(file);
        assert_eq!(
            text.matches(from).count(),
            1,
            "{file}: patch anchor {from:?}"
        );
        let editor = &mut self.h.editor;
        let mut overlays = editor.snapshot_file_backed_sources();
        overlays.retain(|overlay| overlay.path != path);
        overlays.push(eseqlisp::SourceOverlay {
            path: path.clone(),
            text: text.replacen(from, to, 1),
            dirty: true,
            revision: 1,
        });
        let report = editor
            .runtime_mut()
            .reload_paths_transactional(vec![path], overlays);
        assert!(report.success, "{file}: {:?}", report.diagnostics);
        editor.process_lisp_reload_report(report);
        self.render();
    }

    fn undo(&mut self) {
        self.h.undo();
        self.render();
    }

    fn redo(&mut self) {
        app::edit::redo(&mut self.h.app);
        self.h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.render();
    }

    fn undo_len(&self) -> usize {
        self.h.app.history.undo_len()
    }

    /// End a pointer gesture (a drag's release).
    fn release(&mut self) {
        self.h.gesture.pointer_down = false;
        app::edit::finish_active_gesture(&mut self.h.app);
    }
}

/// Create an instance of `kind` (owned by the rack `group`, or the
/// project), labelled `label` when given; sync; returns its id.
fn create(h: &mut Harness, kind: &str, label: Option<&str>) -> u64 {
    let before = crate::host_commands::instances::instance_ids(&h.app);
    let mut payload = vec![("kind", s(kind))];
    if let Some(label) = label {
        payload.push(("label", s(label)));
    }
    h.command("instance-create", map_value(payload));
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created.difference(&before).next().expect("an instance");
    h.sync();
    h.show_all();
    h.sync();
    h.show_all();
    id
}

/// The node of `layout` whose stable key ends with `suffix` (an instance
/// view's keys are prefixed with its instance).
fn keyed<'a>(layout: &'a LayoutNode, suffix: &str) -> Option<&'a LayoutNode> {
    if layout
        .stable_key
        .as_deref()
        .is_some_and(|key| key.ends_with(suffix))
    {
        return Some(layout);
    }
    layout
        .children
        .iter()
        .find_map(|child| keyed(child, suffix))
}

/// `keyed`, laid out with a finite, non-empty rect.
fn measured<'a>(layout: &'a LayoutNode, suffix: &str) -> &'a LayoutNode {
    let node = keyed(layout, suffix).unwrap_or_else(|| panic!("{suffix} shows"));
    let rect = node.rect;
    assert!(
        rect.row.is_finite() && rect.col.is_finite() && rect.width > 0.0 && rect.height > 0.0,
        "{suffix}: {rect:?}"
    );
    node
}

/// Whether `value` is the number `expected` (inlet values are stored as
/// f32).
fn close(value: Value, expected: f64) -> bool {
    matches!(value, Value::Number(n) if (n - expected).abs() < 1e-6)
}

fn prop(node: &LayoutNode, name: &str) -> Value {
    node.props.get(name).cloned().unwrap_or(Value::Nil)
}

/// A bound prop's current value.
fn slot_value(value: &Value) -> f64 {
    match value {
        Value::ReactiveRef { slot, .. } => read_float_slot(slot),
        other => panic!("not a binding: {other:?}"),
    }
}

/// The (full-buffer, subtree) re-runs a render after `edit` makes.
fn reruns(panel: &mut Panel, edit: impl FnOnce(&mut Panel)) -> (u64, u64) {
    panel.render();
    let before = panel.h.rt().ui_work_counters();
    edit(panel);
    panel.render();
    let after = panel.h.rt().ui_work_counters();
    (
        after.full_buffer_reruns - before.full_buffer_reruns,
        after.subtree_reruns - before.subtree_reruns,
    )
}

// ── the kind and its panel ───────────────────────────────────────────────

/// Importing the package registers its `neural` kind (instance-kinds spec
/// §3/§8.1) and publishes nothing: the id comes from the package manifest,
/// the kind carries its :state view cells, :view, :keymap and :on-create,
/// and the manifest's declared kinds pass the attach check. An instance
/// publishes the kind's graph under its own id.
#[test]
fn importing_the_neural_package_registers_its_kind() {
    let mut h = Harness::new();
    let published = h.shared.state.published_sequencers().len();
    h.eval("(import alez.neural.variable-reset)");
    assert_eq!(
        h.shared.state.published_sequencers().len(),
        published,
        "importing a kind creates no instance"
    );
    let kind = sequencer::lisp_host::registered_kind(NEURAL_KIND)
        .expect("alez/neural:neural registered on import");
    assert_eq!(kind.module.as_deref(), Some("alez.neural.variable-reset"));
    assert_eq!(
        kind.state_fields,
        [
            "expanded-node",
            "selected-neuron",
            "map-slot",
            "map-port",
            "piano-depth"
        ]
        .map(str::to_string)
        .to_vec()
    );
    assert!(kind.has_view);
    assert_eq!(
        kind.keymap.as_deref(),
        Some("eseq.sequencer-keys/sequencer-keys")
    );
    let schema = h.rt().instance_kind_schema(NEURAL_KIND).expect("schema");
    assert!(
        schema.on_create.is_some(),
        "the ring default is the kind's :on-create"
    );
    let manifest = kind
        .sequencer
        .clone()
        .expect("the kind carries a :sequencer");
    assert_eq!(manifest.max_poly, 4);
    assert_eq!(manifest.node.params.len(), 9);
    let published = sequencer::lisp_host::instance_published_sequencer(&kind, 7, None)
        .expect("instance sequencer");
    assert_eq!((published.id, published.name.as_str()), (7, "neural#7"));
    assert_eq!(published.graph.expect("graph").node, manifest.node);
    let catalog = sequencer::app_paths::app_paths().package_catalog();
    let package = catalog
        .package_for_module("alez.neural.variable-reset")
        .expect("alez/neural installed");
    assert_eq!(
        sequencer::lisp_host::check_manifest_kinds(&package.manifest, "alez.neural.variable-reset"),
        Ok(Vec::new())
    );
}

/// The panel reads its graph through the kinds: the config controls bind
/// their fields, each row its node's fields and params, the matrices the
/// weights and playback; edits go through the setters, one undo entry each.
#[test]
fn the_neural_panel_binds_its_graph_and_edits_through_the_kinds() {
    let mut p = Panel::new();
    let layout = p.layout_of(&p.bay.clone());
    for key in [
        "graph-variable-reset-node-count",
        "graph-variable-reset-max-poly-selection",
        "graph-variable-reset-threshold",
        "graph-variable-reset-weight-matrix",
        "graph-variable-reset-trigger-matrix",
        "graph-variable-reset-energy-matrix",
        "graph-variable-reset-dampening-matrix",
        "graph-variable-reset-event-view",
        "graph-variable-reset-piano",
        "graph-variable-reset-row-7",
        "graph-variable-reset-sounding-7",
        "graph-variable-reset-expand-7",
    ] {
        measured(&layout, key);
    }
    assert!(
        keyed(&layout, "graph-variable-reset-row-8").is_none(),
        "eight nodes"
    );
    let bay = p.bay.clone();
    let (tree, _) = p.h.buffer_tree(&bay);
    let instance = |p: &mut Panel, code: &str| match p.eval(code) {
        Value::Instance(id) => id,
        other => panic!("{code}: {other:?}"),
    };
    let g = instance(&mut p, "(graph-of nn)");
    let node = instance(&mut p, "(node 2)");
    let transpose = instance(&mut p, "(graph-param-named (node 2) \"transpose\")");
    let threshold = instance(&mut p, "(graph-param-named (node 0) \"threshold\")");
    let widget = |key: &str| widget_keyed(&tree, key).unwrap_or_else(|| panic!("{key}"));
    assert!(binds(
        &widget("graph-variable-reset-node-count"),
        "value",
        g,
        "node-count"
    ));
    assert!(binds(
        &widget("graph-variable-reset-reset-bars"),
        "value",
        g,
        "reset-bars"
    ));
    assert!(binds(
        &widget("graph-variable-reset-threshold"),
        "value",
        threshold,
        "value"
    ));
    assert!(binds(
        &widget("graph-variable-reset-delay-2"),
        "value",
        node,
        "delay"
    ));
    assert!(binds(
        &widget("graph-variable-reset-seed-route-2"),
        "value",
        node,
        "seed-route"
    ));
    assert!(binds(
        &widget("graph-variable-reset-transpose-2"),
        "value",
        transpose,
        "value"
    ));
    assert!(binds(
        &widget("graph-variable-reset-event-view"),
        "current-beat",
        g,
        "beat"
    ));
    assert_eq!(
        widget("graph-variable-reset-route-2")["value"],
        s("Track 1")
    );
    assert_eq!(
        widget("graph-variable-reset-resolution-2")["value"],
        s("16")
    );
    assert_eq!(widget("graph-variable-reset-group-2")["value"], s("A"));
    assert_eq!(
        widget("graph-variable-reset-max-poly-selection")["value"],
        s("propagation")
    );
    // The ring from :on-create shows in the weight matrix.
    let weights = widget("graph-variable-reset-weight-matrix")["value"].clone();
    assert_eq!(items(&items(&weights)[0])[1], number(1.0));

    // Edits: a setter each, one undo entry each.
    let entries = p.undo_len();
    for (key, value) in [
        ("graph-variable-reset-delay-2", number(3.0)),
        ("graph-variable-reset-transpose-2", number(5.0)),
        ("graph-variable-reset-route-2", s("Track 2")),
        ("graph-variable-reset-group-2", s("C")),
        ("graph-variable-reset-quantize-2", s("off")),
        ("graph-variable-reset-max-poly-selection", s("random")),
    ] {
        p.change_in(&bay, key, value);
    }
    assert_eq!(p.undo_len(), entries + 6, "one entry per edit");
    assert_eq!(
        p.eval("(let ((n (node 2))) (list n.delay n.group n.quantize n.route.index))"),
        list_value([number(3.0), number(2.0), s("off"), number(1.0)])
    );
    assert_eq!(
        p.eval("(let ((p (graph-param-named (node 2) \"transpose\"))) p.value)"),
        number(5.0)
    );
    assert_eq!(
        p.eval("(let ((g (graph-of nn))) g.max-poly-selection)"),
        s("random")
    );
    p.change_in(&bay, "graph-variable-reset-route-2", s("Off"));
    assert_eq!(p.eval("(let ((n (node 2))) n.route)"), Value::Nil);
    p.undo();
    assert_eq!(
        p.eval("(let ((n (node 2))) n.route.index)"),
        number(1.0),
        "undo restores the route"
    );

    // A weight cell, pressed and changed.
    let layout = p.layout_of(&bay);
    let matrix = measured(&layout, "graph-variable-reset-weight-matrix").clone();
    let invoke = |p: &mut Panel, prop: &str, args: Vec<Value>| {
        let callback = matrix.props.get(prop).cloned().expect(prop);
        p.h.editor.runtime_mut().invoke(callback, args).expect(prop);
        p.render();
    };
    invoke(&mut p, "on-cell-press", vec![number(0.0), number(3.0)]);
    assert_eq!(p.eval("nn.selected-neuron"), number(3.0));
    let (tree, _) = p.h.buffer_tree(&bay);
    let row = widget_keyed(&tree, "graph-variable-reset-row-3").expect("row 3");
    assert_eq!(
        row["selected"],
        Value::Bool(true),
        "the pressed column's row lights"
    );
    invoke(&mut p, "on-cell-release", vec![number(0.0), number(3.0)]);
    invoke(
        &mut p,
        "on-cell-change",
        vec![number(2.0), number(5.0), number(0.4)],
    );
    assert_eq!(
        p.eval("(let ((n (node 2)) (e (graph-edge-to n 5)) (w (graph-param-named e \"weight\"))) w.value)"),
        number(0.4)
    );
}

/// The batch params set every node at once, one undo entry (a drag's
/// frames join it): the threshold up to the capacity, so a node that
/// becomes active later carries it; global transpose and dur x the active
/// nodes. Growing and shrinking keeps a dormant node's overrides.
#[test]
fn the_neural_panels_batch_params_are_one_entry_and_reach_dormant_nodes() {
    let mut p = Panel::new();
    let bay = p.bay.clone();
    let entries = p.undo_len();
    p.h.gesture.pointer_down = true;
    for v in [0.6, 0.7, 0.8] {
        p.change_in(&bay, "graph-variable-reset-threshold", number(v));
    }
    p.release();
    assert_eq!(p.undo_len(), entries + 1, "the drag is one entry");
    p.change_in(&bay, "graph-variable-reset-global-transpose", number(12.0));
    assert_eq!(p.undo_len(), entries + 2);
    let overrides = (p.h.shared.state.current_graph_overrides().into_iter())
        .find(|graph| graph.sequencer_id == p.graph)
        .expect("overrides");
    let count = |name: &str, value: f64| {
        (overrides.node_params.iter())
            .filter(|param| param.param == name && param.value == value)
            .count()
    };
    assert_eq!(count("threshold", 0.8), 16, "every node up to the capacity");
    assert_eq!(count("global-transpose", 12.0), 8, "the active nodes");
    let threshold_14 = "(let ((p (graph-param-named (node 14) \"threshold\"))) p.value)";
    p.change_in(&bay, "graph-variable-reset-node-count", number(16.0));
    assert_eq!(
        p.eval(threshold_14),
        number(0.8),
        "a node grown later carries the threshold"
    );
    // Shrinking leaves node 14 dormant, not reset: grown back, it still
    // carries the override.
    p.change_in(&bay, "graph-variable-reset-node-count", number(8.0));
    assert_eq!(
        p.eval("(let ((g (graph-of nn))) (len g.nodes))"),
        number(8.0)
    );
    p.change_in(&bay, "graph-variable-reset-node-count", number(16.0));
    assert_eq!(
        p.eval(threshold_14),
        number(0.8),
        "a dormant node keeps its override"
    );
    assert_eq!(p.undo_len(), entries + 5, "one entry per count edit");
    for _ in 0..4 {
        p.undo();
    }
    assert_eq!(
        p.eval("(let ((p (graph-param-named (node 3) \"global-transpose\"))) p.value)"),
        number(0.0),
        "undo restores every node's"
    );
    p.undo();
    assert_eq!(
        p.eval("(let ((p (graph-param-named (node 3) \"threshold\"))) p.value)"),
        number(0.55),
        "the whole drag undone at once"
    );
}

/// Routes: the graph's tracks, then the jakis with its owner (gated, then
/// restarted), then Off; two jakis sharing a label carry their ids, and a
/// pick routes to the jaki it names. The route color strip follows the
/// track.
#[test]
fn the_neural_route_menu_offers_tracks_and_jakis() {
    let mut p = Panel::new();
    p.eval("(import alez.jaki.kind)");
    let drums = create(&mut p.h, "alez/jaki:jaki", Some("drums"));
    let drums_2 = create(&mut p.h, "alez/jaki:jaki", Some("drums"));
    let bass = create(&mut p.h, "alez/jaki:jaki", Some("bass"));
    p.render();
    let bay = p.bay.clone();
    let (tree, _) = p.h.buffer_tree(&bay);
    let route = widget_keyed(&tree, "graph-variable-reset-route-0").expect("route");
    let labels = |names: &[&str]| list_value(names.iter().map(|name| s(name)));
    assert_eq!(
        route["options"],
        labels(&[
            "Track 1",
            "Track 2",
            &format!("→ drums #{drums}"),
            &format!("→ drums #{drums_2}"),
            "→ bass",
            &format!("↺ drums #{drums}"),
            &format!("↺ drums #{drums_2}"),
            "↺ bass",
            "Off",
        ])
    );
    p.change_in(
        &bay,
        "graph-variable-reset-route-0",
        s(&format!("→ drums #{drums_2}")),
    );
    assert_eq!(
        p.eval("(let ((n (node 0))) (list n.generator n.restart))"),
        list_value([number(drums_2 as f64), Value::Bool(false)])
    );
    p.change_in(&bay, "graph-variable-reset-route-1", s("↺ bass"));
    assert_eq!(
        p.eval("(let ((n (node 1))) (list n.generator n.restart))"),
        list_value([number(bass as f64), Value::Bool(true)])
    );
    let (tree, _) = p.h.buffer_tree(&bay);
    let label = |key: &str| widget_keyed(&tree, key).expect(key)["value"].clone();
    assert_eq!(
        label("graph-variable-reset-route-0"),
        s(&format!("→ drums #{drums_2}"))
    );
    assert_eq!(label("graph-variable-reset-route-1"), s("↺ bass"));
    let strip = widget_keyed(&tree, "graph-variable-reset-route-color-0").expect("strip");
    assert_eq!(
        strip["active"],
        number(0.0),
        "gating a jaki: no track color"
    );
    // Back to a track.
    p.change_in(&bay, "graph-variable-reset-route-0", s("Track 2"));
    assert_eq!(
        p.eval("(let ((n (node 0))) (list n.generator n.route.index))"),
        list_value([number(-1.0), number(1.0)])
    );
    let (tree, _) = p.h.buffer_tree(&bay);
    let strip = widget_keyed(&tree, "graph-variable-reset-route-color-0").expect("strip");
    assert_eq!(strip["active"], number(1.0));
    let color = p.eval("(let ((t (nth (tracks) 1))) (eseq.view-kit/rgb-part t.color 0))");
    assert_eq!(strip["track-r"], color, "the track's color");
}

/// A rack-owned instance: its routes are the rack's members ("n name"), it
/// wears the rack's name, and the piano colors by member.
#[test]
fn a_rack_owned_neural_routes_to_its_members() {
    let mut h = distro();
    h.pkg_tracks(3);
    let (group, _) = h.app.create_drum_rack_recorded(None).expect("rack");
    let first = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    for (pad, track) in [2, 0].into_iter().enumerate() {
        h.app
            .assign_rack_pad_track_recorded(group, first + pad as i32, track)
            .expect("pad");
    }
    h.app.state.set_rack_memberships(h.app.rack_memberships());
    h.share_buses_and_groups();
    h.sync();
    h.eval("(import alez.neural.variable-reset)");
    let before = crate::host_commands::instances::instance_ids(&h.app);
    h.command(
        "instance-create",
        map_value([("kind", s(NEURAL_KIND)), ("group-id", number(group as f64))]),
    );
    let created = crate::host_commands::instances::instance_ids(&h.app);
    let id = *created.difference(&before).next().expect("an instance");
    for _ in 0..2 {
        h.sync();
        h.show_all();
    }
    let label = h.app.instances.get(id).expect("instance").label.clone();
    let buffer = sequencer::lisp_host::instance_view_buffer_name("neural", &label);
    let (tree, _) = h.buffer_tree(&buffer);
    let route = widget_keyed(&tree, "graph-variable-reset-route-0").expect("route");
    let names = &h.app.tracks;
    assert_eq!(
        route["options"],
        list_value([
            s(&format!("3 {}", names[2])),
            s(&format!("1 {}", names[0])),
            s("Off")
        ])
    );
    let badge = widget_keyed(&tree, "graph-variable-reset-owner-rack").expect("the rack chip");
    let rack_name = (h.app.groups.iter())
        .find(|g| g.id == group)
        .map(|g| g.name.clone())
        .expect("the rack");
    assert_eq!(
        badge["text"],
        s(&format!(
            "attached to {}",
            &rack_name[..rack_name.len().min(14)]
        ))
    );
    let piano = widget_keyed(&tree, "graph-variable-reset-piano").expect("piano");
    assert_eq!(
        items(&piano["track-colors"]).len(),
        2,
        "one color per member"
    );
}

/// Playback repaints only the panel's playback views: the firing history
/// and the trigger, energy and dampening matrices re-run their subtrees,
/// the beat is bound, the controls stay; the tracks' notes re-run only the
/// keyboard. The views follow the node count.
#[test]
fn the_neural_panels_playback_reruns_only_the_playback_views() {
    let mut p = Panel::new();
    let bay = p.bay.clone();
    let state = p.h.shared.state.clone();
    for count in [8, 3] {
        if count != 8 {
            p.change_in(
                &bay,
                "graph-variable-reset-node-count",
                number(count as f64),
            );
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
            let graph = p.graph;
            let (full, subtrees) = reruns(&mut p, |_| {
                state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
                    id: graph,
                    name: "neural".to_string(),
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
            });
            assert_eq!(full, 0, "playback never re-runs the panel");
            assert_eq!(subtrees, 4, "the history and the three playback matrices");
            let layout = p.layout_of(&bay);
            let events = measured(&layout, "graph-variable-reset-event-view");
            assert_eq!(slot_value(&prop(events, "current-beat")), beat);
            assert_eq!(prop(events, "y-max"), number((count - 1) as f64));
            assert_eq!(
                prop(events, "events"),
                list_value([list_value([1.0, 0.0, beat, 7.0, 0.75].map(number))])
            );
            for (key, field) in [
                (
                    "trigger-matrix",
                    "(let ((g (graph-of nn))) (map (lambda (x) (list x)) g.triggers))",
                ),
                (
                    "energy-matrix",
                    "(let ((g (graph-of nn))) (map (lambda (x) (list x)) g.energy))",
                ),
                ("dampening-matrix", "(let ((g (graph-of nn))) g.dampening)"),
            ] {
                let matrix = measured(&layout, &format!("graph-variable-reset-{key}")).clone();
                assert_eq!(prop(&matrix, "rows"), number(count as f64));
                assert_eq!(prop(&matrix, "value"), p.eval(field), "{key}");
            }
            let energy = prop(
                measured(&layout, "graph-variable-reset-energy-matrix"),
                "value",
            );
            assert_eq!(items(&items(&energy)[1])[0], number(beat));

            let (full, subtrees) = reruns(&mut p, |_| {
                state.replace_live_notes(0, [(60 + beat as u8, 0.75)]);
            });
            assert_eq!(full, 0);
            assert_eq!(subtrees, 1, "only the keyboard");
            let layout = p.layout_of(&bay);
            let notes = prop(
                measured(&layout, "graph-variable-reset-piano"),
                "notes-by-track",
            );
            assert_eq!(
                (items(&items(&notes)[0]).iter())
                    .map(|note| items(note)[0].clone())
                    .collect::<Vec<_>>(),
                vec![number(60.0 + beat)]
            );
        }
    }
    // Stopped and cleared: no history, no notes.
    state.set_graph_visualizations(Vec::new());
    state.replace_live_notes(0, []);
    p.render();
    let layout = p.layout_of(&bay);
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

/// What a node sounds (`n.sounding`): one chip per open gate at the audio
/// clock, each as opaque as its velocity; a change re-runs that node's
/// readout alone, and stopping clears it.
#[test]
fn the_neural_panels_sounding_notes_rerun_only_their_row_readout() {
    let mut p = Panel::new();
    let bay = p.bay.clone();
    let note = |note: f32, start_sample: u64, end_sample: u64| GraphSoundingNote {
        note,
        velocity: 0.25 + note.abs() / 100.0,
        start_sample,
        end_sample,
    };
    let mut node_sounding = vec![Vec::new(); 8];
    // Two overlapping gates on node 1 plus one that already closed and one
    // that has not opened yet at the audio clock.
    node_sounding[1] = vec![
        note(9.0, 0, 500),
        note(-3.0, 600, 2_000),
        note(5.0, 900, 1_200),
        note(7.0, 1_500, 1_900),
    ];
    let state = p.h.shared.state.clone();
    state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        id: p.graph,
        name: "neural".to_string(),
        active: true,
        num_nodes: 8,
        node_sounding,
        ..Default::default()
    }]);
    state.set_audio_rendered_sample(1_000);
    p.render();
    let layout = p.layout_of(&bay);
    assert_eq!(
        prop(
            measured(&layout, "graph-variable-reset-sounding-1"),
            "count"
        ),
        number(0.0),
        "stopped"
    );
    let (full, subtrees) = reruns(&mut p, |p| p.h.set_playing(true));
    assert_eq!((full, subtrees), (0, 1), "node 1's readout alone");
    let layout = p.layout_of(&bay);
    let row = measured(&layout, "graph-variable-reset-sounding-1");
    assert_eq!(row.widget_type, "number-list");
    assert_eq!(prop(row, "count"), number(2.0));
    assert_eq!(prop(row, "values"), list_value([number(-3.0), number(5.0)]));
    let levels: Vec<f64> = items(&prop(row, "levels")).into_iter().map(num).collect();
    assert!(
        (levels[0] - 0.28).abs() < 1e-6 && (levels[1] - 0.30).abs() < 1e-6,
        "each chip's level is its note's velocity: {levels:?}"
    );
    // An unchanged clock re-runs nothing; stopping clears the row.
    assert_eq!(reruns(&mut p, |_| {}), (0, 0));
    let (_, subtrees) = reruns(&mut p, |p| p.h.set_playing(false));
    assert_eq!(subtrees, 1);
    let layout = p.layout_of(&bay);
    assert_eq!(
        prop(
            measured(&layout, "graph-variable-reset-sounding-1"),
            "count"
        ),
        number(0.0)
    );
}

/// Two instances of the real package (instance-kinds spec §10/§11): each
/// gets the ring from :on-create, and document edits, view state and widget
/// keys stay per instance.
#[test]
fn neural_instances_get_the_ring_on_create_and_stay_independent() {
    let mut p = Panel::new();
    let first = p.graph;
    let second = create(&mut p.h, NEURAL_KIND, None);
    p.eval(&format!("(def n2 (instance-ref {second}))"));
    let weight = |p: &mut Panel, inst: &str, from: usize, to: usize| {
        p.eval(&format!(
            "(let ((g (graph-of {inst})) (n (nth g.nodes {from})) (e (graph-edge-to n {to}))
                   (w (graph-param-named e \"weight\"))) w.value)"
        ))
    };
    for from in 0..8 {
        for inst in ["nn", "n2"] {
            assert_eq!(
                weight(&mut p, inst, from, (from + 1) % 8),
                number(1.0),
                "{inst} ring {from}"
            );
            assert_eq!(weight(&mut p, inst, from, (from + 2) % 8), number(0.0));
        }
    }
    assert_eq!(
        p.eval("(let ((n (node 0))) n.seed-route)"),
        Value::Bool(true),
        "node 0 seeds from its route"
    );

    // The second's matrix shows its own edit; the first is untouched.
    p.eval("(let ((g (graph-of n2)) (n (nth g.nodes 3)) (e (graph-edge-to n 5)) (w (graph-param-named e \"weight\"))) (set! w.value 0.75))");
    let second_buffer = sequencer::lisp_host::instance_view_buffer_name("neural", "neural 2");
    let layout = p.layout_of(&second_buffer);
    let matrix = measured(&layout, "graph-variable-reset-weight-matrix");
    assert!(
        matrix
            .stable_key
            .as_deref()
            .is_some_and(|key| key.starts_with(&format!("instance:{second}::"))),
        "{:?}",
        matrix.stable_key
    );
    assert_eq!(items(&items(&prop(matrix, "value"))[3])[5], number(0.75));
    assert_eq!(
        weight(&mut p, "nn", 3, 5),
        number(0.0),
        "instance 1 is untouched"
    );
    let layout = p.layout_of(&p.bay.clone());
    let matrix = measured(&layout, "graph-variable-reset-weight-matrix");
    assert!(
        matrix
            .stable_key
            .as_deref()
            .is_some_and(|key| key.starts_with(&format!("instance:{first}::")))
    );

    // View state is per instance.
    p.expand(2);
    assert_eq!(p.eval("nn.expanded-node"), number(2.0));
    assert_eq!(p.eval("n2.expanded-node"), number(-1.0));
}

// ── the expanded node editor ─────────────────────────────────────────────

/// The node's bay is the sequencer's patchbay over `n.processes`: a cable
/// from a dragged port wires through the process setters, and a selected
/// cable goes on a real click of the "× cable" chip and on a real
/// Backspace in the instance's buffer (the kind's :keymap).
#[test]
fn the_node_bay_wires_through_the_kinds_and_removes_the_selected_cable() {
    let mut p = Panel::open();
    let (rand, cmp) = (p.add("lane-rand"), p.add("lane-cmp"));
    let ns = num(p.eval("(eseq.sequencer/lane-patch-node-namespace nn 1)")) as u64;
    assert_eq!(
        ns,
        sequencer::lisp_host::graph_node_lane_patch_namespace(p.graph, 1) as u64
    );
    // rand's wire port (slot 0, ordinal 0) dropped on cmp's `a` (slot 1, its
    // first in port).
    let port_id = ns * 4096 * 16;
    let entries = p.undo_len();
    p.eval(&format!(
        "(eseq.sequencer/lane-patch-connect {ns} {port_id} 1 0)"
    ));
    assert_eq!(p.undo_len(), entries + 1, "one recorded edit");
    let wire = list_value([number(cmp as f64), s("a")]);
    assert_eq!(p.wired_to(rand, "wire"), wire);
    let bay = p.bay.clone();
    let select = |p: &mut Panel| {
        p.eval(&format!(
            "(eseq.sequencer/lane-patch-select-cable {ns} {port_id} 1 0)"
        ));
        assert_eq!(
            p.eval("(eseq.sequencer/lane-patch-cable-selected?)"),
            Value::Bool(true)
        );
    };
    select(&mut p);
    let layout = p.layout_of(&bay);
    let chip = measured(&layout, "lane-patch-remove-cable").clone();
    let (col, row) = (
        chip.rect.col + chip.rect.width * 0.5,
        chip.rect.row + chip.rect.height * 0.5,
    );
    p.h.editor.handle_mouse_precise(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col.floor() as u16,
            row: row.floor() as u16,
            modifiers: KeyModifiers::NONE,
        },
        0,
        0,
        240,
        100,
        col,
        row,
    );
    p.render();
    assert_eq!(p.wired_to(rand, "wire"), Value::Nil, "× removed the cable");

    p.eval(&format!(
        "(eseq.sequencer/lane-patch-connect {ns} {port_id} 1 0)"
    ));
    assert_eq!(p.wired_to(rand, "wire"), wire);
    select(&mut p);
    p.activate(&bay);
    p.h.editor
        .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    p.render();
    assert_eq!(
        p.wired_to(rand, "wire"),
        Value::Nil,
        "Backspace removed the cable"
    );
    assert_eq!(
        p.eval("(eseq.sequencer/lane-patch-cable-selected?)"),
        Value::Bool(false)
    );
}

/// The bay's card edits go through the setters: bypass, move and the card
/// menu's delete; the + box adds a class by its node label (classes that do
/// nothing on a node fire left out).
#[test]
fn the_node_bay_edits_cards_through_the_kinds() {
    let mut p = Panel::open();
    let (rand, cmp) = (p.add("lane-rand"), p.add("lane-cmp"));
    let ns = num(p.eval("(eseq.sequencer/lane-patch-node-namespace nn 1)")) as u64;
    let bay = p.bay.clone();
    p.click_in(&bay, &format!("lane-patch-enable-{rand}"));
    assert_eq!(
        p.chain("enabled"),
        vec![Value::Bool(false), Value::Bool(true)]
    );
    p.eval(&format!(
        "(eseq.sequencer/lane-patch-move-card {ns} {cmp} {rand})"
    ));
    assert_eq!(p.ids(), vec![cmp, rand], "moved before it");
    p.eval(&format!(
        "(eseq.sequencer/lane-patch-remove-card {ns} {cmp})"
    ));
    assert_eq!(p.ids(), vec![rand]);

    let layout = p.layout_of(&bay);
    let add = measured(&layout, "graph-variable-reset-proc-add-1").clone();
    let options: Vec<Value> = items(&prop(&add, "options"));
    let classes: Vec<Value> = items(&p.eval(
        "(map (lambda (c) c.node-label) (filter (lambda (c) (not c.node-hidden)) process-library.classes))",
    ));
    assert_eq!(options[..classes.len()], classes[..]);
    for (shown, hidden) in [("transpose", "lane-roll"), ("rand", "lane-grab")] {
        assert!(options.contains(&s(shown)), "{shown}");
        assert!(
            !options.contains(&s(hidden)) && !options.contains(&s(&hidden[5..])),
            "{hidden}"
        );
    }
    let on_change = prop(&add, "on-change");
    p.h.editor
        .runtime_mut()
        .invoke(on_change, vec![s("transpose")])
        .expect("pick");
    p.render();
    assert_eq!(
        p.classes(),
        vec!["lane-rand".to_string(), "neural-transpose".to_string()]
    );
}

/// The process card's inlets read and set the kinds: a gate is a toggle, an
/// enum a dropdown of its labels, a track inlet a dropdown of the graph's
/// tracks then its neurons (stored as -(k+1)); a number picker binds the
/// inlet's value.
#[test]
fn the_process_card_edits_its_inlets_through_the_kinds() {
    let mut p = Panel::open();
    let grab = p.add("lane-grab");
    p.select(grab);
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    let bay = p.bay.clone();
    let key = |name: &str| format!("graph-variable-reset-proc-1-{grab}-{name}");
    let layout = p.layout_of(&bay);
    measured(&layout, &format!("graph-variable-reset-proc-card-1-{grab}"));
    assert_eq!(prop(measured(&layout, &key("value")), "value"), s("note"));
    let source = measured(&layout, &key("source")).clone();
    let options = items(&prop(&source, "options"));
    assert_eq!(options[..3], [s("Track 1"), s("Track 2"), s("nrn 0")]);
    assert_eq!(options.len(), 2 + 8);
    assert_eq!(prop(&source, "value"), s("Track 1"));

    p.change_in(&bay, &key("value"), s("dur"));
    assert!(close(p.inlet(grab, "value"), 2.0));
    p.change_in(&bay, &key("source"), s("nrn 3"));
    assert!(close(p.inlet(grab, "source"), -4.0), "a neuron source");
    let layout = p.layout_of(&bay);
    assert_eq!(prop(measured(&layout, &key("source")), "value"), s("nrn 3"));
    p.change_in(&bay, &key("source"), s("Track 2"));
    assert!(close(p.inlet(grab, "source"), 1.0));
    p.change_in(&bay, &key("grab"), Value::Bool(true));
    assert!(close(p.inlet(grab, "grab"), 1.0));

    let rand = p.add("lane-rand");
    p.select(rand);
    let (tree, _) = p.h.buffer_tree(&bay);
    let hi = widget_keyed(&tree, &format!("graph-variable-reset-proc-1-{rand}-hi")).expect("hi");
    let inlet = match p.eval(&format!(
        "(let ((p (proc {rand}))) (named p.inlets \"hi\"))"
    )) {
        Value::Instance(id) => id,
        other => panic!("{other:?}"),
    };
    assert!(binds(&hi, "value", inlet, "value"), "{:?}", hi["value"]);
}

// ── the *processes* dock (docs/expr-process-spec.md §7) ──────────────────

/// The dock shows only while a node editor is open in the main tile, and
/// then it takes the right column in place of *step*/*track*; the browser
/// sidebar never changes. When it hides *step*/*track* come back; the
/// defcustom keeps it off.
#[test]
fn processes_buffer_replaces_step_and_track_while_a_node_editor_is_open() {
    let mut p = Panel::open();
    p.show_seq_tab();
    assert!(
        !p.showing(),
        "the instance's tab is not in the main tile yet"
    );
    assert!(!p.column_spec().contains("*processes*"));

    p.show_tab();
    assert!(p.showing(), "node 1's editor is open in the main tile");
    let spec = p.column_spec();
    assert!(
        spec.contains("*processes*") && !spec.contains("*step*") && !spec.contains("*track*"),
        "{spec}"
    );
    let sidebar = p.eval("(eseq.seq-layout/samples-sidebar-layout-spec)");
    assert!(
        !eseqlisp::vm::format_lisp_value(&sidebar).contains("*processes*"),
        "the sidebar is the browser's"
    );
    p.relayout();
    p.assert_column(true, None, "docked");
    assert!(p.visible().iter().any(|name| name == &p.bay));

    // All nodes: the dock goes (the observer re-lays out), *step*/*track*
    // are back.
    p.expand(-1);
    assert!(!p.showing(), "all nodes: no node editor");
    p.assert_column(false, None, "all nodes");

    p.expand(2);
    assert!(p.showing());
    p.assert_column(true, None, "docked again");
    p.show_seq_tab();
    assert!(!p.showing(), "another tab in the main tile");
    p.show_tab();
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    assert!(!p.showing(), "the auto-split setting is off");
    let spec = p.column_spec();
    assert!(
        !spec.contains("*processes*") && spec.contains("*step*") && spec.contains("*track*"),
        "{spec}"
    );
}

/// Clicking the main tile's Seq tab gives the right column back to
/// *step*/*track*; clicking the neural tab again brings the inspector and
/// code back with the same node, selection and code state, and the code
/// buffer is still an expr buffer. The browser stays put throughout.
#[test]
fn processes_buffer_follows_the_main_tab_strip() {
    let mut p = Panel::open();
    p.show_tab();
    let expr = p.add("expr");
    p.set_body(expr, "(* x 2)");
    p.select(expr);
    p.relayout();
    let name = "*expr node 1 · slot 1*";
    p.assert_column(true, Some(name), "docked");

    p.click_tab("Seq");
    assert!(
        p.visible().iter().any(|shown| shown == "*sequencer*"),
        "the Seq tab is showing"
    );
    assert!(!p.showing(), "no node editor in the main tile");
    p.assert_column(false, None, "Seq tab");

    p.click_tab("neural 1");
    assert!(
        p.visible().iter().any(|shown| shown == &p.bay),
        "the neural tab is showing"
    );
    p.assert_column(true, Some(name), "back on the neural tab");
    assert_eq!(
        p.eval("(eseq.sequencer/lane-patch-node-selected-id)"),
        number(expr as f64),
        "the same card is selected"
    );
    assert!(
        p.has(
            "*processes*",
            &format!("graph-variable-reset-proc-card-1-{expr}")
        ),
        "its inspector"
    );
    p.relayout();
    p.activate(name);
    assert_eq!(
        p.eval("(current-buffer-mode)"),
        s("eseq.expr-buffer/expr-mode")
    );
    assert!(p.offers_beat(), "the code tile still completes $beat");
}

/// Re-evaluating processes-buffer.lisp or the kind's variable-reset.lisp
/// (to tweak styling) keeps the dock: the inspector registry, the selection
/// and the code tile survive, and the edited source shows at once, both in
/// the dock's own panel and in the kind's renderer (called by name).
#[test]
fn processes_buffer_survives_re_evaluating_its_source_and_the_kinds() {
    let mut p = Panel::open();
    p.show_tab();
    let expr = p.add("expr");
    p.set_body(expr, "(* x 2)");
    p.select(expr);
    p.relayout();
    let name = "*expr node 1 · slot 1*";
    let card = format!("graph-variable-reset-proc-card-1-{expr}");
    assert!(p.has("*processes*", &card));

    let still_docked = |p: &mut Panel, when: &str| {
        assert!(p.showing(), "{when}");
        assert!(
            p.has("*processes*", &card),
            "{when}: the same card's inspector"
        );
        assert!(
            !p.has("*processes*", "processes-hint"),
            "{when}: no 'no inspector' hint"
        );
        assert_eq!(
            p.eval("(eseq.sequencer/lane-patch-node-selected-id)"),
            number(expr as f64),
            "{when}: selection kept"
        );
        assert!(
            p.column_spec().contains(name),
            "{when}: the code tile stays"
        );
        p.relayout();
        assert!(
            p.visible().iter().any(|shown| shown == name),
            "{when}: {:?}",
            p.visible()
        );
    };

    p.reload(
        "ui/processes-buffer.lisp",
        "\"processes-caption\"",
        "\"processes-caption-restyled\"",
    );
    still_docked(&mut p, "after re-evaluating processes-buffer.lisp");
    assert!(
        p.has("*processes*", "processes-caption-restyled"),
        "the edited panel renders"
    );

    p.reload(
        "packages/alez.neural/src/variable-reset.lisp",
        "(gvr-proc-card self g n p (gvr-proc-wired-inlets n) :fill \"dock\")",
        "(box :key \"gvr-inspector-restyled\" :width :fill (gvr-proc-card self g n p (gvr-proc-wired-inlets n) :fill \"dock\"))",
    );
    still_docked(&mut p, "after re-evaluating variable-reset.lisp");
    assert!(
        p.has("*processes*", "gvr-inspector-restyled"),
        "the edited renderer draws the dock"
    );
}

/// The selected card's inspector lives in the dock (not the bay): a hint
/// until a card is selected, then the card with its inlet pickers and out
/// mapping; each edits the node's processes. With the dock off, the bay
/// shows the same card inline.
#[test]
fn processes_buffer_inspects_the_selected_card_and_the_bay_does_not() {
    let mut p = Panel::open();
    p.show_tab();
    let rand = p.add("lane-rand");
    let other = p.add("neural-transpose");
    let card = |id: u64| format!("graph-variable-reset-proc-card-1-{id}");
    let bay = p.bay.clone();

    assert!(
        p.has("*processes*", "processes-hint"),
        "nothing selected: a hint"
    );
    assert!(!p.has("*processes*", &card(rand)));
    assert!(
        !p.has(&bay, &card(rand)),
        "the bay has no inline inspector while docked"
    );

    p.select(rand);
    assert!(
        p.has("*processes*", &card(rand)),
        "the selected card in the dock"
    );
    assert!(!p.has("*processes*", "processes-hint"));
    assert!(
        !p.has("*processes*", "processes-code-toggle"),
        "not an expr card: no code toggle"
    );
    assert!(!p.has(&bay, &card(rand)), "still not in the bay");
    assert!(
        !p.column_spec().contains("*expr"),
        "no code tile for a class card"
    );

    // Inlet picker (hi is a float number picker).
    p.change_in(
        "*processes*",
        &format!("graph-variable-reset-proc-1-{rand}-hi"),
        number(7.0),
    );
    assert!(close(p.inlet(rand, "hi"), 7.0));

    // Map the out port onto the payload: arm in the dock, bind on the row's chip.
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-map-1-{rand}-out"),
    );
    assert_eq!(
        p.eval("nn.map-slot"),
        number(rand as f64),
        "armed from the dock"
    );
    p.click_in(&bay, "graph-variable-reset-transpose-1-map-target");
    assert_eq!(
        p.mapped(rand, "out"),
        s("transpose"),
        "bound from the neuron row"
    );
    assert_eq!(p.eval("nn.map-slot"), number(-1.0), "disarmed");
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-unmap-1-{rand}-out"),
    );
    assert_eq!(p.mapped(rand, "out"), s(""), "unmapped");

    // No < > x on the card (cards reorder by dragging); on/off, and the
    // dock's red delete at its bottom right removes the card.
    assert!(!p.has(
        "*processes*",
        &format!("graph-variable-reset-proc-remove-1-{rand}")
    ));
    assert_eq!(p.classes()[0], "lane-rand");
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-enable-1-{rand}"),
    );
    assert_eq!(p.chain("enabled")[0], Value::Bool(false), "switched off");
    p.click_in("*processes*", "processes-delete");
    assert_eq!(p.classes(), vec!["neural-transpose".to_string()], "removed");
    assert!(
        !p.has("*processes*", "processes-delete"),
        "delete goes with the card"
    );
    assert!(
        p.has("*processes*", "processes-hint"),
        "the removed card's inspector goes"
    );

    // Dock off: the bay shows the same card inline (its first card when
    // nothing is selected).
    p.select(other);
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    assert!(p.has(&bay, &card(other)), "fallback: the inline inspector");
    assert!(
        p.has(&bay, &format!("graph-variable-reset-proc-remove-1-{other}")),
        "fallback: the inline card carries its own delete"
    );
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split true)");
    assert!(!p.has(&bay, &card(other)));
    // The sidebar hidden changes nothing: the dock is in the right column.
    p.eval("(set! eseq.seq-core-state/samples-sidebar-visible false)");
    assert!(!p.has(&bay, &card(other)), "sidebar hidden: still docked");
    // Another layout (the instrument patcher's): inline again.
    p.eval("(set! eseq.seq-step-tabs/seq-layout-mode :instrument-patcher)");
    assert!(
        p.has(&bay, &card(other)),
        "patcher layout: the inline inspector"
    );
}

/// An expr card shows its code under the inspector, splitting the dock
/// about half and half; hide code gives the inspector the whole dock and is
/// remembered; the card's edit button shows the code again and focuses it;
/// commits work there; the tile goes with the card.
#[test]
fn processes_buffer_shows_an_expr_cards_code_half_the_dock() {
    let mut p = Panel::open();
    p.show_tab();
    let expr = p.add("expr");
    p.set_body(expr, "(* x rate)");
    let rand = p.add("lane-rand");
    let name = "*expr node 1 · slot 1*";

    p.select(expr);
    let spec = p.column_spec();
    assert!(
        spec.contains(name),
        "selecting an expr card shows its code: {spec}"
    );
    let tiles = p.tiles();
    let height = |tiles: &[(String, f32)], buffer: &str| {
        (tiles.iter())
            .find(|(shown, _)| shown == buffer)
            .map(|(_, h)| *h)
            .unwrap_or_else(|| panic!("{buffer} not in {tiles:?}"))
    };
    let (inspector, code) = (
        height(&tiles, "*processes*") as f64,
        height(&tiles, name) as f64,
    );
    assert!(inspector > 0.0 && code > 0.0, "{tiles:?}");
    let share = code / (inspector + code);
    assert!(
        (0.4..=0.6).contains(&share),
        "code takes about half the dock: {share} {tiles:?}"
    );
    p.assert_column(true, Some(name), "an expr card selected");
    assert!(p.has("*processes*", "processes-code-toggle"));

    // hide code: the inspector takes the whole dock.
    p.click_in("*processes*", "processes-code-toggle");
    assert_eq!(
        p.eval("eseq.processes-buffer/code-visible"),
        Value::Bool(false)
    );
    assert!(!p.column_spec().contains("*expr"));
    p.relayout();
    assert!(
        !p.visible().iter().any(|shown| shown == name),
        "{:?}",
        p.visible()
    );
    p.select(rand);
    p.select(expr);
    assert!(
        !p.column_spec().contains("*expr"),
        "hidden stays hidden for the session"
    );

    // The card's edit button shows the code again and focuses it.
    p.relayout();
    let bay = p.bay.clone();
    p.click_in(&bay, &format!("lane-patch-expr-edit-{expr}"));
    assert_eq!(
        p.eval("eseq.processes-buffer/code-visible"),
        Value::Bool(true)
    );
    assert_eq!(
        p.h.editor.active_buffer().name,
        name,
        "the code tile has focus"
    );
    assert_eq!(p.h.editor.active_buffer().text(), "(* x rate)");
    let visible = p.visible();
    for required in [name, "*processes*", bay.as_str()] {
        assert!(
            visible.iter().any(|shown| shown == required),
            "{required} not in {visible:?}"
        );
    }

    // Commit from the code tile.
    p.h.editor.active_buffer_mut().set_text("(+ x depth)");
    p.h.editor.active_buffer_mut().dirty = true;
    p.eval("(save-buffer)");
    assert_eq!(p.source(expr), s("(+ x depth)"));

    // Selecting a class card: the inspector takes the whole dock.
    p.select(rand);
    assert!(!p.column_spec().contains("*expr"));
    assert!(
        !p.visible().iter().any(|shown| shown == name),
        "{:?}",
        p.visible()
    );
    p.select(expr);
    assert!(p.visible().iter().any(|shown| shown == name));

    // The card goes (the bay's remove).
    let ns = num(p.eval("(eseq.sequencer/lane-patch-node-namespace nn 1)")) as u64;
    p.eval(&format!(
        "(eseq.sequencer/lane-patch-remove-card {ns} {expr})"
    ));
    assert!(
        !p.column_spec().contains("*expr"),
        "no code tile for a removed card"
    );
    let visible = p.visible();
    assert!(
        !visible.iter().any(|shown| shown == name),
        "the code tile closed: {visible:?}"
    );
    assert!(
        visible.iter().any(|shown| shown == "*processes*"),
        "the dock stays: {visible:?}"
    );
}

/// Promote (docs/expr-process-spec.md §8) from the dock's inspector:
/// promote… opens the name modal in the dock, a commit writes the module
/// into a temp My processes package, loads it and rebinds the card in place
/// (the code tile goes: no longer an expr card); "as expr" turns it back
/// and a second promote updates the class. The rebind undoes back to the
/// expr card (the promoted file stays on disk).
#[test]
fn processes_buffer_promote_moves_the_card_into_my_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let package = tmp.path().join("packages/user.processes");
    let _package_guard =
        sequencer::lisp_host::set_my_processes_package_dir_override(Some(package.clone()));
    let mut p = Panel::open();
    p.show_tab();
    let bay = p.bay.clone();

    let id = num(p
        .eval("(eseq.expr-buffer/add-node-preset nn 1 (eseq.expr-buffer/preset-named \"bounce\"))"))
        as u64;
    p.select(id);
    assert!(
        p.column_spec().contains("*expr node 1"),
        "the bounce card's code shows"
    );
    assert!(
        !p.has(&bay, &format!("graph-variable-reset-proc-promote-1-{id}")),
        "not in the bay"
    );
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-promote-1-{id}"),
    );
    assert_eq!(
        p.eval("(eseq.expr-buffer/promote-open?)"),
        Value::Bool(true),
        "the name modal opens"
    );
    assert_eq!(p.eval("eseq.expr-buffer/promote-origin"), s("dock"));
    assert!(
        p.has("*processes*", "expr-promote-name"),
        "in the dock's mount"
    );

    // The live check refuses a taken name before anything is written.
    p.eval("(eseq.expr-buffer/set-promote-name \"delay\")");
    let problem = p.eval("(eseq.expr-buffer/promote-name-error)");
    assert!(
        matches!(&problem, Value::String(e) if e.contains("already exists")),
        "{problem:?}"
    );
    p.eval("(eseq.expr-buffer/commit-promote)");
    assert!(!package.exists(), "nothing written for a refused name");

    let expr_class = p.classes()[0].clone();
    p.eval("(eseq.expr-buffer/set-promote-name \"bouncy\")");
    p.eval("(eseq.expr-buffer/commit-promote)");
    assert_eq!(
        p.eval("(eseq.expr-buffer/promote-open?)"),
        Value::Bool(false),
        "closed on success"
    );
    let class = "user.processes.bouncy/bouncy";
    assert!(package.join("src/bouncy.lisp").is_file());
    assert_eq!(p.classes(), vec![class.to_string()], "rebound in place");
    assert!(close(p.inlet(id, "decay"), 0.8), "values kept");
    assert!(
        !p.column_spec().contains("*expr"),
        "no longer an expr card: no code tile"
    );
    assert!(!p.has(&bay, &format!("lane-patch-expr-edit-{id}")));
    assert!(
        p.has(
            "*processes*",
            &format!("graph-variable-reset-proc-as-expr-1-{id}")
        ),
        "a promoted card offers as expr"
    );
    let layout = p.layout_of(&bay);
    let add = measured(&layout, "graph-variable-reset-proc-add-1");
    assert!(
        items(&prop(add, "options")).contains(&s("bouncy")),
        "the add menu offers it"
    );

    // The rebind is one entry: undo is the expr card again; redo rebinds.
    p.undo();
    assert_eq!(p.classes(), vec![expr_class], "undo: the expr card again");
    assert!(
        package.join("src/bouncy.lisp").is_file(),
        "the promoted file stays"
    );
    p.redo();
    assert_eq!(
        p.classes(),
        vec![class.to_string()],
        "redo: the promoted class"
    );

    // As expr: back to an expr card holding the body, values kept.
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-as-expr-1-{id}"),
    );
    let classes = p.classes();
    assert!(classes[0].starts_with("expr#"), "{classes:?}");
    assert_eq!(p.source(id), s("(* k (pow decay $n))"));
    assert!(
        p.column_spec().contains("*expr node 1"),
        "an expr card again: its code shows"
    );

    // Tweak and promote again: the modal comes up on the name the card came
    // from, as an update of that class, and the commit replaces it.
    p.set_body(id, "(* k (pow decay $n) 2)");
    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-promote-1-{id}"),
    );
    assert_eq!(p.eval("eseq.expr-buffer/promote-name"), s("bouncy"));
    assert_eq!(p.eval("(eseq.expr-buffer/promote-name-error)"), Value::Nil);
    assert_eq!(
        p.eval("(eseq.expr-buffer/promote-update?)"),
        Value::Bool(true)
    );
    p.eval("(eseq.expr-buffer/commit-promote)");
    assert_eq!(
        p.eval("(eseq.expr-buffer/promote-open?)"),
        Value::Bool(false),
        "closed on success"
    );
    assert_eq!(
        p.classes(),
        vec![class.to_string()],
        "rebound to the updated class"
    );
    let text = std::fs::read_to_string(package.join("src/bouncy.lisp")).unwrap();
    assert!(text.contains(":expr \"(* k (pow decay $n) 2)\""), "{text}");
}

// ── undo (eseq-waa9.23) ──────────────────────────────────────────────────

/// The red delete in the dock undoes whole: undo brings the card back (same
/// id and place, inlet values, the wire into it, the dock's inspector);
/// redo deletes it again. An inlet drag is one step.
#[test]
fn processes_buffer_delete_undo_restores_the_card_and_redo_deletes_it() {
    let mut p = Panel::open();
    p.show_tab();
    let base = p.undo_len();
    let source = p.add("lane-rand");
    let target = p.add("lane-rand");
    p.wire(source, "wire", target, "hi");
    assert_eq!(p.undo_len(), base + 3, "two adds and a wire: one step each");
    let wire = p.wired_to(source, "wire");
    assert_eq!(wire, list_value([number(target as f64), s("hi")]));

    p.select(target);
    let card = format!("graph-variable-reset-proc-card-1-{target}");
    assert!(p.has("*processes*", &card));
    // An inlet picker drag: several values, one gesture, one step.
    let picker = format!("graph-variable-reset-proc-1-{target}-lo");
    let lo_before = p.inlet(target, "lo");
    p.h.gesture.pointer_down = true;
    for value in [0.2, 0.3, 0.4] {
        p.change_in("*processes*", &picker, number(value));
    }
    p.release();
    assert_eq!(p.undo_len(), base + 4, "the drag is one step");
    assert!(close(p.inlet(target, "lo"), 0.4));

    p.click_in("*processes*", "processes-delete");
    assert_eq!(p.undo_len(), base + 5, "the delete is one step");
    assert_eq!(p.ids(), vec![source], "deleted");
    assert_eq!(
        p.wired_to(source, "wire"),
        Value::Nil,
        "the wire into it went too"
    );
    assert!(p.has("*processes*", "processes-hint"), "the inspector went");

    p.undo();
    assert_eq!(p.ids(), vec![source, target], "back, same id, same place");
    assert!(close(p.inlet(target, "lo"), 0.4), "inlet values back");
    assert_eq!(p.wired_to(source, "wire"), wire, "the wire into it back");
    assert!(p.has("*processes*", &card), "the dock inspects it again");
    assert!(p.has("*processes*", "processes-delete"));

    p.redo();
    assert_eq!(p.ids(), vec![source], "redo deletes it again");
    assert!(p.has("*processes*", "processes-hint"));

    // Back through the drag (one step), then the wire and the adds.
    p.undo();
    p.undo();
    assert_eq!(
        p.inlet(target, "lo"),
        lo_before,
        "the whole drag undone at once"
    );
    assert_eq!(p.wired_to(source, "wire"), wire, "the wire is its own step");
    p.undo();
    assert_eq!(p.wired_to(source, "wire"), Value::Nil, "wire undone");
    p.undo();
    assert_eq!(p.ids(), vec![source], "second add undone");
    p.undo();
    assert!(p.ids().is_empty(), "first add undone");
    p.redo();
    assert_eq!(p.ids(), vec![source], "redo re-adds under the same id");
}

/// Clicks are one step each: enable, map; selection is none.
#[test]
fn processes_buffer_enable_and_map_undo_and_selection_is_not_a_step() {
    let mut p = Panel::open();
    p.show_tab();
    let rand = p.add("lane-rand");
    let before = p.undo_len();
    p.select(rand);
    assert_eq!(p.undo_len(), before, "selecting a card is not an edit");

    p.click_in(
        "*processes*",
        &format!("graph-variable-reset-proc-enable-1-{rand}"),
    );
    assert_eq!(p.chain("enabled")[0], Value::Bool(false));
    assert_eq!(p.undo_len(), before + 1);
    p.eval(&format!(
        "(alez.neural.variable-reset/gvr-map-arm nn {rand} \"out\")"
    ));
    let bay = p.bay.clone();
    p.click_in(&bay, "graph-variable-reset-vel-decay-1-map-target");
    assert_eq!(p.mapped(rand, "out"), s("velocity"));
    assert_eq!(p.undo_len(), before + 2);

    p.undo();
    assert_eq!(
        p.chain("enabled")[0],
        Value::Bool(false),
        "the map undone first"
    );
    assert_eq!(p.mapped(rand, "out"), s(""), "unmapped");
    p.undo();
    assert_eq!(p.chain("enabled")[0], Value::Bool(true), "switched back on");
    assert!(p.has(
        "*processes*",
        &format!("graph-variable-reset-proc-card-1-{rand}")
    ));
}

/// Drag gestures are keyed per inlet: two inlets dragged back to back are
/// two steps even without a release between them, and a click (the delete)
/// during an open drag commits the drag as its own step first. A new edit
/// after an undo clears redo; an expr commit undoes to the previous body.
#[test]
fn processes_buffer_inlet_gestures_are_per_inlet_and_a_click_commits_the_open_drag() {
    let mut p = Panel::open();
    p.show_tab();
    let target = p.add("lane-rand");
    p.select(target);
    let base = p.undo_len();
    let lo = format!("graph-variable-reset-proc-1-{target}-lo");
    let hi = format!("graph-variable-reset-proc-1-{target}-hi");
    let (lo_before, hi_before) = (p.inlet(target, "lo"), p.inlet(target, "hi"));
    p.h.gesture.pointer_down = true;
    for value in [0.2, 0.3] {
        p.change_in("*processes*", &lo, number(value));
    }
    for value in [0.7, 0.8] {
        p.change_in("*processes*", &hi, number(value));
    }
    p.h.gesture.pointer_down = false;
    // Still inside the hi drag: the delete click commits it first.
    p.click_in("*processes*", "processes-delete");
    assert_eq!(p.undo_len(), base + 3, "lo drag, hi drag, delete");
    assert!(p.ids().is_empty());

    p.undo();
    assert_eq!(p.ids(), vec![target]);
    assert!(close(p.inlet(target, "hi"), 0.8));
    p.undo();
    assert_eq!(
        p.inlet(target, "hi"),
        hi_before,
        "the hi drag is its own step"
    );
    assert!(close(p.inlet(target, "lo"), 0.3));
    p.undo();
    assert_eq!(
        p.inlet(target, "lo"),
        lo_before,
        "the lo drag is its own step"
    );

    // A new edit drops the redo branch.
    p.change_in("*processes*", &lo, number(0.5));
    p.release();
    assert_eq!(p.undo_len(), base + 1);
    assert!(p.h.app.history.next_redo_patch().is_none(), "redo cleared");

    // An expr commit undoes to the previous body.
    let expr = p.add("expr");
    p.set_body(expr, "(* x 2)");
    let first = p.classes()[1].clone();
    p.set_body(expr, "(+ x 3)");
    p.undo();
    assert_eq!(p.source(expr), s("(* x 2)"));
    assert_eq!(p.classes()[1], first, "the previous hashed class");
    p.redo();
    assert_eq!(p.source(expr), s("(+ x 3)"));
}

// ── expr cards (docs/expr-process-spec.md §2, §3) ───────────────────────

/// The edit button opens a text buffer named for the slot with the stored
/// body; C-c C-c commits it (inlets reconcile, the buffer is saved, removed
/// inlets are toasted); a failed commit leaves the card and the buffer
/// modified, highlights the span and lights the card's error dot; reopening
/// focuses the same buffer; a commit after the card is gone does nothing.
#[test]
fn expr_card_edit_buffer_commits_reports_errors_and_survives_removal() {
    let mut p = Panel::open();
    let rand = p.add("lane-rand");
    let expr = p.add("expr");
    p.set_body(expr, "(sin (* x rate))");
    let bay = p.bay.clone();

    // The uniform card: edit button, preview on the out-port row, no error.
    let layout = p.layout_of(&bay);
    measured(&layout, &format!("lane-patch-expr-preview-{expr}"));
    let dot = keyed(&layout, &format!("lane-patch-expr-error-{expr}")).expect("error dot");
    assert_eq!(prop(dot, "active"), number(0.0));
    assert!(
        keyed(&layout, &format!("lane-patch-expr-edit-{rand}")).is_none(),
        "only expr cards get an edit button"
    );
    let card = |id: u64| {
        keyed(&layout, &format!("lane-patch-col-{id}"))
            .expect("card")
            .rect
    };
    let (rand_card, expr_card) = (card(rand), card(expr));
    assert_eq!(
        (rand_card.width, rand_card.height),
        (expr_card.width, expr_card.height),
        "uniform card size"
    );

    // Inspector: the expr inlets are unbounded relative pickers.
    p.select(expr);
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    let layout = p.layout_of(&bay);
    let picker = measured(&layout, &format!("graph-variable-reset-proc-1-{expr}-rate"));
    assert_eq!(picker.widget_type, "number-picker");
    assert_eq!(prop(picker, "drag"), Value::Keyword("relative".to_string()));
    assert_eq!(
        prop(picker, "min"),
        Value::Nil,
        "no range: the body carries none"
    );

    // Edit opens the buffer with the body, in the expr mode.
    p.click_in(&bay, &format!("lane-patch-expr-edit-{expr}"));
    let name = "*expr node 1 · slot 2*";
    assert_eq!(p.h.editor.active_buffer().name, name);
    assert_eq!(p.h.editor.active_buffer().text(), "(sin (* x rate))");
    assert_eq!(
        p.eval("(current-buffer-mode)"),
        s("eseq.expr-buffer/expr-mode")
    );
    assert!(!p.h.editor.active_buffer().dirty);

    // Completion offers the $ context variables, with their docs.
    p.type_body("(+ $p");
    p.h.editor.active_buffer_mut().cursor = (0, 5);
    p.h.editor
        .handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    let completion = p.h.editor.completion_state().expect("completion popup");
    let phase = (completion.items.iter())
        .find(|item| item.label == "$phase")
        .expect("$phase offered");
    assert!(
        phase.docs.as_deref().is_some_and(|doc| doc.contains("bar")),
        "{phase:?}"
    );
    p.h.editor
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // A good commit: the chain takes the body, rate goes (toasted), saved.
    p.type_body("(+ x depth)");
    p.commit_chord();
    assert_eq!(p.source(expr), s("(+ x depth)"));
    assert!(
        !p.h.editor.active_buffer().dirty,
        "a good commit saves the buffer"
    );
    let toast = p.h.editor.toast().expect("removed-inlet toast");
    assert!(
        toast.message.contains("removed inlets: rate"),
        "{}",
        toast.message
    );

    // A failed commit: the card keeps its body, the buffer stays modified,
    // the span is highlighted, the card lights its error dot.
    p.type_body("(+ x\n   (sinn depth))");
    p.commit_chord();
    assert_eq!(p.source(expr), s("(+ x depth)"));
    assert!(
        p.h.editor.active_buffer().dirty,
        "a failed commit keeps the edit"
    );
    let styles = &p.h.editor.active_buffer().text_styles;
    assert_eq!(styles.len(), 1, "the reported span is highlighted");
    assert_eq!(
        styles[0].line,
        Some(1),
        "on the line of the unknown function"
    );
    assert_eq!(
        p.h.editor.active_buffer().cursor.0,
        1,
        "the cursor goes to that line"
    );
    let toast = p.h.editor.toast().expect("error toast");
    assert_eq!(toast.kind, eseqlisp::ToastKind::Error);
    assert!(toast.message.contains("sinn"), "{}", toast.message);
    let layout = p.layout_of(&bay);
    let dot = keyed(&layout, &format!("lane-patch-expr-error-{expr}")).unwrap();
    assert_eq!(
        prop(dot, "active"),
        number(1.0),
        "failed commit lights the dot"
    );
    let message = keyed(
        &layout,
        &format!("graph-variable-reset-proc-expr-error-{}-{expr}", 1),
    );
    assert!(message.is_some(), "the inspector says why");

    // Reopening focuses the same buffer, with the unsaved edit intact.
    let buffers = p.h.editor.buffers.len();
    p.click_in(&bay, &format!("lane-patch-expr-edit-{expr}"));
    assert_eq!(p.h.editor.active_buffer().name, name);
    assert_eq!(p.h.editor.buffers.len(), buffers);
    assert!(p.h.editor.active_buffer().text().contains("sinn"));

    // File > Save (the Cmd+S menu item) commits an expr buffer instead of
    // saving the project.
    p.type_body("(* x depth)");
    p.h.editor.drain_host_commands();
    p.h.eval("(eseq.transport/file-menu-save)");
    let saved_project = (p.h.editor.drain_host_commands().iter()).any(|command| {
        matches!(command, HostCommand::Custom { name, .. } if name == "project-save-open")
    });
    p.render();
    assert_eq!(p.source(expr), s("(* x depth)"));
    assert!(!p.h.editor.active_buffer().dirty);
    assert!(!saved_project);
    let layout = p.layout_of(&bay);
    let dot = keyed(&layout, &format!("lane-patch-expr-error-{expr}")).unwrap();
    assert_eq!(
        prop(dot, "active"),
        number(0.0),
        "a good commit clears the dot"
    );

    // The card goes away: a later commit changes nothing and says so.
    p.remove(expr);
    p.activate(name);
    p.type_body("(* x 2)");
    p.commit_chord();
    assert!(p.h.editor.active_buffer().dirty);
    let toast = p.h.editor.toast().expect("gone toast");
    assert!(toast.message.contains("gone"), "{}", toast.message);
    assert_eq!(
        p.eval(&format!("(graph-node-process-slot? nn 1 {expr})")),
        Value::Bool(false)
    );
}

/// Port overflow (§3.2 (a)): past three in ports the card shows two plus a
/// `+n` badge, keeps any port a cable lands on, and the inspector still
/// lists every inlet.
#[test]
fn expr_card_overflow_badge_keeps_wired_in_ports() {
    let mut p = Panel::open();
    let rand = p.add("lane-rand");
    let expr = p.add("expr");
    p.set_body(expr, "(+ a (* b c) d e)");
    let bay = p.bay.clone();
    let in_port = |layout: &LayoutNode, name: &str| {
        keyed(layout, &format!("lane-patch-in-port-{expr}-{name}")).is_some()
    };
    let badge = format!("lane-patch-in-overflow-{expr}");
    let layout = p.layout_of(&bay);
    assert!(in_port(&layout, "a") && in_port(&layout, "b"));
    assert!(!in_port(&layout, "c") && !in_port(&layout, "e"));
    measured(&layout, &badge);

    // A cable onto `e` keeps its port on the card.
    p.wire(rand, "wire", expr, "e");
    let layout = p.layout_of(&bay);
    assert!(
        in_port(&layout, "a") && in_port(&layout, "e"),
        "a wired port always shows"
    );
    assert!(!in_port(&layout, "b"));
    measured(&layout, &badge);

    // Every inlet is settable in the inspector.
    p.select(expr);
    p.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    let layout = p.layout_of(&bay);
    for name in ["b", "c", "d"] {
        measured(
            &layout,
            &format!("graph-variable-reset-proc-1-{expr}-{name}"),
        );
    }
    let wired = keyed(&layout, &format!("graph-variable-reset-proc-1-{expr}-e"));
    assert!(wired.is_none(), "a wired inlet shows `wired`, no picker");
}

/// Expr presets (spec §6.1): the node bay's add menu lists the library
/// classes, then an "expr presets" heading over one row per entry of the
/// Lisp preset table. Picking a row through the menu's own `:on-change`
/// adds an expr card with that body committed and its inlets at their
/// starting values; every preset compiles.
#[test]
fn expr_card_presets_in_the_node_add_menu_add_committed_cards() {
    let mut p = Panel::open();
    let bay = p.bay.clone();
    let strings = |value: Value| -> Vec<String> {
        items(&value)
            .into_iter()
            .map(|item| match item {
                Value::String(text) => text,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    let layout = p.layout_of(&bay);
    let add = measured(&layout, "graph-variable-reset-proc-add-1").clone();
    assert_eq!(add.widget_type, "menu-button");
    let rows = strings(prop(&add, "options"));
    let headers = items(&prop(&add, "headers"));
    assert_eq!(headers.len(), 1);
    let header = num(headers[0].clone()) as usize;
    assert_eq!(rows[header], "expr presets");
    let (classes, presets) = (&rows[..header], &rows[header + 1..]);
    assert!(classes.iter().any(|row| row == "expr"), "{classes:?}");
    assert_eq!(
        presets,
        &[
            "× k",
            "+ k",
            "sin",
            "quant",
            "fold",
            "wrap",
            "scale 0..1 → lo..hi",
            "bounce",
            "lfsr"
        ],
        "the preset table, in order"
    );
    for preset in presets {
        assert!(
            !classes.contains(preset),
            "preset label {preset} collides with a class label"
        );
    }

    let on_change = prop(&add, "on-change");
    let expected: [(&str, &str, &[(&str, f64)]); 9] = [
        ("× k", "(* in k)", &[("k", 1.0)]),
        ("+ k", "(+ in k)", &[("k", 1.0)]),
        ("sin", "(sin in)", &[]),
        ("quant", "(quant in step)", &[("step", 1.0)]),
        ("fold", "(fold in lo hi)", &[("lo", 0.0), ("hi", 1.0)]),
        ("wrap", "(wrap in lo hi)", &[("lo", 0.0), ("hi", 1.0)]),
        (
            "scale 0..1 → lo..hi",
            "(scale in 0 1 lo hi)",
            &[("lo", 0.0), ("hi", 1.0)],
        ),
        (
            "bounce",
            "(* k (pow decay $n))",
            &[("k", 1.0), ("decay", 0.8)],
        ),
        (
            "lfsr",
            "(state s 0xACE1)",
            &[("taps", 46080.0), ("grain", 1.0)],
        ),
    ];
    for (index, (label, source, inlets)) in expected.iter().enumerate() {
        (p.h.editor.runtime_mut())
            .invoke(on_change.clone(), vec![s(label)])
            .unwrap_or_else(|e| panic!("pick {label}: {e:?}"));
        p.render();
        let field = |p: &mut Panel, field: &str| {
            p.eval(&format!(
                "(let ((n (node 1)) (p (nth n.processes {index}))) p.{field})"
            ))
        };
        assert_eq!(
            field(&mut p, "expr"),
            Value::Bool(true),
            "{label}: an expr card"
        );
        let Value::String(body) = field(&mut p, "expr-source") else {
            panic!("{label}: no body")
        };
        assert!(body.starts_with(source), "{label}: {body}");
        assert_eq!(field(&mut p, "compile-error"), s(""), "{label}: compiles");
        let Value::String(class) = field(&mut p, "class-name") else {
            panic!("{label}: class")
        };
        assert!(
            sequencer::process::is_expr_process_class(&class),
            "{label}: compiled class {class}"
        );
        assert_eq!(field(&mut p, "name"), s("expr"));
        let id = num(field(&mut p, "proc-id")) as u64;
        for (name, value) in *inlets {
            assert!(close(p.inlet(id, name), *value), "{label}: inlet {name}");
        }
    }
    // The LFSR preset is the §9 body, multi-line, with its delay write.
    let Value::String(lfsr) = p.eval("(let ((n (node 1)) (p (nth n.processes 8))) p.expr-source)")
    else {
        panic!("lfsr body")
    };
    assert!(
        lfsr.contains('\n') && lfsr.contains("(delay! (* grain (bit-and s 7)))"),
        "{lfsr}"
    );

    // The table is extensible: add-preset appends a row the menu shows.
    p.eval("(eseq.expr-buffer/add-preset \"half\" \"(* in 0.5)\" (list))");
    let layout = p.layout_of(&bay);
    let add = measured(&layout, "graph-variable-reset-proc-add-1");
    assert_eq!(
        strings(prop(add, "options")).last().map(String::as_str),
        Some("half")
    );
}

impl Panel {
    /// Assert the active buffer is still an expr edit buffer: named mode,
    /// `$` completion, and a commit of `body` to card `id`.
    fn assert_still_expr_buffer(&mut self, id: u64, body: &str, when: &str) {
        assert_eq!(
            self.eval("(current-buffer-mode)"),
            s("eseq.expr-buffer/expr-mode"),
            "{when}: the buffer keeps the expr mode"
        );
        assert!(
            self.offers_beat(),
            "{when}: $ completion still offers $beat"
        );
        self.type_body(body);
        self.commit_chord();
        assert_eq!(self.source(id), s(body), "{when}: C-c C-c commits");
        assert!(
            !self.h.editor.active_buffer().dirty,
            "{when}: the commit saves the buffer"
        );
    }
}

/// The edit buffer keeps its mode and `$` completion across commits (C-c
/// C-c and File > Save), and across a Lisp hot reload of the UI modules.
#[test]
fn expr_card_edit_buffer_keeps_completion_and_mode_across_commits_and_reloads() {
    let mut p = Panel::open();
    let expr = p.add("expr");
    p.set_body(expr, "(* x 2)");
    let bay = p.bay.clone();
    p.click_in(&bay, &format!("lane-patch-expr-edit-{expr}"));
    let name = p.h.editor.active_buffer().name.clone();
    assert!(p.offers_beat(), "fresh buffer offers $beat");

    p.assert_still_expr_buffer(expr, "(+ x 1)", "after opening");
    p.assert_still_expr_buffer(expr, "(+ x 2)", "after one commit");
    p.type_body("(+ x 3)");
    p.eval("(eseq.transport/file-menu-save)");
    assert_eq!(p.source(expr), s("(+ x 3)"));
    p.activate(&name);
    p.assert_still_expr_buffer(expr, "(+ x 4)", "after File > Save");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/ui");
    for file in ["expr-buffer.lisp", "processes-buffer.lisp"] {
        let path = root.join(file).canonicalize().expect(file);
        let editor = &mut p.h.editor;
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor
            .runtime_mut()
            .reload_paths_transactional(vec![path], overlays);
        assert!(report.success, "{file}: {:?}", report.diagnostics);
        editor.process_lisp_reload_report(report);
        p.render();
        p.activate(&name);
        let body = format!("(+ x {})", file.len());
        p.assert_still_expr_buffer(expr, &body, &format!("after reloading {file}"));
    }
}

/// Docked (eseq-waa9.22): the code tile's buffer keeps its mode and `$`
/// completion across a commit and across hide code / show code.
#[test]
fn expr_card_docked_code_keeps_completion_and_mode_across_commit_and_hide_show() {
    let mut p = Panel::open();
    p.show_tab();
    let expr = p.add("expr");
    p.set_body(expr, "(* x 2)");
    p.select(expr);
    p.relayout();
    let name = "*expr node 1 · slot 1*";
    p.activate(name);
    p.assert_still_expr_buffer(expr, "(+ x 1)", "docked, first open");
    p.assert_still_expr_buffer(expr, "(+ x 2)", "docked, after one commit");

    for round in 0..2 {
        p.eval("(eseq.processes-buffer/toggle-code)");
        p.relayout();
        assert_eq!(
            p.eval("eseq.processes-buffer/code-visible"),
            Value::Bool(false)
        );
        p.eval("(eseq.processes-buffer/toggle-code)");
        p.relayout();
        assert_eq!(
            p.eval("eseq.processes-buffer/code-visible"),
            Value::Bool(true)
        );
        p.activate(name);
        let body = format!("(+ x {})", 10 + round);
        p.assert_still_expr_buffer(expr, &body, &format!("docked, hide/show {round}"));
    }
}
