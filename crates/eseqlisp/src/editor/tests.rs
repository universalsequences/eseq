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
fn lisp_key_handler_source_context_tracks_buffer_revisions_and_switches() {
    let init = r#"
        (def capture-source ()
          (host-command "capture-source" (current-buffer-text)))
        (bind-key "C-k" "capture-source")
    "#;
    let mut editor = Editor::new(
        Runtime::new(),
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.open_scratch_buffer("*first*", "first");

    let capture = |editor: &mut Editor| {
        editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        let command = editor
            .drain_host_commands()
            .into_iter()
            .next()
            .expect("capture-source command");
        match command {
            HostCommand::Custom {
                name,
                payload: Value::String(source),
            } => {
                assert_eq!(name, "capture-source");
                source
            }
            other => panic!("unexpected source capture command: {other:?}"),
        }
    };

    assert_eq!(capture(&mut editor), "first");
    editor.active_buffer_mut().set_text("first revised");
    assert_eq!(capture(&mut editor), "first revised");
    editor.open_scratch_buffer("*second*", "second");
    assert_eq!(capture(&mut editor), "second");
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
    let input = super::find_layout_node_by_stable_key(layout.as_ref(), "search-input")
        .expect("search input");
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
fn remapped_focus_after_relayout_does_not_reselect_all() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def name (state "Track"))
            (effect
              (box :width 24 :height 3
                (text-input
                  :key "rename-input"
                  :width 20
                  :value name
                  :select-all-on-focus true
                  :on-change |v| (set! name v))))
            "#,
        )
        .expect("build rename fixture");
    editor.refresh_runtime_side_effects();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(30, 8);
    editor.widget_layout().expect("rename layout");
    assert!(editor.focus_widget_by_stable_key("rename-input", Some("text-input")));

    // Select-all-on-focus: the first keystroke replaces the whole name.
    editor.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_eq!(
        editor.runtime_mut().eval_str("name"),
        Ok(Some(Value::String("l".to_string())))
    );

    // A relayout can hand the focused input a fresh widget_id; the post-layout
    // remap then re-focuses the same logical widget under the new id. That
    // must not re-apply select-all (it would make the next keystroke replace
    // what was just typed) and must carry the caret to the new id.
    let layout = editor.widget_layout().expect("layout after first keystroke");
    let mut remapped = super::find_layout_node_by_stable_key(layout.as_ref(), "rename-input")
        .expect("rename input after keystroke")
        .clone();
    remapped.widget_id += 101;
    editor.set_focused_widget(remapped);

    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert_eq!(
        editor.runtime_mut().eval_str("name"),
        Ok(Some(Value::String("lu".to_string()))),
        "remapped focus must keep the caret at the end, not re-select the typed text"
    );
}

#[test]
fn text_input_auto_focus_submit_and_blur_callbacks_drive_inline_edit_lifecycle() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.runtime_mut().eval_str(
        r#"
        (def editing (state true))
        (def submitted (state false))
        (def cancelled (state false))
        (def blurred (state false))
        (effect
          (box :width 24 :height 4
            (if editing
              (text-input :key "rename-input" :width 20 :value "Track"
                :auto-focus true :select-all-on-focus true
                :on-submit (lambda () (do (set! submitted true) (set! editing false)))
                :on-cancel (lambda () (do (set! cancelled true) (set! editing false)))
                :on-blur (lambda () (set! blurred true)))
              (label "done"))))
        "#,
    ).expect("build inline edit fixture");
    editor.refresh_runtime_side_effects();
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(30, 8);
    editor.widget_layout().expect("inline edit layout");
    assert!(editor.focused_widget_id().is_some(), "auto-focus should focus the input");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(editor.runtime_mut().eval_str("cancelled"), Ok(Some(Value::Bool(true))));

    editor.runtime_mut().eval_str("(set! editing true)").unwrap();
    editor.refresh_runtime_side_effects();
    editor.widget_layout().expect("submit edit layout");
    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(editor.runtime_mut().eval_str("submitted"), Ok(Some(Value::Bool(true))));

    editor.runtime_mut().eval_str("(set! editing true)").unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("rebuilt inline edit layout");
    let input = super::find_layout_node_by_stable_key(layout.as_ref(), "rename-input")
        .expect("rebuilt rename input");
    editor.focus_widget_by_stable_key("rename-input", Some("text-input"));
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 25, 3),
        0,
        0,
        30,
        8,
        input.rect.col + input.rect.width + 2.0,
        input.rect.row + input.rect.height + 1.0,
    );
    assert_eq!(editor.runtime_mut().eval_str("blurred"), Ok(Some(Value::Bool(true))));
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
fn hot_reload_reevaluates_imported_children_from_the_owner_root() {
    // Module spec §4/§11 q4. Editing a child re-evaluates its *owner root*
    // (`reload_paths_transactional` → `owner_root_for`), and after
    // eseq-mods.9 the root reaches its children through `import`, not
    // `load`. Import is load-once, so without a per-reload reset the root
    // re-eval would skip every child and the edit would silently not land —
    // the whole ui/ tree would stop hot-reloading.
    let dir = hot_reload_temp_dir("eseqlisp-hot-import-child");
    let root = dir.join("root.lisp");
    let child = dir.join("hot-import-child.lisp");
    std::fs::write(
        &root,
        r#"(import hot-import-child)
(effect-buffer "*hot-import*" (label (hot-import-child/child-label)))"#,
    )
    .unwrap();
    std::fs::write(
        &child,
        "(module hot-import-child)\n(def child-label () \"before\")",
    )
    .unwrap();

    let mut runtime = Runtime::new();
    let source = std::fs::read_to_string(&root).unwrap();
    let report = runtime.eval_source_transactional(Some(root.clone()), &source, Vec::new());
    assert!(
        report.success,
        "initial root eval failed: {:?}",
        report.diagnostics
    );
    assert_eq!(
        runtime.eval_str("(hot-import-child/child-label)"),
        Ok(Some(Value::String("before".to_string())))
    );

    std::fs::write(
        &child,
        "(module hot-import-child)\n(def child-label () \"after\")",
    )
    .unwrap();
    let report = runtime.reload_paths_transactional(vec![child.clone()], Vec::new());
    assert!(
        report.success,
        "child reload failed: {:?}",
        report.diagnostics
    );
    assert_eq!(
        runtime.eval_str("(hot-import-child/child-label)"),
        Ok(Some(Value::String("after".to_string()))),
        "re-evaluating the root must re-import the edited child, not skip it"
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
    // Direct programmatic reloads bypass the interactive authoring path, so no
    // AuthoringTransactionBegin/End pair should be emitted; the pair is the
    // responsibility of `evaluate_buffer_transactional` and is covered by
    // `eval_current_buffer_native_uses_transactional_reload`.
    let commands = editor.drain_host_commands();
    assert!(
        commands.iter().all(|command| !matches!(
            command,
            crate::host::HostCommand::AuthoringTransactionBegin { .. }
                | crate::host::HostCommand::AuthoringTransactionEnd { .. }
        )),
        "programmatic hot reload should not emit authoring transaction markers"
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
fn hovering_vertical_tile_border_uses_horizontal_resize_cursor() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");
    editor.split_active_tile(SplitDir::Vertical, 0);
    editor.update_tile_rects(100, 40);

    editor.handle_tiled_mouse_precise(mouse_event(MouseEventKind::Moved, 50, 10), 50.0, 10.0, 1);

    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::EwResize
    );

    editor.handle_tiled_mouse_precise(mouse_event(MouseEventKind::Moved, 2, 10), 2.0, 10.0, 1);

    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::Default
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
fn hovering_horizontal_tile_border_uses_vertical_resize_cursor() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(+ 1 2)");
    editor.split_active_tile(SplitDir::Horizontal, 0);
    editor.update_tile_rects(80, 40);

    editor.handle_tiled_mouse_precise(mouse_event(MouseEventKind::Moved, 20, 19), 20.0, 19.0, 1);

    assert_eq!(
        editor.widget_cursor(),
        crate::widget_render::WidgetCursor::NsResize
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
fn first_click_opens_an_unfocused_conditionally_replaced_dropdown() {
    fn find_dropdown(node: &crate::layout::LayoutNode) -> Option<&crate::layout::LayoutNode> {
        if node.widget_type == "dropdown" {
            return Some(node);
        }
        node.children.iter().find_map(find_dropdown)
    }

    fn click(editor: &mut Editor, dropdown: &crate::layout::LayoutNode) {
        let col = dropdown.rect.col + dropdown.rect.width * 0.5;
        let row = dropdown.rect.row + dropdown.rect.height * 0.5;
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            editor.handle_mouse_precise(
                mouse_event(kind, col.floor() as u16, row.floor() as u16),
                0,
                0,
                30,
                10,
                col,
                row,
            );
        }
    }

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def dropdown-page (state 0))
            (effect
              (v-stack
                (if (= dropdown-page 0)
                  (subtree :key "synth-dropdown"
                    (dropdown :value "main" :options '("main" "alt")))
                  (subtree :key "mods-dropdown"
                    (dropdown :value "off" :options '("off" "lfo" "env"))))))
            "#,
        )
        .unwrap();
    editor.set_layout_viewport(30, 10);

    let old_dropdown = editor
        .runtime
        .current_layout
        .as_ref()
        .and_then(|layout| find_dropdown(layout))
        .expect("initial dropdown")
        .clone();
    click(&mut editor, &old_dropdown);
    assert!(crate::widget_render::dropdown::is_dropdown_open(
        old_dropdown.widget_id
    ));

    editor.runtime_mut().eval_str("(set! dropdown-page 1)").unwrap();
    crate::widget_render::clear_overlay();
    editor.clear_focused_widget();
    let mods_dropdown = editor
        .runtime
        .current_layout
        .as_ref()
        .and_then(|layout| find_dropdown(layout))
        .expect("conditionally shown mods dropdown")
        .clone();
    assert_eq!(
        mods_dropdown.widget_id, old_dropdown.widget_id,
        "the regression requires conditional subtrees to reuse a layout-local widget ID"
    );
    assert_ne!(
        mods_dropdown.subtree_root_id, old_dropdown.subtree_root_id,
        "the replacement dropdowns must retain distinct stable subtree identities"
    );

    click(&mut editor, &mods_dropdown);

    assert_eq!(editor.focused_widget_id(), Some(mods_dropdown.widget_id));
    assert!(
        crate::widget_render::dropdown::is_dropdown_open(mods_dropdown.widget_id),
        "one click must both focus and open a newly shown dropdown instead of closing stale state"
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
fn vim_gd_jumps_to_definition() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    let defs_id = editor.open_scratch_buffer("*defs*", "(def target 42)");
    let callsite_id = editor.open_scratch_buffer("*main*", "(target)");
    editor.set_active_buffer(callsite_id);
    editor.active_buffer_mut().cursor = (0, 1);

    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().id, defs_id);
    assert_eq!(editor.active_buffer().cursor, (0, 5));
    assert_eq!(editor.vim_input_mode, VimInputMode::Normal);
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
fn undo_and_redo_preserve_viewport_when_restored_cursor_is_visible() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            vim_mode: true,
            ..EditorConfig::default()
        },
    );
    let source = (0..80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.open_scratch_buffer("*test*", &source);
    editor.set_layout_viewport(20, 10);
    editor.active_buffer_mut().cursor = (45, 0);
    editor.active_buffer_mut().scroll_top = 40;

    editor.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), source);
    assert_eq!(editor.active_buffer().scroll_top, 40);

    editor.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

    assert_eq!(editor.active_buffer().lines[45], "Xline 45");
    assert_eq!(editor.active_buffer().scroll_top, 40);
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
fn tab_accepts_contextual_box_keyword_completion() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(box :back");
    editor.active_buffer_mut().cursor = (0, "(box :back".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(editor.active_buffer().text(), "(box :background");
}

#[test]
fn typing_bare_colon_opens_box_keyword_completion() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*test*", "(box ");
    editor.active_buffer_mut().cursor = (0, "(box ".len());

    editor.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));

    let completion = editor.completion_state().expect("box keyword completion");
    assert!(completion.items.iter().any(|item| item.label == ":background"));
    assert!(completion.items.iter().any(|item| item.label == ":on-click"));
}

#[test]
fn bare_colon_opens_keyword_completion_for_common_widget_families() {
    for (form, expected_keyword) in [
        ("button", ":on-click"),
        ("slider", ":on-change"),
        ("hslider", ":haptic-value"),
        ("vslider", ":origin"),
        ("number-picker", ":decimals"),
        ("dropdown", ":options"),
        ("menu-button", ":options"),
        ("select", ":options"),
        ("v-stack", ":justify"),
        ("h-stack", ":justify"),
        ("label", ":font-size"),
        ("text-input", ":placeholder"),
        ("textbox", ":max-lines"),
    ] {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        let source = format!("({form} ");
        editor.open_scratch_buffer("*test*", &source);
        editor.active_buffer_mut().cursor = (0, source.len());

        editor.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));

        let completion = editor
            .completion_state()
            .unwrap_or_else(|| panic!("{form} keyword completion"));
        assert!(
            completion.items.iter().any(|item| item.label == expected_keyword),
            "{form} should offer {expected_keyword}"
        );
    }
}

#[test]
fn widget_aliases_share_canonical_keyword_metadata() {
    let runtime = Runtime::new();
    let metadata = runtime.symbol_metadata();

    assert_eq!(metadata["slider"].keyword_args, metadata["hslider"].keyword_args);
    assert_eq!(metadata["dropdown"].keyword_args, metadata["menu-button"].keyword_args);
    assert_eq!(metadata["dropdown"].keyword_args, metadata["select"].keyword_args);
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
    assert!(
        editor
            .completion_state()
            .unwrap()
            .items
            .iter()
            .any(|item| item.label == "def")
    );
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
fn tree_header_rows_are_not_selectable_or_keyboard_targets() {
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
                    :row-height 1.0
                    :items '(
                      (:label "Built-in" :kind "header")
                      (:label "Sampler" :kind "builtin-instrument")
                      (:label "Library" :kind "header")
                      (:label "digitone" :kind "instrument"))
                    :on-select (lambda (item) (set! selected (get item :label)))
                    :on-cursor-change (lambda (item) (set! highlighted (get item :label)))
                    :on-activate (lambda (item) (set! activated (get item :label)))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 10);
    let layout = editor.widget_layout().expect("tree layout");
    assert_eq!(layout.widget_type, "tree");
    let col = layout.rect.col + 1.0;
    let row_y = |row: f32| layout.rect.row + row + 0.5;

    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col as u16,
            row_y(1.0) as u16,
        ),
        0,
        0,
        40,
        10,
        col,
        row_y(1.0),
    );
    assert_eq!(
        editor.runtime.eval_str("selected").unwrap().unwrap(),
        Value::String("Sampler".to_string())
    );

    editor
        .runtime
        .eval_str(r#"(set! selected "")"#)
        .expect("reset selected");
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col as u16,
            row_y(0.0) as u16,
        ),
        0,
        0,
        40,
        10,
        col,
        row_y(0.0),
    );
    assert_eq!(
        editor.runtime.eval_str("selected").unwrap().unwrap(),
        Value::String(String::new()),
        "clicking a header must not invoke on-select"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime.eval_str("highlighted").unwrap().unwrap(),
        Value::String("digitone".to_string()),
        "keyboard navigation should skip header rows"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime.eval_str("highlighted").unwrap().unwrap(),
        Value::String("Sampler".to_string()),
        "reverse keyboard navigation should skip header rows"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime.eval_str("activated").unwrap().unwrap(),
        Value::String("Sampler".to_string())
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
fn box_drag_modifier_latches_shift_pointer_mode_instead_of_drag_and_drop() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(40, 8);
    editor
        .runtime
        .eval_str(
            r#"
                (def pushed (state false))
                (def dragged-with-shift (state false))
                (def released (state false))
                (def dropped (state 0))
                (effect
                  (box :width 20 :height 4
                       :capture-pointer true
                       :drag-type "scene"
                       :drag-modifier :none
                       :drag-payload (dict :scene 0)
                       :drop-types '("scene")
                       :on-drop (lambda (event) (set! dropped (+ dropped 1)))
                       :on-mouse-down (lambda (event) (set! pushed (get event :shift)))
                       :on-drag (lambda (event) (set! dragged-with-shift (get event :shift)))
                       :on-mouse-up (lambda (event) (set! released true))))
                "#,
        )
        .unwrap();
    editor.set_layout_viewport(40, 8);

    let shifted_down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 1,
        modifiers: KeyModifiers::SHIFT,
    };
    editor.handle_mouse(shifted_down, 0, 0, 40, 8);
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 8, 3),
        0,
        0,
        40,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 30, 7),
        0,
        0,
        40,
        8,
    );

    assert_eq!(
        editor.runtime.eval_str("pushed").unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        editor.runtime.eval_str("dragged-with-shift").unwrap(),
        Some(Value::Bool(true)),
        "the pointer-down modifier must remain latched for the whole gesture"
    );
    assert_eq!(
        editor.runtime.eval_str("released").unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap(),
        Some(Value::Number(0.0))
    );

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        0,
        0,
        40,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 8, 3),
        0,
        0,
        40,
        8,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 8, 3),
        0,
        0,
        40,
        8,
    );
    assert_eq!(
        editor.runtime.eval_str("dropped").unwrap(),
        Some(Value::Number(1.0)),
        "an unmodified gesture must retain the original drag-and-drop mode"
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
fn cmd_k_opens_visible_patcher_agentic_bubble_without_widget_focus() {
    let path = temp_file_path("patcher-visible-cmd-k");
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
    editor.clear_focused_widget();
    assert_eq!(
        crate::widget_render::patcher::patcher_agentic_bubble_count(&patcher),
        0
    );

    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::SUPER));

    assert_eq!(
        crate::widget_render::patcher::patcher_agentic_bubble_count(&patcher),
        1,
        "cmd+k should open a bubble on the visible patcher without clicking it first"
    );
    assert_eq!(
        editor
            .focused_widget_node()
            .expect("cmd+k should focus the patcher it opened a bubble on")
            .widget_type,
        "patcher"
    );
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
    editor.set_text_cell_dimensions(10.0, 20.0, 10.0, 20.0);

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.show_border = true;
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
    let precise_row: f32 = 10.5 + 0.1 + 0.9;
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
fn button_release_stays_bound_to_the_original_pressed_button() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(8, 4);
    editor
        .runtime
        .eval_str(
            r#"
                (def pressed (state false))
                (def released (state false))
                (effect
                  (button "hold" :width 4 :height 2
                    :on-press (lambda (evt) (set! pressed true))
                    :on-release (lambda (evt) (set! released true))))
                "#,
        )
        .unwrap();

    editor.handle_mouse(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1),
        1,
        1,
        8,
        4,
    );
    editor.handle_mouse(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 7, 3),
        1,
        1,
        8,
        4,
    );

    assert_eq!(
        editor.runtime.eval_str("pressed").unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        editor.runtime.eval_str("released").unwrap(),
        Some(Value::Bool(true)),
        "releasing outside the button must not leave a momentary action engaged"
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
fn focused_number_picker_escape_cancels_edit_and_runs_global_escape_binding() {
    let init = r#"
        (def escape-count (state 0))
        (def handle-global-escape ()
          (do
            (set! escape-count (+ escape-count 1))
            true))
        (bind-key "ESC" "handle-global-escape")
    "#;
    let runtime = Runtime::with_init_source(init);
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init.to_string()),
            ..EditorConfig::default()
        },
    );
    editor.set_layout_viewport(30, 8);
    editor.refresh_runtime_side_effects();
    let tree = editor
        .runtime_mut()
        .eval_str(
            r#"
                (number-picker
                  :key "focused-picker"
                  :value 12
                  :min 0
                  :max 99
                  :decimals 0
                  :width 8
                  :height 1.4)
                "#,
        )
        .expect("build number picker")
        .expect("number picker should produce a widget tree");
    editor
        .active_buffer_mut()
        .set_widget_tree(Some(tree.clone()), None);
    editor.runtime_mut().set_widget_tree(tree);
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(30, 8);
    let _ = editor.widget_layout().expect("number picker layout");
    assert!(editor.focus_widget_by_stable_key("focused-picker", Some("number-picker")));
    let picker_id = editor.focused_widget_id().expect("number picker focus");

    editor.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
    assert!(
        crate::widget_render::number_picker::number_picker_edit_state(picker_id).editing,
        "numeric input should put the focused picker into edit mode"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(
        !crate::widget_render::number_picker::number_picker_edit_state(picker_id).editing,
        "Escape should cancel the picker edit"
    );
    assert_eq!(
        editor.focused_widget_id(),
        None,
        "Escape should blur the picker"
    );
    assert_eq!(
        editor.runtime_mut().eval_str("escape-count").unwrap(),
        Some(Value::Number(1.0)),
        "the same Escape keypress should continue to the global binding"
    );
}

