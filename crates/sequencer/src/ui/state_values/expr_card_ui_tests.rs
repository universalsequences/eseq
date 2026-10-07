//! Expr cards in a `neural` node bay (docs/expr-process-spec.md §2, §3.1,
//! §3.2; bead eseq-waa9.11): the card, its edit buffer and the inspector,
//! driven through real clicks and keys on the package panel.
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

struct Bay {
    state: Arc<SequencerState>,
    editor: Editor,
    graph: u64,
    buffer: String,
}

impl Bay {
    /// The package panel with node 1 expanded, the builtin process library
    /// (which holds the plain `expr` class) published.
    fn open() -> Self {
        let (state, mut editor, graph) = super::graph_visualization_ui_tests::graph_panel_editor();
        let mut authoring = Runtime::new();
        sequencer::lisp_host::register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        authoring
            .eval_str(&sequencer::lisp_host::load_process_library_source())
            .expect("builtin process library");
        editor.runtime_mut().set_reactive("SEQ", "process-run-errors", test_list(vec![]));
        let buffer = sequencer::lisp_host::instance_view_buffer_name("neural", "neural 1");
        let mut bay = Self { state, editor, graph, buffer };
        bay.eval(&format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {graph}) 1)"));
        bay
    }

    fn eval(&mut self, source: &str) -> Option<Value> {
        let value = self.editor.runtime_mut().eval_str(source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
        value
    }

    fn add(&mut self, class: &str) -> u64 {
        match self.eval(&format!("(graph-node-process-add {} 1 \"{class}\")", self.graph)) {
            Some(Value::Number(id)) => id as u64,
            other => panic!("add {class}: {other:?}"),
        }
    }

    fn set_body(&mut self, id: u64, body: &str) {
        let ok = self.eval(&format!(
            "(get (graph-node-process-expr-set {} 1 {id} \"{body}\") :ok)",
            self.graph
        ));
        assert_eq!(ok, Some(Value::Bool(true)), "{body}");
    }

    fn source(&mut self, id: u64) -> Option<Value> {
        self.eval(&format!("(graph-node-process-expr-source {} 1 {id})", self.graph))
    }

    fn show_bay(&mut self) {
        let id = self.editor.buffers.iter().find(|item| item.name == self.buffer).expect("bay buffer").id;
        self.editor.set_active_buffer(id);
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    fn layout(&mut self) -> Arc<eseqlisp::layout::LayoutNode> {
        self.show_bay();
        self.editor.widget_layout().expect("bay layout")
    }

    fn click(&mut self, key: &str) {
        let layout = self.layout();
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

    fn active_name(&self) -> String {
        self.editor.active_buffer().name.clone()
    }

    /// Replace the edit buffer's text the way typing does: the buffer is
    /// modified afterwards.
    fn type_body(&mut self, body: &str) {
        let buffer = self.editor.active_buffer_mut();
        buffer.set_text(body);
        buffer.dirty = true;
    }

    fn commit_chord(&mut self) {
        for _ in 0..2 {
            self.editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        }
    }
}

fn prop<'a>(node: &'a eseqlisp::layout::LayoutNode, key: &str) -> Option<&'a Value> {
    node.props.get(key)
}

