//! Keyed kinds and `:host` fields, driven by a fake host
//! (docs/kind-bindings-spec.md §3.1, §3.2, §4–§6; stage 3).

use std::cell::RefCell;
use std::rc::Rc;

use super::super::{EffectTarget, PendingUiUpdate, VM, VMError, Value};
use super::{FieldType, HostField};
use super::{
    INSTANCE_NAMESPACE_PREFIX, InstanceError, InstanceKindSchema, KEYED_INSTANCE_ID_BASE,
    SINGLETON_INSTANCE_ID_BASE,
};
use crate::reactive::read_float_slot;

const TRACK: &str = "(def-kind track :key (index)
  :host ((name    :string :doc \"Track name\")
         (volume  :number :range (0 1) :set set-volume)
         (muted   :bool   :set seq-set-mute)
         (peak    :number)
         (color   :rgb)
         (steps   (list-of step)))
  :state ((open false)))";

/// The fake host's side: `set-volume` is a Lisp function recording the
/// request in `log.last`; `seq-set-mute` a native recording its arguments.
type Calls = Rc<RefCell<Vec<Vec<Value>>>>;

fn vm() -> (VM, Calls) {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    super::super::register_math_natives(&mut vm);
    crate::widgets::register_widget_natives(&mut vm);
    let calls: Calls = Rc::default();
    let sink = calls.clone();
    vm.register_native_with_vm("seq-set-mute", move |args, _vm| {
        sink.borrow_mut().push(args);
        Value::Nil
    });
    eval(
        &mut vm,
        "(def-kind log :key () :state ((last 0) (calls 0)))
         (def set-volume (t v) (do (set! log.last v) (set! log.calls (+ log.calls 1))))",
    );
    eval(&mut vm, TRACK);
    (vm, calls)
}

fn eval(vm: &mut VM, code: &str) -> Value {
    vm.eval_str(code)
        .unwrap_or_else(|e| panic!("{code}: {e:?} {:?}", vm.take_source_load_errors()))
        .unwrap_or(Value::Nil)
}

fn error(vm: &mut VM, code: &str) -> String {
    match vm.eval_str(code) {
        Err(VMError::Instance(message)) => message,
        other => panic!("{code}: expected an instance error, got {other:?}"),
    }
}

fn compile_errors(vm: &mut VM, code: &str) -> String {
    assert_eq!(vm.eval_str(code), Err(VMError::CompileError), "{code}");
    vm.take_source_load_errors().join("\n")
}

fn rendered_targets(vm: &mut VM) -> Vec<String> {
    let mut targets: Vec<String> = vm
        .pending_widget_trees
        .drain(..)
        .filter_map(|update| match update {
            PendingUiUpdate::FullTree(tree) => match tree.target {
                EffectTarget::BufferName(name) => Some(name),
                _ => None,
            },
            PendingUiUpdate::ReplaceSubtree { target, .. } => match target {
                EffectTarget::BufferName(name) => Some(name),
                _ => None,
            },
        })
        .collect();
    targets.sort();
    targets
}

fn push(vm: &mut VM, id: u64, field: &str, value: Value) {
    vm.set_instance_field(id, field, value)
        .unwrap_or_else(|e| panic!("push {field}: {e}"));
    vm.process_dirty_reactive().expect("process");
}

#[test]
fn the_constructor_resolves_registered_keys_and_answers_nil_otherwise() {
    let (mut vm, _) = vm();
    assert_eq!(eval(&mut vm, "(track 3)"), Value::Nil);
    let id = vm.register_keyed_instance("track", &[3]).expect("register");
    assert!(id >= KEYED_INSTANCE_ID_BASE && id < SINGLETON_INSTANCE_ID_BASE);
    assert_eq!(
        vm.register_keyed_instance("scratch:track", &[3]),
        Ok(id),
        "idempotent"
    );
    assert_eq!(vm.keyed_instance("track", &[3]), Some(id));
    assert_eq!(eval(&mut vm, "(track 3)"), Value::Instance(id));
    assert_eq!(eval(&mut vm, "(track 4)"), Value::Nil);
    assert_eq!(eval(&mut vm, "(= (track 3) (track 3))"), Value::Bool(true));
    assert_eq!(
        eval(&mut vm, "(let ((t (track 3))) t.kind)"),
        Value::String("scratch:track".into())
    );
    assert_eq!(
        eval(&mut vm, "(let ((t (track 3))) t.key)"),
        eval(&mut vm, "(list 3)")
    );
    assert_eq!(
        eval(&mut vm, "(let ((t (track 3))) t.id)"),
        Value::Number(id as f64)
    );
    // Keyed kinds have id, kind and key; no owner/label.
    assert!(error(&mut vm, "(let ((t (track 3))) t.label)").contains(
        "kind 'scratch:track' has no field 'label'; fields: id, kind, key, name, volume, muted, \
         peak, color, steps, open"
    ));
    assert!(error(&mut vm, "(track \"3\")").contains("(track i) takes one non-negative integer"));
    assert!(error(&mut vm, "(track -1)").contains("non-negative integer"));
    // Printing names the kind and the key.
    assert_eq!(
        eval(&mut vm, "(str (track 3))"),
        Value::String(format!("<track#{id} [3]>"))
    );
    assert_eq!(eval(&mut vm, "(str log)"), Value::String("<log>".into()));
}

