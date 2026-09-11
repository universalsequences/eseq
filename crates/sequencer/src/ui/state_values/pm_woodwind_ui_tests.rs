use super::*;

fn check_woodwind_surface(instrument: &str, pages: usize) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/instruments/Physical Models").join(instrument);
    // The compiler is authoritative for grouped names, nested declarations,
    // ranges and defaults. Hand-parsed source misses the flute's inline params.
    let dsp = std::fs::read_to_string(root.join("dsp.lisp")).unwrap();
    let compiled = sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
        &dsp, 48000, Some(&root)).expect("compile factory woodwind manifest");
    let mut values = Vec::new();
    let mut expected = std::collections::HashSet::new();
    let mut params: Vec<Value> = compiled.manifest.params.iter().enumerate().filter(|(_, p)| !p.hidden)
        .map(|(index, p)| {
            assert!(p.default >= p.min && p.default <= p.max, "{} default must be editable", p.name);
            expected.insert(p.name.clone());
            let mut param = test_param_map(&p.name, index, p.default as f64, p.min as f64, p.max as f64);
            let field = format!("pm-test-{}", p.name);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            values.push((field, Value::Number(p.default as f64)));
            Value::Map(param)
        }).collect();
    params.push(Value::Map(test_base_note_param_map(compiled.manifest.params.len())));
    let mut inst = test_instrument_map();
    inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));
    let ui = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".into(), root.join("ui.lisp").display().to_string(),
        std::fs::read_to_string(root.join("ui.lisp")).unwrap(),
    )));
    let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
    editor.set_layout_viewport(140, 22);
    editor.runtime_mut().register_reactive("SEQ", vec![
        ("num-tracks", Value::Number(1.0)), ("compiling", Value::Bool(false)),
        ("available-effects", test_list(vec![])), ("available-builtin-effects", test_list(vec![])),
        ("available-midi-effects", test_list(vec![])), ("bus-names", test_list(vec![])),
        ("effects", test_list(vec![])), ("midi-effects", test_list(vec![])),
        ("instrument-panel", test_list(vec![Value::Map(inst)])), ("bus-effects", test_list(vec![])),
    ], true);
    for (field, value) in values { editor.runtime_mut().set_reactive("SEQ", &field, value); }
    editor.runtime_mut().eval_str(r#"
        (def eseq.seq-core-state/selected-bus-name () "Mix")
        (def seq-has-selection? () false)
        (def eseq.browser/sbrowser-editor-name "")
        (defmacro eseq.materials/slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
        (def custom-midi-fx-ui (fx) false)
        (def custom-audio-fx-ui (fx) false)
        (defstate eseq.seq-core-state/selected-bus -1)
    "#).unwrap();
    register_test_delete_target_natives(&mut editor, 1);
    editor.runtime_mut().eval_str(&ui).expect("load woodwind UI");
    editor.runtime_mut().eval_str(&read_ui_source("effects.lisp").unwrap()).unwrap();
    editor.refresh_runtime_side_effects();
    let fx = editor.buffers.iter().find(|buffer| buffer.name == "*fx*").unwrap().id;
    editor.set_active_buffer(fx);

    fn visit<'a>(node: &'a eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode,
        controls: &mut Vec<&'a eseqlisp::layout::LayoutNode>)
    {
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker" | "adsr-editor" | "button" | "label")
            || node.widget_type.starts_with("pm-") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row && node.rect.col >= panel.rect.col
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.01
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.01,
                "{} clipped: {:?} in {:?}", node.widget_type, node.rect, panel.rect);
        }
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker") { controls.push(node); }
        for child in &node.children { visit(child, panel, controls); }
    }
    let mut seen = std::collections::HashSet::new();
    for section in 0..pages {
        editor.runtime_mut().eval_str(&format!(
            "((eseq.effects.custom-ui-sections/ui-section-select-callback {section}) false)")).unwrap();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() { panic!("{instrument} page {section}: {status}"); }
        let layout = editor.widget_layout().unwrap();
        let base_note = find_layout_node_by_debug_name(&layout, "instrument-base-note")
            .expect("every instrument page exposes the host base note");
        assert_finite_nonzero_rect(base_note, "base note");
        editor.drain_host_commands();
        editor.runtime_mut().invoke(base_note.props["on-change"].clone(), vec![Value::Number(12.0)]).unwrap();
        assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
            eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }
                if name == "set-instrument-base-note" && *payload["value"].borrow() == Value::Number(12.0))));
        let panel = find_layout_node_by_debug_name(&layout, "pm-surface").expect("physical model surface");
        let display = find_layout_node_by_debug_name(panel, "pm-display").expect("center display");
        assert_finite_nonzero_rect(panel, "physical model surface");
        assert_finite_nonzero_rect(display, "center display");
        let mut controls = Vec::new();
        visit(panel, panel, &mut controls);
        assert_eq!(controls.iter().filter(|n| n.widget_type == "knob-number").count(), 8);
        for node in controls {
            let Some(Value::ReactiveRef { field, .. }) = node.props.get("value") else { panic!("unbound control"); };
            let name = field.strip_prefix("pm-test-").expect("real parameter binding");
            assert!(expected.contains(name), "unknown parameter {name}");
            seen.insert(name.to_string());
            let index = compiled.manifest.params.iter().position(|p| p.name == name).unwrap();
            for locked in [false, true] {
                editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                editor.drain_host_commands();
                editor.runtime_mut().invoke(node.props["on-change"].clone(), vec![Value::Number(0.5)]).unwrap();
                let command = if locked { "set-instrument-plock" } else { "set-instrument-param" };
                let commands = editor.drain_host_commands();
                assert!(commands.iter().any(|cmd| matches!(cmd,
                    eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }
                        if name == command && *payload["param-idx"].borrow() == Value::Number(index as f64))),
                    "{instrument} {name} must edit its own parameter: {commands:?}");
            }
        }
        // Each display page is reachable through its actual button callback.
        for page in 0..pages {
            let button = find_layout_node_by_debug_name(display, &format!("pm-page-{page}")).unwrap();
            editor.runtime_mut().invoke(button.props["on-click"].clone(), vec![Value::Bool(false)]).unwrap();
            assert_eq!(editor.runtime_mut().eval_str("eseq.vanilla/custom-ui-selected-section").unwrap(),
                Some(Value::Number(page as f64)));
        }
        if let Some(env) = find_layout_node_by_debug_name(display, "pm-envelope") {
            for property in ["attack", "decay", "sustain", "release"] {
                assert!(matches!(env.props.get(property), Some(Value::ReactiveRef { .. })), "live envelope {property}");
            }
            for locked in [false, true] {
                editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                editor.drain_host_commands();
                let edit = editor.runtime_mut().eval_str(
                    "(dict :attack 20 :decay 100 :sustain 0.6 :release 300 :active false)").unwrap().unwrap();
                editor.runtime_mut().invoke(env.props["on-change"].clone(), vec![edit]).unwrap();
                let expected_command = if locked { "set-instrument-plock-batch" } else { "set-instrument-param-batch" };
                assert!(editor.drain_host_commands().iter().any(|cmd| matches!(cmd,
                    eseqlisp::host::HostCommand::Custom { name, .. } if name == expected_command)));
            }
        }
        let diagrams: &[(&str, &[&str])] = &[
            ("pm-reed", &["slope"]), ("pm-column", &["position"]),
            ("pm-bore-loss", &["cutoff", "loss"]),
            ("pm-flutter", &["rate", "drift", "depth", "floor"]),
            ("pm-vibrato", &["rate", "depth", "wait"]),
            ("pm-body", &["frequency", "q", "amount"]),
            ("pm-partials", &["frequency", "stretch", "brightness"]),
            ("pm-bow", &["pressure", "curve", "speed"]),
            ("pm-pluck", &["duration", "amount", "texture"]),
            ("pm-cello-body", &["amount", "size", "q", "f1", "g1", "f2", "g2", "f3", "g3", "f4", "g4"]),
            ("pm-section-spread", &["amount", "spread"]),
            ("pm-piano-hammer", &["hardness", "contact", "position"]),
            ("pm-piano-strings", &["decay", "damping", "stiffness", "aftersound"]),
            ("pm-piano-body", &["size", "amount", "color", "low", "high"]),
            ("pm-piano-unison", &["spread", "stereo", "stretch"]),
            ("pm-piano-dampers", &["release", "pedal", "upper"]),
            ("pm-piano-motion", &["depth", "rate", "pan", "panrate"]),
            ("pm-piano-swell", &["amount", "duration", "curve", "tail"]),
            ("pm-piano-output", &["drive", "gain"]),
            ("pm-saron-mallet", &["hardness", "contact", "spread"]),
            ("pm-saron-bar", &["decay", "bloom", "loss"]),
            ("pm-saron-tuning", &["amount", "tune"]),
            ("pm-saron-damper", &["touch", "release", "lift"]),
            ("pm-saron-output", &["drive", "gain"]),
            ("pm-cymbal-body", &["size", "character"]),
            ("pm-cymbal-loss", &["decay", "damping", "touch"]),
            ("pm-cymbal-contact", &["hardness", "touch"]),
            ("pm-cymbal-output", &["bell", "wash", "gain", "width"]),
            ("pm-gamelan-mallet", &["hardness", "contact", "spread"]),
            ("pm-gamelan-body", &["decay", "bloom", "loss"]),
            ("pm-gamelan-tuning", &["amount", "tune"]),
            ("pm-gamelan-damper", &["touch", "release", "lift"]),
        ];
        for (debug, properties) in diagrams {
            if let Some(node) = find_layout_node_by_debug_name(display, debug) {
                for property in *properties {
                    assert!(matches!(node.props.get(*property), Some(Value::ReactiveRef { .. })), "{debug}: live {property}");
                }
            }
        }
        if instrument == "PM Hi-Hat" && section == 2 {
            let contact = find_layout_node_by_debug_name(display, "pm-cymbal-contact").unwrap();
            assert!(matches!(contact.props.get("openness"), Some(Value::ReactiveRef { .. })));
        }
        if instrument == "PM Clarinet" && section == 0 {
            let reed = find_layout_node_by_debug_name(display, "pm-reed").unwrap();
            for property in ["closure", "curvature"] {
                assert!(matches!(reed.props.get(property), Some(Value::ReactiveRef { .. })), "live reed {property}");
            }
        }
        editor.runtime_mut().register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
    }
    assert_eq!(seen, expected, "Every factory parameter is reachable in a visible control");
}

