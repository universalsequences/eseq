use super::*;

struct Fixture {
    app: crate::app::App,
    data: Box<AudioCallbackData>,
    engine: engine::HeadlessEngine,
    engine_id: usize,
    output: Vec<f32>,
}

impl Fixture {
    fn new() -> Self {
        let engine = engine::init_headless_engine(48_000, 2).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = crate::app::App::new(
            engine.state.clone(), engine.lg_ptr, engine.sample_rate,
            engine.buses.clone(), engine.master_recorder.clone(), tx,
        );
        let manifest = crate::lisp_host::DGenManifest {
            dylib_path: Default::default(), asset_base: None, version: 1,
            process_abi: String::new(), total_memory_slots: 8,
            params: vec![crate::lisp_host::DGenParam {
                name: "tone".into(), display_name: "tone".into(),
                cell_id: 0, cell_span: 2, default: 0.25, min: 0.0, max: 1.0,
                unit: None, hidden: false, group: None, env: None, role: None, options: None,
            }],
            groups: vec![], envelopes: vec![], inputs: vec![], modulators: vec![],
            mod_outputs: vec![], mod_destinations: vec![], n_inputs: 0, n_outputs: 1,
            tensors: vec![crate::lisp_host::TensorMeta {
                name: "shape".into(), cell_offset: 2, shape: vec![2], kind: "param".into(),
                mutable: true, source_file: None, source_sample_rate: None,
            }], tensor_init_data: vec![], voice_cell_id: None,
        };
        let lib = crate::lisp_host::test_loaded_dgen_lib();
        let engine_id = app.editor.engine_registry.upsert(crate::app::EngineDescriptor {
            name: "shared".into(), source: "shared.lisp".into(),
            manifest: manifest.clone(), lib_index: 0, shared_runtime: true,
        });
        app.editor.instrument_libs.push(crate::lisp_host::test_loaded_dgen_lib());
        app.graph_controller().add_custom_track(
            "flat", engine_id, &manifest, &lib, CustomInstrumentRunMode::Instrument,
        ).unwrap();
        for slots in [2, 1] {
            let track = app.graph_controller().add_empty_layer_rack_track().unwrap();
            for _ in 0..slots {
                app.graph_controller().add_custom_slot_to_rack(
                    track, "shared", engine_id, &manifest, &lib,
                    CustomInstrumentRunMode::Instrument,
                ).unwrap();
            }
        }
        let data = new_audio_callback_data(
            engine.lg_ptr.0, engine.state.clone(), 48_000, 2, 512,
            engine.master_recorder.clone(), rx, engine.buses.bus_effect_runtime.clone(),
            Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
        );
        let mut fixture = Self { app, data, engine, engine_id, output: vec![0.0; 1024] };
        fixture.render();
        // Exercise multiple voices, two slots on one track, and a flat track
        // sharing the same engine. Ownership remains exclusively callback-side.
        for (voice, route) in [0, rack_slot_pool_index(1, 0).unwrap(),
            rack_slot_pool_index(1, 1).unwrap(), rack_slot_pool_index(2, 0).unwrap(),
            rack_slot_pool_index(1, 0).unwrap()].into_iter().enumerate()
        {
            let slot = &mut fixture.data.custom_engine_pools[engine_id].voices[voice];
            slot.active = true;
            slot.assigned_route = Some(route);
            slot.assigned_track = Some(if route == 0 { 0 } else if voice == 3 { 2 } else { 1 });
            slot.fingerprint = 123;
            let node = fixture.app.graph.engine_node_ids[engine_id].as_ref().unwrap().synth_ids[voice];
            unsafe { assert!(add_node_to_watchlist(fixture.engine.lg_ptr.0, node)); }
            let modulator = fixture.app.graph.engine_node_ids[engine_id].as_ref().unwrap().modulator_ids[voice];
            unsafe { assert!(add_node_to_watchlist(fixture.engine.lg_ptr.0, modulator)); }
        }
        fixture.render();
        fixture
    }

