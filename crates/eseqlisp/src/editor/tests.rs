#[test]
fn ctrl_char_keys_are_normalized_to_lowercase() {
    let key = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL);
    assert_eq!(key_str(key), "C-c");
}

#[test]
fn ctrl_c_ctrl_c_binding_enqueues_host_command() {
    let init = r#"
            (def compile-current ()
              (host-command "compile-current" (dict :source (current-buffer-text))))
            (bind-key "C-c C-c" "compile-current")
        "#;
    let runtime = Runtime::with_init_source(init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "(+ 1 2)");

    editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

    let commands = editor.drain_host_commands();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        HostCommand::Custom { name, .. } if name == "compile-current"
    ));
}

#[test]
fn focused_text_input_survives_on_change_rerender() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def query (state ""))
            (effect
              (box :width 24 :height 3
                (text-input
                  :key "search-input"
                  :width 20
                  :value query
                  :on-change |v| (set! query v))))
            "#,
        )
        .expect("build text input");
    editor.refresh_runtime_side_effects();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(30, 8);

    let layout = editor.widget_layout().expect("layout");
    let input =
        super::find_layout_node_by_stable_key(layout.as_ref(), "search-input").expect("search input");
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            input.rect.col as u16,
            input.rect.row as u16,
        ),
        0,
        0,
        30,
        8,
        input.rect.col + 1.0,
        input.rect.row + 0.5,
    );
    let focused_before = editor.focused_widget_id().expect("input should focus");

    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.runtime.current_layout = None;
    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert_eq!(
        editor.runtime_mut().eval_str("query"),
        Ok(Some(Value::String("pi".to_string())))
    );
    assert_eq!(
        editor.focused_widget_id(),
        Some(focused_before),
        "text input focus should survive the on-change widget-tree refresh"
    );
}

#[test]
fn default_window_split_bindings_survive_runtime_sync() {
    let runtime = Runtime::with_init_source("(bind-key \"C-c C-c\" \"ignore\")");
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));

    assert_eq!(editor.tile_root.leaf_count(), 2);
}

#[test]
fn ctrl_x_ctrl_f_opens_find_file_minibuffer() {
    let init = include_str!("../../init.lisp").to_string();
    let runtime = Runtime::with_init_source(&init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));

    assert_eq!(editor.minibuffer_prompt().as_deref(), Some("Find file: "));
}

#[test]
fn find_file_minibuffer_opens_existing_file() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-find-file-open-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("open-me.lisp");
    std::fs::write(&path, "(+ 1 2)\n").unwrap();

    editor.minibuffer_input = Some(super::MinibufferMode::FindFile {
        input: path.display().to_string(),
        selected: 0,
    });
    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().path.as_ref(), Some(&path));
    assert_eq!(editor.active_buffer().text(), "(+ 1 2)\n");
}

#[test]
fn find_file_minibuffer_creates_new_file_backed_buffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-find-file-create-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("new-file.lisp");

    editor.minibuffer_input = Some(super::MinibufferMode::FindFile {
        input: path.display().to_string(),
        selected: 0,
    });
    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().path.as_ref(), Some(&path));
    assert_eq!(editor.active_buffer().text(), "");
    assert!(!path.exists(), "opening a new file should not write it yet");
}

#[test]
fn find_file_tab_completes_input_in_place() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-find-file-complete-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("font-demo.lisp");
    std::fs::write(&path, "(demo)\n").unwrap();
    editor.active_buffer_mut().path = Some(dir.join("anchor.lisp"));

    editor.minibuffer_input = Some(super::MinibufferMode::FindFile {
        input: "f".to_string(),
        selected: 0,
    });
    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(
        editor.minibuffer_prompt().as_deref(),
        Some("Find file: font-demo.lisp  [font-demo.lisp]")
    );
}

fn hot_reload_temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn widget_label_text(value: &Value) -> Option<String> {
    match value {
        Value::Map(map) => {
            if map
                .get("type")
                .is_some_and(|value| matches!(&*value.borrow(), Value::Keyword(kind) | Value::String(kind) if kind == "label"))
            {
                if let Some(text) = map.get("text") {
                    if let Value::String(text) = &*text.borrow() {
                        return Some(text.clone());
                    }
                }
            }
            map.get("children")
                .and_then(|children| match &*children.borrow() {
                    Value::List(children) => children
                        .iter()
                        .find_map(|child| widget_label_text(&child.borrow())),
                    _ => None,
                })
        }
        _ => None,
    }
}

fn widget_has_label_text(value: &Value, expected: &str) -> bool {
    match value {
        Value::Map(map) => {
            let is_matching_label = map
                .get("type")
                .is_some_and(|value| matches!(&*value.borrow(), Value::Keyword(kind) | Value::String(kind) if kind == "label"))
                && map.get("text").is_some_and(
                    |text| matches!(&*text.borrow(), Value::String(text) if text == expected),
                );
            is_matching_label
                || map
                    .get("children")
                    .is_some_and(|children| match &*children.borrow() {
                        Value::List(children) => children
                            .iter()
                            .any(|child| widget_has_label_text(&child.borrow(), expected)),
                        _ => false,
                    })
        }
        _ => false,
    }
}

fn layout_label_node<'a>(
    node: &'a crate::layout::LayoutNode,
    text: &str,
) -> Option<&'a crate::layout::LayoutNode> {
    if matches!(node.props.get("text"), Some(Value::String(value)) if value == text) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| layout_label_node(child, text))
}

#[test]
fn hot_reload_root_load_uses_dirty_child_overlay() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-root-overlay");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(
        &root,
        r#"(load "child.lisp")
(effect-buffer "*hot*" (label hot-label))"#,
    )
    .unwrap();
    std::fs::write(&child, r#"(def hot-label "disk")"#).unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(r#"(def hot-label "dirty")"#);
    editor.buffers[child_idx].dirty = true;

    let source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &source, overlays);
    assert!(report.success, "reload failed: {:?}", report.diagnostics);
    editor.process_lisp_reload_report(report);

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot*")
        .expect("hot buffer");
    assert_eq!(
        hot.widget_tree
            .as_ref()
            .and_then(widget_label_text)
            .as_deref(),
        Some("dirty")
    );
    let hot_id = hot.id;
    editor.set_active_buffer(hot_id);
    editor.update_tile_rects(80, 20);
    let layout = editor
        .runtime
        .current_layout
        .as_ref()
        .expect("hot reload should produce a visible layout");
    let label = layout_label_node(layout, "dirty").expect("dirty label should be measured");
    assert!(
        label.rect.width.is_finite()
            && label.rect.height.is_finite()
            && label.rect.width > 0.0
            && label.rect.height > 0.0,
        "hot reload label should have a finite nonzero rect, got {:?}",
        label.rect
    );
}

#[test]
fn hot_reload_replaces_module_graph_children_on_successful_root_eval() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-graph-edges");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(
        &root,
        r#"(load "child.lisp")
(effect-buffer "*hot-graph*" (label graph-label))"#,
    )
    .unwrap();
    std::fs::write(&child, r#"(def graph-label "child")"#).unwrap();

    let mut runtime = Runtime::new();
    let source = std::fs::read_to_string(&root).unwrap();
    let report = runtime.eval_source_transactional(Some(root.clone()), &source, Vec::new());
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );

    std::fs::write(
        &root,
        r#"(def graph-label "root")
(effect-buffer "*hot-graph*" (label graph-label))"#,
    )
    .unwrap();
    let source = std::fs::read_to_string(&root).unwrap();
    let report = runtime.eval_source_transactional(Some(root.clone()), &source, Vec::new());
    assert!(
        report.success,
        "root reload without child failed: {:?}",
        report.diagnostics
    );

    std::fs::write(&child, r#"(def graph-label "leaf")"#).unwrap();
    let report = runtime.reload_paths_transactional(vec![child.clone()], Vec::new());
    let canonical_child = std::fs::canonicalize(&child).unwrap();
    assert!(
        report.success,
        "leaf reload after root stopped loading it failed: {:?}",
        report.diagnostics
    );
    assert_eq!(
        report.evaluated_path.as_deref(),
        Some(canonical_child.as_path()),
        "stale graph edges must not escalate a detached leaf reload back to the old root"
    );
}

#[test]
fn hot_reload_active_named_buffer_syncs_active_tile_layout_cache() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-active-layout-sync");
    let root = dir.join("root.lisp");
    std::fs::write(&root, r#"(effect-buffer "*hot-active*" (label "before"))"#).unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.set_layout_viewport(80, 20);
    let source = std::fs::read_to_string(&root).unwrap();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &source, Vec::new());
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let hot_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-active*")
        .expect("hot-active buffer")
        .id;
    editor.set_active_buffer(hot_id);
    editor.update_tile_rects(80, 20);
    editor.sync_layout_to_active_leaf();
    let active_layout = editor
        .active_leaf()
        .cached_layout
        .as_deref()
        .expect("active tile should have an initial cached layout");
    assert!(layout_label_node(active_layout, "before").is_some());

    std::fs::write(&root, r#"(effect-buffer "*hot-active*" (label "after"))"#).unwrap();
    let report = editor
        .runtime_mut()
        .reload_paths_transactional(vec![root], Vec::new());
    assert!(
        report.success,
        "hot reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let active_layout = editor
        .active_leaf()
        .cached_layout
        .as_deref()
        .expect("active tile should keep a cached layout after hot reload");
    assert!(
        layout_label_node(active_layout, "after").is_some(),
        "active tile cache should reflect the hot-reloaded runtime layout without a buffer switch"
    );
}

#[test]
fn hot_reload_active_named_buffer_does_not_auto_focus_first_widget() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-active-no-autofocus");
    let root = dir.join("root.lisp");
    std::fs::write(
        &root,
        r#"(effect-buffer "*hot-focus*"
  (v-stack
    (dropdown
      :value "main"
      :options (list "main" "Bus B")
      :width 10
      :height 1.2
      :font-size 10)
    (label "after-dropdown")))
(define-mode "hot-focus-mode" :read-only true)
(set-buffer-mode-for "*hot-focus*" "hot-focus-mode")"#,
    )
    .unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.set_layout_viewport(80, 20);
    let source = std::fs::read_to_string(&root).unwrap();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &source, Vec::new());
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let hot_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-focus*")
        .expect("hot-focus buffer")
        .id;
    editor.set_active_buffer(hot_id);
    editor.update_tile_rects(80, 20);
    editor.sync_layout_to_active_leaf();
    assert_eq!(
        editor.focused_widget_id(),
        None,
        "activating a read-only UI buffer must not invent dropdown focus"
    );

    std::fs::write(
        &root,
        r#"(effect-buffer "*hot-focus*"
  (v-stack
    (dropdown
      :value "main"
      :options (list "main" "Bus B")
      :width 10
      :height 1.2
      :font-size 10)
    (label "after-reload")))
(define-mode "hot-focus-mode" :read-only true)
(set-buffer-mode-for "*hot-focus*" "hot-focus-mode")"#,
    )
    .unwrap();
    let report = editor
        .runtime_mut()
        .reload_paths_transactional(vec![root], Vec::new());
    assert!(
        report.success,
        "hot reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    assert_eq!(
        editor.focused_widget_id(),
        None,
        "hot reload must not focus the first dropdown when no widget was focused before"
    );
}

#[test]
fn hot_reload_leaf_eval_rerenders_owner_root_and_rolls_back_bad_source() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-leaf");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(
        &root,
        r#"(load "child.lisp")
(effect-buffer "*hot-leaf*" (label hot-label))"#,
    )
    .unwrap();
    std::fs::write(&child, r#"(def hot-label "disk")"#).unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(r#"(def hot-label "leaf")"#);
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child.clone()), &leaf_source, overlays);
    assert!(
        report.success,
        "leaf reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);
    let hot_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*hot-leaf*")
        .unwrap();
    assert_eq!(
        editor.buffers[hot_idx]
            .widget_tree
            .as_ref()
            .and_then(widget_label_text)
            .as_deref(),
        Some("leaf")
    );

    editor.buffers[child_idx].set_text(r#"(def hot-label "#);
    editor.buffers[child_idx].dirty = true;
    let bad_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor
        .runtime_mut()
        .eval_source_transactional(Some(child), &bad_source, overlays);
    assert!(!report.success, "bad leaf reload should fail");
    editor.process_lisp_reload_report(report);
    assert_eq!(
        editor.buffers[hot_idx]
            .widget_tree
            .as_ref()
            .and_then(widget_label_text)
            .as_deref(),
        Some("leaf"),
        "failed reload must leave the last rendered tree committed"
    );
}

#[test]
fn hot_reload_effect_buffer_body_update_survives_dependency_rerender() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-effect-body");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(def hot-label "old")
(effect-buffer "*hot-body*" (label hot-label))"#,
    )
    .unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(
        r#"(def hot-label "new")
(effect-buffer "*hot-body*"
  (v-stack
    (label hot-label)
    (label "body-new")))"#,
    );
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child), &leaf_source, overlays);
    assert!(
        report.success,
        "body reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-body*")
        .expect("hot-body buffer");
    assert!(
        hot.widget_tree
            .as_ref()
            .is_some_and(|tree| widget_has_label_text(tree, "body-new")),
        "dependency rerender must keep widgets from the reloaded effect-buffer body, not the stale chunk"
    );
}

#[test]
fn hot_reload_effect_buffer_keeps_reactive_dependencies_after_body_reload() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-effect-reactive-deps");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(effect-buffer "*hot-reactive*" (label (fmt "count: {}" APP.count)))"#,
    )
    .unwrap();

    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("count", Value::Number(1.0))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(
        r#"(effect-buffer "*hot-reactive*"
  (v-stack
    (label (fmt "count: {}" APP.count))
    (label "after-reload")))"#,
    );
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child), &leaf_source, overlays);
    assert!(
        report.success,
        "effect body reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let outcome = editor
        .runtime_mut()
        .set_reactive("APP", "count", Value::Number(2.0));
    assert!(
        outcome.effects_dirty,
        "reloaded effect-buffer should stay subscribed to APP.count"
    );
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-reactive*")
        .expect("hot-reactive buffer");
    assert!(
        hot.widget_tree
            .as_ref()
            .is_some_and(|tree| widget_has_label_text(tree, "count: 2")),
        "reactive update should rerender the hot-reloaded effect-buffer body"
    );
    assert!(
        hot.widget_tree
            .as_ref()
            .is_some_and(|tree| widget_has_label_text(tree, "after-reload")),
        "reactive rerender should preserve widgets introduced by the hot-reloaded body"
    );
}

#[test]
fn hot_reload_effect_buffer_keeps_subtree_reactive_dependencies_after_body_reload() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-subtree-reactive-deps");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(effect-buffer "*hot-subtree-reactive*"
  (v-stack
    (subtree :key "row"
      (label (fmt "count: {}" APP.count)))))"#,
    )
    .unwrap();

    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("count", Value::Number(1.0))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(
        r#"(effect-buffer "*hot-subtree-reactive*"
  (v-stack
    (subtree :key "row"
      (v-stack
        (label (fmt "count: {}" APP.count))
        (label "subtree-after-reload")))))"#,
    );
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child), &leaf_source, overlays);
    assert!(
        report.success,
        "subtree body reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let outcome = editor
        .runtime_mut()
        .set_reactive("APP", "count", Value::Number(2.0));
    assert!(
        outcome.effects_dirty,
        "reloaded subtree should stay subscribed to APP.count"
    );
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-subtree-reactive*")
        .expect("hot-subtree-reactive buffer");
    assert!(
        hot.widget_tree
            .as_ref()
            .is_some_and(|tree| widget_has_label_text(tree, "count: 2")),
        "reactive update should rerender the hot-reloaded subtree"
    );
    assert!(
        hot.widget_tree
            .as_ref()
            .is_some_and(|tree| widget_has_label_text(tree, "subtree-after-reload")),
        "subtree reactive rerender should preserve widgets introduced by hot reload"
    );
}

#[test]
fn hot_reload_replaces_module_subtree_effects_instead_of_accumulating_them() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-subtree-effect-count");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(effect-buffer "*hot-subtree-count*"
  (v-stack
    (subtree :key "row"
      (label (fmt "count: {}" APP.count)))))"#,
    )
    .unwrap();

    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("count", Value::Number(1.0))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let child_module = std::fs::canonicalize(&child).unwrap_or_else(|_| child.clone());

    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);
    assert_eq!(
        editor
            .runtime()
            .debug_effect_count_for_module(&child_module),
        2,
        "initial child module should own one top-level effect and one subtree effect"
    );

    let hot_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-subtree-count*")
        .map(|buffer| buffer.id)
        .expect("hot-subtree-count buffer");
    editor.set_active_buffer(hot_id);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    for revision in 0..3 {
        editor.buffers[child_idx].set_text(&format!(
            r#"(effect-buffer "*hot-subtree-count*"
  (v-stack
    (subtree :key "row"
      (v-stack
        (label (fmt "count: {{}}" APP.count))
        (label "reload-{revision}")))))"#
        ));
        editor.buffers[child_idx].dirty = true;
        let leaf_source = editor.buffers[child_idx].text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(child.clone()),
            &leaf_source,
            overlays,
        );
        assert!(
            report.success,
            "child reload {revision} failed: {:?}",
            report.diagnostics
        );
        editor.process_lisp_reload_report(report);
        assert_eq!(
            editor
                .runtime()
                .debug_effect_count_for_module(&child_module),
            2,
            "reload {revision} should replace stale child subtree effects, not accumulate them"
        );
    }

    let outcome = editor
        .runtime_mut()
        .set_reactive("APP", "count", Value::Number(2.0));
    assert!(
        outcome.effects_dirty,
        "remaining subtree effect should stay subscribed to APP.count"
    );
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let tree = editor
        .runtime
        .current_widget_tree()
        .expect("active hot-subtree-count tree");
    assert!(
        widget_has_label_text(&tree, "reload-2"),
        "reactive rerender should use the latest reloaded subtree body"
    );
}

#[test]
fn hot_reload_active_effect_buffer_updates_after_reactive_change() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-active-reactive");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(effect-buffer "*hot-active-reactive*"
  (v-stack
    (subtree :key "row"
      (label (fmt "count: {}" APP.count)))))"#,
    )
    .unwrap();

    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("count", Value::Number(1.0))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let hot_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-active-reactive*")
        .map(|buffer| buffer.id)
        .expect("hot-active-reactive buffer");
    editor.set_active_buffer(hot_id);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(
        r#"(effect-buffer "*hot-active-reactive*"
  (v-stack
    (subtree :key "row"
      (v-stack
        (label (fmt "count: {}" APP.count))
        (label "active-after-reload")))))"#,
    );
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child), &leaf_source, overlays);
    assert!(
        report.success,
        "active body reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);
    editor.set_active_buffer(hot_id);

    let outcome = editor
        .runtime_mut()
        .set_reactive("APP", "count", Value::Number(2.0));
    assert!(
        outcome.effects_dirty,
        "active reloaded subtree should stay subscribed to APP.count"
    );
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let tree = editor
        .runtime
        .current_widget_tree()
        .expect("active hot buffer tree");
    assert!(
        widget_has_label_text(&tree, "count: 2"),
        "active visible runtime tree should update after reactive change"
    );
    assert!(
        widget_has_label_text(&tree, "active-after-reload"),
        "active visible runtime tree should keep hot-reloaded body widgets"
    );
}

#[test]
fn hot_reload_active_effect_buffer_keeps_selection_and_button_state_reactive() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-active-selection-state");
    let root = dir.join("root.lisp");
    let child = dir.join("child.lisp");
    std::fs::write(&root, r#"(load "child.lisp")"#).unwrap();
    std::fs::write(
        &child,
        r#"(effect-buffer "*hot-selection*"
  (v-stack
    (subtree :key "track-0"
      (v-stack
        (label (if (= APP.selected 0) "track-0-selected" "track-0-idle"))
        (label (if APP.muted "track-0-muted" "track-0-unmuted"))))
    (subtree :key "track-1"
      (label (if (= APP.selected 1) "track-1-selected" "track-1-idle")))))"#,
    )
    .unwrap();

    let mut runtime = Runtime::new();
    runtime.register_reactive(
        "APP",
        vec![
            ("selected", Value::Number(0.0)),
            ("muted", Value::Bool(false)),
        ],
        true,
    );
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    editor
        .create_file_buffer(&child, BufferMode::ESeqLisp)
        .unwrap();
    let root_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &root_source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    let hot_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-selection*")
        .map(|buffer| buffer.id)
        .expect("hot-selection buffer");
    editor.set_active_buffer(hot_id);

    let child_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&child))
        .unwrap();
    editor.buffers[child_idx].set_text(
        r#"(effect-buffer "*hot-selection*"
  (v-stack
    (subtree :key "track-0"
      (v-stack
        (label (if (= APP.selected 0) "track-0-selected" "track-0-idle"))
        (label (if APP.muted "track-0-muted" "track-0-unmuted"))
        (label "after-reload")))
    (subtree :key "track-1"
      (label (if (= APP.selected 1) "track-1-selected" "track-1-idle")))))"#,
    );
    editor.buffers[child_idx].dirty = true;
    let leaf_source = editor.buffers[child_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(child), &leaf_source, overlays);
    assert!(
        report.success,
        "active selection reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);
    editor.set_active_buffer(hot_id);

    editor
        .runtime_mut()
        .set_reactive("APP", "selected", Value::Number(1.0));
    editor
        .runtime_mut()
        .set_reactive("APP", "muted", Value::Bool(true));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let tree = editor
        .runtime
        .current_widget_tree()
        .expect("active hot-selection tree");
    assert!(
        widget_has_label_text(&tree, "track-1-selected"),
        "active selection subtree should update after hot reload"
    );
    assert!(
        widget_has_label_text(&tree, "track-0-muted"),
        "active button-style subtree should update after hot reload"
    );
    assert!(
        widget_has_label_text(&tree, "after-reload"),
        "reactive update should preserve hot-reloaded subtree body"
    );
}

