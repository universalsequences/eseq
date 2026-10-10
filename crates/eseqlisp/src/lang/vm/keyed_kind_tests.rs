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
fn a_kind_keyed_under_several_parent_kinds_takes_an_instance_of_any() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    eval(
        &mut vm,
        "(def-kind device :key ((track bus) did)
           :host ((track track) (bus bus) (name :string)))
         (def-kind track :key (index) :host ((devices (list-of device))))
         (def-kind bus :key (index) :host ((devices (list-of device))))
         (def-kind scene :key (index) :host ((name :string)))",
    );
    let t = vm.register_keyed_instance("track", &[0]).expect("track");
    let b = vm.register_keyed_instance("bus", &[0]).expect("bus");
    let td = vm
        .register_keyed_instance("device", &[t, 7])
        .expect("under a track");
    let bd = vm
        .register_keyed_instance("device", &[b, 7])
        .expect("under a bus");
    assert_ne!(td, bd, "one did under two parents is two devices");
    push(&mut vm, bd, "name", Value::String("Reverb".into()));
    let devices = super::super::list_from_values([Value::Instance(bd)]);
    push(&mut vm, b, "devices", devices);
    assert_eq!(
        eval(&mut vm, "(let ((b (bus 0)) (d (first b.devices))) d.name)"),
        Value::String("Reverb".into())
    );
    assert_eq!(
        eval(&mut vm, "(let ((b (bus 0))) (str (first b.devices)))"),
        Value::String(format!("<device#{bd} [{b} 7]>"))
    );
    // Any other kind is no parent.
    let s = vm.register_keyed_instance("scene", &[0]).expect("scene");
    let refused = vm
        .register_keyed_instance("device", &[s, 1])
        .expect_err("a scene is no parent");
    assert!(
        refused
            .to_string()
            .contains("not a live 'scratch:track' or 'scratch:bus' instance"),
        "{refused}"
    );
    assert_eq!(
        vm.create_instance(1, "scratch:device")
            .expect_err("keyed")
            .to_string(),
        "device instances come from the project; reach them through their track or bus"
    );
    // A device may move between parents of either kind; each parent drops
    // only its own.
    vm.rekey_instance(td, &[b, 8]).expect("to the bus");
    assert!(vm.drop_instance(t));
    assert!(vm.instance_is_live(td));
    assert!(vm.drop_instance(b));
    assert!(!vm.instance_is_live(td) && !vm.instance_is_live(bd));
    // The parent list is part of the key's shape.
    eval(
        &mut vm,
        "(def-kind device :key ((track bus) id) :host ((track track) (bus bus) (name :string)))",
    );
    let message = error(
        &mut vm,
        "(def-kind device :key (track did) :host ((name :string)))",
    );
    assert!(
        message.contains("is already defined keyed (:key ((track bus) id))"),
        "{message}"
    );
    // ... as a set: the same parents in another order are the same shape.
    eval(
        &mut vm,
        "(def-kind device :key ((bus track) id) :host ((track track) (bus bus) (name :string)))",
    );
    let message = error(
        &mut vm,
        "(def-kind device :key ((bus scene) id) :host ((name :string)))",
    );
    assert!(message.contains("is already defined keyed"), "{message}");
    for code in [
        "(def-kind a :key (() index) :host ((x :number)))",
        "(def-kind a :key ((track 1) index) :host ((x :number)))",
    ] {
        let errors = compile_errors(&mut vm, code);
        assert!(errors.contains(":key expects"), "{code}: {errors}");
    }
    // A parent named twice is an error, at compile time and in the VM's own
    // :key parser.
    let errors = compile_errors(
        &mut vm,
        "(def-kind a :key ((track bus track) index) :host ((x :number)))",
    );
    assert!(errors.contains("names parent track twice"), "{errors}");
    let symbol = |name: &str| Rc::new(RefCell::new(Value::Symbol(name.into())));
    let parents = Value::List(vec![symbol("bus"), symbol("bus")]);
    let key = Value::List(vec![Rc::new(RefCell::new(parents)), symbol("index")]);
    let refused = vm
        .def_keyed_kind_from_args(vec![
            Value::Symbol("b".into()),
            Value::Keyword("key".into()),
            key,
        ])
        .expect_err("a duplicate parent");
    assert!(
        format!("{refused:?}").contains("names parent bus twice"),
        "{refused:?}"
    );
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
fn field_info_reports_a_host_fields_declared_metadata() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        r#"
        (def knob-modes '("lo" "mid" "hi"))
        (def knob-wrote nil)
        (def set-knob-level (k v) (set! knob-wrote v))
        (def-kind dial :key (index)
          :host ((level :number :range (0 2) :default 1 :set set-knob-level :doc "Gain")
                 (mode :string :options knob-modes)
                 (name :string)))
        "#,
    );
    vm.register_keyed_instance("dial", &[0]).expect("register");
    // #' binds the number field; the (instance :field) form reaches any.
    let info = |vm: &mut VM, field: &str, key: &str| {
        let form = if field == "level" { "#'k.level".to_string() } else { format!("k :{field}") };
        eval(vm, &format!("(let ((k (dial 0))) (get (field-info {form}) :{key}))"))
    };
    assert_eq!(info(&mut vm, "level", "range"), eval(&mut vm, "(list 0 2)"));
    assert_eq!(info(&mut vm, "level", "default"), Value::Number(1.0));
    assert_eq!(info(&mut vm, "level", "settable"), Value::Bool(true));
    assert_eq!(info(&mut vm, "level", "doc"), Value::String("Gain".into()));
    assert_eq!(info(&mut vm, "level", "type"), Value::String(":number".into()));
    // :options names a global (or is a list): field-info hands back the list
    // as it was when def-kind ran.
    assert_eq!(info(&mut vm, "mode", "options"), eval(&mut vm, "knob-modes"));
    // A field with nothing declared: no range or options, not settable.
    assert_eq!(info(&mut vm, "name", "range"), Value::Nil);
    assert_eq!(info(&mut vm, "name", "settable"), Value::Bool(false));
    // Anything but an instance field binding has no info.
    assert_eq!(eval(&mut vm, "(field-info 3)"), Value::Nil);
    // (field h :f) reads as h.f; (set-field! h :f v) writes as (set! h.f v),
    // a :host field through its :set.
    let id = vm.register_keyed_instance("dial", &[1]).expect("register");
    push(&mut vm, id, "level", Value::Number(0.5));
    assert_eq!(eval(&mut vm, "(field (dial 1) :level)"), Value::Number(0.5));
    eval(&mut vm, "(set-field! (dial 1) :level 1.5)");
    assert_eq!(eval(&mut vm, "knob-wrote"), Value::Number(1.5));
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
    // A numeric field's setter takes true/false too (an on/off value).
    eval(
        &mut vm,
        "(def-kind amp :key (index) :host ((value :number :set seq-set-mute)))",
    );
    let knob = vm.register_keyed_instance("amp", &[0]).expect("register");
    eval(&mut vm, "(let ((k (amp 0))) (set! k.value true))");
    assert_eq!(
        calls.borrow().last(),
        Some(&vec![Value::Instance(knob), Value::Bool(true)])
    );
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
            ":key expects () (a singleton), (index), (parent index) or ((parent …) index)",
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

