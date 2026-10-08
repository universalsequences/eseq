//! `&optional`, `&rest` and `&key` parameters for `def` and `lambda`
//! (eseq-0l17.40).

use super::{VM, VMError, Value};

fn vm() -> VM {
    let mut vm = VM::new(Vec::new());
    super::register_core_natives(&mut vm);
    super::register_math_natives(&mut vm);
    vm
}

fn eval(vm: &mut VM, code: &str) -> Value {
    vm.eval_str(code)
        .unwrap_or_else(|e| panic!("{code}: {e:?}"))
        .unwrap_or(Value::Nil)
}

fn shown(vm: &mut VM, code: &str) -> String {
    super::format_lisp_source(&eval(vm, code))
}

fn arity_error(vm: &mut VM, code: &str) -> String {
    match vm.eval_str(code) {
        Err(VMError::Arity(message)) => message,
        other => panic!("{code}: expected an arity error, got {other:?}"),
    }
}

fn compile_error(vm: &mut VM, code: &str) -> String {
    match vm.eval_str(code) {
        Err(VMError::CompileError) => vm.take_source_load_errors().join("; "),
        other => panic!("{code}: expected a compile error, got {other:?}"),
    }
}

#[test]
fn optional_params_default_to_nil_or_their_default() {
    let mut vm = vm();
    eval(&mut vm, "(def f (a b &optional c (d 10)) (list a b c d))");
    assert_eq!(shown(&mut vm, "(f 1 2)"), "(1 2 nil 10)");
    assert_eq!(shown(&mut vm, "(f 1 2 3)"), "(1 2 3 10)");
    assert_eq!(shown(&mut vm, "(f 1 2 3 4)"), "(1 2 3 4)");
    // An explicit nil is a supplied argument, not a request for the default.
    assert_eq!(shown(&mut vm, "(f 1 2 nil nil)"), "(1 2 nil nil)");
}

#[test]
fn defaults_are_evaluated_per_call_and_see_earlier_params() {
    let mut vm = vm();
    eval(&mut vm, "(def counter 0)");
    eval(
        &mut vm,
        "(def f (a &key (b (+ a 1)) (c (* b 2)) (d (do (set! counter (+ counter 1)) (+ a c)))) \
           (list a b c d))",
    );
    assert_eq!(shown(&mut vm, "(f 1)"), "(1 2 4 5)");
    assert_eq!(shown(&mut vm, "(f 1 :b 5)"), "(1 5 10 11)");
    assert_eq!(shown(&mut vm, "(f 1 :d 9 :c 0 :b 5)"), "(1 5 0 9)");
    eval(
        &mut vm,
        "(def g (a &optional (b (+ a 1)) (c (* b 2))) (list a b c))",
    );
    assert_eq!(shown(&mut vm, "(g 1)"), "(1 2 4)");
    assert_eq!(shown(&mut vm, "(g 1 5)"), "(1 5 10)");
    // The default ran for the two calls that left :d out, and only those.
    assert_eq!(eval(&mut vm, "counter"), Value::Number(2.0));
}

#[test]
fn keyword_args_in_any_order_with_defaults() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(def f (a &key size (color :white) (lit false)) (list a size color lit))",
    );
    assert_eq!(shown(&mut vm, "(f 1)"), "(1 nil :white false)");
    assert_eq!(
        shown(&mut vm, "(f 1 :size 3 :lit true)"),
        "(1 3 :white true)"
    );
    assert_eq!(
        shown(&mut vm, "(f 1 :lit true :color :red :size 2)"),
        "(1 2 :red true)"
    );
}

