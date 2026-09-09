use super::*;

#[test]
fn kick808_display_has_bound_waveform_and_visible_pages() {
    identified_drum_display("808 Kick");
}

#[test]
fn tom808_display_has_bound_waveform_and_visible_pages() {
    identified_drum_display("808 Tom");
}

#[test]
fn kick909_display_has_bound_waveform_and_visible_pages() {
    identified_drum_display("909 Kick");
}

fn identified_drum_display(instrument: &str) {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        format!("instruments/Drums/{instrument}/ui.lisp"),
        read_factory_source(&format!("instruments/Drums/{instrument}/ui.lisp")).unwrap(),
    )));
    let mut kick808_inst = test_instrument_map();
    let dsp = read_factory_source(&format!("instruments/Drums/{instrument}/dsp.lisp")).unwrap();
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
            let field = format!("kick808-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    kick808_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(kick808_inst)])),
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
        .expect("load kick808 custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("kick808 fx lisp status after refresh: {status}");
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
            "eseq-identified-drum-source" | "eseq-identified-drum-transient" | "eseq-identified-drum-bank-sweep" | "wavetable-viewer" | "response-curve-editor" | "lfo-curve") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.001
                && node.rect.col >= panel.rect.col
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.001,
                "{} outside panel: {:?} / {:?}", node.widget_type, node.rect, panel.rect);
        }
        for child in &node.children { visible(child, panel); }
    }
    let mut seen = std::collections::HashSet::new();
    for section in 0..6 {
        editor.runtime_mut().eval_str(&format!("((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false)")).unwrap();
        editor.refresh_runtime_side_effects();
        let current = editor.widget_layout().unwrap();
        let panel = find_layout_node_by_debug_name(&current, "instrument-panel").unwrap();
        let display = find_layout_node_by_debug_name(&current, "kick-display").unwrap();
        visible(panel, panel);
        visible(display, display);
        for line in dsp.lines().map(str::trim).filter(|line| line.starts_with("(param ")) {
            let name = line.split_whitespace().nth(1).unwrap();
            if find_param(&current, name).is_some() { seen.insert(name.to_string()); }
        }
        let visual = if section == 1 { "kick-envelope" } else if section == 4 { "kick-bank-sweep" }
            else if section == 5 { "kick-tone-curve" } else { "kick-source" };
        let visual = find_layout_node_by_debug_name(display, visual).expect("contextual visual");
        assert_finite_nonzero_rect(visual, "visual");
        if section == 0 {
            if instrument != "808 Kick" {
                assert!(matches!(visual.props.get("odd"), Some(Value::ReactiveRef { .. })));
            }
            if instrument == "909 Kick" {
                assert!(matches!(visual.props.get("curvature"), Some(Value::ReactiveRef { .. })));
            }
            for prop in ["tune", "ratio", "sweep", "decay", "attack", "sustain", "amp"] {
                assert!(matches!(visual.props.get(prop), Some(Value::ReactiveRef { .. })), "bound {prop}");
            }
            for locked in [false, true] {
                editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                editor.drain_host_commands();
                editor.runtime_mut().invoke(visual.props["on-drag"].clone(), vec![
                    Value::Number(-0.5), Value::Number(-0.82), Value::Keyword("sweep".into())
                ]).unwrap();
                let commands = editor.drain_host_commands();
                let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                let payload = commands.iter().find_map(|command| match command {
                    eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } if name == expected => Some(payload),
                    _ => None,
                }).unwrap_or_else(|| panic!("expected {expected}: {commands:?}"));
                let index = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                    .position(|line| line.split_whitespace().nth(1) == Some("sweep")).unwrap();
                assert_eq!(*payload["param-idx"].borrow(), Value::Number(index as f64));
                let Value::Number(value) = *payload["value"].borrow() else { panic!("value"); };
                assert!((value - 1200.0 * (0.44_f64 / 1.88).powi(2)).abs() < 0.001);
            }
            editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
            editor.runtime_mut().set_reactive("SEQ", "kick808-test-sweep", Value::Number(300.0));
            editor.runtime_mut().run_reactive_cycle();
            let Value::ReactiveRef { slot, .. } = &visual.props["sweep"] else { panic!("binding"); };
            assert_eq!(eseqlisp::reactive::read_float_slot(slot), 300.0);
        }
        if section == 2 || section == 3 {
            let curve = visual;
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
        if section == 1 {
            for prop in ["attack", "decay", "sustain", "release"] {
                assert!(matches!(visual.props.get(prop), Some(Value::ReactiveRef { .. })), "bound envelope {prop}");
            }
            editor.drain_host_commands();
            let event = Value::Map([("attack", 12.0), ("decay", 230.0), ("sustain", 0.45), ("release", 340.0)]
                .into_iter().map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(Value::Number(value))))).collect());
            editor.runtime_mut().invoke(visual.props["on-change"].clone(), vec![event]).unwrap();
            assert!(editor.drain_host_commands().iter().any(|command| matches!(command,
                eseqlisp::host::HostCommand::Custom { name, .. } if name == "set-instrument-param-batch")));
        }
        if section == 4 {
            assert!(find_layout_node_by_debug_name(display, "kick-tracking").is_some());
            for locked in [false, true] {
                editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                for (region, param, u, sy, dx, dy) in [
                    ("depth", "bank_env", 0.0, 0.72 - 1.44 * 0.34, 0.0, -0.20),
                    ("floor", "bank_freq", 1.0, 0.72 - 1.44 * 0.03, 0.0, -0.20),
                    ("time", "bank_time", (260.0_f64 / 13815.5106).sqrt(), 0.72 - 1.44 * (0.03 + 0.31 / std::f64::consts::E), 0.10, 0.0),
                ] {
                    let rect = visual.rect;
                    let x = rect.col + rect.width * (0.03 + 0.94 * u as f32);
                    let y = rect.row + rect.height * ((sy as f32 + 1.0) * 0.5);
                    let dims = editor.runtime_mut().layout_cell_dims();
                    assert_eq!(eseqlisp::widget_render::sdf_widget::sdf_begin_gesture(visual, x, y, dims.0, dims.1), Value::Keyword(region.into()), "hit {region}");
                    editor.drain_host_commands();
                    for (kind, px, py) in [
                        (crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), x, y),
                        (crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left), x + dx * rect.width * 0.5, y + dy * rect.height * 0.5),
                        (crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left), x + dx * rect.width * 0.5, y + dy * rect.height * 0.5),
                    ] {
                        editor.handle_mouse_precise(crossterm::event::MouseEvent {
                            kind, column: px.floor() as u16, row: py.floor() as u16,
                            modifiers: crossterm::event::KeyModifiers::NONE,
                        }, 0, 0, 180, 18, px, py);
                    }
                    let idx = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                        .position(|line| line.split_whitespace().nth(1) == Some(param)).unwrap();
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    let commands = editor.drain_host_commands();
                    assert!(commands.iter().any(|command| match command {
                        eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } if name == expected => {
                            *payload["param-idx"].borrow() == Value::Number(idx as f64)
                        }
                        _ => false,
                    }), "sweep {region} {expected}: {commands:?}");
                }
            }
            editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
            for param in ["bank_harm", "bank_crunch", "bank_drive", "bank_recon"] {
                let mini = find_layout_node_by_debug_name(display, &format!("kick-bank-mini-{param}")).unwrap();
                assert_finite_nonzero_rect(mini, "bank diagram");
                assert!(matches!(mini.props.get("value"), Some(Value::ReactiveRef { .. })));
                editor.drain_host_commands();
                editor.runtime_mut().invoke(mini.props["on-drag"].clone(), vec![Value::Number(0.0), Value::Number(0.0), Value::Keyword("value".into())]).unwrap();
                assert!(!editor.drain_host_commands().is_empty(), "diagram edit {param}");
            }
            for param in ["bank_time", "bank_freq", "bank_harm", "bank_crunch", "bank_drive", "bank_recon"] {
                let tree = editor.widget_layout().unwrap();
                let group = find_param(&tree, param).unwrap();
                let picker = find_layout_node_by_widget_type(group, "number-picker").unwrap();
                let expected_step = if param == "bank_harm" { 0.5 } else if param == "bank_time" { 1.0 }
                    else if param == "bank_freq" { 0.001 } else { 0.01 };
                let Value::Number(step) = picker.props["step"] else { panic!("step"); };
                assert!((step - expected_step).abs() < 1e-9, "{param} step {step}");
                let Value::ReactiveRef { slot, .. } = &picker.props["value"] else { panic!("bound picker"); };
                let start = eseqlisp::reactive::read_float_slot(slot) as f64;
                let Value::Number(max) = picker.props["max"] else { panic!("max"); };
                let x = picker.rect.col + picker.rect.width * 0.5;
                let y = picker.rect.row + picker.rect.height * 0.5;
                let end = if start >= max { y + 3.0 } else { y - 3.0 };
                editor.drain_host_commands();
                for (kind, row) in [
                    (crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), y),
                    (crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left), end),
                    (crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left), end),
                ] {
                    editor.handle_mouse_precise(crossterm::event::MouseEvent {
                        kind, column: x.floor() as u16, row: row.max(0.0).floor() as u16,
                        modifiers: crossterm::event::KeyModifiers::NONE,
                    }, 0, 0, 180, 18, x, row);
                }
                let idx = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                    .position(|line| line.split_whitespace().nth(1) == Some(param)).unwrap();
                let commands = editor.drain_host_commands();
                assert!(commands.iter().any(|command| match command {
                    eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } if name == "set-instrument-param" => {
                        *payload["param-idx"].borrow() == Value::Number(idx as f64)
                            && matches!(*payload["value"].borrow(), Value::Number(value) if (value - start).abs() > 1e-7)
                    }
                    _ => false,
                }), "drag {param}: {commands:?}");
            }
            seen.insert("bank_track".into());
            for prop in ["floor", "depth", "duration", "track"] {
                assert!(matches!(visual.props.get(prop), Some(Value::ReactiveRef { .. })));
            }
        }
    }
    for line in dsp.lines().map(str::trim).filter(|line| line.starts_with("(param ")) {
        let name = line.split_whitespace().nth(1).unwrap();
        assert!(seen.contains(name), "missing parameter {name}");
    }
}
