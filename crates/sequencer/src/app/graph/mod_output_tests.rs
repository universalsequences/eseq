//! Patches with several `@modulator` outputs (eseq-05ae): one port and hub
//! per output, inputs giving way to them.

use super::*;
use crate::audio::engine;
use crate::lisp_host::{DGenManifest, DGenModOutput};
use crate::sequencer::{ModDestination, MAX_TRACK_MOD_OUTPUTS};

struct GraphGuard(engine::HeadlessEngine);
impl Drop for GraphGuard {
    fn drop(&mut self) {
        unsafe { self.0.destroy(); }
    }
}

/// A silent patch whose outputs 1..=mod_outputs are `@modulator`s.
fn manifest(mod_outputs: usize) -> DGenManifest {
    DGenManifest {
        effect_latency_samples: None, tail_sleep: false, dylib_path: Default::default(), asset_base: None,
        version: 1, process_abi: String::new(), total_memory_slots: 1,
        params: vec![], groups: vec![], envelopes: vec![], inputs: vec![],
        modulators: vec![],
        mod_outputs: (0..mod_outputs)
            .map(|index| DGenModOutput {
                slot: index + 1,
                channel: index + 1,
                name: format!("mod{}", index + 1),
                range: "unipolar".into(),
            })
            .collect(),
        amp_output_channel: None, probes: vec![], mod_destinations: vec![],
        n_inputs: 4, n_outputs: 1 + mod_outputs, tensors: vec![], tensor_init_data: vec![],
        voice_cell_id: None,
        binding_shapes: Default::default(),
    }
}

fn add_custom(app: &mut App, name: &str, mod_outputs: usize) -> usize {
    let manifest = manifest(mod_outputs);
    let engine_id = app.editor.engine_registry.upsert(crate::app::EngineDescriptor {
        name: name.into(), source: format!("{name}.lisp"),
        manifest: manifest.clone(), lib_index: app.editor.instrument_libs.len(), shared_runtime: false,
    });
    app.editor.instrument_libs.push(crate::lisp_host::test_loaded_dgen_lib());
    let lib = crate::lisp_host::test_loaded_dgen_lib();
    app.graph_controller()
        .add_custom_track(name, engine_id, &manifest, &lib, CustomInstrumentRunMode::FreePatch)
        .unwrap();
    app.tracks.len() - 1
}

#[test]
fn a_two_output_patch_shows_two_outs_three_ins_and_routes_each_output_from_its_hub() {
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let guard = GraphGuard(engine);
    let engine = &guard.0;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        engine.state.clone(), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), engine.master_recorder.clone(), tx,
    );
    let source = add_custom(&mut app, "two-outs", 2);
    let dest = add_custom(&mut app, "one-out", 1);
    let plain = add_custom(&mut app, "no-outs", 0);

    assert_eq!(app.graph.track_mod_output_count(source), 2);
    assert_eq!(app.graph.track_mod_output_names(source), vec!["mod1", "mod2"]);
    assert_eq!(app.graph.track_visible_mod_inputs(source), 3);
    assert_eq!(app.graph.track_visible_mod_inputs(dest), 4);
    assert_eq!(app.graph.track_mod_output_count(plain), 0);
    assert_eq!(app.graph.track_visible_mod_inputs(plain), 4);
    // A Modulator keeps its inputs so another modulator can sequence it.
    let modulator = app.graph_controller().add_modulator_track().unwrap();
    assert_eq!(app.graph.track_visible_mod_inputs(modulator), 4);
    assert_eq!(
        app.graph.engine_node_ids[app.graph.track_engine_ids[source].unwrap()]
            .as_ref()
            .unwrap()
            .mod_output_channels,
        vec![1, 2]
    );

    // Output 2 of the source reaches the destination from its own hub.
    app.graph_controller()
        .set_mod_route_to_destination(source, 1, ModDestination::Track(dest), 2)
        .unwrap();
    let hubs = app.graph.track_node_ids[source].mod_out_ids;
    let dest_clip = app.graph.track_node_ids[dest].mod_in_clip_ids[2];
    assert_eq!(app.graph.applied_mod_routes, vec![(hubs[1], dest_clip)]);
    assert_eq!(app.state.current_mod_connections()[0].source_output, 1);
    assert!(hubs.iter().all(|hub| *hub > 0) && hubs[0] != hubs[1]);

    // A third output does not exist, and the two-output track's Ext4 port
    // is taken by its second output.
    assert!(app.graph_controller()
        .set_mod_route_to_destination(source, 2, ModDestination::Track(dest), 0)
        .is_err());
    let error = app.graph_controller()
        .set_mod_route_to_destination(dest, 0, ModDestination::Track(source), 3)
        .unwrap_err();
    assert!(error.contains("Ext4"), "{error}");
    app.graph_controller()
        .set_mod_route_to_destination(dest, 0, ModDestination::Track(source), 2)
        .unwrap();

    // Delete takes the output into account.
    app.graph_controller()
        .delete_mod_route_to_destination(source, 0, ModDestination::Track(dest), 2)
        .unwrap();
    assert_eq!(app.state.current_mod_connections().len(), 2, "output 1 had no such route");
    app.graph_controller()
        .delete_mod_route_to_destination(source, 1, ModDestination::Track(dest), 2)
        .unwrap();
    assert_eq!(app.state.current_mod_connections().len(), 1);
    assert_eq!(MAX_TRACK_MOD_OUTPUTS, 4);
}