#[test]
fn keyword_errors_name_the_function_and_accepted_keys() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(def f (a &key size (color :white) (lit false)) a)",
    );
    assert_eq!(
        arity_error(&mut vm, "(f 1 :colour :red)"),
        "f: unknown keyword :colour; accepts :size :color :lit"
    );
    assert_eq!(
        arity_error(&mut vm, "(f 1 :size 2 :size 3)"),
        "f: keyword :size given twice"
    );
    assert_eq!(
        arity_error(&mut vm, "(f 1 :size)"),
        "f: keyword :size has no value"
    );
    assert_eq!(
        arity_error(&mut vm, "(f 1 2)"),
        "f: expected a keyword argument, got 2; accepts :size :color :lit"
    );
    assert_eq!(
        arity_error(&mut vm, "(f)"),
        "f takes at least 1 argument, got 0"
    );
}

#[test]
fn arity_errors_describe_the_accepted_range() {
    let mut vm = vm();
    eval(&mut vm, "(def f (a b &optional c d) a)");
    assert_eq!(
        arity_error(&mut vm, "(f 1 2 3 4 5)"),
        "f takes 2 to 4 arguments, got 5"
    );
    assert_eq!(
        arity_error(&mut vm, "(f 1)"),
        "f takes 2 to 4 arguments, got 1"
    );
    eval(&mut vm, "(def g (&optional x) x)");
    assert_eq!(
        arity_error(&mut vm, "(g 1 2)"),
        "g takes 0 to 1 arguments, got 2"
    );
    eval(&mut vm, "(def h (a b &rest more) more)");
    assert_eq!(
        arity_error(&mut vm, "(h 1)"),
        "h takes at least 2 arguments, got 1"
    );
    assert_eq!(
        arity_error(&mut vm, "((lambda (a &optional b) a))"),
        "lambda takes 1 to 2 arguments, got 0"
    );
}

#[test]
fn rest_collects_the_remaining_args() {
    let mut vm = vm();
    eval(&mut vm, "(def f (a &rest more) (list a more))");
    assert_eq!(shown(&mut vm, "(f 1)"), "(1 ())");
    assert_eq!(shown(&mut vm, "(f 1 2 3)"), "(1 (2 3))");
    eval(
        &mut vm,
        "(def g (a &optional (b 7) &rest more) (list a b more))",
    );
    assert_eq!(shown(&mut vm, "(g 1)"), "(1 7 ())");
    assert_eq!(shown(&mut vm, "(g 1 2 3 4)"), "(1 2 (3 4))");
}

#[test]
fn rest_with_key_sees_the_keys_and_needs_allow_other_keys_for_unknown_ones() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(def f (a &rest props &key size) (list a size props))",
    );
    assert_eq!(shown(&mut vm, "(f 1 :size 3)"), "(1 3 (:size 3))");
    assert_eq!(
        arity_error(&mut vm, "(f 1 :pad 2)"),
        "f: unknown keyword :pad; accepts :size"
    );
    eval(
        &mut vm,
        "(def g (a &rest props &key size &allow-other-keys) (list a size props))",
    );
    assert_eq!(
        shown(&mut vm, "(g 1 :pad 2 :size 3)"),
        "(1 3 (:pad 2 :size 3))"
    );
}

#[test]
fn optional_with_key_is_rejected() {
    let mut vm = vm();
    let message = compile_error(
        &mut vm,
        "(def f (a &optional (b 1) &key (c 2)) (list a b c))",
    );
    assert!(
        message.contains("f: &optional and &key together are ambiguous; use &key"),
        "{message}"
    );
    // Also with nothing after &optional, and across &rest.
    assert!(compile_error(&mut vm, "(def g (&optional &key c) c)").contains("ambiguous"));
    assert!(compile_error(&mut vm, "(def h (&optional b &rest r &key c) c)").contains("ambiguous"));
}

#[test]
fn destructured_params_are_visible_to_defaults() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(def f ((note vel) &key (gain (* vel 2)) (label (str note))) (list note vel gain label))",
    );
    assert_eq!(
        shown(&mut vm, "(f (dict :note 60 :vel 3))"),
        "(60 3 6 \"60\")"
    );
    assert_eq!(
        shown(&mut vm, "(f (dict :note 60 :vel 3) :gain 1)"),
        "(60 3 1 \"60\")"
    );
    // A closure in the body captures a destructured field.
    eval(&mut vm, "(def g ((x) &optional (y 1)) (lambda () (+ x y)))");
    assert_eq!(eval(&mut vm, "((g (dict :x 4)))"), Value::Number(5.0));
    assert!(compile_error(&mut vm, "(def h ((x) &optional x) x)").contains("appears twice"));
}