#[test]
fn click_outside_focused_number_picker_blurs_it() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(30, 8);
    let tree = editor
        .runtime_mut()
        .eval_str(
            r#"
                (number-picker
                  :key "drag-picker"
                  :value 12
                  :min 0
                  :max 99
                  :decimals 0
                  :width 8
                  :height 1.4)
                "#,
        )
        .expect("build number picker")
        .expect("number picker should produce a widget tree");
    editor
        .active_buffer_mut()
        .set_widget_tree(Some(tree.clone()), None);
    editor.runtime_mut().set_widget_tree(tree);
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport(30, 8);
    let layout = editor.widget_layout().expect("number picker layout");
    let picker =
        super::find_layout_node_by_stable_key(layout.as_ref(), "drag-picker").expect("picker node");
    let (col, row) = (
        picker.rect.col + picker.rect.width * 0.5,
        picker.rect.row + picker.rect.height * 0.5,
    );

    // Drag on the picker (mouse down + drag + up) — this focuses it.
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col as u16,
            row as u16,
        ),
        0,
        0,
        30,
        8,
        col,
        row,
    );
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            col as u16,
            (row + 1.0) as u16,
        ),
        0,
        0,
        30,
        8,
        col,
        row + 1.0,
    );
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            col as u16,
            (row + 1.0) as u16,
        ),
        0,
        0,
        30,
        8,
        col,
        row + 1.0,
    );
    assert!(
        editor.focused_widget_id().is_some(),
        "dragging the picker should focus it"
    );

    // Click well outside the picker — focus must clear.
    editor.handle_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 25, 6),
        0,
        0,
        30,
        8,
        25.0,
        6.0,
    );
    assert_eq!(
        editor.focused_widget_id(),
        None,
        "clicking outside the focused picker should blur it"
    );
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
            ("origin", Value::Number(0.0)),
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
                :origin (bind "APP" "origin")
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
        matches!(knob.props.get("origin"), Some(Value::ReactiveRef { .. })),
        "reactive origin should be accepted by knob-number: {:?}",
        knob.props.get("origin")
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
        overlay_viewport_bottom: 16.0,
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

#[test]
fn hidden_ui_only_status_bar_reappears_in_inspect_mode() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.active_leaf_mut().show_status = false;
    let tile_id = editor.active_tile;

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    assert!(!frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 8);

    editor.toggle_inspect_mode();
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    let status: String = frame.tiles[0]
        .frame
        .status_cells
        .iter()
        .map(|cell| cell.ch)
        .collect();
    assert!(frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 7);
    assert!(status.contains("Inspect mode"), "{status}");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 40, 9);
    assert!(!frame.tiles[0].show_status);
    assert_eq!(editor.tile_content_area(tile_id, 0).unwrap().3, 8);
}

#[test]
fn inspect_hover_reveals_status_for_inactive_ui_tile() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.set_layout_viewport(80, 12);
    editor.update_tile_rects(80, 12);

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*inspect-target*"
              (box :debug-name "inspect-target" :width 10 :height 4))
            (split-window-right "*inspect-target*")
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(80, 12);

    let target_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*inspect-target*")
        .unwrap();
    let target_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| {
            editor
                .tile_root
                .find_leaf(*tile_id)
                .is_some_and(|leaf| leaf.buffer_idx == target_idx)
        })
        .unwrap();
    let other_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| *tile_id != target_tile)
        .unwrap();
    editor.switch_active_tile(other_tile);
    editor
        .tile_root
        .find_leaf_mut(target_tile)
        .unwrap()
        .show_status = false;
    editor.active_leaf_mut().show_status = false;
    let other_idx = editor.tile_root.find_leaf(other_tile).unwrap().buffer_idx;
    editor.buffers[other_idx].view_mode = super::ViewMode::UiOnly;
    editor.buffers[target_idx].view_mode = super::ViewMode::UiOnly;

    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 80, 12);
    assert!(
        !frame
            .tiles
            .iter()
            .find(|tile| tile.tile_id == target_tile)
            .unwrap()
            .show_status
    );

    editor.toggle_inspect_mode();
    let (content_col, content_row, _, _) = editor.tile_content_area(target_tile, 0).unwrap();
    let target_node = editor
        .tile_root
        .find_leaf(target_tile)
        .unwrap()
        .cached_layout
        .as_ref()
        .expect("target tile should have cached layout")
        .clone();
    let hover_col = content_col as f32 + target_node.rect.col + target_node.rect.width * 0.5;
    let hover_row = content_row as f32 + target_node.rect.row + target_node.rect.height * 0.5;
    assert!(
        editor
            .inspect_widget_node_at_tile(
                target_tile,
                content_col,
                content_row,
                hover_col,
                hover_row,
            )
            .is_some(),
        "test hover point should hit the inactive tile widget"
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            hover_col.floor() as u16,
            hover_row.floor() as u16,
        ),
        hover_col,
        hover_row,
        0,
    );

    assert_eq!(
        editor.active_tile, other_tile,
        "inspect hover should not select the hovered tile"
    );
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 80, 12);
    let other = frame
        .tiles
        .iter()
        .find(|tile| tile.tile_id == other_tile)
        .unwrap();
    assert!(
        !other.show_status,
        "non-hovered hidden-status UI tile should not show inspect hover info"
    );
    assert!(
        other.inspect_overlay.is_none(),
        "non-hovered tile should not have an inspect overlay"
    );
    let target = frame
        .tiles
        .iter()
        .find(|tile| tile.tile_id == target_tile)
        .unwrap();
    assert!(target.show_status);
    let overlay = target
        .inspect_overlay
        .expect("hovered tile should carry an inspect overlay");
    assert_eq!(overlay.rect, target_node.rect);
    assert!(overlay.rect.width > 0.0);
    assert!(overlay.rect.height > 0.0);
    let status = target
        .frame
        .status_cells
        .iter()
        .map(|cell| cell.ch)
        .collect::<String>();
    assert!(status.contains("Inspect:"), "{status}");
    assert!(status.contains("inspect-target"), "{status}");
}