#[test]
fn flute_surface_controls_and_pages() { check_woodwind_surface("PM Flute", 4); }

#[test]
fn slenthem_surface_controls_and_pages() { check_woodwind_surface("PM Slenthem", 5); }

#[test]
fn bonang_surface_controls_and_pages() { check_woodwind_surface("PM Bonang", 5); }

#[test]
fn slenthem_slendro_surface_controls_and_pages() { check_woodwind_surface("PM Slenthem Slendro", 5); }

#[test]
fn kempyang_surface_controls_and_pages() { check_woodwind_surface("PM Kempyang", 5); }

#[test]
fn kethuk_surface_controls_and_pages() { check_woodwind_surface("PM Kethuk", 5); }

#[test]
fn saxophone_surface_controls_and_pages() { check_woodwind_surface("PM Saxophone", 6); }

#[test]
fn clarinet_surface_controls_and_pages() { check_woodwind_surface("PM Clarinet", 5); }

#[test]
fn cello_surface_controls_and_pages() { check_woodwind_surface("PM Cello", 8); }

#[test]
fn piano_surface_controls_and_pages() { check_woodwind_surface("PM Piano", 8); }

#[test]
fn saron_surface_controls_and_pages() { check_woodwind_surface("PM Saron", 5); }

#[test]
fn crash_surface_controls_and_pages() { check_woodwind_surface("PM Crash", 4); }

#[test]
fn ride_surface_controls_and_pages() { check_woodwind_surface("PM Ride", 4); }

#[test]
fn hihat_surface_controls_and_pages() { check_woodwind_surface("PM Hi-Hat", 4); }