#[test]
fn a_constructor_reader_re_renders_when_its_key_appears_moves_or_goes() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        r#"
        (effect-buffer "*t0*" (label (if (track 0) "yes" "no")))
        (effect-buffer "*t1*" (label (if (track 1) "yes" "no")))
        "#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*t0*", "*t1*"]);
    let a = vm.register_keyed_instance("track", &[0]).expect("register");
    vm.process_dirty_reactive().expect("process");
    assert_eq!(rendered_targets(&mut vm), vec!["*t0*"]);
    // Registering an existing key changes nothing.
    vm.register_keyed_instance("track", &[0]).expect("register");
    vm.process_dirty_reactive().expect("process");
    assert!(rendered_targets(&mut vm).is_empty());
    vm.rekey_instance(a, &[1]).expect("rekey");
    vm.process_dirty_reactive().expect("process");
    assert_eq!(rendered_targets(&mut vm), vec!["*t0*", "*t1*"]);
    assert!(vm.drop_keyed_instance("track", &[1]));
    vm.process_dirty_reactive().expect("process");
    assert_eq!(rendered_targets(&mut vm), vec!["*t1*"]);
    assert_eq!(eval(&mut vm, "(track 1)"), Value::Nil);
}

#[test]
fn re_keying_keeps_ids_so_a_captured_instance_means_the_same_track() {
    let (mut vm, _) = vm();
    let a = vm.register_keyed_instance("track", &[0]).expect("a");
    let b = vm.register_keyed_instance("track", &[1]).expect("b");
    push(&mut vm, a, "name", Value::String("Kick".into()));
    eval(&mut vm, "(def first-track (track 0))");
    eval(
        &mut vm,
        r#"(effect-buffer "*key*" (label (str first-track.key)))"#,
    );
    rendered_targets(&mut vm);
    // A reorder swaps keys in one step.
    vm.rekey_instances(&[(a, vec![1]), (b, vec![0])])
        .expect("swap");
    vm.process_dirty_reactive().expect("process");
    assert_eq!(rendered_targets(&mut vm), vec!["*key*"], "t.key is tracked");
    assert_eq!(eval(&mut vm, "(track 0)"), Value::Instance(b));
    assert_eq!(eval(&mut vm, "first-track"), Value::Instance(a));
    assert_eq!(
        eval(&mut vm, "first-track.name"),
        Value::String("Kick".into())
    );
    assert_eq!(eval(&mut vm, "first-track.key"), eval(&mut vm, "(list 1)"));
    assert_eq!(vm.instance_key(a), Some(&[1][..]));
    // A move onto a held key fails and changes nothing.
    let error = vm.rekey_instance(a, &[0]).expect_err("conflict");
    assert!(error.to_string().contains("already held"), "{error}");
    assert_eq!(vm.keyed_instance("track", &[0]), Some(b));
    assert_eq!(vm.keyed_instance("track", &[1]), Some(a));
    assert!(vm.rekey_instance(a, &[1, 2]).is_err(), "wrong key length");
}