#[test]
fn inspect_source_opens_as_root_right_tile() {
    use std::collections::HashMap;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*left*", "(label \"left\")");
    editor
        .split_active_tile(SplitDir::Vertical, editor.active_buffer_idx())
        .expect("split should create a second tile");
    editor.set_layout_viewport(40, 10);
    editor
        .runtime_mut()
        .eval_str(r#"(effect (box :debug-name "stale-ui" :width 12 :height 4))"#)
        .unwrap();
    editor.refresh_runtime_side_effects();
    assert!(
        editor.runtime.current_layout.is_some(),
        "regression setup should start with a live UI layout"
    );
    editor.update_tile_rects(120, 20);
    let leaf_count_before = editor.tile_root.leaf_count();
    let root_width_before = editor.tile_root_rect().unwrap().width;

    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-inspect-root-source-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.lisp");
    std::fs::write(
        &path,
        "(def render-target ()\n  (box :debug-name \"target\"))\n",
    )
    .unwrap();

    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(path.display().to_string()),
    );
    props.insert(
        "debug-name".to_string(),
        Value::String("target".to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "box".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert!(editor.open_source_for_inspected_node(&node).unwrap());
    editor.update_tile_rects(120, 20);

    assert_eq!(editor.tile_root.leaf_count(), leaf_count_before + 1);
    let source_tile = editor.active_tile;
    let source_leaf = editor.tile_root.find_leaf(source_tile).unwrap();
    assert_eq!(
        editor.buffers[source_leaf.buffer_idx].path.as_ref(),
        Some(&path)
    );
    assert_eq!(
        editor.buffers[source_leaf.buffer_idx].view_mode,
        super::ViewMode::TextOnly
    );
    assert!(
        editor.runtime.current_layout.is_none(),
        "source tile activation must not inherit the previously active UI layout"
    );
    let source_rect = editor.tile_rect(source_tile).unwrap();
    assert!(
        source_rect.col > root_width_before * 0.6,
        "source tile should be appended to the right of the previous layout"
    );

    let second_path = dir.join("other-source.lisp");
    std::fs::write(
        &second_path,
        "(def render-other ()\n  (button :debug-name \"other-target\"))\n",
    )
    .unwrap();
    let mut second_props = HashMap::new();
    second_props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(second_path.display().to_string()),
    );
    second_props.insert(
        "debug-name".to_string(),
        Value::String("other-target".to_string()),
    );
    let second_node = crate::layout::LayoutNode {
        widget_id: 2,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "button".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props: second_props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert!(editor.open_source_for_inspected_node(&second_node).unwrap());
    editor.update_tile_rects(120, 20);
    assert_eq!(
        editor.tile_root.leaf_count(),
        leaf_count_before + 1,
        "opening another inspected source file should reuse the inspect source tile"
    );
    assert_eq!(editor.active_tile, source_tile);
    let source_leaf = editor.tile_root.find_leaf(source_tile).unwrap();
    assert_eq!(
        editor.buffers[source_leaf.buffer_idx].path.as_ref(),
        Some(&second_path)
    );
    assert_eq!(editor.active_buffer().cursor, (1, 2));
}

#[test]
fn inspect_source_prefers_module_path_over_transient_source_buffer() {
    use std::collections::HashMap;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.open_scratch_buffer("*scripts*", "Scripts > fixtures");
    let scripts_buffer_id = editor.active_buffer().id;

    let path = std::env::temp_dir().join(format!(
        "eseqlisp-inspect-script-picker-source-{}.lisp",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "(def script-ui ()\n  (number-picker :debug-name \"script-delay\"))\n",
    )
    .unwrap();

    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_BUFFER_ID_PROP.to_string(),
        Value::Number(scripts_buffer_id as f64),
    );
    props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(path.display().to_string()),
    );
    props.insert(
        "debug-name".to_string(),
        Value::String("script-delay".to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "number-picker".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert!(editor.open_source_for_inspected_node(&node).unwrap());
    assert_eq!(
        editor.active_buffer().path.as_ref(),
        Some(&path),
        "inspect should open the loaded script file, not the script picker buffer"
    );
    assert_eq!(editor.active_buffer().cursor, (1, 2));

    let _ = std::fs::remove_file(path);
}

#[test]
fn inspect_source_span_opens_exact_widget_form_without_legacy_identity() {
    use std::collections::HashMap;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-inspect-source-span-{}.lisp",
        std::process::id()
    ));
    let source = "(def render-target ()\n  (knob-number :label \"base\"))\n";
    std::fs::write(&path, source).unwrap();
    editor
        .runtime_mut()
        .eval_source_at_path(path.clone(), source)
        .unwrap();
    let start = source.find("(knob-number").unwrap();
    let end = source[start..]
        .find(')')
        .map(|offset| start + offset + 1)
        .unwrap();
    let revision = crate::hot_reload::hash_source(source);

    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(path.display().to_string()),
    );
    props.insert(
        crate::vm::SOURCE_START_BYTE_PROP.to_string(),
        Value::Number(start as f64),
    );
    props.insert(
        crate::vm::SOURCE_END_BYTE_PROP.to_string(),
        Value::Number(end as f64),
    );
    props.insert(
        crate::vm::SOURCE_REVISION_PROP.to_string(),
        Value::String(revision.to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "knob-number".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert!(super::inspect_node_has_source_identity(&node));
    assert!(editor.open_source_for_inspected_node(&node).unwrap());
    assert_eq!(editor.active_buffer().path.as_ref(), Some(&path));
    assert_eq!(
        editor.active_buffer().cursor,
        super::offset_to_position(source, start)
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn inspect_source_span_opens_evaluated_snapshot_when_file_changed() {
    use std::collections::HashMap;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-inspect-source-snapshot-{}.lisp",
        std::process::id()
    ));
    let evaluated_source = "(def render-target ()\n  (knob-number :label \"base\"))\n";
    std::fs::write(&path, evaluated_source).unwrap();
    editor
        .runtime_mut()
        .eval_source_at_path(path.clone(), evaluated_source)
        .unwrap();
    std::fs::write(
        &path,
        "(def render-target ()\n  ;; inserted after render\n  (knob-number :label \"base\"))\n",
    )
    .unwrap();
    let start = evaluated_source.find("(knob-number").unwrap();
    let end = evaluated_source[start..]
        .find(')')
        .map(|offset| start + offset + 1)
        .unwrap();
    let revision = crate::hot_reload::hash_source(evaluated_source);

    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(path.display().to_string()),
    );
    props.insert(
        crate::vm::SOURCE_START_BYTE_PROP.to_string(),
        Value::Number(start as f64),
    );
    props.insert(
        crate::vm::SOURCE_END_BYTE_PROP.to_string(),
        Value::Number(end as f64),
    );
    props.insert(
        crate::vm::SOURCE_REVISION_PROP.to_string(),
        Value::String(revision.to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "knob-number".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert!(editor.open_source_for_inspected_node(&node).unwrap());
    assert!(editor.active_buffer().read_only);
    assert_eq!(editor.active_buffer().path, None);
    assert_eq!(editor.active_buffer().text(), evaluated_source);
    assert_eq!(
        editor.active_buffer().cursor,
        super::offset_to_position(evaluated_source, start)
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn inspect_source_span_opens_sampler_base_knob_fixture() {
    use std::collections::HashMap;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/ui/effects/sampler-panel.lisp")
        .canonicalize()
        .unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    let def_start = source.find("(def sampler-param-knob").unwrap();
    let start = source[def_start..]
        .find("(knob-number")
        .map(|offset| def_start + offset)
        .unwrap();
    let end = source[start..]
        .find(')')
        .map(|offset| start + offset + 1)
        .unwrap();

    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_MODULE_PATH_PROP.to_string(),
        Value::String(path.display().to_string()),
    );
    props.insert(
        crate::vm::SOURCE_START_BYTE_PROP.to_string(),
        Value::Number(start as f64),
    );
    props.insert(
        crate::vm::SOURCE_END_BYTE_PROP.to_string(),
        Value::Number(end as f64),
    );
    props.insert(
        crate::vm::SOURCE_REVISION_PROP.to_string(),
        Value::String(crate::hot_reload::hash_source(&source).to_string()),
    );
    props.insert(
        "debug-name".to_string(),
        Value::String("sampler-param-base-base".to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "knob-number".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: true,
        animation: Default::default(),
    };

    assert!(editor.open_source_for_inspected_node(&node).unwrap());
    let expected_cursor = super::offset_to_position(&source, start);
    assert_eq!(editor.active_buffer().path.as_ref(), Some(&path));
    assert_eq!(editor.active_buffer().cursor, expected_cursor);
    assert!(
        editor.active_buffer().lines[expected_cursor.0].contains("(knob-number"),
        "sampler inspect target must be the constructor form, not the file top"
    );
}

#[test]
fn inspect_source_lookup_finds_debug_named_widget_form() {
    use std::collections::HashMap;

    let source = r#"
(def unrelated ()
  (box :debug-name "other"))

(def render-target ()
  (v-stack
    (box :debug-name "target-box"
      (label "inside"))))
"#;
    let mut props = HashMap::new();
    props.insert(
        "debug-name".to_string(),
        crate::vm::Value::String("target-box".to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "box".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    assert_eq!(super::find_widget_form_in_text(source, &node), Some((6, 4)));
}

#[test]
fn inspect_hit_test_prefers_source_identified_ancestor() {
    use std::collections::HashMap;

    let mut parent_props = HashMap::new();
    parent_props.insert(
        "key".to_string(),
        Value::String("seqv-select-1".to_string()),
    );
    let child = crate::layout::LayoutNode {
        widget_id: 2,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "label".to_string(),
        rect: crate::layout::Rect {
            row: 1.0,
            col: 1.0,
            width: 4.0,
            height: 1.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };
    let parent = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: Some("seqv-select-1".to_string()),
        widget_type: "button".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 8.0,
            height: 3.0,
        },
        props: parent_props,
        children: vec![child],
        focusable: false,
        animation: Default::default(),
    };

    let (hit, _) = super::inspect_hit_test_layout(&parent, 1.2, 2.0).expect("hit");
    assert_eq!(hit.widget_id, 1);
    assert_eq!(hit.stable_key.as_deref(), Some("seqv-select-1"));
}

#[test]
fn inspect_source_lookup_finds_widget_form_with_dynamic_str_key() {
    use std::collections::HashMap;

    let source = r#"
(def seqv-step-cell (track step visible)
  (let ((odd 0))
    (box
      :width 3.05 :height 1.45
      :key (str "seqv-step-cell-" track "-" step)
      :active true
      (box :background "seqv-step-shell"))))
"#;
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: Some("seqv-step-cell-1-0".to_string()),
        widget_type: "box".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props: HashMap::new(),
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    let expected = source
        .find("(box\n      :width")
        .map(|offset| super::offset_to_position(source, offset));
    assert_eq!(super::find_widget_form_in_text(source, &node), expected);
}

#[test]
fn inspect_source_lookup_finds_unique_widget_form_inside_source_symbol() {
    use std::collections::HashMap;

    let source = r#"
(def instrument-panel (inst)
  (box
    (v-stack
      (waveform
        :height 4.85
        :buffer (get inst :buffer)))))
"#;
    let mut props = HashMap::new();
    props.insert(
        crate::vm::SOURCE_SYMBOL_PROP.to_string(),
        crate::vm::Value::String("instrument-panel".to_string()),
    );
    let node = crate::layout::LayoutNode {
        widget_id: 1,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "waveform".to_string(),
        rect: crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 1.0,
            height: 1.0,
        },
        props,
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };

    let expected = source
        .find("(waveform")
        .map(|offset| super::offset_to_position(source, offset));
    assert_eq!(
        super::find_unique_widget_form_in_definition(source, "instrument-panel", &node),
        expected
    );
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

#[test]
fn remembered_layout_split_restores_its_user_resized_ratio() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*main*", "");
    editor.open_scratch_buffer("*fx*", "");
    editor.open_scratch_buffer("*piano-roll*", "");

    editor
        .runtime_mut()
        .eval_str(
            r#"(set-layout
                (list :rows :remember "lower:piano-roll"
                  0.6 "*main*"
                  0.4 "*piano-roll*"))"#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let (split_id, split_dir) = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => {
            assert_eq!(split.remember_key.as_deref(), Some("lower:piano-roll"));
            (split.id, split.dir)
        }
        crate::tile::TileNode::Leaf(_) => panic!("remembered layout should build a split"),
    };
    editor.update_tile_split_ratio(
        split_id,
        split_dir,
        crate::layout::Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 100.0,
        },
        0.0,
        42.0,
    );

    editor
        .runtime_mut()
        .eval_str(
            r#"(set-layout
                (list :rows :remember "lower:fx"
                  0.7 "*main*"
                  0.3 "*fx*"))"#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let fx_ratio = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => split.ratio,
        crate::tile::TileNode::Leaf(_) => panic!("FX layout should build a split"),
    };
    assert!((fx_ratio - 0.7).abs() < f32::EPSILON);

    editor
        .runtime_mut()
        .eval_str(
            r#"(set-layout
                (list :rows :remember "lower:piano-roll"
                  0.6 "*main*"
                  0.4 "*piano-roll*"))"#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let restored_ratio = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => split.ratio,
        crate::tile::TileNode::Leaf(_) => panic!("piano-roll layout should build a split"),
    };
    assert!(
        (restored_ratio - 0.42).abs() < f32::EPSILON,
        "piano-roll split should restore its remembered drag ratio: {restored_ratio}"
    );
}

#[test]
fn resize_window_remembers_the_constraint_clamped_ratio() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*main*", "");
    editor.open_scratch_buffer("*fx*", "");
    editor.open_scratch_buffer("*piano-roll*", "");

    let piano_roll_layout = r#"
        (set-layout
          (list :rows :remember "lower:piano-roll"
            0.6 "*main*"
            0.4 (list :buf "*piano-roll*" :min-height 40)))
    "#;
    editor.runtime_mut().eval_str(piano_roll_layout).unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(100, 100);
    let root_height = editor
        .tile_root_rect()
        .expect("piano-roll layout root rect")
        .height;
    assert!(editor.switch_active_tile_to_buffer_named("*piano-roll*"));

    editor
        .runtime_mut()
        .eval_str("(resize-window 0.3)")
        .unwrap();
    editor.refresh_runtime_side_effects();
    let resized_ratio = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => split.ratio,
        crate::tile::TileNode::Leaf(_) => panic!("piano-roll layout should build a split"),
    };
    let expected_ratio = 1.0 - 40.0 / root_height;
    assert!(
        (resized_ratio - expected_ratio).abs() < f32::EPSILON,
        "resize-window should clamp the lower pane to its minimum height: {resized_ratio}"
    );

    editor
        .runtime_mut()
        .eval_str(
            r#"(set-layout
                (list :rows :remember "lower:fx"
                  0.7 "*main*"
                  0.3 "*fx*"))"#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.runtime_mut().eval_str(piano_roll_layout).unwrap();
    editor.refresh_runtime_side_effects();

    let restored_ratio = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => split.ratio,
        crate::tile::TileNode::Leaf(_) => panic!("piano-roll layout should build a split"),
    };
    assert!(
        (restored_ratio - resized_ratio).abs() < f32::EPSILON,
        "restored layout should use the clamped ratio that was actually displayed: resized={resized_ratio}, restored={restored_ratio}"
    );
}

#[test]
fn fixed_pane_collapses_only_after_dragging_through_its_threshold() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*main*", "");
    editor.open_scratch_buffer("*panel*", "");
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def panel-collapsed (state false))
            (def sidebar-collapsed (state false))
            (set-layout
              (list :rows
                0.8 "*main*"
                0.2 (list :buf "*panel*"
                      :min-height 20
                      :max-height 20
                      :collapse-threshold 0.25
                      :on-collapse (lambda () (set! panel-collapsed true)))))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let (split_id, split_dir) = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => (split.id, split.dir),
        crate::tile::TileNode::Leaf(_) => panic!("collapsible layout should build a split"),
    };
    let area = crate::layout::Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 100.0,
    };

    editor.active_tile_resize_drag = Some(super::TileResizeDrag {
        split_id,
        dir: split_dir,
        area,
    });
    editor.handle_tile_resize_drag(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 0, 84),
        0.0,
        84.0,
    );
    assert_eq!(
        editor.runtime_mut().eval_str("panel-collapsed").unwrap(),
        Some(Value::Bool(false)),
        "dragging less than 25% through the fixed height must keep the pane visible"
    );

    editor.active_tile_resize_drag = Some(super::TileResizeDrag {
        split_id,
        dir: split_dir,
        area,
    });
    editor.handle_tile_resize_drag(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 0, 86),
        0.0,
        86.0,
    );
    assert_eq!(
        editor.runtime_mut().eval_str("panel-collapsed").unwrap(),
        Some(Value::Bool(true)),
        "crossing 25% through the fixed height should collapse during the drag"
    );
    assert!(
        editor.active_tile_resize_drag.is_none(),
        "collapsing should cancel the stale divider drag before rebuilding the layout"
    );
    assert!(
        editor.suppress_mouse_until_left_up,
        "the remainder of a collapsed pane's pointer gesture should be suppressed"
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 0, 86),
        0.0,
        86.0,
        0,
    );
    assert!(
        !editor.suppress_mouse_until_left_up,
        "mouse-up should finish suppression after the pane has collapsed"
    );

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (set-layout
              (list :cols
                0.2 (list :buf "*panel*"
                      :min-width 20
                      :max-width 20
                      :collapse-threshold 0.25
                      :on-collapse (lambda () (set! sidebar-collapsed true)))
                0.8 "*main*"))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    let (split_id, split_dir) = match &editor.tile_root {
        crate::tile::TileNode::Split(split) => (split.id, split.dir),
        crate::tile::TileNode::Leaf(_) => panic!("collapsible sidebar should build a split"),
    };
    editor.active_tile_resize_drag = Some(super::TileResizeDrag {
        split_id,
        dir: split_dir,
        area,
    });
    editor.handle_tile_resize_drag(
        mouse_event(MouseEventKind::Drag(MouseButton::Left), 14, 0),
        14.0,
        0.0,
    );
    assert_eq!(
        editor.runtime_mut().eval_str("sidebar-collapsed").unwrap(),
        Some(Value::Bool(true)),
        "a fixed left sidebar should collapse as soon as its drag crosses the threshold"
    );
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
            && tab_rect.rect.row + tab_rect.rect.height
                <= editor.tile_body_rect(tile_id).unwrap().row + f32::EPSILON,
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

