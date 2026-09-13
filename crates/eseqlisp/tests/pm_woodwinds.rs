//! Factory patches must keep the editable graph and executable source in sync.
use std::collections::HashMap;
use std::path::PathBuf;

use eseqlisp::layout::{LayoutNode, Rect};
use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::vm::Value;
use eseqlisp::widget_render::patcher::{
    PatcherIntent, emit_patch_writeback_source, emitted_source_buffer_snapshot,
    source_opens_in_patch_editor,
};

// Parameter and audio checks alone cannot detect a stale, nearly equivalent
// but much more expensive DSP algorithm. Count operators inside each local
// macro, independent of generated binding names and statement order.
fn piano_macro_operators(source: &str) -> HashMap<String, HashMap<String, usize>> {
    let tokens = Parser::new(source.to_string()).parse().unwrap();
    let expressions = ASTParser::new(tokens).parse().unwrap();
    let mut result = HashMap::new();
    for expression in &expressions {
        let Expression::List(items) = expression else { continue };
        let [Expression::Symbol(op), Expression::Symbol(name), _, body @ ..] = items.as_slice()
            else { continue };
        if op != "defmacro" || !name.starts_with("piano-") { continue }
        let mut operators = HashMap::new();
        let mut pending: Vec<_> = body.iter().collect();
        while let Some(expression) = pending.pop() {
            if let Expression::List(items) = expression {
                if let Some(Expression::Symbol(op)) = items.first() {
                    if op != "def" { *operators.entry(op.clone()).or_insert(0) += 1; }
                }
                pending.extend(items.iter());
            }
        }
        result.insert(name.clone(), operators);
    }
    result
}

fn check_factory_sidecars(names: &[&str]) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    eseqlisp::defmacro_library::set_default_library_root(root.join("content/defmacros"));
    // Allow generated instruments to be verified before installing them live.
    let factory = std::env::var_os("ESEQ_PM_FACTORY_DIR").map(PathBuf::from)
        .unwrap_or_else(|| root.join("content/instruments/Physical Models"));
    for name in names {
        let path = factory.join(name).join("dsp.lisp");
        let source = std::fs::read_to_string(&path).unwrap();
        assert!(source_opens_in_patch_editor(&path, &source, PatcherIntent::Instrument));
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "patcher".into(),
            rect: Rect { row: 0.0, col: 0.0, width: 160.0, height: 100.0 },
            props: HashMap::from([
                ("path".into(), Value::String(path.display().to_string())),
                ("intent".into(), Value::Keyword("instrument".into())),
            ]),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let emitted = emitted_source_buffer_snapshot(&node).unwrap().source;
        // Projecting both executable forms normalizes generated node names and
        // ordering. A stale sidecar would silently undo controls on patch save.
        let canonical = emit_patch_writeback_source(&source, PatcherIntent::Instrument).unwrap();
        let saved = emit_patch_writeback_source(&emitted, PatcherIntent::Instrument).unwrap();
        let expected = eseqlisp::defmacro_library::materialize_with_default_library(&canonical).unwrap();
        let actual = eseqlisp::defmacro_library::materialize_with_default_library(&saved).unwrap();
        if let Some(output) = std::env::var_os("ESEQ_PM_VERIFY_DIR") {
            let output = PathBuf::from(output).join(name);
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("dsp.lisp"), &actual).unwrap();
        }
        // Graph identities can preserve explicit cables that source projection
        // prints inline. Compare parameter contracts, then use the compiled
        // roundtrip renders in tools/pm-woodwinds/verify.py for audio equivalence.
        let params = |text: &str| {
            let mut result: Vec<_> = text.lines().filter(|line| line.trim_start().starts_with("(param "))
                .map(|line| line.trim().to_string()).collect();
            result.sort();
            result
        };
        assert_eq!(params(&expected), params(&actual), "{name} parameter contract");
        // Numeric modal coefficients are executable state too. Comparing only
        // parameters missed scientific literals split into a number and symbol.
        let tensor_tables = |text: &str| {
            // Both inputs are canonical writeback, whose literal tensors use
            // this fixed attribute order and bracket spelling.
            text.split("(def ").filter_map(|definition| {
                let (name, body) = definition.split_once(" (tensor @shape [")?;
                let (shape, body) = body.split_once("] @data [")?;
                let (data, _) = body.split_once(']')?;
                let shape: Vec<usize> = shape.split_whitespace().map(|n| n.parse().unwrap()).collect();
                let data: Vec<f64> = data.split_whitespace().map(|n| n.parse().unwrap()).collect();
                assert_eq!(shape.iter().product::<usize>(), data.len(), "{name} tensor length");
                Some((name.to_string(), (shape, data)))
            }).collect::<HashMap<_, _>>()
        };
        let expected_tables = tensor_tables(&expected);
        if *name == "PM Piano" {
            assert_eq!(expected_tables.len(), 9, "piano calibration tables");
            let expected_macros = piano_macro_operators(&expected);
            assert!(!expected_macros.is_empty(), "piano DSP macros");
            assert_eq!(expected_macros, piano_macro_operators(&actual), "piano macro algorithms");
        }
        assert_eq!(expected_tables, tensor_tables(&actual), "{name} tensor coefficients");
    }
}

#[test]
fn factory_woodwind_sidecars_preserve_executable_controls() {
    check_factory_sidecars(&["PM Flute", "PM Saxophone", "PM Clarinet"]);
}

#[test]
fn factory_cello_sidecar_preserves_executable_controls() {
    check_factory_sidecars(&["PM Cello"]);
}

#[test]
fn factory_piano_sidecar_preserves_executable_controls() {
    check_factory_sidecars(&["PM Piano"]);
}

#[test]
fn factory_saron_sidecar_preserves_executable_controls() {
    check_factory_sidecars(&["PM Saron"]);
}

#[test]
fn factory_gamelan_sidecars_preserve_executable_controls() {
    check_factory_sidecars(&["PM Slenthem", "PM Bonang", "PM Slenthem Slendro", "PM Kempyang", "PM Kethuk"]);
}

#[test]
fn factory_cymbal_sidecars_preserve_executable_controls() {
    check_factory_sidecars(&["PM Crash", "PM Ride", "PM Hi-Hat"]);
}