#[test]
fn hot_reload_preserves_existing_defstate_values() {
    let dir = hot_reload_temp_dir("eseqlisp-hot-defstate");
    let root = dir.join("root.lisp");
    std::fs::write(
        &root,
        r#"(defstate hot-state "initial")
(effect-buffer "*hot-state*" (label hot-state))"#,
    )
    .unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_or_create_file_buffer(&root).unwrap();
    let source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report =
        editor
            .runtime_mut()
            .eval_source_transactional(Some(root.clone()), &source, overlays);
    assert!(
        report.success,
        "initial reload failed: {:?}",
        report.diagnostics
    );
    editor.process_lisp_reload_report(report);

    editor
        .runtime
        .eval_str(r#"(set! hot-state "changed")"#)
        .unwrap();
    editor.runtime.run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let root_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&root))
        .unwrap();
    editor.buffers[root_idx].set_text(
        r#"(defstate hot-state "new-initial")
(effect-buffer "*hot-state*" (label hot-state))"#,
    );
    editor.buffers[root_idx].dirty = true;
    let source = editor.buffers[root_idx].text();
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor
        .runtime_mut()
        .eval_source_transactional(Some(root), &source, overlays);
    assert!(report.success, "reload failed: {:?}", report.diagnostics);
    editor.process_lisp_reload_report(report);

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-state*")
        .expect("hot-state buffer");
    assert_eq!(
        hot.widget_tree
            .as_ref()
            .and_then(widget_label_text)
            .as_deref(),
        Some("changed")
    );
}

#[test]
fn eval_current_buffer_native_uses_transactional_reload() {
    let dir = hot_reload_temp_dir("eseqlisp-eval-current-buffer");
    let root = dir.join("root.lisp");
    std::fs::write(
        &root,
        r#"(defstate hot-state "initial")
(effect-buffer "*hot-state*" (label hot-state))"#,
    )
    .unwrap();

    let init = r#"
        (bind-key "C-x C-b" "eval-current-buffer")
    "#;
    let mut editor = Editor::new(
        Runtime::new(),
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_or_create_file_buffer(&root).unwrap();
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

    editor
        .runtime
        .eval_str(r#"(set! hot-state "changed")"#)
        .unwrap();
    editor.runtime.run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let root_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.path.as_ref() == Some(&root))
        .unwrap();
    editor.buffers[root_idx].set_text(
        r#"(defstate hot-state "new-initial")
(effect-buffer "*hot-state*" (label hot-state))"#,
    );
    editor.buffers[root_idx].dirty = true;
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

    let hot = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*hot-state*")
        .expect("hot-state buffer");
    assert_eq!(
        hot.widget_tree
            .as_ref()
            .and_then(widget_label_text)
            .as_deref(),
        Some("changed")
    );
}

#[test]
fn dragging_vertical_tile_border_updates_split_ratio() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");
    editor.split_active_tile(SplitDir::Vertical, 0);
    editor.update_tile_rects(100, 40);

    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 50, 10),
        50.0,
        10.0,
        1,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 70, 10),
        70.0,
        10.0,
        1,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 70, 10),
        70.0,
        10.0,
        1,
    );

    let super::TileNode::Split(split) = &editor.tile_root else {
        panic!("expected root split");
    };
    assert!(
        (split.ratio - 0.7).abs() < 0.05,
        "ratio was {}",
        split.ratio
    );
}

#[test]
fn dragging_horizontal_tile_border_updates_split_ratio() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");
    editor.split_active_tile(SplitDir::Horizontal, 0);
    editor.update_tile_rects(80, 40);

    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 20, 19),
        20.0,
        19.0,
        1,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 20, 8),
        20.0,
        8.0,
        1,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 20, 8),
        20.0,
        8.0,
        1,
    );

    let super::TileNode::Split(split) = &editor.tile_root else {
        panic!("expected root split");
    };
    assert!(
        (split.ratio - (8.0 / 39.0)).abs() < 0.05,
        "ratio was {}",
        split.ratio
    );
}

#[test]
fn hover_scroll_switches_focus_to_hovered_tile() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*left*", &vec!["left"; 40].join("\n"));
    let right_tile = editor
        .split_active_tile(SplitDir::Vertical, editor.active_buffer_idx())
        .expect("split should create a second tile");
    editor.switch_active_tile(right_tile);
    editor.open_scratch_buffer("*right*", &vec!["right"; 40].join("\n"));

    let left_tile = editor.tile_root.leaf_ids()[0];
    editor.switch_active_tile(left_tile);
    editor.update_tile_rects(90, 30);

    let right_rect = editor.tile_rect(right_tile).expect("right tile rect");
    let hover_col = right_rect.col + 2.0;
    let hover_row = right_rect.row + 2.0;

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::ScrollDown,
            hover_col as u16,
            hover_row as u16,
        ),
        hover_col,
        hover_row,
        1,
    );

    let left_buffer_idx = editor
        .tile_root
        .find_leaf(left_tile)
        .expect("left tile")
        .buffer_idx;
    let right_buffer_idx = editor
        .tile_root
        .find_leaf(right_tile)
        .expect("right tile")
        .buffer_idx;

    assert_eq!(
        editor.active_tile, right_tile,
        "hover scroll should focus hovered tile"
    );
    assert_eq!(editor.buffers[left_buffer_idx].scroll_top, 0);
    assert_eq!(editor.buffers[right_buffer_idx].scroll_top, 3);
}

#[test]
fn clicking_inactive_tile_uses_target_tile_viewport_for_hit_testing() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*left*", "(label :text \"left\")");
    let right_tile = editor
        .split_active_tile(SplitDir::Vertical, editor.active_buffer_idx())
        .expect("split should create a second tile");

    editor.switch_active_tile(right_tile);
    editor
        .runtime
        .eval_str(
            r#"
                (def enabled (state true))
                (effect
                  (toggle :bind enabled))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(8, 4);
    editor.switch_active_tile(editor.tile_root.leaf_ids()[0]);
    editor.update_tile_rects(90, 30);

    let right_rect = editor.tile_rect(right_tile).expect("right tile rect");
    let (content_col, content_row, _, _) = editor
        .tile_content_area(right_tile, 1)
        .expect("right tile content area");
    let precise_col = (right_rect.col + 1.5).max(content_col as f32);
    let precise_row = (right_rect.row + 1.5).max(content_row as f32);

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        1,
    );

    assert_eq!(
        editor.active_tile, right_tile,
        "click should select hovered tile"
    );
    assert_eq!(
        editor.runtime.eval_str("enabled").unwrap(),
        Some(Value::Bool(false)),
        "first click in newly selected tile should hit the widget under the pointer"
    );
}

#[test]
fn ctrl_a_moves_to_start_of_line() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abcdef");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (0, 0));
}

#[test]
fn ctrl_e_moves_to_end_of_line() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abcdef");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (0, 6));
}

#[test]
fn ctrl_f_moves_forward_one_page() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let text = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &text);
    editor.set_layout_viewport(20, 6);
    editor.active_buffer_mut().cursor = (2, 2);

    editor.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (8, 2));
    assert_eq!(editor.active_buffer().scroll_top, 6);
}

#[test]
fn ctrl_b_moves_backward_one_page() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let text = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &text);
    editor.set_layout_viewport(20, 6);
    editor.active_buffer_mut().cursor = (14, 2);
    editor.active_buffer_mut().scroll_top = 12;

    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (8, 2));
    assert_eq!(editor.active_buffer().scroll_top, 6);
}

#[test]
fn ctrl_l_recenters_cursor_without_moving_it() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let text = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &text);
    editor.set_layout_viewport(20, 6);
    editor.active_buffer_mut().cursor = (10, 2);
    editor.active_buffer_mut().scroll_top = 0;

    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (10, 2));
    assert_eq!(editor.active_buffer().scroll_top, 7);
}

#[test]
fn mouse_scroll_does_not_move_cursor_or_snap_back_on_render() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let text = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &text);
    editor.set_layout_viewport(20, 6);
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 1, 1), 1, 1, 20, 6);

    assert_eq!(editor.active_buffer().cursor, (0, 0));
    assert_eq!(editor.active_buffer().scroll_top, 3);

    let _frame = crate::ui::frame::build_render_frame(&mut editor, 20, 6);

    assert_eq!(editor.active_buffer().cursor, (0, 0));
    assert_eq!(editor.active_buffer().scroll_top, 3);
}

#[test]
fn ctrl_s_opens_incremental_search_in_minibuffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    assert_eq!(editor.minibuffer_prompt().as_deref(), Some("I-search: "));
}

#[test]
fn incremental_search_moves_cursor_and_ctrl_s_repeats_forward() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta alpha beta");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 0));

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(editor.active_buffer().cursor, (0, 11));

    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(editor.active_buffer().cursor, (0, 0));
}

#[test]
fn incremental_search_scrolls_to_match() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let text = (0..20)
        .map(|i| {
            if i == 12 {
                "needle".to_string()
            } else {
                format!("line {i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &text);
    editor.set_layout_viewport(20, 6);

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    for c in "needle".chars() {
        editor.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    assert_eq!(editor.active_buffer().cursor, (12, 0));
    assert_eq!(editor.active_buffer().scroll_top, 7);
}

#[test]
fn search_escape_restores_original_cursor() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta alpha");
    editor.active_buffer_mut().cursor = (0, 7);

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 11));

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 7));
    assert_eq!(editor.minibuffer_prompt(), None);
}

#[test]
fn search_arrow_key_exits_search_mode_and_allows_normal_editing() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta alpha");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (0, 11));
    assert_eq!(
        editor.minibuffer_prompt().as_deref(),
        Some("I-search: alpha")
    );

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 12));
    assert_eq!(editor.minibuffer_prompt(), None);

    editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "alpha beta azlpha");
}

#[test]
fn search_enter_exits_search_mode_without_editing_buffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta alpha");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 11));
    assert_eq!(editor.active_buffer().text(), "alpha beta alpha");
    assert_eq!(editor.minibuffer_prompt(), None);

    editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "alpha beta zalpha");
}

#[test]
fn search_mouse_click_that_moves_cursor_exits_search_mode() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha beta\ngamma delta");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    for c in "delta".chars() {
        editor.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(editor.active_buffer().cursor, (1, 6));
    assert!(editor.minibuffer_prompt().is_some());

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 1),
        1,
        1,
        20,
        10,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 2));
    assert_eq!(editor.minibuffer_prompt(), None);
}

#[test]
fn search_mouse_scroll_exits_search_mode() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "one\ntwo\nthree\nfour\nfive");

    editor.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    for c in "five".chars() {
        editor.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(editor.active_buffer().cursor, (4, 0));
    assert!(editor.minibuffer_prompt().is_some());

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 1, 1), 1, 1, 20, 3);

    assert_eq!(editor.minibuffer_prompt(), None);
}

#[test]
fn meta_period_jumps_to_definition_and_meta_comma_returns() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let defs_id = editor.open_scratch_buffer("*defs*", "(def target 42)\n(def other 9)");
    let callsite_id = editor.open_scratch_buffer("*main*", "(target)");
    editor.set_active_buffer(callsite_id);
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT));

    assert_eq!(editor.active_buffer().id, defs_id);
    assert_eq!(editor.active_buffer().cursor, (0, 5));

    editor.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::ALT));

    assert_eq!(editor.active_buffer().id, callsite_id);
    assert_eq!(editor.active_buffer().cursor, (0, 1));
}

#[test]
fn meta_period_opens_definition_from_workspace_file() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-goto-definition-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let defs_path = dir.join("defs.lisp");
    let main_path = dir.join("main.lisp");
    std::fs::write(&defs_path, "(def remote-target 99)\n").unwrap();
    std::fs::write(&main_path, "(remote-target)\n").unwrap();

    editor.open_file_buffer(&main_path).unwrap();
    editor.active_buffer_mut().cursor = (0, 2);

    editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT));

    assert_eq!(editor.active_buffer().path.as_ref(), Some(&defs_path));
    assert_eq!(editor.active_buffer().cursor, (0, 5));
}

#[test]
fn esc_period_and_esc_comma_work_as_meta_definition_bindings() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let defs_id = editor.open_scratch_buffer("*defs*", "(def target 42)");
    let callsite_id = editor.open_scratch_buffer("*main*", "(target)");
    editor.set_active_buffer(callsite_id);
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, defs_id);
    assert_eq!(editor.active_buffer().cursor, (0, 5));

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, callsite_id);
    assert_eq!(editor.active_buffer().cursor, (0, 1));
}

#[test]
fn ctrl_left_moves_to_previous_word() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 10);

    editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (0, 8));
}

#[test]
fn ctrl_right_moves_to_next_word() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (0, 3));
}

#[test]
fn alt_left_moves_to_previous_word() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 10);

    editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));

    assert_eq!(editor.active_buffer().cursor, (0, 8));
}

#[test]
fn alt_right_moves_to_next_word() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));

    assert_eq!(editor.active_buffer().cursor, (0, 3));
}

#[test]
fn ctrl_w_deletes_previous_word() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 8);

    editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().text(), "abc ghi");
    assert_eq!(editor.active_buffer().cursor, (0, 4));
}

#[test]
fn ctrl_w_kills_active_region() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().text(), " def ghi");
    assert_eq!(editor.active_buffer().cursor, (0, 0));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn backspace_deletes_active_region() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), " def");
    assert_eq!(editor.active_buffer().cursor, (0, 0));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn alt_w_copies_region_and_ctrl_y_yanks_it() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
    editor.active_buffer_mut().cursor = (0, 7);
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().text(), "abc defabc");
}

#[test]
fn cmd_c_copies_region_to_system_clipboard() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_test_clipboard("");
    editor.open_scratch_buffer("*test*", "abc def");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER));

    assert_eq!(editor.test_clipboard(), Some("abc"));
    assert_eq!(editor.active_buffer().text(), "abc def");
    assert_eq!(editor.active_region_range(), Some(((0, 0), (0, 3))));
}

#[test]
fn cmd_v_pastes_system_clipboard_and_replaces_region() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_test_clipboard("XYZ");
    editor.open_scratch_buffer("*test*", "abc def");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER));

    assert_eq!(editor.active_buffer().text(), "XYZ def");
    assert_eq!(editor.active_buffer().cursor, (0, 3));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn typing_clears_active_mark() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc");
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(editor.active_region_range().is_some());

    editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn vim_mode_starts_in_normal_and_does_not_insert_plain_keys() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");

    editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "abc");
    assert_eq!(editor.vim_input_mode, VimInputMode::Normal);
}

#[test]
fn vim_insert_mode_accepts_text_and_escape_returns_to_normal() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SHIFT));
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "aZbc");
    assert_eq!(editor.vim_input_mode, VimInputMode::Normal);
}

#[test]
fn vim_normal_mode_supports_basic_motions() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc def\nxyz");

    editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().cursor, (0, 4));

    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().cursor, (0, 0));

    editor.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().cursor, (1, 0));

    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().cursor, (0, 0));
}

#[test]
fn vim_normal_mode_accepts_shifted_motions_without_inserting() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc\ndef");

    editor.handle_key(KeyEvent::new(KeyCode::Char('$'), KeyModifiers::SHIFT));
    assert_eq!(editor.active_buffer().text(), "abc\ndef");
    assert_eq!(editor.active_buffer().cursor, (0, 3));

    editor.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
    assert_eq!(editor.active_buffer().text(), "abc\ndef");
    assert_eq!(editor.active_buffer().cursor, (1, 2));
}

#[test]
fn vim_dd_and_p_are_linewise() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "one\ntwo\nthree");
    editor.active_buffer_mut().cursor = (1, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "one\nthree");
    assert_eq!(editor.active_buffer().cursor, (1, 1));

    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "one\nthree\ntwo");
    assert_eq!(editor.active_buffer().cursor, (2, 0));
}

#[test]
fn vim_delete_line_operator_accepts_count_between_keys() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "one\ntwo\nthree\nfour");
    editor.active_buffer_mut().cursor = (1, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "one\nfour");
    assert_eq!(editor.active_buffer().cursor, (1, 0));
}

#[test]
fn vim_yank_line_operator_accepts_count_between_keys() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "one\ntwo\nthree\nfour");
    editor.active_buffer_mut().cursor = (1, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));

    assert_eq!(
        editor.active_buffer().text(),
        "one\ntwo\ntwo\nthree\nthree\nfour"
    );
    assert_eq!(editor.active_buffer().cursor, (2, 0));
}

#[test]
fn vim_delete_word_operator_accepts_count_between_keys() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "one two three four");

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "three four");
    assert_eq!(editor.active_buffer().cursor, (0, 0));
}

#[test]
fn vim_y_yanks_active_selection() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc def");
    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    editor.active_buffer_mut().cursor = (0, 7);
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "abc defabc");
    assert!(editor.active_region_range().is_none());
}

#[test]
fn vim_d_deletes_active_selection() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc def");
    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), " def");
    assert_eq!(editor.active_buffer().cursor, (0, 0));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn vim_r_replaces_character_under_cursor() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "axc");
    assert_eq!(editor.active_buffer().cursor, (0, 1));
    assert_eq!(editor.vim_input_mode, VimInputMode::Normal);
}

#[test]
fn vim_r_accepts_shifted_replacement_character() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));

    assert_eq!(editor.active_buffer().text(), "aXc");
    assert_eq!(editor.active_buffer().cursor, (0, 1));
}

#[test]
fn vim_escape_clears_selection_before_changing_mode() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");
    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(editor.active_region_range().is_some());

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(editor.active_region_range().is_none());
    assert_eq!(editor.vim_input_mode, VimInputMode::Insert);
}

#[test]
fn vim_undo_and_redo_restore_text_edits() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "abc");
    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SHIFT));
    assert_eq!(editor.active_buffer().text(), "Zabc");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "abc");

    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(editor.active_buffer().text(), "Zabc");
}

#[test]
fn plain_typing_coalesces_into_one_undo_snapshot() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "");

    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    for ch in ['a', 'b', 'c'] {
        editor.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(editor.active_buffer().text(), "abc");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "");
}

#[test]
fn cursor_movement_starts_a_new_typing_undo_group() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "");

    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "ba");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "a");
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "");
}

#[test]
fn ctrl_k_deletes_rest_of_line() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc def ghi");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().text(), "abc ");
    assert_eq!(editor.active_buffer().cursor, (0, 4));
}

#[test]
fn tab_accepts_completion_from_runtime_symbols() {
    let mut runtime = Runtime::new();
    runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(seq");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(seq-step");
}

#[test]
fn vim_normal_mode_tab_still_accepts_completion() {
    let mut runtime = Runtime::new();
    runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "(seq-");
    editor.active_buffer_mut().cursor = (0, 5);

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(seq-step");
}

#[test]
fn tab_indents_current_line_when_no_completion_matches() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(if test\n:4t)");
    editor.active_buffer_mut().cursor = (1, 0);

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(if test\n  :4t)");
    assert_eq!(editor.active_buffer().cursor, (1, 2));
}

#[test]
fn enter_inserts_lisp_indentation() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(if test)");
    editor.active_buffer_mut().cursor = (0, 8);

    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(if test\n  )");
    assert_eq!(editor.active_buffer().cursor, (1, 2));
}

#[test]
fn scratch_mode_defaults_to_eseqlisp() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer_with_mode("*dsp*", "(param freq 440)", BufferMode::DGenLisp);

    assert_eq!(editor.active_buffer().mode, BufferMode::DGenLisp);
}

