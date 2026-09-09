use super::*;

#[test]
fn digi_fm_pages_expose_bound_visible_controls() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "content/instruments/Synths/Digi FM/ui.lisp".to_string(),
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content/instruments/Synths/Digi FM/ui.lisp")).unwrap(),
    )));
    let mut digi_fm_inst = test_instrument_map();
    let dsp = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content/instruments/Synths/Digi FM/dsp.lisp")).unwrap();
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
            let field = format!("digi_fm-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            if words[1] == "harmonics" {
                param.insert("mod-value-field".into(), Rc::new(RefCell::new(Value::String("digi_fm-live-harmonics".into()))));
                bindings.push(("digi_fm-live-harmonics".into(), Value::Number(value)));
            }
            Value::Map(param)
        }).collect();
    digi_fm_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(digi_fm_inst)])),
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
        .expect("load digi_fm custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("digi_fm fx lisp status after refresh: {status}");
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
        }
        for value in node.props.values() {
            if let Value::ReactiveRef { field, .. } = value {
                if let Some(name) = field.strip_prefix("digi_fm-test-") { seen.insert(name.to_string()); }
            }
        }
        for child in &node.children { check(child, panel, seen); }
    }
    for section in 0..7 {
        editor.runtime_mut().eval_str(&format!(r#"
            (do (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
                ((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false))
        "#)).unwrap();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() { panic!("page {section}: {status}"); }
        let layout = editor.widget_layout().unwrap();
        for debug in ["df-detail", "df-output", "df-operators"] {
            let panel = find_layout_node_by_debug_name(&layout, debug).expect(debug);
            assert_finite_nonzero_rect(panel, debug);
            check(panel, panel, &mut seen);
        }
        for debug in match section {
            0 => vec!["df-algorithm-1", "df-algorithm-8"],
            1 | 2 => vec!["df-timbre-contour"],
            3 => vec!["df-harmonic-spectrum"],
            5 | 6 => vec!["df-adsr"],
            _ => vec![],
        } {
            let node = find_layout_node_by_debug_name(&layout, debug).expect(debug);
            assert_finite_nonzero_rect(node, debug);
            if section == 3 {
                let Some(Value::ReactiveRef { namespace, field, slot, .. }) = node.props.get("shader-state-harm") else {
                    panic!("spectrum must bind directly to engine telemetry");
                };
                assert_eq!(namespace, "SEQ");
                assert_eq!(field, "digi_fm-live-harmonics");
                // Keep the original widget/slot: meter updates must reach its
                // shader without re-evaluating the custom UI or editing the base.
                for value in [0.75, 3.0, -2.5, 0.0] {
                    editor.runtime_mut().set_reactive("SEQ", field, Value::Number(value));
                    assert_eq!(eseqlisp::reactive::read_float_slot(slot), value);
                    assert_eq!(editor.runtime_mut().eval_str(
                        "(reactive-value (bind \"SEQ\" \"digi_fm-test-harmonics\"))"
                    ).unwrap().unwrap(), Value::Number(0.0));
                }
            }
            if section == 1 || section == 2 {
                assert_eq!(node.widget_type, "adsr-editor");
                assert_eq!(node.props.get("mode"), Some(&Value::Keyword("ade".into())));
                for prop in ["delay", "attack", "decay", "end", "gated", "hold-on-release"] {
                    assert!(matches!(node.props.get(prop), Some(Value::ReactiveRef { .. })), "{prop}");
                }
            }
            if matches!(section, 1 | 2 | 5 | 6) {
                let (prefix, stages) = match section {
                    1 => ("a", vec!["delay", "attack", "decay", "end"]),
                    2 => ("b", vec!["delay", "attack", "decay", "end"]),
                    5 => ("filter", vec!["attack", "decay", "sustain", "release"]),
                    _ => ("amp", vec!["attack", "decay", "sustain", "release"]),
                };
                let names: Vec<_> = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                    .map(|line| line.split_whitespace().nth(1).unwrap()).collect();
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    for active in [true, false] {
                        let payload = editor.runtime_mut().eval_str(&format!(
                            "(dict :delay 100 :attack 200 :decay 300 :end 0.4 :sustain 0.6 :release 400 :active {})",
                            if active { ":decay" } else { "false" })).unwrap().unwrap();
                        editor.drain_host_commands();
                        editor.runtime_mut().invoke(node.props["on-change"].clone(), vec![payload]).unwrap();
                        let commands = editor.drain_host_commands();
                        assert_eq!(commands.len(), 1, "one atomic envelope update: {commands:?}");
                        let eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } = &commands[0] else { panic!("envelope command"); };
                        assert_eq!(name, if locked { "set-instrument-plock-batch" } else { "set-instrument-param-batch" });
                        assert_eq!(*payload["commit"].borrow(), Value::Bool(!active));
                        let Value::List(updates) = &*payload["updates"].borrow() else { panic!("updates"); };
                        assert_eq!(updates.len(), 4);
                        for (update, stage) in updates.iter().zip(&stages) {
                            let Value::Map(update) = &*update.borrow() else { panic!("update"); };
                            let Value::Number(index) = *update["param-idx"].borrow() else { panic!("index"); };
                            let suffix = if matches!(*stage, "end" | "sustain") { "" } else { "_ms" };
                            assert_eq!(names[index as usize], format!("{prefix}_{stage}{suffix}"));
                            let expected = match *stage { "delay" => 100.0, "attack" => 200.0, "decay" => 300.0, "end" => 0.4, "sustain" => 0.6, _ => 400.0 };
                            assert_eq!(*update["value"].borrow(), Value::Number(expected));
                        }
                    }
                }
                editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
            }
        }
    }
    let expected: std::collections::HashSet<_> = dsp.lines().map(str::trim)
        .filter(|line| line.starts_with("(param "))
        .map(|line| line.split_whitespace().nth(1).unwrap().to_string()).collect();
    assert_eq!(seen, expected, "Every DSP parameter must be reachable through the panel");
}
