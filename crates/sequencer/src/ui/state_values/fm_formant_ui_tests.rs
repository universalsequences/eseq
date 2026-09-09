use super::*;

#[test]
fn fm_formant_pages_expose_bound_visible_controls() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "tools/fm-formant/ui.lisp".to_string(),
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fm-formant/ui.lisp")).unwrap(),
    )));
    let mut fm_formant_inst = test_instrument_map();
    let dsp = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/fm-formant/candidate/dsp.lisp")).unwrap();
    let mut bindings = Vec::new();
    let params = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
        .enumerate().map(|(index, line)| {
            let words: Vec<_> = line.trim_end_matches(')').split_whitespace().collect();
            let number = |key| {
                let pos = words.iter().position(|word| *word == key).unwrap();
                words[pos + 1].parse::<f64>().unwrap()
            };
            let value = number("@default");
            let mut param = test_param_map(words[1], index, value, number("@min"), number("@max"));
            let field = format!("fm_formant-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    fm_formant_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

    let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
    editor.set_layout_viewport(180, 18);
    editor.runtime_mut().register_reactive(
        "SEQ",
        vec![
            ("num-tracks", Value::Number(1.0)),
            ("compiling", Value::Bool(false)),
            ("available-effects", test_list(vec![])),
            ("available-builtin-effects", test_list(vec![])),
            ("available-midi-effects", test_list(vec![])),
            ("bus-names", test_list(vec![])),
            ("effects", test_list(vec![])),
            ("midi-effects", test_list(vec![])),
            ("instrument-panel", test_list(vec![Value::Map(fm_formant_inst)])),
            ("bus-effects", test_list(vec![])),
        ],
        true,
    );
    for (field, value) in bindings {
        editor.runtime_mut().set_reactive("SEQ", &field, value);
    }
    editor
        .runtime_mut()
        .eval_str(
            r#"
            (def eseq.seq-core-state/selected-bus-name () "Mix")
            (def seq-has-selection? () false)
            (def eseq.browser/sbrowser-editor-name "")
            (defmacro eseq.materials/slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
            (def custom-midi-fx-ui (fx) false)
            (def custom-audio-fx-ui (fx) false)
            (defstate eseq.seq-core-state/selected-bus -1)
            "#,
        )
        .expect("install fx test helpers");
    register_test_delete_target_natives(&mut editor, 1);
    editor
        .runtime_mut()
        .eval_str(&custom_ui_source)
        .expect("load fm_formant custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("fm_formant fx lisp status after refresh: {status}");
    }

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx lisp should create the *fx* buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor.set_layout_viewport(180, 18);



    let mut seen = std::collections::HashSet::new();
    fn check(node: &eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode, seen: &mut std::collections::HashSet<String>) {
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker" | "dropdown" | "adsr-editor" | "label") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row && node.rect.col >= panel.rect.col
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.01
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.01,
                "{} clipped: {:?} in {:?}", node.widget_type, node.rect, panel.rect);
            for value in node.props.values() {
                if let Value::ReactiveRef { field, .. } = value {
                    if let Some(name) = field.strip_prefix("fm_formant-test-") { seen.insert(name.to_string()); }
                }
            }
        }
        for child in &node.children { check(child, panel, seen); }
    }
    for section in 0..24 {
        editor.runtime_mut().eval_str(&format!(r#"
            (do (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
                ((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false))
        "#)).unwrap();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() { panic!("page {section}: {status}"); }
        let layout = editor.widget_layout().unwrap();
        if section == 0 {
            for mode in 0..3 {
                let diagram = find_layout_node_by_debug_name(&layout, &format!("ff-algorithm-{mode}")).expect("algorithm diagram");
                assert_finite_nonzero_rect(diagram, "algorithm diagram");
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    editor.runtime_mut().invoke(diagram.props["on-click"].clone(), vec![Value::Number(0.0), Value::Number(0.0), Value::Number(0.0)]).unwrap();
                    let commands = editor.drain_host_commands();
                    let expected = if locked { "set-instrument-plock-batch" } else { "set-instrument-param-batch" };
                    assert_eq!(commands.len(), 1, "one atomic routing edit: {commands:?}");
                    let eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } = &commands[0] else { panic!("routing command"); };
                    assert_eq!(name, expected);
                    assert_eq!(*payload["commit"].borrow(), Value::Bool(true));
                    let Value::List(updates) = &*payload["updates"].borrow() else { panic!("updates"); };
                    assert_eq!(updates.len(), 26);
                    let names: Vec<_> = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                        .map(|line| line.split_whitespace().nth(1).unwrap()).collect();
                    let mut indices = std::collections::HashSet::new();
                    let span = 1usize << mode;
                    for update in updates {
                        let Value::Map(update) = &*update.borrow() else { panic!("update map"); };
                        let Value::Number(index) = *update["param-idx"].borrow() else { panic!("index"); };
                        assert!(indices.insert(index as usize), "duplicate update");
                        let param = names[index as usize];
                        let expected_value = if param.starts_with("pm_") {
                            let parts: Vec<_> = param.split('_').collect();
                            let from: usize = parts[1].parse().unwrap();
                            let to: usize = parts[3].parse().unwrap();
                            if from + 1 == to && (to - 1) % span != 0 { 1.0 } else { 0.0 }
                        } else if param.starts_with("fb_") { 0.0 }
                        else {
                            assert!(param.ends_with("_output"));
                            let oscillator: usize = param[1..2].parse().unwrap();
                            if oscillator % span == 0 { 1.0 } else { 0.0 }
                        };
                        assert_eq!(*update["value"].borrow(), Value::Number(expected_value), "{param}");
                    }
                }
            }
            editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
            editor.runtime_mut().set_reactive("SEQ", "fm_formant-test-pm_1_to_2", Value::Number(0.25));
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let custom_layout = editor.widget_layout().unwrap();
            for mode in 0..3 {
                let diagram = find_layout_node_by_debug_name(&custom_layout, &format!("ff-algorithm-{mode}")).unwrap();
                assert_eq!(diagram.props.get("selected"), Some(&Value::Number(0.0)), "custom route must not claim a preset");
            }
            editor.runtime_mut().set_reactive("SEQ", "fm_formant-test-pm_1_to_2", Value::Number(0.0));
            editor.runtime_mut().run_reactive_cycle();
        }
        if section > 0 && section < 17 && section % 4 == 0 {
            let diagram = find_layout_node_by_debug_name(&layout, "ff-incoming").expect("incoming diagram");
            assert_finite_nonzero_rect(diagram, "incoming diagram");
            for source in 1..5 {
                assert!(matches!(diagram.props.get(&format!("f{source}")), Some(Value::ReactiveRef { .. })));
            }
            editor.runtime_mut().invoke(diagram.props["on-click"].clone(), vec![Value::Number(0.0), Value::Number(0.0), Value::Keyword("d".into())]).unwrap();
            assert_eq!(editor.runtime_mut().eval_str("eseq.vanilla/custom-ui-selected-section").unwrap(), Some(Value::Number(16.0)));
        }
        for debug in ["ff-detail", "ff-performance", "ff-oscillators"] {
            let panel = find_layout_node_by_debug_name(&layout, debug).expect(debug);
            assert_finite_nonzero_rect(panel, debug);
            check(panel, panel, &mut seen);
        }
    }
    let expected: std::collections::HashSet<_> = dsp.lines().map(str::trim)
        .filter(|line| line.starts_with("(param "))
        .map(|line| line.split_whitespace().nth(1).unwrap().to_string()).collect();
    assert_eq!(seen, expected, "Every DSP parameter must be reachable through the panel");
}