#[test]
fn valid_patcher_payload_updates_read_only_emitted_source_buffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = "/tmp/eseq/example/dsp.lisp";
    let source = "(def out (out 1 0))";
    let payload = Value::Map(HashMap::from([
        (
            "status".to_string(),
            Rc::new(RefCell::new(Value::Keyword("valid".to_string()))),
        ),
        (
            "path".to_string(),
            Rc::new(RefCell::new(Value::String(path.to_string()))),
        ),
        (
            "source".to_string(),
            Rc::new(RefCell::new(Value::String(source.to_string()))),
        ),
    ]));

    let id = editor
        .sync_patcher_emitted_source_buffer(&[payload])
        .expect("valid patcher payload should create emitted source buffer");
    let buffer = editor
        .buffers
        .iter()
        .find(|buffer| buffer.id == id)
        .expect("emitted source buffer");

    assert_eq!(
        buffer.name,
        crate::widget_render::patcher::emitted_source_buffer_name(path)
    );
    assert_eq!(buffer.text(), source);
    assert_eq!(buffer.mode, BufferMode::DGenLisp);
    assert!(buffer.read_only);
    assert!(!buffer.dirty);
}

#[test]
fn tab_from_patcher_buffer_toggles_emitted_source_split() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = temp_file_path("patcher-source-split-toggle");
    std::fs::write(&path, "(def sig (in 1))\n(out sig 1)\n").unwrap();
    let path = path.to_string_lossy().to_string();
    let patcher_id = editor.open_scratch_buffer("*patcher*", "");
    let patcher_tree = Value::Map(HashMap::from([
        (
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("patcher".to_string()))),
        ),
        (
            "path".to_string(),
            Rc::new(RefCell::new(Value::String(path.clone()))),
        ),
    ]));
    editor
        .active_buffer_mut()
        .set_widget_tree(Some(patcher_tree), Some(patcher_id));
    let emitted_id = editor.upsert_read_only_scratch_buffer_with_mode(
        &crate::widget_render::patcher::emitted_source_buffer_name(&path),
        "(def out (out 1 0))",
        BufferMode::DGenLisp,
    );
    assert_ne!(patcher_id, emitted_id);
    editor.set_active_buffer(patcher_id);
    assert_eq!(editor.active_buffer().id, patcher_id);
    assert!(editor.patcher_source_tab_available());

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, patcher_id);
    assert_eq!(editor.tile_root.leaf_count(), 2);
    assert!(
        editor.tile_root.leaf_ids().into_iter().any(|tile_id| {
            editor
                .tile_root
                .find_leaf(tile_id)
                .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
                .is_some_and(|buffer| buffer.id == emitted_id)
        }),
        "source buffer should be visible in a sibling tile"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, patcher_id);
    assert_eq!(editor.tile_root.leaf_count(), 1);
}

#[test]
fn tab_from_split_patcher_source_tile_hides_source_and_returns_to_patcher() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = temp_file_path("patcher-source-split-hide-from-source");
    std::fs::write(&path, "(def sig (in 1))\n(out sig 1)\n").unwrap();
    let path = path.to_string_lossy().to_string();
    let patcher_id = editor.open_scratch_buffer("*patcher*", "");
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(&format!(
            r#"
                (effect
                  (patcher
                    :height 10
                    :intent :effect
                    :path "{}"))
                "#,
            path
        ))
        .unwrap();
    editor.set_layout_viewport(40, 12);

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(editor.tile_root.leaf_count(), 2);

    let emitted_name = crate::widget_render::patcher::emitted_source_buffer_name(&path);
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| {
            editor
                .tile_root
                .find_leaf(*tile_id)
                .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
                .is_some_and(|buffer| buffer.name == emitted_name)
        })
        .expect("source tile should be visible");
    editor.switch_active_tile(source_tile);
    assert_eq!(editor.active_buffer().name, emitted_name);

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, patcher_id);
    assert_eq!(editor.tile_root.leaf_count(), 1);
}

#[test]
fn upsert_patcher_emitted_source_buffer_preserves_active_buffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = temp_file_path("patcher-host-toggle");
    editor.open_scratch_buffer("*instrument-patcher:test*", "");
    let other_id = editor.open_scratch_buffer("*samples*", "");

    let source_buffer_name = editor
        .upsert_patcher_emitted_source_buffer(
            "*instrument-patcher:test*",
            &path,
            "(def out (out 1 0))",
        )
        .expect("upsert should create emitted source buffer");

    assert_eq!(editor.active_buffer().id, other_id);
    assert_eq!(
        source_buffer_name,
        crate::widget_render::patcher::emitted_source_buffer_name(&path.to_string_lossy())
    );
    let source_buffer = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == source_buffer_name)
        .expect("source buffer");
    assert!(source_buffer.read_only);
}

#[test]
fn cursor_movement_closes_completion_popup() {
    let mut runtime = Runtime::new();
    runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(seq");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
    assert!(editor.completion_state().is_some());

    editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(editor.completion_state().is_none());
}

#[test]
fn key_release_does_not_close_completion_popup() {
    let mut runtime = Runtime::new();
    runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(seq");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
    assert!(editor.completion_state().is_some());

    editor.handle_key(KeyEvent {
        code: KeyCode::Char('-'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    });

    assert_eq!(editor.active_buffer().text(), "(seq-");
    assert!(editor.completion_state().is_some());
}

#[test]
fn typing_exact_special_form_keeps_completion_popup_open() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(");
    editor.active_buffer_mut().cursor = (0, 1);

    for ch in ['d', 'e', 'f'] {
        editor.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    assert_eq!(editor.active_buffer().text(), "(def");
    assert!(editor.completion_state().is_some());
    assert!(editor
        .completion_state()
        .unwrap()
        .items
        .iter()
        .any(|item| item.label == "def"));
}

#[test]
fn no_op_runtime_side_effect_refresh_keeps_completion_popup_open() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    assert!(editor.completion_state().is_some());

    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().text(), "(de");
    assert!(editor.completion_state().is_some());
}

#[test]
fn no_op_runtime_side_effect_refresh_does_not_resync_committed_widget_snapshot() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def level (state 0))
            (effect
              (hslider :min 0 :max 1 :value level :on-change |v| (set! level v)))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    assert!(editor.active_buffer().committed_ui_snapshot.is_some());

    let widget_tree_revision = editor.active_buffer().widget_tree_revision;
    let committed_ui_revision = editor.active_buffer().committed_ui_revision;
    editor.refresh_runtime_side_effects();

    assert_eq!(
        editor.active_buffer().widget_tree_revision,
        widget_tree_revision
    );
    assert_eq!(
        editor.active_buffer().committed_ui_revision,
        committed_ui_revision
    );
}

#[test]
fn completion_scrolls_to_keep_selection_visible() {
    let mut runtime = Runtime::new();
    for name in [
        "seq-a", "seq-b", "seq-c", "seq-d", "seq-e", "seq-f", "seq-g", "seq-h", "seq-i",
    ] {
        runtime.register_native(name, |_args, _ctx| Ok(Value::Bool(true)));
    }
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(seq");
    editor.active_buffer_mut().cursor = (0, 4);

    editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
    for _ in 0..8 {
        editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    let completion = editor.completion_state().unwrap();
    assert_eq!(completion.selected, 8);
    assert_eq!(completion.scroll, 1);
}

#[test]
fn tab_accepts_dotted_completion_from_runtime_maps() {
    let mut runtime = Runtime::new();
    let mut fields = HashMap::new();
    fields.insert(
        "feedback".to_string(),
        Rc::new(RefCell::new(Value::Number(0.0))),
    );
    runtime.set_global_value("MODUM_DELAY", Value::Map(fields));
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(MODUM_DELAY.");
    editor.active_buffer_mut().cursor = (0, "(MODUM_DELAY.".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(MODUM_DELAY.feedback");
}

#[test]
fn map_results_are_shown_in_minibuffer() {
    let init = r#"
            (def eval-sexp ()
              (eval (s-expression-at-cursor)))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
    let mut runtime = Runtime::new();
    runtime.register_native("return-map", |_args, _ctx| {
        let mut map = HashMap::new();
        map.insert(
            "step".to_string(),
            Rc::new(RefCell::new(Value::Number(1.0))),
        );
        Ok(Value::Map(map))
    });
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "(return-map)");
    editor.active_buffer_mut().cursor = (0, "(return-map)".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

    let minibuffer = editor.minibuffer.unwrap_or_default();
    assert!(minibuffer.contains("step"));
}

#[test]
fn eval_updates_after_buffer_contents_change() {
    let init = r#"
            (def eval-sexp ()
              (eval (s-expression-at-cursor)))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*test*", "(+ 5 10)");
    editor.active_buffer_mut().cursor = (0, "(+ 5 10)".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(editor.minibuffer.as_deref(), Some("15"));

    editor.active_buffer_mut().set_text("(+ 100 100)");
    editor.active_buffer_mut().cursor = (0, "(+ 100 100)".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(editor.minibuffer.as_deref(), Some("200"));
}

#[test]
fn load_buffer_reloads_active_file_from_disk() {
    let path = temp_file_path("load-buffer");
    fs::write(&path, "(+ 1 2)\n").unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_file_buffer(&path).unwrap();
    editor.active_buffer_mut().cursor = (0, 0);
    editor.active_buffer_mut().insert_char(';');
    assert!(editor.active_buffer().dirty);

    editor.runtime_mut().eval_str("(load-buffer)").unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().text(), "(+ 1 2)\n");
    assert!(!editor.active_buffer().dirty);
    let expected = format!("Loaded {}", path.display());
    assert_eq!(editor.minibuffer.as_deref(), Some(expected.as_str()));

    let _ = fs::remove_file(path);
}

#[test]
fn load_buffer_errors_for_non_file_backed_buffer() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");

    editor.runtime_mut().eval_str("(load-buffer)").unwrap();
    editor.refresh_runtime_side_effects();

    let minibuffer = editor.minibuffer.unwrap_or_default();
    assert!(minibuffer.contains("buffer is not file-backed"));
}

#[test]
fn movement_clears_minibuffer_message() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc");
    editor.minibuffer = Some("15".to_string());
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(editor.minibuffer, None);
}

#[test]
fn transient_minibuffer_message_expires_without_input() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());

    editor.show_transient_message("view: text");
    editor.minibuffer_expires_at =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    editor.clear_needs_redraw();

    editor.update_timers();

    assert_eq!(editor.minibuffer, None);
    assert!(editor.needs_redraw());
}

#[test]
fn quit_does_not_prompt_for_dirty_scratch_buffer() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc");
    editor.active_buffer_mut().insert_char('x');

    editor.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));

    assert!(editor.should_quit());
    assert_eq!(editor.prompt_text(), None);
}

#[test]
fn quit_prompt_allows_discard_for_dirty_file_buffer() {
    let path = temp_file_path("quit-discard");
    fs::write(&path, "abc\n").unwrap();

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_file_buffer(&path).unwrap();
    editor.active_buffer_mut().insert_char('x');

    editor.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(editor.prompt_text().is_some());
    assert!(!editor.should_quit());

    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert!(editor.should_quit());
    assert_eq!(editor.prompt_text(), None);

    let _ = fs::remove_file(path);
}

#[test]
fn mouse_click_moves_cursor_in_text_view() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha\nbravo");

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 2),
        1,
        1,
        20,
        10,
    );

    assert_eq!(editor.active_buffer().cursor, (1, 2));
}

#[test]
fn mouse_drag_selects_text_region() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha");

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        1,
        1,
        20,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 5, 1),
        1,
        1,
        20,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 5, 1),
        1,
        1,
        20,
        10,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 4));
    assert_eq!(editor.active_region_range(), Some(((0, 1), (0, 4))));
}

#[test]
fn mouse_click_clears_existing_mark() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha");
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(editor.active_region_range(), Some(((0, 1), (0, 2))));

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 1),
        1,
        1,
        20,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 4, 1),
        1,
        1,
        20,
        10,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 3));
    assert!(editor.active_region_range().is_none());
}

#[test]
fn read_only_buffers_allow_keyboard_cursor_movement() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha\nbravo");
    editor.active_buffer_mut().cursor = (0, 2);
    editor.active_buffer_mut().read_only = true;

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().cursor, (1, 0));
}

#[test]
fn read_only_buffers_show_text_cursor() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "alpha");
    editor.active_buffer_mut().cursor = (0, 2);
    editor.active_buffer_mut().read_only = true;

    let frame = crate::frame::build_render_frame(&mut editor, 20, 10);

    assert_eq!(frame.cursor, Some((0, 2)));
}

#[test]
fn mouse_drag_updates_slider_via_on_change_callback() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (hslider
                    :min 0
                    :max 100
                    :value level
                    :on-change |v| (set! level v)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 9, 1),
        1,
        1,
        20,
        6,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert_eq!(n, 0.0),
        _ => panic!("expected numeric slider state"),
    }

    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(n > 90.0),
        _ => panic!("expected numeric slider state"),
    }
}

#[test]
fn slider_drag_continues_after_pointer_leaves_slider_bounds() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (hslider
                    :min 0
                    :max 100
                    :value level
                    :on-change |v| (set! level v)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 9, 1),
        1,
        1,
        20,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 3),
        1,
        1,
        20,
        6,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(
            n > 90.0,
            "expected off-row drag to keep editing slider, got {n}"
        ),
        _ => panic!("expected numeric slider state"),
    }
}

#[test]
fn knob_drag_stays_bound_after_callback_reorders_layout() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(24, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (def selected (state 0))
                (def algo (state 0))
                (def xy (state 0))
                (def algo-knob ()
                  (subtree :key "algo-knob"
                    (knob-number
                      :label "algo" :min 0 :max 10 :value algo :width 4 :height 2
                      :on-change |v| (do (set! selected 1) (set! algo v)))))
                (def xy-knob ()
                  (subtree :key "xy-knob"
                    (knob-number
                      :label "x/y" :min 0 :max 10 :value xy :width 4 :height 2
                      :on-change |v| (set! xy v))))
                (effect
                  (if (= selected 0)
                    (h-stack :gap 0 (algo-knob) (xy-knob))
                    (h-stack :gap 0 (xy-knob) (algo-knob))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 2),
        1,
        1,
        24,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 2, 1),
        1,
        1,
        24,
        6,
    );
    let _ = crate::frame::build_render_frame(&mut editor, 24, 6);
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 2, 0),
        1,
        1,
        24,
        6,
    );
    let frame = crate::frame::build_render_frame(&mut editor, 24, 6);

    let algo_value = editor.runtime.eval_str("algo").unwrap().unwrap();
    let xy_value = editor.runtime.eval_str("xy").unwrap().unwrap();
    match (algo_value, xy_value) {
        (Value::Number(algo), Value::Number(xy)) => {
            assert!(algo > 0.0, "expected captured algo knob to keep dragging");
            assert_eq!(xy, 0.0, "drag retargeted to x/y after layout changed");
        }
        _ => panic!("expected numeric knob states"),
    }

    fn find_label_for_id(node: &crate::layout::LayoutNode, id: u64) -> Option<String> {
        if node.widget_id == id {
            return node.props.get("label").and_then(|value| match value {
                Value::String(label) => Some(label.clone()),
                _ => None,
            });
        }
        node.children
            .iter()
            .find_map(|child| find_label_for_id(child, id))
    }

    let focused_id = frame.focused_widget_id.expect("focused widget");
    let layout = frame.widget_layout.expect("widget layout");
    assert_eq!(
        find_label_for_id(&layout, focused_id).as_deref(),
        Some("algo"),
        "focus retargeted after layout changed"
    );
}

#[test]
fn mouse_drag_updates_slider_via_bind_shorthand() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (hslider
                    :min 0
                    :max 100
                    :bind level))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(n > 90.0),
        _ => panic!("expected numeric slider state"),
    }
}

#[test]
fn mouse_drag_updates_matrix_via_bind_shorthand() {
    fn find_widget<'a>(
        node: &'a crate::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a crate::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget(child, widget_type))
    }

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(12, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def weights (state (list (list 0 0.25) (list 0.5 0.75))))
                (effect
                  (matrix
                    :rows 2
                    :cols 2
                    :min 0
                    :max 1
                    :width 4
                    :height 4
                    :bind weights))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(12, 8);

    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    let matrix = find_widget(layout, "matrix").expect("matrix layout node");
    assert!(matrix.rect.width > 0.0 && matrix.rect.height > 0.0);
    assert!(matrix.rect.width.is_finite() && matrix.rect.height.is_finite());

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 2),
        1,
        1,
        12,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 1),
        1,
        1,
        12,
        8,
    );

    let value = editor.runtime.eval_str("weights").unwrap().unwrap();
    let Value::List(rows) = value else {
        panic!("expected matrix rows");
    };
    let Value::List(first_row) = &*rows[0].borrow() else {
        panic!("expected first matrix row");
    };
    assert_eq!(*first_row[0].borrow(), Value::Number(0.0));
    assert_eq!(*first_row[1].borrow(), Value::Number(0.75));
}

#[test]
fn mouse_drag_updates_matrix_via_on_change() {
    fn find_widget<'a>(
        node: &'a crate::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a crate::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget(child, widget_type))
    }

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(12, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def weights (state (list (list 0 0.25) (list 0.5 0.75))))
                (effect
                  (matrix
                    :rows 2
                    :cols 2
                    :min 0
                    :max 1
                    :width 4
                    :height 4
                    :value weights
                    :on-change (lambda (next-weights)
                      (set! weights next-weights))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(12, 8);

    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    let matrix = find_widget(layout, "matrix").expect("matrix layout node");
    assert!(matrix.rect.width > 0.0 && matrix.rect.height > 0.0);
    assert!(matrix.rect.width.is_finite() && matrix.rect.height.is_finite());

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 2),
        1,
        1,
        12,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 1),
        1,
        1,
        12,
        8,
    );

    let value = editor.runtime.eval_str("weights").unwrap().unwrap();
    let Value::List(rows) = value else {
        panic!("expected matrix rows");
    };
    let Value::List(first_row) = &*rows[0].borrow() else {
        panic!("expected first matrix row");
    };
    assert_eq!(*first_row[0].borrow(), Value::Number(0.0));
    assert_eq!(*first_row[1].borrow(), Value::Number(0.75));
}

#[test]
fn widget_mouse_hit_testing_ignores_layout_aspect_scaling() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_aspect(2.35);
    editor.set_layout_viewport(24, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (v-stack
                    (label :text "a")
                    (label :text "b")
                    (label :text "c")
                    (hslider
                      :min 0
                      :max 100
                      :bind level)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 10);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 12, 3),
        0,
        0,
        24,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 15, 3),
        0,
        0,
        24,
        10,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(
            n > 80.0,
            "expected drag on visual slider row to hit, got {n}"
        ),
        _ => panic!("expected numeric slider state"),
    }
}

#[test]
fn widget_mouse_hit_testing_does_not_trigger_from_aspect_shifted_row() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_aspect(2.35);
    editor.set_layout_viewport(24, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (v-stack
                    (label :text "a")
                    (label :text "b")
                    (label :text "c")
                    (hslider
                      :min 0
                      :max 100
                      :bind level)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 10);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 15, 7),
        0,
        0,
        24,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 15, 7),
        0,
        0,
        24,
        10,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert_eq!(n, 0.0),
        _ => panic!("expected numeric slider state"),
    }
}

#[test]
fn tree_mouse_hit_testing_keeps_lower_row_edge_on_same_item() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(24, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def picked (state ""))
                (effect
                  (tree
                    :items '(
                      (:label "one")
                      (:label "two")
                      (:label "three"))
                    :on-select (lambda (item) (set! picked (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 10);

    let layout = editor.widget_layout().expect("tree layout");
    assert_eq!(layout.widget_type, "tree");
    let row_height = layout.rect.height / 3.0;
    let click_row = layout.rect.row + row_height * 0.92;

    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            layout.rect.col as u16,
            click_row as u16,
        ),
        0,
        0,
        24,
        10,
        layout.rect.col + 1.0,
        click_row,
    );

    let value = editor.runtime.eval_str("picked").unwrap().unwrap();
    assert_eq!(value, Value::String("one".to_string()));
}

#[test]
fn tree_row_height_prop_controls_hit_testing() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(24, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def picked (state ""))
                (effect
                  (tree
                    :row-height 2.0
                    :items '(
                      (:label "one")
                      (:label "two"))
                    :on-select (lambda (item) (set! picked (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 10);

    let layout = editor.widget_layout().expect("tree layout");
    assert_eq!(layout.rect.height, 4.0);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        0,
        0,
        24,
        10,
        layout.rect.col + 1.0,
        layout.rect.row + 1.7,
    );

    let value = editor.runtime.eval_str("picked").unwrap().unwrap();
    assert_eq!(value, Value::String("one".to_string()));
}

#[test]
fn tree_click_cursor_and_activate_have_separate_callbacks() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def selected (state ""))
                (def highlighted (state ""))
                (def activated (state ""))
                (effect
                  (tree
                    :focusable true
                    :items '(
                      (:label "one" :path "/one.wav")
                      (:label "two" :path "/two.wav"))
                    :on-select (lambda (item) (set! selected (get item :label)))
                    :on-cursor-change (lambda (item) (set! highlighted (get item :label)))
                    :on-activate (lambda (item) (set! activated (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0),
        0,
        0,
        40,
        10,
        1.0,
        0.2,
    );
    assert_eq!(
        editor.runtime.eval_str("selected").unwrap().unwrap(),
        Value::String("one".to_string())
    );
    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String(String::new())
    );

    editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime.eval_str("highlighted").unwrap().unwrap(),
        Value::String("two".to_string())
    );
    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String(String::new())
    );

    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String("two".to_string())
    );
}