#[test]
fn a_host_field_is_observed_while_a_reader_depends_on_it_or_a_binding_is_held() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    assert!(!vm.host_field_observed(id, "name"));
    eval(&mut vm, "(def t0 (track 0))");
    // An untracked read observes nothing.
    eval(&mut vm, "t0.name");
    assert!(!vm.host_field_observed(id, "name"));
    eval(&mut vm, r#"(effect-buffer "*name*" (label t0.name))"#);
    rendered_targets(&mut vm);
    assert!(vm.host_field_observed(id, "name"));
    assert!(!vm.host_field_observed(id, "volume"));
    // A held binding observes its field; letting go of it stops observing.
    eval(&mut vm, "(def held #'t0.volume)");
    assert!(vm.host_field_observed(id, "volume"));
    eval(&mut vm, "(set! held nil)");
    assert!(!vm.host_field_observed(id, "volume"));
    eval(&mut vm, "(def rgb #'t0.color)");
    assert!(vm.host_field_observed(id, "color"));
}

#[test]
fn a_read_of_an_unobserved_host_field_asks_the_hosts_reader() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = asked.clone();
    vm.set_host_field_reader(Some(Rc::new(move |vm: &mut VM, read: u64, field: &str| {
        assert!(vm.instance_is_live(read));
        log.borrow_mut().push(field.to_string());
        (field == "volume").then_some(Value::Number(0.75))
    })));
    eval(&mut vm, "(def t0 (track 0))");
    // A cold read gets the host's value, and the cell keeps it.
    assert_eq!(eval(&mut vm, "t0.volume"), Value::Number(0.75));
    assert_eq!(vm.instance_field(id, "volume"), Ok(Value::Number(0.75)));
    // `None` keeps the cell; :state fields never ask.
    push(&mut vm, id, "name", Value::String("Kick".into()));
    assert_eq!(eval(&mut vm, "t0.name"), Value::String("Kick".into()));
    eval(&mut vm, "t0.open");
    assert_eq!(*asked.borrow(), vec!["volume", "name"]);
    // A binding seeds its slot from the reader.
    let held = eval(&mut vm, "(def held #'t0.volume) held");
    let Value::ReactiveRef { slot, .. } = &held else {
        panic!("expected a ref, got {held:?}");
    };
    assert_eq!(read_float_slot(slot), 0.75);
    asked.borrow_mut().clear();
    // Observed fields are the host's to keep current: reads use the cell.
    assert_eq!(eval(&mut vm, "t0.volume"), Value::Number(0.75));
    assert!(asked.borrow().is_empty());
    // A value of the wrong type is an error, like any host push.
    vm.set_host_field_reader(Some(Rc::new(|_: &mut VM, _: u64, _: &str| {
        Some(Value::String("loud".into()))
    })));
    assert!(error(&mut vm, "t0.peak").contains(":number"));
}