    fn render(&mut self) {
        // Watch snapshots publish every four graph blocks.
        for _ in 0..4 { audio_callback(&mut self.data, &mut self.output); }
    }

    fn node_state(&self, node: i32, slots: usize) -> Vec<f32> {
        let mut state = vec![0.0_f32; slots];
        let mut size = 0;
        unsafe { assert!(get_node_state_into(self.engine.lg_ptr.0, node,
            state.as_mut_ptr().cast(), std::mem::size_of_val(state.as_slice()), &mut size)); }
        state
    }

    fn cells(&self, voice: usize, offset: usize) -> [f32; 2] {
        let node = self.app.graph.engine_node_ids[self.engine_id].as_ref().unwrap().synth_ids[voice];
        let state = self.node_state(node, crate::lisp_host::dgen_total_state_slots(8));
        let start = crate::lisp_host::HEADER_SLOTS + offset;
        [state[start], state[start + 1]]
    }

    fn tone(&self, voice: usize) -> [f32; 2] { self.cells(voice, 0) }
}

impl Drop for Fixture {
    fn drop(&mut self) { unsafe { self.engine.destroy(); } }
}

#[test]
fn live_rack_instrument_edits_only_reach_the_slots_current_voices() {
    let mut f = Fixture::new();
    let before: Vec<_> = (0..5).map(|voice| f.tone(voice)).collect();
    assert!(f.app.set_rack_slot_instrument_param(1, 0, 0, 0.8));
    f.render();
    for voice in 0..5 {
        assert_eq!(f.tone(voice), if voice == 1 || voice == 4 { [0.8; 2] } else { before[voice] },
            "rack edit leaked to voice {voice}");
    }
}

#[test]
fn live_flat_instrument_edits_do_not_reach_shared_rack_voices() {
    let mut f = Fixture::new();
    let before: Vec<_> = (0..5).map(|voice| f.tone(voice)).collect();
    f.app.send_instrument_param(0, 0, 0.6);
    f.render();
    for voice in 0..5 {
        assert_eq!(f.tone(voice), if voice == 0 { [0.6; 2] } else { before[voice] },
            "flat edit leaked to voice {voice}");
    }
}

#[test]
fn live_instrument_edits_follow_reassigned_voices_and_invalidate_only_their_cache() {
    let mut f = Fixture::new();
    let before: Vec<_> = (0..5).map(|voice| f.tone(voice)).collect();
    f.app.send_rack_slot_instrument_param(1, 0, 0, 0.7);
    let pool = &mut f.data.custom_engine_pools[f.engine_id];
    pool.voices[1].assigned_route = Some(rack_slot_pool_index(2, 0).unwrap());
    pool.voices[3].assigned_route = Some(rack_slot_pool_index(1, 0).unwrap());
    // Releasing voices still belong to the slot; inactive voices do not.
    pool.voices[3].release_started_sample = Some(0);
    pool.voices[4].active = false;
    let (_, allocations) = crate::test_alloc::measure(|| {
        live_params::apply_live_instrument_params(&mut f.data);
    });
    assert_eq!(allocations, crate::test_alloc::Counts::default());
    f.render();
    for voice in 0..5 {
        assert_eq!(f.tone(voice), if voice == 3 { [0.7; 2] } else { before[voice] });
        assert_eq!(f.data.custom_engine_pools[f.engine_id].voices[voice].fingerprint,
            if voice == 3 { 0 } else { 123 });
    }
}