#[test]
fn a_default_reading_a_later_param_is_a_compile_error() {
    let mut vm = vm();
    // A global of the same name must not be read silently instead.
    eval(&mut vm, "(def c 100)");
    let message = compile_error(&mut vm, "(def f (a &optional (b c) c) (list a b c))");
    assert!(
        message.contains("f: the default for `b` reads `c`, which is not bound yet; defaults see earlier parameters only"),
        "{message}"
    );
    // Itself, a dotted read, the rest list and a key all count as later.
    assert!(compile_error(&mut vm, "(def g (&key (n (+ n 1))) n)").contains("reads `n`"));
    assert!(compile_error(&mut vm, "(def g (&key (n m.x) m) n)").contains("reads `m`"));
    assert!(
        compile_error(&mut vm, "(lambda (&optional (n (len more)) &rest more) n)")
            .contains("reads `more`")
    );
    // Quoted data is not a read, and earlier params are fine.
    eval(&mut vm, "(def ok (a &key (b 'c) (c (list a b))) c)");
    assert_eq!(shown(&mut vm, "(ok 1)"), "(1 c)");
}

#[test]
fn apply_reads_a_bound_list_and_rejects_a_non_list() {
    let mut vm = vm();
    crate::widgets::register_widget_natives(&mut vm);
    eval(
        &mut vm,
        "(def-kind k :key () :state ((tint :rgb :default (rgb 1 0 0))))",
    );
    let direct = shown(&mut vm, "k.tint");
    assert_eq!(shown(&mut vm, "(apply list #'k.tint)"), direct);
    assert_eq!(
        shown(&mut vm, "(apply list 9 #'k.tint)"),
        format!("(9 {}", &direct[1..])
    );
    match vm.eval_str("(apply list 1 2)") {
        Err(VMError::Type(message)) => {
            assert_eq!(message, "apply: the last argument must be a list, got 2")
        }
        other => panic!("expected a type error, got {other:?}"),
    }
}

#[test]
fn macro_lambda_list_errors_have_messages() {
    let mut vm = vm();
    for (code, expected) in [
        (
            "(defmacro m (a &optional b) `(list ,a))",
            "m: macros take &rest only",
        ),
        (
            "(defmacro m (a &key b) `(list ,a))",
            "m: macros take &rest only",
        ),
        (
            "(defmacro m ((a b)) `(list ,a))",
            "m: macros take &rest only; a parameter must be a name",
        ),
        (
            "(defmacro m (&rest) `(list))",
            "m: &rest needs a parameter name",
        ),
        (
            "(defmacro m (&rest xs y) `(list))",
            "m: &rest takes one parameter name",
        ),
        (
            "(defmacro m (x &rest x) `(list))",
            "m: parameter `x` appears twice",
        ),
        (
            "(defmacro m (x x) `(list))",
            "m: parameter `x` appears twice",
        ),
    ] {
        let message = compile_error(&mut vm, code);
        assert!(message.contains(expected), "{code}: {message}");
    }
    // A &rest macro still packs its trailing arguments.
    eval(&mut vm, "(defmacro both (x &rest xs) `(list ,x ,@xs))");
    assert_eq!(shown(&mut vm, "(both 1 2 3)"), "(1 2 3)");
    assert_eq!(shown(&mut vm, "(both 1)"), "(1)");
}

#[test]
fn lambda_list_syntax_errors_are_compile_errors() {
    let mut vm = vm();
    let message = compile_error(&mut vm, "(def f (a &key b &optional c) a)");
    assert!(message.contains("out of order"), "{message}");
    assert!(compile_error(&mut vm, "(def f (a &rest) a)").contains("&rest needs a parameter name"));
    assert!(compile_error(&mut vm, "(def f (a &rest b c) a)").contains("&rest takes one"));
    assert!(compile_error(&mut vm, "(def f (a &optional a) a)").contains("appears twice"));
    assert!(compile_error(&mut vm, "(def f (&aux a) a)").contains("unknown lambda-list marker"));
}