#[test]
fn tree_double_click_activates_leaf() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def activated (state ""))
                (effect
                  (tree
                    :items '((:label "one" :path "/one.wav"))
                    :on-activate (lambda (item) (set! activated (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 10);

    let click = mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0);
    editor.handle_mouse_precise(click, 0, 0, 40, 10, 1.0, 0.2);
    editor.handle_mouse_precise(click, 0, 0, 40, 10, 1.0, 0.2);

    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String("one".to_string())
    );
}

#[test]
fn stale_overlay_node_does_not_swallow_underlying_clicks() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def clicked (state 0))
                (effect
                  (box :width 12 :height 2 :on-click (lambda (info) (set! clicked (+ clicked 1)))
                    (label "target")))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 8);
    assert!(editor.widget_layout().is_some());

    crate::widget_render::set_overlay(
        999_999,
        crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 20.0,
            height: 4.0,
        },
    );

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        0,
        0,
        40,
        8,
        2.0,
        1.0,
    );

    assert_eq!(
        editor.runtime.eval_str("clicked").unwrap().unwrap(),
        Value::Number(1.0),
        "a stale overlay id must be cleared and the current layout should receive the click"
    );
    assert_eq!(crate::widget_render::overlay_widget_id(), None);
}

#[test]
fn clickable_child_inside_draggable_parent_does_not_start_parent_drag() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def clicked (state 0))
                (def dropped (state 0))
                (effect
                  (box :width 20 :height 4
                       :drag-type "thing"
                       :drag-payload (dict :id 1)
                       :drop-types '("thing")
                       :on-drop (lambda (event) (set! dropped (+ dropped 1)))
                    (box :width 8 :height 2
                         :on-click (lambda (info) (set! clicked (+ clicked 1)))
                      (label "child"))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 8);
    assert!(editor.widget_layout().is_some());

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        0,
        0,
        40,
        8,
        2.0,
        1.0,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 6, 1),
        0,
        0,
        40,
        8,
        6.0,
        1.0,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 6, 1),
        0,
        0,
        40,
        8,
        6.0,
        1.0,
    );

    assert_eq!(
        editor.runtime.eval_str("clicked").unwrap().unwrap(),
        Value::Number(1.0)
    );
    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::Number(0.0),
        "dragging after pressing a clickable child must not arm the draggable parent"
    );
}

#[test]
fn patcher_background_double_click_consumes_local_only_event() {
    let path = temp_file_path("patcher-double-click-create");
    std::fs::write(&path, "(+ 1 2)\n").unwrap();

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(&format!(
            r#"
                (def last-event (state nil))
                (effect
                  (patcher
                    :height 10
                    :path "{}"
                    :on-change (lambda (event) (set! last-event event))))
                "#,
            path.display()
        ))
        .unwrap();
    editor.set_layout_viewport(40, 12);

    let click = mouse_event(MouseEventKind::Down(MouseButton::Left), 20, 5);
    editor.handle_mouse_precise(click, 0, 0, 40, 12, 20.0, 5.0);
    editor.handle_mouse_precise(click, 0, 0, 40, 12, 20.0, 5.0);
    editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_ne!(
        editor.runtime.eval_str("last-event").unwrap().unwrap(),
        Value::Nil,
        "double-click should leave the patcher text edit active so Enter emits a patcher change"
    );
}

#[test]
fn cmd_y_toggles_visible_patcher_selected_cable_without_widget_focus() {
    let path = temp_file_path("patcher-visible-cable-cmd-y");
    std::fs::write(&path, "(def sig (in 1))\n(out sig 1)\n").unwrap();

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(&format!(
            r#"
                (effect
                  (patcher
                    :height 10
                    :path "{}"))
                "#,
            path.display()
        ))
        .unwrap();
    editor.set_layout_viewport(40, 12);
    let patcher = first_patcher_layout_node(
        editor
            .runtime
            .current_layout
            .as_ref()
            .expect("layout should contain patcher"),
    )
    .expect("patcher node");
    crate::widget_render::patcher::select_first_patcher_cable_for_test(&patcher)
        .expect("selected cable");
    let initially_segmented =
        crate::widget_render::patcher::selected_patcher_cable_is_segmented_for_test(&patcher)
            .expect("selected cable should have segmentation state");
    editor.clear_focused_widget();

    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::SUPER));

    let after =
        crate::widget_render::patcher::selected_patcher_cable_is_segmented_for_test(&patcher)
            .expect("selected cable should still have segmentation state");
    assert_ne!(after, initially_segmented);
}

#[test]
fn tree_sample_drag_drops_on_compatible_box_target() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 40 :height 4
                    (tree
                      :width 18
                      :height 2
                      :drag-type "sample"
                      :items '((:label "kick.wav" :path "/samples/kick.wav"))
                      :on-select (lambda (item) nil))
                    (box
                      :key "drop-target"
                      :width 18
                      :height 2
                      :drop-types (list "sample")
                      :drop-meta (dict :kind "test")
                      :on-drop (lambda (event)
                        (set! dropped (get (get event :payload) :path)))))) 
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    assert!(
        crate::widget_render::active_drop_hover_target().is_some(),
        "compatible drop target should become the active hover target during drag"
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("/samples/kick.wav".to_string())
    );
    assert_eq!(
        crate::widget_render::active_drop_hover_target(),
        None,
        "drop hover target should clear after mouse up"
    );
}

#[test]
fn tree_drag_payload_does_not_require_path_field() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 40 :height 4
                    (tree
                      :width 18
                      :height 2
                      :drag-type "audio-effect"
                      :items '((:label "Filter" :kind "builtin-audio-effect" :name "Filter"))
                      :on-select (lambda (item) nil))
                    (box
                      :key "drop-target"
                      :width 18
                      :height 2
                      :drop-types (list "audio-effect")
                      :on-drop (lambda (event)
                        (set! dropped (get (get event :payload) :name))))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("Filter".to_string())
    );
}

#[test]
fn box_drag_payload_drops_on_compatible_box_target() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 40 :height 4
                    (box
                      :key "drag-source"
                      :width 18
                      :height 2
                      :drag-type "effect-instance"
                      :drag-payload (dict :kind "audio-effect-instance" :slot 3 :name "Delay")
                      (label "Delay"))
                    (box
                      :key "drop-target"
                      :width 18
                      :height 2
                      :drop-types (list "effect-instance")
                      :on-drop (lambda (event)
                        (set! dropped (get (get event :payload) :name))))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("Delay".to_string())
    );
}

#[test]
fn box_drag_drops_on_target_inside_scrolled_container() {
    fn find_by_key<'a>(
        node: &'a crate::layout::LayoutNode,
        key: &str,
    ) -> Option<&'a crate::layout::LayoutNode> {
        if node.stable_key.as_deref() == Some(key) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_by_key(child, key))
    }

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 48 :height 4
                    (box
                      :key "drag-source"
                      :width 18
                      :height 2
                      :drag-type "audio-effect"
                      :drag-payload (dict :kind "builtin-audio-effect" :name "Filter")
                      (label "Filter"))
                    (scroll :key "drop-scroll" :width 22 :height 4
                      (v-stack :gap 0
                        (box :width 20 :height 8)
                        (box
                          :key "drop-target"
                          :width 20
                          :height 2
                          :drop-types (list "audio-effect")
                          :on-drop (lambda (event)
                            (set! dropped (get (get event :payload) :name))))))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    let layout = editor.widget_layout().expect("layout");
    let scroll = find_by_key(&layout, "drop-scroll").expect("drop scroll node");
    crate::widget_render::scroll::set_scroll_state(
        crate::widget_render::scroll::scroll_state_key(scroll),
        crate::widget_render::scroll::ScrollState {
            offset_y: 7.0,
            viewport_height: 4.0,
            content_height: 10.0,
            synced_selection: None,
        },
    );

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 1),
        0,
        0,
        80,
        10,
        24.0,
        1.4,
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 1),
        0,
        0,
        80,
        10,
        24.0,
        1.4,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("Filter".to_string())
    );
}

#[test]
fn tree_sample_drag_cursor_starts_after_pointer_moves_past_threshold() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 40 :height 4
                    (tree
                      :width 18
                      :height 2
                      :drag-type "sample"
                      :items '((:label "kick.wav" :path "/samples/kick.wav"))
                      :on-select (lambda (item) nil))
                    (box
                      :key "drop-target"
                      :width 18
                      :height 2
                      :drop-types (list "sample")
                      :on-drop (lambda (event)
                        (set! dropped (get (get event :payload) :path)))))) 
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.2,
        0.4,
    );
    assert_eq!(crate::widget_render::active_drop_hover_target(), None);
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::Default
    );

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    assert!(
        crate::widget_render::active_drop_hover_target().is_some(),
        "drag hover should start once the pointer moves past the gesture threshold"
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("/samples/kick.wav".to_string())
    );
}

#[test]
fn tree_sample_drag_ignores_incompatible_box_target() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect
                  (h-stack :width 40 :height 4
                    (tree
                      :width 18
                      :height 2
                      :drag-type "sample"
                      :items '((:label "kick.wav" :path "/samples/kick.wav"))
                      :on-select (lambda (item) nil))
                    (box
                      :width 18
                      :height 2
                      :drop-types (list "instrument")
                      :on-drop (lambda (event)
                        (set! dropped "bad"))))) 
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(80, 10);

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 0),
        0,
        0,
        80,
        10,
        2.0,
        0.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragNotAllowed
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 24, 0),
        0,
        0,
        80,
        10,
        24.0,
        0.4,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn typed_widget_drag_can_drop_on_another_tile() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect-buffer "*source*"
                  (tree
                    :width 18
                    :height 2
                    :drag-type "sample"
                    :items '((:label "kick.wav" :path "/samples/kick.wav"))
                    :on-select (lambda (item) nil)))
                (effect-buffer "*target*"
                  (box
                    :width 18
                    :height 2
                    :drop-types (list "sample")
                    :on-drop (lambda (event)
                      (set! dropped (get (get event :payload) :path)))))
                "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let source = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*source*")
        .expect("source buffer");
    let target = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*target*")
        .expect("target buffer");
    editor.set_active_buffer(editor.buffers[source].id);
    let target_tile = editor
        .split_active_tile(SplitDir::Vertical, target)
        .expect("split should create target tile");
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|id| *id != target_tile)
        .expect("source tile");
    editor.switch_active_tile(source_tile);
    editor.update_tile_rects(100, 20);

    let (source_col, source_row, _, _) = editor
        .tile_content_area(source_tile, 1)
        .expect("source content area");
    let (target_col, target_row, _, _) = editor
        .tile_content_area(target_tile, 1)
        .expect("target content area");
    let down = (source_col as f32 + 2.0, source_row as f32 + 0.4);
    let drop = (target_col as f32 + 2.0, target_row as f32 + 0.4);

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            down.0.floor() as u16,
            down.1.floor() as u16,
        ),
        down.0,
        down.1,
        1,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            drop.0.floor() as u16,
            drop.1.floor() as u16,
        ),
        drop.0,
        drop.1,
        1,
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            drop.0.floor() as u16,
            drop.1.floor() as u16,
        ),
        drop.0,
        drop.1,
        1,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("/samples/kick.wav".to_string())
    );
}

#[test]
fn typed_widget_drag_can_drop_on_horizontally_scrolled_target_tile() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def dropped (state ""))
                (effect-buffer "*source*"
                  (tree
                    :width 18
                    :height 2
                    :drag-type "sample"
                    :items '((:label "kick.wav" :path "/samples/kick.wav"))
                    :on-select (lambda (item) nil)))
                (effect-buffer "*target*"
                  (h-stack :gap 0
                    (box :width 30 :height 2)
                    (box
                      :key "drop-target"
                      :width 30
                      :height 2
                      :drop-types (list "sample")
                      :on-drop (lambda (event)
                        (set! dropped (get (get event :payload) :path))))))
                "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let source = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*source*")
        .expect("source buffer");
    let target = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*target*")
        .expect("target buffer");
    editor.set_active_buffer(editor.buffers[source].id);
    let target_tile = editor
        .split_active_tile(SplitDir::Vertical, target)
        .expect("split should create target tile");
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|id| *id != target_tile)
        .expect("source tile");
    editor.switch_active_tile(source_tile);
    editor.update_tile_rects(100, 20);
    editor
        .tile_root
        .find_leaf_mut(target_tile)
        .expect("target tile leaf")
        .widget_scroll_left = 20.0;

    let (source_col, source_row, _, _) = editor
        .tile_content_area(source_tile, 0)
        .expect("source content area");
    let (target_col, target_row, _, _) = editor
        .tile_content_area(target_tile, 0)
        .expect("target content area");
    let down = (source_col as f32 + 2.0, source_row as f32 + 0.4);
    let drop = (target_col as f32 + 35.0, target_row as f32 + 0.4);

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            down.0.floor() as u16,
            down.1.floor() as u16,
        ),
        down.0,
        down.1,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            drop.0.floor() as u16,
            drop.1.floor() as u16,
        ),
        drop.0,
        drop.1,
        0,
    );
    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::DragCopy
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            drop.0.floor() as u16,
            drop.1.floor() as u16,
        ),
        drop.0,
        drop.1,
        0,
    );

    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("/samples/kick.wav".to_string())
    );
}

#[test]
fn scrolled_tree_double_click_and_drag_use_visible_row() {
    fn find_by_key<'a>(
        node: &'a crate::layout::LayoutNode,
        key: &str,
    ) -> Option<&'a crate::layout::LayoutNode> {
        if node.stable_key.as_deref() == Some(key) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_by_key(child, key))
    }

    let items = (0..20)
        .map(|idx| format!("(:label \"item-{idx}.wav\" :path \"/samples/item-{idx}.wav\")"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!(
        r#"
            (def selected (state ""))
            (def activated (state ""))
            (def dropped (state ""))
            (effect
              (h-stack :width 48 :height 5
                (scroll :key "sample-scroll" :width 20 :height 4
                  (tree
                    :key "sample-tree"
                    :width 20
                    :row-height 1.0
                    :drag-type "sample"
                    :items '({items})
                    :on-select (lambda (item) (set! selected (get item :path)))
                    :on-activate (lambda (item) (set! activated (get item :path)))))
                (box
                  :key "drop-target"
                  :width 20
                  :height 4
                  :drop-types (list "sample")
                  :on-drop (lambda (event)
                    (set! dropped (get (get event :payload) :path))))))
        "#
    );

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 10);
    editor.runtime.eval_str(&source).unwrap();
    editor.set_layout_viewport(80, 10);

    let layout = editor.widget_layout().expect("layout");
    let scroll = find_by_key(&layout, "sample-scroll").expect("scroll node");
    crate::widget_render::scroll::set_scroll_state(
        crate::widget_render::scroll::scroll_state_key(scroll),
        crate::widget_render::scroll::ScrollState {
            offset_y: 10.0,
            viewport_height: 4.0,
            content_height: 20.0,
            synced_selection: None,
        },
    );

    let click = mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1);
    editor.handle_mouse_precise(click, 0, 0, 80, 10, 2.0, 1.4);
    assert_eq!(
        editor.runtime.eval_str("selected").unwrap().unwrap(),
        Value::String("/samples/item-11.wav".to_string())
    );
    editor.handle_mouse_precise(click, 0, 0, 80, 10, 2.0, 1.4);
    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String("/samples/item-11.wav".to_string())
    );

    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 3),
        0,
        0,
        80,
        10,
        2.0,
        3.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 26, 3),
        0,
        0,
        80,
        10,
        26.0,
        3.4,
    );
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 26, 3),
        0,
        0,
        80,
        10,
        26.0,
        3.4,
    );
    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap().unwrap(),
        Value::String("/samples/item-13.wav".to_string())
    );
}

#[test]
fn tiled_mouse_hit_testing_uses_fractional_tile_origin() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(24, 9);
    editor
        .runtime
        .eval_str(
            r#"
                (def picked (state ""))
                (effect
                  (tree
                    :items '(
                      (:label "one")
                      (:label "two"))
                    :on-select (lambda (item) (set! picked (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(24, 9);

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.show_border = false;
    }
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 3.25,
            row: 10.5,
            width: 24.0,
            height: 10.0,
        },
    )];
    let layout = editor.widget_layout().expect("tree layout");
    let row_height = layout.rect.height / 2.0;
    let precise_col = 3.25 + layout.rect.col + 1.0;
    let precise_row = 10.5 + layout.rect.row + row_height * 0.9;

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );

    let value = editor.runtime.eval_str("picked").unwrap().unwrap();
    assert_eq!(value, Value::String("one".to_string()));
}

#[test]
fn tiled_text_click_uses_precise_content_origin_and_border_inset() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abcdef\nsecond");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.set_layout_viewport(20, 8);
    struct TestTextMeasurer;
    impl crate::layout::TextMeasurer for TestTextMeasurer {
        fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
            text.chars().count() as f32 * 10.0
        }

        fn line_height_px(&self, _font_size: f32) -> f32 {
            20.0
        }
    }
    editor.set_text_measurer(Box::new(TestTextMeasurer), 10.0, 20.0);

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.border_width_px = 2.0;
    }
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 3.25,
            row: 10.5,
            width: 20.0,
            height: 8.0,
        },
    )];

    let precise_col: f32 = 3.25 + 0.2 + 2.9;
    let precise_row: f32 = 10.5 + 0.2 + 0.9;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 2));
}

#[test]
fn metal_tiled_widget_click_uses_fractional_layout_viewport_without_relayout() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime
        .eval_str(
            r#"
                (def clicked (state false))
                (effect
                  (button "hit"
                    :width 4
                    :height 1
                    :on-click (lambda (info) (set! clicked true))))
            "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.show_border = true;
        leaf.show_status = false;
        leaf.border_width_px = 0.25;
    }
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 3.25,
            row: 10.5,
            width: 20.0,
            height: 8.0,
        },
    )];
    editor.set_layout_viewport_exact(19.5, 7.5);
    editor.runtime.drain_rendered_layouts();
    let layout_revision = editor.widget_layout_revision();

    let precise_col: f32 = 3.25 + 0.25 + 0.5;
    let precise_row: f32 = 10.5 + 0.25 + 0.5;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );

    assert_eq!(editor.widget_layout_revision(), layout_revision);
    assert!(
        editor.runtime.drain_rendered_layouts().is_empty(),
        "routing a Metal tile click must not relayout from fractional viewport to floored viewport"
    );
    assert_eq!(
        editor.runtime.eval_str("clicked").unwrap().unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn metal_tiled_fill_widget_does_not_scroll_horizontally_from_floored_content_width() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (box :width :fill :height :fill
                    (label "fits")))
            "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.show_border = true;
        leaf.show_status = false;
        leaf.border_width_px = 0.25;
    }
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 3.25,
            row: 10.5,
            width: 20.0,
            height: 8.0,
        },
    )];

    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::ScrollRight, 10, 12),
        4.0,
        12.0,
        0,
    );

    let layout = editor.widget_layout().expect("widget layout");
    assert_eq!(layout.rect.width, 19.5);
    assert_eq!(
        crate::ui::hit::max_extent_exact(&layout, editor.layout_aspect()).0,
        19.5
    );
    assert_eq!(
        editor.widget_scroll_left(),
        0.0,
        "Metal tiled wheel routing must use the exact 19.5-cell layout viewport, not the floored 19-cell event width"
    );
}

#[test]
fn mouse_down_updates_knob_via_bind_shorthand() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(6, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (knob
                    :size 2
                    :min 0
                    :max 100
                    :bind level))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(6, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        1,
        1,
        6,
        6,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(n >= 99.0),
        _ => panic!("expected numeric knob state"),
    }
}

