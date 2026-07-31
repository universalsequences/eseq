    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::macro_engine::{MacroCurve, MacroKind, MacroMapping, MacroParamKey};
    use crate::process::ParamTarget;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternSnapshot, ProjectArrangement, SceneEvent,
        SequencerState,
    };
    use crate::app::edit::{try_apply_command, EditOutcome};
    use crate::app::{AppCommand, AudioBuses};
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};

    fn topology_test_slot(
        instrument_type: InstrumentType,
        engine_id: Option<usize>,
    ) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note: None,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: 4,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default(
                &EffectDescriptor::builtin_sampler(),
                1,
            ),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState {
                engine_id,
                loaded_preset: None,
                dirty: false,
            },
            sample_id: (instrument_type == InstrumentType::Sampler)
                .then(|| (1, "kick".to_string(), 44_100)),
        }
    }

    fn topology_test_rack() -> RackTrackSnapshot {
        RackTrackSnapshot::new(
            RackRouting::Broadcast,
            vec![
                topology_test_slot(InstrumentType::Sampler, None),
                topology_test_slot(InstrumentType::Sampler, None),
            ],
            crate::sequencer::default_rack_macros(),
        )
    }

    #[test]
    fn signature_equal_for_identical_snapshots() {
        let first = topology_test_rack();
        let second = first.clone();

        assert_eq!(
            rack_topology_signature(&first),
            rack_topology_signature(&second)
        );
    }

    #[test]
    fn signature_ignores_parameter_fields() {
        let first = topology_test_rack();
        let mut second = first.clone();
        let slot = &mut second.slots[0];
        slot.gain = 0.25;
        slot.pan = -0.5;
        slot.mute = true;
        slot.solo = true;
        slot.max_polyphony = 1;
        slot.sample_id = Some((99, "other-kick".to_string(), 48_000));
        slot.pad_note = Some(36);
        slot.choke_group = Some(2);
        assert!(slot
            .param_plocks
            .set(0, crate::sequencer::RackSlotParam::Gain, 0.75));
        second.macros[0].value = 0.8;
        second.routing = RackRouting::ByPitch;

        assert_eq!(
            rack_topology_signature(&first),
            rack_topology_signature(&second)
        );
    }

    #[test]
    fn signature_detects_topology_changes() {
        let base = topology_test_rack();
        let signature = rack_topology_signature(&base);
        let assert_changed = |candidate: &RackTrackSnapshot| {
            assert_ne!(signature, rack_topology_signature(candidate));
        };

        let mut slot_count = base.clone();
        slot_count.slots.pop();
        assert_changed(&slot_count);

        let mut slot_order = RackTrackSnapshot::new(
            RackRouting::Broadcast,
            vec![
                topology_test_slot(InstrumentType::Sampler, None),
                topology_test_slot(InstrumentType::Custom, Some(7)),
            ],
            crate::sequencer::default_rack_macros(),
        );
        let ordered_signature = rack_topology_signature(&slot_order);
        slot_order.slots.swap(0, 1);
        assert_ne!(ordered_signature, rack_topology_signature(&slot_order));

        let mut instrument_type = base.clone();
        instrument_type.slots[0] = topology_test_slot(InstrumentType::Custom, Some(7));
        assert_changed(&instrument_type);

        let mut engine_id = instrument_type.clone();
        engine_id.slots[0].track_sound_state.engine_id = Some(8);
        assert_ne!(
            rack_topology_signature(&instrument_type),
            rack_topology_signature(&engine_id)
        );

        let mut run_mode = instrument_type.clone();
        run_mode.slots[0].instrument_run_mode = CustomInstrumentRunMode::FreePatch;
        assert_ne!(
            rack_topology_signature(&instrument_type),
            rack_topology_signature(&run_mode)
        );

        let mut fx_node = base.clone();
        fx_node.slots[0].effect_slots[0].node_id = 42;
        assert_changed(&fx_node);

        let mut fx_length = base.clone();
        fx_length.slots[0].effect_slots.pop();
        fx_length.slots[0].effect_descriptors.pop();
        assert_changed(&fx_length);
    }

    struct TestLiveGraph {
        ptr: LiveGraphPtr,
        block_size: i32,
        channels: usize,
    }

    struct TestProjectFile(PathBuf);

    impl Drop for TestProjectFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create test WAV");
        for frame in 0..256 {
            let phase = frame as f32 / 256.0;
            let sample = (phase * std::f32::consts::TAU).sin();
            writer
                .write_sample((sample * i16::MAX as f32) as i16)
                .expect("write test WAV sample");
        }
        writer.finalize().expect("finalize test WAV");
    }

    impl TestLiveGraph {
        fn new(label: &str) -> Self {
            const BLOCK_SIZE: i32 = 64;
            const SAMPLE_RATE: i32 = 44_100;
            const CHANNELS: usize = 2;
            crate::audiograph::initialize_engine_for_test(BLOCK_SIZE, SAMPLE_RATE);
            let label = CString::new(label).expect("test graph label should not contain NUL");
            let ptr = unsafe {
                crate::audiograph::create_live_graph(
                    32,
                    BLOCK_SIZE,
                    label.as_ptr(),
                    CHANNELS as i32,
                )
            };
            assert!(!ptr.is_null(), "test live graph should be created");
            Self {
                ptr: LiveGraphPtr(ptr),
                block_size: BLOCK_SIZE,
                channels: CHANNELS,
            }
        }

        fn add_gain(&self, gain: f32, name: &str) -> i32 {
            add_gain_node_checked(self.ptr.0, gain, name, "test graph gain")
                .expect("test gain node should be queued")
        }

        fn add_voice_modulator(&self, engine_id: usize, voice: usize) -> i32 {
            let name = CString::new(format!("test_modulator_{voice}"))
                .expect("test modulator name should not contain NUL");
            let initial_state =
                crate::instruments::voice_modulator::custom_engine_initial_state(engine_id, voice);
            let node_id = unsafe {
                crate::audiograph::add_node(
                    self.ptr.0,
                    crate::instruments::voice_modulator::voice_modulator_vtable(),
                    crate::instruments::voice_modulator::STATE_SIZE * std::mem::size_of::<f32>(),
                    name.as_ptr(),
                    crate::instruments::voice_modulator::INPUT_COUNT as i32,
                    crate::instruments::voice_modulator::NUM_OUTPUTS as i32,
                    (&initial_state as *const crate::instruments::voice_modulator::VoiceModulatorInitialState)
                        .cast(),
                    std::mem::size_of::<crate::instruments::voice_modulator::VoiceModulatorInitialState>(),
                )
            };
            assert!(node_id >= 0, "test modulator node should be queued");
            node_id
        }

        fn process_block(&self) {
            let mut output = vec![0.0_f32; self.block_size as usize * self.channels];
            unsafe {
                self.ptr
                    .process_next_block(output.as_mut_ptr(), self.block_size);
            }
        }
    }

    impl Drop for TestLiveGraph {
        fn drop(&mut self) {
            unsafe { crate::audiograph::destroy_live_graph(self.ptr.0) };
        }
    }

    #[test]
    fn watchlist_throttle_starts_each_live_graph_with_a_snapshot() {
        let first = TestLiveGraph::new("first-watchlist-throttle-test");
        let second = TestLiveGraph::new("second-watchlist-throttle-test");
        let first_node = first.add_gain(0.25, "first_watched_gain");
        let second_node = second.add_gain(0.75, "second_watched_gain");

        assert!(unsafe {
            crate::audiograph::add_node_to_watchlist(first.ptr.0, first_node)
        });
        assert!(unsafe {
            crate::audiograph::add_node_to_watchlist(second.ptr.0, second_node)
        });
        first.process_block();
        second.process_block();

        for (graph, node_id, expected) in [
            (&first, first_node, 0.25_f32),
            (&second, second_node, 0.75_f32),
        ] {
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            assert!(unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            });
            assert_eq!(state_size, std::mem::size_of_val(&state));
            assert_eq!(state[0], expected);
        }
    }

    struct RouteTargets {
        voice_sum_id: i32,
        voice_sum_r_id: i32,
        track_mod_out_id: i32,
        track_mod_in_clip_ids: [i32; EXT_MOD_INPUT_COUNT],
    }

    fn test_app(graph: &TestLiveGraph) -> App {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    fn test_app_with_track_count(graph: &TestLiveGraph, track_count: usize) -> App {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let (keyboard_tx, _keyboard_rx) = mpsc::channel();
        App::new(
            state,
            graph.ptr,
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        )
    }

    fn test_instrument_manifest() -> DGenManifest {
        DGenManifest {
            dylib_path: PathBuf::new(),
            version: 1,
            process_abi: String::new(),
            total_memory_slots: 1,
            params: Vec::new(),
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: ["gate", "pitch", "velocity", "trigger"]
                .into_iter()
                .enumerate()
                .map(|(channel, name)| lisp_host::DGenInput {
                    channel,
                    name: name.to_string(),
                })
                .collect(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 4,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    fn install_custom_track_swap_fixture(
        app: &mut App,
        graph: &TestLiveGraph,
        track_count: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) {
        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "old", manifest, lib)
                .expect("old engine should materialize");
        }
        let (synth_ids, gatepitch_ids, node_id, modulator_node_id) = {
            let engine = app.graph.engine_node_ids[0]
                .as_ref()
                .expect("old engine should exist");
            (
                engine.synth_ids.clone(),
                engine.gatepitch_ids.clone(),
                first_graph_node_identity(&engine.synth_ids),
                first_graph_node_identity(&engine.modulator_ids),
            )
        };
        let descriptor = lisp_host::instrument_descriptor_from_manifest("old", manifest);

        for track in 0..track_count {
            let nodes = TrackNodeIds {
                sampler_ids: Vec::new(),
                sampler_gatepitch_ids: Vec::new(),
                sampler_modulator_ids: Vec::new(),
                voice_sum_id: graph.add_gain(1.0, &format!("track_{track}_voice_sum")),
                voice_sum_r_id: graph.add_gain(1.0, &format!("track_{track}_voice_sum_r")),
                pan_id: graph.add_gain(1.0, &format!("track_{track}_pan")),
                filter_id: graph.add_gain(1.0, &format!("track_{track}_filter")),
                delay_id: graph.add_gain(1.0, &format!("track_{track}_delay")),
                send_id: graph.add_gain(1.0, &format!("track_{track}_send")),
                mod_out_id: graph.add_gain(1.0, &format!("track_{track}_mod_out")),
                mod_in_clip_ids: std::array::from_fn(|input| {
                    graph.add_gain(1.0, &format!("track_{track}_mod_in_{input}"))
                }),
                mod_env_id: graph.add_gain(1.0, &format!("track_{track}_mod_env")),
                bus_send_ids: Vec::new(),
                rack_slots: Vec::new(),
                rack_signature: None,
            };
            {
                let _batch = GraphEditBatchGuard::new(graph.ptr.0);
                app.graph_controller()
                    .connect_engine_to_track(
                        0,
                        track,
                        track,
                        &format!("Track {}", track + 1),
                        nodes.voice_sum_id,
                        nodes.voice_sum_r_id,
                        nodes.mod_out_id,
                        nodes.mod_in_clip_ids,
                    )
                    .expect("old engine route should connect");
            }
            app.tracks.push(format!("Track {}", track + 1));
            app.track_registry
                .allocate()
                .expect("allocate fixture track id");
            app.graph.track_node_ids.push(nodes);
            app.graph.track_buffer_ids.push(-1);
            app.graph.track_sample_rates.push(44_100);
            app.graph.track_voice_lids.push(Vec::new());
            app.graph
                .track_instrument_types
                .push(InstrumentType::Custom);
            app.graph
                .track_instrument_run_modes
                .push(CustomInstrumentRunMode::Instrument);
            app.graph.track_engine_ids.push(Some(0));
            app.graph.track_synth_node_ids.push(synth_ids.clone());
            app.graph
                .track_gatepitch_node_ids
                .push(gatepitch_ids.clone());
            app.graph
                .effect_descriptors
                .push(EffectDescriptor::default_full_chain());
            app.graph.instrument_descriptors.push(descriptor.clone());
            app.graph.record_armed.push(false);
            app.state
                .reset_instrument_slot_all_patterns(
                    track,
                    &descriptor,
                    node_id,
                    modulator_node_id,
                    0,
                    CustomInstrumentRunMode::Instrument,
                )
                .expect("initial instrument state should reset");
        }
        app.sync_scratch_runtime_descriptors();
    }

    fn assert_test_slot_snapshot_eq(actual: &EffectSlotSnapshot, expected: &EffectSlotSnapshot) {
        assert_eq!(actual.node_id, expected.node_id);
        assert_eq!(actual.modulator_node_id, expected.modulator_node_id);
        assert_eq!(actual.num_params, expected.num_params);
        assert_eq!(actual.defaults, expected.defaults);
        assert_eq!(actual.plocks, expected.plocks);
        assert_eq!(actual.plock_param_ids, expected.plock_param_ids);
        assert_eq!(actual.key_locks, expected.key_locks);
        assert_eq!(actual.key_lock_param_ids, expected.key_lock_param_ids);
        assert_eq!(actual.param_node_indices, expected.param_node_indices);
        assert_eq!(actual.param_node_spans, expected.param_node_spans);
        assert_eq!(actual.tensor_params.len(), expected.tensor_params.len());
    }

    fn install_test_engine(app: &mut App, graph: &TestLiveGraph) -> RouteTargets {
        let synth_ids = (0..MAX_VOICES)
            .map(|voice| graph.add_gain(1.0, &format!("test_synth_{voice}")))
            .collect();
        let modulator_ids = (0..MAX_VOICES)
            .map(|voice| graph.add_voice_modulator(0, voice))
            .collect();
        app.graph.engine_node_ids = vec![Some(EngineNodeIds {
            synth_ids,
            synth_inputs: 0,
            synth_outputs: 1,
            audio_output_channels: vec![0],
            mod_output_channels: vec![0],
            gatepitch_ids: Vec::new(),
            modulator_ids,
            route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
            ext_route_gain_ids: (0..crate::sequencer::MAX_SAMPLER_POOLS)
                .map(|_| Vec::new())
                .collect(),
        })];
        RouteTargets {
            voice_sum_id: graph.add_gain(1.0, "test_voice_sum"),
            voice_sum_r_id: graph.add_gain(1.0, "test_voice_sum_r"),
            track_mod_out_id: graph.add_gain(1.0, "test_track_mod_out"),
            track_mod_in_clip_ids: std::array::from_fn(|input| {
                graph.add_gain(1.0, &format!("test_track_mod_in_{input}"))
            }),
        }
    }

    fn connect_test_engine(app: &mut App, targets: &RouteTargets) -> Result<(), String> {
        app.graph_controller().connect_engine_to_track(
            0,
            0,
            0,
            "Test Track",
            targets.voice_sum_id,
            targets.voice_sum_r_id,
            targets.track_mod_out_id,
            targets.track_mod_in_clip_ids,
        )
    }

    #[test]
    fn graph_edit_batch_reports_audio_thread_application() {
        let graph = TestLiveGraph::new("graph-edit-application-watermark-test");

        unsafe { crate::audiograph::begin_graph_edit_batch(graph.ptr.0) };
        let serial = unsafe { crate::audiograph::graph_edit_current_batch_serial(graph.ptr.0) };
        assert!(serial > 0, "an open batch should expose its serial");
        graph.add_gain(1.0, "watermark_probe");
        unsafe { crate::audiograph::end_graph_edit_batch(graph.ptr.0) };

        assert!(
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(graph.ptr.0) } < serial,
            "producer commit must not be mistaken for audio-thread application"
        );
        graph.process_block();
        assert!(
            unsafe { crate::audiograph::graph_edit_applied_batch_serial(graph.ptr.0) } >= serial,
            "processing the next block should acknowledge the committed batch"
        );
    }

    #[test]
    fn track_registration_and_deletion_keep_stable_registry_aligned() {
        let graph = TestLiveGraph::new("stable-track-registry-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add first sampler track");
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add second sampler track");

        let first = app.track_registry.id_at(0).expect("first stable id");
        let second = app.track_registry.id_at(1).expect("second stable id");
        assert_ne!(first, second);
        assert_eq!(app.track_registry.index_of(second), Some(1));

        app.graph_controller()
            .delete_track(0)
            .expect("delete first track");
        assert_eq!(app.track_registry.ids(), &[second]);
        assert_eq!(app.track_registry.index_of(second), Some(0));
        assert_eq!(app.track_registry.len(), app.tracks.len());
        graph.process_block();
    }

    #[test]
    fn sampler_track_added_to_existing_scenes_keeps_its_instrument_descriptor() {
        let graph = TestLiveGraph::new("sampler-track-existing-scenes-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(0, &[]),
                PatternSnapshot::new_default(0, &[]),
            ],
            0,
        );

        let track = app
            .graph_controller()
            .add_blank_sampler_track()
            .expect("add sampler track to the existing scenes");
        let sampler_descriptor = EffectDescriptor::builtin_sampler();
        let expected_param_count = sampler_descriptor.params.len();
        let enabled_param = sampler_descriptor
            .params
            .iter()
            .position(|param| param.name == "enabled")
            .expect("sampler enabled parameter");
        let expected_node_id = app.state.pattern.instrument_slots[track]
            .node_id
            .load(Ordering::Relaxed);
        let expected_modulator_node_id = app.state.pattern.instrument_slots[track]
            .modulator_node_id
            .load(Ordering::Relaxed);

        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetInstrumentParam {
                    track,
                    param_idx: 0,
                    value: 25.0,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));

        let sample_ids = app
            .state
            .launch_scene(
                1,
                app.tracks.len(),
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            )
            .expect("switch to the other existing scene");

        let slot = &app.state.pattern.instrument_slots[track];
        assert_eq!(
            slot.num_params.load(Ordering::Relaxed) as usize,
            expected_param_count,
        );
        assert_eq!(slot.node_id.load(Ordering::Relaxed), expected_node_id);
        assert_eq!(
            slot.modulator_node_id.load(Ordering::Relaxed),
            expected_modulator_node_id,
        );
        assert_eq!(slot.defaults.get(enabled_param), 1.0);
        // The other scene eagerly materialized its own pattern for the new
        // track, and registration seeded the track's loaded sample onto it.
        assert_eq!(sample_ids[track].0, app.graph.track_buffer_ids[track]);
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetInstrumentParam {
                    track,
                    param_idx: 0,
                    value: 50.0,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        graph.process_block();
    }

    #[test]
    fn sampler_tracks_added_to_committed_arrangement_extend_and_stamp_their_lanes() {
        let graph = TestLiveGraph::new("sampler-track-arrangement-lane-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(0, &[]),
                PatternSnapshot::new_default(0, &[]),
            ],
            0,
        );
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add first sampler track");
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("add second sampler track");

        let mut arrangement = ProjectArrangement::new(2, 16.0);
        arrangement.scene_lane = vec![
            SceneEvent {
                start_beat: 0.0,
                scene: 0,
            },
            SceneEvent {
                start_beat: 4.0,
                scene: 1,
            },
            SceneEvent {
                start_beat: 8.0,
                scene: 0,
            },
        ];
        app.state
            .set_committed_arrangement(Some(arrangement))
            .expect("install the two-track arrangement");

        let added = app
            .graph_controller()
            .add_blank_sampler_track()
            .expect("add third sampler track to the arrangement");
        let added_again = app
            .graph_controller()
            .add_blank_sampler_track()
            .expect("add fourth sampler track to the arrangement");
        assert_eq!(added, 2);
        assert_eq!(added_again, 3);
        let arrangement = app.state.committed_arrangement().expect("arrangement");
        assert_eq!(arrangement.track_lanes.len(), 4);
        for track in [added, added_again] {
            assert_eq!(
                arrangement.track_lanes[track]
                    .iter()
                    .map(|clip| (clip.start_beat, clip.end_beat))
                    .collect::<Vec<_>>(),
                vec![(0.0, 4.0), (4.0, 8.0), (8.0, 16.0)]
            );
        }
        assert!(
            app.state.committed_song().is_some(),
            "track registration recompiles the arrangement"
        );
        graph.process_block();
    }

    #[test]
    fn loading_arranged_project_over_another_clears_old_arrangement_before_tracks() {
        let graph = TestLiveGraph::new("arranged-project-reload-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_modulator_track()
            .expect("add first source-project track");
        app.graph_controller()
            .add_modulator_track()
            .expect("add second source-project track");
        app.state
            .set_committed_arrangement(Some(ProjectArrangement::new(2, 16.0)))
            .expect("install source-project arrangement");

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let project_name = format!(
            "__test-arranged-project-reload-{}-{nonce}",
            std::process::id()
        );
        let captured = app
            .capture_project(&project_name)
            .expect("arranged target project should capture");
        let project_path = crate::project::save_project(&project_name, &captured)
            .expect("arranged target project should save");
        let _cleanup = TestProjectFile(project_path);

        app.queue_project_load_named(&project_name)
            .expect("arranged target project should queue");
        for _ in 0..3 {
            app.advance_pending_project_load()
                .expect("target tracks should rebuild after clearing the old arrangement");
        }

        assert_eq!(app.tracks.len(), 2);
        assert!(app.state.committed_arrangement().is_none());
        assert!(app.state.committed_song().is_none());
        assert!(
            app.has_pending_project_load(),
            "the target arrangement is installed later, during finalization"
        );
        app.editor.pending_project_load = None;
        graph.process_block();
    }

    #[test]
    fn project_load_reuses_one_sample_buffer_across_tracks_patterns_and_take_chunks() {
        let graph = TestLiveGraph::new("project-sample-pool-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sample_path = std::env::temp_dir().join(format!(
            "eseq-project-sample-pool-{}-{nonce}.wav",
            std::process::id()
        ));
        write_test_wav(&sample_path);
        let _sample_cleanup = TestProjectFile(sample_path.clone());

        app.graph_controller()
            .add_track(&sample_path)
            .expect("add first source-project sampler track");
        app.graph_controller()
            .add_track(&sample_path)
            .expect("add second source-project sampler track");

        let project_name = format!(
            "__test-project-sample-pool-{}-{nonce}",
            std::process::id()
        );
        let mut project = app
            .capture_project(&project_name)
            .expect("sample-pool project should capture");
        project.patterns.push(project.patterns[0].clone());
        let mut take_chunk = project.patterns[0].clone();
        take_chunk.sample_paths[1] = None;
        take_chunk.sample_names[1].clear();
        project.take_pools = vec![
            crate::project::ProjectTrackTakePool {
                takes: vec![crate::project::ProjectTake {
                    id: 0,
                    name: "Take 1".to_string(),
                    total_len_steps: 64,
                    chunks: vec![take_chunk],
                }],
                next_take_id: 1,
            },
            crate::project::ProjectTrackTakePool::default(),
        ];
        let project_path = crate::project::save_project(&project_name, &project)
            .expect("sample-pool project should save");
        let _project_cleanup = TestProjectFile(project_path);

        app.queue_project_load_named(&project_name)
            .expect("sample-pool project should queue");
        for _ in 0..64 {
            if app
                .editor
                .pending_project_load
                .as_ref()
                .is_some_and(|pending| pending.built_patterns.len() == 2)
            {
                break;
            }
            app.advance_pending_project_load()
                .expect("sample-pool project should build its patterns");
        }
        let pending = app
            .editor
            .pending_project_load
            .as_ref()
            .expect("project should remain pending before finalization");
        assert_eq!(
            pending.sample_assets.len(),
            1,
            "all track and pattern references should share one loaded asset"
        );
        let shared_buffer_id = app.graph.track_buffer_ids[0];
        assert_eq!(app.graph.track_buffer_ids[1], shared_buffer_id);
        let snapshots = pending.built_patterns.clone();
        assert_eq!(snapshots.len(), 2);
        for snapshot in &snapshots {
            assert_eq!(snapshot.sample_ids[0].0, shared_buffer_id);
            assert_eq!(snapshot.sample_ids[1].0, shared_buffer_id);
        }

        let take_chunk = pending.project.take_pools[0].takes[0].chunks[0].clone();
        let mut sample_assets = {
            let pending = app
                .editor
                .pending_project_load
                .as_mut()
                .expect("project should remain pending for take conversion");
            std::mem::take(&mut pending.sample_assets)
        };
        let (take_snapshot, _, fallback_count) = app
            .project_pattern_into_snapshot(take_chunk, &mut sample_assets)
            .expect("take chunk should reuse the pending project asset pool");
        assert_eq!(fallback_count, 0);
        assert_eq!(sample_assets.len(), 1);
        assert_eq!(take_snapshot.sample_ids[0].0, shared_buffer_id);
        assert_eq!(
            take_snapshot.sample_ids[1].0, -1,
            "an empty non-owning take lane should remain unbound"
        );
        assert!(
            app.sample_buffer_path_registry
                .values()
                .all(|path| path == &sample_path),
            "an empty non-owning take lane must not load an arbitrary fallback WAV"
        );
        app.editor.pending_project_load = None;
        graph.process_block();
    }

    #[test]
    fn pattern_with_a_moved_sample_loads_unbound_instead_of_aborting_the_project() {
        let graph = TestLiveGraph::new("project-missing-sample-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sample_path = std::env::temp_dir().join(format!(
            "eseq-project-missing-sample-{}-{nonce}.wav",
            std::process::id()
        ));
        write_test_wav(&sample_path);
        let _sample_cleanup = TestProjectFile(sample_path.clone());
        app.graph_controller()
            .add_track(&sample_path)
            .expect("add sampler track");

        let project = app
            .capture_project("__test-project-missing-sample")
            .expect("project should capture");
        let mut pattern = project.patterns[0].clone();
        // The saved reference points at an asset the user has since moved:
        // neither the path nor the name resolves anywhere.
        pattern.sample_paths[0] = Some(
            std::env::temp_dir()
                .join(format!("eseq-moved-away-{nonce}.wav"))
                .to_string_lossy()
                .into_owned(),
        );
        pattern.sample_names[0] = format!("eseq-moved-away-{nonce}");

        let mut sample_assets = std::collections::HashMap::new();
        let (snapshot, _, fallback_count) = app
            .project_pattern_into_snapshot(pattern, &mut sample_assets)
            .expect("a moved sample must not abort the load");
        assert_eq!(
            snapshot.sample_ids[0].0, -1,
            "the unresolvable lane should be left unbound"
        );
        assert_eq!(
            fallback_count, 1,
            "the unresolvable lane should be reported as a fallback sample"
        );
        graph.process_block();
    }

    #[test]
    fn new_project_clears_in_flight_take_recording_and_transport_mode() {
        let graph = TestLiveGraph::new("new-project-capture-reset-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_modulator_track()
            .expect("add source-project track");
        app.song_transport_mode = crate::app::song_transport::SongTransportMode::ArrangementCapture;
        app.take_recording = Some(crate::app::take_recording::TakeRecordingSession::new(
            0.0,
            app.tracks.len(),
        ));

        app.start_new_project();

        assert!(
            app.take_recording.is_none(),
            "an in-flight take recording must not survive a project reset"
        );
        assert!(app.song_capture_take.is_none());
        assert!(app.active_runtime_song.is_none());
        assert_eq!(app.active_song_start_beat, None);
        assert_eq!(app.song_mirrored_row, None);
        assert_eq!(
            app.song_transport_mode,
            crate::app::song_transport::SongTransportMode::Stopped
        );
        graph.process_block();
    }

    #[test]
    fn new_project_clears_arrangement_state_before_removing_tracks() {
        let graph = TestLiveGraph::new("new-project-arrangement-reset-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_modulator_track()
            .expect("add source-project track");
        app.state
            .set_committed_arrangement(Some(ProjectArrangement::new(1, 16.0)))
            .expect("install source-project arrangement");
        app.use_arrangement = true;

        app.start_new_project();

        assert!(app.tracks.is_empty());
        assert!(app.state.committed_arrangement().is_none());
        assert!(app.state.committed_song().is_none());
        assert!(!app.use_arrangement);
        graph.process_block();
    }

    #[test]
    fn recorded_sampler_track_creation_undoes_and_redoes_with_stable_identity() {
        let graph = TestLiveGraph::new("track-creation-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("seed sampler track");
        let created = app.graph_controller()
            .add_blank_sampler_track()
            .expect("create sampler track");
        let created_id = app.track_registry.id_at(created).expect("created stable id");
        app.commit_created_track(created, "Add sampler track")
            .expect("record creation");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.track_registry.index_of(created_id), None);

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.track_registry.index_of(created_id), Some(1));
        assert_eq!(
            app.graph.track_instrument_types[1],
            InstrumentType::Sampler
        );
        graph.process_block();
    }

    #[test]
    fn recorded_modulator_track_creation_round_trips() {
        let graph = TestLiveGraph::new("modulator-track-creation-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let created = app.graph_controller().add_modulator_track().unwrap();
        let created_id = app.track_registry.id_at(created).unwrap();
        app.commit_created_track(created, "Add modulator track").unwrap();

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(created_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Modulator);
        assert_ne!(app.state.runtime.modulator_lids[1].load(Ordering::Acquire), 0);
        graph.process_block();
    }

    #[test]
    fn recorded_middle_track_deletion_restores_order_identity_and_pattern_lane() {
        let graph = TestLiveGraph::new("track-deletion-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for _ in 0..3 {
            app.graph_controller().add_blank_sampler_track()
                .expect("seed sampler track");
        }
        app.tracks[0] = "First".to_string();
        app.tracks[1] = "Deleted".to_string();
        app.tracks[2] = "Last".to_string();
        app.state.pattern.patterns[1].set_step_active(7, true);
        let effect_slot = app.add_builtin_effect_sync(1, "OTT")
            .expect("add retained track effect");
        let compressor_slot = app.add_builtin_effect_sync(0, "compressor")
            .expect("add sidechain compressor");
        let sidechain_param = app.graph.effect_descriptors[0][compressor_slot].params.iter()
            .position(|param| matches!(
                param.host_control,
                Some(crate::effects::HostControl::FxSidechain { .. })
            )).expect("compressor sidechain parameter");
        app.state.pattern.effect_chains[0][compressor_slot]
            .defaults.set(sidechain_param, 2.0);
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 9,
            name: "All tracks".to_string(),
            color: [0.2, 0.3, 0.4],
            collapsed: false,
            members: vec![0, 1, 2],
            bus_id: crate::sequencer::DEFAULT_BUS_A_ID,
        });
        let ids = app.track_registry.ids().to_vec();

        app.delete_track_recorded(1).expect("delete middle track");
        assert_eq!(app.tracks, ["First", "Last"]);
        assert_eq!(app.track_registry.ids(), &[ids[0], ids[2]]);
        assert_eq!(app.groups[0].members, [0, 1]);
        assert_eq!(
            app.state.pattern.effect_chains[0][compressor_slot]
                .defaults.get(sidechain_param).to_bits(),
            1.0f32.to_bits(),
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks, ["First", "Deleted", "Last"]);
        assert_eq!(app.track_registry.ids(), ids.as_slice());
        assert_eq!(app.groups[0].members, [0, 1, 2]);
        assert!(app.state.pattern.patterns[1].is_active(7));
        assert_eq!(app.graph.effect_descriptors[1][effect_slot].name, "OTT");
        assert_ne!(
            app.state.pattern.effect_chains[1][effect_slot]
                .node_id.load(Ordering::Relaxed),
            0,
        );
        assert_eq!(
            app.state.pattern.effect_chains[0][compressor_slot]
                .defaults.get(sidechain_param).to_bits(),
            2.0f32.to_bits(),
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.tracks, ["First", "Last"]);
        assert_eq!(app.track_registry.ids(), &[ids[0], ids[2]]);
        graph.process_block();
    }

    #[test]
    fn recorded_rack_track_deletion_restores_slot_effect_graph() {
        let graph = TestLiveGraph::new("rack-track-deletion-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for _ in 0..3 {
            app.graph_controller().add_blank_sampler_track()
                .expect("seed sampler track");
        }
        app.graph_controller().group_track_to_instrument_rack(1)
            .expect("build rack track");
        app.add_builtin_rack_slot_effect_sync(1, 0, "OTT")
            .expect("add rack slot effect");
        let rack_id = app.track_registry.id_at(1).unwrap();

        app.delete_track_recorded(1).expect("delete rack track");
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(rack_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Rack);
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[1]
            .clone().expect("rack state restored");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(rack.slots[0].effect_slots[0].node_id, 0);
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(rack_id), None);
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[1]
                .as_ref().unwrap().slots[0].effect_descriptors[0].name,
            "OTT",
        );
        graph.process_block();
    }

    #[test]
    fn recorded_custom_track_deletion_rebuilds_engine_route_at_original_index() {
        let graph = TestLiveGraph::new("custom-track-deletion-history-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "history-engine".to_string(),
            source: "history-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller().add_custom_track(
            "history-engine",
            engine_id,
            &manifest,
            &lib,
            CustomInstrumentRunMode::Instrument,
        ).expect("add custom track");
        app.editor.instrument_libs.push(lib);
        app.graph_controller().add_blank_sampler_track().unwrap();
        let custom_id = app.track_registry.id_at(1).unwrap();

        app.delete_track_recorded(1).expect("delete custom track");
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.track_registry.index_of(custom_id), Some(1));
        assert_eq!(app.graph.track_instrument_types[1], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[1], Some(engine_id));
        let engine = app.graph.engine_node_ids[engine_id].as_ref()
            .expect("engine runtime restored");
        assert_eq!(engine.route_gain_ids[1].len(), MAX_VOICES);
        assert!(engine.route_gain_ids[1].iter().all(|route| route[0] > 0 && route[1] > 0));
        graph.process_block();
    }

    #[test]
    fn free_patch_idle_route_stays_closed_while_transport_is_stopped() {
        assert_eq!(free_patch_idle_route_value(2, 2, false), 0.0);
        assert_eq!(free_patch_idle_route_value(1, 2, false), 0.0);
    }

    #[test]
    fn free_patch_idle_route_opens_only_target_track_while_transport_is_playing() {
        assert_eq!(free_patch_idle_route_value(2, 2, true), 1.0);
        assert_eq!(free_patch_idle_route_value(1, 2, true), 0.0);
    }

    #[test]
    fn ensure_custom_engine_runtime_rolls_back_before_runtime_publication() {
        let graph = TestLiveGraph::new("engine-materialization-rollback-test");
        let mut app = test_app(&graph);
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        set_test_graph_build_failure_after(4);

        let error = {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "test", &manifest, &lib)
                .expect_err("injected engine materialization failure should be returned")
        };
        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.engine_node_ids.len(), 1);
        assert!(app.graph.engine_node_ids[0].is_none());
        assert_eq!(
            app.state.runtime.engine_voice_counts[0].load(Ordering::Acquire),
            0
        );
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_voice_lids[0][voice].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_synth_node_ids[0][voice].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_modulator_node_ids[0][voice].load(Ordering::Acquire),
                0
            );
        }

        let created_nodes = take_test_graph_build_node_ids();
        let rolled_back_nodes = take_test_graph_build_rollback_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            rolled_back_nodes,
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        let created_connections = take_test_graph_build_connections();
        assert!(!created_connections.is_empty());
        assert_eq!(
            take_test_graph_build_rollback_connections(),
            created_connections
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        );

        for &node_id in &created_nodes {
            assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, node_id) });
        }
        graph.process_block();
        for node_id in created_nodes {
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            assert!(!unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            });
            assert_eq!(state_size, 0);
        }
    }

    #[test]
    fn ensure_custom_engine_runtime_publishes_only_complete_voice_pool() {
        let graph = TestLiveGraph::new("engine-materialization-commit-test");
        let mut app = test_app(&graph);
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        begin_test_graph_build_capture();

        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            app.graph_controller()
                .ensure_custom_engine_runtime(0, "test", &manifest, &lib)
                .expect("engine materialization should succeed");
        }

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("complete engine should be published");
        assert_eq!(engine.gatepitch_ids.len(), MAX_VOICES);
        assert_eq!(engine.modulator_ids.len(), MAX_VOICES);
        assert_eq!(engine.synth_ids.len(), MAX_VOICES);
        assert_eq!(
            app.state.runtime.engine_voice_counts[0].load(Ordering::Acquire),
            MAX_VOICES as u32
        );
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_voice_lids[0][voice].load(Ordering::Acquire),
                engine.gatepitch_ids[voice] as u64
            );
            assert_eq!(
                app.state.runtime.engine_synth_node_ids[0][voice].load(Ordering::Acquire),
                engine.synth_ids[voice] as u32
            );
            assert_eq!(
                app.state.runtime.engine_modulator_node_ids[0][voice].load(Ordering::Acquire),
                engine.modulator_ids[voice] as u32
            );
        }
        assert_eq!(take_test_graph_build_node_ids().len(), MAX_VOICES * 3);
        assert!(take_test_graph_build_rollback_node_ids().is_empty());
        assert!(take_test_graph_build_rollback_connections().is_empty());
        graph.process_block();
    }

    #[test]
    fn swap_custom_track_rebinds_only_the_target_and_collects_unreferenced_runtime() {
        let graph = TestLiveGraph::new("custom-track-swap-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 2);
        install_custom_track_swap_fixture(&mut app, &graph, 2, &manifest, &lib);
        let track_zero_sum = app.graph.track_node_ids[0].voice_sum_id;
        let track_one_slot_before =
            EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[1]);
        let macro_id = app
            .macro_engine
            .create_macro("swap cleanup", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::InstrumentParam {
                        param: "old tone".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.0,
                    1.0,
                    MacroCurve::Linear,
                )
                .expect("instrument mapping"),
            )
            .expect("instrument mapping should attach");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::EffectParam {
                        slot: 0,
                        effect: "filter".to_string(),
                        param: "enabled".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.2,
                    0.8,
                    MacroCurve::Linear,
                )
                .expect("effect mapping"),
            )
            .expect("effect mapping should attach");
        app.macro_engine.set_value(macro_id, 0.5);
        app.state
            .publish_macro_overrides(app.macro_engine.override_snapshot());

        let summary = app
            .graph_controller()
            .swap_custom_track_instrument(
                0,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("first track should swap to the new engine");
        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.graph.track_engine_ids, vec![Some(1), Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, track_zero_sum);
        assert_eq!(
            app.tracks[0], "new",
            "track name should follow the replacement instrument"
        );
        let macro_mappings = &app
            .macro_engine
            .macro_definition(macro_id)
            .expect("macro should survive the swap")
            .mappings;
        assert_eq!(macro_mappings.len(), 1);
        assert!(matches!(
            &macro_mappings[0].target,
            ParamTarget::EffectParam { .. }
        ));
        assert_eq!(
            app.macro_engine
                .override_value(&MacroParamKey::Instrument { track: 0, param: 0 }),
            None
        );
        assert!(app
            .macro_engine
            .override_value(&MacroParamKey::Effect {
                track: 0,
                slot: 0,
                param: 0,
            })
            .is_some_and(|value| (value - 0.5).abs() < 1.0e-6));
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("shared old engine must remain for track 2");
        assert!(old_engine.route_gain_ids[0].is_empty());
        assert_eq!(old_engine.route_gain_ids[1].len(), MAX_VOICES);
        let new_engine = app.graph.engine_node_ids[1]
            .as_ref()
            .expect("new engine should be materialized");
        assert_eq!(new_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert!(new_engine.route_gain_ids[1].is_empty());
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[1]),
            &track_one_slot_before,
        );
        assert_eq!(
            app.state.runtime.track_engine_ids[1].load(Ordering::Acquire),
            0
        );

        app.graph_controller()
            .swap_custom_track_instrument(
                1,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("second track should swap to the already materialized engine");
        assert_eq!(app.graph.track_engine_ids, vec![Some(1), Some(1)]);
        assert!(
            app.graph.engine_node_ids[0].is_none(),
            "unreferenced old graph runtime should be collected"
        );
        let new_engine = app.graph.engine_node_ids[1]
            .as_ref()
            .expect("new engine should remain live");
        assert_eq!(new_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(new_engine.route_gain_ids[1].len(), MAX_VOICES);
        graph.process_block();
    }

    #[test]
    fn sampler_track_converts_to_custom_instrument_without_replacing_its_shell() {
        let graph = TestLiveGraph::new("sampler-to-custom-conversion-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let sampler_ids = app.graph.track_node_ids[0].sampler_ids.clone();

        let summary = app
            .graph_controller()
            .replace_track_with_custom_instrument(
                0,
                "new",
                0,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("sampler track should convert to a custom instrument");

        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.tracks, vec!["new".to_string()]);
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Custom]
        );
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert!(app.graph.track_node_ids[0].sampler_ids.is_empty());
        assert!(app.graph.track_voice_lids[0].is_empty());
        assert_eq!(app.graph.track_buffer_ids[0], -1);
        assert_eq!(app.state.runtime.voice_counts[0].load(Ordering::Acquire), 0);
        assert!(sampler_ids
            .iter()
            .all(|node_id| { !app.graph.track_node_ids[0].sampler_ids.contains(node_id) }));
        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("new custom engine should remain live");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(
            app.state.export_pattern_repository()[0].sample_ids[0],
            (-1, String::new(), 44_100)
        );
        graph.process_block();
    }

    #[test]
    fn grouping_sampler_track_moves_insert_chain_into_rack_slot_without_replacing_shell() {
        let graph = TestLiveGraph::new("sampler-group-to-rack-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let pan_id = app.graph.track_node_ids[0].pan_id;
        let delay_id = app.graph.track_node_ids[0].delay_id;
        let effect_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("OTT should be inserted on the flat track");
        let effect_node = app.state.pattern.effect_chains[0][effect_slot]
            .node_id
            .load(Ordering::Relaxed);
        assert_ne!(effect_node, 0);

        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("flat sampler should group to a one-slot rack");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert_eq!(app.graph.track_node_ids[0].pan_id, pan_id);
        assert_eq!(app.graph.track_node_ids[0].delay_id, delay_id);
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), 1);
        assert_eq!(
            app.state.pattern.effect_chains[0][effect_slot]
                .node_id
                .load(Ordering::Relaxed),
            0,
            "track-level insert chain should be empty after grouping"
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should be published");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        graph.process_block();
    }

    #[test]
    fn grouping_sampler_track_is_undoable_with_its_insert_chain() {
        let graph = TestLiveGraph::new("sampler-group-to-rack-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        app.tracks[0] = "Original".to_string();
        let effect_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("OTT should be inserted on the flat track");
        app.state.pattern.effect_chains[0][effect_slot]
            .defaults
            .set(0, 0.63);

        app.group_track_to_instrument_rack_recorded(0)
            .expect("grouping should enter history");

        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        let grouped = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("grouped rack state");
        assert_eq!(grouped.slots[0].effect_descriptors[effect_slot].name, "OTT");
        assert_eq!(grouped.slots[0].effect_slots[effect_slot].defaults[0], 0.63);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Sampler);
        assert_eq!(app.tracks[0], "Original");
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        assert_eq!(app.graph.effect_descriptors[0][effect_slot].name, "OTT");
        assert_ne!(
            app.state.pattern.effect_chains[0][effect_slot]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            app.state.pattern.effect_chains[0][effect_slot]
                .defaults
                .get(0),
            0.63
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        let regrouped = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("regrouped rack state");
        assert_eq!(regrouped.slots[0].effect_descriptors[effect_slot].name, "OTT");
        assert_eq!(regrouped.slots[0].effect_slots[effect_slot].defaults[0], 0.63);
        graph.process_block();
    }

    #[test]
    fn rack_rebuild_defers_old_sampler_nodes_until_forced_reap() {
        let graph = TestLiveGraph::new("rack-deferred-sampler-teardown-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("flat sampler should group to a rack");

        let old_sampler_id = app.graph.track_node_ids[0].rack_slots[0].sampler_ids[0];
        assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, old_sampler_id) });
        graph.process_block();
        let mut sampler_state = vec![0.0_f32; crate::instruments::sampler::SAMPLER_STATE_SIZE];
        let mut state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });

        let mut rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should exist");
        rack.slots.push(rack.slots[0].clone());
        app.graph_controller()
            .rebuild_rack_slot_graph(0, &mut rack)
            .expect("rack topology should rebuild");
        assert_eq!(app.graph.deferred_rack_teardowns.len(), 1);
        assert_ne!(
            app.graph.track_node_ids[0].rack_slots[0].sampler_ids[0],
            old_sampler_id
        );

        graph.process_block();
        state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });

        app.graph_controller().force_reap_all_rack_teardowns();
        assert!(app.graph.deferred_rack_teardowns.is_empty());
        graph.process_block();
        state_size = 0;
        assert!(!unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_sampler_id,
                sampler_state.as_mut_ptr().cast(),
                sampler_state.len() * std::mem::size_of::<f32>(),
                &mut state_size,
            )
        });
        assert_eq!(state_size, 0);
    }

    #[test]
    fn adding_sampler_rack_slot_refreshes_live_topology_signature() {
        let graph = TestLiveGraph::new("rack-append-signature-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("one-slot rack should load");
        app.apply_recorded_rack_slot_add(0, "Add rack sample", |app| {
            app.graph_controller().add_sampler_slot_to_rack(0, sample)
        })
            .expect("second sampler slot should append");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should remain published");
        assert_eq!(rack.slots.len(), 2);
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&rack))
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            1
        );
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            2
        );
        graph.process_block();
    }

    #[test]
    fn same_engine_rack_rebuild_replaces_only_the_rack_route_generation() {
        let graph = TestLiveGraph::new("rack-deferred-engine-route-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "deferred-engine".to_string(),
            source: "deferred-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller()
            .add_custom_track(
                "deferred-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        app.editor.instrument_libs.push(lib);
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group to a rack");

        let route_idx = rack_slot_pool_index(0, 0).expect("rack route identity");
        let old_routes = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("engine runtime should exist")
            .route_gain_ids[route_idx]
            .clone();
        let mut rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should exist");
        rack.slots[0].instrument_run_mode = CustomInstrumentRunMode::FreePatch;
        app.graph_controller()
            .rebuild_rack_slot_graph(0, &mut rack)
            .expect("changed rack topology should rebuild");

        assert_eq!(app.graph.deferred_rack_teardowns.len(), 1);
        let reused_routes = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("engine runtime should remain live")
            .route_gain_ids[route_idx]
            .clone();
        assert_ne!(reused_routes, old_routes);
        assert_eq!(app.graph.deferred_rack_teardowns[0].engine_routes.len(), 1);
        assert_eq!(
            lisp_host::get_dgen_engine_enabled_voices(engine_id),
            1,
            "a reused live engine must not be retired"
        );

        app.graph_controller().force_reap_all_rack_teardowns();
        graph.process_block();
        assert!(app.graph.engine_node_ids[engine_id].is_some());
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("replacement engine runtime should survive")
                .route_gain_ids[route_idx],
            reused_routes
        );
    }

    #[test]
    fn flat_track_and_rack_slot_share_one_custom_engine_with_distinct_routes() {
        let graph = TestLiveGraph::new("shared-flat-and-rack-engine-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "shared-engine".to_string(),
            source: "shared-engine.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());
        app.graph_controller()
            .add_custom_track(
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("flat custom track should be created");
        let rack_track = app
            .graph_controller()
            .add_empty_layer_rack_track()
            .expect("rack track should be created");
        app.graph_controller()
            .add_custom_slot_to_rack(
                rack_track,
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("rack slot should consume the existing engine");
        app.apply_recorded_rack_slot_add(rack_track, "Add rack instrument", |app| {
            app.graph_controller().add_custom_slot_to_rack(
                rack_track,
                "shared-engine",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
        })
            .expect("a second rack slot should also consume the existing engine");

        let rack_route = rack_slot_pool_index(rack_track, 0).expect("rack route identity");
        let second_rack_route =
            rack_slot_pool_index(rack_track, 1).expect("second rack route identity");
        let engine = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("shared engine runtime should exist");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(engine.route_gain_ids[rack_route].len(), MAX_VOICES);
        assert_eq!(engine.route_gain_ids[second_rack_route].len(), MAX_VOICES);
        assert_eq!(
            app.state.runtime.rack_engine_route_engine_ids[rack_route].load(Ordering::Acquire),
            engine_id as u32
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            1
        );
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots
                .len(),
            2
        );
        assert_eq!(
            app.graph
                .engine_node_ids
                .iter()
                .filter(|engine| engine.is_some())
                .count(),
            1,
            "rack routing must not create a second DSP engine"
        );
        graph.process_block();
    }

    #[test]
    fn rack_custom_source_replacement_replays_retained_engines() {
        let graph = TestLiveGraph::new("rack-custom-source-history-test");
        let manifest = test_instrument_manifest();
        let first_lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        for (index, name) in ["first", "second"].into_iter().enumerate() {
            let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
                name: name.to_string(),
                source: format!("{name}.lisp"),
                manifest: manifest.clone(),
                lib_index: index,
                shared_runtime: true,
            });
            assert_eq!(engine_id, index);
            app.editor
                .instrument_libs
                .push(lisp_host::test_loaded_dgen_lib());
        }
        let rack_track = app
            .graph_controller()
            .add_empty_layer_rack_track()
            .unwrap();
        app.graph_controller()
            .add_custom_slot_to_rack(
                rack_track,
                "first",
                0,
                &manifest,
                &first_lib,
                CustomInstrumentRunMode::Instrument,
            )
            .unwrap();

        app.apply_recorded_rack_slot_source_replacement(
            rack_track,
            0,
            "Replace rack instrument",
            |app| {
                app.graph_controller().replace_rack_slot_with_custom(
                    rack_track,
                    0,
                    "second",
                    1,
                    &manifest,
                    CustomInstrumentRunMode::Instrument,
                )
            },
        )
        .unwrap();
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(1)
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(0)
        );
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[rack_track]
                .as_ref()
                .unwrap()
                .slots[0]
                .track_sound_state
                .engine_id,
            Some(1)
        );
        graph.process_block();
    }

    #[test]
    fn rack_teardown_queue_is_bounded_and_reaps_due_generations() {
        let graph = TestLiveGraph::new("rack-deferred-queue-test");
        let mut app = test_app_with_track_count(&graph, 0);
        for track_idx in 0..=MAX_DEFERRED_RACK_TEARDOWNS {
            app.graph_controller()
                .enqueue_deferred_rack_teardown(DeferredRackTeardown {
                    slots: Vec::new(),
                    engine_routes: Vec::new(),
                    track_idx,
                    due_at: Instant::now() + RACK_TEARDOWN_TAIL,
                });
        }
        app.graph_controller().reap_excess_rack_teardowns();
        assert_eq!(
            app.graph.deferred_rack_teardowns.len(),
            MAX_DEFERRED_RACK_TEARDOWNS
        );
        assert_eq!(app.graph.deferred_rack_teardowns[0].track_idx, 1);

        for teardown in &mut app.graph.deferred_rack_teardowns {
            teardown.due_at = Instant::now();
        }
        app.graph_controller().reap_due_rack_teardowns();
        assert!(app.graph.deferred_rack_teardowns.is_empty());
    }

    #[test]
    fn grouping_custom_track_preserves_instrument_engine_state_and_insert_fx() {
        let graph = TestLiveGraph::new("custom-group-to-rack-test");
        let mut manifest = test_instrument_manifest();
        manifest.params.push(crate::lisp_host::DGenParam {
            name: "tone".to_string(),
            cell_id: 0,
            cell_span: 1,
            default: 0.25,
            min: 0.0,
            max: 1.0,
            unit: None,
            hidden: false,
            group: None,
            env: None,
            role: None,
        });
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "test-synth".to_string(),
            source: "test-synth.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.graph_controller()
            .add_custom_track(
                "test-synth",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        let effect_slot = app
            .add_builtin_effect_sync(0, "OTT")
            .expect("custom track should accept OTT");
        app.state.pattern.instrument_slots[0].defaults.set(0, 0.73);
        app.state.pattern.effect_chains[0][effect_slot]
            .defaults
            .set(0, 0.63);
        let instrument_before = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let effect_node = app.state.pattern.effect_chains[0][effect_slot]
            .node_id
            .load(Ordering::Relaxed);
        let engine_routes_before = app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("custom engine runtime")
            .route_gain_ids[0]
            .clone();

        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group without losing its instrument");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_engine_ids[0], None);
        assert_eq!(
            app.graph.track_node_ids[0].rack_slots[0].engine_id,
            Some(engine_id)
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should be published");
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(rack.slots[0].track_sound_state.engine_id, Some(engine_id));
        assert_eq!(
            rack.slots[0].instrument_slot.node_id,
            instrument_before.node_id
        );
        assert_eq!(rack.slots[0].instrument_slot.defaults[0], 0.73);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        let stored_patterns = app.state.export_pattern_repository();
        assert!(stored_patterns.iter().all(|pattern| {
            pattern
                .rack_tracks
                .first()
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.first())
                .is_some_and(|slot| {
                    slot.instrument_type == InstrumentType::Custom
                        && slot.track_sound_state.engine_id == Some(engine_id)
                })
        }));
        let stored_slot = &stored_patterns[0].rack_tracks[0]
            .as_ref()
            .expect("stored rack")
            .slots[0];
        assert_eq!(stored_slot.instrument_slot.defaults[0], 0.73);
        assert_eq!(stored_slot.effect_slots[effect_slot].defaults[0], 0.63);
        let rack_route = rack_slot_pool_index(0, 0).expect("rack route identity");
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("custom engine should remain live")
                .route_gain_ids[rack_route],
            engine_routes_before,
            "grouping should move the existing engine route instead of rebuilding it"
        );
        assert!(app.graph.engine_node_ids[engine_id]
            .as_ref()
            .expect("custom engine should remain live")
            .route_gain_ids[0]
            .is_empty());
        assert!(app
            .rack_slot_instrument_descriptor(&rack.slots[0])
            .is_some());
        graph.process_block();
    }

    #[test]
    fn replacing_expanded_rack_instrument_preserves_slot_fx_and_defers_old_engine() {
        let graph = TestLiveGraph::new("rack-slot-instrument-replacement-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "test-synth".to_string(),
            source: "test-synth.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());
        app.graph_controller()
            .add_custom_track(
                "test-synth",
                engine_id,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("custom track should be created");
        app.graph_controller()
            .group_track_to_instrument_rack(0)
            .expect("custom track should group");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let effect_node = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .expect("rack state")
            .slots[0]
            .effect_slots[effect_slot]
            .node_id;
        let old_slot_pan_id = app.graph.track_node_ids[0].rack_slots[0].slot_pan_id;

        app.apply_recorded_rack_slot_source_replacement(
            0,
            0,
            "Replace rack sample",
            |app| app.graph_controller().replace_rack_slot_with_sampler(
                0,
                0,
                Path::new("assets/ir/lexicon-300-rich-plate.wav"),
            ),
        )
            .expect("expanded rack instrument should be replaceable");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state should remain published");
        assert_eq!(rack.slots.len(), 1, "replacement must not append a layer");
        assert_eq!(rack.slots[0].instrument_type, InstrumentType::Sampler);
        assert_eq!(rack.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert_eq!(rack.slots[0].effect_descriptors[effect_slot].name, "OTT");
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), 1);
        assert_eq!(app.graph.track_node_ids[0].rack_slots[0].engine_id, None);
        assert!(
            app.graph.engine_node_ids[engine_id].is_some(),
            "the replaced instrument runtime must survive for the release tail"
        );
        assert_eq!(
            lisp_host::get_dgen_engine_enabled_voices(engine_id),
            0,
            "an unreferenced deferred engine must stop consuming DSP"
        );
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let undone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .unwrap();
        assert_eq!(undone.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(undone.slots[0].effect_slots[effect_slot].node_id, effect_node);
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .unwrap();
        assert_eq!(redone.slots[0].instrument_type, InstrumentType::Sampler);
        assert_eq!(redone.slots[0].effect_slots[effect_slot].node_id, effect_node);
        graph.process_block();
        let mut old_panner_state =
            vec![0.0_f32; crate::effects::stereo_panner::STEREO_PANNER_STATE_SIZE];
        let mut old_panner_state_size = 0;
        assert!(unsafe {
            crate::audiograph::get_node_state_into(
                graph.ptr.0,
                old_slot_pan_id,
                old_panner_state.as_mut_ptr().cast(),
                old_panner_state.len() * std::mem::size_of::<f32>(),
                &mut old_panner_state_size,
            )
        });
        assert_eq!(
            old_panner_state[crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE as usize],
            1.0,
            "the outgoing custom slot must stop carrying future shared-engine audio"
        );
        app.graph_controller().force_reap_all_rack_teardowns();
        graph.process_block();
        assert!(
            app.graph.engine_node_ids[engine_id].is_none(),
            "the replaced instrument runtime should retire when its tail is reaped"
        );
    }

    #[test]
    fn rack_preset_save_load_and_sound_promotion_preserve_slot_fx() {
        let graph = TestLiveGraph::new("rack-preset-promotion-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let preset_name = format!("rack-preset-test-{}", std::process::id());
        let preset_path = app
            .save_rack_preset(0, &preset_name, true)
            .expect("rack preset should save");
        let _preset_guard = TestProjectFile(preset_path);

        app.delete_rack_slot_effect_slot(0, 0, 0)
            .expect("live rack effect should be removable");
        app.load_rack_preset_onto_track(0, &preset_name)
            .expect("rack preset should restore");
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("restored rack state");
        assert_eq!(restored.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(restored.slots[0].effect_slots[0].node_id, 0);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let undone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("pre-preset rack state should be restored");
        assert_eq!(undone.slots[0].effect_slots[0].node_id, 0);
        assert_eq!(undone.slots[0].custom_effect_names[0], None);

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack preset should be restored on redo");
        assert_eq!(redone.slots[0].effect_descriptors[0].name, "OTT");
        assert_ne!(redone.slots[0].effect_slots[0].node_id, 0);

        let sound_path = app
            .promote_preset_to_sound(0, &preset_name)
            .expect("rack preset should promote to Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());
        let sound = crate::project::load_sound_preset(&sound_path)
            .expect("promoted Sound should be readable");
        assert_eq!(
            sound.rack.slots[0].custom_effects[0].as_deref(),
            Some("builtin:OTT")
        );
        graph.process_block();
    }

    #[test]
    fn deleting_rack_slot_with_fx_removes_chain_state_and_lease_host() {
        let graph = TestLiveGraph::new("delete-rack-slot-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        assert!(app
            .editor
            .effect_chain_leases
            .contains_host(FxChainLocator::RackSlot { track: 0, slot: 0 }));

        app.graph_controller()
            .delete_rack_slot(0, 0)
            .expect("rack slot with FX should delete cleanly");

        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack container should remain");
        assert!(rack.slots.is_empty());
        assert!(app.graph.track_node_ids[0].rack_slots.is_empty());
        assert!(!app
            .editor
            .effect_chain_leases
            .contains_host(FxChainLocator::RackSlot { track: 0, slot: 0 }));
        graph.process_block();
    }

    #[test]
    fn recorded_rack_slot_delete_undoes_with_identity_fx_and_macro_state() {
        let graph = TestLiveGraph::new("recorded-delete-rack-slot-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf(), sample.to_path_buf()])
            .expect("rack samples should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let track_id = app.track_registry.id_at(0).expect("stable track id");
        let deleted_id = app.device_registry.rack_slot(track_id, 0);

        app.apply_recorded_instrument_binding_mutation(0, "Delete rack layer", |app| {
            app.graph_controller().delete_rack_slot(0, 0)
        })
        .expect("rack deletion should record");
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref().expect("rack state").slots.len(),
            1
        );

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should restore");
        assert_eq!(restored.slots.len(), 2);
        assert_eq!(restored.slots[0].effect_descriptors[0].name, "OTT");
        assert_eq!(app.device_registry.rack_slot(track_id, 0), deleted_id);

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref().expect("rack state").slots.len(),
            1
        );
        graph.process_block();
    }

    #[test]
    fn recorded_scene_creation_redo_restores_the_same_track_pattern_identity() {
        let graph = TestLiveGraph::new("recorded-scene-create-test");
        let mut app = test_app_with_track_count(&graph, 1);
        let created_scene = app.apply_recorded_scene_structure_mutation(
            "Create scene",
            |app| {
                let source_scene = app.state.current_scene_index();
                let new_scene = app.state.clone_pattern(
                    app.tracks.len(),
                    &app.graph.track_buffer_ids,
                    &app.graph.track_sample_rates,
                    &app.tracks,
                    &app.graph.track_instrument_types,
                );
                app.clone_bus_pattern_from_to(source_scene, new_scene);
                Ok(new_scene)
            },
        ).expect("scene creation should record");
        let created_pattern = app.state.scene_track_pattern_id(created_scene, 0)
            .expect("created scene track pattern id");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.state.scene_count(), 1);
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.state.scene_count(), 2);
        assert_eq!(
            app.state.scene_track_pattern_id(created_scene, 0),
            Some(created_pattern),
            "redo must restore the captured PatternId instead of allocating another"
        );
        graph.process_block();
    }

    #[test]
    fn recorded_process_configuration_restores_stable_instance_and_lane_state() {
        let graph = TestLiveGraph::new("recorded-process-config-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track()
            .expect("sampler track");
        let instance_id = crate::process::ProcessInstanceId(77);
        let chain = crate::process::TrackProcessChain {
            slots: vec![crate::process::TrackProcessSlot {
                instance_id,
                instance_name: Some("history-process".to_string()),
                class_name: "history-process".to_string(),
                enabled: true,
                project_layer: false,
                inlets: std::collections::BTreeMap::new(),
                lanes: std::collections::BTreeMap::from([(
                    "amount".to_string(),
                    crate::process::ProcessLane { values: vec![0.0, 1.0] },
                )]),
                bindings: std::collections::BTreeMap::new(),
            }],
        };
        assert!(app.state.set_track_process_chain(0, chain));

        app.apply_recorded_scene_structure_mutation("Edit process chain", |app| {
            let enabled = app.state.set_track_process_slot_enabled(0, instance_id, false);
            let lane = app.state.set_process_lane_value(0, instance_id, "amount", 1, 0.75);
            (enabled && lane).then_some(())
                .ok_or_else(|| "process edit failed".to_string())
        }).expect("process edit should record");
        let edited = app.state.track_process_chain(0).expect("edited process chain");
        assert_eq!(edited.slots[0].instance_id, instance_id);
        assert!(!edited.slots[0].enabled);
        assert_eq!(edited.slots[0].lanes["amount"].values[1], 0.75);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.track_process_chain(0).expect("restored process chain");
        assert_eq!(restored.slots[0].instance_id, instance_id);
        assert!(restored.slots[0].enabled);
        assert_eq!(restored.slots[0].lanes["amount"].values[1], 1.0);
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.track_process_chain(0).expect("redone process chain");
        assert_eq!(redone.slots[0].instance_id, instance_id);
        assert!(!redone.slots[0].enabled);
        assert_eq!(redone.slots[0].lanes["amount"].values[1], 0.75);
        graph.process_block();
    }

    #[test]
    fn completed_recording_take_is_one_exact_history_entry() {
        let graph = TestLiveGraph::new("recording-take-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track()
            .expect("sampler track");
        let before_step_2 = app.state.capture_step_snapshot(0, 2);
        let before_step_7 = app.state.capture_step_snapshot(0, 7);

        app.begin_recording_take_history().expect("begin take");
        app.state.pattern.patterns[0].toggle_step(2);
        app.state.pattern.chord_data[0].add_note_with_timing(2, 3.0, 0.5, 0.125);
        app.mark_recording_take_changed();
        app.state.pattern.patterns[0].toggle_step(7);
        app.state.pattern.chord_data[0].add_note_with_timing(7, -4.0, 1.25, 0.0);
        app.mark_recording_take_changed();
        app.finish_recording_take_history().expect("finish take")
            .expect("changed take should create history");

        assert_eq!(app.history.undo_len(), 1, "the complete take must be atomic");
        let after_step_2 = app.state.capture_step_snapshot(0, 2);
        let after_step_7 = app.state.capture_step_snapshot(0, 7);
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(crate::app::history::step_snapshot_bit_exact_eq(
            &before_step_2,
            &app.state.capture_step_snapshot(0, 2),
        ));
        assert!(crate::app::history::step_snapshot_bit_exact_eq(
            &before_step_7,
            &app.state.capture_step_snapshot(0, 7),
        ));
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert!(crate::app::history::step_snapshot_bit_exact_eq(
            &after_step_2,
            &app.state.capture_step_snapshot(0, 2),
        ));
        assert!(crate::app::history::step_snapshot_bit_exact_eq(
            &after_step_7,
            &app.state.capture_step_snapshot(0, 7),
        ));
        graph.process_block();
    }

    #[test]
    fn cancelled_recording_take_restores_initial_state_without_history() {
        let graph = TestLiveGraph::new("recording-take-cancel-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller().add_blank_sampler_track()
            .expect("sampler track");
        let before = app.state.capture_step_snapshot(0, 4);

        app.begin_recording_take_history().expect("begin take");
        app.state.pattern.patterns[0].toggle_step(4);
        app.state.pattern.chord_data[0].add_note(4, 7.0);
        app.mark_recording_take_changed();
        assert!(app.cancel_recording_take_history().expect("cancel take"));

        assert_eq!(app.history.undo_len(), 0);
        assert!(crate::app::history::step_snapshot_bit_exact_eq(
            &before,
            &app.state.capture_step_snapshot(0, 4),
        ));
        graph.process_block();
    }

    #[test]
    fn two_slot_rack_hosts_builtin_and_compiled_fx_independently() {
        let graph = TestLiveGraph::new("two-rack-slot-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav").to_path_buf();
        app.graph_controller()
            .add_sampler_rack_track(&[sample.clone(), sample])
            .expect("two-slot rack should load");
        let builtin_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("slot 0 should accept OTT");
        let compiled_slot = app
            .add_rack_slot_effect_sync(0, 1, "stereo-tremolo")
            .expect("slot 1 should accept a compiled effect");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_ne!(before.slots[0].effect_slots[builtin_slot].node_id, 0);
        assert_ne!(before.slots[1].effect_slots[compiled_slot].node_id, 0);
        let builtin_node = before.slots[0].effect_slots[builtin_slot].node_id;

        app.delete_rack_slot_effect_slot(0, 1, compiled_slot)
            .expect("compiled effect should be removable independently");
        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            after.slots[0].effect_slots[builtin_slot].node_id,
            builtin_node
        );
        assert_eq!(after.slots[1].effect_slots[compiled_slot].node_id, 0);
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_reorder_moves_occupied_neighbors_and_leases_together() {
        let graph = TestLiveGraph::new("rack-slot-fx-reorder-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let ott_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let filter_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        assert_eq!((ott_slot, filter_slot), (0, 1));
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let ott_node = before.slots[0].effect_slots[0].node_id;
        let filter_node = before.slots[0].effect_slots[1].node_id;

        app.move_rack_slot_effect_slot_sync(0, 0, 0, 1)
            .expect("occupied neighboring effects should reorder");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_descriptors[1].name, "OTT");
        assert_eq!(after.slots[0].effect_slots[0].node_id, filter_node);
        assert_eq!(after.slots[0].effect_slots[1].node_id, ott_node);
        graph.process_block();
    }

    #[test]
    fn deleting_rack_slot_effect_compacts_state_and_lease_slots() {
        let graph = TestLiveGraph::new("rack-slot-fx-delete-compaction-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        app.add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let filter_node = before.slots[0].effect_slots[1].node_id;
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&before))
        );

        app.delete_rack_slot_effect_slot(0, 0, 0)
            .expect("first effect should delete");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_slots[0].node_id, filter_node);
        assert_eq!(after.slots[0].effect_slots[1].node_id, 0);
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&after))
        );
        let replacement_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("the first empty compacted slot should remain installable");
        assert_eq!(replacement_slot, 1);
        app.move_rack_slot_effect_slot_sync(0, 0, 0, 1)
            .expect("the compacted lease should remain movable");
        let reordered = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            app.graph.track_node_ids[0].rack_signature,
            Some(rack_topology_signature(&reordered))
        );
        graph.process_block();
    }

    #[test]
    fn recorded_rack_slot_effect_delete_restores_identity_values_and_macro_mapping() {
        let graph = TestLiveGraph::new("rack-slot-fx-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app.apply_recorded_rack_effect_chain_mutation(
            0,
            0,
            "Add rack-slot effect",
            |app| app.add_builtin_rack_slot_effect_sync(0, 0, "filter"),
        ).expect("recorded rack filter add should succeed");
        let track_id = app.track_registry.id_at(0).unwrap();
        let rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        let effect_id = app.device_registry.rack_audio_effect(rack_slot_id, effect_slot);
        app.state.update_rack_slot_in_all_pattern_snapshots(0, 0, |slot| {
            slot.effect_slots[effect_slot].defaults[0] = 0.37;
        });
        app.state.update_rack_macros_for_all_pattern_snapshots(0, |macros| {
            macros[0].mappings.push(crate::sequencer::RackMacroMapping {
                target: crate::sequencer::RackMacroTarget::SlotEffectParam {
                    slot: 0,
                    effect_slot,
                    param: "cutoff".to_string(),
                    param_index: 0,
                },
                range_min: 0.0,
                range_max: 1.0,
                curve: crate::sequencer::RackMacroCurve::Linear,
            });
        });

        app.apply_recorded_rack_effect_chain_mutation(
            0,
            0,
            "Delete rack-slot effect",
            |app| app.delete_rack_slot_effect_slot(0, 0, effect_slot),
        ).expect("recorded rack filter delete should succeed");
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(restored.slots[0].effect_descriptors[effect_slot].name, "Filter");
        assert_eq!(restored.slots[0].effect_slots[effect_slot].defaults[0].to_bits(), 0.37_f32.to_bits());
        assert_eq!(restored.macros[0].mappings.len(), 1);
        assert_eq!(
            app.device_registry.rack_audio_effect_location(effect_id),
            Some((rack_slot_id, effect_slot))
        );
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(redone.slots[0].effect_slots[effect_slot].node_id, 0);
        assert!(redone.macros[0].mappings.is_empty());
        graph.process_block();
    }

    #[test]
    fn inserting_rack_slot_effect_before_existing_effect_shifts_state_and_leases() {
        let graph = TestLiveGraph::new("rack-slot-fx-insert-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        app.add_builtin_rack_slot_effect_sync(0, 0, "OTT")
            .expect("rack slot should accept OTT");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let ott_node = before.slots[0].effect_slots[0].node_id;

        let inserted_slot = app
            .insert_builtin_rack_slot_effect_before_slot_sync(0, 0, 0, "filter")
            .expect("filter should insert before OTT");

        assert_eq!(inserted_slot, 0);
        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(after.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(after.slots[0].effect_descriptors[1].name, "OTT");
        assert_eq!(after.slots[0].effect_slots[1].node_id, ott_node);
        app.move_rack_slot_effect_slot_sync(0, 0, 1, 0)
            .expect("shifted OTT lease should remain movable");
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_plocks_preserve_defaults_and_node_identity() {
        let graph = TestLiveGraph::new("rack-slot-fx-plock-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "filter")
            .expect("rack slot should accept filter");
        let before = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let default = before.slots[0].effect_slots[effect_slot].defaults[1];

        app.set_rack_slot_effect_plocks(0, 0, effect_slot, &[2, 3], 1, 0.75)
            .expect("selected rack effect steps should accept p-locks");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let slot = &after.slots[0].effect_slots[effect_slot];
        assert_eq!(slot.defaults[1], default);
        assert_eq!(slot.plocks[2][1], Some(0.75));
        assert_eq!(slot.plocks[3][1], Some(0.75));
        assert!(slot.plock_param_ids[2][1].is_some());
        graph.process_block();
    }

    #[test]
    fn rack_slot_effect_options_resolve_descriptor_labels() {
        let graph = TestLiveGraph::new("rack-slot-fx-option-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack sample should load");
        let effect_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "Phaser-Flanger")
            .expect("rack slot should accept Phaser-Flanger");
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let circuit_param = rack.slots[0].effect_descriptors[effect_slot]
            .params
            .iter()
            .position(|param| param.name == "phaser circuit")
            .expect("Phaser-Flanger should expose its circuit option");

        app.set_rack_slot_effect_param_option(0, 0, effect_slot, circuit_param, "stack")
            .expect("rack option labels should route through the rack host");

        let after = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        assert_eq!(
            after.slots[0].effect_slots[effect_slot].defaults[circuit_param],
            0.0
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_replaces_instrument_container_but_preserves_track_fx() {
        let graph = TestLiveGraph::new("sound-swap-preserves-track-fx-test");
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("target sampler track should be created");
        let track_fx_slot = app
            .add_builtin_effect_sync(0, "filter")
            .expect("target track should accept a track-level effect");
        let track_fx_node = app.state.pattern.effect_chains[0][track_fx_slot]
            .node_id
            .load(Ordering::Relaxed);
        let original_buffer_id = app.graph.track_buffer_ids[0];
        let original_name = app.tracks[0].clone();

        let ott = EffectDescriptor::builtin_ott();
        let ott_snapshot = EffectSlotSnapshot::new_default(&ott, 0);
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Test Rack Sound".to_string(),
                tags: vec!["test".to_string()],
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: crate::sequencer::TrackId(1),
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: vec![crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some("assets/ir/lexicon-300-rich-plate.wav".to_string()),
                        sample_name: Some("plate".to_string()),
                        instrument_name: None,
                    }],
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: vec![crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode: crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: vec![crate::project::ProjectEffectSlot::from(&ott_snapshot)],
                    custom_effects: vec![Some("builtin:OTT".to_string())],
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some("assets/ir/lexicon-300-rich-plate.wav".to_string()),
                    sample_name: Some("plate".to_string()),
                }],
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-swap-{}-{}.sound",
            std::process::id(),
            track_fx_node
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize test Sound"),
        )
        .expect("write test Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("Sound should load onto the target track");

        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id
                .load(Ordering::Relaxed),
            track_fx_node,
            "Sound swap must not replace track-level FX"
        );
        let rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("Sound rack should be live");
        assert_ne!(rack.slots[0].effect_slots[0].node_id, 0);
        assert_eq!(rack.slots[0].effect_descriptors[0].name, "OTT");

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Sampler);
        assert_eq!(app.graph.track_buffer_ids[0], original_buffer_id);
        assert_eq!(app.tracks[0], original_name);
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id.load(Ordering::Relaxed),
            track_fx_node,
            "undo must preserve the track-level effect chain",
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.effect_chains[0][track_fx_slot]
                .node_id.load(Ordering::Relaxed),
            track_fx_node,
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_over_rack_undoes_as_one_container_replacement() {
        let graph = TestLiveGraph::new("sound-rack-replacement-history-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample_path.to_path_buf()])
            .expect("target rack should be created");
        let filter_slot = app
            .add_builtin_rack_slot_effect_sync(0, 0, "Filter")
            .expect("target rack should accept Filter");
        app.set_rack_slot_effect_param(0, 0, filter_slot, 2, 2_345.0)
            .expect("target rack Filter should accept a cutoff value");
        let track_id = app.track_registry.id_at(0).unwrap();
        let original_rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        let original_effect_id = app.device_registry
            .rack_audio_effect(original_rack_slot_id, filter_slot);

        let ott = EffectDescriptor::builtin_ott();
        let ott_snapshot = EffectSlotSnapshot::new_default(&ott, 0);
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Replacement Rack".to_string(),
                tags: Vec::new(),
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: crate::sequencer::TrackId(1),
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: (0..2).map(|slot| crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some(sample_path.to_string_lossy().into_owned()),
                        sample_name: Some(format!("replacement-{}", slot + 1)),
                        instrument_name: None,
                    }).collect(),
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: (0..2).map(|slot| crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode: crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: vec![crate::project::ProjectEffectSlot::from(&ott_snapshot)],
                    custom_effects: vec![Some("builtin:OTT".to_string())],
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some(sample_path.to_string_lossy().into_owned()),
                    sample_name: Some(format!("replacement-{}", slot + 1)),
                }).collect(),
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-rack-history-{}-{}.sound",
            std::process::id(),
            original_effect_id.0,
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize replacement Sound"),
        ).expect("write replacement Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("replacement Sound should load");
        let replacement = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("replacement rack should be live");
        assert_eq!(replacement.slots.len(), 2);
        assert_eq!(replacement.slots[0].effect_descriptors[0].name, "OTT");
        let replacement_rack_slot_id = app.device_registry.rack_slot(track_id, 0);
        assert_ne!(replacement_rack_slot_id, original_rack_slot_id);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let restored = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("original rack should be restored");
        assert_eq!(restored.slots[0].effect_descriptors[0].name, "Filter");
        assert_eq!(restored.slots[0].effect_slots[0].defaults[2].to_bits(), 2_345.0_f32.to_bits());
        assert_eq!(
            app.device_registry.rack_slot_location(original_rack_slot_id),
            Some((track_id, 0)),
        );
        assert_eq!(
            app.device_registry.rack_audio_effect_location(original_effect_id),
            Some((original_rack_slot_id, 0)),
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let redone = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("replacement rack should be restored on redo");
        assert_eq!(redone.slots.len(), 2);
        assert_eq!(redone.slots[0].effect_descriptors[0].name, "OTT");
        assert_eq!(
            app.device_registry.rack_slot_location(replacement_rack_slot_id),
            Some((track_id, 0)),
        );
        graph.process_block();
    }

    #[test]
    fn loading_sound_rack_over_custom_instrument_undoes_and_redoes() {
        let graph = TestLiveGraph::new("sound-rack-over-custom-history-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "old".to_string(),
            source: "old.lisp".to_string(),
            manifest: manifest.clone(),
            lib_index: 0,
            shared_runtime: true,
        });
        assert_eq!(engine_id, 0);
        app.editor.instrument_libs.push(lisp_host::test_loaded_dgen_lib());
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let track_id = app.track_registry.id_at(0).expect("track id");
        let original_name = app.tracks[0].clone();

        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: "Sampler Rack".to_string(),
                tags: Vec::new(),
                author: "test".to_string(),
            },
            track: crate::project::ProjectTrack {
                id: track_id,
                color: None,
                collapsed: false,
                kind: crate::project::ProjectTrackKind::Rack {
                    routing: crate::project::ProjectRackRouting::Broadcast,
                    slots: vec![crate::project::ProjectRackTrackSlot {
                        instrument_type: crate::project::ProjectInstrumentType::Sampler,
                        sample_path: Some(sample_path.to_string_lossy().into_owned()),
                        sample_name: Some("rack sample".to_string()),
                        instrument_name: None,
                    }],
                },
            },
            rack: crate::project::ProjectRackTrackPattern {
                macros: crate::project::default_project_rack_macros(),
                routing: crate::project::ProjectRackRouting::Broadcast,
                slots: vec![crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode:
                        crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: crate::project::ProjectEffectSlot::default(),
                    effect_slots: Vec::new(),
                    custom_effects: Vec::new(),
                    track_sound_state: crate::project::ProjectTrackSoundState::default(),
                    sample_path: Some(sample_path.to_string_lossy().into_owned()),
                    sample_name: Some("rack sample".to_string()),
                }],
            },
        };
        let sound_path = std::env::temp_dir().join(format!(
            "eseq-sound-rack-over-custom-history-{}.sound",
            std::process::id(),
        ));
        std::fs::write(
            &sound_path,
            serde_json::to_string(&sound).expect("serialize Sound"),
        ).expect("write Sound");
        let _sound_guard = TestProjectFile(sound_path.clone());

        app.load_sound_onto_track(0, &sound_path)
            .expect("Sound rack should replace the custom instrument");
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));
        assert_eq!(app.tracks[0], original_name);
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        assert_eq!(
            app.graph.engine_node_ids[engine_id]
                .as_ref()
                .expect("retained custom engine should be rebuilt")
                .route_gain_ids[0]
                .len(),
            MAX_VOICES,
        );

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .expect("Sound rack should be restored")
                .slots
                .len(),
            1,
        );
        graph.process_block();
    }

    #[test]
    fn replacing_rack_with_saved_instrument_undoes_and_redoes() {
        let graph = TestLiveGraph::new("rack-to-saved-instrument-history-test");
        let manifest = test_instrument_manifest();
        let mut app = test_app_with_track_count(&graph, 0);
        let sample_path = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample_path.to_path_buf()])
            .expect("rack should be created");
        let original_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("rack state");
        let source = "target.lisp";
        let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
            name: "target".to_string(),
            source: source.to_string(),
            manifest,
            lib_index: 0,
            shared_runtime: true,
        });
        app.editor.instrument_libs.push(lisp_host::test_loaded_dgen_lib());

        app.try_swap_track_to_cached_saved_instrument_sync(
            0,
            "target",
            source,
            CustomInstrumentRunMode::Instrument,
        )
        .expect("cached instrument should be found")
        .expect("rack should accept a saved instrument");
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));

        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        let restored_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone()
            .expect("undo should restore the rack");
        assert_eq!(restored_rack.slots.len(), original_rack.slots.len());
        assert_eq!(restored_rack.slots[0].sample_id, original_rack.slots[0].sample_id);

        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Custom);
        assert_eq!(app.graph.track_engine_ids[0], Some(engine_id));
        assert!(app.state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        graph.process_block();
    }

    #[test]
    fn rack_to_sampler_conversion_keeps_rack_binding_when_voice_build_fails() {
        let graph = TestLiveGraph::new("rack-to-sampler-conversion-rollback-test");
        let mut app = test_app_with_track_count(&graph, 0);
        let sample = Path::new("assets/ir/lexicon-300-rich-plate.wav");
        app.graph_controller()
            .add_sampler_rack_track(&[sample.to_path_buf()])
            .expect("rack should be created");
        let before_nodes = app.graph.track_node_ids[0].rack_slots.clone();
        let before_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should be live");
        let buffer_id = crate::instruments::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");
        set_test_graph_build_failure_after(4);

        let error = app.graph_controller()
            .replace_rack_track_with_sampler(0, buffer_id, 48_000, "restored")
            .expect_err("injected sampler voice failure should abort conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.track_instrument_types[0], InstrumentType::Rack);
        assert_eq!(app.graph.track_node_ids[0].rack_slots.len(), before_nodes.len());
        for (after, before) in app.graph.track_node_ids[0].rack_slots.iter().zip(&before_nodes) {
            assert_eq!(after.sampler_ids, before.sampler_ids);
            assert_eq!(after.sampler_gatepitch_ids, before.sampler_gatepitch_ids);
            assert_eq!(after.sampler_modulator_ids, before.sampler_modulator_ids);
            assert_eq!(after.slot_sum_l_id, before.slot_sum_l_id);
            assert_eq!(after.slot_sum_r_id, before.slot_sum_r_id);
            assert_eq!(after.slot_pan_id, before.slot_pan_id);
        }
        let after_rack = app.state.pattern.rack_tracks.lock().unwrap()[0]
            .clone().expect("rack should remain live");
        assert_eq!(after_rack.slots.len(), before_rack.slots.len());
        assert_eq!(
            after_rack.slots[0].sample_id,
            before_rack.slots[0].sample_id,
        );
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>(),
        );
        graph.process_block();
    }

    #[test]
    fn sampler_to_custom_conversion_keeps_sampler_binding_when_engine_build_fails() {
        let graph = TestLiveGraph::new("sampler-to-custom-conversion-rollback-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 0);
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("blank sampler track should be created");
        let old_nodes = app.graph.track_node_ids[0].clone();
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let old_buffer_id = app.graph.track_buffer_ids[0];
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .replace_track_with_custom_instrument(
                0,
                "new",
                0,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect_err("injected engine failure should abort sampler conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Sampler]
        );
        assert_eq!(app.graph.track_engine_ids, vec![None]);
        assert_eq!(app.graph.track_buffer_ids, vec![old_buffer_id]);
        assert_eq!(
            app.graph.track_node_ids[0].sampler_ids,
            old_nodes.sampler_ids
        );
        assert_eq!(
            app.graph.track_node_ids[0].sampler_gatepitch_ids,
            old_nodes.sampler_gatepitch_ids
        );
        assert_eq!(
            app.graph.track_node_ids[0].sampler_modulator_ids,
            old_nodes.sampler_modulator_ids
        );
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        assert!(app.graph.engine_node_ids[0].is_none());
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        graph.process_block();
    }

    #[test]
    fn custom_track_converts_to_sampler_without_replacing_its_shell() {
        let graph = TestLiveGraph::new("custom-to-sampler-conversion-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let voice_sum_id = app.graph.track_node_ids[0].voice_sum_id;
        let voice_sum_r_id = app.graph.track_node_ids[0].voice_sum_r_id;
        let buffer_id = crate::instruments::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");

        let summary = app
            .graph_controller()
            .convert_custom_track_to_sampler(0, buffer_id, 48_000, "snare")
            .expect("custom track should convert to a sampler");

        assert_eq!(summary.patterns_reset, 1);
        assert_eq!(app.tracks, vec!["snare".to_string()]);
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Sampler]
        );
        assert_eq!(app.graph.track_engine_ids, vec![None]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, voice_sum_id);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_r_id, voice_sum_r_id);
        assert_eq!(app.graph.track_node_ids[0].sampler_ids.len(), MAX_VOICES);
        assert_eq!(app.graph.track_voice_lids[0].len(), MAX_VOICES);
        assert_eq!(app.graph.track_buffer_ids[0], buffer_id);
        assert_eq!(app.graph.track_sample_rates[0], 48_000);
        assert!(app.graph.engine_node_ids[0].is_none());
        assert_eq!(
            app.state.runtime.voice_counts[0].load(Ordering::Acquire),
            MAX_VOICES as u32
        );
        assert_eq!(
            app.state.export_pattern_repository()[0].sample_ids[0],
            (buffer_id, "snare".to_string(), 48_000)
        );

        let sample_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/ir/lexicon-300-rich-plate.wav");
        app.sampler_paths.push(Some(sample_path.clone()));
        app.register_loaded_sample_path("snare", buffer_id, sample_path.clone());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let project_name = format!(
            "__test-custom-to-sampler-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let captured = app
            .capture_project(&project_name)
            .expect("converted sampler project should capture");
        let project_path = crate::project::save_project(&project_name, &captured)
            .expect("converted sampler project should save");
        let _cleanup = TestProjectFile(project_path);
        let restored = crate::project::load_project(&project_name)
            .expect("converted sampler project should load");
        assert!(matches!(
            restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Sampler { sample_path: restored_path },
                ..
            }] if restored_path == &sample_path.to_string_lossy()
        ));
        graph.process_block();
    }

    #[test]
    fn custom_to_sampler_conversion_keeps_custom_binding_when_voice_build_fails() {
        let graph = TestLiveGraph::new("custom-to-sampler-conversion-rollback-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        let old_sum = app.graph.track_node_ids[0].voice_sum_id;
        let buffer_id = crate::instruments::sampler::create_silent_buffer(graph.ptr.0)
            .expect("silent sampler buffer should be created");
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .convert_custom_track_to_sampler(0, buffer_id, 48_000, "snare")
            .expect_err("injected sampler voice failure should abort conversion");

        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(
            app.graph.track_instrument_types,
            vec![InstrumentType::Custom]
        );
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(app.graph.track_node_ids[0].voice_sum_id, old_sum);
        assert!(app.graph.track_node_ids[0].sampler_ids.is_empty());
        assert_eq!(app.graph.track_buffer_ids, vec![-1]);
        assert_eq!(app.state.runtime.voice_counts[0].load(Ordering::Acquire), 0);
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("old custom engine should remain live");
        assert_eq!(old_engine.route_gain_ids[0].len(), MAX_VOICES);
        let created_nodes = take_test_graph_build_node_ids();
        assert_eq!(created_nodes.len(), 4);
        assert_eq!(
            take_test_graph_build_rollback_node_ids(),
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        graph.process_block();
    }

    #[test]
    fn project_roundtrip_persists_swapped_custom_instrument() {
        let graph = TestLiveGraph::new("custom-track-swap-project-roundtrip-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        for (expected_id, name) in ["old", "new"].into_iter().enumerate() {
            let engine_id = app.editor.engine_registry.upsert(EngineDescriptor {
                name: name.to_string(),
                source: format!("{name}.lisp"),
                manifest: manifest.clone(),
                lib_index: 0,
                shared_runtime: true,
            });
            assert_eq!(engine_id, expected_id);
        }
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        app.editor
            .instrument_libs
            .push(lisp_host::test_loaded_dgen_lib());

        app.apply_recorded_instrument_binding_mutation(0, "Replace instrument", |app| {
            app.graph_controller().swap_custom_track_instrument(
                0, "new", 1, &manifest, &lib, CustomInstrumentRunMode::Instrument,
            )
        })
            .expect("track should swap before saving");
        assert_eq!(app.graph.track_engine_ids, vec![Some(1)]);

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        assert!(matches!(
            crate::app::edit::undo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let undo_project_name = format!(
            "__test-instrument-swap-undo-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let undo_captured = app
            .capture_project(&undo_project_name)
            .expect("undone project should capture");
        let undo_project_path = crate::project::save_project(&undo_project_name, &undo_captured)
            .expect("undone project should save");
        let _undo_cleanup = TestProjectFile(undo_project_path);
        let undo_restored = crate::project::load_project(&undo_project_name)
            .expect("undone project should load");
        assert!(matches!(
            undo_restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Custom { instrument_name },
                ..
            }] if instrument_name == "old"
        ));
        assert!(matches!(
            crate::app::edit::redo(&mut app),
            crate::app::history::HistoryReplay::Applied(_)
        ));
        let project_name = format!(
            "__test-instrument-swap-roundtrip-{}-{nonce}",
            std::process::id()
        );
        let captured = app
            .capture_project(&project_name)
            .expect("swapped project should capture");
        let project_path = crate::project::save_project(&project_name, &captured)
            .expect("swapped project should save");
        let _cleanup = TestProjectFile(project_path);
        let restored =
            crate::project::load_project(&project_name).expect("swapped project should load");

        assert!(matches!(
            restored.tracks.as_slice(),
            [crate::project::ProjectTrack {
                kind: crate::project::ProjectTrackKind::Custom { instrument_name },
                ..
            }] if instrument_name == "new"
        ));
        graph.process_block();
    }

    #[test]
    fn swap_custom_track_leaves_old_binding_intact_when_new_engine_build_fails() {
        let graph = TestLiveGraph::new("custom-track-swap-failure-test");
        let manifest = test_instrument_manifest();
        let lib = lisp_host::test_loaded_dgen_lib();
        let mut app = test_app_with_track_count(&graph, 1);
        install_custom_track_swap_fixture(&mut app, &graph, 1, &manifest, &lib);
        let macro_id = app
            .macro_engine
            .create_macro("failed swap", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                macro_id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::InstrumentParam {
                        param: "old tone".to_string(),
                        param_id: None,
                    },
                    Some(0),
                    0.0,
                    1.0,
                    MacroCurve::Linear,
                )
                .expect("instrument mapping"),
            )
            .expect("instrument mapping should attach");
        let old_slot = EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]);
        set_test_graph_build_failure_after(4);

        let error = app
            .graph_controller()
            .swap_custom_track_instrument(
                0,
                "new",
                1,
                &manifest,
                &lib,
                CustomInstrumentRunMode::Instrument,
            )
            .expect_err("injected engine build failure should abort the swap");
        assert!(error.contains("injected graph node allocation failure"));
        assert_eq!(app.graph.track_engine_ids, vec![Some(0)]);
        assert_eq!(
            app.state.runtime.track_engine_ids[0].load(Ordering::Acquire),
            0
        );
        assert_test_slot_snapshot_eq(
            &EffectSlotSnapshot::capture(&app.state.pattern.instrument_slots[0]),
            &old_slot,
        );
        assert_eq!(
            app.macro_engine
                .macro_definition(macro_id)
                .expect("failed swap must preserve the macro")
                .mappings
                .len(),
            1,
            "failed swap must preserve the old instrument mapping"
        );
        let old_engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("old engine runtime must remain live");
        assert_eq!(old_engine.route_gain_ids[0].len(), MAX_VOICES);
        assert!(app.graph.engine_node_ids[1].is_none());
        graph.process_block();
    }

    #[test]
    fn connect_engine_to_track_rolls_back_every_graph_edit_before_publication() {
        let graph = TestLiveGraph::new("engine-route-rollback-test");
        let mut app = test_app(&graph);
        let targets = install_test_engine(&mut app, &graph);
        set_test_graph_build_failure_after(3);

        let error = {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            connect_test_engine(&mut app, &targets)
                .expect_err("injected route construction failure should be returned")
        };
        assert!(error.contains("injected graph node allocation failure"));

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("test engine should remain registered");
        assert!(engine.route_gain_ids[0].is_empty());
        assert!(engine.ext_route_gain_ids[0].is_empty());
        for voice in 0..MAX_VOICES {
            assert_eq!(
                app.state.runtime.engine_route_lids[0][voice][0].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                app.state.runtime.engine_route_lids_r[0][voice][0].load(Ordering::Acquire),
                0
            );
            for input in 0..EXT_MOD_INPUT_COUNT {
                assert_eq!(
                    app.state.runtime.engine_ext_route_lids[0][voice][0][input]
                        .load(Ordering::Acquire),
                    0
                );
            }
        }

        let created_nodes = take_test_graph_build_node_ids();
        let rolled_back_nodes = take_test_graph_build_rollback_node_ids();
        assert_eq!(created_nodes.len(), 3);
        assert_eq!(
            rolled_back_nodes,
            created_nodes.iter().rev().copied().collect::<Vec<_>>()
        );
        let created_connections = take_test_graph_build_connections();
        let rolled_back_connections = take_test_graph_build_rollback_connections();
        assert!(!created_connections.is_empty());
        assert_eq!(
            rolled_back_connections,
            created_connections
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        );

        for &node_id in &created_nodes {
            assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, node_id) });
        }
        let probe_id = graph.add_gain(0.75, "post_rollback_probe");
        assert!(
            probe_id > *created_nodes.iter().max().unwrap(),
            "compensating rollback must not reuse logical IDs that were already queued"
        );
        assert!(unsafe { crate::audiograph::add_node_to_watchlist(graph.ptr.0, probe_id) });
        let mut observed_probe_gain = None;
        for _ in 0..4 {
            graph.process_block();
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            let copied = unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    probe_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            };
            if copied {
                assert_eq!(state_size, std::mem::size_of_val(&state));
                observed_probe_gain = Some(state[0]);
                break;
            }
        }
        assert_eq!(observed_probe_gain, Some(0.75));
        for node_id in created_nodes {
            let mut state = [0.0_f32; 1];
            let mut state_size = 0;
            let copied = unsafe {
                crate::audiograph::get_node_state_into(
                    graph.ptr.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size,
                )
            };
            assert!(!copied, "rolled-back node {node_id} must not remain live");
            assert_eq!(state_size, 0);
        }
    }

    #[test]
    fn connect_engine_to_track_commits_complete_voice_routes_at_once() {
        let graph = TestLiveGraph::new("engine-route-commit-test");
        let mut app = test_app(&graph);
        let targets = install_test_engine(&mut app, &graph);
        begin_test_graph_build_capture();

        {
            let _batch = GraphEditBatchGuard::new(graph.ptr.0);
            connect_test_engine(&mut app, &targets).expect("complete route build should succeed");
        }

        let engine = app.graph.engine_node_ids[0]
            .as_ref()
            .expect("test engine should remain registered");
        assert_eq!(engine.route_gain_ids[0].len(), MAX_VOICES);
        assert_eq!(engine.ext_route_gain_ids[0].len(), MAX_VOICES);
        for voice in 0..MAX_VOICES {
            let [left_id, right_id] = engine.route_gain_ids[0][voice];
            assert!(left_id > 0);
            assert!(right_id > 0);
            assert_eq!(
                app.state.runtime.engine_route_lids[0][voice][0].load(Ordering::Acquire),
                left_id as u64
            );
            assert_eq!(
                app.state.runtime.engine_route_lids_r[0][voice][0].load(Ordering::Acquire),
                right_id as u64
            );
            for input in 0..EXT_MOD_INPUT_COUNT {
                let ext_id = engine.ext_route_gain_ids[0][voice][input];
                assert!(ext_id > 0);
                assert_eq!(
                    app.state.runtime.engine_ext_route_lids[0][voice][0][input]
                        .load(Ordering::Acquire),
                    ext_id as u64
                );
            }
        }

        assert_eq!(
            take_test_graph_build_node_ids().len(),
            MAX_VOICES * (2 + EXT_MOD_INPUT_COUNT)
        );
        assert!(!take_test_graph_build_connections().is_empty());
        assert!(take_test_graph_build_rollback_node_ids().is_empty());
        assert!(take_test_graph_build_rollback_connections().is_empty());
        graph.process_block();
    }
