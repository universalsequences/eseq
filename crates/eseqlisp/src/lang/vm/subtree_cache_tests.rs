use super::*;

#[test]
fn removed_descendants_release_their_render_cache() {
    let mut vm = VM::new(Vec::new());
    register_core_natives(&mut vm);
    vm.eval_str(r#"
        (def panel (generation)
          (subtree :key "parent"
            (subtree :key (str "child-" generation)
              (dict :type :label :text (str generation)))))
    "#).unwrap();
    let mut previous_text: Option<std::rc::Weak<RefCell<Value>>> = None;
    for generation in 0..100 {
        vm.eval_str(&format!("(panel {generation})")).unwrap();
        assert_eq!(vm.subtree_render_cache.len(), 2,
            "retired child renders must not accumulate after parent rerenders");
        assert!(vm.subtree_render_cache.keys()
            .all(|root| vm.dag.effect_id_for_subtree_root(*root).is_some()));
        if let Some(previous) = previous_text.take() {
            assert!(previous.upgrade().is_none(), "retired rendered cells must be released");
        }
        let cached = vm.subtree_render_cache.values().next().unwrap();
        let Value::Map(props) = &cached.value else { panic!("expected widget map") };
        previous_text = Some(Rc::downgrade(&props["text"]));
    }
}

// Pre-optimization comparator, retained only as a benchmark/semantic oracle.
/// Equality for cached subtree captured inputs. Deliberately stricter than
/// `Value::PartialEq`: callables compare by chunk index only there, which
/// would treat two closures over different data as equal, so any callable
/// (or otherwise opaque value) makes the inputs "changed" and forces a
/// fresh render.
fn subtree_input_value_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::NodeRef(x), Value::NodeRef(y)) => x == y,
        (Value::Instance(x), Value::Instance(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(a, b)| subtree_input_value_equal(&a.borrow(), &b.borrow()))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, a)| {
                    y.get(key)
                        .is_some_and(|b| subtree_input_value_equal(&a.borrow(), &b.borrow()))
                })
        }
        (
            Value::ReactiveRef {
                namespace: a_ns,
                field: a_field,
                index: a_index,
                kind: a_kind,
                ..
            },
            Value::ReactiveRef {
                namespace: b_ns,
                field: b_field,
                index: b_index,
                kind: b_kind,
                ..
            },
        ) => a_ns == b_ns && a_field == b_field && a_index == b_index && a_kind == b_kind,
        _ => false,
    }
}

fn cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(entries.into_iter().map(|(key, value)| (key.to_string(), cell(value))).collect())
}

#[test]
fn compact_inputs_preserve_exact_comparison_semantics() {
    let binding = |kind, index, bits| Value::ReactiveRef {
        namespace: "APP".to_string(), field: "value".to_string(), index, kind,
        slot: Arc::new(AtomicU64::new(bits)),
    };
    let values = vec![
        Value::Nil, Value::Bool(false), Value::Bool(true),
        Value::Number(0.0), Value::Number(-0.0), Value::Number(1.0),
        Value::Number(f64::NAN), Value::Number(f64::INFINITY),
        Value::String("µ音".to_string()), Value::String("other".to_string()),
        Value::Symbol("µ音".to_string()), Value::Keyword("µ音".to_string()),
        Value::NodeRef(1), Value::NodeRef(2), Value::Instance(1), Value::Instance(2),
        Value::List(vec![]), Value::List(vec![cell(Value::Number(1.0))]),
        map([]), map([("x", Value::Number(1.0))]), map([("y", Value::Number(1.0))]),
        map([("x", Value::Number(1.0)), ("y", Value::Bool(true))]),
        map([("y", Value::Bool(true)), ("x", Value::Number(1.0))]),
        binding(BindingKind::Float, None, 0),
        binding(BindingKind::Float, None, 123), // Slot contents do not identify a binding.
        binding(BindingKind::Float, Some(0), 0),
        binding(BindingKind::Float, Some(usize::MAX), 0),
        binding(BindingKind::InstanceFloat(1), None, 0),
        binding(BindingKind::InstanceFloat(2), None, 0),
        binding(BindingKind::InstanceRgb(1), None, 0),
        Value::Function(0), Value::Closure(0, vec![]),
        Value::OverrideDispatcher("fn".to_string()), Value::OverrideOriginal("fn".to_string()),
    ];
    for old in &values {
        let snapshot = SubtreeInputs::capture(&[cell(old.clone())]);
        for live in &values {
            assert_eq!(snapshot.as_ref().is_some_and(|s| s.matches(&[cell(live.clone())])),
                subtree_input_value_equal(live, old), "old={old:?}, live={live:?}");
        }
    }
    let empty = SubtreeInputs::capture(&[]).unwrap();
    assert!(empty.matches(&[]));
    assert!(!empty.matches(&[cell(Value::Nil)]));
}