/// The edit button opens a text buffer named for the slot with the stored
/// body; C-c C-c commits it (inlets reconcile, the buffer is saved, removed
/// inlets are toasted); a failed commit leaves the card and the buffer
/// modified, highlights the span and lights the card's error dot; reopening
/// focuses the same buffer; a commit after the card is gone does nothing.
#[test]
fn expr_card_edit_buffer_commits_reports_errors_and_survives_removal() {
    let mut bay = Bay::open();
    let rand = bay.add("lane-rand");
    let expr = bay.add("expr");
    bay.set_body(expr, "(sin (* x rate))");

    // The uniform card: edit button, preview on the out-port row, no error.
    let layout = bay.layout();
    let preview = find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-preview-{expr}"))
        .expect("expr preview label");
    assert_finite_nonzero_rect(preview, "expr preview");
    let dot = find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-error-{expr}"))
        .expect("expr error dot");
    assert_eq!(prop(dot, "active"), Some(&Value::Number(0.0)));
    assert!(
        find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-edit-{rand}")).is_none(),
        "only expr cards get an edit button"
    );
    let card = |layout: &eseqlisp::layout::LayoutNode, id: u64| {
        find_layout_node_by_stable_key_suffix(layout, &format!("lane-patch-col-{id}")).expect("card").rect
    };
    let (rand_card, expr_card) = (card(&layout, rand), card(&layout, expr));
    assert_eq!((rand_card.width, rand_card.height), (expr_card.width, expr_card.height), "uniform card size");

    // Inspector: the expr inlets are unbounded relative pickers.
    bay.eval(&format!("(eseq.sequencer/lane-patch-node-select {expr})"));
    let layout = bay.layout();
    let picker = find_layout_node_by_stable_key_suffix(&layout, &format!("graph-variable-reset-proc-1-{expr}-rate"))
        .expect("rate picker");
    assert_eq!(picker.widget_type, "number-picker");
    assert_eq!(prop(picker, "drag"), Some(&Value::Keyword("relative".to_string())));
    assert_eq!(prop(picker, "min"), None, "no range: the body carries none");

    // Edit opens the buffer with the body, in the expr mode.
    bay.click(&format!("lane-patch-expr-edit-{expr}"));
    assert_eq!(bay.active_name(), "*expr node 1 · slot 2*");
    assert_eq!(bay.editor.active_buffer().text(), "(sin (* x rate))");
    assert_eq!(
        bay.eval("(current-buffer-mode)"),
        Some(Value::String("eseq.expr-buffer/expr-mode".to_string()))
    );
    assert!(!bay.editor.active_buffer().dirty);

    // Completion in the buffer offers the $ context variables and the
    // direct writes (spec §4, §6), with their docs.
    bay.type_body("(+ $p");
    bay.editor.active_buffer_mut().cursor = (0, 5);
    bay.editor.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    let completion = bay.editor.completion_state().expect("completion popup");
    let phase = completion.items.iter().find(|item| item.label == "$phase").expect("$phase offered");
    assert!(phase.docs.as_deref().is_some_and(|doc| doc.contains("bar")), "{phase:?}");
    bay.editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // A good commit: the chain takes the body, rate goes (toasted), saved.
    bay.type_body("(+ x depth)");
    bay.commit_chord();
    assert_eq!(bay.source(expr), Some(Value::String("(+ x depth)".to_string())));
    assert!(!bay.editor.active_buffer().dirty, "a good commit saves the buffer");
    let toast = bay.editor.toast().expect("removed-inlet toast");
    assert!(toast.message.contains("removed inlets: rate"), "{}", toast.message);

    // A failed commit: the card keeps its body, the buffer stays modified,
    // the span is highlighted, the card lights its error dot.
    bay.type_body("(+ x\n   (sinn depth))");
    bay.commit_chord();
    assert_eq!(bay.source(expr), Some(Value::String("(+ x depth)".to_string())));
    assert!(bay.editor.active_buffer().dirty, "a failed commit keeps the edit");
    let styles = &bay.editor.active_buffer().text_styles;
    assert_eq!(styles.len(), 1, "the reported span is highlighted");
    assert_eq!(styles[0].line, Some(1), "on the line of the unknown function");
    assert_eq!(bay.editor.active_buffer().cursor.0, 1, "the cursor goes to that line");
    let toast = bay.editor.toast().expect("error toast");
    assert_eq!(toast.kind, eseqlisp::ToastKind::Error);
    assert!(toast.message.contains("sinn"), "{}", toast.message);
    let layout = bay.layout();
    let dot = find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-error-{expr}")).unwrap();
    assert_eq!(prop(dot, "active"), Some(&Value::Number(1.0)), "failed commit lights the dot");

    // Reopening focuses the same buffer, with the unsaved edit intact.
    let buffers = bay.editor.buffers.len();
    bay.click(&format!("lane-patch-expr-edit-{expr}"));
    assert_eq!(bay.active_name(), "*expr node 1 · slot 2*");
    assert_eq!(bay.editor.buffers.len(), buffers);
    assert!(bay.editor.active_buffer().text().contains("sinn"));

    // File > Save (the Cmd+S menu item) commits an expr buffer instead of
    // saving the project.
    bay.type_body("(* x depth)");
    bay.editor.drain_host_commands();
    bay.eval("(eseq.transport/file-menu-save)");
    assert_eq!(bay.source(expr), Some(Value::String("(* x depth)".to_string())));
    assert!(!bay.editor.active_buffer().dirty);
    assert!(!bay.editor.drain_host_commands().iter().any(|command| matches!(command,
        HostCommand::Custom { name, .. } if name == "project-save-open")));
    let layout = bay.layout();
    let dot = find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-error-{expr}")).unwrap();
    assert_eq!(prop(dot, "active"), Some(&Value::Number(0.0)), "a good commit clears the dot");

    // The card goes away: a later commit changes nothing and says so.
    bay.eval(&format!("(graph-node-process-remove {} 1 {expr})", bay.graph));
    let id = bay.editor.buffers.iter().find(|item| item.name == "*expr node 1 · slot 2*").unwrap().id;
    bay.editor.set_active_buffer(id);
    bay.type_body("(* x 2)");
    bay.commit_chord();
    assert!(bay.editor.active_buffer().dirty);
    let toast = bay.editor.toast().expect("gone toast");
    assert!(toast.message.contains("gone"), "{}", toast.message);
    assert_eq!(
        bay.eval(&format!("(graph-node-process-slot? {} 1 {expr})", bay.graph)),
        Some(Value::Bool(false))
    );
}