#[test]
fn reserved_kind_names_belong_to_their_module() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    vm.reserve_kind_names("eseq.kinds", &["track"]);
    assert!(
        error(
            &mut vm,
            "(def-kind track :key (index) :host ((name :string)))"
        )
        .contains("kind name 'track' is reserved for the host kinds of eseq.kinds")
    );
    eval(&mut vm, "(def-kind tracker :key () :state ((open false)))");
    eval(
        &mut vm,
        "(module eseq.kinds) (def-kind track :key (index) :host ((name :string)))",
    );
    assert!(vm.instance_kind_schema("eseq.kinds:track").is_some());
}

/// A kind with more than 32 `:host` fields: the observed mask is a `u64`
/// (eseq-0l17.71), so bits past 31 come back like the low ones, and asking
/// about more than [`MAX_OBSERVED_FIELDS`] fields is a clear panic.
#[test]
fn observed_bits_cover_more_than_32_fields() {
    let (mut vm, _) = vm();
    let names: Vec<String> = (0..40).map(|i| format!("f{i}")).collect();
    let host = names
        .iter()
        .map(|name| format!("({name} :number)"))
        .collect::<Vec<_>>()
        .join(" ");
    eval(
        &mut vm,
        &format!("(def-kind wide :key (index) :host ({host}))"),
    );
    let id = vm.register_keyed_instance("wide", &[0]).expect("register");
    eval(&mut vm, "(def w0 (wide 0))");
    eval(
        &mut vm,
        r#"(effect-buffer "*wide*" (label (+ w0.f2 w0.f35)))"#,
    );
    rendered_targets(&mut vm);
    eval(&mut vm, "(def held #'w0.f39)");
    let fields: Vec<&str> = names.iter().map(String::as_str).collect();
    let expected: super::ObservedMask = (1 << 2) | (1 << 35) | (1 << 39);
    assert_eq!(vm.host_fields_observed(id, &fields), expected);
    assert!(vm.host_field_observed(id, "f39"));
    assert!(!vm.host_field_observed(id, "f38"));
    let too_many: Vec<&str> = std::iter::repeat_n("f0", super::MAX_OBSERVED_FIELDS + 1).collect();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.host_fields_observed(id, &too_many)
    }))
    .expect_err("more than MAX_OBSERVED_FIELDS fields panics");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("at most 64 fields"), "{message}");
}

