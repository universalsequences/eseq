use super::*;
use sequencer::sequencer::{RackMacroCurve, RackMacroId, RackMacroMapping, RackMacroTarget};

fn show_track(editor: &mut Editor, app: &app::App, track: usize) {
    let selected = Arc::new(Mutex::new(HashSet::new()));
    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "current-track", Value::Number(track as f64));
    rt.set_reactive("SEQ", "instrument-panel", build_instrument_panel_value(app, track, &selected));
    sync_rack_macro_value_fields(rt, app, track, None);
    sync_rack_panel_param_value_fields(rt, app, track, None);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
}

fn assert_view(editor: &mut Editor, slots: bool, macros: bool, device: bool) {
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("rack layout");
    assert_finite_layout_tree(&layout);
    let rack = find_layout_node_by_debug_name(&layout, "rack-panel").unwrap();
    assert_finite_nonzero_rect(rack, "rack panel");
    for (name, visible) in [
        ("rack-chain-list", slots), ("rack-macro-bank", macros), ("sampler-panel", device),
    ] {
        let node = find_layout_node_by_debug_name(&layout, name);
        assert_eq!(node.is_some(), visible, "{name}");
        if let Some(node) = node {
            assert_finite_nonzero_rect(node, name);
        }
    }
    if macros {
        let bank = find_layout_node_by_debug_name(&layout, "rack-macro-bank").unwrap();
        assert_layout_inside(bank, rack, "macro bank");
        for index in 0..8 {
            let name = format!("rack-macro-knob-{index}");
            let knob = find_layout_node_by_debug_name(bank, &name).unwrap();
            assert_finite_nonzero_rect(knob, &name);
            assert_layout_inside(knob, bank, &name);
        }
    }
}

fn click_toggle(editor: &mut Editor, name: &str) {
    let layout = editor.widget_layout().unwrap();
    let toggle = find_layout_node_by_debug_name(&layout, name).unwrap();
    assert_finite_nonzero_rect(toggle, name);
    editor.runtime_mut().invoke(toggle.props["on-click"].clone(),
        vec![Value::Number(0.0), Value::Number(0.0), Value::Nil]).unwrap();
}

#[test]
fn rack_views_follow_stable_tracks_and_reset_on_project_replacement() {
    let (mut app, mut editor) = rack_slot_indicator_tests::rack_slot_indicator_editor();
    let first_id = app.track_registry.id_at(0).unwrap();
    app.track_registry.allocate().unwrap();
    app.tracks.push("Second rack".into());
    app.graph.track_instrument_types.push(sequencer::sequencer::InstrumentType::Rack);
    app.graph.instrument_descriptors.push(sequencer::effects::EffectDescriptor::empty_custom_slot());
    {
        let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
        racks.resize(2, None);
        racks[1] = racks[0].clone();
    }
    editor.runtime_mut().set_reactive("SEQ", "num-tracks", Value::Number(2.0));
    show_track(&mut editor, &app, 0);
    assert_view(&mut editor, true, false, true);
    click_toggle(&mut editor, "rack-slot-list-view-toggle");
    click_toggle(&mut editor, "rack-chain-view-toggle");
    click_toggle(&mut editor, "rack-macro-view-toggle");
    assert_view(&mut editor, false, true, false);

    show_track(&mut editor, &app, 1);
    assert_view(&mut editor, true, false, true);
    click_toggle(&mut editor, "rack-macro-view-toggle");
    assert_view(&mut editor, true, true, true);
    show_track(&mut editor, &app, 0);
    assert_view(&mut editor, false, true, false);

    // Identical rack payloads let us isolate identity from dense index/name.
    app.track_registry.move_to(first_id, 1).unwrap();
    show_track(&mut editor, &app, 1);
    assert_view(&mut editor, false, true, false);
    show_track(&mut editor, &app, 0);
    assert_view(&mut editor, true, true, true);

    sync_project_scene_state(editor.runtime_mut(), &app.state);
    editor.runtime_mut().run_reactive_cycle();
    assert_view(&mut editor, true, false, true);
    show_track(&mut editor, &app, 1);
    assert_view(&mut editor, true, false, true);
}