#[test]
fn closures_capture_optional_and_key_params() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(def adder (&key (n 1) (scale 1)) (lambda (x) (* scale (+ x n))))",
    );
    eval(&mut vm, "(def add1 (adder))");
    eval(&mut vm, "(def add5x2 (adder :n 5 :scale 2))");
    assert_eq!(eval(&mut vm, "(add1 10)"), Value::Number(11.0));
    assert_eq!(eval(&mut vm, "(add5x2 10)"), Value::Number(30.0));
    // A lambda with a lambda list closes over its environment too.
    eval(&mut vm, "(def base 100)");
    eval(&mut vm, "(def g (lambda (x &key (by base)) (+ x by)))");
    assert_eq!(eval(&mut vm, "(g 1)"), Value::Number(101.0));
    assert_eq!(eval(&mut vm, "(g 1 :by 2)"), Value::Number(3.0));
}

#[test]
fn apply_map_and_natives_invoke_lambda_list_functions() {
    let mut vm = vm();
    eval(&mut vm, "(def f (a &key (b 10) (c 0)) (+ a b c))");
    assert_eq!(eval(&mut vm, "(apply f (list 1))"), Value::Number(11.0));
    assert_eq!(
        eval(&mut vm, "(apply f 1 :b 2 (list :c 3))"),
        Value::Number(6.0)
    );
    assert_eq!(shown(&mut vm, "(map f (list 1 2))"), "(11 12)");
    assert_eq!(
        shown(
            &mut vm,
            "(filter (lambda (x &optional y) (= y nil)) (list 1 2))"
        ),
        "(1 2)"
    );
    eval(
        &mut vm,
        "(def sum (&rest xs) (reduce (lambda (acc x) (+ acc x)) 0 xs))",
    );
    assert_eq!(
        eval(&mut vm, "(apply sum 1 2 (list 3 4))"),
        Value::Number(10.0)
    );
    // A native that hands the error back surfaces it to the caller.
    assert_eq!(
        arity_error(&mut vm, "(apply f 1 :b 2 (list :d 3))"),
        "f: unknown keyword :d; accepts :b :c"
    );
    // The host calling a closure goes through the same binding.
    let f = vm.eval_str("f").unwrap().unwrap();
    let result = vm.invoke(f.clone(), vec![Value::Number(1.0)]).unwrap();
    assert_eq!(result, Some(Value::Number(11.0)));
    let result = vm.invoke(
        f.clone(),
        vec![
            Value::Number(1.0),
            Value::Keyword("c".into()),
            Value::Number(4.0),
        ],
    );
    assert_eq!(result.unwrap(), Some(Value::Number(15.0)));
    assert!(matches!(vm.invoke(f, vec![]), Err(VMError::Arity(_))));
}

#[test]
fn fixed_arity_functions_keep_their_behaviour() {
    let mut vm = vm();
    eval(&mut vm, "(def f (a b) (list a b))");
    assert_eq!(shown(&mut vm, "(f 1 2)"), "(1 2)");
    // Too few: the missing param is unbound and fails when read, as before.
    assert!(matches!(vm.eval_str("(f 1)"), Err(VMError::UnknownVariable(name)) if name == "b"));
    assert_eq!(vm.eval_str("(f 1 2 3)"), Err(VMError::ArityMismatch));
    // A plain function carries no lambda list, so calls take the direct path.
    let Value::Closure(chunk, _) = vm.eval_str("f").unwrap().unwrap() else {
        panic!("closure")
    };
    assert!(vm.chunks[chunk].lambda_list.is_none());
    // Destructuring patterns still work as required params next to options.
    eval(
        &mut vm,
        "(def g ((note vel) &optional (gain 1)) (* note vel gain))",
    );
    assert_eq!(
        eval(&mut vm, "(g (dict :note 2 :vel 3))"),
        Value::Number(6.0)
    );
    assert_eq!(
        eval(&mut vm, "(g (dict :note 2 :vel 3) 2)"),
        Value::Number(12.0)
    );
}

