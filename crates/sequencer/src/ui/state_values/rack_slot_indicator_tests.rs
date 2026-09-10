use super::*;
use sequencer::sequencer::{RackMacroCurve, RackMacroId, RackMacroMapping, RackMacroTarget, RackSlotParam};

#[test]
fn rack_slot_macro_indicators_follow_mapping_changes() {
    let mut app = test_app_with_rack_panel();
    {
        let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
        let rack = racks[0].as_mut().unwrap();
        rack.slots.push(rack.slots[0].clone());
    }
    let selected = Arc::new(Mutex::new(HashSet::new()));
    let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
    editor.set_layout_viewport(160, 20);
    editor.runtime_mut().register_reactive(
        "SEQ",
        vec![
            ("num-tracks", Value::Number(1.0)),
            ("compiling", Value::Bool(false)),
            ("tp-gate", Value::Bool(false)),
            ("available-effects", test_list(vec![])),
            ("available-builtin-effects", test_list(vec![])),
            ("available-midi-effects", test_list(vec![])),
            ("bus-names", test_list(vec![])),
            ("effects", test_list(vec![])),
            ("midi-effects", test_list(vec![])),
            ("instrument-panel", build_instrument_panel_value(&app, 0, &selected)),
            ("bus-effects", test_list(vec![])),
            ("delete-target-version", Value::Number(0.0)),
        ],
        true,
    );
    editor.runtime_mut().eval_str(r#"
        (def eseq.seq-core-state/selected-bus-name () "Mix")
        (def seq-has-selection? () false)
        (def eseq.browser/sbrowser-editor-name "")
        (def eseq.browser/sample-selected-path () "")
        (def eseq.browser/add-selected-rack-layer () false)
        (defmacro eseq.materials/slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
        (def custom-midi-fx-ui (fx) false)
        (def custom-audio-fx-ui (fx) false)
        (defstate eseq.seq-core-state/selected-bus -1)
    "#).expect("install rack UI helpers");
    register_test_delete_target_natives(&mut editor, 1);
    editor.runtime_mut().eval_str(&read_ui_source("effects.lisp").unwrap())
        .expect("load rack panel");
    editor.refresh_runtime_side_effects();
    let fx_id = editor.buffers.iter().find(|buffer| buffer.name == "*fx*").unwrap().id;
    editor.set_active_buffer(fx_id);

    let macro_id = RackMacroId::from_index(0).unwrap();
    for mapped_param in RackSlotParam::ALL {
        app.map_rack_macro(0, macro_id, RackMacroMapping {
            target: RackMacroTarget::SlotParam {
                slot: 0,
                param: mapped_param.name().to_string(),
            },
            range_min: 0.0,
            range_max: 1.0,
            curve: RackMacroCurve::Linear,
        }).expect("map rack slot control");
        for mapped in [true, false] {
            if !mapped {
                assert!(app.unmap_rack_macro(0, macro_id, 0));
            }
            editor.runtime_mut().set_reactive(
                "SEQ", "instrument-panel", build_instrument_panel_value(&app, 0, &selected));
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().expect("rack slot indicator layout");
            let panel = find_layout_node_by_debug_name(&layout, "rack-panel").unwrap();
            for slot in 0..2 {
                for param in RackSlotParam::ALL {
                    let key = format!("rack-slot-control-{slot}-{}", param.name());
                    let wrapper = find_layout_node_by_debug_name(panel, &key)
                        .unwrap_or_else(|| panic!("missing {key}"));
                    assert_eq!(layout_prop_number(wrapper, "macro-owned"),
                        Some(if mapped && slot == 0 && param == mapped_param { 1.0 } else { 0.0 }),
                        "wrong indicator for {key} with {mapped_param:?} mapped={mapped}");
                    assert_finite_nonzero_rect(wrapper, &key);
                    assert_layout_inside(wrapper, panel, &key);
                    let widget_type = if matches!(param, RackSlotParam::Mute | RackSlotParam::Solo) {
                        "button"
                    } else {
                        "number-picker"
                    };
                    let control = find_layout_node_by_widget_type(wrapper, widget_type).unwrap();
                    assert_finite_nonzero_rect(control, &key);
                    if widget_type == "number-picker" {
                        assert!(matches!(control.props.get("value"), Some(Value::ReactiveRef { .. })),
                            "{key} must retain its live value binding");
                    }
                }
            }
        }
    }
}
