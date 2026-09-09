use super::*;

#[test]
fn grit_context_surface() { check_mnm_surface("Grit", 5); }

#[test]
fn vox_context_surface() { check_mnm_surface("Vox", 6); }

fn check_mnm_surface(instrument: &str, pages: usize) {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        format!("instruments/Synths/{instrument}/ui.lisp"),
        read_factory_source(&format!("instruments/Synths/{instrument}/ui.lisp")).unwrap(),
    )));
    let mut melt_inst = test_instrument_map();
    let dsp = read_factory_source(&format!("instruments/Synths/{instrument}/dsp.lisp")).unwrap();
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
            let field = format!("mnm-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    melt_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(melt_inst)])),
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
        .expect("load melt custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("melt fx lisp status after refresh: {status}");
    }

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx lisp should create the *fx* buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor.set_layout_viewport(180, 18);




    fn visit<'a>(node: &'a eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode,
        controls: &mut Vec<&'a eseqlisp::layout::LayoutNode>, fields: &mut std::collections::HashSet<String>) {
        if matches!(node.widget_type.as_str(), "number-picker" | "knob-number" | "dropdown" | "adsr-editor" | "response-curve-editor")
            || (node.widget_type.starts_with("mnm-") || node.widget_type.starts_with("vox-")) {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.01,
                "{} {:?} outside panel {:?}", node.widget_type, node.props.get("debug-name"), panel.rect);
            controls.push(node);
        }
        for value in node.props.values() {
            if let Value::ReactiveRef { field, .. } = value { fields.insert(field.clone()); }
        }
        for child in &node.children { visit(child, panel, controls, fields); }
    }
    let mut seen = std::collections::HashSet::new();
    for section in 0..pages {
        editor.runtime_mut().eval_str(&format!(
            "((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false)")).unwrap();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() { panic!("{status}"); }
        let layout = editor.widget_layout().unwrap();
        let panel = find_layout_node_by_debug_name(&layout, "instrument-panel").unwrap();
        let display = find_layout_node_by_debug_name(&layout, "mnm-display").unwrap();
        assert_finite_nonzero_rect(display, "context display");
        let mut controls = Vec::new();
        visit(panel, panel, &mut controls, &mut seen);
        for control in controls {
            if control.widget_type == "adsr-editor" && (section == 2 || section == 3) {
                let release = control.props.get("debug-name") == Some(&Value::String("mnm-release".into()));
                assert_eq!(control.props.get("mode"), Some(&Value::Keyword(if release { "decay" } else { "ahd" }.into())));
                let stages = if release { vec!["time"] } else if section == 2 { vec!["attack", "decay"] } else { vec!["attack", "hold", "decay"] };
                for stage in &stages { assert!(matches!(control.props.get(*stage), Some(Value::ReactiveRef { .. }))); }
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    for active in [true, false] {
                        let event = editor.runtime_mut().eval_str(&format!(
                            "(dict :attack 20 :hold 100 :decay 600 :time 500 :active {})",
                            if active { ":decay" } else { "false" })).unwrap().unwrap();
                        editor.drain_host_commands();
                        editor.runtime_mut().invoke(control.props["on-change"].clone(), vec![event]).unwrap();
                        let commands = editor.drain_host_commands();
                        assert_eq!(commands.len(), 1, "one native envelope edit: {commands:?}");
                        let eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) } = &commands[0] else { panic!("envelope batch"); };
                        assert_eq!(name, if locked { "set-instrument-plock-batch" } else { "set-instrument-param-batch" });
                        assert_eq!(*payload["commit"].borrow(), Value::Bool(!active));
                        let Value::List(updates) = &*payload["updates"].borrow() else { panic!("updates"); };
                        assert_eq!(updates.len(), stages.len());
                        let names: Vec<_> = dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
                            .map(|line| line.split_whitespace().nth(1).unwrap()).collect();
                        for (update, stage) in updates.iter().zip(&stages) {
                            let Value::Map(update) = &*update.borrow() else { panic!("update"); };
                            let Value::Number(index) = *update["param-idx"].borrow() else { panic!("index"); };
                            let expected = if release { "amp_release_ms".to_string() } else { format!("{}_{}_ms", if section == 2 { "fenv" } else { "amp" }, stage) };
                            assert_eq!(names[index as usize], expected);
                        }
                    }
                }
            }
            if control.props.contains_key("on-mouse-down") && control.props.contains_key("on-drag") {
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    let x = control.rect.col + control.rect.width * 0.6;
                    let y = control.rect.row + control.rect.height * 0.5;
                    for (kind, px) in [
                        (crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), x),
                        (crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left), x + control.rect.width * 0.1),
                        (crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left), x + control.rect.width * 0.1),
                    ] {
                        editor.handle_mouse_precise(crossterm::event::MouseEvent { kind, column: px as u16, row: y as u16,
                            modifiers: crossterm::event::KeyModifiers::NONE }, 0, 0, 180, 18, px, y);
                    }
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    let commands = editor.drain_host_commands();
                    assert!(commands.iter().any(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)), "{commands:?}");
                }
            }
            if control.widget_type == "response-curve-editor" {
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    let mut event = [("id", 0.0), ("freq", 1000.0), ("gain", 3.0), ("q", 2.0)]
                        .into_iter().map(|(k,v)| (k.to_string(), Rc::new(RefCell::new(Value::Number(v)))))
                        .collect::<std::collections::HashMap<_, _>>();
                    event.insert("type".into(), Rc::new(RefCell::new(Value::Keyword("change-band".into()))));
                    editor.runtime_mut().invoke(control.props["on-action"].clone(), vec![Value::Map(event)]).unwrap();
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)));
                }
            }
            if matches!(control.widget_type.as_str(), "knob-number" | "number-picker")
                && control.props.get("debug-name") != Some(&Value::String("mnm-base-note".into())) {
                assert!(matches!(control.props.get("value"), Some(Value::ReactiveRef { .. })));
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    editor.runtime_mut().invoke(control.props["on-change"].clone(), vec![Value::Number(0.5)]).unwrap();
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)));
                }
            }
        }
    }
    for line in dsp.lines().map(str::trim).filter(|line| line.starts_with("(param ")) {
        let name = line.split_whitespace().nth(1).unwrap();
        assert!(seen.contains(&format!("mnm-test-{name}")), "unreachable parameter {name}");
    }
}