#[cfg(target_os = "macos")]
#[test]
fn dropdown_overlay_captures_move_and_mouse_up_without_selecting_the_tile_below() {
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
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def underlay-clicked (state false))
            (effect-buffer "*transport*"
              (dropdown
                :width 6
                :height 1
                :options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar")
                :value "off"
                :on-change (lambda (value) (host-command "overlay-selected" value))))
            (effect-buffer "*sequencer*"
              (button "underlay"
                :width 60
                :height 20
                :on-click (lambda (event) (set! underlay-clicked true))))
            (set-layout
              (list :rows :gap 0
                0.1 (list :buf "*transport*" :hide-status true)
                0.9 (list :buf "*sequencer*" :hide-status true)))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    let transport_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*transport*")
        .unwrap();
    let sequencer_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*sequencer*")
        .unwrap();
    let transport_tile = editor
        .tile_root
        .find_leaf_by_buffer_idx(transport_idx)
        .unwrap()
        .id;
    let sequencer_tile = editor
        .tile_root
        .find_leaf_by_buffer_idx(sequencer_idx)
        .unwrap()
        .id;
    let (transport_col, transport_row, _, transport_height) = editor
        .tile_content_area(transport_tile, 0)
        .expect("transport content area");
    let transport_layout = editor
        .tile_root
        .find_leaf(transport_tile)
        .and_then(|leaf| leaf.cached_layout.clone())
        .expect("transport layout");
    let dropdown = find_widget(&transport_layout, "dropdown")
        .expect("transport dropdown")
        .clone();

    let trigger_col = transport_col as f32 + dropdown.rect.col + dropdown.rect.width * 0.5;
    let trigger_row = transport_row as f32 + dropdown.rect.row + dropdown.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, trigger_col.floor() as u16, trigger_row.floor() as u16),
            trigger_col,
            trigger_row,
            0,
        );
    }

    assert_eq!(editor.active_tile, transport_tile);
    let _ = crate::widget_render::collect_metal_primitives(
        &transport_layout,
        crate::widget_render::WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 600.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0 - transport_row as f32,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        0.0,
        transport_height,
    );
    let menu = crate::widget_render::get_overlay_rect().expect("open dropdown overlay");
    // Select 1/8 (third row). This point is below the two-row transport tile.
    let item_col = transport_col as f32 + menu.col + menu.width * 0.5;
    let item_row = transport_row as f32 + menu.row + 0.3 + 2.0 * 1.4 + 0.7;
    assert!(
        item_row >= editor.tile_rect(sequencer_tile).unwrap().row,
        "test click must visibly overlap the sequencer tile"
    );

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            item_col.floor() as u16,
            item_row.floor() as u16,
        ),
        item_col,
        item_row,
        0,
    );
    let commands = editor.drain_host_commands();
    assert!(
        matches!(
            commands.as_slice(),
            [HostCommand::Custom { name, payload: Value::String(value) }]
                if name == "overlay-selected" && value == "1/8"
        ),
        "menu={menu:?}, item=({item_col}, {item_row}), commands={commands:?}"
    );
    assert_eq!(editor.active_tile, transport_tile);
    // AppKit commonly emits a move between the item mouse-down and the
    // matching mouse-up. That move must not release overlay pointer capture.
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            item_col.floor() as u16,
            item_row.floor() as u16,
        ),
        item_col,
        item_row,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            item_col.floor() as u16,
            item_row.floor() as u16,
        ),
        item_col,
        item_row,
        0,
    );

    assert_eq!(editor.active_tile, transport_tile);
    assert_eq!(
        editor
            .runtime
            .eval_str("underlay-clicked")
            .unwrap()
            .unwrap(),
        Value::Bool(false),
        "the same click must not reach the buffer beneath the menu"
    );
    crate::widget_render::clear_overlay();
}

#[test]
fn clicking_folder_tab_close_invokes_callback_without_selecting_tab() {
    let mut editor = editor_with_tabbed_buffers();
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def closed-buffer (state ""))
            (def closed-index (state -1))
            (set-layout
              (list :buf "*sequencer*"
                :tabs (list
                  (list "Sequencer" "*sequencer*")
                  (list "Matrix" "*matrix*"
                    :on-close (lambda (buffer index)
                      (do
                        (set! closed-buffer buffer)
                        (set! closed-index index)))))
                :hide-status true
                :border-radius 12
                :border-width 4))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 12);

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
    let close_rect = crate::tile::tile_tab_layouts_with_hover(
        editor.tile_rect(tile_id).unwrap(),
        &leaf.tabs,
        leaf.selected_tab,
        Some(1),
    )
    .into_iter()
    .find(|tab| tab.index == 1)
    .and_then(|tab| tab.close_rect)
    .expect("matrix tab close rect");
    let precise_col = close_rect.col + close_rect.width * 0.5;
    let precise_row = close_rect.row + close_rect.height * 0.5;

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            precise_col.floor() as u16,
            precise_row.floor() as u16,
        ),
        precise_col,
        precise_row,
        0,
    );
    let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 12);
    let hovered_tab = frame.tiles[0]
        .tabs
        .iter()
        .find(|tab| tab.index == 1)
        .expect("matrix tab in frame");
    assert!(hovered_tab.close_visible);

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

    assert_eq!(editor.active_buffer().name, "*sequencer*");
    assert_eq!(editor.active_leaf().selected_tab, Some(0));
    assert_eq!(
        editor.runtime.eval_str("closed-buffer").unwrap().unwrap(),
        Value::String("*matrix*".to_string())
    );
    assert_eq!(
        editor.runtime.eval_str("closed-index").unwrap().unwrap(),
        Value::Number(1.0)
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
fn tab_selection_survives_moving_and_temporarily_removing_its_tile() {
    let mut editor = editor_with_tabbed_buffers();
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*top*" (label "top"))
            (effect-buffer "*side*" (label "side"))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    editor
        .runtime_mut()
        .eval_str(r#"(set-window-buffer-for "*sequencer*" "*matrix*")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();

    editor
        .runtime_mut()
        .eval_str(
            r#"
            (set-layout
              (list :rows
                0.2 "*top*"
                0.8 (list :cols
                  0.3 "*side*"
                  0.7 (list :buf "*sequencer*"
                    :tabs (list
                      (list "Sequencer" "*sequencer*")
                      (list "Matrix" "*matrix*"))))))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().name, "*matrix*");
    assert_eq!(editor.active_leaf().selected_tab, Some(1));

    editor
        .runtime_mut()
        .eval_str(r#"(set-layout "*side*")"#)
        .unwrap();
    editor.refresh_runtime_side_effects();
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
    assert_eq!(
        editor.tile_body_rect(editor.active_tile).unwrap(),
        tile.body_rect
    );
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

    // Relaying out the active tree replaces the runtime's widget-binding map.
    // The editor must immediately restore the union of all visible layouts,
    // even though their cached layout identities have not changed.
    editor.set_layout_viewport(61, 12);

    editor.clear_needs_redraw();
    let _ = editor.take_dirty_widget_ids();

    editor
        .runtime_mut()
        .set_reactive("APP", "peak", Value::Number(0.7));
    editor.set_layout_viewport(62, 12);

    assert!(
        editor.needs_redraw(),
        "binding-only writes in inactive visible tiles must survive an active-tree relayout and schedule a render"
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
fn unpresented_tiled_frame_requeues_inactive_tile_widget_dirtiness() {
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
    let meters_tile_id = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| {
            editor
                .tile_root
                .find_leaf(*tile_id)
                .is_some_and(|leaf| leaf.buffer_idx == meters_idx)
        })
        .unwrap();
    let widget_id = editor
        .tile_root
        .find_leaf(meters_tile_id)
        .and_then(|leaf| leaf.cached_layout.as_ref())
        .expect("meter layout should be cached")
        .widget_id;

    let _ = editor.take_dirty_widget_ids();
    editor
        .runtime_mut()
        .set_reactive("APP", "peak", Value::Number(0.7));

    let unpresented = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 12);
    let first_dirty = unpresented
        .tiles
        .iter()
        .find(|tile| tile.tile_id == meters_tile_id)
        .expect("meters tile should be visible")
        .frame
        .dirty_widget_ids
        .clone();
    assert_eq!(first_dirty, vec![widget_id]);

    crate::ui::frame::requeue_unpresented_tiled_frame(&mut editor, &unpresented);
    assert!(editor.needs_redraw());

    let retry = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 12);
    let retried_dirty = &retry
        .tiles
        .iter()
        .find(|tile| tile.tile_id == meters_tile_id)
        .expect("meters tile should remain visible")
        .frame
        .dirty_widget_ids;
    assert_eq!(retried_dirty, &first_dirty);
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
fn widget_scroll_limit_cache_recomputes_after_layout_revision_changes() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
    editor.set_layout_viewport_exact(10.0, 8.0);
    editor
        .runtime
        .eval_str("(effect (box :width 40 :height 20))")
        .unwrap();

    let first = editor.clamp_widget_scroll_offsets();
    assert!(first.0 > 0.0 && first.1 > 0.0, "first limits={first:?}");
    assert!(editor.active_leaf().widget_scroll_limits_cache.is_some());

    editor
        .runtime
        .eval_str("(effect (box :width 12 :height 9))")
        .unwrap();
    let second = editor.clamp_widget_scroll_offsets();
    assert!(second.0 < first.0, "first={first:?} second={second:?}");
    assert!(second.1 < first.1, "first={first:?} second={second:?}");
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
                    :lane-height 1
                    :lanes (list
                      (dict :id 0 :label "L0") (dict :id 1 :label "L1")
                      (dict :id 2 :label "L2") (dict :id 3 :label "L3")
                      (dict :id 4 :label "L4") (dict :id 5 :label "L5")
                      (dict :id 6 :label "L6") (dict :id 7 :label "L7")
                      (dict :id 8 :label "L8") (dict :id 9 :label "L9")
                      (dict :id 10 :label "L10") (dict :id 11 :label "L11")
                      (dict :id 12 :label "L12") (dict :id 13 :label "L13")
                      (dict :id 14 :label "L14") (dict :id 15 :label "L15"))
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
    assert_eq!(editor.widget_scroll_left(), 4.0);
}

#[test]
fn text_horizontal_scroll_stays_fixed_until_cursor_reaches_left_margin() {
    for view_mode in [super::ViewMode::TextOnly, super::ViewMode::Both] {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "01234567890123456789");
        editor.active_buffer_mut().view_mode = view_mode;
        editor.set_layout_viewport(10, 8);
        editor.active_buffer_mut().cursor = (0, 9);
        editor.active_leaf_mut().widget_scroll_left = 4.0;

        let frame = crate::frame::build_render_frame(&mut editor, 10, 8);
        assert_eq!(frame.widget_scroll_left, 4.0);

        for expected_col in [8, 7] {
            editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
            let frame = crate::frame::build_render_frame(&mut editor, 10, 8);
            assert_eq!(editor.active_buffer().cursor, (0, expected_col));
            assert_eq!(frame.widget_scroll_left, 4.0);
        }

        editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let frame = crate::frame::build_render_frame(&mut editor, 10, 8);
        assert_eq!(editor.active_buffer().cursor, (0, 6));
        assert_eq!(frame.widget_scroll_left, 3.0);
    }
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
    assert_eq!(editor.widget_scroll_left(), 4.0);
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

/// A clicked clip is published on BOTH per-lane channels (a click selects and
/// binds the sound), so the widget must dedup them: two copies of the id here
/// meant Backspace emitted two :delete-items for one clip — the first delete
/// succeeded and the second errored on the already-gone id.
#[test]
fn focused_timeline_delete_dedups_selected_and_bound_channel_ids() {
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
                    :items (list (dict :id 10 :lane 0 :start 4 :end 8))
                    :selected-id 10
                    :bound-id 10
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
    let Value::Map(map) = &action else {
        panic!("delete action is a map");
    };
    let ids = map.get("ids").expect("action carries :ids");
    let Value::List(ids) = &*ids.borrow() else {
        panic!(":ids is a list");
    };
    assert_eq!(
        ids.len(),
        1,
        "the selected+bound clip is named exactly once"
    );
    assert!(
        matches!(&*ids[0].borrow(), Value::Number(n) if *n == 10.0),
        "the one id is the clip's"
    );
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
#[test]
fn inline_slider_evaluates_to_plain_value_and_registers_in_editor_buffers() {
    let mut headless = Runtime::new();
    let value = headless
        .eval_str("(~slider 12 :min 0 :max 24)")
        .expect("headless inline form should evaluate")
        .expect("inline form should return its value");
    assert_eq!(value, Value::Number(12.0));
    assert!(headless.take_pending_inline_widgets().is_none());

    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def amount (~slider 12 :min 0 :max 24))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::Both);
    assert_eq!(editor.active_buffer().inline_code_widgets().len(), 1);
    let inline = &editor.active_buffer().inline_code_widgets()[0];
    let anchor = editor
        .active_buffer()
        .source_anchor(inline.anchor_id)
        .expect("inline widget source anchor");
    assert_eq!(
        &editor.active_buffer().text()[anchor.start_byte..anchor.end_byte],
        "(~slider 12 :min 0 :max 24)"
    );
}

#[test]
fn inline_widget_snapshot_sync_preserves_code_and_inline_layout() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    let source = "(def amount (~slider 12 :min 0 :max 24))";
    editor.active_buffer_mut().set_text(source);
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    // A later editor refresh adopts the runtime's committed UI snapshot. Inline
    // roots must remain hybrid code+UI buffers rather than becoming UI-only.
    editor.refresh_runtime_side_effects();

    assert_eq!(editor.active_buffer().view_mode, super::ViewMode::Both);
    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let rendered_source = frame.lines[0]
        .iter()
        .map(|cell| cell.ch)
        .collect::<String>();
    let mut expected = source.to_string();
    expected.insert_str(source.find("12").unwrap(), "        ");
    assert_eq!(rendered_source.trim_end(), expected);
    assert!(frame.widget_layout.as_ref().is_some_and(|layout| {
        layout
            .children
            .iter()
            .any(|node| node.widget_type == "hslider")
    }));
}

#[test]
fn inline_slider_in_dormant_function_body_registers_during_compilation() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def tune ()\n  (~slider 24 :min 0 :max 48))\n(def untouched 1)");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    assert_eq!(editor.active_buffer().inline_code_widgets().len(), 1);
    let inline = &editor.active_buffer().inline_code_widgets()[0];
    let anchor = editor
        .active_buffer()
        .source_anchor(inline.anchor_id)
        .expect("body inline widget source anchor");
    assert_eq!(
        &editor.active_buffer().text()[anchor.start_byte..anchor.end_byte],
        "(~slider 24 :min 0 :max 48)"
    );
}

#[test]
fn inline_slider_layout_stays_on_value_line_and_survives_edit_above() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def header 1)\n(def amount (~slider 12 :min 0 :max 24))\n(def footer 2)");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let frame = crate::frame::build_render_frame(&mut editor, 80, 10);
    let text_height_scale = frame.text_cell_height_scale;
    let layout = frame.widget_layout.expect("inline widget layout");
    let slider = layout
        .children
        .iter()
        .find(|node| node.widget_type == "hslider")
        .expect("inline hslider");
    assert_eq!(slider.rect.row, text_height_scale);

    editor.active_buffer_mut().cursor = (0, 0);
    editor.active_buffer_mut().insert_str(";; moved\n");
    let frame = crate::frame::build_render_frame(&mut editor, 80, 10);
    let layout = frame
        .widget_layout
        .expect("inline widget layout after edit");
    let slider = layout
        .children
        .iter()
        .find(|node| node.widget_type == "hslider")
        .expect("inline hslider after edit");
    assert_eq!(slider.rect.row, 2.0 * text_height_scale);
    assert!(!matches!(
        slider.props.get("muted"),
        Some(Value::Bool(true))
    ));
}

#[test]
fn nested_inline_knob_inserts_columns_before_its_value_without_reserving_rows() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def nested\n  (list\n    (~knob 0.5 :min 0 :max 1)))\n(def after 2)");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let map = editor.active_buffer().inline_display_row_map();
    assert_eq!(map.display_row_for_buffer_line(2), Some(2));
    assert_eq!(map.display_row_for_buffer_line(3), Some(3));

    let frame = crate::frame::build_render_frame(&mut editor, 80, 10);
    let text_width_scale = frame.text_cell_width_scale;
    let text_height_scale = frame.text_cell_height_scale;
    let layout = frame.widget_layout.expect("inline knob layout");
    let knob = layout
        .children
        .iter()
        .find(|node| node.widget_type == "inline-knob")
        .expect("inline knob");
    assert_eq!(knob.rect.row, 2.0 * text_height_scale);
    assert_eq!(knob.rect.height, text_height_scale);
    assert_eq!(knob.rect.width, 3.0 * text_width_scale);
    let value_col = editor.active_buffer().lines[2].find("0.5").unwrap();
    assert_eq!(knob.rect.col, value_col as f32 * text_width_scale);

    editor.set_text_zoom(1.5).unwrap();
    let zoomed = crate::frame::build_render_frame(&mut editor, 80, 10);
    let zoomed_layout = zoomed.widget_layout.expect("zoomed inline knob layout");
    let knob = zoomed_layout
        .children
        .iter()
        .find(|node| node.widget_type == "inline-knob")
        .expect("zoomed inline knob");
    assert_eq!(knob.rect.row, 3.0);
    assert_eq!(knob.rect.col, value_col as f32 * 1.5);
    assert_eq!(knob.rect.width, 4.5);
    assert_eq!(knob.rect.height, 1.5);
}