#[test]
fn mouse_click_toggle_via_bind_shorthand_round_trips_bool_state() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(8, 4);
    editor
        .runtime
        .eval_str(
            r#"
                (def enabled (state true))
                (effect
                  (toggle :bind enabled))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(8, 4);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        1,
        1,
        8,
        4,
    );
    assert_eq!(
        editor.runtime.eval_str("enabled").unwrap(),
        Some(Value::Bool(false))
    );

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        1,
        1,
        8,
        4,
    );
    assert_eq!(
        editor.runtime.eval_str("enabled").unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn box_pointer_handlers_receive_clicks_through_noninteractive_sdf_child() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(8, 4);
    editor
        .runtime
        .eval_str(
            r#"
                (def armed (state false))
                (def toggled (state false))
                (defwidget visual-dot
                  :width 2 :height 2
                  :shader (sdf/layer
                            (sdf/fill (sdf/circle 0.7) :accent)))
                (effect
                  (box :width 4 :height 2 :align :center
                    :on-mouse-down (lambda (evt) (set! armed true))
                    :on-mouse-up (lambda (evt)
                      (if armed
                        (set! toggled true)
                        nil))
                    (visual-dot)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(8, 4);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        1,
        1,
        8,
        4,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 2, 1),
        1,
        1,
        8,
        4,
    );

    assert_eq!(
        editor.runtime.eval_str("armed").unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        editor.runtime.eval_str("toggled").unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn knob_updates_shared_label_state_from_each_binding() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(
            r#"
                (defstate steps '(20 30 40 50))
                (effect
                  (h-stack
                    (grid :cols 4 :col-width 4
                      (each steps |step|
                        (knob :min 0 :max 100 :bind step)))
                    (grid :cols 4 :col-width 4
                      (each steps |step|
                        (label (fmt "{:.0}" step))))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 12);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        1,
        1,
        40,
        12,
    );

    let value = editor.runtime.eval_str("steps").unwrap().unwrap();
    let Value::List(items) = value else {
        panic!("expected steps list");
    };
    let first = items[0].borrow().clone();
    match first {
        Value::Number(n) => assert!(n >= 99.0),
        _ => panic!("expected numeric step"),
    }

    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    let rendered = crate::layout::format_layout_tree_lines(layout, 0);
    assert!(
        rendered.iter().any(|line| line.contains("text=\"100\"")),
        "expected shared label text to reflect updated knob value: {rendered:?}"
    );
}

#[test]
fn knob_drag_clamps_after_leaving_hit_rect() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(10, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (def level (state 0))
                (effect
                  (knob
                    :size 4
                    :min 0
                    :max 100
                    :bind level))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(10, 10);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 4),
        1,
        1,
        10,
        10,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 2, 0),
        1,
        1,
        10,
        10,
    );

    let value = editor.runtime.eval_str("level").unwrap().unwrap();
    match value {
        Value::Number(n) => assert!(n >= 99.0),
        _ => panic!("expected numeric knob state"),
    }
}

#[test]
fn eval_sexp_replaces_previous_preview_effect_layout() {
    let init = r#"
            (def eval-sexp ()
              (let ((form (s-expression-at-cursor)))
                (if (= form "")
                  (status "No s-expression at cursor")
                  (let ((result (eval form)))
                    result))))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer(
        "*test*",
        "(effect (h-stack (label \"hello\") (hslider :min 0 :max 100 :bind x)))",
    );
    editor.runtime.eval_str("(defstate x 0)").unwrap();
    editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

    editor
        .active_buffer_mut()
        .set_text("(effect (h-stack (hslider :min 0 :max 100 :bind x)))");
    editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

    editor
        .runtime
        .set_reactive("APP", "unused", Value::Number(0.0));
    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    assert_eq!(layout.widget_type, "h-stack");
    assert_eq!(layout.children.len(), 1);
    assert_eq!(layout.children[0].widget_type, "hslider");
}

#[test]
fn mouse_drag_updates_bound_step_field_from_each() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 6);
    editor
        .runtime
        .eval_str(
            r#"
                (defstate pattern
                  (dict :steps
                    (list
                      (dict :velocity 20)
                      (dict :velocity 50))))
                (effect
                  (h-stack
                    (each pattern.steps |v|
                      (hslider :min 0 :max 100 :bind v.velocity))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 6);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 10, 1),
        1,
        1,
        40,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 10, 1),
        1,
        1,
        40,
        6,
    );

    let value = editor.runtime.eval_str("pattern.steps").unwrap().unwrap();
    let Value::List(items) = value else {
        panic!("expected steps list");
    };
    let first = items[0].borrow().clone();
    let Value::Map(map) = first else {
        panic!("expected step map");
    };
    let velocity = map.get("velocity").expect("velocity").borrow().clone();
    match velocity {
        Value::Number(n) => assert!(n > 50.0),
        _ => panic!("expected numeric velocity"),
    }
}

#[test]
fn mouse_drag_updates_bound_list_item_from_each() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(
            r#"
                (defstate steps '(20 30 40 50 60))
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 12);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 5),
        1,
        1,
        40,
        12,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 2),
        1,
        1,
        40,
        12,
    );

    let value = editor.runtime.eval_str("steps").unwrap().unwrap();
    let Value::List(items) = value else {
        panic!("expected steps list");
    };
    match &*items[0].borrow() {
        Value::Number(n) => assert!(*n > 20.0),
        _ => panic!("expected numeric step"),
    }
}

#[test]
fn mouse_updates_zipped_destructured_each_bindings() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 12);
    editor
        .runtime
        .eval_str(
            r#"
                (defstate toggles (list true true))
                (defstate levels (list 20 30))
                (effect
                  (h-stack :gap 1
                    (each (zip toggles levels) |(enabled level)|
                      (v-stack :gap 1
                        (toggle :bind enabled)
                        (vslider :min 0 :max 100 :bind level)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 12);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        1,
        1,
        20,
        12,
    );

    let toggles = editor.runtime.eval_str("toggles").unwrap().unwrap();
    let Value::List(toggle_items) = toggles else {
        panic!("expected toggle list");
    };
    assert_eq!(*toggle_items[0].borrow(), Value::Bool(false));

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 6),
        1,
        1,
        20,
        12,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 3),
        1,
        1,
        20,
        12,
    );

    let levels = editor.runtime.eval_str("levels").unwrap().unwrap();
    let Value::List(level_items) = levels else {
        panic!("expected level list");
    };
    match &*level_items[0].borrow() {
        Value::Number(n) => assert!(*n > 20.0),
        _ => panic!("expected numeric level"),
    }
}

#[test]
fn reevaluating_defstate_and_effect_rebuilds_each_layout() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());

    editor
        .runtime
        .eval_str("(defstate steps '(1 2 3 4 5))")
        .unwrap();
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
        )
        .unwrap();

    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    assert_eq!(layout.children.len(), 5);

    editor
        .runtime
        .eval_str("(defstate steps '(1 2 3 4 5 6 7 8 9))")
        .unwrap();
    let result = editor.runtime.eval_str(
        r#"
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
    );

    assert!(result.is_ok(), "effect re-eval failed: {result:?}");
    let layout = editor.runtime.current_layout.as_ref().expect("layout");
    assert_eq!(layout.children.len(), 9);
}

#[test]
fn dired_mode_loads_and_refreshes() {
    let init = std::fs::read_to_string("init.lisp").unwrap_or_default();
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );

    // Call dired-here and verify full state
    editor.call_lisp_handler("dired-here");

    assert_eq!(editor.active_buffer().name, "*dired*");
    assert!(
        editor.active_buffer().read_only,
        "dired buffer should be read-only, mode={:?}",
        editor.active_buffer().mode
    );
    assert!(
        matches!(editor.active_buffer().mode, BufferMode::Named(ref n) if n == "dired-mode"),
        "mode should be dired-mode, got {:?}",
        editor.active_buffer().mode
    );
    assert!(
        editor.widget_layout().is_none(),
        "dired should not have a widget layout"
    );
    assert!(
        editor.active_buffer().lines.len() >= 3,
        "dired should render a text listing"
    );
    assert_eq!(
        editor.active_buffer().cursor.0,
        2,
        "cursor should land on ../"
    );
}

#[test]
fn editable_widget_buffers_do_not_auto_focus_widgets() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| e))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    assert_eq!(editor.focused_widget_id(), None);
}

#[test]
fn editable_buffers_keep_text_navigation_when_widget_is_unfocused() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().set_text("abc");
    editor.active_buffer_mut().cursor = (0, 1);
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| e))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    assert_eq!(editor.focused_widget_id(), None);

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().cursor, (0, 2));

    editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(editor.active_buffer().text(), "ac");
}

#[test]
fn timeline_draw_click_focuses_even_without_pointer_down_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :tool :draw
                    :lanes (list (dict :id 0 :label "L0"))
                    :items ()
                    :view-start 0
                    :view-duration 16
                    :on-action |e| e))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 10, 3),
        1,
        1,
        30,
        8,
    );

    assert!(
        editor.focused_widget_id().is_some(),
        "timeline clicks should focus even when mouse-down does not dispatch an action"
    );
}

#[test]
fn widget_interaction_survives_buffer_switch() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);

    editor
        .runtime_mut()
        .eval_str(
            r#"(def level (state 0))
               (effect (hslider :min 0 :max 100 :value level :on-change |v| (set! level v)))"#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);
    assert!(editor.widget_layout().is_some());

    // Interact before switch — should work
    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );
    let _val = editor.runtime_mut().eval_str("level").unwrap().unwrap();

    // Switch away
    editor.open_scratch_buffer("*other*", "hello");
    assert!(editor.widget_layout().is_none());

    // Switch back
    let id = editor
        .buffers
        .iter()
        .find(|b| b.name == "*scratch*")
        .unwrap()
        .id;
    editor.set_active_buffer(id);
    assert!(
        editor.widget_layout().is_some(),
        "layout should be restored"
    );

    // Try to interact after switch back
    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );

    let val = editor.runtime_mut().eval_str("level").unwrap().unwrap();
    match val {
        Value::Number(n) => assert!(n > 0.0, "level should have changed, got {n}"),
        _ => panic!("expected number"),
    }
}

#[test]
fn widget_tree_survives_buffer_switch() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);

    // Create an effect layout in the scratch buffer
    editor
        .runtime_mut()
        .eval_str(
            r#"(def level (state 0))
               (effect (hslider :min 0 :max 100 :value level :on-change |v| (set! level v)))"#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);

    assert!(
        editor.widget_layout().is_some(),
        "should have layout before switch"
    );
    let original_buffer_name = editor.active_buffer().name.clone();

    // Open a new buffer (simulating switch away)
    editor.open_scratch_buffer("*other*", "hello");
    assert_eq!(editor.active_buffer().name, "*other*");
    // Widget should be gone for this buffer
    assert!(
        editor.widget_layout().is_none(),
        "other buffer should have no layout"
    );

    // Switch back
    let original_id = editor
        .buffers
        .iter()
        .find(|b| b.name == original_buffer_name)
        .unwrap()
        .id;
    editor.set_active_buffer(original_id);
    assert_eq!(editor.active_buffer().name, original_buffer_name);

    // Widget should be restored
    assert!(
        editor.widget_layout().is_some(),
        "widget layout should be restored after switching back. widget_tree={:?}",
        editor.active_buffer().widget_tree.is_some()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn knob_number_rich_mod_props_survive_lisp_layout_and_emit_scene_primitives() {
    fn find_widget<'a>(
        node: &'a crate::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a crate::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget(child, widget_type))
    }

    let mut runtime = Runtime::new();
    runtime.register_reactive(
        "APP",
        vec![
            ("base", Value::Number(0.0)),
            ("depth-1", Value::Number(0.5)),
        ],
        true,
    );
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 16);
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*controls*"
              (knob-number :label "cut"
                :value 0 :min -1 :max 1 :decimals 2
                :base-value (bind "APP" "base") :base-min -1 :base-max 1
                :selected-mod-slot 1
                :mod-range-0-slot 1
                :mod-range-0-depth (bind "APP" "depth-1")
                :mod-ranges (list
                  (dict :slot 2 :depth -0.25))
                :width 4 :height 2.8
                :value-align :center))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let controls_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap()
        .id;
    editor.set_active_buffer(controls_id);

    let layout = editor.widget_layout().expect("rich knob layout");
    let knob = find_widget(&layout, "knob-number").expect("knob-number layout node");
    assert!(
        matches!(knob.props.get("mod-ranges"), Some(Value::List(items)) if items.len() == 1),
        "rich mod-ranges prop should survive Lisp -> layout: {:?}",
        knob.props.get("mod-ranges")
    );
    assert!(
        matches!(
            knob.props.get("mod-range-0-depth"),
            Some(Value::ReactiveRef { .. })
        ),
        "flat reactive mod range props should be accepted by knob-number: {:?}",
        knob.props.get("mod-range-0-depth")
    );

    let viewport = crate::widget_render::WidgetViewport {
        cell_w: 10.0,
        cell_h: 10.0,
        vp_w: 600.0,
        vp_h: 160.0,
        time_seconds: 0.0,
        focused_widget_id: None,
        focused_branch: false,
        tile_content_rows: 16.0,
        scroll_top: 0.0,
        scroll_left: 0.0,
        inherited_hover: false,
    };
    let primitives = crate::widget_render::widget_primitives_for_node(knob, viewport);
    let base_count = primitives
        .iter()
        .filter(|primitive| {
            matches!(
                primitive,
                crate::widget_render::MetalPrimitive::WidgetInstance { widget_type, .. }
                    if widget_type == "knob-number"
            )
        })
        .count();
    let range_uniforms = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            crate::widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
                ..
            } if widget_type == "knob-number-mod-range" => Some(instance.uniform_b),
            _ => None,
        })
        .collect::<Vec<_>>();
    let text = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            crate::widget_render::MetalPrimitive::ProportionalText(text) => {
                Some(text.text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(base_count, 1);
    assert_eq!(range_uniforms.len(), 2);
    assert_eq!(range_uniforms[0], [0.956, 0.5, 0.375, 0.0]);
    assert!((range_uniforms[1][0] - 0.895).abs() < 0.000_01);
    assert_eq!(&range_uniforms[1][1..], &[0.5, 0.75, 1.0]);
    assert!(
        text.contains(&"cut"),
        "expected knob label text primitive: {text:?}"
    );
}

#[test]
fn cycle_view_mode_toggles_between_text_and_ui_when_ui_exists() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.active_buffer_mut().widget_tree = Some(Value::String("ui".to_string()));

    editor.runtime_mut().eval_str("(cycle-view-mode)").unwrap();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::UiOnly);

    editor.runtime_mut().eval_str("(cycle-view-mode)").unwrap();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::TextOnly);
}

#[test]
fn switching_to_widget_only_scratch_buffer_restores_ui_view() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(r#"(effect-buffer "*piano-roll*" (label "notes"))"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    let piano_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*piano-roll*")
        .unwrap();
    editor.buffers[piano_idx].view_mode = super::ViewMode::TextOnly;

    editor
        .runtime_mut()
        .eval_str(r#"(set-window-buffer "*piano-roll*")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().name, "*piano-roll*");
    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::UiOnly);
    assert!(editor.widget_layout().is_some());
}

#[test]
fn cycle_view_mode_stays_in_text_when_no_ui_exists() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;

    editor.runtime_mut().eval_str("(cycle-view-mode)").unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::TextOnly);
    assert_eq!(editor.minibuffer.as_deref(), Some("No UI in this buffer"));
}

#[test]
fn text_visible_buffer_forces_status_bar_even_when_tile_hides_status() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "abc");
    let tile_id = editor.active_tile;
    editor.active_leaf_mut().show_status = false;
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 0.0,
            row: 0.0,
            width: 20.0,
            height: 8.0,
        },
    )];

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 20, 9);

    assert!(frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 7);
}

#[test]
fn ui_only_buffer_can_still_hide_status_bar() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    let tile_id = editor.active_tile;
    editor.active_leaf_mut().show_status = false;
    editor.cached_tile_rects = vec![(
        tile_id,
        crate::layout::Rect {
            col: 0.0,
            row: 0.0,
            width: 20.0,
            height: 8.0,
        },
    )];

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 20, 9);

    assert!(!frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 8);
}

#[test]
fn hidden_ui_only_status_bar_reappears_for_chord_and_minibuffer_input() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.active_leaf_mut().show_status = false;
    let tile_id = editor.active_tile;

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    assert!(!frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 8);

    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    let status: String = frame.tiles[0]
        .frame
        .status_cells
        .iter()
        .map(|cell| cell.ch)
        .collect();
    assert!(frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 7);
    assert!(status.contains("C-x -"));

    editor.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    let status: String = frame.tiles[0]
        .frame
        .status_cells
        .iter()
        .map(|cell| cell.ch)
        .collect();
    assert!(frame.tiles[0].show_status);
    assert!(status.contains("Find file: "));

    editor.minibuffer_input = Some(super::MinibufferMode::FindFile {
        input: "demo".to_string(),
        selected: 0,
    });
    assert_eq!(
        editor.minibuffer_prompt().as_deref(),
        Some("Find file: demo  [font-demo.lisp]")
    );
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    let status: String = frame.tiles[0]
        .frame
        .status_cells
        .iter()
        .map(|cell| cell.ch)
        .collect();
    assert!(frame.tiles[0].show_status);
    assert!(status.contains("Find file: demo"), "{status}");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    assert!(!frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 8);
}

fn eval_tabbed_test_layout(editor: &mut Editor) {
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (set-layout
              (list :buf "*sequencer*"
                :tabs (list
                  (list "Sequencer" "*sequencer*")
                  (list "Matrix" "*matrix*"))
                :hide-status true
                :border-radius 12
                :border-width 4))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
}

fn editor_with_tabbed_buffers() -> Editor {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def tab-clicked (state false))
            (effect-buffer "*sequencer*"
              (button "under-tab"
                :width 10
                :height 1
                :on-click (lambda (evt) (set! tab-clicked true))))
            (effect-buffer "*matrix*" (label "matrix"))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    eval_tabbed_test_layout(&mut editor);
    editor.update_tile_rects(60, 12);
    editor
}

#[test]
fn tabbed_layout_selects_primary_buffer_by_default() {
    let editor = editor_with_tabbed_buffers();
    let leaf = editor.active_leaf();

    assert_eq!(editor.active_buffer().name, "*sequencer*");
    assert_eq!(leaf.tabs.len(), 2);
    assert_eq!(leaf.selected_tab, Some(0));
}

#[test]
fn tabbed_layout_rejects_malformed_tabs_before_applying_layout() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*sequencer*", "");
    editor.open_scratch_buffer("*matrix*", "");

    let result = editor.runtime_mut().eval_str(
        r#"(set-layout
            (list :buf "*sequencer*"
              :tabs (list (list "Matrix" "*matrix*"))))"#,
    );
    if result.is_ok() {
        editor.refresh_runtime_side_effects();
        assert!(
            editor.active_leaf().tabs.is_empty(),
            "malformed tab specs must not apply a tabbed tile"
        );
    } else {
        let error = result.unwrap_err();
        assert!(
            format!("{error:?}").contains("must include the primary"),
            "unexpected error: {error:?}"
        );
    }
}

#[test]
fn clicking_folder_tab_switches_buffer_without_dispatching_underlying_widget() {
    let mut editor = editor_with_tabbed_buffers();
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*transport*"
              (button "under-tab"
                :width 80
                :height 4
                :on-click (lambda (evt) (set! tab-clicked true))))
            (set-layout
              (list :rows :gap 0
                0.25 (list :buf "*transport*" :hide-status true)
                0.75 (list :buf "*sequencer*"
                  :tabs (list
                    (list "Sequencer" "*sequencer*")
                    (list "Matrix" "*matrix*"))
                  :hide-status true
                  :border-radius 12
                  :border-width 4)))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);

    let sequencer_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*sequencer*")
        .unwrap();
    let tile_id = editor
        .tile_root
        .find_leaf_by_buffer_idx(sequencer_idx)
        .unwrap()
        .id;
    let leaf = editor.tile_root.find_leaf(tile_id).unwrap();
    let tab_rect = crate::tile::tile_tab_layouts(
        editor.tile_rect(tile_id).unwrap(),
        &leaf.tabs,
        leaf.selected_tab,
    )
    .into_iter()
    .find(|tab| tab.index == 1)
    .expect("matrix tab layout");

    let precise_col = tab_rect.rect.col + tab_rect.rect.width * 0.5;
    let precise_row = tab_rect.rect.row + tab_rect.rect.height * 0.5;
    let tile_rect = editor.tile_rect(tile_id).unwrap();
    assert!(
        tab_rect.rect.row >= tile_rect.row
            && tab_rect.rect.row + tab_rect.rect.height <= editor.tile_body_rect(tile_id).unwrap().row + f32::EPSILON,
        "tabs should live in the tile header row above the content body"
    );
    assert_eq!(
        editor.tile_at_screen(precise_col, precise_row),
        Some(tile_id),
        "tab click point should be inside the tabbed tile"
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );

    assert_eq!(editor.active_buffer().name, "*matrix*");
    assert_eq!(editor.active_tile, tile_id);
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );
    assert_eq!(editor.active_buffer().name, "*matrix*");
    assert_eq!(
        editor.active_tile, tile_id,
        "releasing on the internal tab header must keep the tabbed tile selected"
    );
    assert_eq!(editor.active_leaf().selected_tab, Some(1));
    assert_eq!(
        editor.runtime.eval_str("tab-clicked").unwrap().unwrap(),
        Value::Bool(false),
        "tab clicks must not dispatch to buffer widgets underneath"
    );
}

