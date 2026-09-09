use super::*;

#[test]
fn poseidon_display_preserves_oscillator_envelope_and_filter_modes() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "instruments/Synths/Poseidon/ui.lisp".to_string(),
        read_factory_source("instruments/Synths/Poseidon/ui.lisp").unwrap(),
    )));
    let mut poseidon_inst = test_instrument_map();
    let dsp = read_factory_source("instruments/Synths/Poseidon/dsp.lisp").unwrap();
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
            let field = format!("poseidon-test-{}", words[1]);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            bindings.push((field, Value::Number(value)));
            Value::Map(param)
        }).collect();
    poseidon_inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));

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
            ("instrument-panel", test_list(vec![Value::Map(poseidon_inst)])),
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
        .expect("load poseidon custom instrument ui");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("poseidon fx lisp status after refresh: {status}");
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
            "adsr-editor" | "wavetable-viewer" | "response-curve-editor" | "lfo-curve") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.001
                && node.rect.col >= panel.rect.col
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.001,
                "{} outside panel: {:?} / {:?}", node.widget_type, node.rect, panel.rect);
        }
        for child in &node.children { visible(child, panel); }
    }
    let index = |param: &str| dsp.lines().map(str::trim).filter(|line| line.starts_with("(param "))
        .position(|line| line.split_whitespace().nth(1) == Some(param)).unwrap() as f64;
    for section in 0..7 {
        let current = editor.widget_layout().unwrap();
        let panel = find_layout_node_by_debug_name(&current, &format!("tri-panel-{section}")).unwrap();
        editor.runtime_mut().invoke(panel.props["on-click"].clone(), vec![Value::Bool(false)]).unwrap();
        editor.refresh_runtime_side_effects();
        for mode in [0, 1] {
            editor.runtime_mut().set_reactive("SEQ", "poseidon-test-filter_mode", Value::Number(mode as f64));
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let current = editor.widget_layout().unwrap();
            let panel = find_layout_node_by_debug_name(&current, "instrument-panel").unwrap();
            let display = find_layout_node_by_debug_name(&current, "tri-detail-display").unwrap();
            visible(panel, panel);
            visible(display, display);
            for param in ["osc1_set", "osc1_wave", "osc1_gain_db", "osc2_set", "osc2_wave", "osc2_gain_db",
                "cutoff", "resonance", "drive", "vel_to_amp", "voice_pan", "volume_db", "lfo1_rate_hz", "lfo2_rate_hz"] {
                assert!(find_param(&current, param).is_some(), "persistent {param}");
            }
            assert_eq!(find_layout_node_by_widget_type(display, "wavetable-viewer").is_some(), section <= 1);
            assert_eq!(find_layout_node_by_widget_type(display, "lfo-curve").is_some(), section == 4 || section == 5);
            assert_eq!(find_layout_node_by_widget_type(display, "response-curve-editor").is_some(), section == 2);
            if section <= 1 {
                let prefix = format!("osc{}", section + 1);
                for suffix in ["octave", "vel_wave", "warp", "fold"] {
                    assert!(find_param(display, &format!("{prefix}_{suffix}")).is_some());
                }
            }
            if section == 2 {
                let curve = find_layout_node_by_debug_name(display, "tri-filter-response").unwrap();
                let Value::List(bands) = &curve.props["bands"] else { panic!("bands"); };
                assert_eq!(bands.len(), if mode == 1 { 2 } else { 1 });
                assert_eq!(find_param(display, "hp_freq").is_some(), mode == 1);
            }
            if section == 2 || section == 3 {
                let env = find_layout_node_by_debug_name(display, "tri-envelope").unwrap();
                let prefix = if section == 2 { "feg" } else { "aeg" };
                for (prop, suffix) in [("attack", "attack_ms"), ("decay", "decay_ms"), ("sustain", "sustain"), ("release", "release_ms")] {
                    let Value::ReactiveRef { field, .. } = &env.props[prop] else { panic!("bound envelope {prop}"); };
                    assert_eq!(field, &format!("poseidon-test-{prefix}_{suffix}"));
                }
                editor.drain_host_commands();
                let event = Value::Map([("attack", 12.0), ("decay", 230.0), ("sustain", 0.45), ("release", 340.0)]
                    .into_iter().map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(Value::Number(value))))).collect());
                editor.runtime_mut().invoke(env.props["on-change"].clone(), vec![event]).unwrap();
                let commands = editor.drain_host_commands();
                let [eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }] = commands.as_slice() else { panic!("ADSR batch: {commands:?}"); };
                assert_eq!(name, "set-instrument-param-batch");
                let updates = payload["updates"].borrow();
                let Value::List(updates) = &*updates else { panic!("updates"); };
                assert_eq!(updates.len(), 4);
                for (update, suffix) in updates.iter().zip(["attack_ms", "decay_ms", "sustain", "release_ms"]) {
                    let update = update.borrow();
                    let Value::Map(update) = &*update else { panic!("update"); };
                    assert_eq!(*update["param-idx"].borrow(), Value::Number(index(&format!("{prefix}_{suffix}"))));
                }
            }
            if section == 4 || section == 5 {
                let number = section - 3;
                for suffix in ["wave", "keysync", "fade_ms", "to_cutoff"] {
                    assert!(find_param(display, &format!("lfo{number}_{suffix}")).is_some());
                }
                for suffix in ["src", "dest", "amt"] {
                    assert!(find_param(display, &format!("ams{number}_{suffix}")).is_some());
                }
            }
            if section == 6 {
                for param in ["peg_amt_st", "peg_attack_ms", "peg_decay_ms"] {
                    assert!(find_param(display, param).is_some());
                }
            }
        }
    }
    // Editing persistent controls selects their owning panel and preserves
    // the ordinary parameter route rather than only changing visual state.
    for (param, value, owner) in [("osc2_wave", 3.0, 1), ("cutoff", 1400.0, 2),
        ("volume_db", -8.0, 3), ("lfo1_rate_hz", 2.0, 4), ("lfo2_rate_hz", 0.5, 5)] {
        let current = editor.widget_layout().unwrap();
        let control = find_param(&current, param).unwrap();
        let knob = find_layout_node_by_widget_type(control, "knob-number").unwrap();
        editor.drain_host_commands();
        editor.runtime_mut().invoke(knob.props["on-change"].clone(), vec![Value::Number(value)]).unwrap();
        let commands = editor.drain_host_commands();
        let [eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }] = commands.as_slice() else { panic!("knob command: {commands:?}"); };
        assert_eq!(name, "set-instrument-param");
        assert_eq!(*payload["param-idx"].borrow(), Value::Number(index(param)));
        assert_eq!(*payload["value"].borrow(), Value::Number(value));
        editor.refresh_runtime_side_effects();
        // Read the scoped selection via the same dispatcher used by the panel.
        let selected = editor.runtime_mut().eval_str(r#"
            (do (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
                eseq.vanilla/custom-ui-selected-section)
        "#).unwrap();
        assert_eq!(selected, Some(Value::Number(owner as f64)));
    }
}