#[test]
fn compact_inputs_detect_nested_mutation_without_retaining_live_cells() {
    let leaf = cell(Value::String("before".to_string()));
    let items = cell(Value::List(vec![leaf.clone(), leaf.clone()]));
    let input = cell(Value::Map(HashMap::from([("nested".to_string(), items.clone())])));
    let weak = Rc::downgrade(&input);
    let snapshot = SubtreeInputs::capture(&[input.clone()]).unwrap();
    assert_eq!(Rc::strong_count(&input), 1);
    assert_eq!(Rc::strong_count(&leaf), 3);
    assert!(snapshot.matches(&[input.clone()]));
    *leaf.borrow_mut() = Value::String("after".to_string());
    assert!(!snapshot.matches(&[input.clone()]));
    *leaf.borrow_mut() = Value::String("before".to_string());
    assert!(snapshot.matches(&[input.clone()]));
    *items.borrow_mut() = Value::List(vec![leaf]);
    assert!(!snapshot.matches(&[input.clone()]));
    drop(input);
    assert!(weak.upgrade().is_none(), "snapshots own comparison bytes, not the input graph");
}

#[test]
fn compact_inputs_decline_opaque_cyclic_and_borrowed_values() {
    let data = cell(Value::List(vec![cell(Value::Number(1.0)); 1024]));
    let callback = cell(Value::Closure(0, vec![data.clone()]));
    let input = cell(Value::List(vec![data.clone(), callback]));
    assert!(SubtreeInputs::capture(&[input]).is_none());
    assert_eq!(Rc::strong_count(&data), 1, "no partial snapshot retains callback captures");
    let cyclic = cell(Value::Nil);
    *cyclic.borrow_mut() = Value::List(vec![cyclic.clone()]);
    assert!(SubtreeInputs::capture(&[cyclic.clone()]).is_none());
    *cyclic.borrow_mut() = Value::Nil; // Release the fixture's own cycle.
    let snapshot = SubtreeInputs::capture(&[cyclic.clone()]).unwrap();
    let _borrow = cyclic.borrow_mut();
    assert!(!snapshot.matches(&[cyclic.clone()]));
    assert!(SubtreeInputs::capture(&[cyclic.clone()]).is_none());
}