#[test]
fn tab_selection_survives_reapplying_same_layout() {
    let mut editor = editor_with_tabbed_buffers();

    editor
        .runtime_mut()
        .eval_str(r#"(set-window-buffer-for "*sequencer*" "*matrix*")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.active_buffer().name, "*matrix*");
    assert_eq!(editor.active_leaf().selected_tab, Some(1));

    eval_tabbed_test_layout(&mut editor);

    assert_eq!(editor.active_buffer().name, "*matrix*");
    assert_eq!(editor.active_leaf().selected_tab, Some(1));
}

#[test]
fn tabbed_tile_frame_reserves_internal_header_row_for_tabs() {
    let mut editor = editor_with_tabbed_buffers();
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 12);
    let tile = &frame.tiles[0];

    assert!(!tile.tabs.is_empty());
    assert!(tile.body_rect.row > tile.rect.row);
    assert_eq!(tile.body_rect.col, tile.rect.col);
    assert_eq!(tile.body_rect.width, tile.rect.width);
    assert!(tile.body_rect.height < tile.rect.height);
    let first_tab_height = tile.tabs[0].rect.height;
    for tab in &tile.tabs {
        assert!(tab.rect.width > 0.0);
        assert_eq!(tab.rect.height, first_tab_height);
        assert!(tab.rect.col + tab.rect.width <= tile.rect.col + tile.rect.width);
        assert!(tab.rect.row >= tile.rect.row);
        assert!(tab.rect.row + tab.rect.height <= tile.body_rect.row + f32::EPSILON);
    }
    assert!(tile.frame.widget_layout.as_ref().unwrap().rect.height > 0.0);
    assert_eq!(editor.tile_body_rect(editor.active_tile).unwrap(), tile.body_rect);
}

#[test]
fn set_view_mode_supports_both_as_secondary_mode() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().widget_tree = Some(Value::String("ui".to_string()));

    editor
        .runtime_mut()
        .eval_str(r#"(set-view-mode "both")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::Both);
}

#[test]
fn effect_buffer_creates_target_buffer_without_switching() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);

    editor
        .runtime_mut()
        .eval_str(r#"(effect-buffer "*controls*" (knob :size 4))"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().name, "*scratch*");
    let controls = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap();
    assert!(controls.widget_tree.is_some());

    editor.set_active_buffer(controls.id);
    assert!(editor.widget_layout().is_some());
}

#[test]
fn effect_buffer_ui_is_visible_immediately_in_split_target_window() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);
    editor.update_tile_rects(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*controls*"
              (box :padding 2
                (knob :value 2 :size 4 :min 0 :max 10)))
            (split-window-right "*controls*")
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 12);

    let controls_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*controls*")
        .unwrap();
    let controls_leaf = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|tile_id| editor.tile_root.find_leaf(tile_id))
        .find(|leaf| leaf.buffer_idx == controls_idx)
        .unwrap();

    assert!(
        controls_leaf.cached_layout.is_some(),
        "split target should have cached layout immediately"
    );
}

#[test]
fn visible_inactive_tile_binding_write_marks_editor_for_redraw() {
    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);
    editor.update_tile_rects(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*meters*"
              (mixer-meter
                :level-l (bind "APP" "peak")
                :level-r 0.0
                :width 2.22
                :height 4.24))
            (split-window-right "*meters*")
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 12);
    editor.sync_reactive_bindings_for_visible_layouts();

    let meters_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*meters*")
        .unwrap();
    let meters_leaf = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|tile_id| editor.tile_root.find_leaf(tile_id))
        .find(|leaf| leaf.buffer_idx == meters_idx)
        .unwrap();
    assert_ne!(
        meters_leaf.id, editor.active_tile,
        "regression setup expects the bound widget to be in an inactive visible tile"
    );
    let widget_id = meters_leaf
        .cached_layout
        .as_ref()
        .expect("meter layout should be cached for visible inactive tile")
        .widget_id;

    editor.clear_needs_redraw();
    let _ = editor.take_dirty_widget_ids();

    editor
        .runtime_mut()
        .set_reactive("APP", "peak", Value::Number(0.7));

    assert!(
        editor.needs_redraw(),
        "binding-only writes in inactive visible tiles must schedule a render"
    );
    assert_eq!(editor.take_dirty_widget_ids(), vec![widget_id]);
}

#[test]
fn tiled_frame_routes_binding_dirty_ids_to_inactive_tile() {
    let mut runtime = Runtime::new();
    runtime.register_reactive("APP", vec![("peak", Value::Number(0.1))], true);
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);
    editor.update_tile_rects(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*meters*"
              (mixer-meter
                :level-l (bind "APP" "peak")
                :level-r 0.0
                :width 2.22
                :height 4.24))
            (split-window-right "*meters*")
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 12);
    editor.sync_reactive_bindings_for_visible_layouts();

    let meters_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*meters*")
        .unwrap();
    let meters_leaf = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|tile_id| editor.tile_root.find_leaf(tile_id))
        .find(|leaf| leaf.buffer_idx == meters_idx)
        .unwrap();
    assert_ne!(meters_leaf.id, editor.active_tile);
    let meters_tile_id = meters_leaf.id;
    let widget_id = meters_leaf
        .cached_layout
        .as_ref()
        .expect("meter layout should be cached")
        .widget_id;

    let _ = editor.take_dirty_widget_ids();
    editor
        .runtime_mut()
        .set_reactive("APP", "peak", Value::Number(0.7));

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 12);
    let meters_tile = frame
        .tiles
        .iter()
        .find(|tile| tile.tile_id == meters_tile_id)
        .expect("meters tile should be in tiled frame");
    assert_eq!(meters_tile.frame.dirty_widget_ids, vec![widget_id]);
}

#[test]
fn effect_buffer_updates_live_when_named_target_buffer_is_active() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (defstate level 2)
            (effect-buffer "*controls*"
              (label (fmt "{}" level)))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap()
        .id;
    editor.set_active_buffer(controls_id);

    editor.runtime_mut().eval_str("(set! level 7)").unwrap();

    let tree = editor.runtime.current_widget_tree().unwrap();
    let rendered = crate::vm::format_lisp_value(&tree);
    assert!(
        rendered.contains("\"7\""),
        "tree should reflect updated state: {rendered}"
    );
}

#[test]
fn active_named_buffer_reactive_full_update_replaces_current_runtime_tree() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);
    editor
        .runtime_mut()
        .register_reactive("APP", vec![("count", Value::Number(2.0))], true);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*controls*"
              (label (fmt "{}" APP.count)))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap()
        .id;
    editor.set_active_buffer(controls_id);

    editor
        .runtime_mut()
        .set_reactive("APP", "count", Value::Number(7.0));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let tree = editor.runtime.current_widget_tree().unwrap();
    let rendered = crate::vm::format_lisp_value(&tree);
    assert!(
        rendered.contains("\"7\""),
        "active named buffer runtime tree should reflect full reactive update: {rendered}"
    );
    let layout = editor.widget_layout().expect("updated active layout");
    assert!(
        matches!(layout.props.get("text"), Some(Value::String(text)) if text == "7"),
        "active named buffer hit-test layout should be rebuilt from the updated tree"
    );
}

#[test]
fn named_effect_buffer_commits_nested_subtree_roots() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (defstate level 2)
            (effect-buffer "*controls*"
              (v-stack
                (subtree :key "counter-label"
                  (label (fmt "{}" level)))
                (label "static")))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap();
    let snapshot = controls
        .committed_ui_snapshot
        .as_ref()
        .expect("expected committed snapshot");
    assert!(
        snapshot
            .subtree_roots
            .values()
            .any(|subtree| subtree.stable_key.as_deref() == Some("counter-label")),
        "expected nested subtree root to be indexed in committed snapshot"
    );
}

#[test]
fn named_effect_buffer_nested_subtree_updates_when_inactive() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (defstate level 2)
            (effect-buffer "*controls*"
              (v-stack
                (subtree :key "counter-label"
                  (label (fmt "{}" level)))
                (label "static")))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*controls*")
        .unwrap();
    let initial_tree = editor.buffers[controls_idx]
        .widget_tree
        .clone()
        .expect("initial widget tree");
    let initial_rendered = crate::vm::format_lisp_value(&initial_tree);
    assert!(initial_rendered.contains("\"2\""));

    editor.runtime_mut().eval_str("(set! level 7)").unwrap();
    editor.refresh_runtime_side_effects();

    let updated_tree = editor.buffers[controls_idx]
        .widget_tree
        .clone()
        .expect("updated widget tree");
    let updated_rendered = crate::vm::format_lisp_value(&updated_tree);
    assert!(
        updated_rendered.contains("\"7\""),
        "inactive named buffer subtree should update in place: {updated_rendered}"
    );
}

#[test]
fn active_named_buffer_batches_multiple_subtree_updates_into_one_relayout() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);
    editor.runtime_mut().register_reactive(
        "APP",
        vec![
            ("left-level", Value::Number(2.0)),
            ("right-level", Value::Number(3.0)),
        ],
        true,
    );

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*controls*"
              (v-stack
                (subtree :key "left-label"
                  (label (fmt "left:{}" APP.left-level) :width 8))
                (subtree :key "right-label"
                  (label (fmt "right:{}" APP.right-level) :width 8))))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap()
        .id;
    editor.set_active_buffer(controls_id);
    editor
        .runtime_mut()
        .set_reactive("APP", "left-level", Value::Number(7.0));
    editor
        .runtime_mut()
        .set_reactive("APP", "right-level", Value::Number(9.0));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let trace = editor
        .runtime()
        .last_ui_invalidation_trace()
        .expect("reactive invalidation trace");
    assert_eq!(trace.pending_subtree_patch_count, 2);
    assert_eq!(trace.subtree_failure_reason, None);
    let tree = editor.runtime.current_widget_tree().unwrap();
    let rendered = crate::vm::format_lisp_value(&tree);
    assert!(
        rendered.contains("\"left:7\"") && rendered.contains("\"right:9\""),
        "batched active subtree update should apply both patches: {rendered}"
    );
}

#[test]
fn active_full_rerender_survives_delegated_subtree_patches() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (defstate panel-open false)
            (effect-buffer "*fx*"
              (v-stack
                (if panel-open
                  (label "panel-open")
                  (label "panel-closed"))
                (subtree :key "stable-child"
                  (label (if panel-open "stable-open" "stable-closed")))
                (subtree :key (if panel-open "mode-open-key" "mode-closed-key")
                  (label (if panel-open "mode-open" "mode-closed")))))
            "#,
        )
        .expect("install structural active effect");
    editor.refresh_runtime_side_effects();

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor
        .runtime_mut()
        .eval_str("(set! panel-open true)")
        .expect("open panel");
    editor.refresh_runtime_side_effects();

    let tree = editor
        .runtime
        .current_widget_tree()
        .expect("active fx tree after open");
    assert!(
        widget_has_label_text(&tree, "panel-open"),
        "full active rerender should keep structural panel insertion"
    );
    assert!(
        widget_has_label_text(&tree, "stable-open"),
        "stable subtree patch should still apply"
    );
    assert!(
        widget_has_label_text(&tree, "mode-open"),
        "state-dependent subtree key should come from the full rerender"
    );
    assert!(
        !widget_has_label_text(&tree, "panel-closed"),
        "delegated subtree fallback must not restore the old active buffer tree"
    );
}

#[test]
fn named_effect_buffer_emits_replace_subtree_for_committed_root() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (defstate level 2)
            (effect-buffer "*controls*"
              (v-stack
                (subtree :key "counter-label"
                  (label (fmt "{}" level)))
                (label "static")))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let controls = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*controls*")
        .unwrap();
    let committed_root_id = controls
        .committed_ui_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot
                .subtree_roots
                .values()
                .find(|subtree| subtree.stable_key.as_deref() == Some("counter-label"))
                .and_then(|subtree| subtree.subtree_root_id)
        })
        .expect("committed subtree root id");

    editor.runtime_mut().eval_str("(set! level 7)").unwrap();
    let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
    let emitted_root_id = pending
        .iter()
        .find_map(|update| match update {
            crate::vm::PendingUiUpdate::ReplaceSubtree {
                target: crate::vm::EffectTarget::BufferName(name),
                subtree_root_id,
                ..
            } if name == "*controls*" => Some(*subtree_root_id),
            _ => None,
        })
        .expect("replace subtree update for named buffer");

    assert_eq!(
        emitted_root_id, committed_root_id,
        "replace subtree should target committed subtree root"
    );
}

#[test]
fn first_layout_buffer_stays_interactive_after_second_layout_buffer_eval() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 6);

    editor
        .runtime_mut()
        .eval_str(
            r#"
                (def roll-level (state 0))
                (effect
                  (h-stack
                    (label (fmt "roll={}" roll-level))
                    (hslider :min 0 :max 100 :value roll-level :on-change |v| (set! roll-level v))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);
    let roll_id = editor.active_buffer().id;

    editor.open_scratch_buffer("*grid*", "");
    editor
        .runtime_mut()
        .eval_str(
            r#"
                (def grid-level (state 0))
                (effect
                  (h-stack
                    (label (fmt "grid={}" grid-level))
                    (hslider :min 0 :max 100 :value grid-level :on-change |v| (set! grid-level v))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(20, 6);

    editor.set_active_buffer(roll_id);
    assert!(
        editor.widget_layout().is_some(),
        "roll layout should be restored"
    );

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );

    let val = editor
        .runtime_mut()
        .eval_str("roll-level")
        .unwrap()
        .unwrap();
    match val {
        Value::Number(n) => assert!(n > 0.0, "roll-level should have changed, got {n}"),
        _ => panic!("expected number"),
    }

    let layout = editor.widget_layout().expect("roll layout");
    let label_text = layout.children[0]
        .props
        .get("text")
        .and_then(|value| match value {
            Value::String(text) => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");
    assert_ne!(label_text, "roll=0", "roll layout should have rerendered");
}

#[test]
fn first_layout_buffer_stays_interactive_after_buffer_list_switch() {
    let init = include_str!("../../init.lisp").to_string();
    let runtime = Runtime::with_init_source(&init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );
    editor.set_layout_viewport(80, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
                (def roll-level (state 0))
                (effect
                  (hslider :min 0 :max 100 :value roll-level :on-change |v| (set! roll-level v)))
                "#,
        )
        .unwrap();
    let roll_id = editor.active_buffer().id;

    editor.open_scratch_buffer("*grid*", "");
    editor
        .runtime_mut()
        .eval_str(
            r#"
                (def grid-level (state 0))
                (effect
                  (hslider :min 0 :max 100 :value grid-level :on-change |v| (set! grid-level v)))
                "#,
        )
        .unwrap();

    editor.runtime_mut().eval_str("(buffer-list-here)").unwrap();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.active_buffer().name, "*buffers*");

    editor.set_active_buffer(roll_id);
    assert!(
        editor.widget_layout().is_some(),
        "roll layout should be restored"
    );

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
        1,
        1,
        20,
        6,
    );

    let val = editor
        .runtime_mut()
        .eval_str("roll-level")
        .unwrap()
        .unwrap();
    match val {
        Value::Number(n) => assert!(n > 0.0, "roll-level should have changed, got {n}"),
        _ => panic!("expected number"),
    }
}

#[test]
fn buffer_list_mode_accepts_filter_input_while_read_only() {
    let init = include_str!("../../init.lisp").to_string();
    let runtime = Runtime::with_init_source(&init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );

    editor.open_scratch_buffer("*grid*", "");
    editor.open_scratch_buffer("*gain*", "");
    editor.runtime_mut().eval_str("(buffer-list-here)").unwrap();
    editor.refresh_runtime_side_effects();

    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().name, "*buffers*");
    assert_eq!(editor.minibuffer.as_deref(), Some("1 buffers"));
    assert_eq!(editor.active_buffer().lines[0], "Switch to: gr");
    assert!(
        editor
            .active_buffer()
            .lines
            .iter()
            .any(|line| line.contains("*grid*")),
        "filtered list should include *grid*"
    );
    assert!(
        !editor
            .active_buffer()
            .lines
            .iter()
            .any(|line| line.contains("*gain*")),
        "filtered list should exclude *gain*"
    );
}

#[test]
fn buffer_list_native_order_tracks_recent_buffer_switches() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());

    let grid_id = editor.open_scratch_buffer("*grid*", "");
    editor.open_scratch_buffer("*gain*", "");
    editor.set_active_buffer(grid_id);

    let value = editor
        .runtime_mut()
        .eval_str("(buffer-list)")
        .unwrap()
        .unwrap();
    let Value::List(items) = value else {
        panic!("expected buffer-list to return a list");
    };
    let names = items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::String(name) => name.clone(),
            other => panic!("expected buffer name string, got {other:?}"),
        })
        .collect::<Vec<_>>();

    assert_eq!(names[0], "*grid*");
    assert_eq!(names[1], "*gain*");
}

#[test]
fn named_buffer_text_natives_replace_append_and_create_buffers() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());

    let target_id = editor.create_scratch_buffer("*target*", "alpha", BufferMode::ESeqLisp);
    editor.set_active_buffer(target_id);

    editor
        .runtime_mut()
        .eval_str(r#"(set-buffer-text-for "*target*" "beta")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    let target = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*target*")
        .expect("target buffer");
    assert_eq!(target.text(), "beta");

    editor
        .runtime_mut()
        .eval_str(r#"(append-buffer-lines-for "*target*" (list "delta" "epsilon"))"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    let target = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*target*")
        .expect("target buffer");
    assert_eq!(target.text(), "beta\n\ndelta\nepsilon");

    editor
        .runtime_mut()
        .eval_str(r#"(append-buffer-lines-for "*created*" (list "gamma"))"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    let created = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*created*")
        .expect("created buffer");
    assert_eq!(created.text(), "gamma");
}

#[test]
fn buffer_list_mode_shows_previous_buffer_first() {
    let init = include_str!("../../init.lisp").to_string();
    let runtime = Runtime::with_init_source(&init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );

    let grid_id = editor.open_scratch_buffer("*grid*", "");
    editor.open_scratch_buffer("*gain*", "");
    editor.set_active_buffer(grid_id);

    editor.runtime_mut().eval_str("(buffer-list-here)").unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().name, "*buffers*");
    assert_eq!(editor.active_buffer().lines[1], "> *grid*");
    assert!(
        !editor
            .active_buffer()
            .lines
            .iter()
            .any(|line| line.contains("*buffers*")),
        "buffer-list mode should not offer the buffer-list buffer itself"
    );
}

#[test]
fn buffer_list_mode_backspace_updates_filter() {
    let init = include_str!("../../init.lisp").to_string();
    let runtime = Runtime::with_init_source(&init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init),
            ..EditorConfig::default()
        },
    );

    editor.open_scratch_buffer("*grid*", "");
    editor.runtime_mut().eval_str("(buffer-list-here)").unwrap();
    editor.refresh_runtime_side_effects();

    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().lines[0], "Switch to: g");
}

#[test]
fn timeline_click_item_emits_select_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 9, 2),
        1,
        1,
        30,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("select".to_string())
    );
    assert_eq!(super::get_first_list_number(&action, "ids"), Some(10.0));
}

#[test]
fn timeline_drag_item_emits_move_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 15, 2),
        1,
        1,
        30,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("move-items-absolute".to_string())
    );
    assert!(super::get_map_field_number(&action, "start").unwrap_or(0.0) > 4.5);
    assert_eq!(super::get_first_list_number(&action, "ids"), Some(10.0));
}

#[test]
fn timeline_drag_unselected_item_moves_that_item_even_if_another_is_selected() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :sidebar-width 4
                    :lanes (list
                      (dict :id 0 :label "L0")
                      (dict :id 1 :label "L1"))
                    :items (list
                      (dict :id 10 :lane 0 :start 2 :end 6 :selected true)
                      (dict :id 11 :lane 1 :start 8 :end 12 :selected false))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 26, 6),
        1,
        1,
        40,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 30, 6),
        1,
        1,
        40,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("move-items-absolute".to_string())
    );
    assert_eq!(
        super::get_first_list_number(&action, "ids"),
        Some(11.0),
        "dragging an unselected item should target that item, not stale selection"
    );
}

#[test]
fn focusable_timeline_click_still_dispatches_pointer_selection_before_drag() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :sidebar-width 4
                    :lanes (list
                      (dict :id 0 :label "L0")
                      (dict :id 1 :label "L1"))
                    :items (list
                      (dict :id 10 :lane 0 :start 2 :end 6 :selected true)
                      (dict :id 11 :lane 1 :start 8 :end 12 :selected false))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 26, 6),
        1,
        1,
        40,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("select".to_string()),
        "focusable widgets should still receive pointer down if they do not have :on-enter"
    );
    assert_eq!(super::get_first_list_number(&action, "ids"), Some(11.0));
}

#[test]
fn focusable_timeline_mouse_down_does_not_scroll_before_drag_gesture() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 13);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 35
                    :focusable true
                    :lane-height 1
                    :lanes (list
                      (dict :id 0 :label "L0") (dict :id 1 :label "L1")
                      (dict :id 2 :label "L2") (dict :id 3 :label "L3")
                      (dict :id 4 :label "L4") (dict :id 5 :label "L5")
                      (dict :id 6 :label "L6") (dict :id 7 :label "L7")
                      (dict :id 8 :label "L8") (dict :id 9 :label "L9")
                      (dict :id 10 :label "L10") (dict :id 11 :label "L11")
                      (dict :id 12 :label "L12") (dict :id 13 :label "L13")
                      (dict :id 14 :label "L14") (dict :id 15 :label "L15")
                      (dict :id 16 :label "L16") (dict :id 17 :label "L17")
                      (dict :id 18 :label "L18") (dict :id 19 :label "L19"))
                    :items (list)
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 13);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 10, 5),
        1,
        1,
        40,
        13,
    );

    assert_eq!(
        editor.widget_scroll_top(),
        0.0,
        "mouse down focus must not move widget scroll before gesture capture"
    );

    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 12, 7),
        1,
        1,
        40,
        13,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("marquee-select".to_string())
    );
    assert_eq!(super::get_map_field_number(&action, "lane-a"), Some(3.0));
    assert_eq!(super::get_map_field_number(&action, "lane-b"), Some(5.0));
}