#[test]
fn live_instrument_modulation_and_tensor_edits_respect_the_consumer() {
    use crate::instruments::voice_modulator as vm;
    let mut f = Fixture::new();
    let mod_idx = vm::slot_param_idx(0, vm::PARAM_LFO_RATE_HZ);
    let param_idx = f.app.graph.instrument_descriptors[0].params.iter().position(|p|
        p.node_param_idx == vm::MOD_PARAM_BASE + mod_idx as u32).unwrap();
    let mod_value = |f: &Fixture, voice: usize| {
        let node = f.app.graph.engine_node_ids[f.engine_id].as_ref().unwrap().modulator_ids[voice];
        f.node_state(node, vm::STATE_SIZE)[mod_idx]
    };
    let before_mod: Vec<_> = (0..5).map(|voice| mod_value(&f, voice)).collect();
    let before_tensor: Vec<_> = (0..5).map(|voice| f.cells(voice, 2)).collect();
    f.app.send_rack_slot_instrument_param(2, 0, param_idx, 3.5);
    f.app.send_instrument_tensor_param(0, 0, &[0.3, 0.9]);
    let (_, allocations) = crate::test_alloc::measure(|| {
        live_params::apply_live_instrument_params(&mut f.data);
    });
    assert_eq!(allocations, crate::test_alloc::Counts::default());
    f.render();
    for voice in 0..5 {
        assert_eq!(mod_value(&f, voice), if voice == 3 { 3.5 } else { before_mod[voice] });
        assert_eq!(f.cells(voice, 2), if voice == 0 { [0.3, 0.9] } else { before_tensor[voice] });
    }
}

#[test]
fn pending_live_instrument_edits_are_rejected_after_route_or_engine_replacement() {
    let mut f = Fixture::new();
    let before: Vec<_> = (0..5).map(|voice| f.tone(voice)).collect();
    f.app.send_rack_slot_instrument_param(1, 0, 0, 0.8);
    let route = rack_slot_pool_index(1, 0).unwrap();
    let old_route = f.app.state.runtime.rack_engine_route_lids[route][0].swap(0, Ordering::AcqRel);
    live_params::apply_live_instrument_params(&mut f.data);
    f.app.state.runtime.rack_engine_route_lids[route][0].store(old_route, Ordering::Release);
    f.app.send_instrument_param(0, 0, 0.6);
    let old_synth = f.app.state.runtime.engine_synth_node_ids[f.engine_id][0].swap(0, Ordering::AcqRel);
    live_params::apply_live_instrument_params(&mut f.data);
    f.app.state.runtime.engine_synth_node_ids[f.engine_id][0].store(old_synth, Ordering::Release);
    f.render();
    for voice in 0..5 { assert_eq!(f.tone(voice), before[voice]); }
}

#[test]
fn live_instrument_edits_reject_voice_pool_from_another_runtime_generation() {
    let mut f = Fixture::new();
    let before = f.tone(1);
    f.app.send_rack_slot_instrument_param(1, 0, 0, 0.8);
    let voice = &mut f.data.custom_engine_pools[f.engine_id].voices[1];
    let logical_id = voice.logical_id;
    voice.logical_id = logical_id + 1;
    live_params::apply_live_instrument_params(&mut f.data);
    f.data.custom_engine_pools[f.engine_id].voices[1].logical_id = logical_id;
    f.render();
    assert_eq!(f.tone(1), before);
    assert_eq!(f.tone(4), [0.8; 2]);
}

#[test]
fn live_free_patch_edits_reach_the_idle_voice_without_a_note() {
    let mut f = Fixture::new();
    f.app.graph_controller().set_track_instrument_run_mode(0, CustomInstrumentRunMode::FreePatch).unwrap();
    let engine_id = f.app.graph.track_engine_ids[0].unwrap();
    assert_ne!(engine_id, f.engine_id, "free patches require a dedicated engine");
    let node = f.app.graph.engine_node_ids[engine_id].as_ref().unwrap().synth_ids[0];
    unsafe { assert!(add_node_to_watchlist(f.engine.lg_ptr.0, node)); }
    f.render();
    f.app.send_instrument_param(0, 0, 0.4);
    f.render();
    let state = f.node_state(node, crate::lisp_host::dgen_total_state_slots(8));
    assert_eq!(&state[crate::lisp_host::HEADER_SLOTS..][..2], &[0.4; 2]);
    assert_eq!(f.tone(1), [0.25; 2]);
}
