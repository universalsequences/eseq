//! The *processes* dock (docs/expr-process-spec.md §7; beads eseq-waa9.16,
//! eseq-waa9.22): the inspector for the card selected in an open node
//! editor, over an expr card's code, driven through the real Lisp (showing?,
//! the right column's layout spec, the tile tree) and real clicks.
use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

struct Dock {
    state: Arc<SequencerState>,
    editor: Editor,
    graph: u64,
    bay: String,
}

impl Dock {
    /// The package panel with node 1 expanded (graph_panel_editor) and the
    /// builtin library published; the instance's tab is not in the main
    /// tile until `show_tab`.
    fn open() -> Self {
        let (state, editor, graph) = super::graph_visualization_ui_tests::graph_panel_editor();
        let mut authoring = Runtime::new();
        sequencer::lisp_host::register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        authoring
            .eval_str(&sequencer::lisp_host::load_process_library_source())
            .expect("builtin process library");
        Self::with_editor(state, editor, graph)
    }

    fn with_editor(state: Arc<SequencerState>, mut editor: Editor, graph: u64) -> Self {
        editor.runtime_mut().set_reactive("SEQ", "process-run-errors", test_list(vec![]));
        let bay = sequencer::lisp_host::instance_view_buffer_name("neural", "neural 1");
        let mut dock = Self { state, editor, graph, bay };
        dock.eval(&format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {graph}) 1)"));
        dock
    }

    fn show_tab(&mut self) {
        let (graph, bay) = (self.graph, self.bay.clone());
        self.eval(&format!(
            "(do (eseq.seq-step-tabs/seq-register-instance-tab {graph} \"neural 1\" \"{bay}\")
                 (set! eseq.seq-step-tabs/step-panel-buffer \"{bay}\"))"
        ));
    }

    fn eval(&mut self, source: &str) -> Option<Value> {
        let value = self.editor.runtime_mut().eval_str(source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
        value
    }

    fn showing(&mut self) -> bool {
        self.eval("(eseq.processes-buffer/showing?)") == Some(Value::Bool(true))
    }

    fn add(&mut self, class: &str) -> u64 {
        match self.eval(&format!(
            "(let ((id (graph-node-process-add {} 1 \"{class}\"))) (do (eseq.sequencer/lane-patch-node-touch) id))",
            self.graph
        )) {
            Some(Value::Number(id)) => id as u64,
            other => panic!("add {class}: {other:?}"),
        }
    }

    /// Select card `id` in node 1's bay, as a click on the card does.
    fn select(&mut self, id: u64) {
        let graph = self.graph;
        self.eval(&format!(
            "(eseq.sequencer/lane-patch-select-lane (eseq.sequencer/lane-patch-node-namespace (instance-ref {graph}) 1) {id})"
        ));
    }

    fn activate(&mut self, name: &str) {
        let id = self.editor.buffers.iter().find(|item| item.name == name).expect(name).id;
        self.editor.set_active_buffer(id);
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    /// `name` alone in the window (replaces the tile tree).
    fn layout_of(&mut self, name: &str) -> Arc<eseqlisp::layout::LayoutNode> {
        let name = name.to_string();
        self.eval(&format!("(set-layout (list :buf \"{name}\" :hide-status true))"));
        self.activate(&name);
        self.editor.widget_layout().expect("layout")
    }

    fn has(&mut self, buffer: &str, key: &str) -> bool {
        let layout = self.layout_of(buffer);
        find_layout_node_by_stable_key_suffix(&layout, key).is_some()
    }

    fn click_in(&mut self, buffer: &str, key: &str) {
        let layout = self.layout_of(buffer);
        let node = find_layout_node_by_stable_key_suffix(&layout, key).expect(key).clone();
        assert_finite_nonzero_rect(&node, key);
        let (col, row) = (node.rect.col + node.rect.width * 0.5, node.rect.row + node.rect.height * 0.5);
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            self.editor.handle_mouse_precise(
                MouseEvent { kind, column: col.floor() as u16, row: row.floor() as u16, modifiers: KeyModifiers::NONE },
                0, 0, 240, 100, col, row,
            );
        }
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    /// Call widget `key`'s `:on-change` in `buffer` with `value`.
    fn change_in(&mut self, buffer: &str, key: &str, value: Value) {
        let layout = self.layout_of(buffer);
        let node = find_layout_node_by_stable_key_suffix(&layout, key).expect(key).clone();
        assert_finite_nonzero_rect(&node, key);
        let on_change = node.props.get("on-change").cloned().expect("on-change");
        self.editor.runtime_mut().invoke(on_change, vec![value]).expect("on-change");
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    /// The real layout (sidebar, main, right column) as the app lays it out now.
    fn relayout(&mut self) {
        self.eval("(eseq.seq-layout/refresh-current-layout)");
    }

    /// Buffer names of the tiles on screen, with each leaf's height in a
    /// 240x100 window.
    fn tiles(&self) -> Vec<(String, f32)> {
        let root = &self.editor.tile_root;
        let area = eseqlisp::layout::Rect { row: 0.0, col: 0.0, width: 240.0, height: 100.0 };
        root.compute_rects(area, 0.0, 1.0, 1.0)
            .into_iter()
            .filter_map(|(id, rect)| root.find_leaf(id).map(|leaf| (leaf, rect)))
            .map(|(leaf, rect)| (self.editor.buffers[leaf.buffer_idx].name.clone(), rect.height))
            .collect()
    }

    fn visible(&self) -> Vec<String> {
        self.tiles().into_iter().map(|(name, _)| name).collect()
    }

    fn chain(&mut self, field: &str) -> Vec<Value> {
        let Some(Value::List(slots)) = self.eval(&format!(
            "(map (lambda (s) (get s :{field})) (graph-node-process-chain {} 1))",
            self.graph
        )) else {
            panic!("chain")
        };
        slots.iter().map(|slot| slot.borrow().clone()).collect()
    }

    fn chain_classes(&mut self) -> Vec<String> {
        self.chain("class")
            .into_iter()
            .map(|class| match class {
                Value::String(class) => class,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// Click the main tile's buffer tab labelled `label` with the mouse, as
    /// the user does (the eseqlisp tab strip, not a Lisp tab command).
    fn click_tab(&mut self, label: &str) {
        self.editor.update_tile_rects(240, 100);
        let (col, row) = self
            .editor
            .tile_rects()
            .iter()
            .find_map(|(id, rect)| {
                let leaf = self.editor.tile_root.find_leaf(*id)?;
                let index = leaf.tabs.iter().position(|tab| tab.label == label)?;
                let tab = eseqlisp::tile::tile_tab_layouts(*rect, &leaf.tabs, leaf.selected_tab)
                    .into_iter()
                    .find(|tab| tab.index == index)?;
                Some((tab.rect.col + tab.rect.width * 0.5, tab.rect.row + tab.rect.height * 0.5))
            })
            .unwrap_or_else(|| panic!("no tab {label} on screen"));
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            self.editor.handle_tiled_mouse_precise(
                MouseEvent { kind, column: col.floor() as u16, row: row.floor() as u16, modifiers: KeyModifiers::NONE },
                col,
                row,
                0,
            );
            self.editor.runtime_mut().run_reactive_cycle();
            self.editor.refresh_runtime_side_effects();
        }
    }

    /// Type `(+ $be` into the active buffer: whether completion offers
    /// `$beat`. Restores the text.
    fn offers_beat(&mut self) -> bool {
        use crossterm::event::{KeyCode, KeyEvent};
        let before = self.editor.active_buffer().text();
        let dirty = self.editor.active_buffer().dirty;
        self.editor.active_buffer_mut().set_text("(+ $b");
        self.editor.active_buffer_mut().cursor = (0, 5);
        self.editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        let offered = self
            .editor
            .completion_state()
            .is_some_and(|completion| completion.items.iter().any(|item| item.label == "$beat"));
        self.editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let buffer = self.editor.active_buffer_mut();
        buffer.set_text(&before);
        buffer.dirty = dirty;
        offered
    }

    /// Re-evaluate `file` (relative to content/) through the hot-reload
    /// pipeline, as saving or eval-buffer does, with `from` replaced by `to`
    /// in its source (an in-memory styling edit).
    fn reload(&mut self, file: &str, from: &str, to: &str) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let path = root.join(file).canonicalize().expect(file);
        let text = std::fs::read_to_string(&path).expect(file);
        assert_eq!(text.matches(from).count(), 1, "{file}: patch anchor {from:?}");
        let mut overlays = self.editor.snapshot_file_backed_sources();
        overlays.retain(|overlay| overlay.path != path);
        overlays.push(eseqlisp::SourceOverlay { path: path.clone(), text: text.replacen(from, to, 1), dirty: true, revision: 1 });
        let report = self.editor.runtime_mut().reload_paths_transactional(vec![path], overlays);
        assert!(report.success, "{file}: {:?}", report.diagnostics);
        self.editor.process_lisp_reload_report(report);
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    fn inlet(&mut self, id: u64, name: &str) -> Option<Value> {
        self.eval(&format!(
            "(get (get (reduce (lambda (acc s) (if (= (get s :instance-id) {id}) s acc)) nil (graph-node-process-chain {} 1)) :inlets) :{name})",
            self.graph
        ))
    }
}

/// The main area's layout spec: the main tile and the right column.
fn column_spec(dock: &mut Dock) -> String {
    let value = dock.eval("(eseq.seq-layout/step-and-track-panel-layout-spec)").expect("spec");
    eseqlisp::vm::format_lisp_value(&value)
}

/// The tiles on screen: the browser always; in the right column either the
/// dock (with `code` under it, or not) or *step* over *track*.
fn assert_column(dock: &Dock, docked: bool, code: Option<&str>, when: &str) {
    let visible = dock.visible();
    let shows = |name: &str| visible.iter().any(|shown| shown == name);
    assert!(shows("*samples*"), "{when}: the browser never moves: {visible:?}");
    assert_eq!(shows("*processes*"), docked, "{when}: {visible:?}");
    assert_eq!(shows("*step*"), !docked, "{when}: {visible:?}");
    assert_eq!(shows("*track*"), !docked, "{when}: {visible:?}");
    let code_tiles = visible.iter().filter(|shown| shown.starts_with("*expr")).count();
    match code {
        Some(name) => assert!(shows(name) && code_tiles == 1, "{when}: code tile {name}: {visible:?}"),
        None => assert_eq!(code_tiles, 0, "{when}: no code tile: {visible:?}"),
    }
}

/// The dock shows only while a node editor is open in the main tile, and
/// then it takes the right column in place of *step*/*track*; the browser
/// sidebar never changes. When it hides *step*/*track* come back; the
/// defcustom keeps it off.
#[test]
fn processes_buffer_replaces_step_and_track_while_a_node_editor_is_open() {
    let mut dock = Dock::open();
    assert!(!dock.showing(), "the instance's tab is not in the main tile yet");
    assert!(!column_spec(&mut dock).contains("*processes*"));

    dock.show_tab();
    assert!(dock.showing(), "node 1's editor is open in the main tile");
    let spec = column_spec(&mut dock);
    assert!(spec.contains("*processes*") && !spec.contains("*step*") && !spec.contains("*track*"), "{spec}");
    let sidebar = dock.eval("(eseq.seq-layout/samples-sidebar-layout-spec)").expect("sidebar");
    assert!(!eseqlisp::vm::format_lisp_value(&sidebar).contains("*processes*"), "the sidebar is the browser's");
    dock.relayout();
    assert_column(&dock, true, None, "docked");
    assert!(dock.visible().iter().any(|name| name == &dock.bay));

    // All nodes: the dock goes (the observer re-lays out), *step*/*track* are back.
    let graph = dock.graph;
    dock.eval(&format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {graph}) -1)"));
    assert!(!dock.showing(), "all nodes: no node editor");
    assert_column(&dock, false, None, "all nodes");

    dock.eval(&format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {graph}) 2)"));
    assert!(dock.showing());
    assert_column(&dock, true, None, "docked again");
    dock.eval("(set! eseq.seq-step-tabs/step-panel-buffer \"*sequencer*\")");
    assert!(!dock.showing(), "another tab in the main tile");
    dock.show_tab();
    dock.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    assert!(!dock.showing(), "the auto-split setting is off");
    let spec = column_spec(&mut dock);
    assert!(!spec.contains("*processes*") && spec.contains("*step*") && spec.contains("*track*"), "{spec}");
}

/// Clicking the main tile's Seq tab (a real mouse click on the tab strip)
/// gives the right column back to *step*/*track*; clicking the neural tab
/// again brings the inspector and code back with the same node, selection
/// and code state, and the code buffer is still an expr buffer. The browser
/// stays put throughout.
#[test]
fn processes_buffer_follows_the_main_tab_strip() {
    let mut dock = Dock::open();
    dock.show_tab();
    let graph = dock.graph;
    let expr = dock.add("expr");
    dock.eval(&format!("(graph-node-process-expr-set {graph} 1 {expr} \"(* x 2)\")"));
    dock.select(expr);
    dock.relayout();
    let name = "*expr node 1 · slot 1*";
    assert_column(&dock, true, Some(name), "docked");

    dock.click_tab("Seq");
    assert!(dock.visible().iter().any(|shown| shown == "*sequencer*"), "the Seq tab is showing");
    assert!(!dock.showing(), "no node editor in the main tile");
    assert_column(&dock, false, None, "Seq tab");

    dock.click_tab("neural 1");
    assert!(dock.visible().iter().any(|shown| shown == &dock.bay), "the neural tab is showing");
    assert_column(&dock, true, Some(name), "back on the neural tab");
    assert_eq!(
        dock.eval("(eseq.sequencer/lane-patch-node-selected-id)"),
        Some(Value::Number(expr as f64)),
        "the same card is selected"
    );
    assert!(dock.has("*processes*", &format!("graph-variable-reset-proc-card-1-{expr}")), "its inspector");
    dock.relayout();
    dock.activate(name);
    assert_eq!(
        dock.eval("(current-buffer-mode)"),
        Some(Value::String("eseq.expr-buffer/expr-mode".to_string()))
    );
    assert!(dock.offers_beat(), "the code tile still completes $beat");
}

/// Re-evaluating processes-buffer.lisp or the kind's variable-reset.lisp
/// (to tweak styling) keeps the dock: the inspector registry, the selection
/// and the code tile survive, and the edited source shows at once, both
/// in the dock's own panel and in the kind's renderer (called by name).
#[test]
fn processes_buffer_survives_re_evaluating_its_source_and_the_kinds() {
    let mut dock = Dock::open();
    dock.show_tab();
    let graph = dock.graph;
    let expr = dock.add("expr");
    dock.eval(&format!("(graph-node-process-expr-set {graph} 1 {expr} \"(* x 2)\")"));
    dock.select(expr);
    dock.relayout();
    let name = "*expr node 1 · slot 1*";
    let card = format!("graph-variable-reset-proc-card-1-{expr}");
    assert!(dock.has("*processes*", &card));

    let still_docked = |dock: &mut Dock, when: &str| {
        assert!(dock.showing(), "{when}");
        assert!(dock.has("*processes*", &card), "{when}: the same card's inspector");
        assert!(!dock.has("*processes*", "processes-hint"), "{when}: no 'no inspector' hint");
        assert_eq!(
            dock.eval("(eseq.sequencer/lane-patch-node-selected-id)"),
            Some(Value::Number(expr as f64)),
            "{when}: selection kept"
        );
        assert!(column_spec(dock).contains(name), "{when}: the code tile stays");
        dock.relayout();
        assert!(dock.visible().iter().any(|shown| shown == name), "{when}: {:?}", dock.visible());
    };

    dock.reload("ui/processes-buffer.lisp", "\"processes-caption\"", "\"processes-caption-restyled\"");
    still_docked(&mut dock, "after re-evaluating processes-buffer.lisp");
    assert!(dock.has("*processes*", "processes-caption-restyled"), "the edited panel renders");

    dock.reload(
        "packages/alez.neural/src/variable-reset.lisp",
        "(gvr-proc-card self n slots index (gvr-proc-wired-inlets (gvr-node-patch-entries self n)) :fill \"dock\")",
        "(box :key \"gvr-inspector-restyled\" :width :fill (gvr-proc-card self n slots index (gvr-proc-wired-inlets (gvr-node-patch-entries self n)) :fill \"dock\"))",
    );
    still_docked(&mut dock, "after re-evaluating variable-reset.lisp");
    assert!(dock.has("*processes*", "gvr-inspector-restyled"), "the edited renderer draws the dock");
}

/// The selected card's inspector lives in the dock (not the bay): a hint
/// until a card is selected, then the card with its inlet pickers, < > x,
/// out mapping; each edits the node's chain. With the dock off, the bay
/// shows the same card inline.
#[test]
fn processes_buffer_inspects_the_selected_card_and_the_bay_does_not() {
    let mut dock = Dock::open();
    dock.show_tab();
    let rand = dock.add("lane-rand");
    let other = dock.add("neural-transpose");
    let card = |id: u64| format!("graph-variable-reset-proc-card-1-{id}");

    assert!(dock.has("*processes*", "processes-hint"), "nothing selected: a hint");
    assert!(!dock.has("*processes*", &card(rand)));
    assert!(!dock.has(&dock.bay.clone(), &card(rand)), "the bay has no inline inspector while docked");

    dock.select(rand);
    assert!(dock.has("*processes*", &card(rand)), "the selected card in the dock");
    assert!(!dock.has("*processes*", "processes-hint"));
    assert!(!dock.has("*processes*", "processes-code-toggle"), "not an expr card: no code toggle");
    assert!(!dock.has(&dock.bay.clone(), &card(rand)), "still not in the bay");
    let spec = column_spec(&mut dock);
    assert!(!spec.contains("*expr"), "no code tile for a class card: {spec}");

    // Inlet picker (hi is a float number picker).
    dock.change_in("*processes*", &format!("graph-variable-reset-proc-1-{rand}-hi"), Value::Number(7.0));
    assert_eq!(dock.inlet(rand, "hi"), Some(Value::Number(7.0)));

    // Map the out port onto the payload: arm in the dock, bind on the row's chip.
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-map-1-{rand}-out"));
    let graph = dock.graph;
    assert_eq!(
        dock.eval(&format!("(let ((i (instance-ref {graph}))) i.map-slot)")),
        Some(Value::Number(rand as f64)),
        "armed from the dock"
    );
    let bay = dock.bay.clone();
    dock.click_in(&bay, "graph-variable-reset-transpose-1-map-target");
    let mapped = dock.eval(&format!(
        "(get (first (filter (lambda (p) (= (get p :name) \"out\")) (get (first (graph-node-process-chain {graph} 1)) :ports))) :mapped-to)"
    ));
    assert_eq!(mapped, Some(Value::String("transpose".to_string())), "bound from the neuron row");
    assert!(dock.has("*processes*", &format!("graph-variable-reset-proc-unmap-1-{rand}-out")));

    // No < > x on the card (cards reorder by dragging); on/off, and the
    // dock's red delete at its bottom right removes the card.
    assert!(!dock.has("*processes*", &format!("graph-variable-reset-proc-right-1-{rand}")));
    assert!(!dock.has("*processes*", &format!("graph-variable-reset-proc-left-1-{rand}")));
    assert!(!dock.has("*processes*", &format!("graph-variable-reset-proc-remove-1-{rand}")));
    assert_eq!(dock.chain_classes()[0], "lane-rand");
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-enable-1-{rand}"));
    assert_eq!(dock.chain("enabled")[0], Value::Bool(false), "switched off");
    dock.click_in("*processes*", "processes-delete");
    assert_eq!(dock.chain_classes(), vec!["neural-transpose".to_string()], "removed");
    assert!(!dock.has("*processes*", "processes-delete"), "delete goes with the card");
    assert!(dock.has("*processes*", "processes-hint"), "the removed card's inspector goes");

    // Dock off: the bay shows the same card inline (its first card when
    // nothing is selected).
    dock.select(other);
    dock.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split false)");
    assert!(dock.has(&dock.bay.clone(), &card(other)), "fallback: the inline inspector");
    assert!(
        dock.has(&dock.bay.clone(), &format!("graph-variable-reset-proc-remove-1-{other}")),
        "fallback: the inline card carries its own delete"
    );
    dock.eval("(setopt eseq.processes-buffer/processes-buffer-auto-split true)");
    assert!(!dock.has(&dock.bay.clone(), &card(other)));
    // The sidebar hidden changes nothing: the dock is in the right column.
    dock.eval("(set! eseq.seq-core-state/samples-sidebar-visible false)");
    assert!(!dock.has(&dock.bay.clone(), &card(other)), "sidebar hidden: still docked");
    // Another layout (the instrument patcher's): inline again.
    dock.eval("(set! eseq.seq-step-tabs/seq-layout-mode :instrument-patcher)");
    assert!(dock.has(&dock.bay.clone(), &card(other)), "patcher layout: the inline inspector");
}

/// An expr card shows its code under the inspector, splitting the dock
/// about half and half; hide code gives the inspector the whole dock and is
/// remembered; the card's edit button shows the code again and focuses it;
/// commits work there; the tile goes with the card.
#[test]
fn processes_buffer_shows_an_expr_cards_code_half_the_dock() {
    let mut dock = Dock::open();
    dock.show_tab();
    let graph = dock.graph;
    let expr = dock.add("expr");
    dock.eval(&format!("(graph-node-process-expr-set {graph} 1 {expr} \"(* x rate)\")"));
    let rand = dock.add("lane-rand");
    let name = "*expr node 1 · slot 1*";

    dock.select(expr);
    let spec = column_spec(&mut dock);
    assert!(spec.contains(name), "selecting an expr card shows its code: {spec}");
    let tiles = dock.tiles();
    let height = |tiles: &[(String, f32)], buffer: &str| {
        tiles.iter().find(|(shown, _)| shown == buffer).map(|(_, h)| *h).unwrap_or_else(|| panic!("{buffer} not in {tiles:?}"))
    };
    let (inspector, code) = (height(&tiles, "*processes*") as f64, height(&tiles, name) as f64);
    assert!(inspector > 0.0 && code > 0.0, "{tiles:?}");
    let share = code / (inspector + code);
    assert!((0.4..=0.6).contains(&share), "code takes about half the dock: {share} {tiles:?}");
    assert_column(&dock, true, Some(name), "an expr card selected");
    assert!(dock.has("*processes*", "processes-code-toggle"));

    // hide code: the inspector takes the whole dock; a class card never has code.
    dock.click_in("*processes*", "processes-code-toggle");
    assert_eq!(dock.eval("eseq.processes-buffer/code-visible"), Some(Value::Bool(false)));
    assert!(!column_spec(&mut dock).contains("*expr"));
    dock.relayout();
    assert!(!dock.visible().iter().any(|shown| shown == name), "{:?}", dock.visible());
    dock.select(rand);
    dock.select(expr);
    assert!(!column_spec(&mut dock).contains("*expr"), "hidden stays hidden for the session");

    // The card's edit button shows the code again and focuses it.
    dock.relayout();
    let bay = dock.bay.clone();
    dock.click_in(&bay, &format!("lane-patch-expr-edit-{expr}"));
    assert_eq!(dock.eval("eseq.processes-buffer/code-visible"), Some(Value::Bool(true)));
    assert_eq!(dock.editor.active_buffer().name, name, "the code tile has focus");
    assert_eq!(dock.editor.active_buffer().text(), "(* x rate)");
    let visible = dock.visible();
    for required in [name, "*processes*", bay.as_str()] {
        assert!(visible.iter().any(|shown| shown == required), "{required} not in {visible:?}");
    }

    // Commit from the code tile.
    dock.editor.active_buffer_mut().set_text("(+ x depth)");
    dock.editor.active_buffer_mut().dirty = true;
    dock.eval("(save-buffer)");
    assert_eq!(
        dock.eval(&format!("(graph-node-process-expr-source {graph} 1 {expr})")),
        Some(Value::String("(+ x depth)".to_string()))
    );

    // Selecting a class card: the inspector takes the whole dock.
    dock.select(rand);
    assert!(!column_spec(&mut dock).contains("*expr"));
    let visible = dock.visible();
    assert!(!visible.iter().any(|shown| shown == name), "{visible:?}");
    dock.select(expr);
    assert!(dock.visible().iter().any(|shown| shown == name));

    // The card goes (the bay's remove: the native, then the version bump).
    dock.eval(&format!(
        "(do (graph-node-process-remove {graph} 1 {expr}) (eseq.sequencer/lane-patch-node-touch))"
    ));
    assert!(!column_spec(&mut dock).contains("*expr"), "no code tile for a removed card");
    let visible = dock.visible();
    assert!(!visible.iter().any(|shown| shown == name), "the code tile closed: {visible:?}");
    assert!(visible.iter().any(|shown| shown == "*processes*"), "the dock stays: {visible:?}");
}

/// Promote (docs/expr-process-spec.md §8; eseq-waa9.17) from the dock's
/// inspector: promote… opens the name modal in the dock, a commit writes the
/// module into a temp My processes package, loads it and rebinds the card
/// in place (the code tile goes: no longer an expr card); "as expr" turns it
/// back and a second promote updates the class.
#[test]
fn processes_buffer_promote_moves_the_card_into_my_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let package = tmp.path().join("packages/user.processes");
    let _package_guard = sequencer::lisp_host::set_my_processes_package_dir_override(Some(package.clone()));
    let (state, mut editor, graph) = super::graph_visualization_ui_tests::graph_panel_editor();
    // The app's UI VM holds def-process and the builtin library itself, so
    // `load` of a promoted module registers and publishes its class.
    sequencer::lisp_host::register_published_process_authoring_natives(
        editor.runtime_mut(),
        Arc::clone(&state),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    editor
        .runtime_mut()
        .eval_str(&sequencer::lisp_host::load_process_library_source())
        .expect("builtin process library");
    let mut dock = Dock::with_editor(Arc::clone(&state), editor, graph);
    dock.show_tab();
    let bay = dock.bay.clone();

    let id = match dock.eval(&format!(
        "(eseq.expr-buffer/add-node-preset (instance-ref {graph}) 1 (eseq.expr-buffer/preset-named \"bounce\"))"
    )) {
        Some(Value::Number(id)) => id as u64,
        other => panic!("{other:?}"),
    };
    dock.eval("(eseq.sequencer/lane-patch-node-touch)");
    dock.select(id);
    assert!(column_spec(&mut dock).contains("*expr node 1"), "the bounce card's code shows");
    assert!(!dock.has(&bay, &format!("graph-variable-reset-proc-promote-1-{id}")), "not in the bay");
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-promote-1-{id}"));
    assert_eq!(dock.eval("(eseq.expr-buffer/promote-open?)"), Some(Value::Bool(true)), "the name modal opens");
    assert_eq!(dock.eval("eseq.expr-buffer/promote-origin"), Some(Value::String("dock".to_string())));
    assert!(dock.has("*processes*", "expr-promote-name"), "in the dock's mount");

    // The live check refuses a taken name before anything is written.
    dock.eval("(eseq.expr-buffer/set-promote-name \"delay\")");
    let problem = dock.eval("(eseq.expr-buffer/promote-name-error)");
    assert!(matches!(&problem, Some(Value::String(e)) if e.contains("already exists")), "{problem:?}");
    dock.eval("(eseq.expr-buffer/commit-promote)");
    assert!(!package.exists(), "nothing written for a refused name");

    dock.eval("(eseq.expr-buffer/set-promote-name \"bouncy\")");
    dock.eval("(eseq.expr-buffer/commit-promote)");
    assert_eq!(dock.eval("(eseq.expr-buffer/promote-open?)"), Some(Value::Bool(false)), "closed on success");
    let class = "user.processes.bouncy/bouncy";
    assert!(package.join("src/bouncy.lisp").is_file());
    assert_eq!(dock.chain_classes(), vec![class.to_string()], "rebound in place");
    assert_eq!(dock.inlet(id, "decay"), Some(Value::Number(0.8)), "values kept");
    assert!(!column_spec(&mut dock).contains("*expr"), "no longer an expr card: no code tile");
    assert!(!dock.has(&bay, &format!("lane-patch-expr-edit-{id}")));
    assert!(
        dock.has("*processes*", &format!("graph-variable-reset-proc-as-expr-1-{id}")),
        "a promoted card offers as expr"
    );
    assert!(
        dock.eval(&format!(
            "(reduce (lambda (acc c) (or acc (= (get c :label) \"bouncy\"))) false (graph-node-process-classes))"
        )) == Some(Value::Bool(true)),
        "the add menu offers it"
    );

    // As expr: back to an expr card holding the body, values kept.
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-as-expr-1-{id}"));
    let classes = dock.chain_classes();
    assert!(classes[0].starts_with("expr#"), "{classes:?}");
    assert_eq!(
        dock.eval(&format!("(graph-node-process-expr-source {graph} 1 {id})")),
        Some(Value::String("(* k (pow decay $n))".to_string()))
    );
    assert!(column_spec(&mut dock).contains("*expr node 1"), "an expr card again: its code shows");

    // Tweak and promote again: the modal comes up on the name the card came
    // from, as an update of that class, and the commit replaces it.
    let tweaked = dock.eval(&format!(
        "(graph-node-process-expr-set {graph} 1 {id} \"(* k (pow decay $n) 2)\")"
    ));
    assert!(matches!(&tweaked, Some(Value::Map(_))), "{tweaked:?}");
    dock.eval("(eseq.sequencer/lane-patch-node-touch)");
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-promote-1-{id}"));
    assert_eq!(dock.eval("eseq.expr-buffer/promote-name"), Some(Value::String("bouncy".to_string())));
    assert_eq!(dock.eval("(eseq.expr-buffer/promote-name-error)"), Some(Value::Nil));
    assert_eq!(dock.eval("(eseq.expr-buffer/promote-update?)"), Some(Value::Bool(true)));
    dock.eval("(eseq.expr-buffer/commit-promote)");
    assert_eq!(dock.eval("(eseq.expr-buffer/promote-open?)"), Some(Value::Bool(false)), "closed on success");
    assert_eq!(dock.chain_classes(), vec![class.to_string()], "rebound to the updated class");
    let text = std::fs::read_to_string(package.join("src/bouncy.lisp")).unwrap();
    assert!(text.contains(":expr \"(* k (pow decay $n) 2)\""), "{text}");
}

// ── Undo (eseq-waa9.23) ─────────────────────────────────────────────────
// The natives apply a node edit at once and queue its history record; the
// host records it (`graph-node-process-history`) and Cmd+Z replays it.

/// The app half of the dock: its history, over the dock's state.
struct History {
    app: app::App,
}

impl History {
    fn new(dock: &mut Dock) -> Self {
        let history = Self { app: super::test_app_for_track_visual_state(Arc::clone(&dock.state)) };
        dock.editor.drain_host_commands();
        history
    }

    /// Hand every queued node-process history record to the host handler,
    /// as the event loop's host-command drain does.
    fn record(&mut self, dock: &mut Dock) {
        for command in dock.editor.drain_host_commands() {
            if let eseqlisp::host::HostCommand::Custom { name, payload } = command {
                if name == sequencer::lisp_host::GRAPH_NODE_PROCESS_HISTORY_COMMAND {
                    crate::host_commands::graph_node_processes::apply_graph_node_process_history_host_command(
                        &mut self.app,
                        &payload,
                    )
                    .expect("record node process edit");
                }
            }
        }
    }

    /// Pointer release: ends a coalescing gesture.
    fn release(&mut self) {
        app::edit::finish_active_gesture(&mut self.app);
    }

    fn undo_len(&self) -> usize {
        self.app.history.undo_len()
    }

    fn replay(&mut self, dock: &mut Dock, undo: bool) {
        let replay = if undo { app::edit::undo(&mut self.app) } else { app::edit::redo(&mut self.app) };
        assert!(matches!(replay, app::history::HistoryReplay::Applied(_)), "{}: {replay:?}", if undo { "undo" } else { "redo" });
        // The UI tick's graph-read sweep (the replay republished the
        // scheduler snapshot); no `ui_epoch` bump, no version touch.
        sequencer::lisp_host::queue_graph_read_invalidations(dock.editor.runtime_mut(), &dock.state);
        dock.editor.runtime_mut().run_reactive_cycle();
        dock.editor.refresh_runtime_side_effects();
    }

    fn undo(&mut self, dock: &mut Dock) {
        self.replay(dock, true);
    }

    fn redo(&mut self, dock: &mut Dock) {
        self.replay(dock, false);
    }
}

fn ids(dock: &mut Dock) -> Vec<u64> {
    dock.chain("instance-id")
        .into_iter()
        .map(|id| match id {
            Value::Number(id) => id as u64,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// Where `from`'s port `port` is wired, as the chain read reports it.
fn wired_to(dock: &mut Dock, from: u64, port: &str) -> Value {
    dock.eval(&format!(
        "(get (first (filter (lambda (p) (= (get p :name) \"{port}\"))
                 (get (reduce (lambda (acc s) (if (= (get s :instance-id) {from}) s acc)) nil
                         (graph-node-process-chain {} 1)) :ports))) :wired-to)",
        dock.graph
    ))
    .unwrap_or(Value::Nil)
}

/// The user's report: the red delete in the dock could not be undone. Undo
/// brings the card back whole (same id and place, inlet values, the wire
/// into it, the dock's inspector); redo deletes it again. The drag that set
/// the inlet was one step.
#[test]
fn processes_buffer_delete_undo_restores_the_card_and_redo_deletes_it() {
    let mut dock = Dock::open();
    dock.show_tab();
    let mut history = History::new(&mut dock);
    let source = dock.add("lane-rand");
    let target = dock.add("lane-rand");
    let graph = dock.graph;
    dock.eval(&format!("(graph-node-process-wire {graph} 1 {source} \"wire\" {target} \"hi\")"));
    history.record(&mut dock);
    assert_eq!(history.undo_len(), 3, "two adds and a wire: one step each");
    let wire = wired_to(&mut dock, source, "wire");
    assert!(matches!(&wire, Value::Map(_)), "wired: {wire:?}");

    dock.select(target);
    let card = format!("graph-variable-reset-proc-card-1-{target}");
    assert!(dock.has("*processes*", &card));
    // An inlet picker drag: several values, one gesture, one step.
    let picker = format!("graph-variable-reset-proc-1-{target}-lo");
    let lo_before = dock.inlet(target, "lo");
    for value in [0.2, 0.3, 0.4] {
        dock.change_in("*processes*", &picker, Value::Number(value));
        history.record(&mut dock);
    }
    history.release();
    assert_eq!(history.undo_len(), 4, "the drag is one step");
    assert_eq!(dock.inlet(target, "lo"), Some(Value::Number(0.4)));

    dock.click_in("*processes*", "processes-delete");
    history.record(&mut dock);
    assert_eq!(history.undo_len(), 5, "the delete is one step");
    assert_eq!(ids(&mut dock), vec![source], "deleted");
    assert_eq!(wired_to(&mut dock, source, "wire"), Value::Nil, "the wire into it went too");
    assert!(dock.has("*processes*", "processes-hint"), "the inspector went");

    history.undo(&mut dock);
    assert_eq!(ids(&mut dock), vec![source, target], "back, same id, same place");
    assert_eq!(dock.inlet(target, "lo"), Some(Value::Number(0.4)), "inlet values back");
    assert_eq!(wired_to(&mut dock, source, "wire"), wire, "the wire into it back");
    assert!(dock.has("*processes*", &card), "the dock inspects it again");
    assert!(dock.has("*processes*", "processes-delete"));

    history.redo(&mut dock);
    assert_eq!(ids(&mut dock), vec![source], "redo deletes it again");
    assert!(dock.has("*processes*", "processes-hint"));

    // Back through the drag (one step), then the wire and the adds.
    history.undo(&mut dock);
    history.undo(&mut dock);
    assert_eq!(dock.inlet(target, "lo"), lo_before, "the whole drag undone at once");
    assert_eq!(wired_to(&mut dock, source, "wire"), wire, "the wire is its own step");
    history.undo(&mut dock);
    assert_eq!(wired_to(&mut dock, source, "wire"), Value::Nil, "wire undone");
    history.undo(&mut dock);
    assert_eq!(ids(&mut dock), vec![source], "second add undone");
    history.undo(&mut dock);
    assert!(ids(&mut dock).is_empty(), "first add undone");
    history.redo(&mut dock);
    assert_eq!(ids(&mut dock), vec![source], "redo re-adds under the same id");
}

/// Clicks are one step each: enable, map; selection is none.
#[test]
fn processes_buffer_enable_and_map_undo_and_selection_is_not_a_step() {
    let mut dock = Dock::open();
    dock.show_tab();
    let mut history = History::new(&mut dock);
    let rand = dock.add("lane-rand");
    history.record(&mut dock);
    let before = history.undo_len();
    dock.select(rand);
    history.record(&mut dock);
    assert_eq!(history.undo_len(), before, "selecting a card is not an edit");

    dock.click_in("*processes*", &format!("graph-variable-reset-proc-enable-1-{rand}"));
    history.record(&mut dock);
    assert_eq!(dock.chain("enabled")[0], Value::Bool(false));
    assert_eq!(history.undo_len(), before + 1);
    let graph = dock.graph;
    dock.eval(&format!("(graph-node-process-map {graph} 1 {rand} \"out\" :transpose)"));
    history.record(&mut dock);
    assert_eq!(history.undo_len(), before + 2);

    history.undo(&mut dock);
    assert_eq!(dock.chain("enabled")[0], Value::Bool(false), "the map undone first");
    let mapped = dock.eval(&format!(
        "(get (first (filter (lambda (p) (= (get p :name) \"out\")) (get (first (graph-node-process-chain {graph} 1)) :ports))) :mapped-to)"
    ));
    assert_eq!(mapped, Some(Value::Nil), "unmapped");
    history.undo(&mut dock);
    assert_eq!(dock.chain("enabled")[0], Value::Bool(true), "switched back on");
    assert!(dock.has("*processes*", &format!("graph-variable-reset-proc-card-1-{rand}")));
}

/// Expr commits undo to the previous body; a promote's rebind undoes back
/// to the expr card (the promoted file stays on disk).
#[test]
fn processes_buffer_expr_commit_and_promote_rebind_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let package = tmp.path().join("packages/user.processes");
    let _package_guard = sequencer::lisp_host::set_my_processes_package_dir_override(Some(package.clone()));
    let (state, mut editor, graph) = super::graph_visualization_ui_tests::graph_panel_editor();
    sequencer::lisp_host::register_published_process_authoring_natives(
        editor.runtime_mut(),
        Arc::clone(&state),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    editor
        .runtime_mut()
        .eval_str(&sequencer::lisp_host::load_process_library_source())
        .expect("builtin process library");
    let mut dock = Dock::with_editor(Arc::clone(&state), editor, graph);
    dock.show_tab();
    let mut history = History::new(&mut dock);
    let expr = dock.add("expr");
    dock.eval(&format!("(graph-node-process-expr-set {graph} 1 {expr} \"(* x 2)\")"));
    let first = dock.chain_classes()[0].clone();
    dock.eval(&format!("(graph-node-process-expr-set {graph} 1 {expr} \"(+ x 3)\")"));
    history.record(&mut dock);
    let second = dock.chain_classes()[0].clone();
    assert_ne!(first, second);
    let source = |dock: &mut Dock| dock.eval(&format!("(graph-node-process-expr-source {graph} 1 {expr})"));

    history.undo(&mut dock);
    assert_eq!(source(&mut dock), Some(Value::String("(* x 2)".to_string())));
    assert_eq!(dock.chain_classes(), vec![first.clone()], "the previous hashed class");
    history.redo(&mut dock);
    assert_eq!(source(&mut dock), Some(Value::String("(+ x 3)".to_string())));

    // Promote: write + load the module, then the recorded rebind.
    dock.eval("(eseq.sequencer/lane-patch-node-touch)");
    dock.select(expr);
    dock.click_in("*processes*", &format!("graph-variable-reset-proc-promote-1-{expr}"));
    dock.eval("(eseq.expr-buffer/set-promote-name \"plusthree\")");
    dock.eval("(eseq.expr-buffer/commit-promote)");
    history.record(&mut dock);
    let class = "user.processes.plusthree/plusthree";
    assert_eq!(dock.chain_classes(), vec![class.to_string()], "rebound");
    history.undo(&mut dock);
    assert_eq!(dock.chain_classes(), vec![second], "undo: the expr card again");
    assert_eq!(source(&mut dock), Some(Value::String("(+ x 3)".to_string())));
    assert!(package.join("src/plusthree.lisp").is_file(), "the promoted file stays");
    history.redo(&mut dock);
    assert_eq!(dock.chain_classes(), vec![class.to_string()], "redo: the promoted class");
}

/// Drag gestures are keyed per inlet: two inlets dragged back to back are
/// two steps even without a release between them, and a click (the delete)
/// during an open drag commits the drag as its own step first. A new edit
/// after an undo clears redo.
#[test]
fn processes_buffer_inlet_gestures_are_per_inlet_and_a_click_commits_the_open_drag() {
    let mut dock = Dock::open();
    dock.show_tab();
    let mut history = History::new(&mut dock);
    let target = dock.add("lane-rand");
    history.record(&mut dock);
    dock.select(target);
    let base = history.undo_len();
    let lo = format!("graph-variable-reset-proc-1-{target}-lo");
    let hi = format!("graph-variable-reset-proc-1-{target}-hi");
    let (lo_before, hi_before) = (dock.inlet(target, "lo"), dock.inlet(target, "hi"));
    for value in [0.2, 0.3] {
        dock.change_in("*processes*", &lo, Value::Number(value));
        history.record(&mut dock);
    }
    for value in [0.7, 0.8] {
        dock.change_in("*processes*", &hi, Value::Number(value));
        history.record(&mut dock);
    }
    // Still inside the hi drag: the delete click commits it first.
    dock.click_in("*processes*", "processes-delete");
    history.record(&mut dock);
    assert_eq!(history.undo_len(), base + 3, "lo drag, hi drag, delete");
    assert!(ids(&mut dock).is_empty());

    history.undo(&mut dock);
    assert_eq!(ids(&mut dock), vec![target]);
    assert_eq!(dock.inlet(target, "hi"), Some(Value::Number(0.8)));
    history.undo(&mut dock);
    assert_eq!(dock.inlet(target, "hi"), hi_before, "the hi drag is its own step");
    assert_eq!(dock.inlet(target, "lo"), Some(Value::Number(0.3)));
    history.undo(&mut dock);
    assert_eq!(dock.inlet(target, "lo"), lo_before, "the lo drag is its own step");

    // A new edit drops the redo branch.
    dock.change_in("*processes*", &lo, Value::Number(0.5));
    history.record(&mut dock);
    history.release();
    assert_eq!(history.undo_len(), base + 1);
    assert!(history.app.history.next_redo_patch().is_none(), "redo cleared");
}