#[test]
fn observed_bits_batch_children_by_kind_and_the_schema_generation() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    eval(&mut vm, "(def t0 (track 0))");
    eval(&mut vm, r#"(effect-buffer "*name*" (label t0.name))"#);
    rendered_targets(&mut vm);
    eval(&mut vm, "(def held #'t0.volume) (def rgb #'t0.color)");
    let fields = ["name", "volume", "peak", "color", "steps"];
    assert_eq!(vm.host_fields_observed(id, &fields), 0b01011);
    for (bit, field) in fields.iter().enumerate() {
        assert_eq!(
            vm.host_field_observed(id, field),
            0b01011 & (1 << bit) != 0,
            "{field}"
        );
    }
    // Children of one kind, with their keys.
    eval(
        &mut vm,
        "(def-kind step :key (track index) :host ((active :bool)))
         (def-kind clip :key (track index) :host ((name :string)))",
    );
    let step = vm.register_keyed_instance("step", &[id, 3]).expect("step");
    vm.register_keyed_instance("clip", &[id, 0]).expect("clip");
    let step_kind = vm.instance_kind(step).expect("kind id").to_string();
    let steps: Vec<(u64, Vec<u64>)> = vm
        .keyed_children_of_kind(id, &step_kind)
        .map(|(child, key)| (child, key.to_vec()))
        .collect();
    assert_eq!(steps, vec![(step, vec![id, 3])]);
    // Observers of children: a new binding moves the observer epoch.
    assert!(!vm.keyed_children_observed(id, &step_kind, &["active"]));
    let epoch = vm.instance_observer_epoch();
    push(
        &mut vm,
        id,
        "steps",
        Value::List(vec![Rc::new(RefCell::new(Value::Instance(step)))]),
    );
    eval(&mut vm, "(def s3 (first t0.steps))");
    eval(&mut vm, "(def held-step #'s3.active)");
    assert!(vm.instance_observer_epoch() > epoch);
    assert!(vm.keyed_children_observed(id, &step_kind, &["active"]));
    // Re-registering the same declaration (a module re-evaluated by a later
    // import pass: fresh setter closures) moves no schema generation
    // (eseq-0l17.59); a changed declaration moves it.
    let generation = vm.instance_kind_schema_generation();
    eval(&mut vm, TRACK);
    assert_eq!(vm.instance_kind_schema_generation(), generation);
    let changed = TRACK.replace("(open false)", "(open false) (pinned false)");
    eval(&mut vm, &changed);
    assert!(vm.instance_kind_schema_generation() > generation);
    let generation = vm.instance_kind_schema_generation();
    let unset = TRACK.replace(":range (0 1) :set set-volume", ":range (0 1)");
    eval(
        &mut vm,
        &unset.replace("(open false)", "(open false) (pinned false)"),
    );
    assert!(
        vm.instance_kind_schema_generation() > generation,
        "a field losing its setter is a change"
    );
}

/// Every `__stable-key` in a widget tree, depth first.
fn stable_keys(tree: &Value, out: &mut Vec<String>) {
    match tree {
        Value::Map(map) => {
            if let Some(key) = map.get(super::super::STABLE_KEY_PROP)
                && let Value::String(key) = &*key.borrow()
            {
                out.push(key.clone());
            }
            for value in map.values() {
                stable_keys(&value.borrow(), out);
            }
        }
        Value::List(items) => {
            for item in items {
                stable_keys(&item.borrow(), out);
            }
        }
        _ => {}
    }
}

#[test]
fn subtree_keys_take_instances_and_lists_of_parts() {
    let (mut vm, _) = vm();
    let t0 = vm.register_keyed_instance("track", &[0]).expect("t0");
    let t1 = vm.register_keyed_instance("track", &[1]).expect("t1");
    let key = |vm: &mut VM, code: &str| super::super::subtree_key_string(&eval(vm, code));
    assert_eq!(key(&mut vm, "(track 0)"), Some(format!("#<{t0}>")));
    assert_eq!(
        key(&mut vm, "(list :preset (track 1) 2)"),
        Some(format!(":preset/#<{t1}>/2"))
    );
    assert_eq!(
        key(&mut vm, "(list :preset (dict))"),
        None,
        "every part must key"
    );
    eval(
        &mut vm,
        r#"
        (effect-buffer "*keys*"
          (v-stack
            (subtree :key (track 0) (label "a"))
            (subtree :key (list :preset (track 1)) (label "b"))))
        "#,
    );
    let mut keys = Vec::new();
    for update in vm.pending_widget_trees.drain(..) {
        if let PendingUiUpdate::FullTree(tree) = update {
            stable_keys(&tree.tree, &mut keys);
        }
    }
    assert!(
        keys.iter().any(|k| k.ends_with(&format!("#<{t0}>"))),
        "{keys:?}"
    );
    assert!(
        keys.iter()
            .any(|k| k.ends_with(&format!(":preset/#<{t1}>"))),
        "{keys:?}"
    );
}

/// eseq-0l17.60: a field reusing a built-in field (`key` on a keyed kind)
/// is a compile error naming the field and the kind, for every key shape.
#[test]
fn a_field_named_like_a_built_in_field_is_a_compile_error() {
    let (mut vm, _) = vm();
    for (code, expected) in [
        (
            "(def-kind gen-mark :key (index) :host ((key :number)))",
            "def-kind gen-mark: :host field 'key' is a built-in field of a keyed kind \
             (id, kind, key); rename it",
        ),
        (
            "(def-kind gen-mark :key (index) :state ((kind 0)))",
            "def-kind gen-mark: :state field 'kind' is a built-in field of a keyed kind",
        ),
        (
            "(def-kind panel :key () :state ((id 0)))",
            "def-kind panel: :state field 'id' is a built-in field of a singleton (:key ()) \
             (id, kind)",
        ),
        (
            "(def-kind thing :state ((owner 0)))",
            "def-kind thing: :state field 'owner' is a built-in field of a created kind \
             (id, kind, owner, label)",
        ),
        (
            "(def-kind thing :document ((label \"x\")))",
            "def-kind thing: :document field 'label' is a built-in field",
        ),
    ] {
        let errors = compile_errors(&mut vm, code);
        assert!(errors.contains(expected), "{code}: {errors}");
    }
    // Only a keyed kind has `key`, and only a created kind `owner`.
    eval(&mut vm, "(def-kind panel :key () :state ((key 0) (owner 1)))");
    assert_eq!(eval(&mut vm, "panel.key"), Value::Number(0.0));
}