/// Port overflow (§3.2 (a)): past three in ports the card shows two plus a
/// `+n` badge, keeps any port a cable lands on, and the inspector still
/// lists every inlet. A scheduler run error lights the dot through
/// `SEQ.process-run-errors`.
#[test]
fn expr_card_overflow_badge_and_run_error_dot() {
    let mut bay = Bay::open();
    let rand = bay.add("lane-rand");
    let expr = bay.add("expr");
    bay.set_body(expr, "(+ a (* b c) d e)");

    let in_port = |layout: &eseqlisp::layout::LayoutNode, name: &str| {
        find_layout_node_by_stable_key_suffix(layout, &format!("lane-patch-in-port-{expr}-{name}")).is_some()
    };
    let badge = |layout: &eseqlisp::layout::LayoutNode| {
        find_layout_node_by_stable_key_suffix(layout, &format!("lane-patch-in-overflow-{expr}"))
            .map(|node| {
                assert_finite_nonzero_rect(node, "overflow badge");
                node.clone()
            })
    };
    let layout = bay.layout();
    assert!(in_port(&layout, "a") && in_port(&layout, "b"));
    assert!(!in_port(&layout, "c") && !in_port(&layout, "e"));
    assert!(badge(&layout).is_some());

    // A cable onto `e` keeps its port on the card.
    bay.eval(&format!(
        "(do (graph-node-process-wire {} 1 {rand} \"wire\" {expr} \"e\") (eseq.sequencer/lane-patch-node-touch))",
        bay.graph
    ));
    let layout = bay.layout();
    assert!(in_port(&layout, "a") && in_port(&layout, "e"), "a wired port always shows");
    assert!(!in_port(&layout, "b"));
    assert!(badge(&layout).is_some());

    // Every inlet is settable in the inspector.
    bay.eval(&format!("(eseq.sequencer/lane-patch-node-select {expr})"));
    let layout = bay.layout();
    for name in ["b", "c", "d"] {
        let key = format!("graph-variable-reset-proc-1-{expr}-{name}");
        let picker = find_layout_node_by_stable_key_suffix(&layout, &key).expect(&key);
        assert_finite_nonzero_rect(picker, &key);
    }

    // A run error reaches the dot without re-rendering the bay.
    let dot_active = |bay: &mut Bay| {
        let layout = bay.layout();
        let dot = find_layout_node_by_stable_key_suffix(&layout, &format!("lane-patch-expr-error-{expr}")).unwrap();
        prop(dot, "active").cloned()
    };
    assert_eq!(dot_active(&mut bay), Some(Value::Number(0.0)));
    bay.state.publish_process_run_errors([(expr, "step budget exceeded".to_string())].into_iter().collect());
    bay.show_bay();
    bay.editor.runtime_mut().set_reactive(
        "SEQ",
        "process-run-errors",
        build_process_run_errors_value(&bay.state),
    );
    bay.editor.runtime_mut().run_reactive_cycle();
    bay.editor.refresh_runtime_side_effects();
    assert_eq!(dot_active(&mut bay), Some(Value::Number(1.0)));
}

