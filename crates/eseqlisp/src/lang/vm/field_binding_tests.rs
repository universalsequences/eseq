//! `#'` field bindings and self-reading refs (docs/kind-bindings-spec.md
//! §7.1, §8; stage 2).

use super::super::{BindingKind, EffectTarget, PendingUiUpdate, VM, VMError, Value};
use super::{INSTANCE_NAMESPACE_PREFIX, InstanceKindSchema, SINGLETON_INSTANCE_ID_BASE};
use crate::reactive::read_float_slot;
use crate::runtime::Runtime;

const KIND: &str = "test/pkg:probe";
const K: &str = "(def-kind k :key () :state ((flag false) (level 0.5) (name \"n\") \
                 (tint :rgb :default (rgb 1 0 0))))";

fn vm() -> VM {
    let mut vm = VM::new(Vec::new());
    super::super::register_core_natives(&mut vm);
    super::super::register_math_natives(&mut vm);
    crate::widgets::register_widget_natives(&mut vm);
    vm.register_instance_kind(
        InstanceKindSchema::new(KIND)
            .field("x", Value::Number(1.0))
            .field("label-text", Value::String("t".into())),
    )
    .expect("register kind");
    vm.eval_str(K).expect("def-kind k");
    vm
}

fn eval(vm: &mut VM, code: &str) -> Value {
    vm.eval_str(code)
        .unwrap_or_else(|e| panic!("{code}: {e:?}"))
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

/// The float a ref's slot holds now.
fn slot_value(value: &Value) -> f64 {
    let Value::ReactiveRef { slot, .. } = value else {
        panic!("expected a ref, got {value:?}");
    };
    read_float_slot(slot)
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

#[test]
fn hash_quote_on_a_singleton_field_returns_a_ref_over_its_slot() {
    let mut vm = vm();
    let flag = eval(&mut vm, "#'k.flag");
    let Value::ReactiveRef {
        namespace,
        field,
        index,
        kind,
        ..
    } = &flag
    else {
        panic!("expected a ref, got {flag:?}");
    };
    assert_eq!(
        namespace,
        &format!("{INSTANCE_NAMESPACE_PREFIX}{SINGLETON_INSTANCE_ID_BASE}")
    );
    assert_eq!(
        (field.as_str(), *index, *kind),
        (
            "flag",
            None,
            BindingKind::InstanceFloat(SINGLETON_INSTANCE_ID_BASE)
        )
    );
    assert_eq!(slot_value(&flag), 0.0);
    // Seeded from the current value, then written on every change.
    let level = eval(&mut vm, "#'k.level");
    assert_eq!(slot_value(&level), 0.5);
    eval(&mut vm, "(set! k.flag true)");
    eval(&mut vm, "(set! k.level 0.8)");
    assert_eq!((slot_value(&flag), slot_value(&level)), (1.0, 0.8));
    // A path through a value: the head and inner steps are by-value reads.
    eval(&mut vm, "(def holder (dict :menu k))");
    assert_eq!(slot_value(&eval(&mut vm, "#'holder.menu.level")), 0.8);
    // `str` formats the value, like any native that is not ref-aware.
    assert_eq!(
        eval(&mut vm, "(str \"Vol \" #'k.level \" \" #'k.flag)"),
        Value::String("Vol 0.8 true".into())
    );
}

#[test]
fn a_ref_in_a_value_position_reads_itself() {
    let mut vm = vm();
    let branch = "(if #'k.flag \"on\" \"off\")";
    assert_eq!(eval(&mut vm, branch), Value::String("off".into()));
    assert_eq!(eval(&mut vm, "(not #'k.flag)"), Value::Bool(true));
    eval(&mut vm, "(set! k.flag true)");
    assert_eq!(eval(&mut vm, branch), Value::String("on".into()));
    assert_eq!(
        eval(&mut vm, "(if (and #'k.flag #'k.flag) 1 2)"),
        Value::Number(1.0)
    );
    // A bool binding reads as the bool, like `k.flag`.
    assert_eq!(eval(&mut vm, "(= #'k.flag true)"), Value::Bool(true));
    assert_eq!(eval(&mut vm, "(= #'k.level 0.5)"), Value::Bool(true));
    assert_eq!(eval(&mut vm, "(= #'k.level #'k.level)"), Value::Bool(true));
    assert_eq!(eval(&mut vm, "(< #'k.level 1)"), Value::Bool(true));
    assert_eq!(eval(&mut vm, "(>= 0.25 #'k.level)"), Value::Bool(false));
    assert_eq!(eval(&mut vm, "(+ #'k.level 1)"), Value::Number(1.5));
    assert_eq!(eval(&mut vm, "(- 1 #'k.level)"), Value::Number(0.5));
    assert_eq!(eval(&mut vm, "(* #'k.level 4)"), Value::Number(2.0));
    assert_eq!(eval(&mut vm, "(/ #'k.level 2)"), Value::Number(0.25));
    assert_eq!(eval(&mut vm, "(min #'k.level 0.2)"), Value::Number(0.2));
    assert_eq!(eval(&mut vm, "(max #'k.level 0.2)"), Value::Number(0.5));
    // Natives receive the value at the call boundary, both through the
    // owned route and the borrowing fast path, and through `apply`-style
    // invocation.
    eval(&mut vm, "(set! k.level -0.5)");
    assert_eq!(eval(&mut vm, "(abs #'k.level)"), Value::Number(0.5));
    assert_eq!(eval(&mut vm, "(number? #'k.level)"), Value::Bool(true));
    assert_eq!(
        eval(&mut vm, "(fmt \"{}\" #'k.level)"),
        Value::String("-0.5".into())
    );
    assert_eq!(
        eval(&mut vm, "(map abs (list #'k.level))"),
        eval(&mut vm, "(list 0.5)")
    );
    // A filter predicate returning a ref is read too.
    assert_eq!(
        eval(&mut vm, "(len (filter (lambda (x) #'k.flag) (list 1 2)))"),
        Value::Number(2.0)
    );
    // Arithmetic on a bool binding is the same type error as on `k.flag`.
    assert_eq!(vm.eval_str("(+ #'k.flag 1)"), Err(VMError::IncorrectType));
}

#[test]
fn ref_aware_natives_keep_the_ref() {
    let mut vm = vm();
    let is_ref = |value: Value| matches!(value, Value::ReactiveRef { .. });
    assert!(is_ref(eval(&mut vm, "(first (list #'k.flag))")));
    assert!(is_ref(eval(&mut vm, "(get (dict :a #'k.flag) :a)")));
    assert!(is_ref(eval(&mut vm, "(first (cons #'k.flag (list)))")));
    assert!(is_ref(eval(&mut vm, "(get (merge (dict) :a #'k.flag) :a)")));
    // A closure passes a ref through untouched.
    assert!(is_ref(eval(&mut vm, "((lambda (r) r) #'k.flag)")));
    assert_eq!(
        eval(&mut vm, "(reactive-value #'k.level)"),
        Value::Number(0.5)
    );
}

#[test]
fn an_rgb_field_binds_three_slots_and_reads_as_rgb() {
    let mut vm = vm();
    let tint = eval(&mut vm, "#'k.tint");
    let Value::ReactiveRef {
        namespace, kind, ..
    } = &tint
    else {
        panic!("expected a ref");
    };
    assert_eq!(*kind, BindingKind::InstanceRgb(SINGLETON_INSTANCE_ID_BASE));
    let components = |vm: &VM| {
        vm.reactive_float_slots
            .rgb_slots(namespace, "tint")
            .map(|slot| read_float_slot(&slot))
    };
    assert_eq!(components(&vm), [1.0, 0.0, 0.0]);
    assert_eq!(slot_value(&tint), 1.0, "the ref's own slot is r");
    eval(&mut vm, "(set! k.tint (rgb 0 1 0.5))");
    assert_eq!(components(&vm), [0.0, 1.0, 0.5]);
    assert_eq!(
        eval(&mut vm, "(reactive-value #'k.tint)"),
        eval(&mut vm, "(rgb 0 1 0.5)")
    );
    assert_eq!(
        eval(&mut vm, "(= #'k.tint (rgb 0 1 0.5))"),
        Value::Bool(true)
    );
}

#[test]
fn reading_a_ref_in_a_render_records_a_dependency() {
    let mut vm = vm();
    eval(
        &mut vm,
        r#"
        (effect-buffer "*flag*" (label (if #'k.flag "on" "off")))
        (effect-buffer "*level*" (box :width (+ 1 #'k.level) :height 1))
        "#,
    );
    assert_eq!(rendered_targets(&mut vm), vec!["*flag*", "*level*"]);
    eval(&mut vm, "(set! k.flag true)");
    assert_eq!(rendered_targets(&mut vm), vec!["*flag*"]);
    eval(&mut vm, "(set! k.level 2)");
    assert_eq!(rendered_targets(&mut vm), vec!["*level*"]);
}

#[test]
fn unknown_and_value_only_fields_are_errors_naming_the_bindable_fields() {
    let mut vm = vm();
    assert_eq!(
        error(&mut vm, "#'k.nope"),
        "kind 'scratch:k' has no field 'nope'; bindable fields: flag, level, tint"
    );
    assert_eq!(
        error(&mut vm, "#'k.name"),
        "#': field 'name' of kind 'scratch:k' is :string, which is not bindable; \
         bindable fields: flag, level, tint"
    );
    assert!(
        error(&mut vm, "#'k.id").contains("field 'id' of kind 'scratch:k' is a built-in field")
    );
    let message = error(&mut vm, "(def d (dict :a 1)) #'d.a");
    assert!(
        message.contains("binds a field of an instance"),
        "{message}"
    );
    // A string head is not a reactive namespace: only the compiler knows those.
    let message = error(&mut vm, "(def s \"SEQ\") #'s.playing");
    assert!(
        message.contains("binds a field of an instance"),
        "{message}"
    );
}

#[test]
fn hash_quote_takes_only_a_field_path() {
    let mut vm = vm();
    for code in ["#'k", "#'(foo)", "#'1", "#'a/b.c", "#'k..flag"] {
        assert!(
            compile_errors(&mut vm, code).contains("#' takes a field path like t.volume"),
            "{code}"
        );
    }
}

#[test]
fn created_kind_fields_bind_and_a_drop_frees_the_slots() {
    let mut vm = vm();
    let handle = vm.create_instance(7, KIND).expect("create");
    vm.set_global_value("inst", handle);
    let x = eval(&mut vm, "#'inst.x");
    let namespace = format!("{INSTANCE_NAMESPACE_PREFIX}7");
    assert!(vm.reactive_float_slots.has_field(&namespace, "x"));
    vm.set_instance_field(7, "x", Value::Number(4.0))
        .expect("host write");
    assert_eq!(slot_value(&x), 4.0);
    assert_eq!(
        vm.take_pending_binding_repaints(),
        vec![(namespace.clone(), "x".to_string())]
    );
    // Writing the value it already has repaints nothing; neither does
    // binding the field again.
    vm.set_instance_field(7, "x", Value::Number(4.0))
        .expect("host write");
    eval(&mut vm, "#'inst.x");
    assert!(vm.take_pending_binding_repaints().is_empty());
    assert!(vm.drop_instance(7));
    assert_eq!(
        slot_value(&x),
        1.0,
        "a held binding reads the stale default"
    );
    assert!(!vm.reactive_float_slots.has_field(&namespace, "x"));
    // Unbound fields cost no slot and queue nothing.
    let other = vm.create_instance(8, KIND).expect("create");
    vm.set_global_value("other", other);
    eval(&mut vm, "(set! other.x 3)");
    assert!(
        !vm.reactive_float_slots
            .has_field(&format!("{INSTANCE_NAMESPACE_PREFIX}8"), "x")
    );
    vm.take_pending_binding_repaints();
    eval(&mut vm, "(set! other.x 5)");
    assert!(vm.take_pending_binding_repaints().is_empty());
}

#[test]
fn re_registering_a_kind_rewrites_bound_slots() {
    let mut vm = vm();
    let handle = vm.create_instance(7, KIND).expect("create");
    vm.set_global_value("inst", handle);
    let x = eval(&mut vm, "#'inst.x");
    eval(&mut vm, "(set! inst.x 3)");
    // `x` becomes a string field: the slot is dropped, the old ref keeps 3.
    vm.register_instance_kind(InstanceKindSchema::new(KIND).field("x", Value::String("s".into())))
        .expect("re-register");
    assert_eq!(slot_value(&x), 3.0);
    assert!(
        !vm.reactive_float_slots
            .has_field(&format!("{INSTANCE_NAMESPACE_PREFIX}7"), "x")
    );
}

// ---- runtime: widgets, repaint-only writes, legacy refs ---------------

fn runtime() -> Runtime {
    let mut runtime = Runtime::new();
    runtime.set_layout_viewport(40, 10);
    runtime
}

#[test]
fn a_field_binding_reaches_a_widget_prop_and_writes_only_repaint() {
    let mut runtime = runtime();
    runtime.eval_str(K).expect("def-kind");
    runtime
        .eval_str("(effect (label \"armed\" :active #'k.flag))")
        .expect("bound label effect");
    let layout = runtime.current_layout.as_ref().expect("label layout");
    assert_eq!(layout.widget_type, "label");
    let widget_id = layout.widget_id;
    assert!(matches!(
        layout.props.get("active"),
        Some(Value::ReactiveRef { field, .. }) if field == "flag"
    ));
    let _ = runtime.take_dirty_widget_ids();
    let _ = runtime.drain_rendered_layouts();

    runtime.eval_str("(set! k.flag true)").expect("write");
    assert_eq!(runtime.take_dirty_widget_ids(), vec![widget_id]);
    assert!(
        runtime.drain_rendered_layouts().is_empty(),
        "a binding-only write must not rerun the effect"
    );
    assert_eq!(
        runtime
            .current_layout
            .as_ref()
            .map(|layout| layout.widget_id),
        Some(widget_id)
    );
    let Some(Value::ReactiveRef { slot, .. }) = runtime
        .current_layout
        .as_ref()
        .and_then(|layout| layout.props.get("active"))
        .cloned()
    else {
        panic!("bound prop");
    };
    let slot = &slot;
    assert_eq!(read_float_slot(slot), 1.0);

    // A host write repaints the same way.
    runtime
        .set_instance_field(SINGLETON_INSTANCE_ID_BASE, "flag", Value::Bool(false))
        .expect("host write");
    assert_eq!(runtime.take_dirty_widget_ids(), vec![widget_id]);
    assert_eq!(read_float_slot(slot), 0.0);
    assert!(runtime.drain_rendered_layouts().is_empty());
}

#[test]
fn reactive_namespace_paths_build_the_legacy_ref() {
    let mut runtime = runtime();
    runtime.register_reactive("SEQ", vec![("playing", Value::Bool(false))], true);
    for code in ["#'SEQ.playing", "(bind-seq \"playing\")"] {
        let value = runtime.eval_str(code).expect("legacy ref");
        assert!(
            matches!(
                &value,
                Some(Value::ReactiveRef { namespace, field, index: None, kind: BindingKind::Float, .. })
                    if namespace == "SEQ" && field == "playing"
            ),
            "{code}: {value:?}"
        );
    }
    assert_eq!(runtime.eval_str("#'SEQ.a.b"), Err(VMError::CompileError));
}

#[test]
fn a_legacy_ref_in_a_condition_follows_its_value() {
    let mut runtime = runtime();
    runtime.register_reactive("SEQ", vec![("playing", Value::Bool(false))], true);
    let branch = "(if (bind-seq \"playing\") \"yes\" \"no\")";
    assert_eq!(
        runtime.eval_str(branch).unwrap(),
        Some(Value::String("no".into()))
    );
    runtime.set_reactive("SEQ", "playing", Value::Bool(true));
    runtime.run_reactive_cycle();
    assert_eq!(
        runtime.eval_str(branch).unwrap(),
        Some(Value::String("yes".into()))
    );
    assert_eq!(
        runtime.eval_str("(+ (bind-seq \"playing\") 1)").unwrap(),
        Some(Value::Number(2.0))
    );
}

#[test]
fn a_ref_held_in_a_local_reads_itself_as_a_get_field_target() {
    let mut vm = vm();
    // The target reads to its value first, so `.field` on a held number
    // binding is the same error as on the number itself.
    assert_eq!(
        vm.eval_str("(let ((r #'k.level)) r.x)"),
        vm.eval_str("(let ((v k.level)) v.x)")
    );
    assert_eq!(
        vm.eval_str("(let ((r #'k.level)) r.x)"),
        Err(VMError::IncorrectType)
    );
    // A legacy ref the same way.
    assert_eq!(
        vm.eval_str("(let ((r (bind \"SEQ\" \"x\"))) r.y)"),
        Err(VMError::IncorrectType)
    );
}

#[test]
fn a_ref_used_as_a_reactive_nth_index_reads_itself_and_records_its_dependency() {
    let mut runtime = runtime();
    runtime.register_reactive(
        "SEQ",
        vec![(
            "names",
            super::super::list_from_values(["a", "b", "c"].map(|name| Value::String(name.into()))),
        )],
        true,
    );
    runtime
        .eval_str("(def-kind k :key () :state ((i 1)))")
        .expect("def-kind");
    assert_eq!(
        runtime.eval_str("(nth SEQ.names #'k.i)").expect("nth"),
        Some(Value::String("b".into()))
    );
    runtime
        .eval_str("(effect (label (nth SEQ.names #'k.i)))")
        .expect("effect");
    let _ = runtime.drain_rendered_layouts();
    runtime.eval_str("(set! k.i 2)").expect("write");
    let layouts = runtime.drain_rendered_layouts();
    assert_eq!(layouts.len(), 1, "the index ref's field is a dependency");
    assert_eq!(
        runtime.eval_str("(nth SEQ.names #'k.i)").expect("nth"),
        Some(Value::String("c".into()))
    );
}