#[test]
fn module_exported_functions_take_keyword_args() {
    let mut vm = vm();
    let path =
        std::env::temp_dir().join(format!("eseqlisp-lambda-list-{}.lisp", std::process::id()));
    vm.eval_module_source(
        path,
        "(module test.opts)\n(export pill)\n(def pill (text &key (w 6) lit) (list text w lit))",
        1,
    )
    .expect("module");
    assert_eq!(
        shown(&mut vm, "(test.opts/pill \"a\" :lit true)"),
        "(\"a\" 6 true)"
    );
    assert_eq!(
        arity_error(&mut vm, "(test.opts/pill \"a\" :h 2)"),
        "pill: unknown keyword :h; accepts :w :lit"
    );
}

#[test]
fn hot_reload_picks_up_a_changed_lambda_list() {
    let mut vm = vm();
    let path = std::env::temp_dir().join(format!(
        "eseqlisp-lambda-reload-{}.lisp",
        std::process::id()
    ));
    vm.eval_module_source(
        path.clone(),
        "(module test.reload)\n(export f)\n(def f (a b) (+ a b))",
        1,
    )
    .expect("v1");
    eval(&mut vm, "(def call-it () (test.reload/f 1 2))");
    assert_eq!(eval(&mut vm, "(call-it)"), Value::Number(3.0));
    vm.eval_module_source(
        path.clone(),
        "(module test.reload)\n(export f)\n(def f (a &optional (b 10) (c 0)) (+ a b c))",
        2,
    )
    .expect("v2");
    assert_eq!(eval(&mut vm, "(call-it)"), Value::Number(3.0));
    assert_eq!(eval(&mut vm, "(test.reload/f 1)"), Value::Number(11.0));
    assert_eq!(eval(&mut vm, "(test.reload/f 1 10 5)"), Value::Number(16.0));
    // And back to fixed arity.
    vm.eval_module_source(
        path,
        "(module test.reload)\n(export f)\n(def f (a b) (* a b))",
        3,
    )
    .expect("v3");
    assert_eq!(eval(&mut vm, "(call-it)"), Value::Number(2.0));
}

#[test]
fn shipped_bodies_lower_scene_reads_in_defaults_before_their_shadowing_param() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let mut vm = vm();
    vm.register_native("__defscene-register", |_| Value::Nil);
    let shipped = Rc::new(RefCell::new(String::new()));
    let sink = Rc::clone(&shipped);
    vm.register_native("def-sequencer", move |args| {
        if let Some(idx) = args
            .iter()
            .position(|arg| *arg == Value::Keyword("tick".into()))
        {
            *sink.borrow_mut() = super::format_lisp_source(&args[idx + 1]);
        }
        Value::Nil
    });
    eval(&mut vm, "(defscene vel 0.5)");
    eval(
        &mut vm,
        r#"(def-sequencer "s" :resolution :16
             :tick (do (def f (x &optional (y vel)) (list x y))
                       ((lambda (a &key (b vel) (vel 1) (d vel)) (list a b vel d)) 1)))"#,
    );
    let tick = shipped.borrow().clone();
    // Before the `vel` key, a default reads the scene slot by name; after it
    // (and in the body), `vel` is the parameter.
    assert!(tick.contains("(y (__defscene-resolve \"vel\"))"), "{tick}");
    assert!(tick.contains("(b (__defscene-resolve \"vel\"))"), "{tick}");
    assert!(tick.contains("(d vel)"), "{tick}");
    assert!(tick.contains("(list a b vel d)"), "{tick}");
    assert_eq!(tick.matches("__defscene-resolve").count(), 2, "{tick}");
}