/// eseq-0l17.60: the field above broke a whole module load with nothing
/// to show for it. The import still fails (its string value and the load
/// error queue name the module), and the queue now carries the compile
/// error naming the field, which is also logged.
#[test]
fn a_module_whose_def_kind_reuses_a_built_in_field_reports_the_field() {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    let dir = std::env::temp_dir().join(format!(
        "eseqlisp-builtin-field-import-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("module dir");
    std::fs::write(
        dir.join("kbtest.bad-kinds.lisp"),
        "(module kbtest.bad-kinds)\n\
         (def-kind gen :key (index) :host ((name :string)))\n\
         (def-kind gen-mark :key (gen index) :host ((key :number) (value :number)))",
    )
    .expect("write module");
    vm.source_manager.set_module_load_roots(vec![dir.clone()]);
    let value = vm.eval_str("(import kbtest.bad-kinds)").expect("import form");
    let errors = vm.take_source_load_errors().join("\n");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        matches!(&value, Some(Value::String(message))
            if message.contains("import kbtest.bad-kinds")),
        "{value:?}"
    );
    assert!(
        errors.contains("def-kind gen-mark: :host field 'key' is a built-in field of a keyed kind"),
        "{errors}"
    );
    assert!(vm.instance_kind_schema("kbtest.bad-kinds:gen").is_none());
}

/// eseq-0l17.24: an index-keyed kind named like a widget got widget source
/// props on its constructor calls, so `(knob 0)` failed with "takes one
/// non-negative integer". A singleton or index-keyed kind (both bind their
/// name) may not take a widget's name; a parent-keyed kind binds nothing.
#[test]
fn a_kind_that_binds_a_widget_name_is_a_compile_error() {
    let (mut vm, _) = vm();
    for (code, name) in [
        ("(def-kind knob :key (index) :host ((value :number)))", "knob"),
        ("(def-kind label :key () :state ((open false)))", "label"),
    ] {
        let errors = compile_errors(&mut vm, code);
        assert!(
            errors.contains(&format!(
                "def-kind {name}: '{name}' is a built-in widget; a :key () or :key (index) \
                 kind binds its name"
            )),
            "{code}: {errors}"
        );
    }
    eval(&mut vm, "(def-kind knob :key (track index) :host ((value :number)))");
    assert!(vm.instance_kind_schema("scratch:knob").is_some());
}

/// eseq-0l17.62: a `:key` of names with no `:host` is view-local state Lisp
/// owns. The constructor answers the instance under its arguments, creating
/// it on first call; equal arguments answer the same instance, and each
/// instance's fields dirty only their own readers.
#[test]
fn a_view_local_kind_creates_one_instance_per_key() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        "(def-kind adsr-gesture :key (scope section) :state ((attack false) (decay false)))",
    );
    let core = eval(&mut vm, r#"(adsr-gesture "core" -1)"#);
    let Value::Instance(core_id) = core else {
        panic!("expected an instance, got {core:?}");
    };
    assert_eq!(eval(&mut vm, r#"(adsr-gesture "core" -1)"#), core);
    assert_eq!(
        eval(
            &mut vm,
            r#"(= (adsr-gesture "core" -1) (adsr-gesture "core" 0))"#
        ),
        Value::Bool(false)
    );
    assert_eq!(
        eval(
            &mut vm,
            r#"(= (adsr-gesture "core" 0) (adsr-gesture "core" -0))"#
        ),
        Value::Bool(true)
    );
    assert_eq!(
        eval(&mut vm, r#"(str (adsr-gesture "core" -1))"#),
        Value::String(format!("<adsr-gesture#{core_id} [\"core\" -1]>"))
    );
    assert_eq!(
        eval(
            &mut vm,
            r#"(let ((g (adsr-gesture "core" -1))) (str g.key))"#
        ),
        Value::String("(\"core\" -1)".into())
    );
    assert!(core_id >= KEYED_INSTANCE_ID_BASE);
    eval(
        &mut vm,
        r#"(def flag (scope) (let ((g (adsr-gesture scope -1))) (if g.attack "a" "-")))
           (effect-buffer "*a*" (label (flag "core")))
           (effect-buffer "*b*" (label (flag "triton")))"#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*a*", "*b*"]);
    eval(
        &mut vm,
        r#"(let ((g (adsr-gesture "triton" -1))) (set! g.attack true))"#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*b*"]);
    assert_eq!(eval(&mut vm, r#"(flag "core")"#), Value::String("-".into()));
    assert_eq!(vm.local_instances("adsr-gesture").len(), 3);
    // Keyword, symbol and boolean parts key too; a binding works as usual.
    eval(&mut vm, "(def g (adsr-gesture :env 'amp))");
    eval(&mut vm, "(def held #'g.decay)");
    eval(&mut vm, "(set! g.decay true)");
    assert_eq!(
        eval(&mut vm, "(let ((g (adsr-gesture :env 'amp))) g.decay)"),
        Value::Bool(true)
    );
    // Re-evaluating the def-kind keeps every instance and its values.
    eval(
        &mut vm,
        "(def-kind adsr-gesture :key (scope section) :state ((attack false) (decay false)))",
    );
    assert_eq!(eval(&mut vm, r#"(adsr-gesture "core" -1)"#), core);
    assert_eq!(
        eval(&mut vm, "(let ((g (adsr-gesture :env 'amp))) g.decay)"),
        Value::Bool(true)
    );
}

/// `(drop-instance g)` drops a view-local instance: its handle turns stale,
/// readers of its constructor re-run, and the next call creates a fresh one.
#[test]
fn drop_instance_drops_a_view_local_instance_and_its_readers_re_run() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        "(def-kind row-ui :key (slot) :state ((open false)))",
    );
    eval(&mut vm, "(def r (row-ui 3))");
    eval(&mut vm, "(set! r.open true)");
    // A reader that only holds the instance (a binding, no by-value read)
    // still re-runs when its key's instance goes.
    eval(
        &mut vm,
        r#"(effect-buffer "*row*" (let ((u (row-ui 3))) (do (def bound #'u.open) (label "x"))))"#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*row*"]);
    assert_eq!(eval(&mut vm, "(drop-instance r)"), Value::Bool(true));
    assert_eq!(rendered_targets(&mut vm), vec!["*row*"]);
    assert_eq!(eval(&mut vm, "r.open"), Value::Bool(false), "stale handle");
    assert_eq!(eval(&mut vm, "(drop-instance r)"), Value::Bool(false));
    assert_eq!(eval(&mut vm, "(= r (row-ui 3))"), Value::Bool(false));
    assert_eq!(
        eval(&mut vm, "(let ((u (row-ui 3))) u.open)"),
        Value::Bool(false),
        "fresh defaults"
    );
    // Host and singleton instances are not Lisp's to drop.
    vm.register_keyed_instance("track", &[0]).expect("register");
    assert!(error(&mut vm, "(drop-instance (track 0))").contains("view-local kind"));
    assert!(error(&mut vm, "(drop-instance log)").contains("view-local kind"));
    assert!(error(&mut vm, "(drop-instance 3)").contains("view-local kind"));
}

#[test]
fn view_local_keys_are_checked_and_the_host_cannot_create_them() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        "(def-kind pick :key (scope section) :state ((on false)))",
    );
    let message = error(&mut vm, r#"(pick "a")"#);
    assert!(
        message.contains("(pick scope section) takes 2 key values"),
        "{message}"
    );
    let message = error(&mut vm, r#"(pick "a" (list 1))"#);
    assert!(message.contains("takes 2 key values"), "{message}");
    assert!(error(&mut vm, r#"(pick "a" nil)"#).contains("takes 2 key values"));
    assert_eq!(
        vm.register_keyed_instance("pick", &[1, 2])
            .unwrap_err()
            .to_string(),
        "pick instances are view-local; create them with (pick key …)"
    );
    assert_eq!(
        vm.create_instance(7, "scratch:pick")
            .unwrap_err()
            .to_string(),
        "pick instances are view-local; create them with (pick key …)"
    );
    // Any number of names; the key's length is fixed until restart.
    eval(&mut vm, "(def-kind cell-ui :key (a b c) :state ((n 0)))");
    eval(&mut vm, "(cell-ui 1 2 3)");
    assert!(error(&mut vm, "(def-kind cell-ui :key (a b) :state ((n 0)))").contains("restart"));
    let errors = compile_errors(&mut vm, "(def-kind bad :key ((a b) c) :state ((n 0)))");
    assert!(
        errors.contains("a view-local :key (no :host) is a list of names"),
        "{errors}"
    );
    let errors = compile_errors(&mut vm, "(def-kind bad :key (a a) :state ((n 0)))");
    assert!(errors.contains(":key names a twice"), "{errors}");
    let errors = compile_errors(&mut vm, "(def-kind knob :key (a) :state ((n 0)))");
    assert!(errors.contains("'knob' is a built-in widget"), "{errors}");
    let errors = compile_errors(&mut vm, "(def-kind bad :key (a) :state ((key 0)))");
    assert!(
        errors.contains("built-in field of a keyed kind"),
        "{errors}"
    );
}

/// A key part that is an instance makes the view-local instance its child:
/// dropping the parent drops it, and a stale parent answers nil.
#[test]
fn a_view_local_instance_keyed_by_an_instance_goes_with_it() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        "(def-kind track-ui :key (track row) :state ((open false)))",
    );
    let track = vm.register_keyed_instance("track", &[0]).expect("register");
    eval(&mut vm, "(def t0 (track 0))");
    let Value::Instance(ui) = eval(&mut vm, "(track-ui t0 2)") else {
        panic!("expected an instance");
    };
    assert!(vm.instance_is_live(ui));
    assert!(vm.keyed_children(track).contains(&ui));
    assert!(vm.drop_instance(track));
    assert!(!vm.instance_is_live(ui));
    assert_eq!(eval(&mut vm, "(track-ui t0 2)"), Value::Nil);
    assert!(vm.local_instances("track-ui").is_empty());
}

#[test]
fn a_view_local_instance_keyed_by_multiple_instances_goes_with_any_parent() {
    for dropped in ["a", "b", "c"] {
        let (mut vm, _) = vm();
        eval(
            &mut vm,
            r#"(def-kind owner-ui :key (name) :state ((open false)))
               (def-kind composite-ui :key (scope left row middle right)
                 :state ((open false)))
               (def-kind child-ui :key (parent) :state ((open false)))
               (def a (owner-ui "a"))
               (def b (owner-ui "b"))
               (def c (owner-ui "c"))
               (def u (composite-ui "core" a 2 b c))
               (def child (child-ui u))
               (set! u.open true)
               (set! child.open true)
               (def bound #'u.open)"#,
        );
        let parents: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| match eval(&mut vm, name) {
                Value::Instance(id) => id,
                _ => panic!("expected an instance"),
            })
            .collect();
        let Value::Instance(ui) = eval(&mut vm, "u") else {
            panic!("expected an instance");
        };
        let Value::Instance(child) = eval(&mut vm, "child") else {
            panic!("expected an instance");
        };
        for parent in &parents {
            assert_eq!(vm.keyed_children(*parent), vec![ui]);
        }
        eval(
            &mut vm,
            r#"(effect-buffer "*composite*"
                 (label (if (composite-ui "core" a 2 b c) "live" "stale")))"#,
        );
        assert_eq!(rendered_targets(&mut vm), vec!["*composite*"]);
        assert_eq!(eval(&mut vm, "(reactive-value bound)"), Value::Bool(true));

        assert_eq!(
            eval(&mut vm, &format!("(drop-instance {dropped})")),
            Value::Bool(true)
        );
        assert!(!vm.instance_is_live(ui), "dropped parent {dropped}");
        assert!(!vm.instance_is_live(child), "descendant must go too");
        assert!(vm.local_instances("composite-ui").is_empty());
        assert!(vm.local_instances("child-ui").is_empty());
        for parent in &parents {
            assert!(vm.keyed_children(*parent).is_empty());
        }
        assert_eq!(rendered_targets(&mut vm), vec!["*composite*"]);
        assert_eq!(eval(&mut vm, "u.open"), Value::Bool(false));
        assert_eq!(eval(&mut vm, "child.open"), Value::Bool(false));
        assert_eq!(eval(&mut vm, "(reactive-value bound)"), Value::Bool(false));
        assert_eq!(eval(&mut vm, r#"(composite-ui "core" a 2 b c)"#), Value::Nil);
        assert_eq!(eval(&mut vm, "(child-ui u)"), Value::Nil);
    }
}

#[test]
fn dropping_a_view_local_composite_instance_detaches_it_from_all_parents() {
    let (mut vm, _) = vm();
    eval(
        &mut vm,
        r#"(def-kind owner-ui :key (name) :state ((open false)))
           (def-kind composite-ui :key (left right repeated) :state ((open false)))
           (def a (owner-ui "a"))
           (def b (owner-ui "b"))
           (def u (composite-ui a b a))"#,
    );
    let Value::Instance(a) = eval(&mut vm, "a") else {
        panic!("expected an instance");
    };
    let Value::Instance(b) = eval(&mut vm, "b") else {
        panic!("expected an instance");
    };
    let Value::Instance(ui) = eval(&mut vm, "u") else {
        panic!("expected an instance");
    };
    assert_eq!(vm.keyed_children(a), vec![ui], "repeated parents are deduplicated");
    assert_eq!(vm.keyed_children(b), vec![ui]);
    assert_eq!(eval(&mut vm, "(drop-instance u)"), Value::Bool(true));
    assert!(vm.keyed_children(a).is_empty());
    assert!(vm.keyed_children(b).is_empty());
    let Value::Instance(fresh) = eval(&mut vm, "(composite-ui a b a)") else {
        panic!("expected a fresh instance");
    };
    assert_ne!(fresh, ui);
    assert_eq!(vm.keyed_children(a), vec![fresh]);
    assert_eq!(vm.keyed_children(b), vec![fresh]);
    assert_eq!(eval(&mut vm, "(drop-instance b)"), Value::Bool(true));
    assert!(!vm.instance_is_live(fresh));
    assert!(vm.keyed_children(a).is_empty());
    assert!(vm.local_instances("composite-ui").is_empty());
    assert_eq!(eval(&mut vm, "(composite-ui a b a)"), Value::Nil);
}

/// eseq-0l17.23: `(describe-kind 'k)` lists every field with its group,
/// type and options.
#[test]
fn describe_kind_lists_every_field_with_its_options() {
    let (mut vm, _) = vm();
    let Value::String(text) = eval(&mut vm, "(describe-kind 'track)") else {
        panic!("describe-kind returns text");
    };
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "kind scratch:track: keyed (:key (index)); built-in fields: id, kind, key"
    );
    let line = |name: &str| {
        lines
            .iter()
            .find(|line| line.split_whitespace().nth(1) == Some(name))
            .unwrap_or_else(|| panic!("no line for {name}: {text}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(line("name"), r#":host name :string :doc "Track name""#);
    assert_eq!(
        line("volume"),
        ":host volume :number :set set-volume :range (0 1)"
    );
    assert_eq!(line("muted"), ":host muted :bool :set seq-set-mute");
    assert_eq!(line("steps"), ":host steps (list-of step)");
    assert_eq!(line("open"), ":state open :bool :default false");
    assert_eq!(lines.len(), 8, "{text}");
    // A singleton, a view-local kind; unknown names are errors.
    let Value::String(text) = eval(&mut vm, "(describe-kind \"log\")") else {
        panic!("text");
    };
    assert!(
        text.starts_with("kind scratch:log: as a singleton (:key ()); built-in fields: id, kind")
    );
    eval(
        &mut vm,
        "(def-kind pick :key (scope section) :state ((on false)))",
    );
    let Value::String(text) = eval(&mut vm, "(describe-kind 'pick)") else {
        panic!("text");
    };
    assert!(
        text.starts_with("kind scratch:pick: view-local (:key (scope section))"),
        "{text}"
    );
    assert!(error(&mut vm, "(describe-kind 'nope)").contains("no kind named 'nope'"));
}

/// eseq-0l17.23: with the re-render log on, a by-value field read in a
/// view logs the field and the function that read it each time a change of
/// that field re-renders the view; a binding (`#'`) logs nothing.
#[test]
fn the_rerender_log_names_the_field_and_the_reader() {
    let (mut vm, _) = vm();
    let id = vm.register_keyed_instance("track", &[0]).expect("register");
    // On before the view renders: a read is located when it happens.
    assert_eq!(eval(&mut vm, "(rerender-log! true)"), Value::Bool(true));
    eval(
        &mut vm,
        r#"(def t0 (track 0))
           (def row-label (t) (label t.name))
           (effect-buffer "*row*"
             (v-stack (subtree :key :row-name (row-label t0))
                      (hslider :value #'t0.volume)))"#,
    );
    rendered_targets(&mut vm);
    eval(&mut vm, "(rerender-reasons)");
    push(&mut vm, id, "name", Value::String("Kick".into()));
    push(&mut vm, id, "volume", Value::Number(0.5));
    let reasons = eval(&mut vm, "(rerender-reasons)");
    let Value::List(lines) = reasons else {
        panic!("a list");
    };
    let lines: Vec<String> = lines
        .iter()
        .map(|line| match &*line.borrow() {
            Value::String(line) => line.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        lines,
        vec![format!(
            "[rerender] subtree :row-name (*row*): <track#{id} [0]>.name read in row-label"
        )]
    );
    assert_eq!(
        eval(&mut vm, "(rerender-reasons)"),
        Value::List(Vec::new()),
        "cleared"
    );
    eval(&mut vm, "(rerender-log! false)");
    push(&mut vm, id, "name", Value::String("Snare".into()));
    assert_eq!(
        eval(&mut vm, "(rerender-reasons)"),
        Value::List(Vec::new()),
        "off"
    );
}
