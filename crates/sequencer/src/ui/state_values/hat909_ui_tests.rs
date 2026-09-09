use super::*;

#[test]
fn hat909_performance_controls_and_envelopes_are_bound_and_visible() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "instruments/Drums/909 Open Hat/ui.lisp".to_string(),
        read_factory_source("instruments/Drums/909 Open Hat/ui.lisp").unwrap(),
    )));
    let mut hat909_inst = test_instrument_map();
    let dsp = read_factory_source("instruments/Drums/909 Open Hat/dsp.lisp").unwrap();
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
            let field = format!("hat909-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    hat909_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(hat909_inst)])),
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
        .expect("load hat909 custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("hat909 fx lisp status after refresh: {status}");
    }

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx lisp should create the *fx* buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor.set_layout_viewport(180, 18);



    let layout = editor.widget_layout().unwrap();
    let display = find_layout_node_by_debug_name(&layout, "hat-display").unwrap();
    assert_finite_nonzero_rect(display, "hat display");
    fn check(node: &eseqlisp::layout::LayoutNode, display: &eseqlisp::layout::LayoutNode) {
        assert_finite_nonzero_rect(node, "envelope");
        assert!(node.rect.row >= display.rect.row && node.rect.row + node.rect.height <= display.rect.row + display.rect.height);
    }
    for index in 1..=12 {
        let node = find_layout_node_by_debug_name(display, &format!("hat-mode-{index}")).unwrap();
        check(node, display);
        for prop in ["gain", "rate", "tail", "decay", "metal", "attack", "hold"] {
            assert!(matches!(node.props.get(prop), Some(Value::ReactiveRef { .. })), "{prop}");
        }
    }
    let wash = find_layout_node_by_debug_name(display, "hat-wash").unwrap();
    check(wash, display);
    for prop in ["tail", "fast", "amount", "decay", "wash", "gain", "attack", "hold"] {
        assert!(matches!(wash.props.get(prop), Some(Value::ReactiveRef { .. })));
    }

    for debug in ["hat-mode-1", "hat-mode-12", "hat-wash"] {
        let curve = find_layout_node_by_debug_name(display, debug).unwrap();
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    let x = curve.rect.col + curve.rect.width * 0.5;
                    let y = curve.rect.row + curve.rect.height * 0.5;
                    for (kind, px, py) in [
                        (crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), x, y),
                        (crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left), x + curve.rect.width * 0.1, y - curve.rect.height * 0.05),
                        (crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left), x + curve.rect.width * 0.1, y),
                    ] {
                        editor.handle_mouse_precise(crossterm::event::MouseEvent { kind, column: px as u16, row: py as u16,
                            modifiers: crossterm::event::KeyModifiers::NONE }, 0, 0, 180, 18, px, py);
                    }
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    let commands = editor.drain_host_commands();
                    assert!(commands.iter().any(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)), "curve {} {commands:?}", curve.widget_type);
                }
    }
    fn knobs<'a>(node: &'a eseqlisp::layout::LayoutNode, found: &mut Vec<&'a eseqlisp::layout::LayoutNode>) {
        if node.widget_type == "knob-number" { found.push(node); }
        for child in &node.children { knobs(child, found); }
    }
    let mut controls = Vec::new();
    knobs(&layout, &mut controls);
    assert_eq!(controls.len(), 8, "only the performance controls");
    for (index, control) in controls.iter().enumerate() {
        assert_finite_nonzero_rect(control, "performance knob");
        assert!(matches!(control.props.get("value"), Some(Value::ReactiveRef { .. })));
        for locked in [false, true] {
            editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
            editor.drain_host_commands();
            editor.runtime_mut().invoke(control.props["on-change"].clone(), vec![Value::Number(0.5)]).unwrap();
            let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
            assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
                eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)), "control {index}");
        }
    }
}