#[test]
fn subtree_reuses_inputs_and_rerenders_after_an_in_place_edit() {
    let mut vm = VM::new(Vec::new());
    register_core_natives(&mut vm);
    let renders = Rc::new(std::cell::Cell::new(0));
    let count = renders.clone();
    vm.register_native("count-render", move |_| {
        count.set(count.get() + 1);
        Value::Nil
    });
    vm.eval_str(r#"
        (def make-panel (input)
          (lambda ()
            (count-render)
            (dict :type :label :text (get input :text))))
    "#).unwrap();
    let text = cell(Value::String("before".to_string()));
    let input = Value::Map(HashMap::from([("text".to_string(), text.clone())]));
    let callable = vm.invoke(vm.global_value("make-panel").unwrap(), vec![input]).unwrap().unwrap();
    vm.evaluate_subtree_owner("panel", callable.clone()).unwrap();
    vm.evaluate_subtree_owner("panel", callable.clone()).unwrap();
    assert_eq!(renders.get(), 1, "equal captured inputs must retain warm reuse");
    *text.borrow_mut() = Value::String("after".to_string());
    let updated = vm.evaluate_subtree_owner("panel", callable).unwrap();
    assert_eq!(renders.get(), 2);
    let Value::Map(props) = updated else { panic!("expected widget map") };
    assert_eq!(*props["text"].borrow(), Value::String("after".to_string()));
}

fn large_descriptor() -> Value {
    let params = (0..128).map(|index| cell(map([
        ("name", Value::String(format!("parameter-{index}"))),
        ("label", Value::String(format!("Parameter {index}"))),
        ("idx", Value::Number(index as f64)),
        ("min", Value::Number(0.0)),
        ("max", Value::Number(127.0)),
        ("value", Value::Number(64.0)),
        ("modulatable", Value::Bool(true)),
        ("options", Value::List((0..8).map(|i| cell(Value::String(format!("choice-{i}")))).collect())),
    ]))).collect();
    map([("name", Value::String("instrument".to_string())), ("params", Value::List(params))])
}

// Lower bound on separately owned payload: excludes HashMap bucket overhead,
// allocator rounding and all root headers. Cells include Rc's two counters.
fn owned_input_bytes(value: &Value) -> usize {
    let child_bytes = |value: &Rc<RefCell<Value>>| {
        size_of::<RefCell<Value>>() + 2 * size_of::<usize>() + owned_input_bytes(&value.borrow())
    };
    match value {
        Value::String(s) | Value::Symbol(s) | Value::Keyword(s) => s.capacity(),
        Value::List(items) => items.capacity() * size_of::<Rc<RefCell<Value>>>()
            + items.iter().map(child_bytes).sum::<usize>(),
        Value::Map(items) => items.iter().map(|(key, value)| key.capacity() + child_bytes(value)).sum(),
        _ => 0,
    }
}

#[test]
fn compact_inputs_eliminate_per_value_allocations_for_large_descriptors() {
    let input = cell(large_descriptor());
    let snapshot = SubtreeInputs::capture(&[input.clone()]).unwrap();
    let old_bytes = owned_input_bytes(&input.borrow().deep_clone());
    assert!(snapshot.retained_bytes() * 5 < old_bytes,
        "comparison storage {} vs old payload lower bound {old_bytes}", snapshot.retained_bytes());
    assert!(snapshot.matches(&[input]));
}

#[test]
#[ignore = "manual release comparison of subtree input allocation and warm reuse"]
fn subtree_input_storage_benchmark() {
    assert!(!cfg!(debug_assertions), "run with --release");
    let cases = [
        ("empty", vec![], 16_384),
        ("small", vec![cell(Value::Instance(42)), cell(Value::Bool(true)),
            cell(Value::Number(3.0)), cell(Value::String("control-42".to_string()))], 4096),
        ("descriptor", vec![cell(large_descriptor())], 64),
    ];
    for (name, inputs, iterations) in cases {
        let old_bytes = inputs.iter().map(|input| owned_input_bytes(&input.borrow().deep_clone()))
            .sum::<usize>() + inputs.len() * size_of::<Value>();
        let bytes = SubtreeInputs::capture(&inputs).unwrap().retained_bytes();
        eprintln!("case={name} snapshot_bytes={bytes} old_payload_lower_bound={old_bytes}");
        for trial in 0..7 {
            let start = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(inputs.iter().map(|cell| cell.borrow().deep_clone()).collect::<Vec<_>>());
            }
            let old_capture = start.elapsed();
            let start = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(SubtreeInputs::capture(&inputs).unwrap());
            }
            let new_capture = start.elapsed();
            let old = inputs.iter().map(|cell| cell.borrow().deep_clone()).collect::<Vec<_>>();
            let new = SubtreeInputs::capture(&inputs).unwrap();
            let start = Instant::now();
            for _ in 0..iterations * 4 {
                assert!(std::hint::black_box(inputs.iter().zip(&old)
                    .all(|(live, old)| subtree_input_value_equal(&live.borrow(), old))));
            }
            let old_reuse = start.elapsed();
            let start = Instant::now();
            for _ in 0..iterations * 4 {
                assert!(std::hint::black_box(new.matches(&inputs)));
            }
            let new_reuse = start.elapsed();
            eprintln!("case={name} trial={trial} capture_us={:.3}->{:.3} reuse_us={:.3}->{:.3}",
                old_capture.as_secs_f64()*1e6/iterations as f64,
                new_capture.as_secs_f64()*1e6/iterations as f64,
                old_reuse.as_secs_f64()*1e6/(iterations*4) as f64,
                new_reuse.as_secs_f64()*1e6/(iterations*4) as f64);
        }
    }
}