#[test]
fn parent_keyed_instances_hang_off_their_parent_and_reach_lisp_through_its_list_field() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    // `step` is declared before its parent: the parent resolves when a step
    // registers.
    eval(
        &mut vm,
        "(def-kind step :key (track index) :host ((active :bool) (velocity :number)))
         (def-kind track :key (index) :host ((steps (list-of step))))",
    );
    assert!(
        vm.eval_str("step").is_err(),
        "a parent-keyed kind has no constructor"
    );
    let t = vm.register_keyed_instance("track", &[2]).expect("track");
    let s0 = vm.register_keyed_instance("step", &[t, 0]).expect("step 0");
    let s1 = vm.register_keyed_instance("step", &[t, 1]).expect("step 1");
    push(&mut vm, s1, "active", Value::Bool(true));
    let steps = super::super::list_from_values([Value::Instance(s0), Value::Instance(s1)]);
    push(&mut vm, t, "steps", steps);
    assert_eq!(
        eval(
            &mut vm,
            "(let ((t (track 2))) (map (lambda (s) s.active) t.steps))"
        ),
        eval(&mut vm, "(list false true)")
    );
    assert_eq!(
        eval(&mut vm, "(let ((t (track 2))) (str (nth t.steps 1)))"),
        Value::String(format!("<step#{s1} [{t} 1]>"))
    );
    // A list of the wrong kind is a type error.
    let error = vm
        .set_instance_field(
            t,
            "steps",
            super::super::list_from_values([Value::Instance(t)]),
        )
        .expect_err("a track is not a step");
    assert!(error.to_string().contains("(list-of step)"), "{error}");
    // The parent must be a live instance of the parent kind.
    let error = vm
        .register_keyed_instance("step", &[s0, 0])
        .expect_err("parent");
    assert!(
        error
            .to_string()
            .contains("not a live 'scratch:track' instance"),
        "{error}"
    );
    assert!(
        vm.register_keyed_instance("step", &[t]).is_err(),
        "key length"
    );
    assert_eq!(
        vm.create_instance(1, "scratch:step")
            .expect_err("keyed")
            .to_string(),
        "step instances come from the project; reach them through their track"
    );
    // Re-keying the parent leaves its steps alone; dropping it drops them.
    vm.rekey_instance(t, &[5]).expect("rekey");
    assert_eq!(vm.keyed_instance("step", &[t, 1]), Some(s1));
    assert!(vm.drop_instance(t));
    assert!(!vm.instance_is_live(s0) && !vm.instance_is_live(s1));
    assert_eq!(vm.keyed_instance("step", &[t, 1]), None);
}

#[test]
fn a_parent_that_is_not_keyed_is_an_error_at_registration() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    eval(
        &mut vm,
        "(def-kind menu :key () :state ((open false)))
         (def-kind item :key (menu index) :host ((label :string)))
         (def-kind orphan :key (nothing index) :host ((label :string)))",
    );
    let error = vm
        .register_keyed_instance("item", &[SINGLETON_INSTANCE_ID_BASE, 0])
        .expect_err("menu is a singleton");
    assert!(
        error
            .to_string()
            .contains("keyed under 'menu', which is not a keyed kind"),
        "{error}"
    );
    assert!(vm.register_keyed_instance("orphan", &[1, 0]).is_err());
    assert!(matches!(
        vm.register_keyed_instance("menu", &[0]),
        Err(InstanceError::InvalidKey(message)) if message.contains("not keyed")
    ));
    assert_eq!(
        vm.register_keyed_instance("nope", &[0]),
        Err(InstanceError::UnknownKind("nope".into()))
    );
}

#[test]
fn host_fields_read_their_pushed_values_and_dirty_only_their_readers() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    // Before the host pushes anything, each field reads its type's default.
    assert_eq!(
        eval(
            &mut vm,
            "(let ((t (track 0))) (list t.name t.volume t.muted t.steps t.color))"
        ),
        eval(&mut vm, "(list \"\" 0 false (list) (rgb 0 0 0))")
    );
    eval(
        &mut vm,
        r#"
        (def t0 (track 0))
        (effect-buffer "*name*" (label t0.name))
        (effect-buffer "*peak*" (box :width (+ 1 t0.peak) :height 1))
        "#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*name*", "*peak*"]);
    push(&mut vm, id, "name", Value::String("Bass".into()));
    assert_eq!(rendered_targets(&mut vm), vec!["*name*"]);
    push(&mut vm, id, "peak", Value::Number(0.25));
    assert_eq!(rendered_targets(&mut vm), vec!["*peak*"]);
    push(&mut vm, id, "peak", Value::Number(0.25));
    assert!(
        rendered_targets(&mut vm).is_empty(),
        "an unchanged push dirties nothing"
    );
    assert_eq!(eval(&mut vm, "t0.name"), Value::String("Bass".into()));
    // Pushes are type-checked.
    assert_eq!(
        vm.set_instance_field(id, "peak", Value::String("loud".into())),
        Err(InstanceError::TypeMismatch {
            kind: "scratch:track".into(),
            field: "peak".into(),
            expected: ":number".into(),
            got: "\"loud\"".into(),
        })
    );
    assert!(vm.set_instance_field(id, "nope", Value::Nil).is_err());
    // The host cannot write the key through a field push.
    assert!(vm.set_instance_field(id, "key", Value::Nil).is_err());
    // Range and doc are schema metadata.
    let schema = vm.instance_kind_schema("scratch:track").expect("schema");
    assert_eq!(schema.host[1].range, Some((0.0, 1.0)));
    assert_eq!(schema.host[0].doc.as_deref(), Some("Track name"));
}

