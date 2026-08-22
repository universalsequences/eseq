    use super::{
        apply_fit_to_scale_to_trigger, apply_neuron_output_overrides, delayed_step_sample_time,
        enqueue_resolved_trigger, enqueue_step_event_with_midi_fx, invoke_process_cascade,
        midi_fx_window_events_from_step, process_device_write_value, quantized_live_tick_sample,
        reconcile_graph_runtimes, resolve_effect_defaults, resolve_effect_params,
        resolve_instrument_plocks,
        reconcile_playing_topology_change, resolve_sampler_params, resolve_track_send_params,
        resolved_slot_param_value, run_midi_fx_chain_for_track, schedule_playing_lookahead,
        should_reload_neural_runtime, topology_edit_frontier_drained,
        swing_delay_samples_from_quarter, swung_network_sample_time,
        track_active_note_spans_at_beat,
        track_note_spans_for_trigger, EmittedNetworkEventSource, LiveMidiFxTrackState, MidiFxEvent,
        MidiFxQuantizerState, SchedulerLookaheadState, SnapshotSequencerClock,
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
        MAX_STEPS, MAX_TRACKS,
    };
    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn track_send_plock_resolves_at_step_and_restores_pattern_baseline() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let destination = crate::sequencer::BusId::DEFAULT_A;
        state.pattern.track_params[0].set_sends(vec![crate::sequencer::TrackSendSnapshot {
            destination,
            amount: 0.2,
        }]);
        state.pattern.track_send_plocks[0].set(3, destination, 0.85);
        state.set_track_send_runtime_targets(0, vec![crate::sequencer::TrackSendRuntimeTarget {
            destination,
            left_id: 700,
            right_id: 701,
        }]);
        let snapshot = state.publish_scheduler_snapshot();

        let locked = resolve_track_send_params(&snapshot, 0, 3);
        assert_eq!(
            locked.iter().map(|param| (param.logical_id, param.value)).collect::<Vec<_>>(),
            vec![(700, 0.85), (701, 0.85)],
        );
        let restored = resolve_track_send_params(&snapshot, 0, 4);
        assert_eq!(
            restored.iter().map(|param| (param.logical_id, param.value)).collect::<Vec<_>>(),
            vec![(700, 0.2), (701, 0.2)],
        );
    }

    #[test]
    fn scheduler_events_apply_track_send_plock_then_restore_zero_baseline() {
        // schedule_playing_lookahead needs the scheduler thread's stack budget.
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            let destination = crate::sequencer::BusId::DEFAULT_A;
            state.pattern.track_params[0].set_sends(vec![crate::sequencer::TrackSendSnapshot {
                destination,
                amount: 0.0,
            }]);
            state.pattern.track_send_plocks[0].set(1, destination, 0.8);
            state.set_track_send_runtime_targets(0, vec![crate::sequencer::TrackSendRuntimeTarget {
                destination,
                left_id: 700,
                right_id: 701,
            }]);
            for step in 0..3 {
                state.toggle_step_and_clear_plocks(0, step);
            }
            // Toggling an empty step clears its complete payload, so stamp the lock
            // after activating the step exactly as the authoring command does.
            state.pattern.track_send_plocks[0].set(1, destination, 0.8);
            state.toggle_play();
            let snapshot = state.publish_scheduler_snapshot();
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            let queue = ScheduledEventQueue::<64>::new();
            let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            let mut scratch_runtime = None;
            let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;

            schedule_playing_lookahead(
                &mut scheduler,
                &state,
                &snapshot,
                &queue,
                &mut scratch_runtime,
                &live_midi_fx_tracks,
                snapshot.transport.pattern_epoch,
                0,
                24_000,
                24_000,
                12_000,
                samples_per_quarter,
                0,
                false,
                false,
            );

            let mut values = Vec::new();
            while let Some(event) = queue.pop() {
                if let ScheduledEventKind::ResolvedTrigger { step, effect_params, .. } = event.kind {
                    let value = effect_params.iter()
                        .find(|param| param.logical_id == 700)
                        .map(|param| param.value);
                    values.push((step, value));
                }
            }
            assert!(values.contains(&(0, Some(0.0))), "step 0 baseline missing: {values:?}");
            assert!(values.contains(&(1, Some(0.8))), "step 1 send p-lock missing: {values:?}");
            assert!(values.contains(&(2, Some(0.0))), "step 2 baseline restore missing: {values:?}");
        });
    }

    #[test]
    fn topology_delete_waits_for_existing_lookahead_frontier() {
        assert!(!topology_edit_frontier_drained(11_999, 12_000));
        assert!(topology_edit_frontier_drained(12_000, 12_000));
        assert!(topology_edit_frontier_drained(12_001, 12_000));
    }

    #[test]
    fn additive_track_topology_preserves_existing_lookahead_and_schedules_new_lane() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                3,
                vec![
                    default_empty_effect_chain(),
                    default_empty_effect_chain(),
                    default_empty_effect_chain(),
                ],
            ));
            for track in 0..3 {
                for step in 0..MAX_STEPS {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }
            state.transport.playing.store(true, Ordering::Relaxed);

            let published = state.publish_scheduler_snapshot();
            let mut initial = (*published).clone();
            initial.tracks.truncate(2);
            initial.transport.num_tracks = 2;
            let initial = Arc::new(initial);
            let queue = ScheduledEventQueue::<32>::new();
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            let mut scratch_runtime = None;
            let samples_per_quarter = 48_000.0 * 60.0 / initial.transport.bpm as f64;

            let first = schedule_playing_lookahead(
                &mut scheduler,
                &state,
                &initial,
                &queue,
                &mut scratch_runtime,
                &live_midi_fx_tracks,
                initial.transport.pattern_epoch,
                0,
                6_000,
                48_000,
                6_000,
                samples_per_quarter,
                0,
                false,
                false,
            );
            let old_frontier = first.scheduled_until_sample;

            let added = state.publish_event_compatible_topology();
            assert_eq!(
                added.transport.pattern_epoch, initial.transport.pattern_epoch,
                "an additive topology publication must keep queued event epochs valid"
            );
            let mut scheduled_until_sample = old_frontier;
            reconcile_playing_topology_change(
                &mut scheduler,
                &state,
                &added,
                &queue,
                old_frontier,
                &mut scheduled_until_sample,
                2,
                initial.transport.pattern_epoch,
            );
            assert_eq!(
                scheduled_until_sample, old_frontier,
                "additive growth must preserve the existing scheduling frontier"
            );

            let second = schedule_playing_lookahead(
                &mut scheduler,
                &state,
                &added,
                &queue,
                &mut scratch_runtime,
                &live_midi_fx_tracks,
                added.transport.pattern_epoch,
                old_frontier,
                6_000,
                48_000,
                6_000,
                samples_per_quarter,
                scheduled_until_sample,
                false,
                false,
            );
            assert!(second.scheduled_until_sample > old_frontier);

            let mut saw_preserved = [false; 2];
            let mut saw_new_track = false;
            while let Some(event) = queue.pop() {
                assert_eq!(
                    event.pattern_epoch, added.transport.pattern_epoch,
                    "the audio callback must accept events across the additive publication"
                );
                if let ScheduledEventKind::ResolvedTrigger { track, .. } = event.kind {
                    if track < 2 && event.sample_time < old_frontier {
                        saw_preserved[track] = true;
                    }
                    if track == 2 && event.sample_time >= old_frontier {
                        saw_new_track = true;
                    }
                }
            }
            assert_eq!(saw_preserved, [true, true], "existing tracks lost queued events");
            assert!(saw_new_track, "the appended track never joined scheduling");
        });
    }

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
                neural_group: None,
            }],
            node_params: Vec::new(),
            edge_params: Vec::new(),
            reset_every_beats: None,
            max_poly: None,
            max_poly_selection: None,
            node_count: None,
            group_gain: None,
            group_coupling: None,
            group_trace_decay: None,
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
    fn graph_node_count_reconcile_keeps_surviving_deltas_and_drops_out_of_range_keys() {
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
        let surviving = crate::graph::GraphDeltaKey::EdgeParam {
            from: 1,
            to: 1,
            param: "weight".to_string(),
        };
        let removed = crate::graph::GraphDeltaKey::EdgeParam {
            from: 7,
            to: 7,
            param: "weight".to_string(),
        };
        runtimes[0].nudge(surviving.clone(), 0.25).unwrap();
        runtimes[0].nudge(removed.clone(), 0.5).unwrap();

        let overrides = ProjectGraphOverrides {
            sequencer_id: 1,
            sequencer_name: "g".into(),
            node_count: Some(4),
            ..ProjectGraphOverrides::default()
        };
        reconcile_graph_runtimes(
            vec![manifest],
            &[overrides],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        assert_eq!(runtimes[0].num_nodes(), 4);
        assert_eq!(runtimes[0].delta(&surviving), 0.25);
        assert_eq!(runtimes[0].delta(&removed), 0.0);
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

    /// Recording an arrangement stamps free-run phase through the pattern's
    /// REAL geometry (`PatternStepGeometry`); anchored song playback of that
    /// stamp must then reproduce the free-run performance trigger-for-trigger
    /// — including on patterns with timebase and sync plocks, where the old
    /// uniform base-timebase stamping drifted the clip clock against the
    /// transport and made sync plocks snap to the wrong grid.
    #[test]
    fn anchored_playback_reproduces_free_run_with_timebase_and_sync_plocks() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.toggle_play();
        for step in 0..16 {
            state.toggle_step_and_clear_plocks(0, step);
        }
        let mut snapshot = (*state.latest_scheduler_snapshot()).clone();
        {
            let track = Arc::make_mut(&mut snapshot.tracks[0]);
            // Step 0: half-beat step + 1-beat sync grid (pads the cycle to
            // 5.0); step 5: 1-beat sync grid (mid-pattern wait 1.5 -> 2.0).
            track.steps[0].timebase_override = Some(Timebase::Eighth);
            track.steps[0].params[StepParam::Sync.index()] = 3.0;
            track.steps[5].params[StepParam::Sync.index()] = 3.0;
        }

        let run = |clock: &mut SnapshotSequencerClock| {
            let mut triggers = Vec::new();
            while clock.total_beats < 24.0 {
                triggers.extend(
                    clock
                        .process_chunk(512, &snapshot, &state)
                        .into_iter()
                        .map(|trigger| (trigger.step, trigger.absolute_beats)),
                );
            }
            triggers
        };

        let mut free_run = SnapshotSequencerClock::new(48_000);
        let free_triggers = run(&mut free_run);

        // The unquantized scene change the performance made at beat 5.3,
        // stamped with the same real geometry capture uses.
        let geometry = crate::sequencer::PatternStepGeometry::new(
            16,
            Timebase::Sixteenth,
            |step| match step {
                0 => (Some(Timebase::Eighth), 3.0),
                5 => (None, 3.0),
                _ => (None, 0.0),
            },
        );
        let anchor_beat = 5.3;
        let offset = geometry.steps_at_beats(anchor_beat);
        let mut anchored = SnapshotSequencerClock::new(48_000);
        anchored.set_song_row_anchors(anchor_beat, &[offset]);
        let anchored_triggers = run(&mut anchored);

        assert_eq!(free_triggers.len(), anchored_triggers.len());
        for (free, anchored) in free_triggers.iter().zip(&anchored_triggers) {
            assert_eq!(free.0, anchored.0, "step order must match free-run");
            assert!(
                (free.1 - anchored.1).abs() < 1e-3,
                "step {} fired at {} anchored vs {} free-run",
                free.0,
                anchored.1,
                free.1
            );
        }

        // The user-facing guarantee: the sync-plocked steps stay on the
        // global transport's 1-beat grid under the anchored (recorded) clip.
        for (step, beats) in &anchored_triggers {
            if *step == 0 || *step == 5 {
                let distance = (beats - beats.round()).abs();
                assert!(
                    distance < 1e-3,
                    "sync-plocked step {step} fired off the transport grid at {beats}"
                );
            }
        }
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

    /// Live step-param printing (bead eseq-jc9): with the print latch armed,
    /// the scheduler substitutes the latched values when it resolves a step
    /// event on the armed track — that is what makes a print audible on the
    /// SAME pass (the pattern write lands behind the playhead), instead of
    /// one loop later. Untouched params and other tracks stay untouched.
    #[test]
    fn scheduler_lookahead_substitutes_armed_step_print_values() {
        run_with_scheduler_stack(scheduler_lookahead_substitutes_armed_step_print_values_body);
    }

    fn scheduler_lookahead_substitutes_armed_step_print_values_body() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Velocity, 0.7);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 5.0);
        state.transport.playing.store(true, Ordering::Relaxed);
        state
            .step_print_override
            .set(0, Some(0.25), Some(3.0), None);

        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;

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

        let scheduled = queue.pop().expect("step 0 trigger");
        let ScheduledEventKind::ResolvedTrigger {
            track, resolved, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert_eq!(track, 0);
        assert!((resolved.velocity - 0.25).abs() < 1e-6, "latched velocity plays");
        assert!((resolved.duration - 3.0).abs() < 1e-6, "latched duration plays");
        assert_eq!(resolved.transpose, 5.0, "untouched transpose keeps the step value");

        // Disarmed (or armed for another track): the step plays as stored.
        state.step_print_override.clear();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
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
        let scheduled = queue.pop().expect("step 0 trigger after disarm");
        let ScheduledEventKind::ResolvedTrigger { resolved, .. } = scheduled.kind else {
            panic!("expected resolved trigger");
        };
        assert!((resolved.velocity - 0.7).abs() < 1e-6, "disarm restores stored values");
    }

    /// Chord-backed steps own their sounding durations per note, which beat
    /// `resolved.duration` at fire time — so the print substitution must
    /// carry the duration delta onto the scheduled chord (mirroring the
    /// write-behind stamp's `set_step_param_no_publish` chord move), or a
    /// duration print on a recorded step is only heard one loop later.
    #[test]
    fn scheduler_lookahead_shifts_chord_durations_for_armed_step_print() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Duration, 2.0);
        state.pattern.chord_data[0].add_note_with_duration(0, 0.0, 2.0);
        state.pattern.chord_data[0].add_note_with_duration(0, 7.0, 1.0);
        state.transport.playing.store(true, Ordering::Relaxed);
        // Latch duration 3.0: delta over the stored base (2.0) is +1.0, so
        // the chord's explicit per-note durations move to 3.0 and 2.0.
        state.step_print_override.set(0, None, Some(3.0), None);

        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;

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

        let scheduled = queue.pop().expect("chord step trigger");
        let ScheduledEventKind::ResolvedTrigger {
            resolved, chord, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert!((resolved.duration - 3.0).abs() < 1e-6);
        assert_eq!(chord.count, 2);
        assert!(
            (chord.durations[0] - 3.0).abs() < 1e-6,
            "first chord note takes the stamp's delta (+1.0)"
        );
        assert!(
            (chord.durations[1] - 2.0).abs() < 1e-6,
            "relative chord durations are preserved, matching the stamp"
        );
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
    fn scheduler_runtime_imports_jaki_package_without_source_injection() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = super::build_scheduler_scratch_runtime(
            state,
            r#"(import alez.jaki.surface :refer (jak))
               (jak "package-import" :16 . . - . -> 0)"#,
            false,
        )
        .expect("imported Jaki sequencer should keep the scheduler runtime alive");

        assert!(
            runtime
                .sequencer_defs()
                .iter()
                .any(|definition| definition.name == "package-import"),
            "the scheduler VM must compile the imported macro itself"
        );
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

    /// docs/jaki-live-channel-widgets-spec.md 7 and 8.1: control-thread
    /// channel writes reach `chan-get` on the next chunk, while a process write
    /// is mirrored back through the same channel handle for inline UI polling.
    #[test]
    fn channel_writes_cross_the_scheduler_boundary_in_both_directions() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"(def warp (defchan warp 0.25))
                       (__register-sequencer "warp-reader"
                         :resolution :4
                         :tick (lambda () (seq-emit :track 0 :at :now :vel (chan-get "warp" 0.25))))
                       (def drive (defchan drive 0))
                       (tap drive (lambda (value) (send warp 0.8)))"#,
                )
                .expect("declare channel and generator");

            let velocities = |scheduler: &mut SchedulerLookaheadState,
                              scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
                              rendered: u64| {
                let snapshot = state.publish_scheduler_snapshot();
                let queue = ScheduledEventQueue::<32>::new();
                let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                    std::array::from_fn(|_| LiveMidiFxTrackState::default());
                schedule_playing_lookahead(
                    scheduler,
                    &state,
                    &snapshot,
                    &queue,
                    scratch_runtime,
                    &live_midi_fx_tracks,
                    snapshot.transport.pattern_epoch,
                    rendered,
                    rendered + 24_000,
                    48_000,
                    6_000,
                    24_000.0,
                    rendered,
                    false,
                    false,
                );
                let mut out = Vec::new();
                while let Some(event) = queue.pop() {
                    if let ScheduledEventKind::NetworkTrigger { resolved, .. } = event.kind {
                        out.push(resolved.velocity);
                    }
                }
                out
            };

            state.transport.playing.store(true, Ordering::Relaxed);
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            scheduler
                .generator_runtime
                .sync_definitions(&scratch.sequencer_defs(), 0.0);
            scheduler
                .process_runtime
                .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
            let mut scratch_runtime = Some(scratch);

            let before = velocities(&mut scheduler, &mut scratch_runtime, 0);
            assert!(!before.is_empty(), "generator emitted nothing");
            assert!(
                before.iter().all(|vel| (vel - 0.25).abs() < 1e-6),
                "expected the defchan initial before any write, got {before:?}"
            );

            // The stubbed handle used to swallow this call entirely.
            let handle = scratch_runtime
                .as_mut()
                .expect("scratch runtime")
                .eval("(warp :set 0.9)")
                .expect("write the channel through its handle");
            assert_eq!(handle, Some(Value::Bool(true)));

            let after = velocities(&mut scheduler, &mut scratch_runtime, 24_000);
            assert!(!after.is_empty(), "generator emitted nothing after the write");
            assert!(
                after.iter().all(|vel| (vel - 0.9).abs() < 1e-6),
                "expected the written channel value, got {after:?}"
            );
            assert!(
                state.take_process_channel_writes().is_empty(),
                "the scheduler drain should have consumed the queue"
            );

            scratch_runtime
                .as_mut()
                .expect("scratch runtime")
                .eval("(drive :set 1)")
                .expect("wake the process that contends with the UI write");
            let driven = velocities(&mut scheduler, &mut scratch_runtime, 48_000);
            assert!(
                !driven.is_empty(),
                "generator emitted nothing after process write"
            );
            assert!(
                driven.iter().all(|vel| (vel - 0.8).abs() < 1e-6),
                "expected the process-driven channel value, got {driven:?}"
            );
            assert_eq!(
                scratch_runtime
                    .as_mut()
                    .expect("scratch runtime")
                    .eval("(warp :__inline-read :set)")
                    .expect("poll contended channel mirror"),
                Some(Value::Number(0.8)),
                "the process's later write should visibly win over the UI echo"
            );
        });
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
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-project-performance-lanes-demo.lisp");
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
    #[ignore = "eseq-4tl: manual release-mode performance profile"]
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
            let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase7-reads-demo.lisp");
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
            let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-conductor-demo.lisp");
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
            let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase3a-ports-demo.lisp");
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
                        neural_group: None,
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
                    group_gain: None,
                    group_coupling: None,
                    group_trace_decay: None,
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

    // eseq-ur4: a stale/legacy pool copy of an effect slot can carry an empty
    // `param_node_indices` while still reporting params. The old positional
    // fallback then treated descriptor param N as node param N — and Space
    // Echo's node param 16 is its sample-rate state slot, not a param, so a
    // knob-scale value landed there and the audio worker aborted on the next
    // block. An unknown layout must produce no params at all.
    #[test]
    fn effect_slot_without_param_layout_pushes_nothing() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor {
            name: "space echo-ish".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![ParamDescriptor {
                name: "wow".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                // Descriptor param 16 maps to node param 20; node param 16 is
                // the device's sample-rate state slot.
                node_param_idx: 20,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }],
        };
        state.pattern.effect_chains[track][0].apply_descriptor_with_modulator(&desc, 42, 77);
        state.pattern.effect_chains[track][0].set_plock(step, 0, 0.75);

        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let stale_track = Arc::make_mut(&mut snapshot.tracks[track]);
        stale_track.effect_slots[0].param_node_indices.clear();

        assert!(resolve_effect_params(&snapshot, track, step).is_empty());
        assert!(resolve_effect_defaults(&snapshot, track).is_empty());
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

    // ------------------------------------------------------------------
    // Song mode: deterministic scheduler tests (docs/song-mode-spec.md 14.3)
    // ------------------------------------------------------------------

    fn song_mode_configure_pattern(data: &mut crate::sequencer::TrackPatternData, transpose: f32) {
        data.track_params.num_steps = 16;
        for step in 0..16 {
            data.track_bits[step / 64] |= 1 << (step % 64);
            data.step_data[step][StepParam::Transpose.index()] = transpose;
        }
    }

    /// Two tracks, three scenes. Scene `s` resolves every track to a fully
    /// active 16-step pattern with transpose `s + 1`, so an observed trigger
    /// identifies which row's snapshot scheduled it. Track 0 additionally has
    /// an extra pool pattern (transpose 9) used as a row override target;
    /// its id is returned.
    fn song_mode_fixture() -> (Arc<SequencerState>, crate::sequencer::PatternId) {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let override_id = state.with_scenes_mut(|scenes| {
            let snapshots = vec![
                crate::sequencer::PatternSnapshot::new_default(2, &[]),
                crate::sequencer::PatternSnapshot::new_default(2, &[]),
                crate::sequencer::PatternSnapshot::new_default(2, &[]),
            ];
            *scenes = crate::sequencer::ProjectScenes::from_pattern_snapshots(&snapshots, 0);
            for scene in 0..3 {
                for track in 0..2 {
                    let id = scenes.scenes[scene].cells[track].expect("scene cell");
                    assert!(scenes.track_pools[track].edit(id, |data| {
                        song_mode_configure_pattern(data, (scene + 1) as f32);
                    }));
                }
            }
            let mut extra = scenes.track_pools[0]
                .get(crate::sequencer::PatternId(1))
                .expect("source pattern")
                .clone();
            song_mode_configure_pattern(&mut extra, 9.0);
            scenes.track_pools[0].insert(extra)
        });
        (state, override_id)
    }

    fn song_mode_row(
        id: u64,
        start_beat: f64,
        scene: usize,
        overrides: Vec<(usize, u64)>,
    ) -> crate::sequencer::ProjectSongRow {
        crate::sequencer::ProjectSongRow {
            id: crate::sequencer::SongRowId(id),
            start_beat,
            scene: Some(scene),
            overrides: overrides
                .into_iter()
                .map(|(track, pattern_id)| crate::sequencer::ProjectSongTrackOverride::new(track, Some(pattern_id)))
                .collect(),
        }
    }

    fn song_mode_commit(
        state: &SequencerState,
        rows: Vec<crate::sequencer::ProjectSongRow>,
        end_beat: f64,
        loop_enabled: bool,
    ) {
        let next_row_id = rows.iter().map(|row| row.id.0 + 1).max().unwrap_or(0);
        state.set_committed_song(Some(crate::sequencer::ProjectSong {
            rows,
            end_beat,
            loop_enabled,
            next_row_id,
        }));
    }

    /// Run the production lookahead pass in song mode from sample 0 and
    /// return the observed triggers plus the scheduler-authoritative song
    /// notices. `samples_per_quarter` is 24_000 (48 kHz at the default
    /// 120 BPM), so one default 16th step is 6_000 samples.
    fn drive_song_lookahead(
        state: &Arc<SequencerState>,
        runtime: Arc<crate::sequencer::RuntimeSong>,
        block: usize,
        lookahead: u64,
    ) -> (
        Vec<ObservedTrigger>,
        Vec<crate::sequencer::SongPlaybackNotice>,
    ) {
        state.transport.playing.store(true, Ordering::Relaxed);
        let base = state.publish_scheduler_snapshot();
        let samples_per_quarter = 48_000.0 * 60.0 / base.transport.bpm as f64;
        // Heap-allocated: the inline event slots are too large for the
        // harness thread stack alongside the lookahead pass itself.
        let queue = Box::new(ScheduledEventQueue::<128>::new());
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler.song = Some(
            crate::sequencer::SongPlaybackRuntime::new(runtime, 0.0, samples_per_quarter)
                .expect("song playback runtime"),
        );
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;
        schedule_playing_lookahead(
            &mut scheduler,
            state,
            &base,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            base.transport.pattern_epoch,
            0,
            lookahead,
            48_000,
            block,
            samples_per_quarter,
            0,
            false,
            false,
        );
        (
            observed_triggers(&queue),
            state.drain_song_playback_notices(),
        )
    }

    fn song_row_applied(
        notices: &[crate::sequencer::SongPlaybackNotice],
    ) -> Vec<crate::sequencer::AudibleSongRowApplied> {
        notices
            .iter()
            .filter_map(|notice| match notice {
                crate::sequencer::SongPlaybackNotice::RowApplied(applied) => Some(*applied),
                _ => None,
            })
            .collect()
    }

    /// Row ordinal governing `sample` per the scheduler's own application
    /// records (the latest record at or before the sample).
    fn song_row_at_sample(
        applied: &[crate::sequencer::AudibleSongRowApplied],
        sample: u64,
    ) -> usize {
        applied
            .iter()
            .filter(|record| record.effective_sample <= sample)
            .next_back()
            .expect("a row must govern every scheduled sample")
            .row_ordinal
    }

    fn start_runtime_at_beat(
        song: Arc<crate::sequencer::RuntimeSong>,
        beat: f64,
        mailbox: &crate::sequencer::SongPlaybackMailbox,
    ) -> crate::sequencer::SongPlaybackRuntime {
        let samples_per_quarter = 24_000.0;
        let mut runtime =
            crate::sequencer::SongPlaybackRuntime::new(song, 0.0, samples_per_quarter)
                .expect("song playback runtime");
        let plan = runtime.next_chunk(
            (beat * samples_per_quarter) as u64,
            beat,
            samples_per_quarter as usize,
            mailbox,
        );
        assert!(matches!(
            plan,
            crate::sequencer::SongChunkPlan::Schedule { .. }
        ));
        mailbox.drain_notices();
        runtime
    }

    /// User repro (unified-transport rev 3): a lane silent for a gap row
    /// re-enters on the next row with a note on step 1 — that boundary
    /// trigger must fire, on the first pass, at the boundary sample.
    #[test]
    fn row_boundary_step_zero_trigger_fires_when_a_lane_re_enters() {
        std::thread::Builder::new()
            .name("row-boundary-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(row_boundary_step_zero_trigger_body)
            .expect("harness thread")
            .join()
            .expect("harness thread joins");
    }

    fn row_boundary_step_zero_trigger_body() {
        let (state, _) = song_mode_fixture();
        // Row 0 [0, 4): track 0 explicitly empty (the gap), track 1 plays.
        // Row 1 [4, 8): scene 1 — every step active on both tracks.
        let mut gap_row = song_mode_row(10, 0.0, 0, Vec::new());
        gap_row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new(0, None)];
        song_mode_commit(
            &state,
            vec![gap_row, song_mode_row(11, 4.0, 1, Vec::new())],
            8.0,
            false,
        );
        let runtime = state.preflight_runtime_song().expect("preflight");
        // 8 beats at 24_000 samples/quarter = 192_000 samples; one default
        // 16th step is 6_000 samples. Row 1 starts at sample 96_000.
        let (triggers, _notices) = drive_song_lookahead(&state, runtime, 128, 192_000);
        let track0: Vec<_> = triggers
            .iter()
            .filter(|trigger| trigger.track == 0)
            .collect();
        assert!(
            !track0.is_empty(),
            "track 0 must sound once its row re-enters"
        );
        let first = track0[0];
        // The boundary split floors fractional samples, so the boundary
        // chunk can start one sample early — the trigger may land at
        // boundary-1. Anything later than a couple hundred samples is a
        // missed downbeat.
        assert!(
            first.sample_time + 2 >= 96_000 && first.sample_time < 96_000 + 256,
            "the re-entry row's STEP 1 must fire at the boundary (sample \
             96000), got sample {}",
            first.sample_time
        );
        // Every step of the 16-step row must fire across [4, 8): 16 steps.
        assert_eq!(
            track0.len(),
            16,
            "all 16 steps of the re-entry row must sound on the first pass"
        );
    }

    /// The fractional variant — captured arrangements stamp UNQUANTIZED row
    /// boundaries. A gap running a hair past an exact cycle multiple wraps
    /// the silenced lane's clock into step 0 just before the boundary
    /// (trigger suppressed, dedup memory updated), and without the
    /// source-change step-memory reset the re-entry row's step-0 downbeat
    /// is swallowed — the user's "drums skip the first kick" bug.
    #[test]
    fn fractional_row_boundary_does_not_swallow_the_downbeat() {
        std::thread::Builder::new()
            .name("fractional-row-boundary-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(fractional_row_boundary_body)
            .expect("harness thread")
            .join()
            .expect("harness thread joins");
    }

    fn fractional_row_boundary_body() {
        let (state, _) = song_mode_fixture();
        // The gap row spans [0, 4.003): one full 4-beat cycle plus a hair,
        // so the silenced lane derives step 0 again just before the
        // boundary. Row 1 re-enters at the fractional beat 4.003.
        let mut gap_row = song_mode_row(10, 0.0, 0, Vec::new());
        gap_row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new(0, None)];
        song_mode_commit(
            &state,
            vec![gap_row, song_mode_row(11, 4.003, 1, Vec::new())],
            8.0,
            false,
        );
        let runtime = state.preflight_runtime_song().expect("preflight");
        let boundary_sample = (4.003 * 24_000.0) as u64;
        let (triggers, _notices) = drive_song_lookahead(&state, runtime, 128, 192_000);
        let track0: Vec<_> = triggers
            .iter()
            .filter(|trigger| trigger.track == 0)
            .collect();
        assert!(!track0.is_empty(), "track 0 must sound after the boundary");
        let first = track0[0];
        assert!(
            first.sample_time + 2 >= boundary_sample
                && first.sample_time < boundary_sample + 256,
            "the fractional re-entry's step-0 downbeat must fire at the \
             boundary (sample {boundary_sample}), got sample {}",
            first.sample_time
        );
    }

    #[test]
    fn song_rebuild_remaps_from_clock_beat_and_preserves_the_sounding_anchor() {
        let (state, _) = song_mode_fixture();
        song_mode_commit(
            &state,
            vec![
                song_mode_row(10, 0.0, 0, Vec::new()),
                song_mode_row(11, 4.0, 1, Vec::new()),
                song_mode_row(12, 8.0, 2, Vec::new()),
            ],
            12.0,
            false,
        );
        let original = state.preflight_runtime_song().expect("preflight original");
        let mut runtime = start_runtime_at_beat(
            Arc::clone(&original),
            5.0,
            state.song_playback(),
        );
        assert_eq!(runtime.current_row(), 1);
        assert!((runtime.row_clock_anchor(1).0 - 4.0).abs() < 1e-9);

        // Move the governing row's start without changing what it resolves.
        // A row-index-based rebuild would either retain ordinal 1 by accident
        // or move its phase anchor to beat 2.
        song_mode_commit(
            &state,
            vec![
                song_mode_row(20, 0.0, 0, Vec::new()),
                song_mode_row(21, 2.0, 1, Vec::new()),
                song_mode_row(22, 7.0, 2, Vec::new()),
            ],
            12.0,
            false,
        );
        let rebuilt = state.preflight_runtime_song().expect("preflight rebuild");
        runtime.rebuild_song(Arc::clone(&rebuilt), 5.0);

        assert!(
            Arc::ptr_eq(runtime.song(), &rebuilt),
            "identical sounding source/offset identity swaps immediately"
        );
        assert_eq!(
            runtime.current_row(),
            rebuilt.row_index_at_beat(5.0).unwrap(),
            "the cursor is selected from the runtime clock beat"
        );
        assert!(
            (runtime.row_clock_anchor(runtime.current_row()).0 - 4.0).abs() < 1e-9,
            "an immediate remap must not move the sounding phase anchor"
        );
        assert!(
            state.song_playback().drain_notices().is_empty(),
            "an immediate identity-compatible remap never re-enters the row"
        );
    }

    #[test]
    fn song_rebuild_with_a_changed_current_row_waits_for_the_next_boundary() {
        let (state, _) = song_mode_fixture();
        song_mode_commit(
            &state,
            vec![
                song_mode_row(10, 0.0, 0, Vec::new()),
                song_mode_row(11, 4.0, 1, Vec::new()),
                song_mode_row(12, 8.0, 2, Vec::new()),
            ],
            12.0,
            false,
        );
        let original = state.preflight_runtime_song().expect("preflight original");
        let mut runtime = start_runtime_at_beat(
            Arc::clone(&original),
            5.0,
            state.song_playback(),
        );

        song_mode_commit(
            &state,
            vec![
                song_mode_row(20, 0.0, 0, Vec::new()),
                song_mode_row(21, 3.0, 2, Vec::new()),
                song_mode_row(22, 10.0, 1, Vec::new()),
            ],
            12.0,
            false,
        );
        let rebuilt = state.preflight_runtime_song().expect("preflight rebuild");
        runtime.rebuild_song(Arc::clone(&rebuilt), 5.0);
        assert!(
            Arc::ptr_eq(runtime.song(), &original),
            "changed sounding identity stays on the old immutable song"
        );

        let before_boundary = runtime.next_chunk(
            7 * 24_000,
            7.0,
            24_000,
            state.song_playback(),
        );
        assert!(matches!(
            before_boundary,
            crate::sequencer::SongChunkPlan::Schedule { row: 1, .. }
        ));
        assert!(Arc::ptr_eq(runtime.song(), &original));
        assert!(
            state.song_playback().drain_notices().is_empty(),
            "the rebuilt row must not apply before the old row boundary"
        );

        let at_boundary = runtime.next_chunk(
            8 * 24_000,
            8.0,
            24_000,
            state.song_playback(),
        );
        assert!(matches!(
            at_boundary,
            crate::sequencer::SongChunkPlan::Schedule {
                row_changed: true,
                ..
            }
        ));
        assert!(Arc::ptr_eq(runtime.song(), &rebuilt));
        assert_eq!(runtime.current_row(), rebuilt.row_index_at_beat(8.0).unwrap());
        let applied = song_row_applied(&state.song_playback().drain_notices());
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!((applied[0].effective_beat - 8.0).abs() < 1e-9);
        assert!(!applied[0].wrapped);
    }

    #[test]
    fn song_rebuild_past_the_new_end_uses_the_existing_end_and_loop_rules() {
        let (state, _) = song_mode_fixture();
        song_mode_commit(
            &state,
            vec![
                song_mode_row(10, 0.0, 0, Vec::new()),
                song_mode_row(11, 4.0, 1, Vec::new()),
            ],
            12.0,
            false,
        );
        let original = state.preflight_runtime_song().expect("preflight original");

        let exercise = |loop_enabled: bool| {
            let mut runtime = start_runtime_at_beat(
                Arc::clone(&original),
                5.0,
                state.song_playback(),
            );
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(20, 0.0, 0, Vec::new()),
                    song_mode_row(21, 2.0, 2, Vec::new()),
                ],
                4.0,
                loop_enabled,
            );
            let rebuilt = state.preflight_runtime_song().expect("preflight rebuild");
            runtime.rebuild_song(rebuilt, 5.0);
            let plan = runtime.next_chunk(
                5 * 24_000,
                5.0,
                24_000,
                state.song_playback(),
            );
            let notices = state.song_playback().drain_notices();
            (plan, notices)
        };

        let (ended, notices) = exercise(false);
        assert!(matches!(ended, crate::sequencer::SongChunkPlan::Ended));
        assert!(
            notices.iter().any(|notice| matches!(
                notice,
                crate::sequencer::SongPlaybackNotice::Ended { end_beat, .. }
                    if (*end_beat - 4.0).abs() < 1e-9
            )),
            "{notices:?}"
        );

        let (wrapped, notices) = exercise(true);
        assert!(matches!(
            wrapped,
            crate::sequencer::SongChunkPlan::Schedule {
                wrapped: true,
                ..
            }
        ));
        assert!(
            notices.iter().any(|notice| matches!(
                notice,
                crate::sequencer::SongPlaybackNotice::RowApplied(applied)
                    if applied.wrapped && applied.effective_beat == 0.0
            )),
            "{notices:?}"
        );
        assert!(
            !notices
                .iter()
                .any(|notice| matches!(notice, crate::sequencer::SongPlaybackNotice::Ended { .. }))
        );
    }

    #[test]
    fn song_row_boundary_inside_block_splits_scheduling() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.5, 1, Vec::new()),
                ],
                4.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 96_000);
            let applied = song_row_applied(&notices);
            assert_eq!(applied[0].row_ordinal, 0);
            assert_eq!(applied[0].effective_sample, 0);
            let row1 = applied
                .iter()
                .find(|record| record.row_ordinal == 1)
                .expect("row 1 applied");
            // Beat 1.5 at 24_000 samples per quarter: exactly where the
            // scheduler clock crosses beat 1.5, inside the [32_000, 48_000)
            // processing block — not at either block edge.
            assert!(
                (35_999..=36_000).contains(&row1.effective_sample),
                "{row1:?}"
            );
            assert!((row1.effective_beat - 1.5).abs() < 1e-9);
            assert!(!row1.wrapped);
            let boundary = row1.effective_sample;
            let track0: Vec<_> = events.iter().filter(|event| event.track == 0).collect();
            assert!(
                track0.iter().any(|event| event.sample_time < boundary),
                "expected pre-boundary steps: {track0:#?}"
            );
            assert!(
                track0.iter().any(|event| event.sample_time == boundary),
                "expected the coincident step exactly at the boundary: {track0:#?}"
            );
            for event in &track0 {
                let expected = if event.sample_time < boundary { 1.0 } else { 2.0 };
                assert_eq!(
                    event.transpose, expected,
                    "split at {boundary}: {event:?}"
                );
            }
        });
    }

    /// Repro for the arrtest3 "drone lane drops a loop at row boundaries"
    /// report: track 1 is an 8-step pattern with ONLY step 0 active (a
    /// half-bar clip whose trigger lands exactly on every even beat), pinned
    /// to the same pool pattern across rows whose start beats — including
    /// the exact fractional splice boundaries from the project file — often
    /// coincide with its step-0 crossing. Every 2-beat loop must schedule
    /// exactly one trigger, regardless of scheduler block size and of how
    /// the lookahead pass is paced.
    #[test]
    fn song_boundary_coincident_drone_trigger_never_drops() {
        {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            let drone_id = state.with_scenes_mut(|scenes| {
                let snapshots = vec![
                    crate::sequencer::PatternSnapshot::new_default(2, &[]),
                    crate::sequencer::PatternSnapshot::new_default(2, &[]),
                    crate::sequencer::PatternSnapshot::new_default(2, &[]),
                ];
                *scenes = crate::sequencer::ProjectScenes::from_pattern_snapshots(&snapshots, 0);
                for scene in 0..3 {
                    let id = scenes.scenes[scene].cells[0].expect("scene cell");
                    assert!(scenes.track_pools[0].edit(id, |data| {
                        song_mode_configure_pattern(data, (scene + 1) as f32);
                    }));
                    let id = scenes.scenes[scene].cells[1].expect("scene cell");
                    assert!(scenes.track_pools[1].edit(id, |data| {
                        data.track_params.num_steps = 8;
                        data.track_bits = [1, 0, 0, 0];
                        data.step_data[0][StepParam::Transpose.index()] = 7.0;
                        data.step_data[0][StepParam::Duration.index()] = 8.0;
                    }));
                }
                scenes.scenes[0].cells[1].expect("drone cell")
            });
            let drone = |offset_steps: f64| crate::sequencer::ProjectSongTrackOverride {
                track: 1,
                pattern_id: Some(drone_id.0),
                take_id: None,
                offset_steps,
            };
            // Row starts and drone offsets lifted verbatim from
            // tests/fixtures/projects/arrtest3.json (track 6), through beat 28.
            let rows = vec![
                song_mode_row(437, 0.0, 0, Vec::new()),
                {
                    let mut row = song_mode_row(481, 8.0, 0, Vec::new());
                    row.overrides = vec![drone(0.0)];
                    row
                },
                {
                    let mut row = song_mode_row(459, 9.994709105451298, 0, Vec::new());
                    row.overrides = vec![drone(7.978836421805191)];
                    row
                },
                {
                    let mut row = song_mode_row(476, 12.0, 0, Vec::new());
                    row.overrides = vec![drone(0.0)];
                    row
                },
                {
                    let mut row = song_mode_row(439, 13.055999999917828, 1, Vec::new());
                    row.overrides = vec![drone(4.223999999671314)];
                    row
                },
                {
                    let mut row = song_mode_row(475, 14.0, 1, Vec::new());
                    row.overrides = vec![drone(0.0)];
                    row
                },
                {
                    let mut row = song_mode_row(440, 14.266308332895033, 0, Vec::new());
                    row.overrides = vec![drone(1.0652333315801314)];
                    row
                },
                {
                    let mut row = song_mode_row(441, 14.393673416895028, 1, Vec::new());
                    row.overrides = vec![drone(1.5746936675801138)];
                    row
                },
                {
                    let mut row = song_mode_row(480, 16.0, 1, Vec::new());
                    row.overrides = vec![drone(0.0)];
                    row
                },
                {
                    let mut row = song_mode_row(442, 20.474304416895237, 1, Vec::new());
                    row.overrides = vec![drone(1.8972176675809465)];
                    row
                },
                {
                    let mut row = song_mode_row(478, 26.0, 1, Vec::new());
                    row.overrides = vec![drone(0.0)];
                    row
                },
            ];
            song_mode_commit(&state, rows, 28.0, false);
            let runtime = state.preflight_runtime_song().expect("preflight");
            state.transport.playing.store(true, Ordering::Relaxed);
            let base = state.publish_scheduler_snapshot();
            let samples_per_quarter = 48_000.0 * 60.0 / base.transport.bpm as f64;
            let end_sample = (28.0 * samples_per_quarter) as u64;

            for &block in &[128usize, 256, 480, 512, 1000, 16_000] {
                let state = Arc::clone(&state);
                let runtime = Arc::clone(&runtime);
                let base = Arc::clone(&base);
                let triggers: Vec<ObservedTrigger> = run_with_scheduler_stack(move || {
                    let queue = Box::new(ScheduledEventQueue::<128>::new());
                    let mut scheduler = SchedulerLookaheadState::new(48_000);
                    scheduler.song = Some(
                        crate::sequencer::SongPlaybackRuntime::new(
                            runtime,
                            0.0,
                            samples_per_quarter,
                        )
                        .expect("song playback runtime"),
                    );
                    let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                        std::array::from_fn(|_| LiveMidiFxTrackState::default());
                    let mut scratch_runtime = None;
                    // Incremental pacing like production: `rendered` creeps
                    // forward and every pass extends the horizon by at most
                    // the 4-block lookahead window.
                    let mut scheduled_until = 0_u64;
                    let mut rendered = 0_u64;
                    let mut triggers: Vec<ObservedTrigger> = Vec::new();
                    while rendered < end_sample + 48_000 {
                        let result = schedule_playing_lookahead(
                            &mut scheduler,
                            &state,
                            &base,
                            &queue,
                            &mut scratch_runtime,
                            &live_midi_fx_tracks,
                            base.transport.pattern_epoch,
                            rendered,
                            (block * 4) as u64,
                            48_000,
                            block,
                            samples_per_quarter,
                            scheduled_until,
                            false,
                            false,
                        );
                        scheduled_until = result.scheduled_until_sample;
                        triggers.extend(observed_triggers(queue.as_ref()));
                        rendered += block as u64;
                    }
                    let _ = state.drain_song_playback_notices();
                    triggers
                });
                let drone_hits: Vec<u64> = triggers
                    .iter()
                    .filter(|event| event.track == 1)
                    .map(|event| event.sample_time)
                    .collect();
                // One trigger per 2-beat loop: beats 0, 2, 4, ... 26.
                let expected: Vec<u64> = (0..14)
                    .map(|loop_idx| (loop_idx as f64 * 2.0 * samples_per_quarter) as u64)
                    .collect();
                assert_eq!(
                    drone_hits.len(),
                    expected.len(),
                    "block={block}: expected one drone trigger per loop, got {drone_hits:?}"
                );
                for (hit, want) in drone_hits.iter().zip(&expected) {
                    let delta = hit.abs_diff(*want);
                    assert!(
                        delta <= 2,
                        "block={block}: drone trigger at {hit} expected near {want} ({drone_hits:?})"
                    );
                }
            }
        }
    }

    /// Install a two-chunk take (256 + 40 = 296 steps, transposes 5/6) on
    /// track 0 and return its id. Chunks are MAX_STEPS-long 16th-timebase
    /// patterns, so one chunk spans 64 beats.
    fn song_mode_install_take(state: &SequencerState) -> crate::sequencer::TakeId {
        state.with_scenes_mut(|scenes| {
            let mut chunk = scenes.track_pools[0]
                .get(crate::sequencer::PatternId(1))
                .expect("source pattern")
                .clone();
            chunk.track_params.num_steps = crate::sequencer::MAX_STEPS;
            for step in 0..crate::sequencer::MAX_STEPS {
                chunk.track_bits[step / 64] |= 1 << (step % 64);
                chunk.step_data[step][StepParam::Transpose.index()] = 5.0;
            }
            let mut chunk_b = chunk.clone();
            for step in 0..crate::sequencer::MAX_STEPS {
                chunk_b.step_data[step][StepParam::Transpose.index()] = 6.0;
            }
            let chunk_a = scenes.track_pools[0].insert(chunk);
            let sound = scenes.track_pools[0].refs(chunk_a).expect("chunk refs");
            // Chunks share the take's one Patch/Mix pair (§16.4) — keep the
            // fixture on-model so entity-sweep double-application bugs are
            // observable here.
            let chunk_b = scenes.track_pools[0].insert_with_refs(chunk_b, sound);
            scenes.take_pools[0].insert(None, vec![chunk_a, chunk_b], 296, sound)
        })
    }

    /// Note edit-through (docs/realtime-arrangement-feedback-spec.md 5.1):
    /// a step edit to the pattern the SOUNDING row resolves is audible inside
    /// that row. `replace_song_in_place` swaps the row `Arc`s and the
    /// Arrangement capture is OPEN-ENDED (docs/song-mode-spec.md 7.4): the
    /// song end is not a stopping point while recording. A take that grooves
    /// past the old song length used to be cut off there and committed —
    /// `Ended` reaches `handle_song_playback_ended`, which for capture calls
    /// the stop-COMMIT. Open-ended, the last row simply keeps playing and
    /// the stop-commit extends `end_beat` to the Stop beat. Plain song
    /// playback still ends exactly as before.
    #[test]
    fn open_ended_capture_plays_past_the_song_end_instead_of_ending() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            // Two rows, song ending at beat 8, NOT looping.
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 4.0, 1, Vec::new()),
                ],
                8.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            let samples_per_quarter = 24_000.0;
            let mailbox = state.song_playback();

            // Drive `next_chunk` from beat zero out past the song end. One
            // beat per call, so the end boundary is crossed on the ninth.
            let drive = |open_ended: bool| -> (Vec<bool>, usize) {
                let mut playback = crate::sequencer::SongPlaybackRuntime::new(
                    Arc::clone(&runtime),
                    0.0,
                    samples_per_quarter,
                )
                .expect("song playback runtime");
                playback.set_open_ended(open_ended);
                let mut ended = Vec::new();
                for beat in 0..12 {
                    let plan = playback.next_chunk(
                        (beat as f64 * samples_per_quarter) as u64,
                        beat as f64,
                        samples_per_quarter as usize,
                        mailbox,
                    );
                    ended.push(matches!(plan, crate::sequencer::SongChunkPlan::Ended));
                }
                let end_notices = mailbox
                    .drain_notices()
                    .iter()
                    .filter(|notice| {
                        matches!(notice, crate::sequencer::SongPlaybackNotice::Ended { .. })
                    })
                    .count();
                (ended, end_notices)
            };

            let (ended, end_notices) = drive(true);
            assert!(
                ended.iter().all(|ended| !ended),
                "open-ended capture must never stop at the song end: {ended:?}"
            );
            assert_eq!(
                end_notices, 0,
                "an Ended notice is what tears the capture down and commits it"
            );

            let (ended, end_notices) = drive(false);
            assert!(
                ended.iter().any(|ended| *ended),
                "plain song playback still ends at the song end"
            );
            assert_eq!(end_notices, 1);
        });
    }

    /// lookahead reads `row_snapshot(row)` per chunk, so steps ahead of the
    /// playhead change while the row itself is never re-entered — no
    /// retrigger, no clock disturbance — and the edit survives the loop wrap.
    #[test]
    fn song_step_edit_reaches_the_sounding_row_without_re_entering_it() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            // One 8-beat row looping on itself: the 4-beat scene-0 pattern
            // (transpose 1) tiles twice per pass.
            song_mode_commit(&state, vec![song_mode_row(0, 0.0, 0, Vec::new())], 8.0, true);
            let runtime = state.preflight_runtime_song().expect("preflight");
            state.transport.playing.store(true, Ordering::Relaxed);
            let base = state.publish_scheduler_snapshot();
            let samples_per_quarter = 48_000.0 * 60.0 / base.transport.bpm as f64;
            let loop_samples = (8.0 * samples_per_quarter) as u64;

            let queue = Box::new(ScheduledEventQueue::<128>::new());
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            scheduler.song = Some(
                crate::sequencer::SongPlaybackRuntime::new(runtime, 0.0, samples_per_quarter)
                    .expect("song playback runtime"),
            );
            let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            let mut scratch_runtime = None;
            let block = 4_800_usize;
            let mut scheduled_until = 0_u64;
            let mut rendered = 0_u64;
            let mut triggers: Vec<ObservedTrigger> = Vec::new();
            let mut applied: Vec<crate::sequencer::AudibleSongRowApplied> = Vec::new();
            // The edit lands one beat in, while the row is sounding.
            let edit_at = samples_per_quarter as u64;
            let mut edited = false;
            let mut edit_horizon = 0_u64;

            while rendered < loop_samples * 2 {
                let result = schedule_playing_lookahead(
                    &mut scheduler,
                    &state,
                    &base,
                    &queue,
                    &mut scratch_runtime,
                    &live_midi_fx_tracks,
                    base.transport.pattern_epoch,
                    rendered,
                    (block * 4) as u64,
                    48_000,
                    block,
                    samples_per_quarter,
                    scheduled_until,
                    false,
                    false,
                );
                scheduled_until = result.scheduled_until_sample;
                triggers.extend(observed_triggers(queue.as_ref()));
                applied.extend(song_row_applied(&state.drain_song_playback_notices()));
                rendered += block as u64;

                if !edited && rendered >= edit_at {
                    // The performer edits the pool pattern the sounding row
                    // resolved, then the control thread re-preflights and
                    // hands the rows over — the production `Refresh` path.
                    state.with_scenes_mut(|scenes| {
                        let id = scenes.scenes[0].cells[0].expect("scene 0 cell");
                        assert!(scenes.track_pools[0].edit(id, |data| {
                            for step in 0..16 {
                                data.step_data[step][StepParam::Transpose.index()] = 7.0;
                            }
                        }));
                    });
                    let refreshed = state.preflight_runtime_song().expect("re-preflight");
                    assert!(
                        scheduler
                            .song
                            .as_mut()
                            .expect("song runtime")
                            .replace_song_in_place(refreshed),
                        "a note edit keeps row layout identical, so the swap must succeed"
                    );
                    edited = true;
                    // Everything already scheduled keeps the old content.
                    edit_horizon = scheduled_until;
                }
            }

            let track0: Vec<&ObservedTrigger> =
                triggers.iter().filter(|event| event.track == 0).collect();
            assert!(
                track0
                    .iter()
                    .filter(|event| event.sample_time < edit_horizon)
                    .all(|event| event.transpose == 1.0),
                "already-scheduled steps must not be rewritten: {track0:#?}"
            );
            let after: Vec<&&ObservedTrigger> = track0
                .iter()
                .filter(|event| event.sample_time >= edit_horizon)
                .collect();
            assert!(
                !after.is_empty() && after.iter().all(|event| event.transpose == 7.0),
                "steps ahead of the playhead in the SOUNDING row carry the edit: {track0:#?}"
            );
            assert!(
                after
                    .iter()
                    .any(|event| event.sample_time >= loop_samples),
                "the edit must survive the loop wrap: {track0:#?}"
            );

            // The row is never re-entered off a boundary: only the initial
            // application and the loop wraps apply it, all at beat zero.
            assert!(
                applied.iter().all(|record| record.row_ordinal == 0
                    && record.effective_beat.abs() < 1e-9),
                "no retrigger: rows apply only at their own start ({applied:?})"
            );
            let wraps = applied.iter().filter(|record| record.wrapped).count();
            assert_eq!(
                applied.len(),
                wraps + 1,
                "one initial application plus one per wrap ({applied:?})"
            );
        });
    }

    #[test]
    fn song_rebuild_ahead_changes_future_playback_without_reentering_the_sounding_row() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(10, 0.0, 0, Vec::new()),
                    song_mode_row(11, 8.0, 1, Vec::new()),
                ],
                12.0,
                false,
            );
            let original = state.preflight_runtime_song().expect("preflight original");
            state.transport.playing.store(true, Ordering::Relaxed);
            let base = state.publish_scheduler_snapshot();
            let samples_per_quarter = 48_000.0 * 60.0 / base.transport.bpm as f64;
            let queue = Box::new(ScheduledEventQueue::<128>::new());
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            scheduler.song = Some(
                crate::sequencer::SongPlaybackRuntime::new(
                    original,
                    0.0,
                    samples_per_quarter,
                )
                .expect("song playback runtime"),
            );
            let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            let mut scratch_runtime = None;
            let block = 4_800_usize;
            let mut scheduled_until = 0_u64;
            let mut rendered = 0_u64;
            let mut triggers = Vec::new();
            let mut applied = Vec::new();
            let edit_at = samples_per_quarter as u64;
            let end_sample = (12.0 * samples_per_quarter) as u64;
            let mut edited = false;

            while rendered < end_sample {
                let result = schedule_playing_lookahead(
                    &mut scheduler,
                    &state,
                    &base,
                    &queue,
                    &mut scratch_runtime,
                    &live_midi_fx_tracks,
                    base.transport.pattern_epoch,
                    rendered,
                    (block * 4) as u64,
                    48_000,
                    block,
                    samples_per_quarter,
                    scheduled_until,
                    false,
                    false,
                );
                scheduled_until = result.scheduled_until_sample;
                triggers.extend(observed_triggers(queue.as_ref()));
                applied.extend(song_row_applied(&state.drain_song_playback_notices()));
                rendered += block as u64;

                if !edited && rendered >= edit_at {
                    // The edit is seven beats ahead and outside the current
                    // lookahead horizon. Scene 2 replaces scene 1 there.
                    song_mode_commit(
                        &state,
                        vec![
                            song_mode_row(20, 0.0, 0, Vec::new()),
                            song_mode_row(21, 8.0, 2, Vec::new()),
                        ],
                        12.0,
                        false,
                    );
                    let rebuilt = state.preflight_runtime_song().expect("preflight rebuild");
                    let clock_beat = scheduler.clock.total_beats;
                    scheduler
                        .song
                        .as_mut()
                        .expect("song runtime")
                        .rebuild_song(rebuilt, clock_beat);
                    edited = true;
                }
            }

            let after_boundary: Vec<_> = triggers
                .iter()
                .filter(|event| {
                    event.track == 0
                        && event.sample_time >= (8.0 * samples_per_quarter) as u64
                })
                .collect();
            assert!(
                !after_boundary.is_empty()
                    && after_boundary
                        .iter()
                        .all(|event| event.transpose == 3.0),
                "the edit ahead must govern when reached: {after_boundary:#?}"
            );
            let sounding_row_entries: Vec<_> = applied
                .iter()
                .filter(|notice| notice.effective_beat < 8.0)
                .collect();
            assert_eq!(
                sounding_row_entries.len(),
                1,
                "the sounding row is entered once at Start, never by Rebuild: {applied:?}"
            );
            assert!(sounding_row_entries[0].effective_beat.abs() < 1e-9);
            assert!(
                applied
                    .iter()
                    .any(|notice| (notice.effective_beat - 8.0).abs() < 1e-9),
                "the rebuilt future row applies at its ordinary boundary: {applied:?}"
            );
        });
    }

    #[test]
    fn preflight_expands_take_rows_at_chunk_boundaries_and_take_end() {
        let (state, _) = song_mode_fixture();
        let take = song_mode_install_take(&state);
        let (chunk_a, chunk_b) = state.with_scenes_mut(|scenes| {
            let take = scenes.take_pools[0].get(take).expect("take");
            (take.chunks[0], take.chunks[1])
        });
        let mut row = song_mode_row(0, 0.0, 0, Vec::new());
        row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new_take(
            0,
            take.0,
            0.0,
        )];
        song_mode_commit(&state, vec![row], 100.0, false);
        let runtime = state.preflight_runtime_song().expect("preflight");

        // One project row expands to three runtime rows: chunk 0 at beat 0,
        // chunk 1 at beat 64 (256 steps of a 16th timebase), and the silent
        // tail at beat 74 (296 steps). All share the project row's id.
        assert_eq!(runtime.rows.len(), 3);
        assert!(runtime
            .rows
            .iter()
            .all(|row| row.id == crate::sequencer::SongRowId(0)));
        let starts: Vec<f64> = runtime.rows.iter().map(|row| row.start_beat).collect();
        assert!((starts[0] - 0.0).abs() < 1e-9, "{starts:?}");
        assert!((starts[1] - 64.0).abs() < 1e-9, "{starts:?}");
        assert!((starts[2] - 74.0).abs() < 1e-9, "{starts:?}");

        // Take lane: content is the governing chunk, identity is the TakeId,
        // chunk-local offsets restart at 0, and the tail is silent (no wrap).
        assert_eq!(runtime.rows[0].resolved_pattern_ids[0], Some(chunk_a));
        assert_eq!(runtime.rows[1].resolved_pattern_ids[0], Some(chunk_b));
        assert_eq!(runtime.rows[2].resolved_pattern_ids[0], None);
        assert_eq!(
            runtime.rows[0].resolved_sources[0],
            crate::sequencer::LaneSource::Take(take)
        );
        assert_eq!(
            runtime.rows[1].resolved_sources[0],
            crate::sequencer::LaneSource::Take(take)
        );
        assert_eq!(
            runtime.rows[2].resolved_sources[0],
            crate::sequencer::LaneSource::Empty
        );
        assert!((runtime.rows[0].lane_offsets[0] - 0.0).abs() < 1e-6);
        assert!((runtime.rows[1].lane_offsets[0] - 0.0).abs() < 1e-6);

        // The scene-resolved lane on track 1 stays phase-continuous across
        // the synthetic splits: 64 beats = 256 steps ≡ 0 (mod 16), 74 beats
        // = 296 steps ≡ 8 (mod 16).
        assert!((runtime.rows[1].lane_offsets[1] - 0.0).abs() < 1e-6);
        assert!((runtime.rows[2].lane_offsets[1] - 8.0).abs() < 1e-6);
        assert_eq!(
            runtime.rows[1].resolved_sources[1],
            crate::sequencer::LaneSource::Pattern(crate::sequencer::PatternId(1))
        );

        // Chunk boundaries are NOT a source change: no accumulator reset for
        // the take lane (or anyone else). The take end IS one for the lane.
        let mut resets = [false; MAX_TRACKS];
        crate::scheduler::mark_song_row_accum_resets(&runtime.rows[0], &runtime.rows[1], &mut resets);
        assert!(resets.iter().all(|reset| !reset), "{resets:?}");
        crate::scheduler::mark_song_row_accum_resets(&runtime.rows[1], &runtime.rows[2], &mut resets);
        assert!(resets[0], "take end silences the lane -> reset");
        assert!(!resets[1], "unrelated lane keeps its accumulator");
    }

    #[test]
    fn preflight_take_offset_resolves_mid_chunk_and_clips_short_spans() {
        let (state, _) = song_mode_fixture();
        let take = song_mode_install_take(&state);
        // Row 0 plays scenes; row 1 at beat 10 re-enters the take at step
        // 12.5; row 2 at beat 20 returns to the scene — the take span never
        // reaches a chunk boundary, so no synthetic split is inserted.
        let mut take_row = song_mode_row(1, 10.0, 0, Vec::new());
        take_row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new_take(
            0,
            take.0,
            12.5,
        )];
        song_mode_commit(
            &state,
            vec![
                song_mode_row(0, 0.0, 0, Vec::new()),
                take_row,
                song_mode_row(2, 20.0, 1, Vec::new()),
            ],
            32.0,
            false,
        );
        let runtime = state.preflight_runtime_song().expect("preflight");
        assert_eq!(runtime.rows.len(), 3, "no synthetic splits");
        assert_eq!(
            runtime.rows[1].resolved_sources[0],
            crate::sequencer::LaneSource::Take(take)
        );
        assert!((runtime.rows[1].lane_offsets[0] - 12.5).abs() < 1e-6);
    }

    #[test]
    fn manual_latch_schedules_track_from_live_snapshot_until_cleared() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            // Live session state: track 0 plays a distinct pattern
            // (transpose 9). Row content is scene 1 (transpose 2).
            state
                .apply_song_row(0, &[], 2, &[], &[], &[], &[], true)
                .expect("seed live state from scene 0");
            let live = state.with_scenes_mut(|scenes| {
                scenes.track_pools[0]
                    .get(crate::sequencer::PatternId(4))
                    .expect("override pool pattern")
                    .clone()
            });
            let mut live_snapshot = crate::sequencer::PatternSnapshot::new_default(2, &[]);
            live_snapshot.set_track_pattern_data(0, live);
            assert!(live_snapshot.restore_track(&state, 0));
            song_mode_commit(
                &state,
                vec![song_mode_row(0, 0.0, 1, Vec::new())],
                4.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");

            // Latched: track 0 schedules from the LIVE snapshot (transpose
            // 9, free-running), track 1 from the song row (transpose 2).
            state.latch_song_manual_override([0]);
            let (events, _) =
                drive_song_lookahead(&state, Arc::clone(&runtime), 16_000, 48_000);
            assert!(!events.is_empty());
            for event in &events {
                let expected = if event.track == 0 { 9.0 } else { 2.0 };
                assert_eq!(
                    event.transpose, expected,
                    "latched track plays the live pattern: {event:?}"
                );
            }

            // Back to Song: the row's content resumes for track 0.
            state.clear_song_manual_latch();
            let (events, _) = drive_song_lookahead(&state, runtime, 16_000, 48_000);
            assert!(events.iter().any(|event| event.track == 0));
            for event in &events {
                assert_eq!(
                    event.transpose, 2.0,
                    "cleared latch restores song resolution: {event:?}"
                );
            }
        });
    }

    /// A track added while the song plays is invisible to the preflighted
    /// row snapshots (frozen at Play). The lookahead merge must append the
    /// live lane so the clock steps it: without that, the new track neither
    /// triggers nor publishes a playhead until the next transport start.
    #[test]
    fn track_added_after_preflight_schedules_and_publishes_its_playhead() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![song_mode_row(0, 0.0, 1, Vec::new())],
                4.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");

            // Mid-play add: the live state grows to 3 tracks AFTER preflight
            // froze 2-track rows; the add path latches the new lane
            // (`latch_track_created_during_song_playback`).
            let mut live = state.with_scenes_mut(|scenes| {
                scenes.track_pools[0]
                    .get(crate::sequencer::PatternId(1))
                    .expect("pool pattern")
                    .clone()
            });
            song_mode_configure_pattern(&mut live, 9.0);
            let mut live_snapshot = crate::sequencer::PatternSnapshot::new_default(3, &[]);
            live_snapshot.set_track_pattern_data(2, live);
            assert!(live_snapshot.restore_track(&state, 2));
            state.transport.num_tracks.store(3, Ordering::Release);
            state.latch_song_manual_override([2]);
            state.transport.track_playheads[2].store(u32::MAX, Ordering::Relaxed);

            let (events, _) =
                drive_song_lookahead(&state, Arc::clone(&runtime), 16_000, 48_000);
            assert!(
                events
                    .iter()
                    .any(|event| event.track == 2 && event.transpose == 9.0),
                "the added track schedules from its live lane: {events:?}"
            );
            for event in &events {
                if event.track < 2 {
                    assert_eq!(
                        event.transpose, 2.0,
                        "row-governed lanes keep the song's content: {event:?}"
                    );
                }
            }
            assert_ne!(
                state.transport.track_playheads[2].load(Ordering::Relaxed),
                u32::MAX,
                "the added track's playhead is published"
            );
        });
    }

    #[test]
    fn song_unquantized_row_boundary_keeps_sample_offset() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            // 1.50037 beats * 24_000 samples/beat = 36_008.88: on no step,
            // block, or launch-quantize grid (spec 8.2).
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.50037, 1, Vec::new()),
                ],
                4.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 96_000);
            let applied = song_row_applied(&notices);
            let row1 = applied
                .iter()
                .find(|record| record.row_ordinal == 1)
                .expect("row 1 applied");
            assert!(
                (36_008..=36_009).contains(&row1.effective_sample),
                "unquantized boundary must keep its sub-block sample offset: {row1:?}"
            );
            assert!((row1.effective_beat - 1.50037).abs() < 1e-9);
            assert_ne!(row1.effective_sample % 16_000, 0, "must not snap to block edges");
            assert_ne!(row1.effective_sample % 6_000, 0, "must not snap to the step grid");
            let boundary = row1.effective_sample;
            // The step just before the boundary still comes from the old
            // row. The new row anchors its lanes at its own start beat
            // (takes spec 7.2: rows re-anchor, free-run is gone), so its
            // step 0 fires exactly at the unquantized boundary sample.
            let track0: Vec<_> = events.iter().filter(|event| event.track == 0).collect();
            let before = track0
                .iter()
                .filter(|event| event.sample_time < boundary)
                .max_by_key(|event| event.sample_time)
                .expect("step before the boundary");
            assert!((35_999..=36_000).contains(&before.sample_time), "{before:?}");
            assert_eq!(before.transpose, 1.0);
            let after = track0
                .iter()
                .filter(|event| event.sample_time >= boundary)
                .min_by_key(|event| event.sample_time)
                .expect("step after the boundary");
            assert!(
                (36_008..=36_009).contains(&after.sample_time),
                "the anchored row's step 0 must fire at the boundary: {after:?}"
            );
            assert_eq!(after.transpose, 2.0);
        });
    }

    /// Anchored clip phase (takes spec 7.1/7.2): a clip whose row starts at
    /// a beat that is NOT a multiple of the pattern length plays step 0 at
    /// its start beat (free-run would land mid-pattern), and a stored
    /// `offset_steps` shifts the start point into the pattern.
    #[test]
    fn song_row_clip_anchors_step_zero_at_its_start_beat() {
        run_with_scheduler_stack(|| {
            let (state, override_id) = song_mode_fixture();
            // Step-indexed transposes on the override pattern so a trigger
            // identifies its step: transpose = 100 + step.
            state.with_scenes_mut(|scenes| {
                assert!(scenes.track_pools[0].edit(override_id, |data| {
                    for step in 0..16 {
                        data.step_data[step][StepParam::Transpose.index()] = 100.0 + step as f32;
                    }
                }));
            });
            // Row 1 starts at beat 5.0 — not a multiple of the 4-beat
            // pattern cycle. Free-run would start at step 4.
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 5.0, 1, vec![(0, override_id.0)]),
                ],
                8.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            let (events, _) = drive_song_lookahead(&state, runtime, 16_000, 240_000);
            let clip: Vec<_> = events
                .iter()
                .filter(|event| event.track == 0 && event.transpose >= 100.0)
                .collect();
            let first = clip.first().expect("clip triggers");
            assert_eq!(first.sample_time, 120_000, "step 0 fires at the row start");
            assert_eq!(first.transpose, 100.0, "clip starts at step 0, not free-run");
            assert_eq!(clip[1].sample_time, 126_000);
            assert_eq!(clip[1].transpose, 101.0);

            // A stored offset starts the clip that many steps in.
            let mut offset_row = song_mode_row(1, 5.0, 1, vec![(0, override_id.0)]);
            offset_row.overrides[0].offset_steps = 4.0;
            song_mode_commit(
                &state,
                vec![song_mode_row(0, 0.0, 0, Vec::new()), offset_row],
                8.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            assert_eq!(runtime.rows[1].lane_offsets[0], 4.0);
            let (events, _) = drive_song_lookahead(&state, runtime, 16_000, 240_000);
            let first = events
                .iter()
                .find(|event| event.track == 0 && event.transpose >= 100.0)
                .expect("clip triggers");
            assert_eq!(first.sample_time, 120_000);
            assert_eq!(
                first.transpose, 104.0,
                "offset 4 steps: the clip starts at step 4 at its start beat"
            );
        });
    }

    #[test]
    fn song_row_scene_plus_override_applies_atomically() {
        run_with_scheduler_stack(|| {
            let (state, override_id) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.0, 1, vec![(0, override_id.0)]),
                ],
                2.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            assert_eq!(runtime.rows[1].resolved_pattern_ids[0], Some(override_id));
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 48_000);
            let applied = song_row_applied(&notices);
            let boundary = applied
                .iter()
                .find(|record| record.row_ordinal == 1)
                .expect("row 1 applied")
                .effective_sample;
            assert!((23_999..=24_000).contains(&boundary));
            for event in &events {
                let expected = match (event.track, event.sample_time < boundary) {
                    (0, true) => 1.0,
                    // The override is part of the row's single state: track 0
                    // schedules from the override pattern from the first
                    // post-boundary sample on.
                    (0, false) => 9.0,
                    (1, true) => 1.0,
                    (1, false) => 2.0,
                    _ => continue,
                };
                assert_eq!(event.transpose, expected, "split at {boundary}: {event:?}");
            }
            assert!(
                events.iter().all(|event| !(event.track == 0 && event.transpose == 2.0)),
                "track 0 must never expose an intermediate scene-without-override state: {events:#?}"
            );
            // Both halves of the row state flip at the same boundary sample.
            assert!(events
                .iter()
                .any(|e| e.track == 0 && e.sample_time == boundary && e.transpose == 9.0));
            assert!(events
                .iter()
                .any(|e| e.track == 1 && e.sample_time == boundary && e.transpose == 2.0));
        });
    }

    #[test]
    fn song_row_transition_accum_reset_is_diff_aware() {
        run_with_scheduler_stack(|| {
            let (state, override_id) = song_mode_fixture();
            let mut silence_row = song_mode_row(2, 2.0, 0, Vec::new());
            silence_row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new(1, None)];
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    // Row 1 changes ONLY track 0 (override); track 1 keeps
                    // scene 0's pattern through the boundary.
                    song_mode_row(1, 1.0, 0, vec![(0, override_id.0)]),
                    // Row 2 drops the override (track 0 changes back) and
                    // explicitly empties track 1.
                    silence_row,
                ],
                3.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");

            let mut resets = [false; MAX_TRACKS];
            crate::scheduler::lookahead::mark_song_row_accum_resets(
                &runtime.rows[0],
                &runtime.rows[1],
                &mut resets,
            );
            assert!(resets[0], "track 0's resolved pattern changed");
            assert!(
                !resets[1],
                "track 1 plays the same pattern through the boundary and must keep \
                 its accumulator state"
            );

            let mut resets = [false; MAX_TRACKS];
            crate::scheduler::lookahead::mark_song_row_accum_resets(
                &runtime.rows[1],
                &runtime.rows[2],
                &mut resets,
            );
            assert!(resets[0], "override removed: track 0 changes back");
            assert!(resets[1], "explicit-empty is a pattern change for track 1");

            // Marking is additive: an already-pending reset survives an
            // unchanged-track boundary.
            let mut resets = [false; MAX_TRACKS];
            resets[1] = true;
            crate::scheduler::lookahead::mark_song_row_accum_resets(
                &runtime.rows[0],
                &runtime.rows[1],
                &mut resets,
            );
            assert!(resets[1], "pending flags are preserved");
        });
    }

    #[test]
    fn song_explicit_empty_override_silences_only_its_track() {
        run_with_scheduler_stack(|| {
            let (state, _override_id) = song_mode_fixture();
            let mut silence_row = song_mode_row(1, 1.0, 0, Vec::new());
            silence_row.overrides = vec![crate::sequencer::ProjectSongTrackOverride::new(0, None)];
            song_mode_commit(
                &state,
                vec![song_mode_row(0, 0.0, 0, Vec::new()), silence_row],
                2.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            // Explicit-empty resolves to no pattern even though scene 0's
            // cell for track 0 holds one.
            assert_eq!(runtime.rows[1].resolved_pattern_ids[0], None);
            assert!(runtime.rows[1].scheduler_snapshot.tracks[0].scene_silenced);
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 48_000);
            let applied = song_row_applied(&notices);
            let boundary = applied
                .iter()
                .find(|record| record.row_ordinal == 1)
                .expect("row 1 applied")
                .effective_sample;
            assert!(
                events
                    .iter()
                    .all(|event| !(event.track == 0 && event.sample_time >= boundary)),
                "track 0 must be silent from the boundary on: {events:#?}"
            );
            // Track 1 keeps playing scene 0's pattern through the boundary.
            assert!(events
                .iter()
                .any(|event| event.track == 1
                    && event.sample_time >= boundary
                    && event.transpose == 1.0));
        });
    }

    #[test]
    fn song_loop_reapplies_row_zero_without_stale_override_or_duplicate_trigger() {
        run_with_scheduler_stack(|| {
            let (state, override_id) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.0, 0, vec![(0, override_id.0)]),
                ],
                2.0,
                true,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            // 2.5 song cycles: two loop wraps inside the horizon.
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 120_000);
            let applied = song_row_applied(&notices);
            let wraps: Vec<_> = applied.iter().filter(|record| record.wrapped).collect();
            assert!(wraps.len() >= 2, "expected two wraps: {applied:#?}");
            let first_wrap = wraps[0];
            assert_eq!(first_wrap.row_ordinal, 0);
            assert_eq!(first_wrap.effective_beat, 0.0);
            assert!((47_999..=48_000).contains(&first_wrap.effective_sample), "{first_wrap:?}");
            // Row assignment follows the scheduler's own application records:
            // after each wrap, row zero governs again (no stale override).
            let track0: Vec<_> = events.iter().filter(|event| event.track == 0).collect();
            for event in &track0 {
                let expected = match song_row_at_sample(&applied, event.sample_time) {
                    0 => 1.0,
                    1 => 9.0,
                    other => panic!("unexpected row ordinal {other}"),
                };
                assert_eq!(
                    event.transpose, expected,
                    "override must not leak across the wrap: {event:?}"
                );
            }
            // Exactly one edge trigger at the wrap sample, from row zero.
            let wrap_edge: Vec<_> = track0
                .iter()
                .filter(|event| event.sample_time == first_wrap.effective_sample)
                .collect();
            assert_eq!(
                wrap_edge.len(),
                1,
                "exactly one edge trigger at the wrap: {wrap_edge:#?}"
            );
            assert_eq!(wrap_edge[0].transpose, 1.0);
        });
    }

    #[test]
    fn song_large_lookahead_window_does_not_apply_rows_early() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.5, 1, Vec::new()),
                ],
                4.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            // One scheduling block covers the whole song; the boundary must
            // still split it at its exact sample rather than moving to
            // either edge of the huge block (spec 14.3).
            let (events, notices) = drive_song_lookahead(&state, runtime, 96_000, 96_000);
            let applied = song_row_applied(&notices);
            let row1 = applied
                .iter()
                .find(|record| record.row_ordinal == 1)
                .expect("row 1 applied");
            assert!((35_999..=36_000).contains(&row1.effective_sample), "{row1:?}");
            let boundary = row1.effective_sample;
            let track0: Vec<_> = events.iter().filter(|event| event.track == 0).collect();
            for event in &track0 {
                let expected = if event.sample_time < boundary { 1.0 } else { 2.0 };
                assert_eq!(event.transpose, expected, "{event:?}");
            }
            assert!(track0.iter().any(|event| event.sample_time < boundary));
            assert!(track0.iter().any(|event| event.sample_time >= boundary));
        });
    }

    #[test]
    fn song_end_stops_scheduling_and_notifies_control() {
        run_with_scheduler_stack(|| {
            let (state, _) = song_mode_fixture();
            song_mode_commit(
                &state,
                vec![
                    song_mode_row(0, 0.0, 0, Vec::new()),
                    song_mode_row(1, 1.0, 1, Vec::new()),
                ],
                2.0,
                false,
            );
            let runtime = state.preflight_runtime_song().expect("preflight");
            let (events, notices) = drive_song_lookahead(&state, runtime, 16_000, 96_000);
            let end_sample = notices
                .iter()
                .find_map(|notice| match notice {
                    crate::sequencer::SongPlaybackNotice::Ended {
                        end_beat,
                        end_sample,
                    } => {
                        assert!((*end_beat - 2.0).abs() < 1e-9);
                        Some(*end_sample)
                    }
                    _ => None,
                })
                .expect("end notice");
            assert!((47_999..=48_000).contains(&end_sample), "{end_sample}");
            assert!(
                events.iter().all(|event| event.sample_time < end_sample),
                "nothing may be scheduled at or past end_beat: {events:#?}"
            );
            assert!(events
                .iter()
                .any(|event| (41_999..=42_000).contains(&event.sample_time)));
        });
    }

    #[test]
    fn song_preflight_rejects_dangling_row_reference() {
        let (state, _) = song_mode_fixture();
        song_mode_commit(
            &state,
            vec![song_mode_row(0, 0.0, 0, vec![(0, 99)])],
            4.0,
            false,
        );
        let err = state.preflight_runtime_song().expect_err("dangling override");
        assert!(err.contains("pattern 99"), "{err}");
        assert!(err.contains("row 1"), "{err}");
    }

    #[test]
    fn song_apply_row_is_one_atomic_control_transition() {
        let (state, override_id) = song_mode_fixture();
        // A stale live override on track 1 must not survive the row.
        state.with_scenes_mut(|scenes| {
            scenes.track_overrides[1] = Some(crate::sequencer::PatternId(2));
        });
        let epoch_before = state.transport.pattern_epoch.load(Ordering::Relaxed);
        let version_before = state.scheduler_snapshot_version();
        let sample_ids = state
            .apply_song_row(1, &[(0, Some(override_id))], 2, &[], &[], &[], &[], true)
            .expect("apply song row");
        assert_eq!(sample_ids.len(), 2);
        state.with_scenes_mut(|scenes| {
            assert_eq!(scenes.current_scene, 1);
            assert_eq!(scenes.track_overrides[0], Some(override_id));
            assert_eq!(scenes.track_overrides[1], None, "stale override must be cleared");
        });
        assert_eq!(state.current_scene_index(), 1);
        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before + 1,
            "one epoch bump for the whole transition"
        );
        assert_eq!(
            state.scheduler_snapshot_version(),
            version_before + 1,
            "one snapshot publish for the whole transition"
        );

        // A rejected row is side-effect free.
        let version_before = state.scheduler_snapshot_version();
        let err = state
            .apply_song_row(7, &[], 2, &[], &[], &[], &[], true)
            .expect_err("missing scene");
        assert!(err.contains("scene 8"), "{err}");
        assert_eq!(state.scheduler_snapshot_version(), version_before);
        assert_eq!(state.current_scene_index(), 1);

        // Explicit-empty override: track 0 is scene-silenced without falling
        // back to the scene cell; track 1 restores normally.
        state
            .apply_song_row(0, &[(0, None)], 2, &[], &[], &[], &[], true)
            .expect("apply explicit-empty row");
        assert!(state.is_scene_silenced(0));
        assert!(!state.is_scene_silenced(1));
    }

    /// Regression: a quantized scene launch must sound the NEW pattern's
    /// first step exactly at the quantize boundary. The old control-side
    /// apply ran after the boundary had rendered — the epoch bump dropped
    /// the in-flight events and the resync seek marked the boundary step as
    /// already played, silencing every first-step hit. The scheduler now
    /// splits the lookahead chunk at the boundary and schedules at/after it
    /// from the launch's prebuilt snapshot (song-row semantics).
    #[test]
    fn boundary_launch_schedules_the_new_patterns_first_step_at_the_boundary() {
        run_with_scheduler_stack(boundary_launch_first_step_body)
    }

    fn boundary_launch_first_step_body() {
        use crate::quantized_launch::{
            LaunchQuantize, PatternLaunchTarget, QuantizedLaunchOwner,
        };
        use crate::sequencer::PatternSnapshot;
        use std::sync::Arc;

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.transport.playing.store(true, Ordering::Relaxed);
        // Scene 0 (the base snapshot) has no active steps; the target scene's
        // prebuilt snapshot has a hit on step 0. Any trigger the lookahead
        // enqueues can therefore only come from the launched snapshot.
        let base_snapshot = state.publish_scheduler_snapshot();
        let mut prebuilt = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut prebuilt.tracks[0]).steps[0].active = true;
        prebuilt.transport.playing = true;
        prebuilt.transport.current_pattern = 1;
        let prebuilt = Arc::new(prebuilt);

        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::Scene { scene: 1 },
                LaunchQuantize::Bar,
                QuantizedLaunchOwner::Transport,
                2,
                1,
                Some(Arc::clone(&prebuilt)),
            )
            .expect("schedule boundary launch");

        let queue = ScheduledEventQueue::<64>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        // Session mode, playing: the request routes to the boundary-split
        // machinery with a frontier-quantized deadline (bar boundary 4.0).
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            0.0,
            scheduler.clock.total_beats,
            true,
            false,
        );
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;
        // 120 BPM at 48k: samples_per_quarter 24_000, bar boundary at sample
        // 96_000. Schedule far enough past it in one pass.
        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &base_snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            base_snapshot.transport.pattern_epoch,
            0,
            120_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let mut triggers = Vec::new();
        while let Some(event) = queue.pop() {
            if let ScheduledEventKind::ResolvedTrigger { track, step, .. } = event.kind {
                triggers.push((event.sample_time, track, step));
            }
        }
        // The boundary maps to samples with the same floor arithmetic song
        // rows use (spec 8.2): exact up to ±1 sample of float accumulation.
        let (first_sample, first_track, first_step) =
            *triggers.first().expect("the launched pattern's step 0 must sound");
        assert!(
            first_sample.abs_diff(96_000) <= 1,
            "the launched pattern's step 0 must sound at the bar boundary: {triggers:?}"
        );
        assert_eq!((first_track, first_step), (0, 0), "{triggers:?}");
        // The boundary application reaches the control thread as a
        // scheduler-applied due stamped with the boundary beat.
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            4.0,
            scheduler.clock.total_beats,
            true,
            false,
        );
        let due = state.quantized_launches().drain_valid_due();
        assert_eq!(due.len(), 1);
        assert!(due[0].scheduler_applied);
        assert_eq!(due[0].deadline_beats, 4.0);
        // Until the control-side mirror acknowledges, chunks schedule from
        // the launch snapshot override.
        assert!(scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .is_some_and(|snapshot| Arc::ptr_eq(&snapshot, &prebuilt)));
        assert_eq!(scheduler.quantized_launches.adopted_pattern(), Some(1));
        state
            .quantized_launches()
            .acknowledge_mirror(due[0].token)
            .expect("ack");
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            4.1,
            scheduler.clock.total_beats,
            true,
            false,
        );
        assert!(scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .is_none());
        // The ack releases the snapshot override but NOT the adoption: the
        // control thread publishes the mirrored snapshot BEFORE it acks, and
        // the worker drains the ack a whole loop iteration before it compares
        // patterns. Releasing here would let the mirror's pattern change fire
        // the resync (queue clear + seek) the boundary split exists to avoid.
        assert_eq!(scheduler.quantized_launches.adopted_pattern(), Some(1));
        assert!(
            scheduler.quantized_launches.observe_pattern_switch(1),
            "the mirrored pattern is adopted, not resynced"
        );
        assert_eq!(scheduler.quantized_launches.adopted_pattern(), None);
        // Spent: a later switch back to the same scene resyncs normally.
        assert!(!scheduler.quantized_launches.observe_pattern_switch(1));
    }

    /// Two owners quantized to the same boundary (a scene launch plus a track
    /// launch) are BOTH made audible: every due is reported as
    /// scheduler-applied, so the control-side mirror skips the epoch bump for
    /// both and a single-slot override would silently drop one launch's
    /// content.
    #[test]
    fn concurrent_boundary_launches_all_reach_the_chunk_snapshot() {
        use crate::quantized_launch::{
            LaunchQuantize, PatternLaunchTarget, QuantizedLaunchOwner,
        };
        use std::sync::Arc;

        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.transport.playing.store(true, Ordering::Relaxed);
        let base_snapshot = state.publish_scheduler_snapshot();
        let mut scene_launch = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut scene_launch.tracks[0]).steps[0].active = true;
        scene_launch.transport.current_pattern = 1;
        let scene_launch = Arc::new(scene_launch);
        let mut track_launch = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut track_launch.tracks[1]).steps[1].active = true;
        let track_launch = Arc::new(track_launch);

        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::Scene { scene: 1 },
                LaunchQuantize::Quarter,
                QuantizedLaunchOwner::Transport,
                2,
                2,
                Some(Arc::clone(&scene_launch)),
            )
            .expect("schedule scene launch");
        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::SceneTracks {
                    scene: 0,
                    tracks: vec![1],
                },
                LaunchQuantize::Quarter,
                QuantizedLaunchOwner::SceneMacro(3),
                2,
                2,
                Some(Arc::clone(&track_launch)),
            )
            .expect("schedule track launch");

        let mut scheduler = SchedulerLookaheadState::new(48_000);
        state
            .quantized_launches()
            .process_scheduler(&mut scheduler.quantized_launches, 0.3, 0.3, true, false);
        // Both land on the same quarter boundary (1.0) and install together.
        let (_, install) = scheduler
            .quantized_launches
            .next_session_chunk(1.0, 1_000.0, 4_096);
        assert!(matches!(
            install,
            crate::quantized_launch::SessionLaunchInstall::AllTracks
        ));
        let merged = scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .expect("both overrides");
        assert!(
            Arc::ptr_eq(&merged.tracks[0], &scene_launch.tracks[0]),
            "the scene launch must still schedule its own tracks"
        );
        assert!(
            Arc::ptr_eq(&merged.tracks[1], &track_launch.tracks[1]),
            "the track launch overrides the scene launch on its mask"
        );
        assert_eq!(scheduler.quantized_launches.adopted_pattern(), Some(1));

        // Both dues report scheduler-applied, and each ack releases only its
        // own override.
        state
            .quantized_launches()
            .process_scheduler(&mut scheduler.quantized_launches, 1.05, 1.05, true, false);
        let due = state.quantized_launches().drain_valid_due();
        assert_eq!(due.len(), 2);
        assert!(due.iter().all(|entry| entry.scheduler_applied));
        for entry in &due {
            state
                .quantized_launches()
                .acknowledge_mirror(entry.token)
                .expect("ack");
        }
        state
            .quantized_launches()
            .process_scheduler(&mut scheduler.quantized_launches, 1.1, 1.1, true, false);
        assert!(scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .is_none());
    }

    /// A prebuilt launch snapshot freezes bpm/track count at preflight, up to
    /// a whole quantize interval before the boundary. The clock derives its
    /// beat rate from the chunk snapshot while the surrounding chunk math uses
    /// the live one, so the live transport must win for the override window.
    #[test]
    fn boundary_launch_override_follows_the_live_tempo_not_the_preflight_one() {
        use crate::quantized_launch::{
            LaunchQuantize, PatternLaunchTarget, QuantizedLaunchOwner,
        };
        use std::sync::Arc;

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.transport.playing.store(true, Ordering::Relaxed);
        state.transport.bpm.store(120, Ordering::Relaxed);
        let mut prebuilt = (*state.publish_scheduler_snapshot()).clone();
        Arc::make_mut(&mut prebuilt.tracks[0]).steps[0].active = true;
        let prebuilt = Arc::new(prebuilt);
        // Tempo edited while the launch waits for its boundary.
        state.transport.bpm.store(90, Ordering::Relaxed);
        let base_snapshot = state.publish_scheduler_snapshot();

        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::Scene { scene: 0 },
                LaunchQuantize::Quarter,
                QuantizedLaunchOwner::Transport,
                1,
                1,
                Some(Arc::clone(&prebuilt)),
            )
            .expect("schedule scene launch");
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        state
            .quantized_launches()
            .process_scheduler(&mut scheduler.quantized_launches, 0.3, 0.3, true, false);
        scheduler
            .quantized_launches
            .next_session_chunk(1.0, 1_000.0, 4_096);
        let override_snapshot = scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .expect("installed override");
        assert_eq!(
            override_snapshot.transport.bpm, 90,
            "the override window must advance the clock at the live tempo"
        );
        assert_eq!(
            override_snapshot.transport.num_tracks,
            base_snapshot.transport.num_tracks
        );
        assert!(
            Arc::ptr_eq(&override_snapshot.tracks[0], &prebuilt.tracks[0]),
            "the launched content still comes from the prebuilt snapshot"
        );
    }

    /// Track-mask boundary launches merge only the launched tracks over the
    /// live base snapshot, and a transport stop degrades pending boundary
    /// launches to the immediate control-side path.
    #[test]
    fn boundary_launch_track_mask_merges_over_base_and_stop_flushes_pending() {
        use crate::quantized_launch::{
            LaunchQuantize, PatternLaunchTarget, QuantizedLaunchOwner,
        };
        use std::sync::Arc;

        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.transport.playing.store(true, Ordering::Relaxed);
        let base_snapshot = state.publish_scheduler_snapshot();
        let mut prebuilt = (*state.latest_scheduler_snapshot()).clone();
        Arc::make_mut(&mut prebuilt.tracks[0]).steps[0].active = true;
        Arc::make_mut(&mut prebuilt.tracks[1]).steps[0].active = true;
        let prebuilt = Arc::new(prebuilt);

        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::SceneTracks {
                    scene: 0,
                    tracks: vec![0],
                },
                LaunchQuantize::Quarter,
                QuantizedLaunchOwner::Transport,
                1,
                2,
                Some(Arc::clone(&prebuilt)),
            )
            .expect("schedule track launch");
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            0.3,
            0.3,
            true,
            false,
        );
        // Chunk clamps to the quarter boundary (1.0): 0.7 beats at 1_000
        // samples per quarter.
        let (frames, _) = scheduler
            .quantized_launches
            .next_session_chunk(0.3, 1_000.0, 4_096);
        assert_eq!(frames, 700);
        // On the boundary: install, launched track resets its accumulator.
        let (frames, install) = scheduler
            .quantized_launches
            .next_session_chunk(1.0, 1_000.0, 4_096);
        assert_eq!(frames, 4_096);
        assert!(matches!(
            install,
            crate::quantized_launch::SessionLaunchInstall::Tracks(ref tracks)
                if tracks == &vec![0]
        ));
        let merged = scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .expect("merged override");
        assert!(Arc::ptr_eq(&merged.tracks[0], &prebuilt.tracks[0]));
        assert!(Arc::ptr_eq(&merged.tracks[1], &base_snapshot.tracks[1]));
        assert_eq!(
            scheduler.quantized_launches.adopted_pattern(),
            None,
            "track launches never change the scene index"
        );

        // Second pending launch, then transport stop: it degrades to an
        // immediately-due legacy launch (scheduler_applied false).
        state
            .quantized_launches()
            .schedule(
                PatternLaunchTarget::SceneTracks {
                    scene: 0,
                    tracks: vec![1],
                },
                LaunchQuantize::Bar,
                QuantizedLaunchOwner::SceneMacro(9),
                1,
                2,
                Some(Arc::clone(&prebuilt)),
            )
            .expect("schedule second launch");
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            1.2,
            1.2,
            true,
            false,
        );
        state.quantized_launches().process_scheduler(
            &mut scheduler.quantized_launches,
            1.3,
            1.3,
            false,
            false,
        );
        let due = state.quantized_launches().drain_valid_due();
        let second = due
            .iter()
            .find(|entry| {
                entry.target
                    == PatternLaunchTarget::SceneTracks {
                        scene: 0,
                        tracks: vec![1],
                    }
            })
            .expect("stopped transport flushes the pending boundary launch");
        assert!(!second.scheduler_applied);
        // The stop also dropped the installed-but-unmirrored override.
        assert!(scheduler
            .quantized_launches
            .session_snapshot(&base_snapshot)
            .is_none());
    }

    // ── Track rolling (docs/rolling-core-spec.md, phase 1) ──────────────────

    use super::{schedule_roll_hits, RollState};
    use crate::sequencer::RollCommand;

    /// 48kHz at the default 120bpm: 24_000 samples per quarter.
    const ROLL_TEST_SPQ: f64 = 24_000.0;

    fn roll_test_state(held: &[(usize, f32)]) -> (Arc<SequencerState>, SchedulerLookaheadState) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.toggle_play();
        state.transport.roll_mode.store(true, Ordering::Release);
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        for (track, transpose) in held {
            scheduler.roll.apply_commands(&[RollCommand::NoteOn {
                track: *track,
                transpose: *transpose,
            }]);
        }
        (state, scheduler)
    }

    fn drive_roll_chunks(
        state: &Arc<SequencerState>,
        scheduler: &mut SchedulerLookaheadState,
        queue: &ScheduledEventQueue<64>,
        rendered: u64,
        scheduled_until: u64,
    ) -> u64 {
        let snapshot = state.publish_scheduler_snapshot();
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;
        schedule_playing_lookahead(
            scheduler,
            state,
            &snapshot,
            queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            rendered,
            24_000,
            48_000,
            12_000,
            ROLL_TEST_SPQ,
            scheduled_until,
            false,
            false,
        )
        .scheduled_until_sample
    }

    /// Pop every enqueued ResolvedTrigger as (sample_time, chord notes, velocity).
    fn drain_roll_triggers(queue: &ScheduledEventQueue<64>) -> Vec<(u64, Vec<f32>, f32)> {
        let mut triggers = Vec::new();
        while let Some(event) = queue.pop() {
            if let ScheduledEventKind::ResolvedTrigger {
                chord, resolved, ..
            } = event.kind
            {
                triggers.push((
                    event.sample_time,
                    chord.notes[..chord.count].to_vec(),
                    resolved.velocity,
                ));
            }
        }
        triggers
    }

    #[test]
    fn roll_emits_boundary_exact_hits_on_the_straight_grid() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            // Default rate: 1/16 → 0.25 beats → 6_000 samples at 120bpm/48kHz.
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![0, 6_000, 12_000, 18_000],
            );
            for (_, notes, velocity) in &triggers {
                // F4: track default velocity, held transpose as the single note.
                assert_eq!(notes.as_slice(), &[3.0]);
                assert_eq!(*velocity, StepParam::Velocity.default_value());
            }
        });
    }

    #[test]
    fn roll_emits_boundary_exact_hits_on_the_triplet_grid() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            state
                .transport
                .roll_rate
                .store(Timebase::SixteenthTriplet as u32, Ordering::Release);
            // 1/16t → 1/6 beat → 4_000 samples.
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![0, 4_000, 8_000, 12_000, 16_000, 20_000],
            );
        });
    }

    #[test]
    fn roll_rate_switch_mid_hold_reschedules_future_boundaries_only() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            let queue = ScheduledEventQueue::<64>::new();
            let scheduled_until = drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            assert_eq!(scheduled_until, 24_000);
            drain_roll_triggers(&queue);

            // F2: the rate is re-read each pass — no scheduler-side state to
            // reshape. Boundaries already enqueued stay; new ones use the new grid.
            state
                .transport
                .roll_rate
                .store(Timebase::ThirtySecond as u32, Ordering::Release);
            scheduler
                .roll
                .apply_commands(&[RollCommand::SetRate {
                    rate: Timebase::ThirtySecond,
                }]);
            drive_roll_chunks(&state, &mut scheduler, &queue, 24_000, scheduled_until);
            let triggers = drain_roll_triggers(&queue);
            // 1/32 → 0.125 beats → 3_000 samples, starting at beat 1.0.
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![24_000, 27_000, 30_000, 33_000, 36_000, 39_000, 42_000, 45_000],
            );
        });
    }

    #[test]
    fn roll_note_off_cancels_every_hit_not_yet_scheduled() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            let queue = ScheduledEventQueue::<64>::new();
            let scheduled_until = drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            assert_eq!(drain_roll_triggers(&queue).len(), 4);

            // F3: the note-off is drained before the lookahead extends, so no
            // further boundary is enqueued.
            let generation_before = scheduler.roll.generation;
            scheduler.roll.apply_commands(&[RollCommand::NoteOff {
                track: 0,
                transpose: 3.0,
            }]);
            assert!(scheduler.roll.generation > generation_before);
            drive_roll_chunks(&state, &mut scheduler, &queue, 24_000, scheduled_until);
            assert!(drain_roll_triggers(&queue).is_empty());
        });
    }

    #[test]
    fn roll_mode_off_emits_nothing_even_with_held_notes() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            state.transport.roll_mode.store(false, Ordering::Release);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            assert!(drain_roll_triggers(&queue).is_empty());
        });
    }

    #[test]
    fn held_transposes_emit_as_a_chord_and_duplicate_presses_collapse() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0), (0, 7.0)]);
            // Duplicate press of an already-held transpose is a no-op (set
            // semantics — spec 3).
            scheduler.roll.apply_commands(&[RollCommand::NoteOn {
                track: 0,
                transpose: 3.0,
            }]);
            assert_eq!(scheduler.roll.held[0].as_slice(), &[3.0, 7.0]);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(triggers.len(), 4);
            for (_, notes, _) in &triggers {
                assert_eq!(notes.as_slice(), &[3.0, 7.0]);
            }
        });
    }

    #[test]
    fn clear_all_empties_held_state_and_bumps_the_generation() {
        let mut roll = RollState::new();
        roll.apply_commands(&[
            RollCommand::NoteOn {
                track: 0,
                transpose: 3.0,
            },
            RollCommand::NoteOn {
                track: 2,
                transpose: -5.0,
            },
        ]);
        assert!(roll.any_held());
        let generation_before = roll.generation;
        roll.apply_commands(&[RollCommand::ClearAll]);
        assert!(!roll.any_held());
        assert!(roll.generation > generation_before);
        // ClearAll on an already-empty state does not churn the generation.
        let settled = roll.generation;
        roll.clear_all();
        assert_eq!(roll.generation, settled);
    }

    #[test]
    fn schedule_roll_hits_scans_half_open_chunk_ranges() {
        run_with_scheduler_stack(|| {
            // A boundary exactly at chunk_end belongs to the NEXT chunk; one
            // exactly at chunk_start belongs to this chunk — no double fire, no
            // gap, across an f64-accumulated chunk seam.
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            let snapshot = state.publish_scheduler_snapshot();
            let mut roll = RollState::new();
            roll.apply_commands(&[RollCommand::NoteOn {
                track: 0,
                transpose: 0.0,
            }]);
            let queue = ScheduledEventQueue::<64>::new();
            let mut track_output_events = Vec::new();
            for (chunk_start, chunk_end, chunk_start_sample) in
                [(0.0, 0.5, 0), (0.5, 1.0, 12_000)]
            {
                assert!(schedule_roll_hits(
                    &queue,
                    &snapshot,
                    &mut track_output_events,
                    &state,
                    &SnapshotSequencerClock::new(48_000),
                    &mut roll,
                    0.25,
                    chunk_start,
                    chunk_end,
                    chunk_start_sample,
                    chunk_start_sample,
                    ROLL_TEST_SPQ,
                    0,
                    0.0,
                ));
            }
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![0, 6_000, 12_000, 18_000],
            );
        });
    }

    #[test]
    fn rolled_hits_feed_back_boundary_exact_record_positions() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            state
                .transport
                .roll_rate
                .store(Timebase::ThirtySecond as u32, Ordering::Release);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let hits = state.drain_roll_recorded_hits();
            // One beat at 1/32 = 8 hits; the default 1/16 track timebase puts
            // two hits per step, the off-grid one as a 0.5-step delay (F5:
            // grids finer than the timebase land as sub-step delays).
            let positions: Vec<(usize, f32)> =
                hits.iter().map(|hit| (hit.step, hit.delay)).collect();
            assert_eq!(
                positions,
                vec![
                    (0, 0.0),
                    (0, 0.5),
                    (1, 0.0),
                    (1, 0.5),
                    (2, 0.0),
                    (2, 0.5),
                    (3, 0.0),
                    (3, 0.5),
                ],
            );
            for hit in &hits {
                assert_eq!(hit.track, 0);
                assert_eq!(hit.transpose, 3.0);
                assert_eq!(hit.velocity, 1.0);
                assert!((hit.duration_steps - 0.5).abs() < 1e-6);
            }
            assert!((hits[1].beat - 0.125).abs() < 1e-9);
            // The drain is destructive: nothing left behind.
            assert!(state.drain_roll_recorded_hits().is_empty());
        });
    }

    #[test]
    fn rolled_triplet_hits_record_fractional_step_delays() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0), (0, 7.0)]);
            state
                .transport
                .roll_rate
                .store(Timebase::SixteenthTriplet as u32, Ordering::Release);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let hits = state.drain_roll_recorded_hits();
            // 6 triplet boundaries per beat, two held transposes per boundary.
            assert_eq!(hits.len(), 12);
            let expected = [
                (0, 0.0_f32),
                (0, 2.0 / 3.0),
                (1, 1.0 / 3.0),
                (2, 0.0),
                (2, 2.0 / 3.0),
                (3, 1.0 / 3.0),
            ];
            for (boundary, (step, delay)) in expected.iter().enumerate() {
                for note in 0..2 {
                    let hit = &hits[boundary * 2 + note];
                    assert_eq!(hit.step, *step, "boundary {boundary}");
                    assert!(
                        (hit.delay - delay).abs() < 1e-4,
                        "boundary {boundary}: delay {} != {delay}",
                        hit.delay,
                    );
                    assert!((hit.duration_steps - 2.0 / 3.0).abs() < 1e-4);
                }
                assert_eq!(hits[boundary * 2].transpose, 3.0);
                assert_eq!(hits[boundary * 2 + 1].transpose, 7.0);
            }
        });
    }

    /// Drive one lookahead pass with an intentionally grid-unaligned frontier
    /// (block 1300 / target 5000 samples) so the catch-up seam is exercised.
    fn drive_roll_chunks_unaligned(
        state: &Arc<SequencerState>,
        scheduler: &mut SchedulerLookaheadState,
        queue: &ScheduledEventQueue<64>,
        rendered: u64,
        scheduled_until: u64,
    ) -> u64 {
        let snapshot = state.publish_scheduler_snapshot();
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = None;
        schedule_playing_lookahead(
            scheduler,
            state,
            &snapshot,
            queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            rendered,
            5_000,
            48_000,
            1_300,
            ROLL_TEST_SPQ,
            scheduled_until,
            false,
            false,
        )
        .scheduled_until_sample
    }

    #[test]
    fn roll_press_catches_a_scheduled_but_unrendered_line() {
        run_with_scheduler_stack(|| {
            // The lookahead frontier passes a 1/16 line before the press
            // drains, but the render head has not reached it yet. Catch-up
            // emits that line retroactively — it is still the next AUDIBLE
            // boundary, so the hit lands sample-exact on the grid.
            let (state, mut scheduler) = roll_test_state(&[]);
            let queue = ScheduledEventQueue::<64>::new();
            let frontier = drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 0, 0);
            assert_eq!(frontier, 5_200, "frontier must sit mid-grid for this test");
            let frontier =
                drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 1_500, frontier);
            assert_eq!(frontier, 6_500, "frontier must sit past the 6000 line");
            assert!(drain_roll_triggers(&queue).is_empty());

            // Press drained with the 6000 line behind the frontier but ahead
            // of the render head (5800) — the line hasn't sounded yet.
            scheduler.roll.apply_commands(&[RollCommand::NoteOn {
                track: 0,
                transpose: 3.0,
            }]);
            drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 5_800, frontier);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![6_000],
                "the scheduled-past, unrendered line is caught sample-exact",
            );
            let hits = state.drain_roll_recorded_hits();
            assert_eq!(
                hits.iter().map(|hit| (hit.step, hit.beat)).collect::<Vec<_>>(),
                vec![(1, 0.25)],
            );
        });
    }

    #[test]
    fn roll_press_after_the_line_rendered_waits_for_the_next_boundary() {
        run_with_scheduler_stack(|| {
            // The render head has passed beat 0 by 1000 samples when the
            // press drains. A retroactive emission would fire immediately —
            // ~21ms late, an audible one-off swing — so the line counts as
            // missed and the first hit waits for the next boundary (F1).
            let (state, mut scheduler) = roll_test_state(&[]);
            let queue = ScheduledEventQueue::<64>::new();
            let frontier = drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 0, 0);

            scheduler.roll.apply_commands(&[RollCommand::NoteOn {
                track: 0,
                transpose: 3.0,
            }]);
            drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 1_000, frontier);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![6_000],
            );
            let hits = state.drain_roll_recorded_hits();
            assert_eq!(
                hits.iter().map(|hit| (hit.step, hit.beat)).collect::<Vec<_>>(),
                vec![(1, 0.25)],
            );
        });
    }

    #[test]
    fn roll_press_far_behind_a_line_waits_for_the_next_boundary() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[]);
            let queue = ScheduledEventQueue::<64>::new();
            let frontier = drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, 0, 0);

            // Render head has caught up to the frontier: beat 0 is ~108ms
            // gone, so F1 semantics hold and the first hit waits for the
            // next line.
            scheduler.roll.apply_commands(&[RollCommand::NoteOn {
                track: 0,
                transpose: 3.0,
            }]);
            drive_roll_chunks_unaligned(&state, &mut scheduler, &queue, frontier, frontier);
            let triggers = drain_roll_triggers(&queue);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![6_000],
            );
            let hits = state.drain_roll_recorded_hits();
            assert_eq!(
                hits.iter().map(|hit| (hit.step, hit.beat)).collect::<Vec<_>>(),
                vec![(1, 0.25)],
            );
        });
    }

    #[test]
    fn roll_hits_on_a_swung_track_match_sequenced_swing_offsets() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            state.pattern.track_params[0].set_swing(75.0);
            state.pattern.track_params[0].set_swing_resolution(SwingResolution::Sixteenth);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            let triggers = drain_roll_triggers(&queue);
            // The exact delay the step scheduler applies to odd swing buckets.
            let swing = swing_delay_samples_from_quarter(
                ROLL_TEST_SPQ,
                75.0,
                SwingResolution::Sixteenth,
            )
            .round() as u64;
            assert_eq!(swing, 3_000);
            assert_eq!(
                triggers.iter().map(|(sample, ..)| *sample).collect::<Vec<_>>(),
                vec![0, 6_000 + swing, 12_000, 18_000 + swing],
                "off-beat roll hits are delayed exactly like sequenced steps",
            );
        });
    }

    #[test]
    fn roll_records_the_straight_grid_even_while_playing_swung() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            state.pattern.track_params[0].set_swing(75.0);
            state.pattern.track_params[0].set_swing_resolution(SwingResolution::Sixteenth);
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            // Record straight (eseq-767.10): no swing-derived micro-timing in
            // the feedback — the swing re-applies at playback, so nothing is
            // printed and later swing changes re-swing the recorded hits.
            let hits = state.drain_roll_recorded_hits();
            assert_eq!(
                hits.iter()
                    .map(|hit| (hit.step, hit.delay, hit.beat))
                    .collect::<Vec<_>>(),
                vec![
                    (0, 0.0, 0.0),
                    (1, 0.0, 0.25),
                    (2, 0.0, 0.5),
                    (3, 0.0, 0.75),
                ],
            );
        });
    }

    #[test]
    fn roll_swing_knob_mid_hold_moves_subsequent_hits() {
        run_with_scheduler_stack(|| {
            let (state, mut scheduler) = roll_test_state(&[(0, 3.0)]);
            let queue = ScheduledEventQueue::<64>::new();
            let scheduled_until = drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);
            assert_eq!(
                drain_roll_triggers(&queue)
                    .iter()
                    .map(|(sample, ..)| *sample)
                    .collect::<Vec<_>>(),
                vec![0, 6_000, 12_000, 18_000],
                "default swing (50) leaves the grid straight",
            );

            // The snapshot is re-read every chunk, so a mid-hold swing change
            // swings every not-yet-scheduled hit.
            state.pattern.track_params[0].set_swing(75.0);
            state.pattern.track_params[0].set_swing_resolution(SwingResolution::Sixteenth);
            drive_roll_chunks(&state, &mut scheduler, &queue, 24_000, scheduled_until);
            assert_eq!(
                drain_roll_triggers(&queue)
                    .iter()
                    .map(|(sample, ..)| *sample)
                    .collect::<Vec<_>>(),
                vec![24_000, 33_000, 36_000, 45_000],
            );
        });
    }

    #[test]
    fn production_lookahead_replays_the_captured_sequence_window() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(4);
            for step in 0..4 {
                state.pattern.patterns[0].set_step_active(step, true);
            }
            state.toggle_play();
            state.transport.roll_mode.store(true, Ordering::Release);
            let snapshot = state.publish_scheduler_snapshot();
            let mut scheduler = SchedulerLookaheadState::new(48_000);
            scheduler.roll.apply_commands_with_clock(
                &[RollCommand::SequenceRoll { on: true }],
                &mut scheduler.clock,
                &snapshot,
            );
            let queue = ScheduledEventQueue::<64>::new();
            drive_roll_chunks(&state, &mut scheduler, &queue, 0, 0);

            let mut steps = Vec::new();
            while let Some(event) = queue.pop() {
                if let ScheduledEventKind::ResolvedTrigger { step, .. } = event.kind {
                    steps.push(step);
                }
            }
            assert!(steps.len() >= 4, "the captured step must retrigger each roll window");
            assert!(steps.iter().all(|step| *step == 0), "rolled steps: {steps:?}");
        });
    }

    #[test]
    fn sequence_roll_remaps_live_phase_across_an_odd_cycle() {
        let cycle = 1.25;
        let grid = 0.5;
        let start = Some(0.75);
        let reads = [1.0, 1.24, 1.25, 1.49, 1.5]
            .map(|live| SnapshotSequencerClock::roll_read_position(live, start, grid, cycle));
        let expected = [0.75, 0.99, 1.0, 1.24, 0.75];
        for (actual, expected) in reads.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-9, "{actual} != {expected}");
        }
    }

    #[test]
    fn sequence_roll_capture_and_rate_switch_reanchor_per_track() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_num_steps(5); // 1.25-beat cycle
        state.pattern.track_params[1].set_num_steps(3); // 0.75-beat cycle
        state.toggle_play();
        let snapshot = state.publish_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        clock.total_beats = 0.9;
        clock.was_playing = true;
        let mut roll = RollState::new();

        roll.apply_commands_with_clock(
            &[
                RollCommand::SetRate { rate: Timebase::EighthTriplet },
                RollCommand::SequenceRoll { on: true },
            ],
            &mut clock,
            &snapshot,
        );
        assert_eq!(roll.window_start[0], Some(0.75));
        assert_eq!(roll.window_start[1], Some(0.0));
        roll.publish_windows(&state, Timebase::EighthTriplet.step_beats(16));
        assert_eq!(
            f64::from_bits(state.transport.roll_window_starts[0].load(Ordering::Relaxed)),
            0.75,
        );
        assert!((f64::from_bits(
            state.transport.roll_window_lengths[0].load(Ordering::Relaxed),
        ) - 1.0 / 3.0)
            .abs()
            < 1.0e-9);

        clock.total_beats = 1.1;
        roll.apply_commands_with_clock(
            &[RollCommand::SetRate { rate: Timebase::Eighth }],
            &mut clock,
            &snapshot,
        );
        assert_eq!(roll.window_start[0], Some(1.0));
        assert_eq!(roll.window_start[1], Some(0.0));

        // A same-rate slow straight re-press is a no-op.
        clock.total_beats = 0.2;
        roll.apply_commands_with_clock(
            &[RollCommand::SetRate { rate: Timebase::Eighth }],
            &mut clock,
            &snapshot,
        );
        assert_eq!(roll.window_start[0], Some(1.0));

        // Fast same-rate re-presses are the deliberate stutter gesture.
        roll.apply_commands_with_clock(
            &[RollCommand::SetRate { rate: Timebase::ThirtySecond }],
            &mut clock,
            &snapshot,
        );
        clock.total_beats = 0.7;
        roll.apply_commands_with_clock(
            &[RollCommand::SetRate { rate: Timebase::ThirtySecond }],
            &mut clock,
            &snapshot,
        );
        assert_eq!(roll.window_start[0], Some(0.75));
    }

    #[test]
    fn sequence_roll_release_resumes_the_true_transport_position() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(5);
        for step in 0..5 {
            state.pattern.patterns[0].set_step_active(step, true);
        }
        state.toggle_play();
        let snapshot = state.publish_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        clock.total_beats = 0.6;
        clock.was_playing = true;
        let mut roll = RollState::new();
        roll.apply_commands_with_clock(
            &[RollCommand::SequenceRoll { on: true }],
            &mut clock,
            &snapshot,
        );

        let rolled = clock.process_chunk_with_roll(
            30_000,
            &snapshot,
            &state,
            Some(&mut roll.window_start),
            Timebase::Sixteenth.step_beats(16),
        );
        assert_eq!(rolled.first().map(|trigger| trigger.step), Some(2));
        assert!(rolled.len() >= 5, "one-step windows must retrigger on every wrap");
        assert!(rolled.iter().all(|trigger| trigger.step == 2));

        roll.apply_commands_with_clock(
            &[RollCommand::SequenceRoll { on: false }],
            &mut clock,
            &snapshot,
        );
        let resumed = clock.process_chunk_with_roll(
            10_000,
            &snapshot,
            &state,
            Some(&mut roll.window_start),
            Timebase::Sixteenth.step_beats(16),
        );
        assert_eq!(resumed.first().map(|trigger| trigger.step), Some(3));
        assert!(resumed.iter().any(|trigger| trigger.step == 4));
    }