#[test]
fn inline_knob_uses_relative_drag_without_staling_its_owned_anchors() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def before-a 1)\n(def before-b 2)\n(def amount (~knob 0.5 :min 0 :max 1))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let knob = frame
        .widget_layout
        .expect("inline knob layout")
        .children
        .iter()
        .find(|node| node.widget_type == "inline-knob")
        .expect("inline knob")
        .clone();
    let col = knob.rect.col + knob.rect.width * 0.5;
    let start_row = knob.rect.row + knob.rect.height * 0.5;
    let end_row = 0.1;

    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col as u16,
            start_row as u16,
        ),
        0,
        0,
        80,
        8,
        col,
        start_row,
    );
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            col as u16,
            end_row as u16,
        ),
        0,
        0,
        80,
        8,
        col,
        end_row,
    );

    assert!(
        !editor.active_buffer().text().contains("(~knob 0.5"),
        "relative drag should rewrite the inline knob literal"
    );
    let dragged = crate::frame::build_render_frame(&mut editor, 80, 8);
    let dragged_layout = dragged.widget_layout.expect("dragged inline knob layout");
    let dragged_knob = dragged_layout
        .children
        .iter()
        .find(|node| node.widget_type == "inline-knob")
        .expect("dragged inline knob");
    assert!(
        !matches!(dragged_knob.props.get("muted"), Some(Value::Bool(true))),
        "an inline widget must not become stale during its own writeback"
    );
}

#[test]
fn inline_toggle_reserves_five_zoom_scaled_text_cells() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    let source = "(def enabled (~toggle 1))";
    editor.active_buffer_mut().set_text(source);
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let text_width_scale = frame.text_cell_width_scale;
    let text_height_scale = frame.text_cell_height_scale;
    let layout = frame.widget_layout.expect("inline toggle layout");
    let toggle = layout
        .children
        .iter()
        .find(|node| node.widget_type == "toggle")
        .expect("inline toggle");
    let value_col = source.find('1').expect("toggle value");
    assert_eq!(toggle.rect.col, value_col as f32 * text_width_scale);
    assert_eq!(toggle.rect.width, 5.0 * text_width_scale);
    assert_eq!(toggle.rect.height, text_height_scale);
}

#[test]
fn inline_slider_shifts_cursor_and_mouse_columns_around_inserted_widget() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    let source = "(def amount (~slider 12 :min 0 :max 24))";
    editor.active_buffer_mut().set_text(source);
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);
    let value_col = source.find("12").unwrap();

    editor.active_buffer_mut().cursor = (0, value_col);
    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    assert_eq!(frame.cursor, Some((0, value_col + 8)));
    let clicked_display_col = (value_col + 8) as f32 * frame.text_cell_width_scale;

    editor.active_buffer_mut().cursor = (0, 0);
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            clicked_display_col.floor() as u16,
            0,
        ),
        0,
        0,
        80,
        8,
        clicked_display_col + 0.1,
        0.1,
    );
    assert_eq!(editor.active_buffer().cursor, (0, value_col));

    editor.active_leaf_mut().widget_scroll_left = 4.0;
    assert_eq!(
        editor.widget_layout_scroll_left(),
        4.0 * frame.text_cell_width_scale
    );
}

#[test]
fn inline_scope_reserves_display_rows_and_keeps_following_source_line_identity() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def before 1)\n(~scope :track 0 :height 3)\n(def after 2)");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let map = editor.active_buffer().inline_display_row_map();
    assert_eq!(map.display_row_for_buffer_line(0), Some(0));
    assert_eq!(map.display_row_for_buffer_line(1), Some(1));
    assert_eq!(map.display_row_for_buffer_line(2), Some(5));

    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    assert_eq!(frame.lines.len(), 6);
    assert!(frame.lines[2].is_empty());
    assert!(frame.lines[3].is_empty());
    assert!(frame.lines[4].is_empty());
    let text_height_scale = frame.text_cell_height_scale;
    let layout = frame.widget_layout.expect("scope layout");
    let scope = layout
        .children
        .iter()
        .find(|node| node.widget_type == "scope")
        .expect("inline scope node");
    assert_eq!(scope.rect.row, 2.0 * text_height_scale);
    assert_eq!(scope.rect.height, 3.0 * text_height_scale);
    assert!(scope.rect.width > 0.0);
}

#[test]
fn inline_slider_infers_range_from_enclosing_keyword_call_metadata() {
    let mut runtime = Runtime::new();
    runtime.register_native("synth", |_args, _ctx| Ok(Value::Nil));
    runtime.set_inline_widget_metadata_resolver(std::rc::Rc::new(|callee, inlet| {
        (callee == "synth" && inlet == "cutoff").then_some(crate::vm::InlineWidgetMetadata {
            min: Some(20.0),
            max: Some(20_000.0),
            step: Some(1.0),
        })
    }));
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(synth :cutoff (~slider 440))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);

    let widget = &editor.active_buffer().inline_code_widgets()[0].widget;
    let Value::Map(map) = widget else {
        panic!("inline slider should be a widget map");
    };
    assert!(matches!(&*map["min"].borrow(), Value::Number(20.0)));
    assert!(matches!(&*map["max"].borrow(), Value::Number(20_000.0)));
    assert!(matches!(&*map["step"].borrow(), Value::Number(1.0)));
}

#[test]
fn inline_slider_drag_writes_literal_and_re_evaluates_once_on_release() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def amount (~slider 12 :min 0 :max 24))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);
    editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].chars().count());

    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let layout = frame.widget_layout.expect("inline slider layout");
    let slider = layout
        .children
        .iter()
        .find(|node| node.widget_type == "hslider")
        .expect("inline slider")
        .clone();
    let start_col = slider.rect.col + slider.rect.width * 0.5;
    let end_col = slider.rect.col + slider.rect.width - 0.1;
    let row = slider.rect.row + 0.5;
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            start_col as u16,
            row as u16,
        ),
        0,
        0,
        80,
        8,
        start_col,
        row,
    );
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            end_col as u16,
            row as u16,
        ),
        0,
        0,
        80,
        8,
        end_col,
        row,
    );
    assert!(
        !editor.active_buffer().text().contains("(~slider 12"),
        "drag should rewrite the literal: {}",
        editor.active_buffer().text()
    );
    editor.handle_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            end_col as u16,
            row as u16,
        ),
        0,
        0,
        80,
        8,
        end_col,
        row,
    );

    assert!(!editor.active_buffer().text().contains("(~slider 12"));
    assert_eq!(editor.active_buffer().inline_code_widgets().len(), 1);
    assert_eq!(editor.undo_stack.len(), 1);
}

#[test]
fn tiled_inline_slider_drag_reapplies_anchor_after_pointer_viewport_relayout() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def amount (~slider 12 :min 0 :max 24))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);
    editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].chars().count());

    let tile_id = editor.active_tile;
    if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
        leaf.show_border = false;
    }
    let tile_rect = crate::layout::Rect {
        col: 7.25,
        row: 3.5,
        width: 80.0,
        height: 9.0,
    };
    editor.cached_tile_rects = vec![(tile_id, tile_rect)];
    editor.set_layout_viewport_exact(80.0, 8.0);
    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let slider = frame
        .widget_layout
        .expect("inline slider layout")
        .children
        .iter()
        .find(|node| node.widget_type == "hslider")
        .expect("inline slider")
        .clone();

    // Reproduce the tiled event router's viewport transition. A raw runtime
    // relayout puts the v-stack child back at its natural left edge; the
    // Editor must restore source-anchor placement before hit-testing.
    editor.runtime.set_layout_viewport_exact(12.0, 4.0);
    let start = (
        tile_rect.col + slider.rect.col + slider.rect.width * 0.25,
        tile_rect.row + slider.rect.row + slider.rect.height * 0.5,
    );
    let end = (
        tile_rect.col + slider.rect.col + slider.rect.width * 0.75,
        start.1,
    );

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            start.0.floor() as u16,
            start.1.floor() as u16,
        ),
        start.0,
        start.1,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            end.0.floor() as u16,
            end.1.floor() as u16,
        ),
        end.0,
        end.1,
        0,
    );
    assert!(
        !editor.active_buffer().text().contains("(~slider 12"),
        "drag at the rendered inline position must rewrite the literal"
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Up(MouseButton::Left),
            end.0.floor() as u16,
            end.1.floor() as u16,
        ),
        end.0,
        end.1,
        0,
    );
}

#[test]
fn inline_widget_dims_in_place_when_its_source_form_is_edited() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .active_buffer_mut()
        .set_text("(def amount (~slider 12 :min 0 :max 24))");
    editor.sync_runtime_context();
    let buffer_id = editor.active_buffer().id;
    editor.evaluate_buffer_transactional(buffer_id);
    let literal = editor.active_buffer().text().find("12").unwrap();
    editor
        .active_buffer_mut()
        .apply_text_edit(crate::buffer::TextEdit::new(literal, literal + 2, "oops"))
        .unwrap();

    let frame = crate::frame::build_render_frame(&mut editor, 80, 8);
    let layout = frame.widget_layout.expect("stale inline layout");
    let slider = layout
        .children
        .iter()
        .find(|node| node.widget_type == "hslider")
        .expect("stale slider remains visible");
    assert!(matches!(slider.props.get("muted"), Some(Value::Bool(true))));
}

// ── Modal input + focus (modal spec phase 3) ────────────────────────────────

/// Clears the thread-local overlay stack when the test ends, pass or fail, so
/// a mid-test assertion failure cannot leak overlay state into whichever test
/// runs next on the same thread.
#[cfg(target_os = "macos")]
struct OverlayClearGuard;

#[cfg(target_os = "macos")]
impl Drop for OverlayClearGuard {
    fn drop(&mut self) {
        crate::widget_render::clear_overlay();
    }
}

#[cfg(target_os = "macos")]
fn find_widget_of_type<'a>(
    node: &'a crate::layout::LayoutNode,
    widget_type: &str,
) -> Option<&'a crate::layout::LayoutNode> {
    if node.widget_type == widget_type {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_widget_of_type(child, widget_type))
}

/// Render the active tile's layout once so open overlays (modal panel,
/// dropdown menus) register their entries on the overlay stack, exactly as a
/// live frame draw would.
#[cfg(target_os = "macos")]
fn register_active_layout_overlays(editor: &mut Editor) {
    let layout = editor
        .runtime
        .current_layout
        .clone()
        .expect("active widget layout");
    // Mirror the backend: overlay rects are emitted in post-scroll tile-local
    // space, so the tile's live scroll has to reach the collector.
    let scroll_left = editor.widget_layout_scroll_left();
    let scroll_top = editor.total_scroll_top();
    let _ = crate::widget_render::collect_metal_primitives(
        &layout,
        crate::widget_render::WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 600.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top,
            scroll_left,
            inherited_hover: false,
        },
        scroll_top,
        20,
    );
}

#[cfg(target_os = "macos")]
fn modal_two_tile_editor(panel_body: &str) -> Editor {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(&format!(
            r#"
            (def modal-open (state true))
            (def modal-clicked (state false))
            (def underlay-clicked (state false))
            (effect-buffer "*panel*" {panel_body})
            (effect-buffer "*sequencer*"
              (button "underlay"
                :width 60
                :height 18
                :on-click (lambda (event) (set! underlay-clicked true))))
            (set-layout
              (list :rows :gap 0
                0.1 (list :buf "*panel*" :hide-status true)
                0.9 (list :buf "*sequencer*" :hide-status true)))
            "#
        ))
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    // Activate the panel tile (top, 2 rows) with a click on its content, then
    // rebuild the frame so the runtime layout is the panel's, laid out against
    // the whole-window frame viewport.
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    let panel_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*panel*")
        .unwrap();
    let panel_tile = editor
        .tile_root
        .find_leaf_by_buffer_idx(panel_idx)
        .unwrap()
        .id;
    assert_eq!(editor.active_tile, panel_tile, "panel tile must be active");
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    editor
}

#[cfg(target_os = "macos")]
const MODAL_PANEL_BODY: &str = r#"
    (modal :is-open modal-open
           :on-close (lambda () (set! modal-open false))
      (v-stack
        (button "inside"
          :focusable true
          :on-click (lambda (event) (set! modal-clicked true)))))
"#;

#[cfg(target_os = "macos")]
fn eval_bool(editor: &mut Editor, expr: &str) -> bool {
    match editor.runtime_mut().eval_str(expr).unwrap().unwrap() {
        Value::Bool(value) => value,
        other => panic!("{expr} => {other:?}"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn modal_click_inside_hits_a_modal_child_not_the_tile_below() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);
    let entry = crate::widget_render::topmost_overlay().expect("modal overlay entry");
    assert_eq!(entry.kind, crate::widget_render::OverlayKind::Modal);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let button = find_widget_of_type(&layout, "button")
        .expect("modal child button")
        .clone();
    let click_col = button.rect.col + button.rect.width * 0.5;
    let click_row = button.rect.row + button.rect.height * 0.5;
    // The click point visibly overlaps the sequencer tile below the 2-row
    // panel tile — without the modal intercept it would land there.
    assert!(
        click_row > 2.0,
        "button row {click_row} must escape the panel tile"
    );

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, click_col.floor() as u16, click_row.floor() as u16),
            click_col,
            click_row,
            0,
        );
    }

    assert!(
        eval_bool(&mut editor, "modal-clicked"),
        "modal child must receive the click"
    );
    assert!(
        !eval_bool(&mut editor, "underlay-clicked"),
        "click must not reach the tile below"
    );
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "inside click must not close the modal"
    );
}

/// Regression: a relayout triggered by a click handler (e.g. the sound
/// palette's "fork" growing its entry list) can reassign widget ids before
/// the next render refreshes the overlay entry. The stale id must not strand
/// the modal — clicks inside and Escape fall back to the open modal node by
/// type instead of consuming every event with no effect.
#[cfg(target_os = "macos")]
#[test]
fn stale_overlay_widget_id_cannot_strand_the_modal() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);
    let entry = crate::widget_render::topmost_overlay().expect("modal overlay entry");

    // Same rect, dead widget id — what the stack looks like between the
    // id-reassigning relayout and the next render.
    crate::widget_render::push_overlay(crate::widget_render::OverlayEntry {
        widget_id: entry.widget_id + 100_000,
        rect: entry.rect,
        kind: crate::widget_render::OverlayKind::Modal,
    });

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let button = find_widget_of_type(&layout, "button")
        .expect("modal child button")
        .clone();
    let click_col = button.rect.col + button.rect.width * 0.5;
    let click_row = button.rect.row + button.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, click_col.floor() as u16, click_row.floor() as u16),
            click_col,
            click_row,
            0,
        );
    }
    assert!(
        eval_bool(&mut editor, "modal-clicked"),
        "inside click must reach the modal child despite the stale entry id"
    );

    // Escape must still request close through the by-type fallback.
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "escape must close the modal despite the stale entry id"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn modal_outside_click_fires_on_close_without_activating_underneath() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);
    let entry = crate::widget_render::topmost_overlay().expect("modal overlay entry");

    // A scrim point outside the panel, over the underlay button's tile.
    let click_col = (entry.rect.col - 2.0).max(0.5);
    let click_row = 18.5;
    assert!(
        click_row < entry.rect.row
            || click_row >= entry.rect.row + entry.rect.height
            || click_col < entry.rect.col,
        "click point must be outside the panel rect {:?}",
        entry.rect
    );

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, click_col.floor() as u16, click_row.floor() as u16),
            click_col,
            click_row,
            0,
        );
    }

    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "scrim click must request close"
    );
    assert!(
        !eval_bool(&mut editor, "underlay-clicked"),
        "the dismissing click must not activate the widget underneath"
    );
}

