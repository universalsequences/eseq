use super::*;
use eseqlisp::widget_render::drift_waveform::{DRIFT_WAVEFORM_WIDGET, evaluation_count};
use eseqlisp::widget_render::{GpuPrimitive, WidgetDefinition, WidgetViewport};

#[test]
fn digidrift_preview_layout_live_bindings_and_idle_probe() {
    let ui = std::fs::read_to_string(
        sequencer::app_paths::app_paths()
            .factory_root()
            .join("instruments/Synths/Digi Drift/ui.lisp"),
    )
    .expect("Digi Drift UI");
    let source = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".into(),
        "instruments/Synths/Digi Drift/ui.lisp".into(),
        ui,
    )));
    let params = [
        ("osc1_wave", 4.0, 0.0, 6.0),
        ("osc1_shape", 0.5, 0.0, 1.0),
        ("osc1_octave", 0.0, -3.0, 3.0),
        ("osc1_on", 1.0, 0.0, 1.0),
        ("osc1_gain_db", -6.0, -36.0, 12.0),
        ("osc2_wave", 3.0, 0.0, 4.0),
        ("osc2_octave", -1.0, -3.0, 3.0),
        ("osc2_detune", 0.0, -12.0, 12.0),
        ("osc2_on", 1.0, 0.0, 1.0),
        ("osc2_gain_db", -6.0, -36.0, 12.0),
        ("noise_gain_db", -60.0, -60.0, 12.0),
        ("lp_freq", 2500.0, 20.0, 18000.0),
    ];
    let mut inst = test_instrument_map();
    inst.insert(
        "synth".into(),
        Rc::new(RefCell::new(test_list(
            params
                .iter()
                .enumerate()
                .map(|(i, (name, default, min, max))| {
                    let mut p = test_param_map(name, i, *default, *min, *max);
                    p.insert(
                        "value-field".into(),
                        Rc::new(RefCell::new(Value::String(format!("preview-{name}")))),
                    );
                    p.insert(
                        "mod-offset-field".into(),
                        Rc::new(RefCell::new(Value::String(format!("preview-{name}-mod")))),
                    );
                    Value::Map(p)
                })
                .collect(),
        ))),
    );
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
            ("instrument-panel", test_list(vec![Value::Map(inst)])),
            ("bus-effects", test_list(vec![])),
        ],
        true,
    );
    for (name, default, _, _) in params {
        editor.runtime_mut().set_reactive(
            "SEQ",
            &format!("preview-{name}"),
            Value::Number(default),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            &format!("preview-{name}-mod"),
            Value::Number(0.0),
        );
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
        .unwrap();
    register_test_delete_target_natives(&mut editor, 1);
    editor.runtime_mut().eval_str(&source).unwrap();
    editor
        .runtime_mut()
        .eval_str(&read_ui_source("effects.lisp").unwrap())
        .unwrap();
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("{status}");
    }
    let fx = editor.buffers.iter().find(|b| b.name == "*fx*").unwrap().id;
    editor.set_active_buffer(fx);
    let layout = editor.widget_layout().unwrap();
    let node = find_layout_node_by_widget_type(&layout, "drift-waveform")
        .expect("preview is a widget, not a binding diagnostic");
    let panel = find_layout_node_by_debug_name(&layout, "instrument-panel").unwrap();
    let r = node.rect;
    assert!(r.width.is_finite() && r.height.is_finite() && r.width > 0.0 && r.height > 0.0);
    assert!(r.col >= panel.rect.col && r.row >= panel.rect.row);
    assert!(r.col + r.width <= panel.rect.col + panel.rect.width);
    assert!(r.row + r.height <= panel.rect.row + panel.rect.height);
    for prop in DRIFT_WAVEFORM_WIDGET.bindable_props() {
        assert!(
            matches!(node.props.get(*prop), Some(Value::ReactiveRef { .. })),
            "{prop}"
        );
    }
    let viewport = WidgetViewport {
        cell_w: 10.0,
        cell_h: 20.0,
        vp_w: 1800.0,
        vp_h: 360.0,
        time_seconds: 0.0,
        focused_widget_id: None,
        focused_branch: false,
        overlay_viewport_bottom: 18.0,
        scroll_top: 0.0,
        scroll_left: 0.0,
        inherited_hover: false,
    };
    let draw = || DRIFT_WAVEFORM_WIDGET.build_primitives("drift-waveform", node, viewport);
    let points = |primitives: Vec<GpuPrimitive>| -> Vec<[f32; 2]> {
        primitives
            .into_iter()
            .flat_map(|p| match p {
                GpuPrimitive::ForegroundMesh(m) => {
                    m.vertices.into_iter().map(|v| v.point).collect()
                }
                _ => vec![],
            })
            .collect()
    };
    let initial = points(draw());
    let warm = evaluation_count();
    let start = std::time::Instant::now();
    for i in 0..120 {
        editor
            .runtime_mut()
            .set_reactive("SEQ", "ui_epoch", Value::Number(i as f64));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "preview-lp_freq",
            Value::Number(500.0 + i as f64),
        );
        assert_eq!(points(draw()), initial);
    }
    assert_eq!(
        evaluation_count(),
        warm,
        "idle/filter frames must not evaluate oscillators"
    );
    eprintln!(
        "[digidrift-preview-probe] 120 draws {:?}, oscillator evaluations=0",
        start.elapsed()
    );
    for (field, offset) in [
        ("osc1_shape", 0.3),
        ("osc1_gain_db", 6.0),
        ("osc2_detune", 4.0),
        ("osc2_gain_db", 6.0),
        ("noise_gain_db", 54.0),
    ] {
        editor.runtime_mut().set_reactive(
            "SEQ",
            &format!("preview-{field}-mod"),
            Value::Number(offset),
        );
        assert_ne!(points(draw()), initial, "live modulation {field}");
        editor.runtime_mut().set_reactive(
            "SEQ",
            &format!("preview-{field}-mod"),
            Value::Number(0.0),
        );
    }
}
