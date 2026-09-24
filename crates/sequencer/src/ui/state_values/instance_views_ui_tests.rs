//! Host-created per-instance buffers and tabs (docs/instance-kinds-spec.md
//! §7), driven through the real step-tab registry of the full UI.
use super::*;
use sequencer::project::{ProjectInstance, ProjectInstanceOwner, ProjectInstances};

const VIEW_SOURCE: &str = r#"
(def probe-view (s)
  (v-stack
    (box :key "weight-matrix" :width 4 :height 1)
    (label (str "n=" s.n) :key "count")))
(def-kind probe :state ((n 0)) :view probe-view :keymap eseq.sequencer-keys/sequencer-keys)
"#;

fn instance(id: u64, label: &str) -> ProjectInstance {
    ProjectInstance {
        id,
        kind: "scratch:probe".to_string(),
        owner: ProjectInstanceOwner::Project,
        label: label.to_string(),
    }
}

fn sync(editor: &mut Editor, instances: &ProjectInstances, reset: bool) {
    sequencer::lisp_host::sync_instance_records(editor.runtime_mut(), instances);
    crate::host_commands::instances::sync_instance_views(editor, instances, reset);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
}

fn eval(editor: &mut Editor, source: &str) -> Option<Value> {
    editor
        .runtime_mut()
        .eval_str(source)
        .unwrap_or_else(|error| panic!("{source}: {error:?}"))
}

fn has_buffer(editor: &Editor, name: &str) -> bool {
    editor.buffers.iter().any(|buffer| buffer.name == name)
}

fn tab_buffer(editor: &mut Editor, id: u64) -> Option<Value> {
    eval(editor, &format!("(eseq.seq-step-tabs/seq-instance-tab-buffer {id})"))
}

fn stable_keys(value: &Value, out: &mut Vec<String>) {
    let Value::Map(map) = value else {
        return;
    };
    if let Some(key) = map.get("__stable-key") {
        if let Value::String(key) = &*key.borrow() {
            out.push(key.clone());
        }
    }
    if let Some(children) = map.get("children") {
        if let Value::List(children) = &*children.borrow() {
            for child in children {
                stable_keys(&child.borrow(), out);
            }
        }
    }
}

fn buffer_keys(editor: &Editor, name: &str) -> Vec<String> {
    let buffer = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == name)
        .unwrap_or_else(|| panic!("{name} exists"));
    let mut keys = Vec::new();
    if let Some(tree) = buffer.widget_tree.as_ref() {
        stable_keys(tree, &mut keys);
    }
    keys
}

#[test]
fn instance_views_get_buffers_and_tabs_that_follow_the_instance_lifecycle() {
    sequencer::lisp_host::clear_kind_registry();
    let mut editor = full_grid_editor_for_scroll_tests();
    sequencer::lisp_host::register_def_kind_native(editor.runtime_mut());
    eval(&mut editor, VIEW_SOURCE);

    let mut instances = ProjectInstances::default();
    instances.list.push(instance(1, "Kit A"));
    instances.list.push(instance(2, "Kit B"));
    sync(&mut editor, &instances, false);

    // One buffer per instance, with the kind's :keymap, and one tab each
    // labelled with the instance label.
    for (id, label) in [(1, "Kit A"), (2, "Kit B")] {
        let name = format!("*probe · {label}*");
        assert!(has_buffer(&editor, &name), "{name}");
        let buffer = editor.buffers.iter().find(|buffer| buffer.name == name).unwrap();
        assert!(
            matches!(&buffer.mode, eseqlisp::BufferMode::Named(mode) if mode.ends_with("sequencer-keys")),
            "{name} keymap: {:?}",
            buffer.mode
        );
        assert_eq!(tab_buffer(&mut editor, id), Some(Value::String(name.clone())));
    }
    let labels = eval(
        &mut editor,
        "(map (lambda (tab) (nth tab 0)) (eseq.seq-step-tabs/seq-main-step-tabs))",
    );
    assert_eq!(
        labels,
        Some(Value::List(
            ["Seq", "Kit A", "Kit B"]
                .iter()
                .map(|label| Rc::new(RefCell::new(Value::String(label.to_string()))))
                .collect()
        ))
    );

    // Selecting a tab renders (view self) with keys scoped per instance.
    eval(&mut editor, "(eseq.seq-step-tabs/seq-select-main-step-tab-by-buffer \"*probe · Kit A*\")");
    eval(&mut editor, "(eseq.seq-step-tabs/seq-select-main-step-tab-by-buffer \"*probe · Kit B*\")");
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let keys_a = buffer_keys(&editor, "*probe · Kit A*");
    let keys_b = buffer_keys(&editor, "*probe · Kit B*");
    assert!(keys_a.contains(&"instance:1::weight-matrix".to_string()), "{keys_a:?}");
    assert!(keys_b.contains(&"instance:2::weight-matrix".to_string()), "{keys_b:?}");

    // Rename: the buffer is renamed in place and the tab follows.
    instances.get_mut(1).unwrap().label = "Kit C".to_string();
    sync(&mut editor, &instances, false);
    assert!(!has_buffer(&editor, "*probe · Kit A*"));
    assert!(has_buffer(&editor, "*probe · Kit C*"));
    assert_eq!(tab_buffer(&mut editor, 1), Some(Value::String("*probe · Kit C*".into())));

    // The tab × closes the tab only: the instance keeps its buffer, a
    // rename does not reopen it, `open` does.
    eval(&mut editor, "(eseq.seq-step-tabs/seq-unregister-step-sequencer-tab \"*probe · Kit C*\")");
    assert_eq!(tab_buffer(&mut editor, 1), Some(Value::Nil));
    instances.get_mut(1).unwrap().label = "Kit D".to_string();
    sync(&mut editor, &instances, false);
    assert!(has_buffer(&editor, "*probe · Kit D*"));
    assert_eq!(tab_buffer(&mut editor, 1), Some(Value::Nil), "a closed tab stays closed");
    crate::host_commands::instances::open_instance_view(&mut editor, 1).expect("open");
    assert_eq!(tab_buffer(&mut editor, 1), Some(Value::String("*probe · Kit D*".into())));
    assert_eq!(
        eval(&mut editor, "eseq.seq-step-tabs/step-panel-buffer"),
        Some(Value::String("*probe · Kit D*".into()))
    );

    // Two instances with one label get distinct buffers.
    instances.get_mut(2).unwrap().label = "Kit D".to_string();
    sync(&mut editor, &instances, false);
    assert!(has_buffer(&editor, "*probe · Kit D #1*"));
    assert!(has_buffer(&editor, "*probe · Kit D #2*"));

    // Delete: the tab, the buffer and the binding go.
    instances.list.retain(|instance| instance.id != 2);
    sync(&mut editor, &instances, false);
    assert!(!has_buffer(&editor, "*probe · Kit D #2*"));
    assert_eq!(tab_buffer(&mut editor, 2), Some(Value::Nil));
    assert_eq!(
        editor
            .runtime()
            .bound_view_buffers()
            .into_iter()
            .map(|(target, _)| target)
            .collect::<Vec<_>>(),
        vec!["*probe · Kit D*".to_string()]
    );

    // A replaced list (project open) starts every view afresh.
    let mut opened = ProjectInstances::default();
    opened.list.push(instance(1, "Other"));
    sync(&mut editor, &opened, true);
    assert!(!has_buffer(&editor, "*probe · Kit D*"));
    assert!(has_buffer(&editor, "*probe · Other*"));
    assert_eq!(tab_buffer(&mut editor, 1), Some(Value::String("*probe · Other*".into())));
}