#[cfg(target_os = "macos")]
const SCROLLABLE_MODAL_PANEL_BODY: &str = r#"
    (modal :is-open modal-open
           :on-close (lambda () (set! modal-open false))
      (scroll :height 10
        (v-stack
          (button "inside"
            :focusable true
            :on-click (lambda (event) (set! modal-clicked true)))
          (box :height 40))))
"#;

/// Regression: touchpad scroll used to route to the tile UNDER THE POINTER
/// while a modal was open. Wherever the panel hangs over a neighbouring
/// tile, the gesture switched the active tile (route_event_to_tile
/// persists the switch) — the modal then no longer existed in
/// runtime.current_layout, so every subsequent pointer event was consumed
/// with no effect and Escape could not close the modal (sound palette
/// "completely stuck" bug). Scroll must route to the modal's own tile.
#[cfg(target_os = "macos")]
#[test]
fn touchpad_scroll_over_modal_stays_in_the_modal_tile() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(SCROLLABLE_MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);
    let entry = crate::widget_render::topmost_overlay().expect("modal overlay entry");
    assert_eq!(entry.kind, crate::widget_render::OverlayKind::Modal);
    let panel_tile = editor.active_tile;

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let scroll_node = find_widget_of_type(&layout, "scroll")
        .expect("modal scroll container")
        .clone();
    let scroll_key = crate::widget_render::scroll::scroll_state_key(&scroll_node);
    let point_col = scroll_node.rect.col + scroll_node.rect.width * 0.5;
    let point_row = scroll_node.rect.row + scroll_node.rect.height * 0.5;
    // The point visibly overlaps the sequencer tile below the 2-row panel
    // tile — pointer-tile routing would switch the active tile there.
    assert!(
        point_row > 2.0,
        "scroll point row {point_row} must escape the panel tile"
    );

    assert!(
        editor.handle_tiled_touchpad_scroll(point_col, point_row, 0, 0.0, -60.0),
        "scroll over the panel must be trapped by the modal"
    );
    assert_eq!(
        editor.active_tile, panel_tile,
        "scroll over the panel must not switch the active tile"
    );
    assert!(
        crate::widget_render::scroll::get_scroll_state(scroll_key).offset_y > 0.0,
        "the modal's scroll container must receive the scroll"
    );

    // The modal must still be closable after the gesture.
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "escape must close the modal after scrolling over a neighbouring tile"
    );
}

/// Regression: inspect mode (ctrl+shift+i) hit-tested the full tile layout
/// with a containment gate at every node — the open modal's own layout node
/// is zero-size, so the recursion never entered its subtree and inspect
/// selected the widgets BEHIND the panel.
#[cfg(target_os = "macos")]
#[test]
fn inspect_mode_hits_modal_children_not_the_tile_below() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let button = find_widget_of_type(&layout, "button")
        .expect("modal child button")
        .clone();
    let hover_col = button.rect.col + button.rect.width * 0.5;
    let hover_row = button.rect.row + button.rect.height * 0.5;
    // The point visibly overlaps the sequencer tile below the 2-row panel
    // tile — pointer-tile routing would inspect the underlay there.
    assert!(
        hover_row > 2.0,
        "hover row {hover_row} must escape the panel tile"
    );

    editor.inspect_mode = true;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            hover_col.floor() as u16,
            hover_row.floor() as u16,
        ),
        hover_col,
        hover_row,
        0,
    );
    assert_eq!(
        editor.inspect_hover_widget_id,
        Some(button.widget_id),
        "inspect hover must select the modal child, not the tile below"
    );
}

/// Same as above but with the live sound-palette structure around the
/// modal: subtree wrapper, fill box root, scroll + each-generated rows.
#[cfg(target_os = "macos")]
const PALETTE_LIKE_MODAL_PANEL_BODY: &str = r#"
  (v-stack :width :fill :gap 0.0
    (subtree :key "test-sound-palette"
      (modal :is-open modal-open
             :on-close (lambda () (set! modal-open false))
        (box :debug-name "palette-panel" :width :fill :height :fill :bg :transparent
          (v-stack :width :fill :gap 0.2
            (scroll :width :fill :flex 1
              (v-stack :width :fill
                (each (range 0 6) |idx|
                  (button (str "entry-" idx)
                    :focusable true
                    :height 3
                    :on-click (lambda (event) (set! modal-clicked true)))))))))))
"#;

#[cfg(target_os = "macos")]
#[test]
fn inspect_mode_hits_palette_like_modal_children() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(PALETTE_LIKE_MODAL_PANEL_BODY);
    register_active_layout_overlays(&mut editor);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let button = find_widget_of_type(&layout, "button")
        .expect("palette entry button")
        .clone();
    let hover_col = button.rect.col + button.rect.width * 0.5;
    let hover_row = button.rect.row + button.rect.height * 0.5;
    assert!(
        hover_row > 2.0,
        "hover row {hover_row} must escape the panel tile"
    );

    editor.inspect_mode = true;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            hover_col.floor() as u16,
            hover_row.floor() as u16,
        ),
        hover_col,
        hover_row,
        0,
    );
    let hover_id = editor.inspect_hover_widget_id.expect("inspect hover hit");
    let modal = super::widget_focus::find_open_modal_node(&layout).expect("open modal");
    assert!(
        crate::layout::layout_contains_widget_id(modal, hover_id),
        "inspect hover must land inside the modal subtree"
    );
}

/// Regression: a modal can be open in a NON-active tile (the sound palette
/// opens from a badge click in the device panel, but mounts in the
/// step/arrangement buffer). Inspect must resolve the tile whose layout
/// contains the modal instead of hit-testing the active tile's layout —
/// which has no modal, so hits fell through to the widgets behind.
#[cfg(target_os = "macos")]
#[test]
fn inspect_mode_resolves_a_modal_in_a_non_active_tile() {
    let _overlay_guard = OverlayClearGuard;
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(&format!(
            r#"
            (def modal-open (state true))
            (def modal-clicked (state false))
            (def underlay-clicked (state false))
            (effect-buffer "*panel*" {MODAL_PANEL_BODY})
            (effect-buffer "*sequencer*"
              (button "underlay"
                :width 60
                :height 5
                :on-click (lambda (event) (set! underlay-clicked true))))
            (set-layout
              (list :rows :gap 0
                0.7 (list :buf "*panel*" :hide-status true)
                0.3 (list :buf "*sequencer*" :hide-status true)))
            "#
        ))
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    // Activate the SEQUENCER tile (bottom) — the modal's panel tile stays
    // inactive, like the palette opened from another tile's badge.
    assert!(editor.switch_active_tile_to_buffer_named("*sequencer*"));
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    let panel_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*panel*")
        .unwrap();
    let panel_leaf = editor
        .tile_root
        .find_leaf_by_buffer_idx(panel_idx)
        .expect("panel tile");
    let panel_tile = panel_leaf.id;
    assert_ne!(
        editor.active_tile, panel_tile,
        "panel tile must be inactive"
    );
    let panel_layout = panel_leaf
        .cached_layout
        .as_deref()
        .expect("inactive panel tile keeps a cached layout")
        .clone();
    let modal = super::widget_focus::find_open_modal_node(&panel_layout)
        .expect("open modal in the inactive tile's layout")
        .clone();

    // Re-register the overlay entry from the panel tile's layout, as its
    // live frame collection does each draw.
    let _ = crate::widget_render::collect_metal_primitives(
        &panel_layout,
        crate::widget_render::WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 600.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        0.0,
        20,
    );
    assert!(crate::widget_render::topmost_overlay().is_some());

    let button = find_widget_of_type(&modal, "button")
        .expect("modal child button")
        .clone();
    let hover_col = button.rect.col + button.rect.width * 0.5;
    let hover_row = button.rect.row + button.rect.height * 0.5;

    editor.inspect_mode = true;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            hover_col.floor() as u16,
            hover_row.floor() as u16,
        ),
        hover_col,
        hover_row,
        0,
    );
    assert_eq!(
        editor.inspect_hover_tile_id,
        Some(panel_tile),
        "inspect must resolve the modal's tile, not the active tile"
    );
    let hover_id = editor.inspect_hover_widget_id.expect("inspect hover hit");
    assert!(
        crate::layout::layout_contains_widget_id(&modal, hover_id),
        "inspect hover must land inside the modal subtree"
    );
}

/// Regression for inspect-source + resize: once source inspection activates
/// another tile, the modal owner becomes inactive. Its cached layout must be
/// rebuilt against the resized whole-frame viewport (not its own short tile),
/// and Escape must dispatch `:on-close` through that owner tile.
#[cfg(target_os = "macos")]
#[test]
fn inactive_modal_survives_frame_resize_and_escape_closes_it() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    let panel_tile = editor.active_tile;
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| *tile_id != panel_tile)
        .expect("second tile");
    editor.switch_active_tile(source_tile);

    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 90, 30);
    assert_eq!(editor.active_tile, source_tile, "source tile stays active");
    let panel_layout = editor
        .tile_root
        .find_leaf(panel_tile)
        .and_then(|leaf| leaf.cached_layout.clone())
        .expect("inactive modal layout after resize");
    let modal =
        super::widget_focus::find_open_modal_node(&panel_layout).expect("open modal after resize");
    let prop = |key: &str| match modal.props.get(key) {
        Some(Value::Number(value)) => *value as f32,
        other => panic!("missing numeric modal prop {key}: {other:?}"),
    };
    assert!((prop("_frame_width") - 90.0).abs() < 0.01);
    assert!((prop("_frame_height") - 30.0).abs() < 0.01);

    // Register the resized inactive tile's overlay as the backend does.
    let _ = crate::widget_render::collect_metal_primitives(
        &panel_layout,
        crate::widget_render::WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 900.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        0.0,
        30,
    );
    assert_eq!(
        crate::widget_render::topmost_overlay().map(|entry| entry.kind),
        Some(crate::widget_render::OverlayKind::Modal),
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "Escape must close a resized modal owned by an inactive tile"
    );
    assert_eq!(
        editor.active_tile, source_tile,
        "closing the modal must return to the inspected source tile"
    );
}

/// A modal remains the exclusive keyboard context when source inspection has
/// made its owner tile inactive. Focused modal controls still receive keys,
/// but an unhandled key must not fall through to a global editor binding.
#[cfg(target_os = "macos")]
#[test]
fn inactive_modal_routes_focused_keys_and_blocks_global_bindings() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def global-down-count (state 0))
            (def count-global-down ()
              (set! global-down-count (+ global-down-count 1)))
            (bind-key "DOWN" "count-global-down")
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();

    let panel_tile = editor.active_tile;
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| *tile_id != panel_tile)
        .expect("second tile");
    editor.switch_active_tile(source_tile);
    assert!(editor.modal_is_open());
    let panel_scroll_before = editor
        .tile_root
        .find_leaf(panel_tile)
        .expect("panel tile")
        .widget_scroll_top;

    editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        editor.runtime_mut().eval_str("global-down-count").unwrap(),
        Some(Value::Number(0.0)),
        "an unhandled modal key must not reach a global binding"
    );
    assert_eq!(
        editor.active_tile, source_tile,
        "modal key routing must preserve the inspected source tile"
    );
    assert_eq!(
        editor
            .tile_root
            .find_leaf(panel_tile)
            .expect("panel tile")
            .widget_scroll_top,
        panel_scroll_before,
        "an unhandled modal arrow key must not scroll its owning tile"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        eval_bool(&mut editor, "modal-clicked"),
        "Enter must still activate the focused control inside the modal"
    );
    assert_eq!(editor.active_tile, source_tile);
}

#[cfg(target_os = "macos")]
#[test]
fn inactive_modal_close_button_still_dispatches_after_resize() {
    let _overlay_guard = OverlayClearGuard;
    let body = r#"
        (modal :is-open modal-open
               :on-close (lambda () (set! modal-open false))
          (button "close"
            :focusable true
            :on-click (lambda (event) (set! modal-open false))))
    "#;
    let mut editor = modal_two_tile_editor(body);
    let panel_tile = editor.active_tile;
    let source_tile = editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .find(|tile_id| *tile_id != panel_tile)
        .expect("second tile");
    editor.switch_active_tile(source_tile);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 90, 30);

    let panel_layout = editor
        .tile_root
        .find_leaf(panel_tile)
        .and_then(|leaf| leaf.cached_layout.clone())
        .expect("inactive modal layout after resize");
    let button = find_widget_of_type(&panel_layout, "button")
        .expect("close button")
        .clone();
    let _ = crate::widget_render::collect_metal_primitives(
        &panel_layout,
        crate::widget_render::WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 900.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
        0.0,
        30,
    );
    let (content_col, content_row, _, _) = editor
        .tile_content_area(panel_tile, 0)
        .expect("panel content area");
    let precise_col = content_col as f32 + button.rect.col + button.rect.width * 0.5;
    let precise_row = content_row as f32 + button.rect.row + button.rect.height * 0.5;

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
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "a modal child close button must dispatch through the inactive owner tile"
    );
    assert_eq!(
        editor.active_tile, source_tile,
        "modal interaction must preserve the inspected source as the active tile"
    );
}

#[cfg(target_os = "macos")]
const MODAL_WITH_DROPDOWN_BODY: &str = r#"
    (modal :is-open modal-open
           :on-close (lambda () (set! modal-open false))
      (v-stack
        (dropdown
          :width 12
          :height 1.4
          :options '("plate" "hall" "quad")
          :value "plate")))
"#;

#[cfg(target_os = "macos")]
fn open_dropdown_inside_modal(editor: &mut Editor) {
    register_active_layout_overlays(editor);
    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let dropdown = find_widget_of_type(&layout, "dropdown")
        .expect("modal dropdown")
        .clone();
    let col = dropdown.rect.col + dropdown.rect.width * 0.5;
    let row = dropdown.rect.row + dropdown.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, col.floor() as u16, row.floor() as u16),
            col,
            row,
            0,
        );
    }
    // Re-render so the now-open menu registers its overlay entry above the modal.
    register_active_layout_overlays(editor);
    let top = crate::widget_render::topmost_overlay().expect("dropdown overlay entry");
    assert_eq!(
        top.kind,
        crate::widget_render::OverlayKind::Dropdown,
        "dropdown must stack above the modal"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn outside_click_closes_dropdown_inside_modal_but_keeps_the_modal() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_WITH_DROPDOWN_BODY);
    open_dropdown_inside_modal(&mut editor);

    // Click the scrim, outside both the menu and the panel.
    let click_col = 0.5;
    let click_row = 19.0;
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 19),
        click_col,
        click_row,
        0,
    );

    let top = crate::widget_render::topmost_overlay().expect("modal must survive");
    assert_eq!(
        top.kind,
        crate::widget_render::OverlayKind::Modal,
        "outside click closes only the dropdown above the modal"
    );
    assert!(eval_bool(&mut editor, "modal-open"), "modal must stay open");

    // The next outside click reaches the modal and requests close.
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 19),
        click_col,
        click_row,
        0,
    );
    assert!(!eval_bool(&mut editor, "modal-open"));
}