#[test]
fn a_bound_host_field_repaints_only_when_a_push_changes_it() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    push(&mut vm, id, "volume", Value::Number(0.25));
    let volume = eval(&mut vm, "(def t0 (track 0)) #'t0.volume");
    let Value::ReactiveRef { slot, .. } = &volume else {
        panic!("expected a ref, got {volume:?}");
    };
    assert_eq!(read_float_slot(slot), 0.25, "seeded from the cell");
    vm.take_pending_binding_repaints();
    push(&mut vm, id, "volume", Value::Number(0.5));
    assert_eq!(read_float_slot(slot), 0.5);
    let namespace = format!("{INSTANCE_NAMESPACE_PREFIX}{id}");
    assert_eq!(
        vm.take_pending_binding_repaints(),
        vec![(namespace, "volume".to_string())]
    );
    push(&mut vm, id, "volume", Value::Number(0.5));
    assert!(vm.take_pending_binding_repaints().is_empty());
    // An :rgb host field binds three slots; a :string one does not bind.
    eval(&mut vm, "#'t0.color");
    assert!(error(&mut vm, "#'t0.name").contains("is :string, which is not bindable"));
    // Dropping the instance writes the type default into the held slot.
    assert!(vm.drop_instance(id));
    assert_eq!(read_float_slot(slot), 0.0);
}

#[test]
fn set_on_a_host_field_calls_its_setter_and_leaves_the_cell_to_the_host() {
    let (mut vm, calls) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    eval(&mut vm, "(def t (track 0))");
    // A Lisp :set function receives the instance and the value.
    assert_eq!(eval(&mut vm, "(set! t.volume 0.7)"), Value::Number(0.7));
    assert_eq!(eval(&mut vm, "log.last"), Value::Number(0.7));
    assert_eq!(
        eval(&mut vm, "t.volume"),
        Value::Number(0.0),
        "the host pushes it back"
    );
    // A native :set.
    eval(&mut vm, "(set! t.muted (not t.muted))");
    assert_eq!(
        *calls.borrow(),
        vec![vec![Value::Instance(id), Value::Bool(true)]]
    );
    // The value is type-checked before the setter runs.
    assert_eq!(
        error(&mut vm, "(set! t.volume \"x\")"),
        "field 'volume' of kind 'scratch:track' is :number; got \"x\""
    );
    assert_eq!(eval(&mut vm, "log.calls"), Value::Number(1.0));
    // Without :set the field is read-only, as are the built-ins.
    assert_eq!(error(&mut vm, "(set! t.peak 1)"), "track.peak is read-only");
    assert!(error(&mut vm, "(set! t.key (list 2))").contains("read-only"));
    // :state fields of a keyed kind are local cells.
    eval(&mut vm, "(set! t.open true)");
    assert_eq!(eval(&mut vm, "t.open"), Value::Bool(true));
}

