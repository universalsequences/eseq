use super::*;

#[test]
fn custom_controls_keep_owners_across_step_selection() {
    let src = read_ui_source("effects.lisp").expect("read fx lisp");
    let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(None);
    let custom_instrument_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".to_string(),
        "instruments/test-instrument/ui.lisp".to_string(),
        r#"
            (defsynth-ui
              (h-stack :gap 0.2
                (eseq.effects.custom-ui-controls/ui-param-knob "cutoff" "cut")
                (eseq.effects.custom-ui-controls/ui-param-knob-c "amp_attack" "attack")
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "amp_decay" "decay" 4.4
                  (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
                (eseq.effects.physical-model-surface/control '("amp_sustain" "sustain" 2) 0)))
            "#
        .to_string(),
    )));

    let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
    editor.runtime_mut().register_reactive("SEQV", vec![], true);
    editor.runtime_mut().register_reactive(
        "SEQ",
        vec![
            ("num-tracks", Value::Number(1.0)),
            ("compiling", Value::Bool(false)),
            ("track-plocks", test_list(vec![])),
            ("track-plock-variants", test_list(vec![])),
            (
                "available-effects",
                test_list(vec![
                    Value::String("dimension-d-chorus".to_string()),
                    Value::String("lexilush".to_string()),
                ]),
            ),
            ("available-builtin-effects", test_list(vec![])),
            ("available-midi-effects", test_list(vec![])),
            (
                "bus-names",
                test_list(vec![Value::String("Mix".to_string())]),
            ),
            (
                "effects",
                test_list(vec![
                    Value::Map(test_fx_map(
                        "dimension-d-chorus",
                        0,
                        test_dimension_d_params(),
                    )),
                    Value::Map(test_fx_map("lexilush", 1, test_lexilush_params())),
                ]),
            ),
            ("midi-effects", test_list(vec![])),
            (
                "instrument-panel",
                test_list(vec![Value::Map(test_instrument_map())]),
            ),
            ("bus-effects", test_list(vec![test_list(vec![])])),
        ],
        true,
    );
    editor
            .runtime_mut()
            .eval_str(
                r#"
                (def eseq.seq-core-state/selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def eseq.browser/sbrowser-editor-name "")
                (defmacro eseq.materials/slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (defstate eseq.seq-core-state/selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
    register_test_delete_target_natives(&mut editor, 1);
    editor
        .runtime_mut()
        .eval_str(&custom_instrument_ui_source)
        .expect("load initial custom instrument UI");
    editor
        .runtime_mut()
        .eval_str(&custom_audio_ui_source)
        .expect("load initial custom audio FX UI");
    editor.runtime_mut().eval_str(&src).expect("load fx lisp");
    editor
        .runtime_mut()
        .eval_str(&custom_instrument_ui_source)
        .expect("load custom instrument UI");
    editor
        .runtime_mut()
        .eval_str(&custom_audio_ui_source)
        .expect("load custom audio FX UI");
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("custom instrument with audio FX lisp status after refresh: {status}");
    }

    let fx_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*fx*")
        .expect("fx lisp should create the *fx* buffer")
        .id;
    editor.set_active_buffer(fx_id);
    editor.set_layout_viewport(200, 18);
    let selected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let selected_native = selected.clone();
    editor
        .runtime_mut()
        .register_native("seq-has-selection?", move |_, _| {
            Ok(Value::Bool(
                selected_native.load(std::sync::atomic::Ordering::Relaxed),
            ))
        });
    // Record the native boundary; the effect p-lock path dispatches here
    // instead of using host-command directly.
    editor
        .runtime_mut()
        .register_native("seq-set-effect-plock", |args, ctx| {
            ctx.enqueue_command(eseqlisp::host::HostCommand::Custom {
                name: "set-effect-plock".into(),
                payload: map_value([
                    ("slot-idx", args[0].clone()),
                    ("param-idx", args[1].clone()),
                    ("value", args[2].clone()),
                    ("target-node-id", args[3].clone()),
                ]),
            });
            Ok(args[2].clone())
        });
    // All owners use overlapping parameter indices. Selection only changes
    // the projection fields, so the individual controls rerun independently
    // of the panel functions that established the ambient render scope.
    let controls = [
        (
            "custom-ui-knob-test-instrument-base-cutoff",
            "knob-number",
            "instrument",
            0,
            0,
            0.75,
        ),
        (
            "custom-ui-knob-c-test-instrument-base-amp_attack",
            "knob-number",
            "instrument",
            0,
            1,
            7.0,
        ),
        (
            "custom-ui-lego-knob-test-instrument-base-amp_decay",
            "knob-number",
            "instrument",
            0,
            2,
            150.0,
        ),
        (
            "pm-detail-test-instrument-base-amp_sustain",
            "number-picker",
            "instrument",
            0,
            3,
            0.6,
        ),
        (
            "custom-ui-lego-knob-dimension-d-chorus-slot-0-base-base",
            "knob-number",
            "effect",
            0,
            2,
            14.0,
        ),
        (
            "custom-ui-lego-num-lexilush-slot-1-base-damping",
            "number-picker",
            "effect",
            1,
            4,
            400.0,
        ),
    ];
    // Start with an effect-only lock, matching okay-harmony: the shared
    // variant color invalidates synth knobs even though they have no locks.
    for (is_selected, effect_only) in [
        (false, true),
        (true, true),
        (false, true),
        (true, false),
        (false, false),
        (true, false),
        (false, false),
    ] {
        selected.store(is_selected, std::sync::atomic::Ordering::Relaxed);
        let rows = if is_selected {
            controls
                .iter()
                .filter(|(_, _, target, _, _, _)| !effect_only || *target == "effect")
                .map(|(_, _, target, slot, idx, _)| {
                    map_value([
                        ("target", Value::String((*target).into())),
                        ("slot-idx", Value::Number(*slot as f64)),
                        ("param-idx", Value::Number(*idx as f64)),
                        ("default", Value::Number(0.25)),
                    ])
                })
                .collect()
        } else {
            vec![]
        };
        editor
            .runtime_mut()
            .set_reactive("SEQ", "track-plocks", test_list(rows));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().expect("selected control layout");
        for (key, widget_type, target, slot, idx, value) in controls {
            let wrapper = find_layout_node_by_stable_key(&layout, key)
                .unwrap_or_else(|| panic!("missing {key} after selection={is_selected}"));
            let control = find_layout_node_by_widget_type(wrapper, widget_type)
                .unwrap_or_else(|| panic!("missing {widget_type} in {key}"));
            assert_finite_nonzero_rect(control, key);
            let callback = control
                .props
                .get("on-change")
                .cloned()
                .expect("control callback");
            editor
                .runtime_mut()
                .invoke(callback, vec![Value::Number(value)])
                .expect("edit rendered control");
            let commands = editor.drain_host_commands();
            assert_eq!(commands.len(), 1, "{key}: {commands:?}");
            let eseqlisp::host::HostCommand::Custom {
                name,
                payload: Value::Map(payload),
            } = &commands[0]
            else {
                panic!("expected parameter edit: {commands:?}");
            };
            let suffix = if is_selected { "plock" } else { "param" };
            assert_eq!(
                name,
                &format!("set-{target}-{suffix}"),
                "{key}, selected={is_selected}"
            );
            assert_eq!(
                value_map_number(payload, "param-idx"),
                Some(idx as f64),
                "{key}"
            );
            assert_eq!(value_map_number(payload, "value"), Some(value), "{key}");
            assert_eq!(
                control.props.get("plock-active"),
                Some(&Value::Number(
                    if is_selected && (!effect_only || target == "effect") {
                        1.0
                    } else {
                        0.0
                    }
                )),
                "{key} must follow its own lock projection"
            );
            assert!(
                matches!(control.props.get("value"), Some(Value::Number(value)) if value.is_finite()),
                "{key} must retain its value binding after selection changes"
            );
            if target == "effect" {
                assert_eq!(
                    value_map_number(payload, "slot-idx"),
                    Some(slot as f64),
                    "{key}"
                );
                assert_eq!(
                    value_map_number(payload, "target-node-id"),
                    Some(100.0 + slot as f64),
                    "{key}"
                );
            }
        }
    }
}