#[test]
fn ui_only_widget_scrolls_when_content_is_taller_than_viewport() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (box :width :fill :height 20
                    (label "tall content")))
                "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(20, 8);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 2, 2), 1, 1, 20, 8);

    assert!(
        editor.widget_scroll_top() > 0.0,
        "UI-only overflow content should allow vertical widget scrolling"
    );
}

#[test]
fn can_reveal_widget_in_visible_inactive_buffer_without_switching_focus() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(60, 10);
    editor
        .runtime
        .eval_str(
            r#"
                (effect-buffer "*sequencer*"
                  (v-stack :width :fill :gap 0
                    (box :width :fill :height 18)
                    (box :key "target-row" :width :fill :height 3)))
                (split-window-right "*sequencer*")
                "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 10);

    let active_before = editor.active_tile;
    let sequencer_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*sequencer*")
        .expect("sequencer buffer");
    let sequencer_scroll_before = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|tile_id| editor.tile_root.find_leaf(tile_id))
        .find(|leaf| leaf.buffer_idx == sequencer_idx)
        .expect("visible sequencer leaf")
        .widget_scroll_top;
    assert_eq!(sequencer_scroll_before, 0.0);

    assert!(
        editor.ensure_widget_stable_key_visible_in_buffer_named("*sequencer*", "target-row", 1.0),
        "revealing a below-viewport row should update the inactive tile scroll"
    );

    assert_eq!(
        editor.active_tile, active_before,
        "revealing an inactive buffer must not steal tile focus"
    );
    let sequencer_scroll_after = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|tile_id| editor.tile_root.find_leaf(tile_id))
        .find(|leaf| leaf.buffer_idx == sequencer_idx)
        .expect("visible sequencer leaf")
        .widget_scroll_top;
    assert!(
        sequencer_scroll_after > 0.0,
        "inactive sequencer tile should scroll to reveal the target row"
    );
}

#[test]
fn ui_only_widget_does_not_scroll_when_content_exactly_fits_viewport() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (box :width :fill :height 8.4
                    (label "fitting content")))
                "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(20, 8);
    editor.active_leaf_mut().widget_viewport_height = 8.4;

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 2, 2), 1, 1, 20, 8);

    assert_eq!(
        editor.widget_scroll_top(),
        0.0,
        "UI-only content that exactly fits the viewport should not scroll"
    );
}

#[test]
fn ui_only_nested_scroll_content_does_not_inflate_outer_widget_scroll() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(20, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (v-stack :width :fill :height 8 :gap 0
                    (scroll :width :fill :height 4
                      (box :width :fill :height 20
                        (label "tall clipped inner content")))
                    (box :width :fill :height 4
                      (label "footer"))))
                "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(20, 8);
    editor.active_leaf_mut().widget_viewport_height = 8.0;

    assert_eq!(
        editor.max_widget_vertical_scroll(),
        0.0,
        "outer widget scroll should ignore content clipped inside a nested scroll container"
    );

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 10, 7), 1, 1, 20, 8);

    assert_eq!(
        editor.widget_scroll_top(),
        0.0,
        "scrolling outside the inner scroll should not move the outer viewport"
    );
}

#[test]
fn ui_only_nested_scroll_content_does_not_inflate_outer_horizontal_scroll() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport_exact(19.5, 8.0);
    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (box :width :fill :height :fill
                    (scroll :width :fill :height 4
                      (h-stack :gap 0
                        (box :width 50 :height 2
                          (label "wide clipped content"))))))
                "#,
        )
        .unwrap();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport_exact(19.5, 8.0);

    let layout = editor.widget_layout().expect("widget layout");
    assert_eq!(
        crate::ui::hit::max_extent_exact(&layout, editor.layout_aspect()).0,
        19.5,
        "outer widget scroll extent should stop at nested scroll viewport"
    );

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 2, 2), 1, 1, 19, 8);

    assert_eq!(
        editor.widget_scroll_left(),
        0.0,
        "content clipped inside nested scroll should not move the outer viewport horizontally"
    );
}

#[test]
fn touchpad_scroll_rematerializes_virtual_stack_on_next_frame() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    let children = (0..40)
        .map(|i| {
            format!(
                r#"(box :key "item-{i}" :width :fill :height 2
                     (label "item-{i}"))"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!(
        r#"
            (scroll :key "outer-scroll" :width :fill :height 8
              (virtual-v-stack
                :key "virtual-list"
                :width :fill
                :gap 0
                :padding 0
                :estimated-item-height 2
                :overscan 0
                {children}))
            "#
    );
    let tree = editor
        .runtime
        .eval_str(&source)
        .unwrap()
        .expect("widget expression should return a tree");
    assert!(
        !matches!(tree, Value::Nil),
        "widget expression should produce a non-nil virtual scroll tree"
    );
    editor.runtime.set_widget_tree(tree);
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    let initial_frame = crate::frame::build_render_frame(&mut editor, 30, 8);
    let initial_layout = initial_frame.widget_layout.expect("initial widget layout");
    let initial_rendered = crate::layout::format_layout_tree_lines(&initial_layout, 0);
    assert!(
        initial_rendered.iter().any(|line| line.contains("item-0")),
        "initial virtual stack should materialize top rows: {initial_rendered:?}"
    );
    assert!(
        !initial_rendered.iter().any(|line| line.contains("item-10")),
        "initial virtual stack should not materialize later rows: {initial_rendered:?}"
    );
    editor.runtime.drain_rendered_layouts();

    for _ in 0..3 {
        assert!(
            editor.handle_touchpad_scroll(1, 1, 4.0, 4.0, 0.0, -140.0),
            "touchpad scroll should be handled by the scroll widget"
        );
    }
    assert!(
        editor.runtime.drain_rendered_layouts().is_empty(),
        "scroll bursts should defer relayout until frame build"
    );

    let scrolled_frame = crate::frame::build_render_frame(&mut editor, 30, 8);
    let rendered_layouts = editor.runtime.drain_rendered_layouts();
    assert_eq!(
        rendered_layouts.len(),
        1,
        "coalesced scroll burst should produce one relayout on the next frame"
    );
    let scrolled_layout = scrolled_frame
        .widget_layout
        .expect("scrolled widget layout");
    let scrolled_rendered = crate::layout::format_layout_tree_lines(&scrolled_layout, 0);
    assert!(
        scrolled_rendered
            .iter()
            .any(|line| line.contains("item-10")),
        "scroll relayout should rematerialize the visible virtual rows: {scrolled_rendered:?}"
    );
    assert!(
        !scrolled_rendered.iter().any(|line| line.contains("item-0")),
        "old top rows should no longer be materialized after scroll: {scrolled_rendered:?}"
    );
}

#[test]
fn timeline_drag_item_edge_emits_resize_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 18, 2),
        1,
        1,
        30,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("resize-item-absolute".to_string())
    );
    assert_eq!(
        super::get_map_field_keyword(&action, "edge"),
        Some("end".to_string())
    );
    assert_eq!(super::get_map_field_number(&action, "id"), Some(10.0));
}

#[test]
fn timeline_draw_tool_emits_create_item() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :tool :draw
                    :lanes (list (dict :id 0 :label "L0"))
                    :items ()
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 6, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 12, 2),
        1,
        1,
        30,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("create-item".to_string())
    );
    assert!(
        super::get_map_field_number(&action, "end").unwrap_or(0.0)
            > super::get_map_field_number(&action, "start").unwrap_or(0.0)
    );
}

#[test]
fn timeline_draw_tool_mouse_up_emits_finish_create_item() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :tool :draw
                    :lanes (list (dict :id 0 :label "L0"))
                    :items ()
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 6, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 12, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 12, 2),
        1,
        1,
        30,
        8,
    );

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("finish-create-item".to_string())
    );
}

#[test]
fn timeline_scroll_content_vertical_wheel_emits_lane_scroll_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 10, 2), 1, 1, 30, 8);

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("scroll-view".to_string())
    );
    assert_eq!(
        super::get_map_field_number(&action, "delta-time"),
        Some(0.0)
    );
    assert!(super::get_map_field_number(&action, "delta-lanes").unwrap_or(0.0) > 0.0);
}

#[test]
fn timeline_scroll_header_vertical_wheel_emits_zoom_view_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 10, 1), 1, 1, 30, 8);

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("zoom-view".to_string())
    );
    assert!(super::get_map_field_number(&action, "factor").unwrap_or(0.0) > 1.0);
}

#[test]
fn timeline_touchpad_magnify_emits_zoom_view_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_touchpad_magnify(1, 1, 10.5, 3.0, 0.2);

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("zoom-view".to_string())
    );
    assert!(super::get_map_field_number(&action, "factor").unwrap_or(0.0) > 1.0);
}

#[test]
fn widget_touchpad_magnify_internal_state_marks_redraw() {
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-patcher-magnify-redraw-{}.lisp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, "(+ 1 2)\n").unwrap();

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 12);
    editor
        .runtime
        .eval_str(&format!(
            r#"
                (effect
                  (patcher
                    :height 10
                    :path "{}"))
                "#,
            path.display()
        ))
        .unwrap();
    editor.set_layout_viewport(40, 12);
    editor.clear_needs_redraw();

    editor.handle_touchpad_magnify(1, 1, 10.0, 4.0, 0.2);

    assert!(editor.needs_redraw());
}

#[test]
fn timeline_scroll_content_horizontal_wheel_emits_time_scroll_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :view-start 0
                    :view-duration 16
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 10, 2), 1, 1, 30, 8);

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("scroll-view".to_string())
    );
    assert_eq!(
        super::get_map_field_number(&action, "delta-lanes"),
        Some(0.0)
    );
    assert!(super::get_map_field_number(&action, "delta-time").unwrap_or(0.0) > 0.0);
}

#[test]
fn text_buffer_horizontal_scroll_ignores_stale_widget_layout_width() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "short line");

    editor
        .runtime
        .eval_str(
            r#"
                (effect
                  (h-stack
                    (label "left")
                    (box :width 120 (label ""))))
                "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    editor.active_buffer_mut().widget_tree = None;

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 10, 2), 1, 1, 30, 8);

    assert_eq!(editor.widget_scroll_left(), 0.0);
}

#[test]
fn text_only_horizontal_scroll_requires_current_line_overflow() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "short\n01234567890123456789");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.active_buffer_mut().cursor = (0, 0);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 10, 2), 1, 1, 10, 8);
    assert_eq!(editor.widget_scroll_left(), 0.0);

    editor.active_buffer_mut().cursor = (1, 0);
    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 10, 2), 1, 1, 10, 8);

    assert_eq!(editor.widget_scroll_left(), 3.0);
}

#[test]
fn text_only_horizontal_scroll_resets_when_cursor_moves_to_fitting_line() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "short\n01234567890123456789");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.active_buffer_mut().cursor = (1, 12);

    editor.handle_mouse(mouse_event(MouseEventKind::ScrollRight, 10, 2), 1, 1, 10, 8);
    assert_eq!(editor.widget_scroll_left(), 3.0);

    editor.active_buffer_mut().cursor = (0, 0);
    let frame = crate::frame::build_render_frame(&mut editor, 10, 8);

    assert_eq!(editor.widget_scroll_left(), 0.0);
    assert_eq!(frame.widget_scroll_left, 0.0);
}

#[test]
fn text_only_mouse_click_uses_horizontal_scroll_offset() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "01234567890123456789");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.set_layout_viewport(10, 8);
    editor.active_leaf_mut().widget_scroll_left = 6.0;

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 1),
        1,
        1,
        10,
        8,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 9));
}

#[test]
fn text_only_mouse_drag_selection_uses_horizontal_scroll_offset() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "01234567890123456789");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.set_layout_viewport(10, 8);
    editor.active_leaf_mut().widget_scroll_left = 6.0;

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 1),
        1,
        1,
        10,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 6, 1),
        1,
        1,
        10,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 6, 1),
        1,
        1,
        10,
        8,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 11));
    assert_eq!(editor.active_region_range(), Some(((0, 8), (0, 11))));
}

#[test]
fn text_only_right_arrow_keeps_cursor_visible_horizontally() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "01234567890123456789");
    editor.active_buffer_mut().view_mode = super::ViewMode::TextOnly;
    editor.set_layout_viewport(10, 8);
    editor.active_buffer_mut().cursor = (0, 9);
    editor.active_leaf_mut().widget_scroll_left = 0.0;

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 10));
    assert_eq!(editor.widget_scroll_left(), 1.0);
}

#[test]
fn text_buffer_mouse_click_uses_horizontal_scroll_offset() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "01234567890123456789");
    editor.set_layout_viewport(10, 8);
    editor.active_leaf_mut().widget_scroll_left = 6.0;

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 1),
        1,
        1,
        10,
        8,
    );

    assert_eq!(editor.active_buffer().cursor, (0, 9));
}

#[test]
fn text_buffer_right_arrow_keeps_cursor_visible_horizontally() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "01234567890123456789");
    editor.set_layout_viewport(10, 8);
    editor.active_buffer_mut().cursor = (0, 9);
    editor.active_leaf_mut().widget_scroll_left = 0.0;

    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().cursor, (0, 10));
    assert_eq!(editor.widget_scroll_left(), 1.0);
}

#[test]
fn focused_timeline_right_arrow_emits_nudge_selection_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("nudge-selection".to_string())
    );
    assert_eq!(
        super::get_map_field_number(&action, "delta-time"),
        Some(1.0)
    );
}

#[test]
fn focused_timeline_delete_emits_delete_items_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("delete-items".to_string())
    );
    assert_eq!(super::get_first_list_number(&action, "ids"), Some(10.0));
}

#[test]
fn focused_timeline_cmd_a_emits_select_all_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list
                      (dict :id 10 :lane 0 :start 4 :end 8)
                      (dict :id 11 :lane 0 :start 9 :end 12))
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER));

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("select".to_string())
    );
    let Value::Map(map) = action else {
        panic!("expected action map");
    };
    let ids = map.get("ids").expect("ids");
    let Value::List(ids) = &*ids.borrow() else {
        panic!("expected ids list");
    };
    assert_eq!(ids.len(), 2);
}

#[test]
fn focused_timeline_escape_emits_clear_selection_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("clear-selection".to_string())
    );
}

#[test]
fn focused_timeline_d_shortcut_emits_set_tool_action() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def last-action (state nil))
                (effect
                  (timeline
                    :height 8
                    :focusable true
                    :tool :pointer
                    :lanes (list (dict :id 0 :label "L0"))
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8 :selected true))
                    :view-start 0
                    :view-duration 16
                    :snap 1
                    :on-action |e| (set! last-action e)))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 8);

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 11, 2),
        1,
        1,
        30,
        8,
    );
    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    let action = editor.runtime.eval_str("last-action").unwrap().unwrap();
    assert_eq!(
        super::get_map_field_keyword(&action, "type"),
        Some("set-tool".to_string())
    );
    assert_eq!(
        super::get_map_field_keyword(&action, "tool"),
        Some("draw".to_string())
    );
}

// ── defmacro tests ────────────────────────────────────────────────────

fn eval_number(runtime: &mut Runtime, src: &str) -> f64 {
    match runtime.eval_str(src).unwrap().unwrap() {
        Value::Number(n) => n,
        other => panic!("expected Number, got {:?}", other),
    }
}

#[test]
fn macro_basic_expansion() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro double (x) `(+ ,x ,x))").unwrap();
    assert_eq!(eval_number(&mut rt, "(double 5)"), 10.0);
}

#[test]
fn macro_nested_arg_expression() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro double (x) `(+ ,x ,x))").unwrap();
    assert_eq!(eval_number(&mut rt, "(double (+ 1 2))"), 6.0);
}

#[test]
fn macro_multiple_params() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro add3 (a b c) `(+ ,a ,b ,c))")
        .unwrap();
    assert_eq!(eval_number(&mut rt, "(add3 1 2 3)"), 6.0);
}

#[test]
fn macro_expanding_to_let() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro with-ten (body) `(let ((x 10)) ,body))")
        .unwrap();
    assert_eq!(eval_number(&mut rt, "(with-ten (+ x 1))"), 11.0);
}

#[test]
fn macro_using_macro() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro square (x) `(* ,x ,x))").unwrap();
    rt.eval_str("(defmacro sum-of-squares (a b) `(+ (square ,a) (square ,b)))")
        .unwrap();
    assert_eq!(eval_number(&mut rt, "(sum-of-squares 3 4)"), 25.0);
}

#[test]
fn macro_persistence_across_eval() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro inc (x) `(+ ,x 1))").unwrap();
    // Macro defined in previous eval_str should be available
    assert_eq!(eval_number(&mut rt, "(inc 41)"), 42.0);
}

#[test]
fn macro_with_literal_symbols() {
    // Symbols in quasiquote that aren't params stay literal
    let mut rt = Runtime::new();
    rt.eval_str("(def y 100)").unwrap();
    rt.eval_str("(defmacro use-y () `y)").unwrap();
    assert_eq!(eval_number(&mut rt, "(use-y)"), 100.0);
}

#[test]
fn defmacro_returns_nil() {
    let mut rt = Runtime::new();
    let result = rt
        .eval_str("(defmacro foo (x) `(+ ,x 1))")
        .unwrap()
        .unwrap();
    assert!(matches!(result, Value::Nil));
}

#[test]
fn macro_in_let_context() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro double (x) `(+ ,x ,x))").unwrap();
    assert_eq!(eval_number(&mut rt, "(let ((a 7)) (double a))"), 14.0);
}

#[test]
fn macro_in_lambda_context() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro double (x) `(+ ,x ,x))").unwrap();
    assert_eq!(eval_number(&mut rt, "((lambda (n) (double n)) 8)"), 16.0);
}

#[test]
fn macro_recursive_expansion() {
    // Macro A expands to a call of macro B
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro inc (x) `(+ ,x 1))").unwrap();
    rt.eval_str("(defmacro inc2 (x) `(inc (inc ,x)))").unwrap();
    assert_eq!(eval_number(&mut rt, "(inc2 10)"), 12.0);
}

#[test]
fn macro_with_keyword_args() {
    let mut rt = Runtime::new();
    rt.eval_str("(defmacro make-dict (k v) `(dict ,k ,v))")
        .unwrap();
    let result = rt
        .eval_str("(get (make-dict :name 42) :name)")
        .unwrap()
        .unwrap();
    assert!(matches!(result, Value::Number(n) if n == 42.0));
}

// ── Math native tests ─────────────────────────────────────────────────

#[test]
fn sin_cos_basic() {
    let mut rt = Runtime::new();
    assert!((eval_number(&mut rt, "(sin 0)")).abs() < 1e-10);
    assert!((eval_number(&mut rt, "(sin 1.5707963267948966)") - 1.0).abs() < 1e-10);
    assert!((eval_number(&mut rt, "(cos 0)") - 1.0).abs() < 1e-10);
    assert!((eval_number(&mut rt, "(cos 3.141592653589793)") + 1.0).abs() < 1e-10);
}

#[test]
fn sqrt_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(sqrt 4)"), 2.0);
    assert_eq!(eval_number(&mut rt, "(sqrt 0)"), 0.0);
    assert_eq!(eval_number(&mut rt, "(sqrt 1)"), 1.0);
}

#[test]
fn abs_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(abs -5)"), 5.0);
    assert_eq!(eval_number(&mut rt, "(abs 3)"), 3.0);
    assert_eq!(eval_number(&mut rt, "(abs 0)"), 0.0);
}

#[test]
fn floor_ceil_round() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(floor 2.7)"), 2.0);
    assert_eq!(eval_number(&mut rt, "(ceil 2.3)"), 3.0);
    assert_eq!(eval_number(&mut rt, "(round 2.5)"), 3.0);
}

#[test]
fn fract_basic() {
    let mut rt = Runtime::new();
    assert!((eval_number(&mut rt, "(fract 3.7)") - 0.7).abs() < 1e-10);
}

#[test]
fn pow_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(pow 2 3)"), 8.0);
    assert_eq!(eval_number(&mut rt, "(pow 4 0.5)"), 2.0);
}