#[test]
fn a_singleton_takes_host_and_state_fields_together() {
    let (mut vm, calls) = vm();
    eval(
        &mut vm,
        "(def-kind transport :key ()
           :host ((playing :bool :set seq-set-mute) (tempo :number))
           :state ((open false)))",
    );
    let id = match eval(&mut vm, "transport") {
        Value::Instance(id) => id,
        other => panic!("expected the instance, got {other:?}"),
    };
    eval(
        &mut vm,
        r#"(effect-buffer "*play*" (label (if transport.playing "on" "off")))"#,
    );
    rendered_targets(&mut vm);
    push(&mut vm, id, "playing", Value::Bool(true));
    assert_eq!(rendered_targets(&mut vm), vec!["*play*"]);
    eval(&mut vm, "(set! transport.playing false)");
    assert_eq!(
        *calls.borrow(),
        vec![vec![Value::Instance(id), Value::Bool(false)]]
    );
    assert_eq!(eval(&mut vm, "transport.playing"), Value::Bool(true));
    assert_eq!(
        error(&mut vm, "(set! transport.tempo 1)"),
        "transport.tempo is read-only"
    );
    // Hot reload keeps pushed values by name.
    push(&mut vm, id, "tempo", Value::Number(120.0));
    eval(
        &mut vm,
        "(def-kind transport :key () :host ((tempo :number) (bar :int)) :state ((open false)))",
    );
    assert_eq!(eval(&mut vm, "transport.tempo"), Value::Number(120.0));
    assert_eq!(eval(&mut vm, "transport.bar"), Value::Number(0.0));
}

#[test]
fn malformed_keyed_kinds_and_host_on_created_kinds_are_errors() {
    let (mut vm, _) = vm();
    for (code, expected) in [
        (
            "(def-kind probe :host ((x :number)) :state ((y 0)))",
            ":host fields need :key",
        ),
        (
            "(def-kind a :key (x y z) :host ((x :number)))",
            ":key expects () (a singleton), (index) or (parent index)",
        ),
        ("(def-kind a :key (1) :host ((x :number)))", ":key expects"),
        (
            "(def-kind a :key (index) :view show)",
            "a keyed kind has no :view",
        ),
        (
            "(def-kind a :key (index) :document ((x 0)))",
            "a keyed kind has no :document",
        ),
        (
            "(def-kind a :key (index) :host ((x)))",
            "each :host entry is (field type option…)",
        ),
        (
            "(def-kind a :key (index) :host ((x 0)))",
            "invalid field type",
        ),
        (
            "(def-kind a :key (index) :host ((x :float)))",
            "unknown field type :float",
        ),
        (
            "(def-kind a :key (index) :host ((x :number :range 1)))",
            ":range takes (lo hi)",
        ),
        (
            "(def-kind a :key (index) :host ((x :number :doc 1)))",
            ":doc takes a string",
        ),
        (
            "(def-kind a :key (index) :host ((x :number :bogus 1)))",
            "unknown option :bogus",
        ),
    ] {
        let errors = compile_errors(&mut vm, code);
        assert!(errors.contains(expected), "{code}: {errors}");
    }
    assert!(
        error(
            &mut vm,
            "(def-kind a :key (index) :host ((x :number :set 3)))"
        )
        .contains(":set takes a function")
    );
    // The same rule for a schema registered from Rust.
    let error = vm
        .register_instance_kind(
            InstanceKindSchema::new("pkg:probe")
                .with_host_field(HostField::new("x", FieldType::Number)),
        )
        .expect_err("D5");
    assert!(
        error.to_string().contains(":host fields need :key"),
        "{error}"
    );
    // A kind keeps its :key shape until restart.
    assert!(
        error_text(vm.register_instance_kind(InstanceKindSchema::new("scratch:track").singleton()))
            .contains("is already defined keyed (:key (index))")
    );
}

fn error_text(result: Result<(), InstanceError>) -> String {
    result.expect_err("expected an error").to_string()
}

#[test]
fn keyed_kinds_stay_out_of_the_hosts_project_instances() {
    let (mut vm, _) = vm();
    vm.register_instance_kind(InstanceKindSchema::new("pkg:probe").field("x", Value::Number(1.0)))
        .expect("created kind");
    vm.create_instance(1, "pkg:probe").expect("create");
    let track = vm.register_keyed_instance("track", &[0]).expect("register");
    assert_eq!(vm.live_instances(), vec![1]);
    assert_eq!(vm.instance_kind_ids(), vec!["pkg:probe".to_string()]);
    assert!(vm.instance_is_live(track));
    assert_eq!(
        vm.create_instance(2, "scratch:track")
            .expect_err("keyed")
            .to_string(),
        "track instances come from the project; use (track i)"
    );
    assert!(!vm.instance_is_live(2));
}

