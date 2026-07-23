    use super::{
        apply_fit_to_scale_to_trigger, apply_neuron_output_overrides, delayed_step_sample_time,
        enqueue_resolved_trigger, enqueue_step_event_with_midi_fx, invoke_process_cascade,
        midi_fx_window_events_from_step, process_device_write_value, quantized_live_tick_sample,
        reconcile_graph_runtimes, resolve_effect_params, resolve_instrument_plocks,
        resolve_sampler_params, resolved_slot_param_value, run_midi_fx_chain_for_track,
        schedule_playing_lookahead, should_reload_neural_runtime, swung_network_sample_time,
        track_active_note_spans_at_beat, track_note_spans_for_trigger, EmittedNetworkEventSource,
        LiveMidiFxTrackState, MidiFxEvent, MidiFxQuantizerState, SchedulerLookaheadState,
        SnapshotSequencerClock,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{
        EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling, TensorParamDescriptor,
    };
    use crate::graph::{
        EdgeSetSpec, GraphDurationSpec, GraphEdge, GraphEmission, GraphManifest, GraphNode,
        GraphPayload, GraphRuntime, NodeEval, NodeFire, NodeProto, ParamSpec,
        ProjectGraphEdgeParamOverride, ProjectGraphNodeIntrinsicOverride,
        ProjectGraphNodeParamOverride, ProjectGraphOverrides, ProjectGraphRouteOverride,
        ProjectGraphSeedFrom, SeedFrom, ShapeSpec, Topology,
    };
    use crate::lisp_host;
    use crate::neural::{
        NeuralMaxPolySelection, NeuralOutput, ParamNodeId, ProjectEffectParamOverride,
        ProjectNeuralNetwork, ProjectNeuron, ProjectParamOverride,
    };
    use crate::scheduled_event::{
        resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
        ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
        ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledInstrumentTensorParams,
        ScheduledSamplerParams, StepEvent,
    };
    use crate::sequencer::{
        default_empty_effect_chain, SequencerState, StepParam, SwingResolution, Timebase,
        MAX_TRACKS,
    };
    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn test_resolved_step() -> ResolvedStep {
        ResolvedStep {
            duration: 1.0,
            velocity: 1.0,
            speed: 1.0,
            aux_a: 0.0,
            aux_b: 0.0,
            transpose: 0.0,
            pan: 0.0,
            chop: 1.0,
        }
    }

    #[test]
    fn macro_snapshot_is_masked_by_plock_and_process_add_reads_effective_default() {
        let descriptor = ParamDescriptor {
            name: "cutoff".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.2,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: 7,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        };
        let effect = EffectDescriptor {
            name: "filter".to_string(),
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![descriptor.clone()],
        };
        let state = SequencerState::new(
            1,
            vec![vec![crate::effects::EffectSlotState::new(&effect, 11)]],
        );
        let live_slot = &state.pattern.effect_chains[0][0];
        live_slot.set_plock(3, 0, 0.9);

        let param_id = live_slot.param_node_id(0);
        let target = crate::process::ParamTarget::EffectParam {
            slot: 0,
            effect: effect.name.clone(),
            param: descriptor.name.clone(),
            param_id,
        };
        let key = crate::macro_engine::MacroParamKey::for_effect(0, 0, 0, param_id);
        let mut macros = crate::macro_engine::MacroEngine::default();
        let macro_id = macros
            .create_macro("push", crate::macro_engine::MacroKind::Mapped)
            .unwrap();
        macros
            .add_mapping(
                macro_id,
                crate::macro_engine::MacroMapping::new(
                    0,
                    target,
                    0.0,
                    0.8,
                    crate::macro_engine::MacroCurve::Linear,
                )
                .unwrap(),
            )
            .unwrap();
        macros.set_value(macro_id, 1.0);
        assert_eq!(macros.override_value(&key), Some(0.8));

        let snapshot = state.publish_macro_overrides(macros.override_snapshot());
        let slot = &snapshot.tracks[0].effect_slots[0];

        assert_eq!(resolved_slot_param_value(&slot, 3, 0, 0.0), 0.9);
        assert_eq!(resolved_slot_param_value(&slot, 0, 0, 0.0), 0.8);
        assert!(
            (process_device_write_value(
                &descriptor,
                resolved_slot_param_value(&slot, 0, 0, 0.0),
                crate::process::ProcessTargetOp::Add,
                0.1,
            ) - 0.9)
                .abs()
                < 1.0e-6,
            "scheduler Add writes must build on the macro-effective default"
        );

        macros.release(macro_id);
        let snapshot = state.publish_macro_overrides(macros.override_snapshot());
        assert_eq!(
            resolved_slot_param_value(&snapshot.tracks[0].effect_slots[0], 0, 0, 0.0),
            0.2,
            "releasing the macro must restore the persisted default in playback snapshots"
        );
    }

    #[test]
    fn process_write_targets_stable_rack_macro_without_mutating_rack_state() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let mut macros = crate::sequencer::default_rack_macros();
        macros[0].value = 0.25;
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            crate::sequencer::RackTrackSnapshot::new(
                crate::sequencer::RackRouting::Broadcast,
                Vec::new(),
                macros,
            ),
        );
        let snapshot = state.publish_scheduler_snapshot();
        let mut resolved = test_resolved_step();
        let mut overlay = super::ProcessTargetOverlay::default();
        super::process_apply_concrete_target_write(
            &snapshot,
            &[],
            0,
            3,
            &mut resolved,
            &mut overlay,
            &crate::process::ParamTarget::RackMacroParam { macro_id: 0 },
            &crate::process::ProcessTargetWrite {
                port: crate::process::DEFAULT_PROCESS_PORT.to_string(),
                target: None,
                op: crate::process::ProcessTargetOp::Add,
                value: 0.5,
            },
        );
        assert_eq!(overlay.rack_macro_values[0], Some(0.75));
        assert_eq!(
            snapshot.tracks[0].rack_track.as_ref().unwrap().macros[0].value,
            0.25
        );
    }

    fn graph_emission(
        sample_time: u64,
        node_index: usize,
        track: Option<usize>,
        transpose: f32,
        velocity: f32,
    ) -> GraphEmission {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        GraphEmission {
            sample_time,
            node_index,
            event: lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track,
                resolved,
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
        }
    }

    fn generator_emission(
        sample_time: u64,
        generator_index: usize,
        track: Option<usize>,
        transpose: f32,
        velocity: f32,
    ) -> crate::generator::GeneratorEmission {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        crate::generator::GeneratorEmission {
            sample_time,
            generator_index,
            event: lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track,
                resolved,
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
        }
    }

    fn neural_output(
        sample_time: u64,
        track: usize,
        neuron: usize,
        transpose: f32,
        velocity: f32,
    ) -> NeuralOutput {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        NeuralOutput {
            sample_time,
            event: StepEvent {
                track,
                samples_per_step: 12_000.0,
                resolved,
                chord: ScheduledChordData {
                    count: 0,
                    notes: [0.0; crate::audio::MAX_VOICES],
                    durations: [0.0; crate::audio::MAX_VOICES],
                    delays: [0.0; crate::audio::MAX_VOICES],
                    step_transpose: 0.0,
                },
                effect_params: Vec::new(),
                instrument_params: ScheduledInstrumentParams::new(),
                instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                sampler_params: ScheduledSamplerParams::default(),
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                source: EventSource::Network {
                    seed: Some((0, 0)),
                    neuron,
                    instrument_fingerprint: 0,
                },
            },
            emit_trigger: true,
        }
    }

    #[test]
    fn neural_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_neural_output_accents(vec![
            neural_output(1_000, 2, 0, 0.0, 0.5),
            neural_output(1_000, 2, 1, 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
    }

    #[test]
    fn generator_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_generator_emission_accents(vec![
            generator_emission(1_000, 0, Some(2), 0.0, 0.5),
            generator_emission(1_000, 1, Some(2), 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
    }

    #[test]
    fn graph_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_graph_emission_accents(vec![
            graph_emission(1_000, 0, Some(2), 0.0, 0.5),
            graph_emission(1_000, 1, Some(2), 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[0].event.resolved.velocity, 0.5);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
        assert_eq!(merged[1].event.resolved.velocity, 0.25);
    }

    #[test]
    fn graph_accent_merge_sums_only_matching_notes() {
        let merged = super::merge_graph_emission_accents(vec![
            graph_emission(1_000, 0, Some(2), 0.0, 0.5),
            graph_emission(1_000, 1, Some(2), 7.0, 0.25),
            graph_emission(1_000, 2, Some(2), 0.0, 0.75),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[0].event.resolved.velocity, 1.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
        assert_eq!(merged[1].event.resolved.velocity, 0.25);
    }

    fn graph_manifest(id: u64, name: &str, shape: ShapeSpec) -> GraphManifest {
        GraphManifest {
            id,
            name: name.into(),
            shape,
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                route: Some(0),
                seed_from: SeedFrom::Route,
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        }
    }

    fn graph_route_override(
        sequencer_id: u64,
        sequencer_name: &str,
        node_index: usize,
        route: usize,
    ) -> ProjectGraphOverrides {
        ProjectGraphOverrides {
            sequencer_id,
            sequencer_name: sequencer_name.into(),
            node_intrinsics: vec![ProjectGraphNodeIntrinsicOverride {
                group: "n".into(),
                instance: node_index,
                resolution: None,
                delay_steps: None,
                quantize: None,
                route: Some(ProjectGraphRouteOverride::Track(route)),
                seed_from: None,
                seed_on_reset: None,
                duration: None,
                swing: None,
            }],
            node_params: Vec::new(),
            edge_params: Vec::new(),
            reset_every_beats: None,
            max_poly: None,
            max_poly_selection: None,
            node_count: None,
        }
    }

    #[test]
    fn graph_seed_duration_uses_source_step_duration_and_step_size() {
        let mut source = GraphNode::default();
        source.seed_track_mask = crate::graph::seed_track_mask(&[0]);
        let target = GraphNode {
            duration: GraphDurationSpec::Seed,
            ..GraphNode::default()
        };
        let graph = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        let mut graphs = vec![graph];
        let event = StepEvent {
            track: 0,
            samples_per_step: 24_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        super::seed_graph_runtimes(&mut graphs, &event, 0.0, 48_000.0);
        let mut out = Vec::new();
        graphs[0].process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.input > 0.0,
                ..NodeFire::default()
            },
            &mut out,
        );

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.duration, 0.5);
    }

    fn process_graph(
        runtime: &mut crate::graph::GraphRuntime,
        start_beats: f64,
        end_beats: f64,
    ) -> Vec<GraphEmission> {
        let mut out = Vec::new();
        runtime.process_block(
            start_beats,
            end_beats,
            0,
            48_000.0,
            0,
            |eval: &NodeEval| NodeFire {
                fired: eval.energy >= 1.0,
                ..NodeFire::default()
            },
            &mut out,
        );
        out
    }

    #[test]
    fn graph_override_reconcile_preserves_pending_runtime_state() {
        let manifest = graph_manifest(1, "g", ShapeSpec::Line(1));
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![manifest],
            &[graph_route_override(1, "g", 0, 2)],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.track, Some(2));
    }

    #[test]
    fn graph_shape_change_rebuilds_and_clears_pending_state() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        reconcile_graph_runtimes(
            vec![graph_manifest(1, "g", ShapeSpec::Line(1))],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![graph_manifest(1, "g", ShapeSpec::Line(2))],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        assert_eq!(runtimes[0].num_nodes(), 2);
        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert!(out.is_empty());
    }

    #[test]
    fn graph_node_count_override_rebuilds_runtime_and_preserves_overrides() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        let manifest = graph_manifest(
            1,
            "g",
            ShapeSpec::VariableLine {
                default: 8,
                min: 1,
                max: 16,
            },
        );
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        assert_eq!(runtimes[0].num_nodes(), 8);
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        let overrides = vec![ProjectGraphOverrides {
            sequencer_id: 1,
            sequencer_name: "g".into(),
            node_count: Some(12),
            node_params: vec![ProjectGraphNodeParamOverride {
                group: "n".into(),
                instance: 14,
                param: "threshold".into(),
                value: 0.25,
            }],
            edge_params: vec![ProjectGraphEdgeParamOverride {
                group: "n->n".into(),
                from: 14,
                to: 3,
                param: "weight".into(),
                value: 0.5,
            }],
            ..ProjectGraphOverrides::default()
        }];
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &overrides,
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        assert_eq!(runtimes[0].num_nodes(), 12);
        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert!(
            out.is_empty(),
            "node-count change must clear pending seed state"
        );
        assert_eq!(overrides[0].node_count, Some(12));
        assert_eq!(overrides[0].node_params[0].instance, 14);
        assert_eq!(overrides[0].edge_params[0].from, 14);

        let shrunk = manifest.runtime_config_with_overrides(Some(&overrides[0]));
        assert_eq!(
            shrunk.nodes.len(),
            12,
            "storage remains dormant until node_count grows"
        );
        let mut restored_overrides = overrides[0].clone();
        restored_overrides.node_count = Some(16);
        let restored = manifest.runtime_config_with_overrides(Some(&restored_overrides));
        assert_eq!(restored.nodes.len(), 16);
        assert_eq!(restored.node_params[14]["threshold"], 0.25);
        assert_eq!(
            restored
                .edges
                .iter()
                .find(|edge| edge.from == 14 && edge.to == 3)
                .expect("restored dormant edge")
                .weight,
            0.5
        );
    }

    #[test]
    fn graph_reconcile_tracks_multiple_graphs_by_id() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        let graph_a = graph_manifest(1, "a", ShapeSpec::Line(1));
        let graph_b = graph_manifest(2, "b", ShapeSpec::Line(1));
        reconcile_graph_runtimes(
            vec![graph_a.clone(), graph_b.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());
        runtimes[1].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![graph_a, graph_b],
            &[graph_route_override(1, "a", 0, 3)],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        let out_a = process_graph(&mut runtimes[0], 0.0, 1.0);
        let out_b = process_graph(&mut runtimes[1], 0.0, 1.0);
        assert_eq!(out_a.len(), 1);
        assert_eq!(out_b.len(), 1);
        assert_eq!(out_a[0].event.track, Some(3));
        assert_eq!(out_b[0].event.track, Some(0));
    }

    #[test]
    fn graph_reset_boundary_preserves_seed_from_snapshot_clock_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let snapshot = state.latest_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;

        let mut seed_node = crate::graph::GraphNode {
            resolution: Timebase::Sixteenth,
            seed_track_mask: crate::graph::seed_track_mask(&[0]),
            ..crate::graph::GraphNode::default()
        };
        seed_node.route = Some(0);
        let routed_node = crate::graph::GraphNode {
            resolution: Timebase::Sixteenth,
            route: Some(1),
            ..crate::graph::GraphNode::default()
        };
        let mut runtime = crate::graph::GraphRuntime::new(
            1,
            "g".into(),
            vec![seed_node, routed_node],
            vec![crate::graph::GraphEdge::new(0, 1, 1.0)],
            1.0,
            4.0,
        );

        let mut scheduled_until_sample = 0_u64;
        let mut emitted = Vec::new();
        while clock.total_beats < 4.5 {
            let chunk_start_beats = clock.total_beats;
            let triggers = clock.process_chunk(512, &snapshot, &state);
            let chunk_end_beats = clock.total_beats;
            for trigger in triggers {
                if trigger.track == 0 && trigger.step == 0 {
                    let seed_beats = trigger.absolute_beats;
                    runtime.seed(
                        trigger.track,
                        seed_beats,
                        crate::graph::GraphPayload::default(),
                    );
                }
            }
            runtime.process_block(
                chunk_start_beats,
                chunk_end_beats,
                scheduled_until_sample,
                samples_per_quarter,
                0,
                |eval: &NodeEval| NodeFire {
                    fired: eval.energy >= 1.0,
                    ..NodeFire::default()
                },
                &mut emitted,
            );
            scheduled_until_sample = scheduled_until_sample.saturating_add(512);
        }

        assert!(
            emitted
                .iter()
                .any(|emission| emission.event.track == Some(1) && emission.sample_time > 96_000),
            "bar-start seed should survive the one-bar reset and re-drive the graph: {emitted:?}"
        );
    }

    #[test]
    fn snapshot_clock_emits_triggers_for_active_steps() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let snapshot = state.latest_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);

        let triggers = clock.process_chunk(12_000, &snapshot, &state);
        assert!(!triggers.is_empty());
        assert_eq!(triggers[0].track, 0);
        assert_eq!(triggers[0].step, 0);
    }

    #[test]
    fn snapshot_clock_suppresses_triggers_for_scene_silenced_tracks() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let mut snapshot = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut snapshot.tracks[0]).scene_silenced = true;
        let mut clock = SnapshotSequencerClock::new(48_000);

        let triggers = clock.process_chunk(12_000, &snapshot, &state);

        assert!(triggers.is_empty());
    }

    #[test]
    fn delayed_step_sample_time_offsets_by_fraction_of_step() {
        let mut params = [0.0; crate::sequencer::NUM_PARAMS];
        params[StepParam::Delay.index()] = 0.5;

        assert_eq!(delayed_step_sample_time(1_000, &params, 6_000.0), 4_000);
    }

    #[test]
    fn enqueue_resolved_trigger_splits_note_delays() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let resolved = ResolvedStep {
            duration: 1.0,
            velocity: 1.0,
            speed: 1.0,
            aux_a: 0.0,
            aux_b: 0.0,
            transpose: 0.0,
            pan: 0.0,
            chop: 1.0,
        };
        let mut chord = ScheduledChordData {
            count: 2,
            notes: [0.0; crate::audio::MAX_VOICES],
            durations: [1.0; crate::audio::MAX_VOICES],
            delays: [0.0; crate::audio::MAX_VOICES],
            step_transpose: 0.0,
        };
        chord.notes[1] = 7.0;
        chord.delays[1] = 0.5;
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0,
            0,
            6_000.0,
            resolved,
            chord,
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
            [None; crate::sequencer::RACK_MACRO_COUNT],
        ));

        let first = queue.pop().expect("first note event");
        let second = queue.pop().expect("second note event");
        assert_eq!(first.sample_time, 1_000);
        assert_eq!(second.sample_time, 4_000);
        assert_eq!(track_output_events.len(), 2);
        assert_eq!(track_output_events[0].beat, 0.0);
        assert_eq!(track_output_events[1].beat, 0.0625);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn enqueue_resolved_trigger_applies_global_transpose_for_opted_in_track() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            5.0,
            0,
            0,
            6_000.0,
            test_resolved_step(),
            ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
            [None; crate::sequencer::RACK_MACRO_COUNT],
        ));

        let event = queue.pop().expect("global-transposed event");
        match event.kind {
            ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                assert_eq!(resolved.transpose, 5.0);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        assert_eq!(track_output_events[0].transpose, 5.0);
    }

    #[test]
    fn enqueue_resolved_trigger_respects_global_transpose_opt_out() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_global_transpose(false);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            5.0,
            0,
            0,
            6_000.0,
            test_resolved_step(),
            ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
            [None; crate::sequencer::RACK_MACRO_COUNT],
        ));

        let event = queue.pop().expect("opted-out event");
        match event.kind {
            ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                assert_eq!(resolved.transpose, 0.0);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        assert_eq!(track_output_events[0].transpose, 0.0);
    }

    #[test]
    fn network_trigger_uses_target_track_swing() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_swing(75.0);
        state.pattern.track_params[0].set_swing_resolution(SwingResolution::Sixteenth);
        let snapshot = state.publish_scheduler_snapshot();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        assert_eq!(
            swung_network_sample_time(&snapshot, &event, 12_000, 0.25, 48_000.0),
            18_000
        );
        assert_eq!(
            swung_network_sample_time(&snapshot, &event, 24_000, 0.5, 48_000.0),
            24_000
        );
    }

    #[test]
    fn network_trigger_enqueue_runs_target_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["octave".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "octave"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :transpose 12)))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
            false,
        ));
        let scheduled = queue.pop().expect("MIDI FX output event");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                resolved,
                source_neuron,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(source_neuron, 0);
                assert_eq!(resolved.transpose, 12.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert_eq!(track_output_events.len(), 1);
        assert_eq!(track_output_events[0].track, 0);
        assert_eq!(track_output_events[0].transpose, 12.0);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn emitted_network_event_runs_midi_fx_and_keeps_target_instrument_defaults() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["octave".to_string()]);
        state.pattern.instrument_slots[0]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[0].defaults.set(12, 2.5);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "octave"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :transpose 12)))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Generator { index: 0 },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(0),
                resolved: test_resolved_step(),
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let scheduled = queue.pop().expect("MIDI FX output event");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                resolved,
                instrument_params,
                sampler_params,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(resolved.transpose, 12.0);
                assert_eq!(sampler_params.playback_speed, 2.5);
                assert!(instrument_params.iter().any(|param| {
                    param.target == ScheduledInstrumentParamTarget::Synth
                        && param.idx == crate::instruments::sampler::PARAM_SPEED
                        && param.value == 2.5
                }));
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
        assert_eq!(track_output_events.len(), 1);
        assert_eq!(track_output_events[0].transpose, 12.0);
    }

    #[test]
    fn emitted_network_event_trigger_to_track_copies_to_selected_target_track() {
        let state = Arc::new(SequencerState::new(
            5,
            vec![
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
            ],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 5.0);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Generator { index: 0 },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(0),
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let mut events = Vec::new();
        while let Some(event) = queue.pop() {
            events.push(event);
        }
        assert_eq!(
            events.len(),
            2,
            "expected source plus copied target trigger"
        );
        let mut tracks_and_transposes = events
            .into_iter()
            .map(|scheduled| {
                assert_eq!(scheduled.sample_time, 1_000);
                match scheduled.kind {
                    ScheduledEventKind::NetworkTrigger {
                        track, resolved, ..
                    } => (track, resolved.transpose),
                    other => panic!("expected network trigger, got {other:?}"),
                }
            })
            .collect::<Vec<_>>();
        tracks_and_transposes.sort_by_key(|(track, _)| *track);
        assert_eq!(tracks_and_transposes, vec![(0, 7.0), (4, 7.0)]);
        let mut telemetry_tracks = track_output_events
            .iter()
            .map(|event| event.track)
            .collect::<Vec<_>>();
        telemetry_tracks.sort_unstable();
        assert_eq!(telemetry_tracks, vec![0, 4]);
    }

    #[test]
    fn quantizer_midi_fx_holds_until_next_grid_and_keeps_highest_velocity() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        assert_eq!(
            quantizer_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            Some("quantize-grid")
        );
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();

        let event = |beat: f32, velocity: f32, transpose: f32| MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose,
                pan: 0.0,
                chop: 1.0,
            },
            chord: vec![transpose],
            chord_durations: vec![1.0],
            chord_delays: vec![0.0],
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: beat,
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: resolve_sampler_params(&snapshot, 0, 0),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        let mut quantizer_state = MidiFxQuantizerState::default();
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.10, 0.3, 1.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.25, 0.8, 7.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.40, 0.6, 12.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());

        let due = quantizer_state.drain_due(1.0);
        assert_eq!(due.len(), 1);
        assert!((due[0].deadline_beats - 1.0).abs() < 1e-9);
        assert_eq!(due[0].event.resolved.transpose, 7.0);
        assert!((due[0].event.resolved.velocity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn scheduler_lookahead_quantizer_keeps_first_on_grid_trigger() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);

        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Velocity, 0.7);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 5.0);

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(runtime);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            6_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let scheduled = queue.pop().expect("on-grid quantized event");
        let ScheduledEventKind::ResolvedTrigger {
            track, resolved, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert_eq!(scheduled.sample_time, 0);
        assert_eq!(track, 0);
        assert_eq!(resolved.transpose, 5.0);
        assert!((resolved.velocity - 0.7).abs() < 1e-6);
    }

    #[test]
    fn scheduler_lookahead_flushes_quantizer_without_trigger_on_grid() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);

        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Velocity, 0.9);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 7.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.4);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 12.0);

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(runtime);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            36_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let scheduled = queue.pop().expect("quantized event");
        let ScheduledEventKind::ResolvedTrigger {
            track, resolved, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert_eq!(scheduled.sample_time, 24_000);
        assert_eq!(track, 0);
        assert_eq!(resolved.transpose, 7.0);
        assert!((resolved.velocity - 0.9).abs() < 1e-6);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn graph_runtime_emission_runs_target_track_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(
            5,
            vec![
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
            ],
        ));
        state.pattern.track_params[1].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[1][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[1][0].defaults.set(0, 5.0);
        let snapshot = state.publish_scheduler_snapshot();

        let mut n0 = GraphNode::default();
        n0.seed_track_mask = 1 << 0;
        n0.route = Some(1);
        let mut graph_runtime = GraphRuntime::new(
            1,
            "g".to_string(),
            vec![n0],
            vec![GraphEdge::new(0, 0, 1.0)],
            1.0,
            0.0,
        );
        graph_runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 7.0,
                velocity: 0.9,
                duration_beats: 0.25,
            },
        );
        let mut graph_emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            1.0,
            1_000,
            48_000.0,
            0,
            |_eval| NodeFire {
                fired: true,
                ..NodeFire::default()
            },
            &mut graph_emissions,
        );
        assert!(!graph_emissions.is_empty());
        assert_eq!(graph_emissions[0].event.track, Some(1));

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            graph_emissions[0].sample_time,
            48_000.0,
            1.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: graph_emissions[0].node_index,
            },
            graph_emissions.remove(0).event,
            false,
        ));

        let mut tracks_and_transposes = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::NetworkTrigger {
                    track, resolved, ..
                } => tracks_and_transposes.push((track, resolved.transpose)),
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        tracks_and_transposes.sort_by_key(|(track, _)| *track);
        assert_eq!(tracks_and_transposes, vec![(1, 7.0), (4, 7.0)]);
        let mut telemetry_tracks = track_output_events
            .iter()
            .map(|event| event.track)
            .collect::<Vec<_>>();
        telemetry_tracks.sort_unstable();
        assert_eq!(telemetry_tracks, vec![1, 4]);
    }

    #[test]
    fn graph_route_off_emission_does_not_fall_back_to_track_zero() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: 0,
            },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: None,
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        assert!(
            queue.pop().is_none(),
            "graph route Off must not enqueue a source-track event or run source-track MIDI FX"
        );
        assert!(track_output_events.is_empty());
    }

    #[test]
    fn graph_runtime_emission_runs_arp_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[1].set_midi_fx_chain(vec!["arp".to_string()]);
        let arp_desc = lisp_host::load_midi_fx_descriptor("arp").expect("arp descriptor");
        state.pattern.midi_fx_slots[1][0].apply_descriptor(&arp_desc, 0);
        state.pattern.midi_fx_slots[1][0].defaults.set(0, 4.0);
        let snapshot = state.publish_scheduler_snapshot();

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<16>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: 0,
            },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(1),
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let mut scheduled = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::NetworkTrigger {
                    track, resolved, ..
                } => scheduled.push((event.sample_time, track, resolved.transpose)),
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        assert_eq!(scheduled.len(), 4);
        assert!(scheduled
            .iter()
            .all(|(_, track, note)| *track == 1 && *note == 7.0));
        assert_eq!(
            scheduled
                .iter()
                .map(|(sample_time, _, _)| *sample_time)
                .collect::<Vec<_>>(),
            vec![1_000, 13_000, 25_000, 37_000]
        );
    }

    fn publish_test_graph_sequencer(state: Arc<SequencerState>, source: &str) {
        let mut authoring = Runtime::new();
        let publish_state = Arc::clone(&state);
        authoring.register_native("def-sequencer", move |args, _ctx| {
            let published = lisp_host::published_sequencer_from_def_args(&args)?;
            let name = published.name.clone();
            publish_state.publish_sequencer(published);
            Ok(Value::String(name))
        });
        authoring
            .eval_str(source)
            .expect("evaluate test graph sequencer");
    }

    #[test]
    fn scheduler_runtime_keeps_builtin_midi_fx_when_project_scratch_fails() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = super::build_scheduler_scratch_runtime(
            Arc::clone(&state),
            r#"(def-sequencer "graph-scratch" :shape (line 1))"#,
            false,
        )
        .expect("builtin MIDI FX should keep scheduler runtime alive");
        let names = runtime.midi_fx_names();
        assert!(
            names.iter().any(|name| name == "arp"),
            "scheduler runtime should keep builtin arp after scratch eval failure: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "trigger-to-track"),
            "scheduler runtime should keep builtin trigger-to-track after scratch eval failure: {names:?}"
        );
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ScheduledTriggerKind {
        Step,
        Network,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct ObservedTrigger {
        kind: ScheduledTriggerKind,
        track: usize,
        sample_time: u64,
        transpose: f32,
        duration: f32,
        sampler_speed: Option<f32>,
        has_speed_param: bool,
    }

    fn observed_triggers<const QUEUE_CAP: usize>(
        queue: &ScheduledEventQueue<QUEUE_CAP>,
    ) -> Vec<ObservedTrigger> {
        let mut out = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::ResolvedTrigger {
                    track,
                    resolved,
                    instrument_params,
                    ..
                } => out.push(ObservedTrigger {
                    kind: ScheduledTriggerKind::Step,
                    track,
                    sample_time: event.sample_time,
                    transpose: resolved.transpose,
                    duration: resolved.duration,
                    sampler_speed: None,
                    has_speed_param: instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::instruments::sampler::PARAM_SPEED
                            && param.value == 2.5
                    }),
                }),
                ScheduledEventKind::NetworkTrigger {
                    track,
                    resolved,
                    instrument_params,
                    sampler_params,
                    ..
                } => out.push(ObservedTrigger {
                    kind: ScheduledTriggerKind::Network,
                    track,
                    sample_time: event.sample_time,
                    transpose: resolved.transpose,
                    duration: resolved.duration,
                    sampler_speed: Some(sampler_params.playback_speed),
                    has_speed_param: instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::instruments::sampler::PARAM_SPEED
                            && param.value == 2.5
                    }),
                }),
                ScheduledEventKind::EffectParams { .. }
                | ScheduledEventKind::InstrumentParams { .. } => {}
            }
        }
        out.sort_by_key(|event| {
            (
                event.sample_time,
                match event.kind {
                    ScheduledTriggerKind::Step => 0_u8,
                    ScheduledTriggerKind::Network => 1_u8,
                },
                event.track,
            )
        });
        out
    }

    fn run_sparse_process_accumulator_fixture() -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        run_sparse_process_accumulator_fixture_impl(false)
    }

    fn run_sparse_process_accumulator_fixture_via_lisp_attach(
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        run_sparse_process_accumulator_fixture_impl(true)
    }

    fn run_sparse_process_accumulator_fixture_impl(
        attach_via_lisp: bool,
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(8);
        for step in 0..8 {
            state.pattern.patterns[0].set_step_active(step, true);
        }

        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-accumulator sparse-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-128 128)
                  :mode :clip)
                "#,
            )
            .expect("define process accumulator");

        if attach_via_lisp {
            scratch
                .eval(
                    r#"
                    (processes :track 0
                      (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0)))
                    "#,
                )
                .expect("attach process chain via lisp");
            let chain = state.track_process_chain(0).expect("track 0 process chain");
            assert_eq!(chain.slots.len(), 1);
            assert_eq!(chain.slots[0].class_name, "sparse-transpose");
        } else {
            assert!(state.set_track_process_chain(
                0,
                crate::process::TrackProcessChain {
                    slots: vec![crate::process::TrackProcessSlot {
                        instance_id: crate::process::ProcessInstanceId(1),
                        instance_name: None,
                        class_name: "sparse-transpose".to_string(),
                        enabled: true,
                        project_layer: false,
                        inlets: std::collections::BTreeMap::new(),
                        lanes: std::collections::BTreeMap::from([(
                            "amount".to_string(),
                            crate::process::ProcessLane {
                                values: vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                            },
                        )]),
                        bindings: std::collections::BTreeMap::new(),
                    }],
                },
            ));
        }

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<32>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            102_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        (state, observed_triggers(&queue))
    }

    fn run_default_inert_process_accumulator_fixture(
        attach_default_process: bool,
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(8);
        let base_transposes = [0.0, 7.0, -3.0, 12.0, 0.0, 5.0, -5.0, 2.0];
        let base_durations = [1.0, 0.5, 2.0, 1.5, 0.75, 1.25, 0.5, 2.0];
        for step in 0..8 {
            state.pattern.patterns[0].set_step_active(step, true);
            state.set_step_param(0, step, StepParam::Transpose, base_transposes[step]);
            state.set_step_param(0, step, StepParam::Duration, base_durations[step]);
        }

        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-accumulator default-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-128 128)
                  :mode :clip)
                "#,
            )
            .expect("define default-inert process accumulator");
        if attach_default_process {
            scratch
                .eval("(processes :track 0 (default-transpose))")
                .expect("attach default process accumulator");
            let chain = state.track_process_chain(0).expect("track 0 process chain");
            assert_eq!(chain.slots.len(), 1);
            assert_eq!(chain.slots[0].class_name, "default-transpose");
            assert!(
                chain.slots[0].lanes.is_empty(),
                "default attachment should not persist any lane overrides"
            );
        }

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<32>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            102_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        (state, observed_triggers(&queue))
    }

    fn run_with_scheduler_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .name("scheduler-process-accumulator-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(f)
            .expect("spawn scheduler process accumulator harness")
            .join()
            .expect("scheduler process accumulator harness panicked")
    }

    fn project_performance_cascade_fixture(
        cache_enabled: bool,
    ) -> (
        Arc<SequencerState>,
        Option<lisp_host::ScratchControlRuntime>,
        crate::process::ProcessRuntime,
    ) {
        const TRACKS: usize = 4;
        let state = Arc::new(SequencerState::new(
            TRACKS,
            (0..TRACKS).map(|_| default_empty_effect_chain()).collect(),
        ));
        for track in 0..TRACKS {
            state.pattern.track_params[track].set_num_steps(16);
            for step in 0..16 {
                state.pattern.patterns[track].set_step_active(step, true);
            }
        }

        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            (0..TRACKS).map(|_| Vec::new()).collect(),
            (0..TRACKS)
                .map(|_| EffectDescriptor::builtin_sampler())
                .collect(),
            0,
            0,
        );
        let script_path = format!(
            "{}/scripts/processes/process-project-performance-lanes-demo.lisp",
            env!("CARGO_MANIFEST_DIR")
        );
        let source = std::fs::read_to_string(&script_path)
            .expect("read project process performance lanes demo");
        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate project process performance lanes demo");
        scratch.set_process_run_cache_enabled(cache_enabled);

        let mut process_runtime = crate::process::ProcessRuntime::default();
        process_runtime.sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        (state, Some(scratch), process_runtime)
    }

    fn run_project_performance_cascade_pass(
        state: &SequencerState,
        scratch: &mut Option<lisp_host::ScratchControlRuntime>,
        process_runtime: &mut crate::process::ProcessRuntime,
        cycle: u64,
    ) -> usize {
        let mut invocation_count = 0;
        for step in 0..16 {
            for track in 0..4 {
                let chain = state
                    .composed_track_process_chain(track)
                    .expect("composed project process chain");
                for slot in &chain.slots {
                    let Some(invocation) = process_runtime.step_process_invocation(
                        slot,
                        crate::process::ProcessStepRunContext {
                            track,
                            step,
                            cycle,
                            beat: cycle as f64 * 4.0 + step as f64 * 0.25,
                            sample_time: cycle * 96_000 + step as u64 * 6_000,
                            step_beats: 0.25,
                            resolved: test_resolved_step(),
                            event: Value::Nil,
                        },
                    ) else {
                        continue;
                    };
                    invocation_count += 1;
                    assert!(invoke_process_cascade(
                        scratch,
                        process_runtime,
                        invocation,
                        false,
                        |_, _, _, _| {},
                    ));
                }
            }
        }
        invocation_count
    }

    fn profile_project_performance_cascade(cache_enabled: bool) -> (Duration, usize) {
        let (state, mut scratch, mut process_runtime) =
            project_performance_cascade_fixture(cache_enabled);
        let warmup_invocations =
            run_project_performance_cascade_pass(&state, &mut scratch, &mut process_runtime, 0);
        let started = Instant::now();
        let measured_invocations =
            run_project_performance_cascade_pass(&state, &mut scratch, &mut process_runtime, 1);
        assert_eq!(warmup_invocations, measured_invocations);
        (started.elapsed(), measured_invocations)
    }

    #[test]
    #[ignore = "manual release-mode performance profile"]
    fn profile_invoke_process_cascade_project_performance_lanes() {
        let (uncached, uncached_invocations) = profile_project_performance_cascade(false);
        let (cached, cached_invocations) = profile_project_performance_cascade(true);
        assert_eq!(uncached_invocations, 128);
        assert_eq!(cached_invocations, uncached_invocations);
        let speedup = uncached.as_secs_f64() / cached.as_secs_f64();
        eprintln!(
            "invoke_process_cascade profile: invocations={} uncached={:?} cached={:?} speedup={:.2}x",
            cached_invocations, uncached, cached, speedup
        );
        assert!(
            speedup >= 10.0,
            "expected at least 10x invoke_process_cascade speedup, measured {speedup:.2}x"
        );
    }

    fn schedule_process_fixture(
        state: &Arc<SequencerState>,
        scratch: lisp_host::ScratchControlRuntime,
    ) -> Vec<ScheduledEventKind> {
        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<64>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            24_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let mut events = Vec::new();
        while let Some(event) = queue.pop() {
            events.push(event.kind);
        }
        events
    }

    fn schedule_process_observed_fixture(
        state: &Arc<SequencerState>,
        scratch: lisp_host::ScratchControlRuntime,
        lookahead_target_samples: u64,
    ) -> Vec<ObservedTrigger> {
        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<64>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            lookahead_target_samples,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        observed_triggers(&queue)
    }

    fn first_resolved_trigger(events: &[ScheduledEventKind]) -> &ScheduledEventKind {
        events
            .iter()
            .find(|event| matches!(event, ScheduledEventKind::ResolvedTrigger { .. }))
            .expect("resolved trigger event")
    }

    #[test]
    fn scheduler_process_accumulator_folds_sparse_lane_into_transpose() {
        let (_state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let transposes = events
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(transposes, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn scheduler_project_layer_runs_on_every_track_with_independent_state() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
                for step in 0..8 {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process count-up
                      :target (step-param :transpose)
                      :state ((acc 0))
                      :run (do
                        (set! acc (+ acc 1))
                        (target-add! acc)))

                    (def-accumulator sparse-transpose
                      :target (step-param :transpose)
                      :amount (amount :lane true :default 0)
                      :range (-128 128)
                      :mode :clip)

                    (processes :project (count-up))
                    (processes :track 1
                      (sparse-transpose :amount (lane 10 0 0 0 0 0 0 0)))
                    "#,
                )
                .expect("attach project layer and track chain");

            let project_chain = state.project_process_chain();
            assert_eq!(project_chain.slots.len(), 1);
            assert!(project_chain.slots[0].project_layer);

            schedule_process_observed_fixture(&state, scratch, 102_000)
        });

        let track_transposes = |track: usize| {
            events
                .iter()
                .filter(|event| event.track == track)
                .take(8)
                .map(|event| event.transpose)
                .collect::<Vec<_>>()
        };
        // The project counter runs on both tracks with independent state:
        // shared configuration, per-(instance, track) runtime state. A shared
        // state cell would interleave to 1..16 across the two tracks.
        assert_eq!(
            track_transposes(0),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            "track 0 runs its own copy of the project counter"
        );
        // Track 1 composes its own chain after the project layer: the sparse
        // accumulator holds +10 from step 0 onward on top of the counter.
        assert_eq!(
            track_transposes(1),
            vec![11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0],
            "track 1 = project counter + its own accumulator"
        );
    }

    #[test]
    fn scheduler_resolved_track_read_uses_previous_tick_not_trigger_visit_order() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(4);
                for step in 0..4 {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }
            for (step, transpose) in [2.0, 4.0, 6.0, 8.0].into_iter().enumerate() {
                state.set_step_param(0, step, StepParam::Transpose, transpose);
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process follow-previous-source
                      :target (step-param :transpose)
                      :run (target-add! (read (track 0 :transpose))))
                    (processes :track 1 (follow-previous-source))
                    "#,
                )
                .expect("define previous-tick track reader");

            schedule_process_observed_fixture(&state, scratch, 54_000)
        });

        let track_transposes = |track: usize| {
            events
                .iter()
                .filter(|event| event.track == track)
                .take(4)
                .map(|event| event.transpose)
                .collect::<Vec<_>>()
        };
        assert_eq!(track_transposes(0), vec![2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            track_transposes(1),
            vec![0.0, 2.0, 4.0, 6.0],
            "track 1 must not observe track 0's same-boundary value even though track 0 sorts first"
        );
    }

    #[test]
    fn scheduler_resolved_track_read_repeats_across_pattern_cycles() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.set_step_param(0, 0, StepParam::Transpose, 7.0);
            state.pattern.patterns[1].set_step_active(4, true);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process repeat-current-source
                      :target (step-param :transpose)
                      :run (target-add! (read (track 0 :transpose))))
                    (processes :track 1 (repeat-current-source))
                    "#,
                )
                .expect("define repeating current-value reader");

            schedule_process_observed_fixture(&state, scratch, 108_000)
        });

        let reader = events
            .iter()
            .filter(|event| event.track == 1)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(reader, vec![7.0, 7.0], "reader must repeat every cycle");
    }

    #[test]
    fn phase7_demo_trigger_history_reader_repeats_across_pattern_cycles() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.set_step_param(0, 0, StepParam::Transpose, 7.0);
            // UI step #3 is zero-based scheduler step 2, where the demo's
            // `:trigs-ago 1` amount lane is active.
            state.pattern.patterns[1].set_step_active(2, true);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            let script_path = format!(
                "{}/scripts/processes/process-phase7-reads-demo.lisp",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&script_path).expect("read Phase 7 reads demo");
            scratch
                .eval_source_at_path(script_path, &source)
                .expect("evaluate Phase 7 reads demo");

            schedule_process_observed_fixture(&state, scratch, 108_000)
        });

        let reader = events
            .iter()
            .filter(|event| event.track == 1)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(
            reader,
            vec![7.0, 7.0],
            "the demo's UI step #3 trigger-history reader must repeat every cycle"
        );
    }

    #[test]
    fn scheduler_fields_are_previous_tick_typed_and_independently_interpreted() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                4,
                (0..4).map(|_| default_empty_effect_chain()).collect(),
            ));
            for track in 0..4 {
                state.pattern.track_params[track].set_num_steps(2);
                for step in 0..2 {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }
            for track in 1..4 {
                for step in 0..2 {
                    state.set_step_param(track, step, StepParam::Transpose, 2.0);
                }
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(); 4],
                vec![EffectDescriptor::builtin_sampler(); 4],
                0,
                0,
            );
            scratch
                .eval(&lisp_host::load_process_library_source())
                .expect("load builtin process library");
            scratch
                .eval(
                    r#"
                    (def-process harmony-publisher
                      :run (suggest :harmony
                             (pitch-field (list 0 4 7) :root 0 :weight 1)))
                    (processes :track 0 (harmony-publisher))
                    (processes :track 1
                      (follow-harmony :listen :harmony :amount 1 :grace 0))
                    (processes :track 2
                      (follow-harmony :listen :harmony :amount 0.5 :grace 0))
                    (processes :track 3
                      (follow-harmony :listen :missing :amount 1 :grace 0))
                    "#,
                )
                .expect("attach typed field publisher and listeners");

            schedule_process_observed_fixture(&state, scratch, 18_000)
        });

        let track_transposes = |track: usize| {
            events
                .iter()
                .filter(|event| event.track == track)
                .take(2)
                .map(|event| event.transpose)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            track_transposes(1),
            vec![2.0, 0.0],
            "same-tick publication is hidden; next tick follows the pitch field fully"
        );
        assert_eq!(
            track_transposes(2),
            vec![2.0, 1.0],
            "each listener applies its own obedience amount"
        );
        assert_eq!(
            track_transposes(3),
            vec![2.0, 2.0],
            "a missing publisher must remain inert"
        );
    }

    #[test]
    fn scheduler_response_phrase_uses_note_count_delay_and_timebase_lanes() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                3,
                (0..3).map(|_| default_empty_effect_chain()).collect(),
            ));
            for track in 0..3 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(); 3],
                vec![EffectDescriptor::builtin_sampler(); 3],
                0,
                0,
            );
            scratch
                .eval(&lisp_host::load_process_library_source())
                .expect("load builtin process library");
            scratch
                .eval(
                    r#"
                    (def-process response-test-publisher
                      :run (suggest :harmony
                             (pitch-field (list 0 4 7) :root 0 :weight 1)))
                    (def-process response-test-player
                      :in ((listen :field :default :harmony)
                           (target :track :default 2)
                           (num-notes :int 0 8 :default 0 :lane true)
                           (play-delay :int 0 16 :default 1 :lane true)
                           (timebase :int 0 12 :default 4 :lane true))
                      :run (let ((field (hear (in :listen))))
                             (if field
                               (let ((pitches (field-pitches field))
                                     (spacing (* (in :play-delay)
                                                 (timebase-beats (in :timebase)))))
                                 (map (lambda (i)
                                        (emit :track (in :target)
                                              :after (* (+ i 1) spacing)
                                              :note (nth pitches (mod i (len pitches)))
                                              :vel 0.75
                                              :duration 0.5))
                                      (range 0 (in :num-notes))))
                               nil)))
                    (processes :track 0
                      (response-test-publisher)
                      (response-test-player
                        :listen :harmony
                        :target 2
                        :num-notes (lane 2 2)
                        :play-delay (lane 2 2)
                        :timebase (lane 4 4)))
                    "#,
                )
                .expect("attach lane-driven response phrase");

            schedule_process_observed_fixture(&state, scratch, 42_000)
        });

        let responses = events
            .iter()
            .filter(|event| event.track == 2)
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>(),
            // The harness's process-emission chunk begins at sample 1. At
            // 120 BPM / 48 kHz, the second call is beat 0.25, then +0.5 and
            // +1.0 beats land at samples 18_001 and 30_001 respectively.
            vec![18_001, 30_001],
            "delay=2 at a sixteenth-note timebase must place replies at +1/8 and +1/4"
        );
        assert_eq!(
            responses
                .iter()
                .map(|event| event.transpose)
                .collect::<Vec<_>>(),
            vec![0.0, 4.0]
        );
    }

    #[test]
    fn scheduler_conductor_runs_once_after_coincident_observed_tracks_resolve() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                3,
                (0..3).map(|_| default_empty_effect_chain()).collect(),
            ));
            for track in 0..3 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[1].set_step_active(0, true);
            state.set_step_param(0, 0, StepParam::Transpose, 3.0);
            state.set_step_param(1, 0, StepParam::Transpose, 7.0);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(); 3],
                vec![EffectDescriptor::builtin_sampler(); 3],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process sum-conductor
                      :run (emit
                             :track (nth (play-tracks) 0)
                             :note (+ (read (track 0 :transpose))
                                      (read (track 1 :transpose)))
                             :vel 0.8
                             :duration 0.5))
                    (def sum-conductor-h
                      (processes :observe (list 0 1) :play (list 2)
                        (sum-conductor)))
                    "#,
                )
                .expect("attach multi-track conductor");

            let authored = scratch.process_authoring_snapshot();
            assert_eq!(authored.conductors.len(), 1);
            assert_eq!(authored.conductors[0].observe_tracks, vec![0, 1]);
            assert_eq!(authored.conductors[0].play_tracks, vec![2]);

            schedule_process_observed_fixture(&state, scratch, 30_000)
        });

        let conductor_events = events
            .iter()
            .filter(|event| event.track == 2)
            .collect::<Vec<_>>();
        assert_eq!(
            conductor_events.len(),
            1,
            "coincident observed tracks must wake one conductor instance once"
        );
        assert_eq!(conductor_events[0].transpose, 10.0);
    }

    #[test]
    fn scheduler_conductor_demo_spreads_delayed_harmony_by_density() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                4,
                (0..4).map(|_| default_empty_effect_chain()).collect(),
            ));
            for track in 0..4 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[1].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);
            state.pattern.patterns[1].set_step_active(1, true);
            for step in 0..2 {
                state.set_step_param(0, step, StepParam::Transpose, 3.0);
                state.set_step_param(1, step, StepParam::Transpose, 7.0);
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(); 4],
                vec![EffectDescriptor::builtin_sampler(); 4],
                0,
                0,
            );
            let script_path = format!(
                "{}/scripts/processes/process-conductor-demo.lisp",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&script_path).expect("read conductor demo");
            scratch
                .eval_source_at_path(script_path, &source)
                .expect("evaluate conductor demo");

            schedule_process_observed_fixture(&state, scratch, 42_000)
        });

        let responses = events
            .iter()
            .filter(|event| (2..=3).contains(&event.track))
            .collect::<Vec<_>>();
        assert_eq!(
            responses
                .iter()
                .map(|event| (event.track, event.sample_time, event.transpose))
                .collect::<Vec<_>>(),
            vec![
                (2, 18_001, 3.0),
                (3, 18_001, 7.0),
                (2, 30_001, 7.0),
                (3, 30_001, 10.0),
            ],
            "the second sparse call should turn the prior suggestion into two delayed phrases"
        );
    }

    #[test]
    fn scheduler_process_accumulator_lisp_attach_matches_manual_chain() {
        let (_state, events) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture_via_lisp_attach);
        let transposes = events
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(transposes, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn scheduler_process_accumulator_carries_across_pattern_cycles() {
        let (_state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let transposes = events
            .iter()
            .take(10)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(
            transposes,
            vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 3.0]
        );
    }

    #[test]
    fn scheduler_process_accumulator_replay_does_not_double_advance_fold() {
        let (_first_state, first) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let (_second_state, second) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let first_transposes = first
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        let second_transposes = second
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(second_transposes, first_transposes);
    }

    #[test]
    fn scheduler_process_chain_defaults_are_audibly_inert() {
        let (_base_state, baseline) =
            run_with_scheduler_stack(|| run_default_inert_process_accumulator_fixture(false));
        let (_process_state, default_attached) =
            run_with_scheduler_stack(|| run_default_inert_process_accumulator_fixture(true));
        assert_eq!(
            default_attached.iter().take(8).cloned().collect::<Vec<_>>(),
            baseline.iter().take(8).cloned().collect::<Vec<_>>(),
            "attaching a process at defaults must not alter scheduled note timing, transpose, duration, or sampler params"
        );
    }

    #[test]
    fn scheduler_process_target_writes_are_transient_step_param_writes() {
        let (state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        assert_eq!(events[4].transpose, 2.0);
        for step in 0..8 {
            assert_eq!(
                state.pattern.step_data[0].get(step, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
        }
        assert!(state
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .iter()
            .all(|registry| registry == &crate::plock_variants::PlockVariantRegistry::default()));
        assert!(state
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .iter()
            .all(|registry| registry == &crate::plock_variants::PlockVariantRegistry::default()));
        assert!(!state.pattern.instrument_slots[0].key_locks.has_any_lock());
    }

    #[test]
    fn scheduler_process_named_ports_accumulate_ordered_step_writes() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process pitch-a
                      :targets '((pitch (step-param :transpose)))
                      :run (target-add! :pitch 3))
                    (def-process pitch-b
                      :target (step-param :transpose)
                      :run (target-add! 4))
                    (processes :track 0 (pitch-a) (pitch-b))
                    "#,
                )
                .expect("define process chain");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 7.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
        });
    }

    #[test]
    fn scheduler_process_inlet_writes_compose_with_downstream_lane_this_fire() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process process-inlet-setter
                      :targets ((out :process-inlet))
                      :run (target-set! :out 3))
                    (def-process process-inlet-adder
                      :targets ((out :process-inlet))
                      :run (target-add! :out 2))
                    (def-process inlet-driven-pitch
                      :in ((amount :float -12 12 :default 0 :lane true))
                      :target (step-param :transpose)
                      :run (target-add! (in :amount)))

                    (def setter (process-inlet-setter))
                    (def adder (process-inlet-adder))
                    (def pitch (inlet-driven-pitch :amount (lane 1)))
                    (processes :track 0 setter adder pitch)
                    (connect! setter :out (inlet pitch :amount))
                    (connect! adder :out (inlet pitch :amount))
                    "#,
                )
                .expect("define process-inlet chain");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 5.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
            let chain = state.track_process_chain(0).expect("track 0 chain");
            assert_eq!(
                chain.slots[2]
                    .lanes
                    .get("amount")
                    .map(|lane| lane.values.as_slice()),
                Some(&[1.0][..])
            );
        });
    }

    #[test]
    fn scheduler_process_inlet_write_to_earlier_slot_arrives_next_fire() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process earlier-pitch
                      :in ((amount :float -12 12 :default 0 :lane true))
                      :target (step-param :transpose)
                      :run (target-add! (in :amount)))
                    (def-process late-writer
                      :targets ((out :process-inlet))
                      :run (target-set! :out 7))

                    (def pitch (earlier-pitch))
                    (def writer (late-writer))
                    (processes :track 0 pitch writer)
                    (connect! writer :out (inlet pitch :amount))
                    "#,
                )
                .expect("define upstream process-inlet chain");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let transposes = events
                .iter()
                .take(2)
                .map(|event| event.transpose)
                .collect::<Vec<_>>();
            assert_eq!(transposes, vec![0.0, 7.0], "{events:?}");
        });
    }

    #[test]
    fn scheduler_process_veto_suppresses_base_event_but_continues_chain() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process kill-base
                      :run (veto!))
                    (def-process clone-after-veto
                      :target (step-param :transpose)
                      :run (do
                        (target-add! 7)
                        (ratchet! :times 1 :mode :repeat :span 0)))
                    (processes :track 0 (kill-base) (clone-after-veto))
                    "#,
                )
                .expect("define veto chain fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].sample_time, 1);
            assert_eq!(events[0].transpose, 7.0);
        });
    }

    #[test]
    fn scheduler_process_commands_apply_target_writes_in_authored_order() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process ordered-command-stream
                      :target (step-param :transpose)
                      :run (do
                        (ratchet! :times 1 :mode :repeat :span 0)
                        (target-add! 7)
                        (veto!)))
                    (processes :track 0 (ordered-command-stream))
                    "#,
                )
                .expect("define ordered command fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].transpose, 0.0, "{events:?}");
        });
    }

    #[test]
    fn scheduler_process_ratchet_subdivide_offsets_and_scales_duration() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process subdivide-burst
                      :run (do
                        (veto!)
                        (ratchet! :times 4 :mode :subdivide :span 0.25)))
                    (processes :track 0 (subdivide-burst))
                    "#,
                )
                .expect("define subdivide ratchet fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(sample_times, vec![1, 1_501, 3_001, 4_501], "{events:?}");
            assert!(events
                .iter()
                .all(|event| (event.duration - 0.25).abs() < 1e-6));
        });
    }

    #[test]
    fn scheduler_process_ratchet_repeat_keeps_duration_for_ring_through_overlap() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.step_data[0].set(0, StepParam::Duration, 1.0);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process repeat-burst
                      :run (do
                        (veto!)
                        (ratchet! :times 3 :mode :repeat :span 0.125)))
                    (processes :track 0 (repeat-burst))
                    "#,
                )
                .expect("define repeat ratchet fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(sample_times, vec![1, 3_001, 6_001], "{events:?}");
            assert!(events
                .iter()
                .all(|event| (event.duration - 1.0).abs() < 1e-6));
        });
    }

    #[test]
    fn scheduler_process_ratchet_shape_error_drops_burst_without_aborting_lookahead() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process broken-ratchet-shape
                      :run (ratchet! :times 3
                                      :mode :subdivide
                                      :span 0.25
                                      :shape (lambda (i ev)
                                               (if (= i 1)
                                                 (vel! ev "not-a-number")
                                                 ev))))
                    (processes :track 0 (broken-ratchet-shape))
                    "#,
                )
                .expect("define broken ratchet shape fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(
                sample_times,
                vec![0, 6_000],
                "a bad shape should drop each burst atomically while base scheduling continues: {events:?}"
            );
        });
    }

    #[test]
    fn scheduler_process_stale_midi_fx_target_is_noop_without_blocking_other_ports() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process stale-port
                      :targets '((pitch (step-param :transpose))
                                 (gate (fx-param :beat-repeat :gate)))
                      :run (do
                        (target-add! :pitch 5)
                        (target-set! :gate 0)))
                    (processes :track 0 (stale-port))
                    "#,
                )
                .expect("define stale-port process");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 5.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
        });
    }

    #[test]
    fn scheduler_process_midi_fx_param_write_applies_to_temporary_slot_snapshot() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            let source = format!(
                "{}\n{}",
                lisp_host::load_midi_fx_library_source(),
                r#"
                (def-process close-repeat-gate
                  :target (fx-param :beat-repeat :gate)
                  :run (target-set! 0.0))
                (seq-use-midi-fx 0 "beat-repeat")
                (processes :track 0 (close-repeat-gate))
                "#
            );
            scratch
                .eval(&source)
                .expect("define MIDI FX process fixture");

            let gate_idx = scratch
                .midi_fx_descriptors()
                .iter()
                .find(|desc| desc.name == "beat-repeat")
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| param.name.eq_ignore_ascii_case("gate"))
                })
                .expect("beat-repeat gate param");
            let stored_default = state.pattern.midi_fx_slots[0][0].defaults.get(gate_idx);
            assert!((stored_default - 0.90).abs() < 1e-6);

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert!((resolved.duration - 0.05).abs() < 1e-6, "{resolved:?}");
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.midi_fx_slots[0][0].plocks.get(0, gate_idx),
                None
            );
            assert!((state.pattern.midi_fx_slots[0][0].defaults.get(gate_idx) - 0.90).abs() < 1e-6);
        });
    }

    #[test]
    fn scheduler_process_device_param_writes_upsert_transient_event_payloads() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);

            let sampler_desc = EffectDescriptor::builtin_sampler();
            let filter_desc = EffectDescriptor::builtin_filter();
            let speed_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "speed")
                .expect("sampler speed param");
            let release_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "release")
                .expect("sampler release param");
            let filter_mode_param_idx = filter_desc
                .params
                .iter()
                .position(|param| param.name == "mode")
                .expect("filter mode param");

            state.pattern.instrument_slots[0].apply_descriptor(&sampler_desc, 12);
            state.pattern.instrument_slots[0].set_plock(0, speed_param_idx, 0.0);
            state.pattern.effect_chains[0][0].apply_descriptor(&filter_desc, 42);
            state.pattern.effect_chains[0][0].set_plock(0, filter_mode_param_idx, 1.0);
            let mut effect_descriptors = EffectDescriptor::default_full_chain();
            effect_descriptors[0] = filter_desc.clone();
            state.set_scratch_runtime_descriptors(
                vec![effect_descriptors.clone()],
                vec![sampler_desc.clone()],
            );

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![effect_descriptors],
                vec![sampler_desc],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process device-writes
                      :targets '((inst (instrument-param :speed))
                                 (release (instrument-param :release))
                                 (mode (effect-param "Filter" :mode)))
                      :run (do
                        (target-set! :inst 1.0)
                        (target-set! :release 1.0)
                        (target-set! :mode 1.0)))
                    (processes :track 0 (device-writes))
                    "#,
                )
                .expect("define device process fixture");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger {
                    effect_params,
                    instrument_params,
                    sampler_params,
                    ..
                } => {
                    assert!(instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::instruments::sampler::PARAM_SPEED
                            && (param.value - 4.0).abs() < 1e-6
                    }));
                    assert!(
                        (sampler_params.playback_speed - 4.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                    assert!(
                        (sampler_params.release_ms - 2000.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                    assert!(effect_params.iter().any(|param| {
                        param.logical_id == 42
                            && param.idx == crate::effects::filter::FILTER_PARAM_MODE as u64
                            && (param.value - 3.0).abs() < 1e-6
                    }));
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, speed_param_idx),
                Some(0.0)
            );
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, release_param_idx),
                None
            );
            assert!(
                (state.pattern.instrument_slots[0]
                    .defaults
                    .get(release_param_idx))
                .abs()
                    < 1e-6
            );
            assert_eq!(
                state.pattern.effect_chains[0][0]
                    .plocks
                    .get(0, filter_mode_param_idx),
                Some(1.0)
            );
        });
    }

    #[test]
    fn scheduler_phase3a_demo_live_edits_drive_pitch_and_sampler_speed() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);

            let sampler_desc = EffectDescriptor::builtin_sampler();
            let speed_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "speed")
                .expect("sampler speed param");
            state.pattern.instrument_slots[0].apply_descriptor(&sampler_desc, 12);
            state.set_scratch_runtime_descriptors(vec![Vec::new()], vec![sampler_desc.clone()]);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![sampler_desc],
                0,
                0,
            );
            let script_path = format!(
                "{}/scripts/processes/process-phase3a-ports-demo.lisp",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&script_path).expect("read Phase 3A process demo");
            scratch
                .eval_source_at_path(script_path, &source)
                .expect("load Phase 3A process demo");
            scratch
                .eval("(phase3a-port-writer-h :pitch 4)")
                .expect("live edit pitch inlet");
            scratch
                .eval("(phase3a-port-writer-h :speed 0.75)")
                .expect("live edit speed inlet");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger {
                    resolved,
                    instrument_params,
                    sampler_params,
                    ..
                } => {
                    assert!((resolved.transpose - 4.0).abs() < 1e-6, "{resolved:?}");
                    assert!(instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::instruments::sampler::PARAM_SPEED
                            && (param.value - 2.0).abs() < 1e-6
                    }));
                    assert!(
                        (sampler_params.playback_speed - 2.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }

            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, speed_param_idx),
                None
            );
            assert!(
                (state.pattern.instrument_slots[0]
                    .defaults
                    .get(speed_param_idx)
                    - 1.0)
                    .abs()
                    < 1e-6
            );
        });
    }

    #[test]
    fn scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx() {
        std::thread::Builder::new()
            .name("scheduler-routing-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx_body)
            .expect("spawn scheduler routing harness")
            .join()
            .expect("scheduler routing harness panicked");
    }

    fn scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx_body() {
        let state = Arc::new(SequencerState::new(
            5,
            (0..5).map(|_| default_empty_effect_chain()).collect(),
        ));
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        state.toggle_step_and_clear_plocks(0, 4);
        state.set_step_param(0, 0, StepParam::Transpose, 7.0);
        state.set_step_param(0, 4, StepParam::Transpose, 7.0);
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 3.0);
        state.pattern.instrument_slots[2]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[2].defaults.set(12, 2.5);

        publish_test_graph_sequencer(
            Arc::clone(&state),
            r#"
            (def-sequencer "routing-harness-graph"
              :shape (line 2)
              :energy-decay 1
              :reset-every 0
              :seed-on-reset 0
              :max-poly 8
              :max-poly-selection :deterministic
              :duration (steps 1)

              (def-node nrn
                :resolution :16
                :delay 1
                :quantize :16
                :route 0
                :seed-from 0
                :reduce :sum
                :params ((threshold :float 0 4 :default 0.5))
	                :state ((energy :leak (per-step :energy-decay)))
	                :update (if (>= (energy) (param :threshold))
	                          (emit :note (in-note) :vel (in-vel))
	                          false))

	              (edges
	                :from nrn
	                :to nrn
	                :topology (all-to-all)
	                :gather (edge :weight)
	                :params ((weight :float -1 1 :default 0))))
            "#,
        );

        let published_graph = state
            .published_sequencers()
            .into_iter()
            .find(|seq| seq.name == "routing-harness-graph")
            .expect("published graph sequencer");
        let manifest = published_graph.graph.as_ref().expect("graph manifest");
        let edge_group = crate::graph::edge_set_group_id(&manifest.edge_sets[0]);
        state
            .edit_current_graph_overrides(|graphs| {
                graphs.push(ProjectGraphOverrides {
                    sequencer_id: published_graph.id,
                    sequencer_name: published_graph.name.clone(),
                    node_intrinsics: vec![ProjectGraphNodeIntrinsicOverride {
                        group: "nrn".to_string(),
                        instance: 1,
                        resolution: None,
                        delay_steps: None,
                        quantize: None,
                        route: None,
                        seed_from: Some(ProjectGraphSeedFrom::Tracks(Vec::new())),
                        seed_on_reset: None,
                        duration: None,
                        swing: None,
                    }],
                    node_params: Vec::new(),
                    edge_params: vec![ProjectGraphEdgeParamOverride {
                        group: edge_group,
                        from: 0,
                        to: 1,
                        param: "weight".to_string(),
                        value: 1.0,
                    }],
                    reset_every_beats: None,
                    max_poly: None,
                    max_poly_selection: None,
                    node_count: None,
                });
                Ok(())
            })
            .expect("install graph routing overrides");
        let snapshot = state.publish_scheduler_snapshot();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let manifests = state
            .published_sequencers()
            .into_iter()
            .filter_map(|seq| seq.graph)
            .collect::<Vec<_>>();
        reconcile_graph_runtimes(
            manifests,
            &snapshot.graph_overrides,
            &mut scheduler.graph_runtimes,
            &mut scheduler.graph_manifests,
            scheduler.clock.total_beats,
        );
        assert_eq!(scheduler.graph_runtimes.len(), 1);

        let mut scratch_runtime = Some(lisp_host::scratch_runtime_with_fallbacks(
            Arc::clone(&state),
            0,
            0,
        ));
        scratch_runtime
            .as_mut()
            .expect("scratch runtime")
            .eval(&lisp_host::load_midi_fx_library_source())
            .expect("load MIDI FX library");
        let queue = ScheduledEventQueue::<64>::new();
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;

        let result = schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            48_000,
            48_000,
            12_000,
            samples_per_quarter,
            0,
            false,
            false,
        );
        assert_eq!(result.scheduled_until_sample, 48_000);

        let events = observed_triggers(&queue);
        assert!(
            events.iter().any(|event| {
                event.kind == ScheduledTriggerKind::Step
                    && event.track == 0
                    && event.sample_time == 0
                    && event.transpose == 7.0
            }),
            "source seed step should be scheduled: {events:#?}"
        );
        assert!(
            events.iter().any(|event| {
                event.kind == ScheduledTriggerKind::Step
                    && event.track == 2
                    && event.sample_time == 0
                    && event.transpose == 7.0
                    && event.has_speed_param
            }),
            "seed step should route through trigger-to-track to target track with target params: {events:#?}"
        );
        let target_networks = events
            .iter()
            .filter(|event| event.kind == ScheduledTriggerKind::Network && event.track == 2)
            .collect::<Vec<_>>();
        assert!(
            target_networks.len() >= 2,
            "graph propagation should route multiple network events to the target track: {events:#?}"
        );
        assert!(
            target_networks.iter().all(|event| {
                event.transpose == 7.0
                    && event.duration == 0.25
                    && event.sampler_speed == Some(2.5)
                    && event.has_speed_param
            }),
            "routed graph events should carry the target track instrument/sampler params: {events:#?}"
        );
        let source_network_samples = events
            .iter()
            .filter(|event| event.kind == ScheduledTriggerKind::Network && event.track == 0)
            .map(|event| event.sample_time)
            .collect::<Vec<_>>();
        let target_network_samples = target_networks
            .iter()
            .map(|event| event.sample_time)
            .collect::<Vec<_>>();
        assert_eq!(
            target_network_samples,
            vec![6_000, 30_000],
            "graph propagation should produce the expected finite routed target events"
        );
        assert_eq!(
            target_network_samples, source_network_samples,
            "trigger-to-track should copy every graph network event to the target track"
        );
    }

    #[test]
    fn network_trigger_enqueue_applies_target_track_fit_to_scale() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.pattern.track_params[1].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut event = StepEvent {
            track: 1,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        event.resolved.transpose = 3.2;
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            NeuralOutput {
                sample_time: 1_000,
                event,
                emit_trigger: true,
            },
            false,
        ));
        let scheduled = queue.pop().expect("network trigger");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track, resolved, ..
            } => {
                assert_eq!(track, 1);
                assert_eq!(resolved.transpose, 4.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn midi_fx_track_send_applies_destination_fit_to_scale() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["send".to_string()]);
        state.pattern.track_params[1].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "send"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :track 1 :transpose 3.2)))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
            false,
        ));
        let scheduled = queue.pop().expect("routed network trigger");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track, resolved, ..
            } => {
                assert_eq!(track, 1);
                assert_eq!(resolved.transpose, 4.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn trigger_to_track_midi_fx_copies_one_network_trigger_once() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["copy-to-track-2".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "copy-to-track-2"
                  (fx-emit 0 :track 1))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
            false,
        ));

        let first = queue.pop().expect("source network trigger");
        let second = queue.pop().expect("target network trigger");
        let mut tracks = [usize::MAX; 2];
        for (idx, scheduled) in [first, second].into_iter().enumerate() {
            assert_eq!(scheduled.sample_time, 1_000);
            match scheduled.kind {
                ScheduledEventKind::NetworkTrigger { track, .. } => tracks[idx] = track,
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        tracks.sort();
        assert_eq!(tracks, [0, 1]);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn trigger_to_track_midi_fx_drops_recursive_route_cycles() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["copy-to-track-2".to_string()]);
        state.pattern.track_params[1].set_midi_fx_chain(vec!["copy-to-track-1".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "copy-to-track-2"
                  (fx-emit 0 :track 1))

                (def-midi-fx "copy-to-track-1"
                  (fx-emit 0 :track 0))
                "#,
            )
            .unwrap();
        let event = MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: test_resolved_step(),
            chord: Vec::new(),
            chord_durations: Vec::new(),
            chord_delays: Vec::new(),
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: 0.0,
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let events =
            run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], None, 0, false);
        let tracks = events.iter().map(|event| event.track).collect::<Vec<_>>();

        assert_eq!(tracks, vec![0, 1]);
    }

    #[test]
    fn fit_to_scale_preserves_chord_accumulator_offset_after_scheduler_quantize() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let mut resolved = test_resolved_step();
        resolved.transpose = 3.2;
        let mut chord = ScheduledChordData {
            count: 2,
            notes: [0.0; crate::audio::MAX_VOICES],
            durations: [1.0; crate::audio::MAX_VOICES],
            delays: [0.0; crate::audio::MAX_VOICES],
            step_transpose: 2.0,
        };
        chord.notes[0] = 3.0;
        chord.notes[1] = 6.0;

        let (resolved, chord) = apply_fit_to_scale_to_trigger(&snapshot, 0, resolved, chord);

        assert_eq!(resolved.transpose, 4.0);
        assert_eq!(
            resolved_chord_transpose(chord.notes[0], chord.step_transpose, resolved.transpose),
            4.0
        );
        assert_eq!(
            resolved_chord_transpose(chord.notes[1], chord.step_transpose, resolved.transpose),
            7.0
        );
    }

    #[test]
    fn midi_fx_track_send_rebinds_target_params_before_target_chain() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["send".to_string()]);
        state.pattern.track_params[1].set_midi_fx_chain(vec!["octave".to_string()]);
        state.pattern.instrument_slots[1]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[1].defaults.set(12, 2.5);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "send"
                  (fx-emit 0 :track 1))

                (def-midi-fx "octave"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :transpose 12)))
                "#,
            )
            .unwrap();
        let event = MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: test_resolved_step(),
            chord: Vec::new(),
            chord_durations: Vec::new(),
            chord_delays: Vec::new(),
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: 0.0,
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        let events =
            run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], None, 0, false);
        let target = events
            .iter()
            .find(|event| event.track == 1)
            .expect("routed target event");
        assert_eq!(target.resolved.transpose, 12.0);
        assert!(target
            .instrument_params
            .iter()
            .any(|param| param.idx == crate::instruments::sampler::PARAM_SPEED as u64 && param.value == 2.5));
    }

    #[test]
    fn neural_runtime_reload_ignores_non_network_snapshot_edits() {
        let mut network = ProjectNeuralNetwork::default();
        network.id = 1;
        network.num_neurons = 1;
        network.neurons.truncate(1);
        network.weights = vec![vec![0.0]];

        let loaded = Some(vec![network.clone()]);
        assert!(!should_reload_neural_runtime(
            &loaded,
            &[network.clone()],
            0,
            0
        ));
        assert!(should_reload_neural_runtime(
            &loaded,
            &[network.clone()],
            0,
            1
        ));

        let mut edited_network = network;
        edited_network.neurons[0].threshold = 0.5;
        assert!(should_reload_neural_runtime(
            &loaded,
            &[edited_network],
            0,
            0
        ));
    }

    #[test]
    fn resolve_instrument_plocks_returns_only_plocked_params_on_inactive_steps() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track].set_plock(step, 12, 2.0);
        state.pattern.instrument_slots[track].set_plock(step, 13, 0.25);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(!snapshot.tracks[track].steps[step].active);

        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert_eq!(
            params.as_slice(),
            vec![
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: crate::instruments::sampler::PARAM_SPEED,
                    span: 1,
                    value: 2.0,
                },
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: crate::instruments::sampler::PARAM_SCRUB_OFFSET,
                    span: 1,
                    value: 0.25,
                },
            ]
        );
    }

    #[test]
    fn resolve_instrument_plocks_drops_stale_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track]
            .plocks
            .set(step, 12, 2.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert!(params.is_empty());
    }

    #[test]
    fn resolve_sampler_params_drops_stale_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track]
            .plocks
            .set(step, 12, 2.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);

        assert_eq!(params.playback_speed, 1.0);
    }

    #[test]
    fn resolve_sampler_params_carries_beats_warp_controls_by_node_param() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor::builtin_sampler();
        let param_idx = |node_idx: u64| {
            desc.params
                .iter()
                .position(|param| param.node_param_idx == node_idx as u32)
                .expect("sampler param should exist")
        };
        let preserve_idx = param_idx(crate::instruments::sampler::PARAM_WARP_PRESERVE);
        let fill_idx = param_idx(crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE);
        let decay_idx = param_idx(crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE);
        let slot = &state.pattern.instrument_slots[track];
        slot.apply_descriptor(&desc, 12);
        slot.defaults
            .set(preserve_idx, crate::instruments::warp_grid::PRESERVE_1_8 as f32);
        slot.defaults
            .set(fill_idx, crate::instruments::sampler::SEG_LOOP_PINGPONG as f32);
        slot.defaults.set(decay_idx, 0.25);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);
        assert_eq!(params.warp_preserve, crate::instruments::warp_grid::PRESERVE_1_8 as f32);
        assert_eq!(
            params.warp_seg_loop_mode,
            crate::instruments::sampler::SEG_LOOP_PINGPONG as f32
        );
        assert!((params.warp_seg_envelope - 0.25).abs() < 0.0001);

        slot.set_plock(step, preserve_idx, crate::instruments::warp_grid::PRESERVE_1_16 as f32);
        slot.set_plock(step, fill_idx, crate::instruments::sampler::SEG_LOOP_OFF as f32);
        slot.set_plock(step, decay_idx, 0.75);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);
        assert_eq!(params.warp_preserve, crate::instruments::warp_grid::PRESERVE_1_16 as f32);
        assert_eq!(
            params.warp_seg_loop_mode,
            crate::instruments::sampler::SEG_LOOP_OFF as f32
        );
        assert!((params.warp_seg_envelope - 0.75).abs() < 0.0001);
    }

    #[test]
    fn enqueue_step_event_step_source_carries_sampler_params() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<4>::new();
        let mut track_output_events = Vec::new();
        let mut sampler_params = ScheduledSamplerParams::default();
        sampler_params.warp_enabled = 1.0;
        sampler_params.warp_mode = crate::instruments::sampler::WARP_MODE_BEATS as f32;
        sampler_params.sample_bpm = 174.0;
        sampler_params.warp_preserve = crate::instruments::warp_grid::PRESERVE_1_16 as f32;
        sampler_params.warp_seg_loop_mode = crate::instruments::sampler::SEG_LOOP_PINGPONG as f32;
        sampler_params.warp_seg_envelope = 0.5;

        let event = StepEvent {
            track: 0,
            samples_per_step: 6_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params,
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        assert!(super::enqueue_step_event(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            event,
        ));

        let scheduled = queue.pop().expect("scheduled step trigger");
        let ScheduledEventKind::ResolvedTrigger { sampler_params, .. } = scheduled.kind else {
            panic!("expected resolved trigger");
        };
        assert_eq!(
            sampler_params.warp_preserve,
            crate::instruments::warp_grid::PRESERVE_1_16 as f32
        );
        assert_eq!(
            sampler_params.warp_seg_loop_mode,
            crate::instruments::sampler::SEG_LOOP_PINGPONG as f32
        );
        assert!((sampler_params.warp_seg_envelope - 0.5).abs() < 0.0001);
    }

    #[test]
    fn resolve_effect_params_routes_modulator_params_to_effect_bank() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor {
            name: "modded effect".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 12,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod1_source".to_string(),
                    min: 0.0,
                    max: 8.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec!["off".to_string(), "lfo".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::instruments::voice_modulator::MOD_PARAM_BASE
                        + crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        state.pattern.effect_chains[track][0].apply_descriptor_with_modulator(&desc, 42, 77);
        state.pattern.effect_chains[track][0].set_plock(step, 0, 0.75);
        state.pattern.effect_chains[track][0].set_plock(step, 1, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_effect_params(&snapshot, track, step);

        assert_eq!(
            params,
            vec![
                ScheduledEffectParam {
                    logical_id: 42,
                    idx: 12,
                    value: 0.75,
                },
                ScheduledEffectParam {
                    logical_id: 77,
                    idx: crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u64,
                    value: 1.0,
                },
            ]
        );
    }

    #[test]
    fn neuron_output_overrides_apply_only_with_matching_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let mut neuron = ProjectNeuron::default();
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track: track,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: crate::instruments::sampler::PARAM_SPEED as u32,
                },
                param_index: 12,
                value: 2.5,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "test".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let mut event = StepEvent {
            track,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let parameter_events =
            apply_neuron_output_overrides(&snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert_eq!(event.sampler_params.playback_speed, 2.5);
        assert_eq!(
            event.instrument_params.as_slice(),
            &[ScheduledInstrumentParam {
                target: ScheduledInstrumentParamTarget::Synth,
                idx: crate::instruments::sampler::PARAM_SPEED,
                span: 1,
                value: 2.5,
            }]
        );

        let mut stale_snapshot = snapshot.clone();
        stale_snapshot.neural_networks[0].neurons[0]
            .output_overrides
            .instrument[0]
            .param_id = ParamNodeId {
            logical_id: 99,
            node_param_idx: crate::instruments::sampler::PARAM_SPEED as u32,
        };
        let mut stale_event = event.clone();
        stale_event.instrument_params.clear();
        stale_event.sampler_params = ScheduledSamplerParams::default();

        let parameter_events = apply_neuron_output_overrides(
            &stale_snapshot,
            0,
            Some(stale_event.track),
            &mut stale_event,
        );
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert!(stale_event.instrument_params.is_empty());
        assert_eq!(stale_event.sampler_params.playback_speed, 1.0);
    }

    #[test]
    fn neuron_effect_output_overrides_match_modulator_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let desc = EffectDescriptor {
            name: "modded effect".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![ParamDescriptor {
                name: "mod1_source".to_string(),
                min: 0.0,
                max: 8.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec!["off".to_string(), "lfo".to_string()],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::instruments::voice_modulator::MOD_PARAM_BASE
                    + crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }],
        };
        state.pattern.effect_chains[track][0].apply_descriptor_with_modulator(&desc, 42, 77);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let mut neuron = ProjectNeuron::default();
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: track,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 77,
                    node_param_idx: crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u32,
                },
                param_index: 0,
                value: 1.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "test".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let mut event = StepEvent {
            track,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let parameter_events =
            apply_neuron_output_overrides(&snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert_eq!(
            event.effect_params,
            vec![ScheduledEffectParam {
                logical_id: 77,
                idx: crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u64,
                value: 1.0,
            }]
        );

        let mut stale_snapshot = snapshot.clone();
        stale_snapshot.neural_networks[0].neurons[0]
            .output_overrides
            .effects[0]
            .param_id = ParamNodeId {
            logical_id: 42,
            node_param_idx: crate::instruments::voice_modulator::PARAM_SLOT_SOURCE as u32,
        };
        event.effect_params.clear();

        let parameter_events =
            apply_neuron_output_overrides(&stale_snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert!(event.effect_params.is_empty());
    }

    #[test]
    fn hidden_neuron_emits_target_parameter_events_without_network_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let sampler_speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        let sampler_speed_node_param_idx =
            sampler_desc.params[sampler_speed_param_idx].node_param_idx;
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let filter_param_idx = 0;
        let filter_node_param_idx =
            EffectDescriptor::builtin_filter().params[filter_param_idx].node_param_idx;
        let mut neuron = ProjectNeuron::default();
        neuron.route = None;
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track: 1,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: sampler_speed_node_param_idx,
                },
                param_index: sampler_speed_param_idx,
                value: 1.75,
            });
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: filter_node_param_idx,
                },
                param_index: filter_param_idx,
                value: 640.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "hidden".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let event = StepEvent {
            track: 0,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            7,
            1234,
            48_000.0,
            0.0,
            0.0,
            NeuralOutput {
                sample_time: 1234,
                event,
                emit_trigger: false,
            },
            false,
        ));

        let first = queue.pop().expect("instrument parameter event");
        assert_eq!(first.pattern_epoch, 7);
        assert_eq!(first.sample_time, 1234);
        match first.kind {
            ScheduledEventKind::InstrumentParams {
                track,
                instrument_params,
                instrument_tensor_params,
            } => {
                assert_eq!(track, 1);
                assert!(instrument_tensor_params.is_empty());
                assert_eq!(
                    instrument_params.as_slice(),
                    &[ScheduledInstrumentParam {
                        target: ScheduledInstrumentParamTarget::Synth,
                        idx: sampler_speed_node_param_idx as u64,
                        span: 1,
                        value: 1.75,
                    }]
                );
            }
            other => panic!("expected instrument params, got {other:?}"),
        }

        let second = queue.pop().expect("effect parameter event");
        assert_eq!(second.pattern_epoch, 7);
        assert_eq!(second.sample_time, 1234);
        match second.kind {
            ScheduledEventKind::EffectParams {
                track,
                effect_params,
            } => {
                assert_eq!(track, 1);
                assert_eq!(
                    effect_params,
                    vec![ScheduledEffectParam {
                        logical_id: 42,
                        idx: filter_node_param_idx as u64,
                        value: 640.0,
                    }]
                );
            }
            other => panic!("expected effect params, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn routed_neuron_emits_cross_track_parameter_event_before_own_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let filter_param_idx = 0;
        let filter_node_param_idx =
            EffectDescriptor::builtin_filter().params[filter_param_idx].node_param_idx;
        let mut neuron = ProjectNeuron::default();
        neuron.route = Some(0);
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: filter_node_param_idx,
                },
                param_index: filter_param_idx,
                value: 900.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "cross".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let event = StepEvent {
            track: 0,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::audio::MAX_VOICES],
                durations: [0.0; crate::audio::MAX_VOICES],
                delays: [0.0; crate::audio::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            7,
            1234,
            48_000.0,
            0.0,
            0.0,
            NeuralOutput {
                sample_time: 1234,
                event,
                emit_trigger: true,
            },
            false,
        ));

        let first = queue.pop().expect("cross-track effect parameter event");
        match first.kind {
            ScheduledEventKind::EffectParams {
                track,
                effect_params,
            } => {
                assert_eq!(track, 1);
                assert_eq!(
                    effect_params,
                    vec![ScheduledEffectParam {
                        logical_id: 42,
                        idx: filter_node_param_idx as u64,
                        value: 900.0,
                    }]
                );
            }
            other => panic!("expected cross-track effect params, got {other:?}"),
        }

        let second = queue.pop().expect("routed network trigger");
        match second.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                effect_params,
                source_neuron,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(source_neuron, 0);
                assert!(effect_params.is_empty());
            }
            other => panic!("expected routed network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn resolve_instrument_plocks_preserves_param_node_spans() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor {
            name: "custom".to_string(),
            input_channels: 0,
            output_channels: 1,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 80.0,
                    max: 12_000.0,
                    default: 7200.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 105,
                    node_param_span: 4,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "__dgen_mod_active__cutoff".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: 109,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        state.pattern.instrument_slots[track].apply_descriptor(&desc, 12);
        state.pattern.instrument_slots[track].set_plock(step, 0, 9155.0);
        state.pattern.instrument_slots[track].set_plock(step, 1, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert_eq!(
            params.as_slice(),
            vec![
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 105,
                    span: 4,
                    value: 9155.0,
                },
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 109,
                    span: 1,
                    value: 1.0,
                },
            ]
        );
    }

    #[test]
    fn resolve_instrument_tensor_params_uses_default_and_step_plocked_matrix() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 9;
        let desc = EffectDescriptor {
            name: "tensor instrument".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            }],
            params: Vec::new(),
        };
        state.pattern.instrument_slots[track].apply_descriptor(&desc, 12);
        state.pattern.instrument_slots[track]
            .tensor_params
            .set_plock_cell(step, 0, 1, 0.95)
            .expect("tensor p-lock edit");

        let snapshot = state.publish_scheduler_snapshot();
        let defaults = super::resolve_instrument_tensor_params(&snapshot, track, 0);
        let plocked = super::resolve_instrument_tensor_params(&snapshot, track, step);
        let explicit_plocks = super::resolve_instrument_tensor_plocks(&snapshot, track, step);
        let default_only = super::resolve_instrument_tensor_defaults(&snapshot, track);

        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].cell_offset, 64);
        assert_eq!(defaults[0].values, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(default_only.as_slice(), defaults.as_slice());
        assert_eq!(plocked.len(), 1);
        assert_eq!(plocked[0].values, vec![0.1, 0.95, 0.3, 0.4]);
        assert_eq!(explicit_plocks.as_slice(), plocked.as_slice());
        assert_ne!(
            super::instrument_sound_fingerprint(
                &snapshot,
                track,
                ScheduledInstrumentParams::new().as_slice(),
                defaults.as_slice(),
            ),
            super::instrument_sound_fingerprint(
                &snapshot,
                track,
                ScheduledInstrumentParams::new().as_slice(),
                plocked.as_slice(),
            )
        );
    }

    #[test]
    fn track_note_spans_fold_later_notes_into_running_group() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 4.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 7.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        assert_eq!(first_group.len(), 3);
        assert_eq!(first_group[0].transpose, 0.0);
        assert_eq!(first_group[1].transpose, 4.0);
        assert_eq!(first_group[2].transpose, 7.0);
        assert_eq!(first_group[2].start_beats, 1.0);
        assert_eq!(first_group[2].end_beats, 2.0);

        let later_group = track_note_spans_for_trigger(&snapshot, track, 4);
        assert!(later_group.is_empty());
    }

    #[test]
    fn track_note_spans_include_step_delay_in_start_time() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.step_data[track].set(0, StepParam::Delay, 0.5);
        state.pattern.step_data[track].set(0, StepParam::Duration, 0.5);

        state.pattern.patterns[track].set_step_active(1, true);
        state.pattern.step_data[track].set(1, StepParam::Transpose, 7.0);
        state.pattern.step_data[track].set(1, StepParam::Delay, 0.25);
        state.pattern.step_data[track].set(1, StepParam::Duration, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        assert_eq!(first_group.len(), 1);
        assert_eq!(first_group[0].start_beats, 0.0);
        assert_eq!(first_group[0].end_beats, 0.125);

        let later_group = track_note_spans_for_trigger(&snapshot, track, 1);
        assert_eq!(later_group.len(), 1);
        assert_eq!(later_group[0].transpose, 7.0);
        assert_eq!(later_group[0].start_beats, 0.0);
        assert_eq!(later_group[0].end_beats, 0.25);
    }

    #[test]
    fn track_note_spans_include_per_note_delays_for_strums() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_timing(0, 0.0, 1.0, 0.0);
        state.pattern.chord_data[track].add_note_with_timing(0, 4.0, 1.0, 0.25);
        state.pattern.chord_data[track].add_note_with_timing(0, 7.0, 1.0, 0.5);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_note_spans_for_trigger(&snapshot, track, 0);

        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].transpose, 0.0);
        assert_eq!(spans[0].start_beats, 0.0);
        assert_eq!(spans[1].transpose, 4.0);
        assert_eq!(spans[1].start_beats, 0.0625);
        assert_eq!(spans[2].transpose, 7.0);
        assert_eq!(spans[2].start_beats, 0.125);
    }

    #[test]
    fn track_note_spans_include_strums_with_no_gridline_note() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_timing(0, 0.0, 1.0, 0.25);
        state.pattern.chord_data[track].add_note_with_timing(0, 4.0, 1.0, 0.5);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_note_spans_for_trigger(&snapshot, track, 0);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start_beats, 0.0625);
        assert_eq!(spans[1].start_beats, 0.125);
    }

    #[test]
    fn scheduler_note_grouping_follows_staggered_piano_roll_pattern() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 12.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 12.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 12.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(8, true);
        state.pattern.step_data[track].set(8, StepParam::Transpose, 19.0);
        state.pattern.step_data[track].set(8, StepParam::Duration, 2.0);

        state.pattern.patterns[track].set_step_active(12, true);
        state.pattern.step_data[track].set(12, StepParam::Transpose, 24.0);
        state.pattern.step_data[track].set(12, StepParam::Duration, 4.0);

        state.toggle_play();
        let snapshot = state.publish_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        let triggers = clock.process_chunk(84_000, &snapshot, &state);
        let active_trigger_steps = triggers
            .iter()
            .filter(|trigger| snapshot.tracks[trigger.track].steps[trigger.step].active)
            .map(|trigger| trigger.step)
            .collect::<Vec<_>>();
        assert_eq!(active_trigger_steps, vec![0, 4, 8, 12]);

        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        let first_transposes = first_group
            .iter()
            .map(|note| note.transpose)
            .collect::<Vec<_>>();
        let first_starts = first_group
            .iter()
            .map(|note| note.start_beats)
            .collect::<Vec<_>>();
        let first_ends = first_group
            .iter()
            .map(|note| note.end_beats)
            .collect::<Vec<_>>();
        assert_eq!(first_transposes, vec![0.0, 7.0, 12.0, 19.0]);
        assert_eq!(first_starts, vec![0.0, 0.0, 1.0, 2.0]);
        assert_eq!(first_ends, vec![3.0, 1.0, 3.0, 2.5]);

        assert!(track_note_spans_for_trigger(&snapshot, track, 4).is_empty());
        assert!(track_note_spans_for_trigger(&snapshot, track, 8).is_empty());

        let next_group = track_note_spans_for_trigger(&snapshot, track, 12);
        assert_eq!(next_group.len(), 1);
        assert_eq!(next_group[0].transpose, 24.0);
        assert_eq!(next_group[0].start_beats, 0.0);
        assert_eq!(next_group[0].end_beats, 1.0);
    }

    #[test]
    fn active_note_spans_at_beat_exposes_current_sequenced_pool_for_live_join() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 12.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_active_note_spans_at_beat(&snapshot, track, 1.0, 0.25);
        let transposes = spans.iter().map(|span| span.transpose).collect::<Vec<_>>();

        assert_eq!(transposes, vec![0.0, 12.0]);
        assert!(spans.iter().all(|span| span.start_beats == 0.0));
        assert!(spans.iter().all(|span| span.end_beats <= 0.25));
    }

    #[test]
    fn midi_fx_window_events_clip_recorded_notes_to_tick_windows() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track].set_midi_fx_chain(vec!["arp".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let arp_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "arp")
            .expect("arp descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(arp_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 4.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        let snapshot = state.publish_scheduler_snapshot();
        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            6_000.0,
            0.25,
            24_000.0,
            0.0,
            ResolvedStep {
                duration: 8.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 8);
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event.offset_beats, idx as f32 * 0.25);
            assert_eq!(event.samples_per_step, 6_000.0);
            assert_eq!(event.step_beats, 0.25);
            assert_eq!(event.resolved.duration, 1.0);
            assert_eq!(event.chord, vec![0.0, 4.0, 7.0]);
            let spans = event.note_spans.as_ref().expect("window spans");
            assert_eq!(spans.len(), 3);
            assert!(spans.iter().all(|span| span.start_beats == 0.0));
            assert!(spans.iter().all(|span| span.end_beats <= 0.25));
            assert_eq!(event.arp_phase_beats, idx as f32 * 0.25);
        }
    }

    #[test]
    fn midi_fx_window_events_do_not_treat_event_param_as_tick_rate() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let trigger_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(trigger_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 6.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0).is_none()
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            2.0,
            24_000.0,
            0.0,
            ResolvedStep {
                duration: 8.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].samples_per_step, 48_000.0);
        assert_eq!(events[0].step_beats, 2.0);
        assert_eq!(
            events[0]
                .note_spans
                .as_ref()
                .expect("source note spans")
                .len(),
            2
        );
    }

    #[test]
    fn midi_fx_window_events_do_not_clock_spatial_harmonic_delay() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track]
            .set_midi_fx_chain(vec!["spatial-harmonic-delay".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let spatial_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "spatial-harmonic-delay")
            .expect("spatial-harmonic-delay descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(spatial_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0).is_none()
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            1.0,
            48_000.0,
            0.0,
            ResolvedStep {
                duration: 4.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].samples_per_step, 48_000.0);
        assert_eq!(events[0].step_beats, 1.0);
        let spans = events[0].note_spans.as_ref().expect("source note spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_beats, 0.0);
        assert_eq!(spans[0].end_beats, 1.0);
    }

    #[test]
    fn midi_fx_window_events_clock_beat_repeat_over_source_duration() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track].set_midi_fx_chain(vec!["beat-repeat".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let repeat_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "beat-repeat")
            .expect("beat-repeat descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(repeat_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert_eq!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0),
            Some(0.25)
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            1.0,
            48_000.0,
            0.0,
            ResolvedStep {
                duration: 4.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 4);
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event.offset_beats, idx as f32 * 0.25);
            assert_eq!(event.samples_per_step, 12_000.0);
            assert_eq!(event.step_beats, 0.25);
            assert_eq!(event.resolved.duration, 1.0);
            assert_eq!(event.chord, vec![0.0]);
        }
    }

    #[test]
    fn event_driven_live_midi_fx_processes_pending_note_once() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut live_tracks: [super::LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| super::LiveMidiFxTrackState::default());
        live_tracks[0].notes.push(super::LiveMidiFxNote {
            transpose: 7.0,
            velocity: 0.8,
            pending_event: true,
        });

        assert!(super::schedule_live_midi_fx(
            Some(&mut runtime),
            &state,
            &snapshot,
            &queue,
            0,
            1_000,
            0.0,
            512,
            48_000,
            &mut live_tracks,
            false,
        ));

        let mut tracks = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::ResolvedTrigger {
                    track, resolved, ..
                } => {
                    assert_eq!(event.sample_time, 1_000);
                    assert_eq!(resolved.transpose, 7.0);
                    tracks.push(track);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
        }
        tracks.sort_unstable();
        assert_eq!(tracks, vec![0, 1]);

        assert!(super::schedule_live_midi_fx(
            Some(&mut runtime),
            &state,
            &snapshot,
            &queue,
            0,
            1_256,
            0.0,
            512,
            48_000,
            &mut live_tracks,
            false,
        ));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn live_midi_fx_start_quantizes_to_next_tick() {
        let rendered_sample = 48_000;
        let samples_per_quarter = 24_000.0;

        assert_eq!(
            quantized_live_tick_sample(rendered_sample, 1.25, 0.25, samples_per_quarter),
            rendered_sample
        );
        assert_eq!(
            quantized_live_tick_sample(rendered_sample, 1.30, 0.25, samples_per_quarter),
            rendered_sample + 4_800
        );
    }