#[test]
fn a_route_from_an_output_the_patch_no_longer_declares_stops_carrying_but_is_kept() {
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let guard = GraphGuard(engine);
    let engine = &guard.0;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        engine.state.clone(), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), engine.master_recorder.clone(), tx,
    );
    let source = add_custom(&mut app, "two-outs", 2);
    let dest = add_custom(&mut app, "plain", 0);
    app.graph_controller()
        .set_mod_route_to_destination(source, 1, ModDestination::Track(dest), 0)
        .unwrap();
    assert_eq!(app.graph.applied_mod_routes.len(), 1);

    // The patch drops to one output: the route is parked, not lost.
    let manifest = manifest(1);
    let engine_id = app.editor.engine_registry.upsert(crate::app::EngineDescriptor {
        name: "one-out".into(), source: "one-out.lisp".into(),
        manifest: manifest.clone(), lib_index: app.editor.instrument_libs.len(), shared_runtime: false,
    });
    app.editor.instrument_libs.push(crate::lisp_host::test_loaded_dgen_lib());
    let lib = crate::lisp_host::test_loaded_dgen_lib();
    app.graph_controller()
        .swap_custom_track_instrument(source, "one-out", engine_id, &manifest, &lib, CustomInstrumentRunMode::FreePatch)
        .unwrap();
    assert_eq!(app.graph.track_mod_output_count(source), 1);
    assert!(app.graph.applied_mod_routes.is_empty());
    assert_eq!(app.state.current_mod_connections().len(), 1);
}

/// Module strips: a patch whose every output is `@modulator` makes no audio,
/// so the mixer shows it as a module with all four inputs, labeled by the
/// `@name` of the slots it reads with `(ext N)`.
#[test]
fn an_all_modulator_patch_is_a_module_with_named_ext_inputs_and_all_four_inputs() {
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let guard = GraphGuard(engine);
    let engine = &guard.0;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        engine.state.clone(), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), engine.master_recorder.clone(), tx,
    );
    app.start_new_project();
    while app.tracks.len() < 3 {
        app.graph_controller().add_empty_track().unwrap();
    }
    let swap = |app: &mut App, track: usize, name: &str, source: &str| {
        let compiled = crate::lisp_host::compile_and_load_instrument(source, 48_000).unwrap();
        app.swap_track_to_compiled_saved_instrument_sync(
            track, name, source, CustomInstrumentRunMode::FreePatch, compiled,
        ).unwrap();
    };
    // The patcher declares every slot as `modN`; only slots read with
    // `(ext N)` and given a real name label their input, either on the
    // declaration or at the use site, `(ext N @name ...)`.
    let clock = "(def mod1 (in 6 @name clock @modulator 1))
                 (def mod2 (in 7 @name reset @modulator 2))
                 (def mod3 (in 8 @name mod3 @modulator 3))
                 (def mod4 (in 9 @name ignored @modulator 4))
                 (out (ext 1) 1 @name x1 @modulator 1)
                 (out (* (ext 1) (ext 2) (ext 3 @name rate)) 2 @name x2 @modulator 2)
                 (out (ext 1) 3 @name x3 @modulator 3)";
    swap(&mut app, 0, "module-clock", clock);
    swap(&mut app, 1, "voice", "(out 0 1 @name audio) (out 0.5 2 @name level @modulator 1)");

    assert!(app.graph.track_is_module(0));
    assert_eq!(app.graph.track_mod_output_count(0), 3);
    assert_eq!(app.graph.track_mod_output_names(0), vec!["x1", "x2", "x3"]);
    assert_eq!(app.graph.track_ext_input_names(0), vec!["clock", "reset", "rate", ""]);
    // Three outputs would leave a plain strip one input; a module keeps four,
    // and routes into its fourth input are accepted.
    assert_eq!(app.graph.track_visible_mod_inputs(0), 4);
    app.graph_controller()
        .set_mod_route_to_destination(1, 0, ModDestination::Track(0), 3)
        .unwrap();

    // An instrument with an audio out stays a plain strip.
    assert!(!app.graph.track_is_module(1));
    assert_eq!(app.graph.track_ext_input_names(1), vec!["", "", "", ""]);
    assert!(!app.graph.track_is_module(2), "an empty track is not a module");
    let modulator = app.graph_controller().add_modulator_track().unwrap();
    assert!(app.graph.track_is_module(modulator), "a Modulator makes no audio");
}