#[test]
fn a_dropped_keyed_instance_reads_defaults_and_ignores_writes() {
    let (mut vm, calls) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    push(&mut vm, id, "name", Value::String("Lead".into()));
    eval(&mut vm, "(def t (track 0)) (set! t.open true)");
    eval(&mut vm, r#"(effect-buffer "*name*" (label t.name))"#);
    rendered_targets(&mut vm);
    assert!(vm.drop_instance(id));
    vm.process_dirty_reactive().expect("process");
    assert_eq!(rendered_targets(&mut vm), vec!["*name*"]);
    assert_eq!(eval(&mut vm, "t.name"), Value::String(String::new()));
    assert_eq!(eval(&mut vm, "t.open"), Value::Bool(false));
    assert_eq!(eval(&mut vm, "t.key"), Value::List(Vec::new()));
    assert_eq!(eval(&mut vm, "(track 0)"), Value::Nil);
    // Writes, :set ones included, are no-ops; host pushes too.
    eval(
        &mut vm,
        "(set! t.muted true) (set! t.volume 1) (set! t.open true)",
    );
    assert!(calls.borrow().is_empty());
    assert_eq!(eval(&mut vm, "log.calls"), Value::Number(0.0));
    assert_eq!(
        vm.set_instance_field(id, "name", Value::String("x".into())),
        Ok(())
    );
    assert_eq!(eval(&mut vm, "t.name"), Value::String(String::new()));
    assert_eq!(
        eval(&mut vm, "(str t)"),
        Value::String(format!("<track#{id}>"))
    );
    // The key is free again: a new registration is a new instance.
    let again = vm.register_keyed_instance("track", &[0]).expect("register");
    assert_ne!(again, id);
    assert!(!vm.instance_is_live(id));
}

#[test]
fn a_move_list_naming_one_instance_twice_is_rejected_and_changes_nothing() {
    let (mut vm, _) = vm();
    let a = vm.register_keyed_instance("track", &[0]).expect("a");
    let error = vm
        .rekey_instances(&[(a, vec![1]), (a, vec![2])])
        .expect_err("one instance, two keys");
    assert!(matches!(error, InstanceError::InvalidKey(_)), "{error}");
    assert_eq!(vm.keyed_instance("track", &[0]), Some(a));
    assert_eq!(vm.keyed_instance("track", &[1]), None);
    assert_eq!(vm.keyed_instance("track", &[2]), None);
    assert_eq!(vm.instance_key(a), Some(&[0][..]));
    // Two movers onto one key fail the same way.
    let b = vm.register_keyed_instance("track", &[1]).expect("b");
    assert!(vm.rekey_instances(&[(a, vec![5]), (b, vec![5])]).is_err());
    assert_eq!(vm.keyed_instance("track", &[5]), None);
    assert_eq!(
        (vm.instance_key(a), vm.instance_key(b)),
        (Some(&[0][..]), Some(&[1][..]))
    );
}

#[test]
fn a_hot_reload_keeps_the_whole_key_shape() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    eval(
        &mut vm,
        "(def-kind track :key (index) :host ((name :string)))
         (def-kind pattern :key (index) :host ((name :string)))
         (def-kind step :key (track index) :host ((active :bool)))",
    );
    // Renaming the index is fine; another parent is not.
    eval(&mut vm, "(def-kind track :key (i) :host ((name :string)))");
    eval(
        &mut vm,
        "(def-kind step :key (track i) :host ((active :bool)))",
    );
    let message = error(
        &mut vm,
        "(def-kind step :key (pattern index) :host ((active :bool)))",
    );
    assert!(
        message.contains("is already defined keyed (:key (track i)); restart to change its :key"),
        "{message}"
    );
    assert!(error(&mut vm, "(def-kind track :key (track index))").contains("restart"));
}

#[test]
fn a_rollback_never_hands_out_a_keyed_id_twice() {
    let (mut vm, _) = vm();
    let snapshot = vm.snapshot_state();
    let first = vm.register_keyed_instance("track", &[0]).expect("first");
    vm.restore_state(snapshot);
    assert!(!vm.instance_is_live(first), "the registration rolled back");
    let second = vm.register_keyed_instance("track", &[0]).expect("second");
    assert_ne!(first, second, "a stale handle never names a new instance");
}

#[test]
fn created_instances_cannot_take_vm_allocated_ids() {
    let (mut vm, _) = vm();
    vm.register_instance_kind(InstanceKindSchema::new("pkg:probe").field("x", Value::Number(1.0)))
        .expect("created kind");
    for id in [
        KEYED_INSTANCE_ID_BASE,
        KEYED_INSTANCE_ID_BASE + 7,
        SINGLETON_INSTANCE_ID_BASE,
    ] {
        assert_eq!(
            vm.create_instance(id, "pkg:probe"),
            Err(InstanceError::ReservedId(id))
        );
    }
    assert!(
        vm.create_instance(KEYED_INSTANCE_ID_BASE - 1, "pkg:probe")
            .is_ok()
    );
}