/// Expr presets (spec §6.1, bead eseq-waa9.15): the node bay's add menu
/// lists the library classes, then an "expr presets" heading over one row
/// per entry of the Lisp preset table. Picking a row through the menu's own
/// `:on-change` adds an expr card with that body committed and its inlets
/// at their starting values; every preset compiles.
#[test]
fn expr_card_presets_in_the_node_add_menu_add_committed_cards() {
    let mut bay = Bay::open();
    let strings = |value: Option<&Value>| -> Vec<String> {
        let Some(Value::List(items)) = value else { panic!("list: {value:?}") };
        items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::String(text) => text.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    };
    let layout = bay.layout();
    let add = find_layout_node_by_stable_key_suffix(&layout, "graph-variable-reset-proc-add-1")
        .expect("node bay add menu")
        .clone();
    assert_eq!(add.widget_type, "menu-button");
    let rows = strings(add.props.get("options"));
    let Some(Value::List(headers)) = add.props.get("headers") else { panic!("headers: {:?}", add.props.get("headers")) };
    assert_eq!(headers.len(), 1);
    let Value::Number(header) = *headers[0].borrow() else { panic!("header index") };
    let header = header as usize;
    assert_eq!(rows[header], "expr presets");
    let (classes, presets) = (&rows[..header], &rows[header + 1..]);
    assert!(classes.iter().any(|row| row == "expr"), "{classes:?}");
    assert_eq!(
        presets,
        &["× k", "+ k", "sin", "quant", "fold", "wrap", "scale 0..1 → lo..hi", "bounce", "lfsr"],
        "the preset table, in order"
    );
    for preset in presets {
        assert!(!classes.contains(preset), "preset label {preset} collides with a class label");
    }

    let on_change = add.props.get("on-change").cloned().expect("add menu on-change");
    let graph = bay.graph;
    let expected: [(&str, &str, &[(&str, f64)]); 9] = [
        ("× k", "(* in k)", &[("k", 1.0)]),
        ("+ k", "(+ in k)", &[("k", 1.0)]),
        ("sin", "(sin in)", &[]),
        ("quant", "(quant in step)", &[("step", 1.0)]),
        ("fold", "(fold in lo hi)", &[("lo", 0.0), ("hi", 1.0)]),
        ("wrap", "(wrap in lo hi)", &[("lo", 0.0), ("hi", 1.0)]),
        ("scale 0..1 → lo..hi", "(scale in 0 1 lo hi)", &[("lo", 0.0), ("hi", 1.0)]),
        ("bounce", "(* k (pow decay $n))", &[("k", 1.0), ("decay", 0.8)]),
        ("lfsr", "(state s 0xACE1)", &[("taps", 46080.0), ("grain", 1.0)]),
    ];
    for (index, (label, source, inlets)) in expected.iter().enumerate() {
        bay.editor
            .runtime_mut()
            .invoke(on_change.clone(), vec![Value::String(label.to_string())])
            .unwrap_or_else(|e| panic!("pick {label}: {e:?}"));
        bay.editor.runtime_mut().run_reactive_cycle();
        bay.editor.refresh_runtime_side_effects();
        let slot = |field: &str| format!("(get (nth (graph-node-process-chain {graph} 1) {index}) {field})");
        assert_eq!(bay.eval(&slot(":expr")), Some(Value::Bool(true)), "{label}: an expr card");
        let Some(Value::String(body)) = bay.eval(&slot(":expr-source")) else { panic!("{label}: no body") };
        assert!(body.starts_with(source), "{label}: {body}");
        assert_eq!(bay.eval(&slot(":error")), Some(Value::Nil), "{label}: compiles and runs clean");
        let Some(Value::String(class)) = bay.eval(&slot(":class")) else { panic!("{label}: class") };
        assert!(sequencer::process::is_expr_process_class(&class), "{label}: compiled class {class}");
        assert_eq!(bay.eval(&slot(":label")), Some(Value::String("expr".to_string())));
        // `in` has no stored value until set: a default-0 inlet.
        for (name, value) in *inlets {
            assert_eq!(
                bay.eval(&format!("(get {} :{name})", slot(":inlets"))),
                Some(Value::Number(*value)),
                "{label}: inlet {name}"
            );
        }
    }
    // The LFSR preset is the §9 body, multi-line, with its delay write.
    let Some(Value::String(lfsr)) = bay.eval(&format!("(get (nth (graph-node-process-chain {graph} 1) 8) :expr-source)"))
    else {
        panic!("lfsr body")
    };
    assert!(lfsr.contains('\n') && lfsr.contains("(delay! (* grain (bit-and s 7)))"), "{lfsr}");

    // The table is extensible: add-preset appends a row the menu shows.
    bay.eval("(eseq.expr-buffer/add-preset \"half\" \"(* in 0.5)\" (list))");
    let layout = bay.layout();
    let add = find_layout_node_by_stable_key_suffix(&layout, "graph-variable-reset-proc-add-1").expect("add menu");
    assert_eq!(strings(add.props.get("options")).last().map(String::as_str), Some("half"));
}

