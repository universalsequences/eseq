use super::*;

fn check_surface(instrument: &str, page_count: usize) {
    check_surface_at(instrument, page_count, sequencer::app_paths::app_paths().instruments_dir().join("Drums"));
}

fn check_surface_at(instrument: &str, page_count: usize, root: std::path::PathBuf) {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        format!("instruments/Drums/{instrument}/ui.lisp"),
        std::fs::read_to_string(root.join(format!("{instrument}/ui.lisp"))).unwrap(),
    )));
    let mut hat909_inst = test_instrument_map();
    let dsp = std::fs::read_to_string(root.join(format!("{instrument}/dsp.lisp"))).unwrap();
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



    fn visit(node: &eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode,
        names: &[String], seen: &mut std::collections::HashSet<String>) {
        if let Some(Value::String(debug)) = node.props.get("debug-name") {
            if let Some(name) = debug.strip_prefix("drum-detail-") { seen.insert(name.to_string()); }
        }
        if let Some(key) = &node.stable_key {
            for name in names { if key.ends_with(name) { seen.insert(name.clone()); } }
        }
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker" | "dropdown" | "adsr-editor")
            || node.widget_type.starts_with("eseq-drum-") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.01,
                "{} outside visible panel", node.widget_type);
        }
        for child in &node.children { visit(child, panel, names, seen); }
    }
    fn controls<'a>(node: &'a eseqlisp::layout::LayoutNode, result: &mut Vec<&'a eseqlisp::layout::LayoutNode>) {
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker") { result.push(node); }
        for child in &node.children { controls(child, result); }
    }
    let required: Vec<String> = dsp.lines().filter(|line| line.contains("@mod true"))
        .map(|line| line.split_whitespace().nth(1).unwrap().to_string()).collect();
    let mut seen = std::collections::HashSet::new();
    let engines = dsp.lines().find(|line| line.starts_with("(param engine "))
        .map(|line| {
            let words: Vec<_> = line.trim_end_matches(')').split_whitespace().collect();
            let max = words.iter().position(|word| *word == "@max").unwrap();
            words[max + 1].parse::<usize>().unwrap()
        }).unwrap_or(1);
    for engine in 1..=engines {
        if engines > 1 {
            editor.runtime_mut().set_reactive("SEQ", "hat909-test-engine", Value::Number(engine as f64));
            editor.runtime_mut().run_reactive_cycle();
        }
        for section in 0..page_count {
            editor.runtime_mut().eval_str(&format!("((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false)")).unwrap();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().unwrap();
            let panel = find_layout_node_by_debug_name(&layout, "instrument-panel").expect(instrument);
            let display = find_layout_node_by_debug_name(&layout, "drum-display").expect("center display");
            assert_finite_nonzero_rect(display, "center display");
            for page in 0..page_count {
                let button = find_layout_node_by_debug_name(display, &format!("drum-page-{page}"));
                let removed = (instrument == "Virus B BassDrum 23" && (page == 0 || page == 3))
                    || (instrument == "Orbit Tom 66" && (page == 1 || page == 3));
                if removed { assert!(button.is_none(), "redundant page"); }
                else { assert_finite_nonzero_rect(button.expect("page selector"), "page selector"); }
            }
            visit(panel, panel, &required, &mut seen);
            if instrument == "Membrane Snare" && section == 1 {
                let strike = find_layout_node_by_debug_name(display, "drum-strike").unwrap();
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    editor.runtime_mut().invoke(strike.props["on-drag"].clone(), vec![Value::Number(0.2), Value::Number(-0.4), Value::Keyword("head".into())]).unwrap();
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    let commands = editor.drain_host_commands();
                    assert_eq!(commands.iter().filter(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)).count(), 2);
                }
            }
            fn curves<'a>(node: &'a eseqlisp::layout::LayoutNode, out: &mut Vec<&'a eseqlisp::layout::LayoutNode>) {
                if node.props.contains_key("on-mouse-down") && node.props.contains_key("on-drag") { out.push(node); }
                for child in &node.children { curves(child, out); }
            }
            let mut editable = Vec::new(); curves(display, &mut editable);
            for curve in editable {
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
            let mut nodes = Vec::new(); controls(panel, &mut nodes);
            assert!(nodes.len() >= 8);
            let knob_fields: Vec<_> = nodes.iter().filter(|node| node.widget_type == "knob-number")
                .filter_map(|node| match node.props.get("value") {
                    Some(Value::ReactiveRef { field, .. }) => Some(field),
                    _ => None,
                }).collect();
            assert_eq!(knob_fields.len(), 8);
            for node in &nodes {
                if node.widget_type == "number-picker" {
                    if let Some(Value::ReactiveRef { field, .. }) = node.props.get("value") {
                        assert!(!knob_fields.contains(&field), "duplicate detail binding {field}");
                    }
                }
            }
            for node in nodes {
                assert!(matches!(node.props.get("value"), Some(Value::ReactiveRef { .. })), "{} bound", node.widget_type);
                for locked in [false, true] {
                    editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                    editor.drain_host_commands();
                    editor.runtime_mut().invoke(node.props["on-change"].clone(), vec![Value::Number(0.5)]).unwrap();
                    let expected = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                    assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
                        eseqlisp::host::HostCommand::Custom { name, .. } if name == expected)), "{instrument} {section}");
                }
            }
        }
    }
    for name in required { assert!(seen.contains(&name), "{instrument}: missing performance control {name}"); }
}

#[test]
fn digi_clap_surface() { check_surface("Digi Clap", 4); }

#[test]
fn digi_hat_surface() { check_surface("Digi Hat", 4); }

#[test]
fn digi_cymbal_surface() { check_surface("Digi Cymbal", 4); }

#[test]
fn digi_snare_surface() { check_surface("Digi Snare", 4); }

#[test]
fn membrane_snare_surface() { check_surface("Membrane Snare", 5); }

#[test]
fn modal_kick_surface() { check_surface("Modal Kick", 6); }

#[test]
fn orbit_tom_66_surface() { check_surface("Orbit Tom 66", 4); }

#[test]
fn r8_kick_03_surface() {
    check_surface_at("R8 Kick 03", 4, sequencer::app_paths::app_paths()
        .dev_instrument_fixtures_dir().expect("development instrument fixtures").join("drums"));
}

#[test]
fn virus_b_bassdrum_23_surface() { check_surface("Virus B BassDrum 23", 6); }
