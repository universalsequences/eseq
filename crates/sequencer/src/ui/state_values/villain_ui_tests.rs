use super::*;

fn villain_root(instrument: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/instruments/Drums").join(instrument)
}

/// One VILLAIN panel's shape: debug-name prefix, detail page ids, the params
/// on the always-visible blocks, params deliberately off the panel, the two
/// member-slot params, and the map props that must follow live params.
struct VillainPanel {
    instrument: &'static str,
    prefix: &'static str,
    pages: &'static [&'static str],
    always_visible: &'static [&'static str],
    not_on_panel: &'static [&'static str],
    slot_a: &'static str,
    slot_b: &'static str,
    map_props: &'static [&'static str],
}

/// A VILLAIN panel: every visible parameter is on exactly one control across
/// the detail pages, nothing is clipped, the family map is bound to the live
/// params, and a click on the map loads the armed member slot.
fn check_villain_panel(panel: VillainPanel) {
    let VillainPanel { instrument, prefix, pages, always_visible, not_on_panel, slot_a, slot_b, map_props } = panel;
    let root = villain_root(instrument);
    let dsp = std::fs::read_to_string(root.join("dsp.lisp")).unwrap();
    let compiled = sequencer::lisp_host::compile_and_load_instrument_with_asset_base(&dsp, 48000, Some(&root))
        .unwrap_or_else(|e| panic!("compile {instrument} manifest: {e:?}"));
    let mut values = Vec::new();
    let mut expected = std::collections::HashSet::new();
    let mut params: Vec<Value> = compiled.manifest.params.iter().enumerate().filter(|(_, p)| !p.hidden)
        .map(|(index, p)| {
            if !not_on_panel.contains(&p.name.as_str()) {
                expected.insert(p.name.clone());
            }
            let mut param = test_param_map(&p.name, index, p.default as f64, p.min as f64, p.max as f64);
            let field = format!("vk-test-{}", p.name);
            param.insert("value-field".into(), Rc::new(RefCell::new(Value::String(field.clone()))));
            values.push((field, Value::Number(p.default as f64)));
            Value::Map(param)
        }).collect();
    params.push(Value::Map(test_base_note_param_map(compiled.manifest.params.len())));
    let index_of = |name: &str| compiled.manifest.params.iter().position(|p| p.name == name).unwrap();
    let mut inst = test_instrument_map();
    inst.insert("synth".into(), Rc::new(RefCell::new(test_list(params))));
    let ui = build_custom_instrument_ui_source_with_overlay(Some((
        "test-instrument".into(), root.join("ui.lisp").display().to_string(),
        std::fs::read_to_string(root.join("ui.lisp")).unwrap(),
    )));
    let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
    editor.set_layout_viewport(180, 22);
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
        (def eseq.browser/clear-editor-name! () nil)
        (defmacro eseq.materials/slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
        (def custom-midi-fx-ui (fx) false)
        (def custom-audio-fx-ui (fx) false)
        (defstate eseq.seq-core-state/selected-bus -1)
    "#).unwrap();
    register_test_delete_target_natives(&mut editor, 1);
    editor.runtime_mut().eval_str(&ui).unwrap_or_else(|e| panic!("load {instrument} UI: {e:?}"));
    editor.runtime_mut().eval_str(&read_ui_source("effects.lisp").unwrap()).unwrap();
    seed_panel_kinds(&mut editor);
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() { panic!("{instrument} UI: {status}"); }
    let fx = editor.buffers.iter().find(|buffer| buffer.name == "*fx*").unwrap().id;
    editor.set_active_buffer(fx);

    fn visit<'a>(node: &'a eseqlisp::layout::LayoutNode, panel: &eseqlisp::layout::LayoutNode,
        controls: &mut Vec<&'a eseqlisp::layout::LayoutNode>)
    {
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker" | "button" | "family-map") {
            assert_finite_nonzero_rect(node, &node.widget_type);
            assert!(node.rect.row >= panel.rect.row && node.rect.col >= panel.rect.col
                && node.rect.row + node.rect.height <= panel.rect.row + panel.rect.height + 0.01
                && node.rect.col + node.rect.width <= panel.rect.col + panel.rect.width + 0.01,
                "{} clipped: {:?} in {:?}", node.widget_type, node.rect, panel.rect);
        }
        if let Some(Value::String(text)) = node.props.get("text") {
            assert!(!text.starts_with("missing"), "unresolved control: {text}");
        }
        if matches!(node.widget_type.as_str(), "knob-number" | "number-picker") { controls.push(node); }
        for child in &node.children { visit(child, panel, controls); }
    }
    let bound_name = |node: &eseqlisp::layout::LayoutNode| -> String {
        let field = bound_field(node.props.get("value")).expect("unbound control");
        field.strip_prefix("vk-test-").expect("real parameter binding").to_string()
    };

    // Every page, through its own tab callback; each param on one control only.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            let layout = editor.widget_layout().unwrap();
            let tab = find_layout_node_by_debug_name(&layout, &format!("{prefix}-page-{page}")).expect("page tab");
            editor.runtime_mut().invoke(tab.props["on-click"].clone(), vec![Value::Bool(false)]).unwrap();
            editor.refresh_runtime_side_effects();
        }
        let layout = editor.widget_layout().unwrap();
        let panel = find_layout_node_by_debug_name(&layout, &format!("{prefix}-surface")).expect("VILLAIN panel surface");
        let mut controls = Vec::new();
        visit(panel, panel, &mut controls);
        for node in controls {
            let name = bound_name(node);
            assert!(expected.contains(&name), "{instrument}: unknown or excluded parameter {name}");
            // The always-visible blocks repeat on every page; count them once.
            let repeats = always_visible.contains(&name.as_str());
            *seen.entry(name).or_default() += usize::from(i == 0 || !repeats);
        }
    }
    let missing: Vec<_> = expected.iter().filter(|name| !seen.contains_key(*name)).collect();
    assert!(missing.is_empty(), "parameters with no control: {missing:?}");
    let doubled: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
    assert!(doubled.is_empty(), "parameters on more than one control: {doubled:?}");

    // The map is bound to the live params and loads the armed slot.
    let layout = editor.widget_layout().unwrap();
    let map = find_layout_node_by_widget_type(&layout, "family-map").expect("family map");
    for &prop in ["a", "b", "blend", "exaggerate"].iter().chain(map_props) {
        assert!(matches!(map.props.get(prop), Some(Value::ReactiveRef { .. })), "map {prop} must follow its param");
    }
    let pick = |editor: &mut eseqlisp::Editor, index: f64, slot: f64| {
        let layout = editor.widget_layout().unwrap();
        let map = find_layout_node_by_widget_type(&layout, "family-map").unwrap();
        editor.drain_host_commands();
        editor.runtime_mut().invoke(map.props["on-pick"].clone(), vec![Value::Number(index), Value::Number(slot)]).unwrap();
        editor.drain_host_commands()
    };
    let sets = |commands: &[eseqlisp::host::HostCommand], param: usize, value: f64| commands.iter().any(|cmd| matches!(cmd,
        eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }
            if name == "set-instrument-param"
                && *payload["param-idx"].borrow() == Value::Number(param as f64)
                && *payload["value"].borrow() == Value::Number(value)));
    let commands = pick(&mut editor, 5.0, 0.0);
    assert!(sets(&commands, index_of(slot_a), 5.0), "slot 0 loads {slot_a}: {commands:?}");
    let commands = pick(&mut editor, 7.0, 1.0);
    assert!(sets(&commands, index_of(slot_b), 7.0), "slot 1 loads {slot_b}: {commands:?}");

    // Arming b reaches the widget, so a plain click then loads b.
    assert_eq!(map.props.get("armed"), Some(&Value::Number(0.0)));
    let arm_b = find_layout_node_by_debug_name(&layout, &format!("{prefix}-arm-b")).expect("arm b button");
    editor.runtime_mut().invoke(arm_b.props["on-click"].clone(), vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)]).unwrap();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    let map = find_layout_node_by_widget_type(&layout, "family-map").unwrap();
    assert_eq!(map.props.get("armed"), Some(&Value::Number(1.0)));
}