#[test]
fn loaded_rack_view_requires_a_name_and_mapping_on_the_same_macro() {
    let (app, mut editor) = rack_slot_indicator_tests::rack_slot_indicator_editor();
    for (name, mapped, macros_only) in [
        ("Cutoff", false, false), ("", true, false), (" \t\n", true, false),
        ("Cutoff", true, true), ("Cutoff", false, false),
    ] {
        {
            let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
            let rack = racks[0].as_mut().unwrap();
            rack.macros[0].name = name.into();
            rack.macros[0].mappings = if mapped { vec![RackMacroMapping {
                target: RackMacroTarget::SlotParam { slot: 0, param: "gain".into() },
                range_min: 0.0, range_max: 1.0, curve: RackMacroCurve::Linear,
            }] } else { vec![] };
        }
        crate::host_commands::initialize_loaded_rack_view(&app, &mut editor, 0);
        show_track(&mut editor, &app, 0);
        assert_view(&mut editor, !macros_only, macros_only, !macros_only);
    }
}

#[test]
fn rack_preset_and_sound_loads_initialize_only_the_loaded_tracks_view() {
    let eng = engine::init_headless_engine(44_100, 2).unwrap();
    struct GraphGuard(sequencer::audiograph::LiveGraphPtr);
    impl Drop for GraphGuard {
        fn drop(&mut self) {
            unsafe {
                sequencer::audiograph::engine_stop_workers();
                sequencer::audiograph::destroy_live_graph(self.0.0);
            }
        }
    }
    let _guard = GraphGuard(eng.lg_ptr);
    let mut app = app::App::new(eng.state, eng.lg_ptr, eng.sample_rate,
        eng.buses, eng.master_recorder, eng.keyboard_tx);
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/impulses/lexicon-300-rich-plate.wav");
    let track = app.graph_controller().add_track(&sample).unwrap();
    app.graph_controller().group_track_to_instrument_rack(track).unwrap();
    app.map_rack_macro(track, RackMacroId::from_index(0).unwrap(), RackMacroMapping {
        target: RackMacroTarget::SlotParam { slot: 0, param: "gain".into() },
        range_min: 0.0, range_max: 1.0, curve: RackMacroCurve::Linear,
    }).unwrap();
    let name = format!("rack-view-regression-{}", std::process::id());
    struct PresetGuard(PathBuf);
    impl Drop for PresetGuard {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
    }
    let preset_path = app.save_rack_preset(track, &name, false).unwrap();
    let _preset = PresetGuard(preset_path.clone());
    let (_, mut editor) = rack_slot_indicator_tests::rack_slot_indicator_editor();
    show_track(&mut editor, &app, track);
    assert_view(&mut editor, true, false, true);

    app.load_rack_preset_onto_track(track, &name).unwrap();
    crate::host_commands::initialize_loaded_rack_view(&app, &mut editor, track);
    show_track(&mut editor, &app, track);
    assert_view(&mut editor, false, true, false);
    click_toggle(&mut editor, "rack-slot-list-view-toggle");

    // Sounds and rack presets share the container format, but enter through
    // separate application paths. Loading onto an unselected track must not
    // change the visible track's settings.
    let second = app.add_track_from_sound(&preset_path).unwrap();
    crate::host_commands::initialize_loaded_rack_view(&app, &mut editor, second);
    assert_view(&mut editor, true, true, false);
    show_track(&mut editor, &app, second);
    assert_view(&mut editor, false, true, false);
    click_toggle(&mut editor, "rack-chain-view-toggle");
    app.load_sound_onto_track(second, &preset_path).unwrap();
    crate::host_commands::initialize_loaded_rack_view(&app, &mut editor, second);
    show_track(&mut editor, &app, second);
    assert_view(&mut editor, false, true, false);
    show_track(&mut editor, &app, track);
    assert_view(&mut editor, true, true, false);
}