#[cfg(target_os = "macos")]
#[test]
fn escape_closes_the_dropdown_first_then_the_modal() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_WITH_DROPDOWN_BODY);
    open_dropdown_inside_modal(&mut editor);

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let top = crate::widget_render::topmost_overlay().expect("modal must survive first escape");
    assert_eq!(top.kind, crate::widget_render::OverlayKind::Modal);
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "first escape closes only the dropdown"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "second escape reaches the modal"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn modal_traps_focus_while_open_and_restores_it_after_close() {
    let _overlay_guard = OverlayClearGuard;
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def modal-open (state false))
            (effect-buffer "*panel*"
              (v-stack
                (button "under"
                  :focusable true
                  :width 20
                  :height 2)
                (modal :is-open modal-open
                       :on-close (lambda () (set! modal-open false))
                  (v-stack
                    (button "inside" :focusable true)))))
            (set-layout (list :buf "*panel*" :hide-status true))
            "#,
        )
        .unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    // Focus the underlay button with a click.
    let layout = editor.runtime.current_layout.clone().expect("layout");
    let under = find_widget_of_type(&layout, "button")
        .expect("under button")
        .clone();
    assert_eq!(
        under.props.get("text"),
        Some(&Value::String("under".to_string()))
    );
    let col = under.rect.col + under.rect.width * 0.5;
    let row = under.rect.row + under.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, col.floor() as u16, row.floor() as u16),
            col,
            row,
            0,
        );
    }
    let focused_before = editor.focused_widget_node().expect("under button focused");
    assert_eq!(
        focused_before.props.get("text"),
        Some(&Value::String("under".to_string()))
    );

    // Open the modal: focus jumps to its first focusable child.
    editor
        .runtime_mut()
        .eval_str("(set! modal-open true)")
        .unwrap();
    editor.refresh_runtime_side_effects();
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    let focused_in_modal = editor.focused_widget_node().expect("modal child focused");
    assert_eq!(
        focused_in_modal.props.get("text"),
        Some(&Value::String("inside".to_string())),
        "focus must be trapped inside the open modal"
    );

    // Close: the previously focused widget gets focus back.
    editor
        .runtime_mut()
        .eval_str("(set! modal-open false)")
        .unwrap();
    editor.refresh_runtime_side_effects();
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    let restored = editor.focused_widget_node().expect("focus restored");
    assert_eq!(
        restored.props.get("text"),
        Some(&Value::String("under".to_string())),
        "closing the modal must restore the previous focus"
    );
}

/// Inspect mode outranks the modal keyboard boundary: the toggle chord and
/// the Esc that exits inspect must work while a modal is open, without
/// disturbing the modal itself.
#[cfg(target_os = "macos")]
#[test]
fn inspect_toggle_and_escape_outrank_an_open_modal() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    assert!(editor.modal_is_open());

    editor.handle_key(KeyEvent::new(
        KeyCode::Char('I'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ));
    assert!(
        editor.inspect_mode,
        "ctrl+shift+i must reach the inspect toggle over an open modal"
    );
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "toggling inspect must not disturb the modal"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!editor.inspect_mode, "Esc must exit inspect mode first");
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "the modal stays open until the next Esc"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "with inspect closed, Esc closes the modal"
    );
}

/// An active prompt outranks the modal keyboard boundary: if a save prompt is
/// on screen while a modal is open, keystrokes must reach the prompt (its
/// filename input / y-n answers), not be swallowed by the modal.
#[cfg(target_os = "macos")]
#[test]
fn save_prompt_keys_outrank_an_open_modal() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = modal_two_tile_editor(MODAL_PANEL_BODY);
    assert!(editor.modal_is_open());

    editor.open_save_prompt(false);
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(
        editor
            .save_prompt
            .as_ref()
            .expect("save prompt stays open")
            .input,
        "x",
        "a keystroke must reach the save prompt, not the open modal"
    );
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "typing into the prompt must not disturb the modal"
    );

    // Esc answers the prompt (cancel) rather than closing the modal.
    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        editor.save_prompt.is_none(),
        "Esc must cancel the prompt first"
    );
    assert!(
        eval_bool(&mut editor, "modal-open"),
        "the modal stays open until the next Esc"
    );

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !eval_bool(&mut editor, "modal-open"),
        "with the prompt gone, Esc closes the modal"
    );
}

// ── Module-system slice 3: late-bound handler module capture (spec §5) ──

fn eval_module(editor: &mut Editor, tag: &str, source: &str) {
    let path = temp_file_path(&format!("module-{tag}.lisp"));
    editor
        .runtime_mut()
        .eval_source_at_path(path, source)
        .expect("module eval");
    editor.refresh_runtime_side_effects();
}

#[test]
fn module_bind_key_captures_module_and_dispatches_local_handler() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*test*", "x");
    eval_module(
        &mut editor,
        "bindkey-local",
        r#"
(module test.keys)
(def my-handler () (host-command "module-handler-ran" true))
(bind-key "C-y" "my-handler")
"#,
    );
    // The stored binding record carries the module (qualified handler).
    assert_eq!(
        editor.runtime.lisp_bindings().get("C-y").map(String::as_str),
        Some("test.keys/my-handler"),
        "bind-key from a declared module must store the module-qualified handler"
    );
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    let commands = editor.drain_host_commands();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HostCommand::Custom { name, .. } if name == "module-handler-ran")),
        "module-local handler should dispatch, got {commands:?}"
    );
}

#[test]
fn module_bind_key_falls_back_to_flat_handler() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*test*", "x");
    editor
        .runtime_mut()
        .eval_str(r#"(def shared-handler () (host-command "shared-ran" true))"#)
        .expect("vanilla handler");
    eval_module(
        &mut editor,
        "bindkey-fallback",
        r#"
(module test.keys2)
(bind-key "C-u" "shared-handler")
"#,
    );
    assert_eq!(
        editor.runtime.lisp_bindings().get("C-u").map(String::as_str),
        Some("test.keys2/shared-handler")
    );
    // test.keys2 never defined shared-handler: dispatch resolves the
    // stored module first, then falls back to the flat name (spec §5).
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let commands = editor.drain_host_commands();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HostCommand::Custom { name, .. } if name == "shared-ran")),
        "flat-handler fallback should dispatch, got {commands:?}"
    );
}

#[test]
fn module_define_mode_qualifies_name_and_on_key_handler() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*modetest*", "");
    eval_module(
        &mut editor,
        "define-mode",
        r#"
(module test.modes)
(def my-on-key (k txt) (host-command "mode-key" k) true)
(define-mode "special" :read-only true :on-key "my-on-key")
(set-buffer-mode "special")
"#,
    );
    // Registry key and buffer mode are the module-qualified name; the flat
    // set-buffer-mode reference from inside the module resolved to it.
    assert!(
        editor.mode_registry.contains_key("test.modes/special"),
        "define-mode in a declared module must register qualified, got {:?}",
        editor.mode_registry.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        editor.active_buffer().mode,
        BufferMode::Named("test.modes/special".to_string())
    );
    // The on-key handler captured its module and dispatches.
    editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    let commands = editor.drain_host_commands();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HostCommand::Custom { name, .. } if name == "mode-key")),
        "module on-key handler should dispatch, got {commands:?}"
    );
}

#[test]
fn major_modes_must_explicitly_opt_into_host_live_keys() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor
        .runtime_mut()
        .eval_str(
            r#"
(define-mode "text-mode")
(define-mode "performance-mode" :live-keys true)
(set-buffer-mode "text-mode")
"#,
        )
        .expect("define modes");
    editor.refresh_runtime_side_effects();
    assert!(!editor.active_mode_accepts_live_keys());

    editor
        .runtime_mut()
        .eval_str(r#"(set-buffer-mode "performance-mode")"#)
        .expect("select performance mode");
    editor.refresh_runtime_side_effects();
    assert!(editor.active_mode_accepts_live_keys());
}

/// A declared module's mode
/// reference is qualified against the *caller*, and must still fall back to
/// a vanilla mode the module never defined.
#[test]
fn module_mode_reference_falls_back_to_a_vanilla_mode() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*plain*", "");
    editor
        .runtime_mut()
        .eval_str(r#"(define-mode "plain-mode" :read-only true)"#)
        .expect("vanilla mode");
    editor.refresh_runtime_side_effects();
    eval_module(
        &mut editor,
        "mode-fallback",
        r#"
(module test.modeconsumer)
(set-buffer-mode-for "*plain*" "plain-mode")
"#,
    );
    let buffer = editor
        .buffers
        .iter()
        .find(|b| b.name == "*plain*")
        .expect("*plain* buffer");
    assert_eq!(
        buffer.mode,
        BufferMode::Named("plain-mode".to_string()),
        "module caller must still reach a vanilla mode by its flat name"
    );
    assert!(buffer.read_only, "the vanilla mode's read-only must apply");
}

/// Hazard (d), second thread: `mode_bind_key` qualifies its *handler* string
/// unconditionally, and `ui/seq-grid-mode.lisp` binds seven handlers defined
/// outside itself. End-to-end proof that the dispatch-side ladder
/// (`Runtime::resolve_handler_name`) covers it.
#[test]
fn module_mode_binding_dispatches_a_vanilla_handler() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*bound*", "");
    editor
        .runtime_mut()
        .eval_str(r#"(def cursor-left () (host-command "cursor-left-ran" true))"#)
        .expect("vanilla handler");
    editor.refresh_runtime_side_effects();
    eval_module(
        &mut editor,
        "mode-bind-foreign",
        r#"
(module test.boundmode)
(define-mode "bound-mode")
(mode-bind-key "bound-mode" "C-b" "cursor-left")
(set-buffer-mode-for "*bound*" "bound-mode")
"#,
    );
    // The binding is stored under the caller's module, which never defined
    // the handler.
    assert_eq!(
        editor
            .mode_registry
            .get("test.boundmode/bound-mode")
            .and_then(|m| m.keybindings.get("C-b"))
            .map(String::as_str),
        Some("test.boundmode/cursor-left"),
        "mode-bind-key qualifies the handler against the caller's module"
    );
    // *bound* is the active buffer (last scratch opened), so the mode keymap
    // is the one consulted on key input.
    assert_eq!(
        editor.active_buffer().mode,
        BufferMode::Named("test.boundmode/bound-mode".to_string())
    );
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    let commands = editor.drain_host_commands();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HostCommand::Custom { name, .. } if name == "cursor-left-ran")),
        "qualified→flat handler fallback should dispatch, got {commands:?}"
    );
}

#[test]
fn active_mode_keybinding_exposes_rebindable_hold_command_semantics() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*hold*", "");
    eval_module(
        &mut editor,
        "hold-binding",
        r#"
(module test.hold)
(def sequence-roll-hold () true)
(def ordinary () true)
(define-mode "hold-mode")
(mode-bind-key "hold-mode" "`" "sequence-roll-hold")
(set-buffer-mode-for "*hold*" "hold-mode")
"#,
    );
    let backquote = KeyEvent::new(KeyCode::Char('`'), KeyModifiers::NONE);
    assert_eq!(
        editor.active_mode_keybinding(backquote),
        Some("test.hold/sequence-roll-hold"),
    );

    // User lisp moves the semantic command by replacing the old binding and
    // assigning the command to a new key; host code only sees the keymap.
    editor
        .runtime_mut()
        .eval_str(
            r#"
(mode-bind-key "test.hold/hold-mode" "`" "ordinary")
(mode-bind-key "test.hold/hold-mode" "q" "test.hold/sequence-roll-hold")
"#,
        )
        .expect("rebind hold command");
    editor.refresh_runtime_side_effects();
    assert_ne!(
        editor.active_mode_keybinding(backquote),
        Some("test.hold/sequence-roll-hold"),
    );
    assert_eq!(
        editor.active_mode_keybinding(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        Some("test.hold/sequence-roll-hold"),
    );
}

#[test]
fn vanilla_bind_key_records_stay_flat() {
    let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
    editor.open_scratch_buffer("*test*", "x");
    editor
        .runtime_mut()
        .eval_str(
            r#"
(def flat-handler () (host-command "flat-ran" true))
(bind-key "C-j" "flat-handler")
"#,
        )
        .expect("vanilla bind");
    editor.refresh_runtime_side_effects();
    assert_eq!(
        editor.runtime.lisp_bindings().get("C-j").map(String::as_str),
        Some("flat-handler"),
        "headerless bind-key must keep today's flat handler string"
    );
    editor.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    let commands = editor.drain_host_commands();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HostCommand::Custom { name, .. } if name == "flat-ran"))
    );
}

#[test]
fn mx_command_defined_in_a_declared_module_requires_its_qualified_name() {
    // M-x exposes a declared module's qualified command and does not retain
    // migration-era flat vocabulary.
    let runtime = Runtime::with_init_source("");
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_source_at_path(
            std::env::temp_dir().join("mx-module-test.lisp"),
            r#"
(module test.mxmod)
(defstate probe-open? false)
(def probe-command () (set! probe-open? true))
"#,
        )
        .expect("module eval");

    // The candidate list carries the qualified name and the typed bare
    // name substring-matches it.
    let candidates = editor.collect_mx_candidates();
    assert!(
        candidates
            .iter()
            .any(|c| c == "test.mxmod/probe-command"),
        "qualified command should be an M-x candidate"
    );
    let filtered = super::filter_candidates(&candidates, "mx-probe-command");
    assert!(
        filtered.is_empty(),
        "retired flat name must not remain an M-x candidate: {:?}",
        candidates
            .iter()
            .filter(|c| c.contains("probe"))
            .collect::<Vec<_>>()
    );

    // Executing by the qualified candidate works…
    editor.execute_mx_command("test.mxmod/probe-command");
    assert_eq!(
        editor.runtime_mut().eval_str("test.mxmod/probe-open?").unwrap(),
        Some(Value::Bool(true)),
        "qualified M-x execution should run the command"
    );

    // Executing the retired flat spelling is a miss.
    editor
        .runtime_mut()
        .eval_str("(set! test.mxmod/probe-open? false)")
        .unwrap();
    editor.execute_mx_command("mx-probe-command");
    assert_eq!(
        editor.runtime_mut().eval_str("test.mxmod/probe-open?").unwrap(),
        Some(Value::Bool(false)),
        "retired flat-name M-x execution must not run the command"
    );
}

/// The S3 batch-1 choose-model shape end-to-end: a modal + dropdown declared
/// inside a module, the row click dispatching an on-change that calls a
/// module-local fn writing module defstates. Covers the full pointer
/// pipeline (overlay hit-test → Custom event → closure) with qualified
/// widget keys and state bindings.
#[cfg(target_os = "macos")]
#[test]
fn module_dropdown_row_click_applies_selection_and_closes_modal() {
    let _overlay_guard = OverlayClearGuard;
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_source_at_path(
            std::env::temp_dir().join("module-picker-test.lisp"),
            r#"
(module test.picker)
(defstate picker-open? true)
(defstate picked "plate")
(def choose (v)
  (do
    (set! picked v)
    (set! picker-open? false)))
(def panel ()
  (modal :is-open picker-open?
         :on-close (lambda () (set! picker-open? false))
    (v-stack
      (dropdown
        :key "dropdown"
        :width 12
        :height 1.4
        :options '("plate" "hall" "quad")
        :value picked
        :on-change (lambda (v) (choose v))))))
"#,
        )
        .expect("module picker source");
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*panel*" (test.picker/panel))
            (effect-buffer "*sequencer*"
              (button "underlay"
                :width 60
                :height 18
                :on-click (lambda (event) true)))
            (set-layout
              (list :rows :gap 0
                0.1 (list :buf "*panel*" :hide-status true)
                0.9 (list :buf "*sequencer*" :hide-status true)))
            "#,
        )
        .expect("mount panel");
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    open_dropdown_inside_modal(&mut editor);

    // Click the "hall" row (index 1) inside the open menu overlay.
    // Mirrors dropdown.rs MENU_PADDING_V=0.3 / MENU_ROW_HEIGHT=1.4.
    let menu = crate::widget_render::get_overlay_rect().expect("open dropdown menu rect");
    let row = menu.row + 0.3 + 1.4 * 1.0 + 0.7;
    let col = menu.col + menu.width * 0.5;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col.floor() as u16,
            row.floor() as u16,
        ),
        col,
        row,
        0,
    );
    editor.refresh_runtime_side_effects();

    assert_eq!(
        editor.runtime_mut().eval_str("test.picker/picked").unwrap(),
        Some(Value::String("hall".to_string())),
        "row click should dispatch on-change through the module fn"
    );
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str("test.picker/picker-open?")
            .unwrap(),
        Some(Value::Bool(false)),
        "the module fn should close the modal"
    );
}