#[test]
fn villain_kick_panel_binds_every_param_and_picks_kicks_on_the_map() {
    check_villain_panel(VillainPanel {
        instrument: "VILLAIN Kick",
        prefix: "vk",
        pages: &["hit", "shape", "colour", "air", "room", "out"],
        always_visible: &["kick_a", "kick_b", "blend", "exaggerate", "pc1", "pc2", "pc3", "bend"],
        not_on_panel: &[],
        slot_a: "kick_a",
        slot_b: "kick_b",
        map_props: &["pc1", "pc2", "pc3", "spread", "beat", "tilt", "drop-time"],
    });
}

#[test]
fn villain_snare_panel_binds_every_param_and_picks_snares_on_the_map() {
    check_villain_panel(VillainPanel {
        instrument: "VILLAIN Snare",
        prefix: "vs",
        pages: &["head", "wires", "air", "out"],
        always_visible: &["snare_a", "snare_b", "blend", "exaggerate", "pc1", "pc2", "pc3", "release"],
        not_on_panel: &[],
        slot_a: "snare_a",
        slot_b: "snare_b",
        map_props: &["pc1", "pc2", "pc3", "decay", "bend", "body", "ring", "tune"],
    });
}

#[test]
fn villain_hat_panel_binds_every_param_and_picks_hats_on_the_map() {
    check_villain_panel(VillainPanel {
        instrument: "VILLAIN Hat",
        prefix: "vh",
        pages: &["metal", "record", "out"],
        always_visible: &["hat_a", "hat_b", "blend", "exaggerate", "release", "decay", "pedal", "keytrack"],
        // A research switch (frozen noise for null tests), never on the panel.
        not_on_panel: &["test_freeze"],
        slot_a: "hat_a",
        slot_b: "hat_b",
        map_props: &[],
    });
}

/// data/family.json covers every member the DSP can select, and the panel's
/// slot labels name the same members.
fn check_villain_sidecar(instrument: &str, slot_param: &str) {
    let root = villain_root(instrument);
    let family = eseqlisp::widget_render::family_map::parse_family(
        &std::fs::read_to_string(root.join("data/family.json")).unwrap())
        .unwrap_or_else(|| panic!("{instrument} family.json parses"));
    let dsp = std::fs::read_to_string(root.join("dsp.lisp")).unwrap();
    assert!(dsp.contains(&format!("(param {slot_param} @default 0 @min 0 @max {})", family.members.len() - 1)),
        "{slot_param} must range over the family's {} members", family.members.len());
    let ui = std::fs::read_to_string(root.join("ui.lisp")).unwrap();
    for (index, member) in family.members.iter().enumerate() {
        assert!(ui.contains(&format!("({index} \"{member}\")")), "{instrument} panel label for {index} = {member}");
    }
}

#[test]
fn villain_family_sidecars_match_their_panels_and_dsp() {
    check_villain_sidecar("VILLAIN Kick", "kick_a");
    check_villain_sidecar("VILLAIN Snare", "snare_a");
    check_villain_sidecar("VILLAIN Hat", "hat_a");
}