#[test]
fn atan2_basic() {
    let mut rt = Runtime::new();
    assert!((eval_number(&mut rt, "(atan2 1 0)") - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
    assert!((eval_number(&mut rt, "(atan2 0 1)")).abs() < 1e-10);
}

#[test]
fn mod_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(mod 7 3)"), 1.0);
    assert_eq!(eval_number(&mut rt, "(mod 5.5 2)"), 1.5);
}

#[test]
fn clamp_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(clamp 5 0 1)"), 1.0);
    assert_eq!(eval_number(&mut rt, "(clamp -1 0 1)"), 0.0);
    assert_eq!(eval_number(&mut rt, "(clamp 0.5 0 1)"), 0.5);
    assert!(eval_number(&mut rt, "(clamp 0 5 4)").is_nan());
}

#[test]
fn mix_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(mix 0 10 0.5)"), 5.0);
    assert_eq!(eval_number(&mut rt, "(mix 0 10 0)"), 0.0);
    assert_eq!(eval_number(&mut rt, "(mix 0 10 1)"), 10.0);
}

#[test]
fn smoothstep_basic() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(smoothstep 0 1 0)"), 0.0);
    assert_eq!(eval_number(&mut rt, "(smoothstep 0 1 1)"), 1.0);
    assert_eq!(eval_number(&mut rt, "(smoothstep 0 1 0.5)"), 0.5);
}

#[test]
fn vec2_length() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(length (vec2 3 4))"), 5.0);
    assert_eq!(eval_number(&mut rt, "(length (vec2 0 0))"), 0.0);
}

#[test]
fn vec2_dot() {
    let mut rt = Runtime::new();
    assert_eq!(eval_number(&mut rt, "(dot (vec2 1 0) (vec2 0 1))"), 0.0);
    assert_eq!(eval_number(&mut rt, "(dot (vec2 2 3) (vec2 4 5))"), 23.0);
}

// ── SDF stdlib tests ──────────────────────────────────────────────────

#[test]
fn sdf_circle_at_origin() {
    let mut rt = Runtime::new();
    // Point at origin, circle radius 0.5: distance = -0.5 (inside)
    let d = eval_number(&mut rt, "(let ((x 0) (y 0)) (sdf/circle 0.5))");
    assert!((d - (-0.5)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_circle_on_boundary() {
    let mut rt = Runtime::new();
    let d = eval_number(&mut rt, "(let ((x 1) (y 0)) (sdf/circle 1.0))");
    assert!(d.abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_circle_outside() {
    let mut rt = Runtime::new();
    let d = eval_number(&mut rt, "(let ((x 2) (y 0)) (sdf/circle 0.5))");
    assert!((d - 1.5).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_rect_inside() {
    let mut rt = Runtime::new();
    // Point at origin, rect half-extents 2x1: distance = -1 (min of -2, -1)
    let d = eval_number(&mut rt, "(let ((x 0) (y 0)) (sdf/rect 2 1))");
    assert!((d - (-1.0)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_rect_on_edge() {
    let mut rt = Runtime::new();
    // Point on the right edge of a 2x1 rect
    let d = eval_number(&mut rt, "(let ((x 2) (y 0)) (sdf/rect 2 1))");
    assert!(d.abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_rect_corner() {
    let mut rt = Runtime::new();
    // Point at (3, 2) outside a 2x1 rect: corner distance = sqrt(1+1) = sqrt(2)
    let d = eval_number(&mut rt, "(let ((x 3) (y 2)) (sdf/rect 2 1))");
    assert!((d - 2.0_f64.sqrt()).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_rounded_rect() {
    let mut rt = Runtime::new();
    // Rounded rect with r=0.1 should have distance reduced by r at origin
    let d_rect = eval_number(&mut rt, "(let ((x 0) (y 0)) (sdf/rect 2 1))");
    let d_rounded = eval_number(&mut rt, "(let ((x 0) (y 0)) (sdf/rounded-rect 2 1 0.1))");
    // rounded-rect at origin = rect(w-r, h-r) - r = rect(1.9, 0.9) - 0.1
    // rect(1.9, 0.9) at origin = min(-1.9, -0.9) = -0.9
    // so rounded = -0.9 - 0.1 = -1.0, same as rect(2,1)
    assert!(
        (d_rect - d_rounded).abs() < 1e-10,
        "rect={} rounded={}",
        d_rect,
        d_rounded
    );
}

#[test]
fn sdf_translate_circle() {
    let mut rt = Runtime::new();
    // Circle at (1,0), point at (1,0) → should be inside
    let d = eval_number(
        &mut rt,
        "(let ((x 1) (y 0)) (sdf/translate 1 0 (sdf/circle 0.5)))",
    );
    assert!((d - (-0.5)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_union() {
    let mut rt = Runtime::new();
    // Union = min of two distances
    let d = eval_number(
        &mut rt,
        "(let ((x 0) (y 0)) (sdf/union (sdf/circle 1) (sdf/circle 0.5)))",
    );
    assert!((d - (-1.0)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_subtract() {
    let mut rt = Runtime::new();
    // Subtract = max(d1, -d2)
    let d = eval_number(
        &mut rt,
        "(let ((x 0) (y 0)) (sdf/subtract (sdf/circle 1) (sdf/circle 0.5)))",
    );
    // d1 = -1, d2 = -0.5, result = max(-1, 0.5) = 0.5
    assert!((d - 0.5).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_intersect() {
    let mut rt = Runtime::new();
    // Intersect = max(d1, d2)
    let d = eval_number(
        &mut rt,
        "(let ((x 0) (y 0)) (sdf/intersect (sdf/circle 1) (sdf/circle 0.5)))",
    );
    // max(-1, -0.5) = -0.5
    assert!((d - (-0.5)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_scale_circle() {
    let mut rt = Runtime::new();
    // Scale a unit circle by 2 → effective radius 2
    let d = eval_number(&mut rt, "(let ((x 0) (y 0)) (sdf/scale 2 (sdf/circle 1)))");
    assert!((d - (-2.0)).abs() < 1e-10, "got {}", d);
}

#[test]
fn sdf_to_metal_native() {
    let mut rt = Runtime::new();
    let result = rt
        .eval_str("(sdf->metal '(sdf/circle 0.5))")
        .unwrap()
        .unwrap();
    if let Value::String(shader) = result {
        assert!(shader.contains("fragment float4 widget_frag"));
        assert!(shader.contains("length(float2(x, y))"));
        assert!(shader.contains("discard_fragment"));
    } else {
        panic!("expected String, got {:?}", result);
    }
}

#[test]
fn defwidget_registers_widget_for_later_forms_in_same_eval() {
    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (defwidget sdf-test
                  :width 3 :height 3
                  :shader (sdf/layer
                            (sdf/fill (sdf/circle 0.7) :accent)))
                (sdf-test)
                "#,
        )
        .unwrap()
        .unwrap();

    let Value::Map(widget) = result else {
        panic!("expected widget map");
    };
    let widget_type = widget
        .get("type")
        .map(|value| value.borrow().clone())
        .unwrap_or(Value::Nil);
    assert!(matches!(widget_type, Value::Keyword(name) if name == "sdf-test"));
}

#[test]
fn defwidget_uses_macros_defined_earlier_in_same_eval() {
    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (defmacro chasis (w h)
                  `(sdf/rect ,w ,h))
                (defwidget spore
                  :width 8 :height 8
                  :shader
                  (sdf/layer
                    (sdf/fill (chasis 0.5 0.3) :accent)))
                (spore)
                "#,
        )
        .unwrap()
        .unwrap();

    let Value::Map(widget) = result else {
        panic!("expected widget map");
    };
    let widget_type = widget
        .get("type")
        .map(|value| value.borrow().clone())
        .unwrap_or(Value::Nil);
    assert!(matches!(widget_type, Value::Keyword(name) if name == "spore"));
}

#[test]
fn defwidget_stores_paint_margin_without_changing_measure_size() {
    use crate::widget_render::sdf_widget::{sdf_widget_def, sdf_widget_measure};

    let mut rt = Runtime::new();
    rt.eval_str(
        r#"
            (defwidget sdf-shadowed
              :width 2 :height 2
              :paint-margin 1
              :shader (sdf/layer
                        (sdf/fill (sdf/circle 0.7)
                          (material
                            :color :accent
                            :shadow (shadow :color (rgba 0 0 0 0.2)
                                            :blur 0.18
                                            :offset (vec2 0 0.05))))))
            "#,
    )
    .unwrap();

    let def = sdf_widget_def("sdf-shadowed").expect("sdf widget def");
    assert_eq!(def.paint_margin, 1.0);
    assert_eq!(def.width, 2.0);
    assert_eq!(def.height, 2.0);

    let size = sdf_widget_measure(
        "sdf-shadowed",
        &Value::Nil,
        &[],
        crate::layout::Constraints {
            min_width: 0.0,
            max_width: 100.0,
            min_height: 0.0,
            max_height: 100.0,
            aspect: 1.0,
        },
        &crate::layout::MeasureCtx {
            cell_w: 1.0,
            cell_h: 1.0,
            text_measurer: None,
            inherited_font_size: 14.0,
        },
    )
    .expect("measure size");

    assert_eq!(size.width, 2.0);
    assert_eq!(size.height, 2.0);
}

#[test]
fn defwidget_derives_paint_margin_from_shadow_material() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    rt.eval_str(
        r#"
            (defwidget sdf-auto-shadow
              :width 2 :height 2
              :shader (sdf/layer
                        (sdf/fill (sdf/circle 0.7)
                          (material
                            :color :accent
                            :shadow (shadow :color (rgba 0 0 0 0.2)
                                            :blur 0.8
                                            :offset (vec2 0 0.4))))))
            "#,
    )
    .unwrap();

    let def = sdf_widget_def("sdf-auto-shadow").expect("sdf widget def");
    assert!(def.paint_margin >= 1.0);
}

#[test]
fn sdf_rotate_90deg() {
    let mut rt = Runtime::new();
    // A 2x0.5 rect is wide and short. Point at (1.5, 0) is inside (dx=1.5-2=-0.5, dy=0-0.5=-0.5).
    // After 90° rotation, the rect becomes tall and narrow (0.5 wide, 2 tall).
    // Point (1.5, 0) should now be outside the rotated rect.
    let pi_2 = std::f64::consts::FRAC_PI_2;
    let d_no_rot = eval_number(&mut rt, "(let ((x 1.5) (y 0)) (sdf/rect 2 0.5))");
    assert!(
        d_no_rot < 0.0,
        "point should be inside un-rotated rect, got {}",
        d_no_rot
    );

    let d_rot = eval_number(
        &mut rt,
        &format!(
            "(let ((x 1.5) (y 0)) (sdf/rotate {} (sdf/rect 2 0.5)))",
            pi_2
        ),
    );
    assert!(
        d_rot > 0.0,
        "point should be outside 90°-rotated rect, got {}",
        d_rot
    );
}

#[test]
fn hslider_material_in_effect_simple() {
    // Simple test: material with just :color :accent inside effect
    let rt = Runtime::new();
    let mut editor = Editor::new(rt, EditorConfig::default());
    editor.set_layout_viewport(40, 20);
    let result = editor.runtime.eval_str(
        r#"
            (defstate test-vol 0.5)
            (effect
              (hslider :min 0 :max 1 :bind test-vol :width 20
                :material (material :color :accent)))
            "#,
    );
    assert!(result.is_ok(), "simple material failed: {:?}", result.err());
}

#[test]
fn hslider_material_with_y_no_effect() {
    // Material with `y` WITHOUT effect wrapper — does auto-quoting work?
    let mut rt = Runtime::new();
    let result = rt.eval_str(
        r#"(hslider :min 0 :max 1 :value 0.5
                :material (material :color (rgba y 0 0 1)))"#,
    );
    assert!(
        result.is_ok(),
        "material with y (no effect) failed: {:?}",
        result.err()
    );
}

#[test]
fn hslider_material_in_effect_with_y_variable() {
    // Material that references shader variable `y` inside effect
    let rt = Runtime::new();
    let mut editor = Editor::new(rt, EditorConfig::default());
    editor.set_layout_viewport(40, 20);
    let result = editor.runtime.eval_str(
        r#"
            (defstate test-vol 0.5)
            (effect
              (hslider :min 0 :max 1 :bind test-vol :width 20
                :material (material
                  :color (rgba y 0 0 1))))
            "#,
    );
    assert!(
        result.is_ok(),
        "material with y (in effect) failed: {:?}",
        result.err()
    );
}

#[test]
fn hslider_material_in_effect_with_lighting() {
    // Full material with lighting + vec3 inside effect
    let rt = Runtime::new();
    let mut editor = Editor::new(rt, EditorConfig::default());
    editor.set_layout_viewport(40, 20);
    let result = editor.runtime.eval_str(
        r#"
            (defstate test-vol 0.5)
            (effect
              (hslider :min 0 :max 1 :bind test-vol :width 20
                :material (material
                  :lighting (lighting :edge-min -0.35 :edge-max 0.5
                    :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
                  :color :accent)))
            "#,
    );
    assert!(
        result.is_ok(),
        "material with lighting failed: {:?}",
        result.err()
    );
}

#[test]
fn hslider_material_in_effect_with_aqua_macro() {
    let rt = Runtime::new();
    let mut editor = Editor::new(rt, EditorConfig::default());
    editor.set_layout_viewport(40, 20);
    let result = editor.runtime.eval_str(
        r#"
            (defmacro test-aqua (base1 base2)
              `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
                    (__base (mix ,base1 ,base2 (smoothstep -0.5 0.5 __ny)))
                    (__rim (smoothstep -0.53 -0.033 d)))
                 (* __base (rgba __rim __rim __rim 1.0))))

            (defstate test-vol 0.5)
            (effect
              (hslider :min 0 :max 1 :bind test-vol :width 20
                :material (material
                  :lighting (lighting :edge-min -0.35 :edge-max 0.5
                    :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
                  :color (test-aqua (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0)))))
            "#,
    );
    assert!(
        result.is_ok(),
        "aqua macro material failed: {:?}",
        result.err()
    );
}

#[test]
fn hslider_material_with_lighting_and_vec3() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (hslider :min 0 :max 1 :value 0.5
                  :material (material
                    :lighting (lighting :edge-min -0.35 :edge-max 0.5
                      :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
                    :color :accent))
                "#,
        )
        .unwrap()
        .unwrap();

    if let Value::Map(map) = &result {
        let shader_type = map
            .get("__shader_type")
            .expect("hslider with :material+lighting should have __shader_type");
        if let Value::String(name) = &*shader_type.borrow() {
            assert!(
                sdf_widget_def(name).is_some(),
                "shader '{}' should be registered",
                name
            );
        } else {
            panic!("__shader_type should be a string");
        }
    } else {
        panic!("hslider should return a map");
    }
}

#[test]
fn hslider_material_with_macro_containing_vec3() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    // Define the aqua-color macro (same as in the demo)
    rt.eval_str(
        r#"
            (defmacro aqua-color (base1 base2)
              `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
                    (__base (mix ,base1 ,base2 (smoothstep -0.5 0.5 __ny)))
                    (__glass (smoothstep 0.1 -0.65 __ny))
                    (__edge-fade (smoothstep 0.0 -0.26 d))
                    (__hi (* __glass __edge-fade 0.655))
                    (__spec (* specular __edge-fade 0.3))
                    (__rim (smoothstep -0.53 -0.033 d)))
                 (+ (* __base (rgba __rim __rim __rim 1.0))
                    (rgba (+ __hi __spec) (+ __hi __spec) (+ __hi __spec) 0.0))))
            "#,
    )
    .unwrap();

    // Use the macro inside a :material expression
    let result = rt
        .eval_str(
            r#"
                (hslider :min 0 :max 1 :value 0.5
                  :material (material
                    :lighting (lighting :edge-min -0.35 :edge-max 0.5
                      :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
                    :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0))))
                "#,
        )
        .unwrap()
        .unwrap();

    if let Value::Map(map) = &result {
        let shader_type = map
            .get("__shader_type")
            .expect("hslider with aqua-color material should have __shader_type");
        if let Value::String(name) = &*shader_type.borrow() {
            assert!(
                sdf_widget_def(name).is_some(),
                "shader '{}' should be registered",
                name
            );
        } else {
            panic!("__shader_type should be a string");
        }
    } else {
        panic!("hslider should return a map");
    }
}

#[test]
fn hslider_material_compiles_and_sets_shader_type() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (hslider :min 0 :max 1 :value 0.5
                  :material (material :color :accent))
                "#,
        )
        .unwrap()
        .unwrap();

    // The widget should have __shader_type set
    if let Value::Map(map) = &result {
        let shader_type = map
            .get("__shader_type")
            .expect("hslider with :material should have __shader_type");
        if let Value::String(name) = &*shader_type.borrow() {
            // The compiled SDF widget should be registered
            assert!(
                sdf_widget_def(name).is_some(),
                "shader '{}' should be registered as SDF widget",
                name
            );
        } else {
            panic!("__shader_type should be a string");
        }
    } else {
        panic!("hslider should return a map");
    }
}

#[test]
fn vslider_material_shader_output_simple() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    // Test with simple color to check fill geometry
    let result = rt
        .eval_str(
            r#"(vslider :min 0 :max 1 :value 0.5 :material (material :color (rgba y 0 0 1)))"#,
        )
        .unwrap()
        .unwrap();

    if let Value::Map(map) = &result {
        if let Some(st) = map.get("__shader_type") {
            if let Value::String(name) = &*st.borrow() {
                let def = sdf_widget_def(name).unwrap();
                assert!(
                    def.shader_source.contains("(0.32 * aspect)"),
                    "expected native vslider width term in shader source"
                );
                assert!(
                    def.shader_source.contains("(aspect * y)"),
                    "expected native vslider y remap in shader source"
                );
            }
        }
    }
}

#[test]
fn vslider_material_includes_origin_t_state() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (vslider :min 0 :max 100 :value 50 :origin 25
                  :material (material :color :accent))
                "#,
        )
        .unwrap()
        .unwrap();

    if let Value::Map(map) = &result {
        let shader_type = map
            .get("__shader_type")
            .expect("vslider with :material should have __shader_type");
        if let Value::String(name) = &*shader_type.borrow() {
            assert!(
                sdf_widget_def(name).is_some(),
                "shader '{}' should be registered",
                name
            );
        } else {
            panic!("__shader_type should be a string");
        }
        // origin_t prop should still be set (25/100 = 0.25)
        let origin_prop = map
            .get("shader-state-origin_t")
            .expect("vslider should have origin_t prop");
        if let Value::Number(n) = &*origin_prop.borrow() {
            assert!(
                (*n - 0.25).abs() < 0.001,
                "origin_t should be 0.25, got {}",
                n
            );
        }
    } else {
        panic!("vslider should return a map");
    }
}

#[test]
fn vslider_material_captures_explicit_props_as_uniforms() {
    use crate::widget_render::sdf_widget::sdf_widget_def;

    let mut rt = Runtime::new();
    let result = rt
        .eval_str(
            r#"
                (vslider :min 0 :max 1 :value 0.5
                  :track-r 0.8 :track-g 0.4 :track-b 0.2
                  :material (material :color (rgba track-r track-g track-b 1)))
                "#,
        )
        .unwrap()
        .unwrap();

    let Value::Map(map) = &result else {
        panic!("vslider should return a map");
    };
    let Value::String(shader_name) = &*map
        .get("__shader_type")
        .expect("vslider with :material should have __shader_type")
        .borrow()
    else {
        panic!("__shader_type should be a string");
    };
    let def = sdf_widget_def(shader_name).expect("shader should be registered");
    assert_eq!(
        def.state_uniforms,
        vec![
            "origin_t".to_string(),
            "track-r".to_string(),
            "track-g".to_string(),
            "track-b".to_string()
        ]
    );
    assert!(
        def.shader_source.contains("sdf_state_track_r"),
        "track-r should compile as a state uniform"
    );
    for (prop, expected) in [
        ("shader-state-track-r", 0.8),
        ("shader-state-track-g", 0.4),
        ("shader-state-track-b", 0.2),
    ] {
        let Value::Number(actual) = &*map.get(prop).expect(prop).borrow() else {
            panic!("{prop} should be a number");
        };
        assert!((actual - expected).abs() < 0.001);
    }
}

#[test]
fn slider_material_caches_identical_expressions() {
    let mut rt = Runtime::new();
    // Compile two sliders with the same material
    let r1 = rt
        .eval_str(r#"(hslider :material (material :color :accent))"#)
        .unwrap()
        .unwrap();
    let r2 = rt
        .eval_str(r#"(hslider :material (material :color :accent))"#)
        .unwrap()
        .unwrap();

    let name1 = if let Value::Map(m) = &r1 {
        if let Value::String(s) = &*m.get("__shader_type").unwrap().borrow() {
            s.clone()
        } else {
            panic!()
        }
    } else {
        panic!()
    };
    let name2 = if let Value::Map(m) = &r2 {
        if let Value::String(s) = &*m.get("__shader_type").unwrap().borrow() {
            s.clone()
        } else {
            panic!()
        }
    } else {
        panic!()
    };

    assert_eq!(
        name1, name2,
        "identical materials should produce the same shader name"
    );
}