/// Load the REAL ui/choose-model.lisp (agent natives stubbed) and click a
/// dropdown row — reproduces the in-app ArityMismatch report if present.
#[cfg(target_os = "macos")]
#[test]
fn real_choose_model_dropdown_row_click_selects() {
    let _overlay_guard = OverlayClearGuard;
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .register_native("agent/models", |_args, _ctx| {
            Ok(Value::List(vec![
                Rc::new(RefCell::new(Value::String("gpt-5.5".into()))),
                Rc::new(RefCell::new(Value::String("claude-fable-5".into()))),
            ]))
        });
    let picked = Rc::new(RefCell::new(String::new()));
    let picked_for_native = picked.clone();
    editor
        .runtime_mut()
        .register_native("agent/patch-model", move |_args, _ctx| {
            Ok(Value::String(picked_for_native.borrow().clone()))
        });
    let picked_for_set = picked.clone();
    editor
        .runtime_mut()
        .register_native("agent/set-patch-model", move |args, _ctx| {
            if let Some(Value::String(v)) = args.first() {
                *picked_for_set.borrow_mut() = v.clone();
            }
            Ok(Value::Nil)
        });
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sequencer/ui/choose-model.lisp");
    let source = std::fs::read_to_string(&source_path).expect("read real choose-model.lisp");
    editor
        .runtime_mut()
        .eval_source_at_path(source_path, &source)
        .expect("load real choose-model.lisp");
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (effect-buffer "*panel*" (eseq.choose-model/panel))
            (effect-buffer "*sequencer*"
              (button "underlay"
                :width 60
                :height 18
                :on-click (lambda (event) true)))
            (set-layout
              (list :rows :gap 0
                0.1 (list :buf "*panel*" :hide-status true)
                0.9 (list :buf "*sequencer*" :hide-status true)))
            "#,
        )
        .expect("mount panel");
    editor
        .runtime_mut()
        .eval_str("(eseq.choose-model/choose-model)")
        .expect("open picker");
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(200, 60);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 200, 60);
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 200, 60);

    open_dropdown_inside_modal(&mut editor);

    // Click the "claude-fable-5" row: options = Default (auto), gpt-5.5,
    // claude-fable-5 → index 2. MENU_PADDING_V=0.3, MENU_ROW_HEIGHT=1.4.
    let menu = crate::widget_render::get_overlay_rect().expect("open dropdown menu rect");
    let row = menu.row + 0.3 + 1.4 * 2.0 + 0.7;
    let col = menu.col + menu.width * 0.5;
    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            col.floor() as u16,
            row.floor() as u16,
        ),
        col,
        row,
        0,
    );
    editor.refresh_runtime_side_effects();
    assert_eq!(
        picked.borrow().as_str(),
        "claude-fable-5",
        "row click should write through agent/set-patch-model"
    );
    assert_eq!(
        editor.runtime_mut().eval_str("eseq.choose-model/open?").unwrap(),
        Some(Value::Bool(false)),
        "select should close the picker"
    );
}


/// Differential probe: identical to module_dropdown_row_click test but the
/// module fn is named `select` (a builtin widget name).
#[cfg(target_os = "macos")]
#[test]
fn module_fn_named_select_compiles_with_correct_arity() {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor
        .runtime_mut()
        .eval_source_at_path(
            std::env::temp_dir().join("module-select-arity-test.lisp"),
            r#"
(module test.selarity)
(defstate picked "plate")
(def select (v) (set! picked v))
(def handler-holder ()
  (lambda (v) (select v)))
"#,
        )
        .expect("module source");
    let cb = editor
        .runtime_mut()
        .eval_str("(test.selarity/handler-holder)")
        .expect("build handler")
        .expect("closure value");
    let one = editor
        .runtime
        .invoke(cb.clone(), vec![Value::String("hall".into())]);
    eprintln!("[probe-arity] invoke 1 arg => {one:?}");
    assert!(one.is_ok(), "lambda calling module fn named select must take 1 arg: {one:?}");
    assert_eq!(
        editor.runtime_mut().eval_str("test.selarity/picked").unwrap(),
        Some(Value::String("hall".to_string()))
    );
}



// ── Context menu: right-click plumbing + anchored overlay (eseq-dkn) ────────

#[cfg(target_os = "macos")]
const CONTEXT_MENU_PROGRAM: &str = r#"
    (def menu-open (state false))
    (def menu-col (state 0))
    (def menu-row (state 0))
    (def selected (state ""))
    (def underlay-clicked (state false))
    (effect-buffer "*panel*"
      (v-stack
        (box :width 58 :height 3
          :on-right-click (lambda (event)
            (set! menu-col (get event :col))
            (set! menu-row (get event :row))
            (set! menu-open true)))
        (context-menu :is-open menu-open
                      :anchor-col menu-col
                      :anchor-row menu-row
                      :on-close (lambda () (set! menu-open false))
          (menu-item "Rename" :shortcut "cmd-R"
            :on-select (lambda (event) (set! selected "rename")))
          (menu-separator)
          (menu-item "Blocked" :disabled true
            :on-select (lambda (event) (set! selected "blocked")))
          (menu-item "Delete"
            :on-select (lambda (event) (set! selected "delete"))))))
    (effect-buffer "*sequencer*"
      (button "underlay"
        :width 60
        :height 18
        :on-click (lambda (event) (set! underlay-clicked true))))
    (set-layout
      (list :rows :gap 0
        0.5 (list :buf "*panel*" :hide-status true)
        0.5 (list :buf "*sequencer*" :hide-status true)))
"#;

/// Same shape, but with a panel far wider and taller than its tile so the
/// tile can be panned/scrolled underneath the right-click.
#[cfg(target_os = "macos")]
const SCROLLED_CONTEXT_MENU_PROGRAM: &str = r#"
    (def menu-open (state false))
    (def menu-col (state 0))
    (def menu-row (state 0))
    (def selected (state ""))
    (def underlay-clicked (state false))
    (effect-buffer "*panel*"
      (v-stack
        (box :width 200 :height 30
          :on-right-click (lambda (event)
            (set! menu-col (get event :col))
            (set! menu-row (get event :row))
            (set! menu-open true)))
        (context-menu :is-open menu-open
                      :anchor-col menu-col
                      :anchor-row menu-row
                      :on-close (lambda () (set! menu-open false))
          (menu-item "Rename" :shortcut "cmd-R"
            :on-select (lambda (event) (set! selected "rename")))
          (menu-separator)
          (menu-item "Delete"
            :on-select (lambda (event) (set! selected "delete"))))))
    (effect-buffer "*sequencer*"
      (button "underlay"
        :width 60
        :height 18
        :on-click (lambda (event) (set! underlay-clicked true))))
    (set-layout
      (list :rows :gap 0
        0.5 (list :buf "*panel*" :hide-status true)
        0.5 (list :buf "*sequencer*" :hide-status true)))
"#;

#[cfg(target_os = "macos")]
fn context_menu_two_tile_editor() -> Editor {
    context_menu_two_tile_editor_for(CONTEXT_MENU_PROGRAM)
}

#[cfg(target_os = "macos")]
fn context_menu_two_tile_editor_for(program: &str) -> Editor {
    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    editor.runtime_mut().eval_str(program).unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(60, 20);
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);

    // Activate the panel tile with a click, then rebuild so the runtime
    // layout is the panel's, laid out against the whole-window viewport.
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    editor.handle_tiled_mouse_precise(
        mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 0),
        1.0,
        0.5,
        0,
    );
    let panel_idx = editor
        .buffers
        .iter()
        .position(|buffer| buffer.name == "*panel*")
        .unwrap();
    let panel_tile = editor
        .tile_root
        .find_leaf_by_buffer_idx(panel_idx)
        .unwrap()
        .id;
    assert_eq!(editor.active_tile, panel_tile, "panel tile must be active");
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    editor
}

/// Right-click at the given point, then rebuild the frame and register the
/// resulting overlay entries, as a live render would.
#[cfg(target_os = "macos")]
fn right_click_at(editor: &mut Editor, col: f32, row: f32) {
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Up(MouseButton::Right),
    ] {
        editor.handle_tiled_mouse_precise(mouse_event(kind, col as u16, row as u16), col, row, 0);
    }
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(editor, 60, 20);
    register_active_layout_overlays(editor);
}

#[cfg(target_os = "macos")]
fn eval_string(editor: &mut Editor, expr: &str) -> String {
    match editor.runtime_mut().eval_str(expr).unwrap().unwrap() {
        Value::String(value) => value,
        other => panic!("{expr} => {other:?}"),
    }
}

#[cfg(target_os = "macos")]
fn find_menu_item<'a>(
    node: &'a crate::layout::LayoutNode,
    text: &str,
) -> Option<&'a crate::layout::LayoutNode> {
    if node.widget_type == "menu-item"
        && matches!(node.props.get("text"), Some(Value::String(t)) if t == text)
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_menu_item(child, text))
}

#[cfg(target_os = "macos")]
#[test]
fn right_click_opens_the_context_menu_at_the_pointer() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);

    assert!(
        eval_bool(&mut editor, "menu-open"),
        ":on-right-click must open the menu"
    );
    let entry = crate::widget_render::topmost_overlay().expect("context menu overlay entry");
    assert_eq!(entry.kind, crate::widget_render::OverlayKind::Modal);
    assert!(
        (entry.rect.col - 6.0).abs() < 0.5 && (entry.rect.row - 1.5).abs() < 0.5,
        "panel {:?} must open at the pointer",
        entry.rect
    );
    assert!(
        !eval_bool(&mut editor, "underlay-clicked"),
        "right-click must not activate widgets underneath"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_flips_and_clamps_near_the_screen_edge() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 57.5, 1.5);

    let entry = crate::widget_render::topmost_overlay().expect("context menu overlay entry");
    assert!(
        entry.rect.col + entry.rect.width <= 60.001,
        "panel {:?} must stay inside the 60-col frame",
        entry.rect
    );
    assert!(entry.rect.col >= -0.001, "panel {:?} clamped left", entry.rect);
    assert!(
        entry.rect.row + entry.rect.height <= 20.001,
        "panel {:?} must stay inside the 20-row frame",
        entry.rect
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_anchors_at_the_pointer_when_the_tile_is_scrolled() {
    // eseq-05t: :col/:row reach the handler in tile CONTENT space (scroll
    // folded in), while the frame viewport the panel is clamped against is
    // fixed in frame space. Panning used to add the scroll to the anchor
    // without ever subtracting it back, flipping the panel to the left of the
    // pointer once the anchor ran past the frame's right edge.
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor_for(SCROLLED_CONTEXT_MENU_PROGRAM);
    {
        let leaf = editor.active_leaf_mut();
        leaf.widget_scroll_left = 55.0;
        leaf.widget_scroll_top = 18.0;
    }
    let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
    assert_eq!(editor.widget_scroll_left(), 55.0, "pan must survive clamping");
    assert_eq!(editor.total_scroll_top(), 18.0, "scroll must survive clamping");

    right_click_at(&mut editor, 6.0, 1.5);

    assert!(
        eval_bool(&mut editor, "menu-open"),
        ":on-right-click must open the menu"
    );
    // The handler sees the content-space pointer, past the frame's right edge.
    assert!(
        (eval_number(editor.runtime_mut(), "menu-col") - 61.0).abs() < 0.5,
        "anchor is reported in content space"
    );
    let entry = crate::widget_render::topmost_overlay().expect("context menu overlay entry");
    assert!(
        (entry.rect.col - 6.0).abs() < 0.5 && (entry.rect.row - 1.5).abs() < 0.5,
        "panel {:?} must open at the pointer despite the 55x18 tile scroll",
        entry.rect
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_item_hover_schedules_redraw_on_pointer_change() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let item = find_menu_item(&layout, "Rename").expect("Rename item").clone();
    let hover_col = item.rect.col + item.rect.width * 0.5;
    let hover_row = item.rect.row + item.rect.height * 0.5;
    crate::widget_render::set_pointer_hover_widget(None);
    editor.clear_needs_redraw();

    editor.handle_tiled_mouse_precise(
        mouse_event(
            MouseEventKind::Moved,
            hover_col.floor() as u16,
            hover_row.floor() as u16,
        ),
        hover_col,
        hover_row,
        0,
    );

    assert!(crate::widget_render::pointer_hovered(item.widget_id));
    assert!(
        editor.needs_redraw(),
        "changing render-only pointer hover state must schedule the next frame"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_item_click_fires_exactly_its_handler_and_closes() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let item = find_menu_item(&layout, "Delete").expect("Delete item").clone();
    let click_col = item.rect.col + item.rect.width * 0.5;
    let click_row = item.rect.row + item.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, click_col as u16, click_row as u16),
            click_col,
            click_row,
            0,
        );
    }

    assert_eq!(
        eval_string(&mut editor, "selected"),
        "delete",
        "exactly the clicked item's handler must fire"
    );
    assert!(
        !eval_bool(&mut editor, "menu-open"),
        "selecting an item must close the menu"
    );
    assert!(!eval_bool(&mut editor, "underlay-clicked"));
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_disabled_item_click_neither_fires_nor_closes() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);

    let layout = editor.runtime.current_layout.clone().expect("panel layout");
    let item = find_menu_item(&layout, "Blocked").expect("Blocked item").clone();
    let click_col = item.rect.col + item.rect.width * 0.5;
    let click_row = item.rect.row + item.rect.height * 0.5;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(
            mouse_event(kind, click_col as u16, click_row as u16),
            click_col,
            click_row,
            0,
        );
    }

    assert_eq!(eval_string(&mut editor, "selected"), "");
    assert!(
        eval_bool(&mut editor, "menu-open"),
        "a disabled item must not close the menu"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_outside_click_closes_without_firing_items() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);
    assert!(eval_bool(&mut editor, "menu-open"));

    // Click well below the panel, over the sequencer tile's underlay button.
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        editor.handle_tiled_mouse_precise(mouse_event(kind, 30, 15), 30.0, 15.5, 0);
    }

    assert!(
        !eval_bool(&mut editor, "menu-open"),
        "outside click must request close"
    );
    assert_eq!(eval_string(&mut editor, "selected"), "");
    assert!(
        !eval_bool(&mut editor, "underlay-clicked"),
        "the dismissing click must be consumed"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn context_menu_escape_closes_without_firing_items() {
    let _overlay_guard = OverlayClearGuard;
    let mut editor = context_menu_two_tile_editor();
    right_click_at(&mut editor, 6.0, 1.5);
    assert!(editor.modal_is_open(), "menu must gate keyboard input");

    editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(
        !eval_bool(&mut editor, "menu-open"),
        "Escape must request close"
    );
    assert_eq!(eval_string(&mut editor, "selected"), "");
}