impl Bay {
    /// Type `$be` into the active buffer and return whether the completion
    /// popup offers `$beat`; closes the popup and restores the text.
    fn offers_beat(&mut self) -> bool {
        let before = self.editor.active_buffer().text();
        let dirty = self.editor.active_buffer().dirty;
        self.type_body("(+ $b");
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

    fn activate(&mut self, name: &str) {
        let id = self.editor.buffers.iter().find(|item| item.name == name).expect(name).id;
        self.editor.set_active_buffer(id);
        self.editor.runtime_mut().run_reactive_cycle();
        self.editor.refresh_runtime_side_effects();
    }

    /// Assert the active buffer is still an expr edit buffer: named mode,
    /// `$` completion, and a save that commits `body` to card `id`.
    fn assert_still_expr_buffer(&mut self, id: u64, body: &str, when: &str) {
        assert_eq!(
            self.eval("(current-buffer-mode)"),
            Some(Value::String("eseq.expr-buffer/expr-mode".to_string())),
            "{when}: the buffer keeps the expr mode"
        );
        assert!(self.offers_beat(), "{when}: $ completion still offers $beat");
        self.type_body(body);
        self.commit_chord();
        assert_eq!(self.source(id), Some(Value::String(body.to_string())), "{when}: C-c C-c commits");
        assert!(!self.editor.active_buffer().dirty, "{when}: the commit saves the buffer");
    }
}

/// The edit buffer keeps its mode and `$` completion across commits: a
/// commit (C-c C-c, and File > Save) bumps the node and re-lays the UI, and
/// the buffer must still complete `$beat` and commit again afterwards.
#[test]
fn expr_card_edit_buffer_keeps_completion_and_mode_after_commit() {
    let mut bay = Bay::open();
    let expr = bay.add("expr");
    bay.set_body(expr, "(* x 2)");
    bay.click(&format!("lane-patch-expr-edit-{expr}"));
    let name = bay.active_name();
    assert!(bay.offers_beat(), "fresh buffer offers $beat");

    bay.assert_still_expr_buffer(expr, "(+ x 1)", "after opening");
    bay.assert_still_expr_buffer(expr, "(+ x 2)", "after one commit");
    bay.type_body("(+ x 3)");
    bay.eval("(eseq.transport/file-menu-save)");
    assert_eq!(bay.source(expr), Some(Value::String("(+ x 3)".to_string())));
    bay.activate(&name);
    bay.assert_still_expr_buffer(expr, "(+ x 4)", "after File > Save");
}

/// Docked (eseq-waa9.22): the code tile's buffer keeps its mode and `$`
/// completion across a commit and across hide code / show code.
#[test]
fn expr_card_docked_code_keeps_completion_and_mode_across_commit_and_hide_show() {
    let mut bay = Bay::open();
    let (graph, buffer) = (bay.graph, bay.buffer.clone());
    bay.eval(&format!(
        "(do (eseq.seq-step-tabs/seq-register-instance-tab {graph} \"neural 1\" \"{buffer}\")
             (set! eseq.seq-step-tabs/step-panel-buffer \"{buffer}\"))"
    ));
    let expr = bay.eval(&format!(
        "(let ((id (graph-node-process-add {graph} 1 \"expr\"))) (do (eseq.sequencer/lane-patch-node-touch) id))"
    ));
    let Some(Value::Number(expr)) = expr else { panic!("add expr: {expr:?}") };
    let expr = expr as u64;
    bay.set_body(expr, "(* x 2)");
    bay.eval(&format!(
        "(eseq.sequencer/lane-patch-select-lane (eseq.sequencer/lane-patch-node-namespace (instance-ref {graph}) 1) 0 {expr})"
    ));
    bay.eval("(eseq.seq-layout/refresh-current-layout)");
    let name = "*expr node 1 · slot 1*";
    bay.activate(name);
    bay.assert_still_expr_buffer(expr, "(+ x 1)", "docked, first open");
    bay.assert_still_expr_buffer(expr, "(+ x 2)", "docked, after one commit");

    for round in 0..2 {
        bay.eval("(eseq.processes-buffer/toggle-code)");
        bay.eval("(eseq.seq-layout/refresh-current-layout)");
        assert_eq!(bay.eval("eseq.processes-buffer/code-visible"), Some(Value::Bool(false)));
        bay.eval("(eseq.processes-buffer/toggle-code)");
        bay.eval("(eseq.seq-layout/refresh-current-layout)");
        assert_eq!(bay.eval("eseq.processes-buffer/code-visible"), Some(Value::Bool(true)));
        bay.activate(name);
        bay.assert_still_expr_buffer(expr, &format!("(+ x {})", 10 + round), &format!("docked, hide/show {round}"));
    }
}

/// A Lisp hot reload re-evaluates the UI modules, re-running expr-buffer's
/// `define-mode`: the open edit buffer must keep its `$` completion (the
/// completions are registered lazily when a buffer opens, not at load) and
/// still commit.
#[test]
fn expr_card_edit_buffer_keeps_completion_across_a_lisp_hot_reload() {
    let mut bay = Bay::open();
    let expr = bay.add("expr");
    bay.set_body(expr, "(* x 2)");
    bay.click(&format!("lane-patch-expr-edit-{expr}"));
    let name = bay.active_name();
    assert!(bay.offers_beat(), "fresh buffer offers $beat");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/ui");
    for file in ["expr-buffer.lisp", "processes-buffer.lisp"] {
        let path = root.join(file).canonicalize().expect(file);
        let overlays = bay.editor.snapshot_file_backed_sources();
        let report = bay.editor.runtime_mut().reload_paths_transactional(vec![path], overlays);
        assert!(report.success, "{file}: {:?}", report.diagnostics);
        bay.editor.process_lisp_reload_report(report);
        bay.editor.runtime_mut().run_reactive_cycle();
        bay.editor.refresh_runtime_side_effects();
        bay.activate(&name);
        bay.assert_still_expr_buffer(expr, &format!("(+ x {})", file.len()), &format!("after reloading {file}"));
    }
}
