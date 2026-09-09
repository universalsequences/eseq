use super::*;

#[test]
fn clap_display_has_bound_waveform_and_visible_pages() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "instruments/Drums/808 Clap/ui.lisp".to_string(),
        read_factory_source("instruments/Drums/808 Clap/ui.lisp").unwrap(),
    )));
    let mut clap_inst = test_instrument_map();
    let dsp = read_factory_source("instruments/Drums/808 Clap/dsp.lisp").unwrap();
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
            let field = format!("clap-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    clap_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(clap_inst)])),
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
        .expect("load clap custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("clap fx lisp status after refresh: {status}");
    }

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx lisp should create the *fx* buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor.set_layout_viewport(180, 18);



    fn find_param<'a>(node: &'a eseqlisp::layout::LayoutNode, suffix: &str) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.stable_key.as_deref().is_some_and(|key| key.ends_with(suffix)) { return Some(node); }
        node.children.iter().find_map(|child| find_param(child, suffix))
    }
    fn visible(node: &eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode) {
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker" | "dropdown" |
            "eseq-clap-burst-display" | "wavetable-viewer" | "response-curve-editor" | "lfo-curve") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.001
                && node.rect.col >= panel.rect.col
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.001,
                "{} outside panel: {:?} / {:?}", node.widget_type, node.rect, panel.rect);
        }
        for child in &node.children { visible(child, panel); }
    }
    for section in 0..4 {
        editor.runtime_mut().eval_str(&format!("((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false)")).unwrap();
        editor.refresh_runtime_side_effects();
        let current = editor.widget_layout().unwrap();
        let panel = find_layout_node_by_debug_name(&current, "instrument-panel").unwrap();
        let display = find_layout_node_by_debug_name(&current, "clap-display").unwrap();
        visible(panel, panel);
        visible(display, display);
        let wave = find_layout_node_by_debug_name(display, "clap-wave").unwrap();
        for prop in ["sp1", "sp2", "sp3", "flam", "l2", "l3", "l4"] {
            assert!(matches!(wave.props.get(prop), Some(Value::ReactiveRef { .. })), "{prop}");
        }
        for param in ["tune", "flam", "snap", "body", "decay", "bright", "drive", "level", "fc1", "q1", "fc2", "out_drive", "out_gain", "out_hp"] {
            assert!(find_param(&current, param).is_some(), "persistent {param}");
        }
        for locked in [false, true] {
            editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
            editor.drain_host_commands();
            editor.runtime_mut().invoke(wave.props["on-drag"].clone(), vec![
                Value::Number(-0.5), Value::Number(-0.74), Value::Keyword("burst-2".into())
            ]).unwrap();
            let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
            let commands = editor.drain_host_commands();
            let payload = commands.iter().find_map(|command| match command {
                eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } if name == expected => Some(payload),
                _ => None,
            }).unwrap_or_else(|| panic!("expected {expected}: {commands:?}"));
            let index = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                .position(|line| line.split_whitespace().nth(1) == Some("sp1")).unwrap();
            assert_eq!(*payload["param-idx"].borrow(), Value::Number(index as f64));
            assert_eq!(*payload["value"].borrow(), Value::Number(16.0));
        }
        editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
        editor.runtime_mut().set_reactive("SEQ", "clap-test-sp1", Value::Number(12.0));
        editor.runtime_mut().run_reactive_cycle();
        let Value::ReactiveRef { slot, .. } = &wave.props["sp1"] else { panic!("binding"); };
        assert_eq!(eseqlisp::reactive::read_float_slot(slot), 12.0);
    }
}