/// A track (`a:track`) with `step` under it, registered from Rust.
fn track_and_step_vm() -> VM {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    vm.register_instance_kind(
        InstanceKindSchema::new("a:step")
            .under("track", "index")
            .with_host_field(HostField::new("active", FieldType::Bool)),
    )
    .expect("step");
    vm.register_instance_kind(InstanceKindSchema::new("b:track").indexed("index"))
        .expect("track");
    vm
}

#[test]
fn dropping_a_track_drops_the_steps_under_it_and_only_those() {
    let mut vm = track_and_step_vm();
    let t0 = vm.register_keyed_instance("track", &[0]).expect("t0");
    let t1 = vm.register_keyed_instance("track", &[1]).expect("t1");
    let steps: Vec<u64> = (0..4)
        .map(|index| {
            vm.register_keyed_instance("a:step", &[t0, index])
                .expect("step")
        })
        .collect();
    let other = vm
        .register_keyed_instance("a:step", &[t1, 0])
        .expect("other");
    assert!(vm.drop_keyed_instance("track", &[0]));
    assert!(steps.iter().all(|step| !vm.instance_is_live(*step)));
    assert!(vm.instance_is_live(other));
    assert_eq!(vm.keyed_instance("a:step", &[t0, 2]), None);
    // A step under a dropped track cannot register.
    assert!(vm.register_keyed_instance("a:step", &[t0, 0]).is_err());
}

#[test]
fn a_step_moved_to_another_track_survives_its_old_track_and_goes_with_its_new_one() {
    let mut vm = track_and_step_vm();
    let t0 = vm.register_keyed_instance("track", &[0]).expect("t0");
    let t1 = vm.register_keyed_instance("track", &[1]).expect("t1");
    let step = vm
        .register_keyed_instance("a:step", &[t0, 3])
        .expect("step");
    vm.rekey_instance(step, &[t1, 0]).expect("move");
    assert!(vm.drop_instance(t0));
    assert!(vm.instance_is_live(step), "the step left t0");
    assert_eq!(vm.keyed_instance("a:step", &[t1, 0]), Some(step));
    assert!(vm.drop_instance(t1));
    assert!(!vm.instance_is_live(step));
    // The new parent must be a live track.
    let t2 = vm.register_keyed_instance("track", &[2]).expect("t2");
    let step = vm
        .register_keyed_instance("a:step", &[t2, 0])
        .expect("step");
    assert!(vm.rekey_instance(step, &[t0, 0]).is_err(), "t0 is gone");
}

#[test]
fn a_parent_kind_resolved_once_stays_when_a_later_kind_shares_its_name() {
    let mut vm = track_and_step_vm();
    let t0 = vm.register_keyed_instance("b:track", &[0]).expect("t0");
    let step = vm
        .register_keyed_instance("a:step", &[t0, 0])
        .expect("step");
    // `track` is ambiguous from now on; `a:step` keeps `b:track`.
    vm.register_instance_kind(InstanceKindSchema::new("c:track").indexed("index"))
        .expect("another track");
    let next = vm
        .register_keyed_instance("a:step", &[t0, 1])
        .expect("still b:track");
    assert!(vm.drop_instance(t0));
    assert!(!vm.instance_is_live(step) && !vm.instance_is_live(next));
}

#[test]
fn a_set_lambda_and_widget_named_fields_compile_like_any_def_kind() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    crate::widgets::register_widget_natives(&mut vm);
    eval(
        &mut vm,
        "(def-kind log :key () :state ((last 0)))
         (def-kind dial :key (index)
           :host ((label :string) (value :number :set (lambda (k v) (set! log.last v))))
           :state ((box (+ 1 2))))",
    );
    let id = vm.register_keyed_instance("dial", &[0]).expect("register");
    eval(&mut vm, "(let ((d (dial 0))) (set! d.value 4))");
    assert_eq!(eval(&mut vm, "log.last"), Value::Number(4.0));
    assert_eq!(vm.instance_field(id, "box"), Ok(Value::Number(3.0)));
    assert_eq!(
        vm.instance_field(id, "label"),
        Ok(Value::String(String::new()))
    );
}
