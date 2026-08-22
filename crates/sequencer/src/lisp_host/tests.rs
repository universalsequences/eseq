/*!
The `lisp_host` test suite, extracted verbatim from the inline
`#[cfg(test)] mod tests` block of the pre-split `lisp_host.rs`. Everything
here is reached through `use super::…`, i.e. the façade's re-exports.
*/

    use super::{
        clear_neural_effect_plock_by_network_id, clear_neural_instrument_plock_by_network_id,
        compile_instrument, compile_instrument_with_asset_base, effect_has_host_modulation,
        effect_sidechain_inputs, fallback_effect_descriptors, fallback_instrument_descriptors,
        lisp_list, new_eval_context, parse_manifest, parse_process_port_def,
        read_eseqlisp_init_source, register_graph_authoring_natives,
        register_published_process_authoring_natives,
        register_sequencer_natives, scheduler_scratch_runtime_with_fallbacks,
        scratch_runtime_with_fallbacks, selected_neural_instrument_plock_value,
        set_selected_neural_instrument_plocks, shared_native_metadata, AccumulatorNoteSpan,
        DGenParam, DGenSidechainInput, ScratchControlRuntime, SelectedNeuralNeuron,
        UI_PROCESS_HANDLE_BASE,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
    use crate::neural::{NeuralMaxPolySelection, ParamNodeId};
    use crate::scheduled_event::{
        ScheduledEffectParam, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    };
    use crate::sequencer::{
        default_empty_effect_chain, PublishedSequencer, SequencerState, StepParam, Timebase,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::vm::Value;
    use eseqlisp::{BufferMode, Editor, EditorConfig, Runtime};
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── graph-mode manifest parsing ──
    #[test]
    fn instrument_preset_key_locks_default_and_serialize_by_note_name_map() {
        let legacy_json = r#"{
            "id": "init",
            "name": "Init",
            "base_note_offset": 0.0,
            "params": {"cutoff": 1200.0}
        }"#;
        let legacy: super::InstrumentPreset =
            serde_json::from_str(legacy_json).expect("legacy preset");
        assert!(legacy.key_locks.is_empty());

        let mut key_locks = std::collections::BTreeMap::new();
        key_locks.insert(
            69,
            std::collections::BTreeMap::from([("cutoff".to_string(), 3000.0)]),
        );
        let preset = super::InstrumentPreset {
            id: "afx".to_string(),
            name: "AFX".to_string(),
            base_note_offset: 0.0,
            params: std::collections::BTreeMap::from([("cutoff".to_string(), 1200.0)]),
            key_locks,
        };
        let json = serde_json::to_string(&preset).expect("serialize preset");
        assert!(json.contains("\"key_locks\""), "{json}");
        assert!(json.contains("\"69\""), "{json}");

        let restored: super::InstrumentPreset =
            serde_json::from_str(&json).expect("roundtrip preset");
        assert_eq!(restored.key_locks[&69]["cutoff"], 3000.0);
    }

    #[test]
    fn instrument_preset_key_locks_roundtrip_through_live_slot_by_param_name() {
        let desc = crate::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter cutoff param");
        let source_slot = crate::effects::EffectSlotState::new(&desc, 41);
        source_slot.set_key_lock(64, cutoff_idx, 300.0);
        source_slot.set_key_lock(67, cutoff_idx, 1_200.0);

        let preset = super::InstrumentPreset {
            id: "keyed-vox".to_string(),
            name: "Keyed Vox".to_string(),
            base_note_offset: 0.0,
            params: std::collections::BTreeMap::new(),
            key_locks: crate::effects::capture_key_locks_by_param_name(&source_slot, &desc),
        };
        let json = serde_json::to_string(&preset).expect("serialize keyed preset");
        let restored_preset: super::InstrumentPreset =
            serde_json::from_str(&json).expect("deserialize keyed preset");
        let restored_slot = crate::effects::EffectSlotState::new(&desc, 42);
        crate::effects::restore_key_locks_by_param_name(
            &restored_slot,
            &desc,
            &restored_preset.key_locks,
        );

        assert_eq!(restored_slot.key_locks.get(64, cutoff_idx), Some(300.0));
        assert_eq!(restored_slot.key_locks.get(67, cutoff_idx), Some(1_200.0));
        assert_eq!(
            restored_slot.key_locks.get_id(64, cutoff_idx),
            restored_slot.param_node_id(cutoff_idx),
            "restored locks must be rebound to the destination instrument node"
        );
    }

    fn gv_num(x: f64) -> Value {
        Value::Number(x)
    }
    fn gv_kw(s: &str) -> Value {
        Value::Keyword(s.to_string())
    }
    fn gv_sym(s: &str) -> Value {
        Value::Symbol(s.to_string())
    }
    fn gv_list(items: Vec<Value>) -> Value {
        Value::List(
            items
                .into_iter()
                .map(|v| Rc::new(RefCell::new(v)))
                .collect(),
        )
    }

    fn sample_graph_args() -> Vec<Value> {
        vec![
            gv_sym("neural"),
            gv_kw("shape"),
            gv_list(vec![gv_sym("line"), gv_num(2.0)]),
            gv_kw("energy-decay"),
            gv_num(0.9),
            gv_kw("max-poly"),
            gv_num(4.0),
            gv_kw("max-poly-selection"),
            gv_kw("propagation"),
            gv_kw("duration"),
            gv_list(vec![gv_sym("steps"), gv_num(1.0)]),
            gv_kw("swing"),
            gv_list(vec![gv_sym("swing"), gv_num(57.0), gv_kw("16")]),
            gv_kw("reset-every"),
            gv_list(vec![gv_sym("bars"), gv_num(4.0)]),
            gv_list(vec![
                gv_sym("def-node"),
                gv_sym("nrn"),
                gv_kw("resolution"),
                gv_kw("16"),
                gv_kw("delay"),
                gv_num(1.0),
                gv_kw("duration"),
                gv_list(vec![gv_sym("delay")]),
                gv_kw("swing"),
                gv_list(vec![gv_sym("swing"), gv_num(62.0), gv_kw("8")]),
                gv_kw("seed-from"),
                gv_num(0.0),
                gv_kw("quantize"),
                gv_kw("off"),
                gv_kw("reduce"),
                gv_sym("sum"),
                gv_kw("params"),
                gv_list(vec![
                    gv_list(vec![
                        gv_sym("threshold"),
                        gv_kw("float"),
                        gv_num(0.0),
                        gv_num(4.0),
                        gv_kw("default"),
                        gv_num(1.0),
                    ]),
                    gv_list(vec![
                        gv_sym("transpose"),
                        gv_kw("int"),
                        gv_num(-24.0),
                        gv_num(24.0),
                        gv_kw("default"),
                        gv_num(0.0),
                    ]),
                ]),
                gv_kw("state"),
                gv_list(vec![gv_list(vec![
                    gv_sym("energy"),
                    gv_kw("leak"),
                    gv_list(vec![gv_sym("per-step"), gv_kw("energy-decay")]),
                ])]),
                gv_kw("update"),
                gv_list(vec![
                    gv_sym(">="),
                    gv_list(vec![gv_sym("node-state"), gv_sym("self"), gv_kw("energy")]),
                    gv_num(1.0),
                ]),
            ]),
            gv_list(vec![
                gv_sym("edges"),
                gv_kw("from"),
                gv_sym("nrn"),
                gv_kw("to"),
                gv_sym("nrn"),
                gv_kw("topology"),
                gv_list(vec![gv_sym("all-to-all")]),
                gv_kw("distribution"),
                gv_kw("weighted-choice"),
                gv_kw("params"),
                gv_list(vec![
                    gv_list(vec![
                        gv_sym("weight"),
                        gv_kw("float"),
                        gv_num(-1.0),
                        gv_num(1.0),
                        gv_kw("default"),
                        gv_num(0.5),
                    ]),
                    gv_list(vec![
                        gv_sym("delay"),
                        gv_kw("int"),
                        gv_num(0.0),
                        gv_num(16.0),
                        gv_kw("default"),
                        gv_num(3.0),
                    ]),
                ]),
            ]),
        ]
    }

    #[test]
    fn graph_mode_detected_by_def_node() {
        assert!(super::graph_mode_present(&sample_graph_args()));
        // A tick-style arg list (no def-node) is not graph mode.
        let tick_args = vec![
            Value::Symbol("liebezeit".into()),
            Value::Keyword("resolution".into()),
            Value::Keyword("16".into()),
            Value::Keyword("tick".into()),
            gv_list(vec![gv_sym("lambda"), gv_list(vec![])]),
        ];
        assert!(!super::graph_mode_present(&tick_args));
    }

    #[test]
    fn parse_graph_manifest_extracts_full_shape() {
        use crate::graph::{
            EdgeDistribution, GraphDurationSpec, GraphSwingSpec, LeakSpec, Reduce, SeedFrom,
            ShapeSpec, Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = super::parse_graph_manifest(&sample_graph_args()).expect("parse");
        assert_eq!(manifest.name, "neural");
        assert_eq!(manifest.shape, ShapeSpec::Line(2));
        assert_eq!(manifest.energy_decay, 0.9);
        assert_eq!(manifest.max_poly, 4);
        assert_eq!(
            manifest.max_poly_selection,
            NeuralMaxPolySelection::Propagation
        );
        assert_eq!(manifest.duration, GraphDurationSpec::Steps { value: 1.0 });
        assert_eq!(manifest.swing, GraphSwingSpec::new(57.0, 0));
        assert_eq!(manifest.reset_every_beats, 16.0); // (bars 4) @ 4/4

        let node = &manifest.node;
        assert_eq!(node.name, "nrn");
        assert_eq!(node.resolution, Timebase::Sixteenth);
        assert_eq!(node.delay_steps, 1);
        assert_eq!(node.quantize, None);
        assert_eq!(node.duration, Some(GraphDurationSpec::Delay));
        assert_eq!(node.swing, Some(GraphSwingSpec::new(62.0, 1)));
        assert_eq!(node.reduce, Reduce::Sum);
        assert_eq!(node.seed_from, SeedFrom::Tracks(vec![0]));
        let off_manifest = super::parse_graph_manifest(&vec![
            gv_sym("off-seed"),
            gv_kw("shape"),
            gv_list(vec![gv_sym("line"), gv_num(1.0)]),
            gv_list(vec![
                gv_sym("def-node"),
                gv_sym("nrn"),
                gv_kw("seed-from"),
                gv_kw("off"),
            ]),
        ])
        .expect("parse seed-from off");
        assert_eq!(off_manifest.node.seed_from, SeedFrom::Tracks(Vec::new()));
        assert_eq!(node.param_default("threshold"), Some(1.0));
        let transpose = node.params.iter().find(|p| p.name == "transpose").unwrap();
        assert!(transpose.is_int);
        assert_eq!(transpose.default, 0.0);
        assert_eq!(node.state.len(), 1);
        assert_eq!(node.state[0].name, "energy");
        assert_eq!(node.state[0].leak, Some(LeakSpec::PerStepEnergyDecay));
        assert!(node.update_source.as_deref().unwrap().contains(">="));

        assert_eq!(manifest.edge_sets.len(), 1);
        let edges = &manifest.edge_sets[0];
        assert_eq!(edges.from, "nrn");
        assert_eq!(edges.to, "nrn");
        assert_eq!(edges.topology, Topology::AllToAll);
        assert_eq!(edges.distribution, EdgeDistribution::WeightedChoice);
        assert_eq!(edges.params[0].name, "weight");
        assert_eq!(edges.params[0].default, 0.5);
        assert_eq!(
            edges
                .params
                .iter()
                .find(|param| param.name == "delay")
                .map(|param| param.default),
            Some(3.0)
        );

        // Materialize: 2 nodes, 2x2 all-to-all edges.
        let runtime = manifest.materialize();
        assert_eq!(runtime.num_nodes(), 2);
        assert!(runtime
            .visualization_snapshot()
            .edges
            .iter()
            .all(|edge| edge.delay_steps == 3
                && edge.distribution == EdgeDistribution::WeightedChoice));
    }

    #[test]
    fn parse_graph_manifest_line_shape_keeps_fixed_and_variable_distinct() {
        use crate::graph::ShapeSpec;

        let fixed = super::parse_graph_manifest(&sample_graph_args()).expect("fixed parse");
        assert_eq!(fixed.shape, ShapeSpec::Line(2));

        let mut shorthand = sample_graph_args();
        shorthand[2] = gv_list(vec![
            gv_sym("line"),
            gv_num(8.0),
            gv_kw("max"),
            gv_num(16.0),
        ]);
        let shorthand = super::parse_graph_manifest(&shorthand).expect("shorthand parse");
        assert_eq!(
            shorthand.shape,
            ShapeSpec::VariableLine {
                default: 8,
                min: 1,
                max: 16,
            }
        );

        let mut keyword = sample_graph_args();
        keyword[2] = gv_list(vec![
            gv_sym("line"),
            gv_kw("default"),
            gv_num(8.0),
            gv_kw("min"),
            gv_num(4.0),
            gv_kw("max"),
            gv_num(16.0),
        ]);
        let keyword = super::parse_graph_manifest(&keyword).expect("keyword parse");
        assert_eq!(
            keyword.shape,
            ShapeSpec::VariableLine {
                default: 8,
                min: 4,
                max: 16,
            }
        );

        let mut invalid = sample_graph_args();
        invalid[2] = gv_list(vec![
            gv_sym("line"),
            gv_kw("default"),
            gv_num(3.0),
            gv_kw("min"),
            gv_num(4.0),
            gv_kw("max"),
            gv_num(16.0),
        ]);
        assert!(super::parse_graph_manifest(&invalid)
            .unwrap_err()
            .contains("default within"));

        let mut missing_max = sample_graph_args();
        missing_max[2] = gv_list(vec![gv_sym("line"), gv_kw("default"), gv_num(8.0)]);
        assert!(super::parse_graph_manifest(&missing_max)
            .unwrap_err()
            .contains("requires :max"));
    }

    #[test]
    fn graph_update_predicate_fires_through_vm_and_engine() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::Timebase;

        // One self-looping node (weight 1) seeded with energy 2; its :update fires when
        // energy >= threshold, evaluated on the real scheduler VM. Exercises node-state
        // / node-param accessors + truthiness -> fire through GraphRuntime::process_block.
        let manifest = GraphManifest {
            id: 99,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 2.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                update_source: Some(
                    "(>= (node-state self :energy) (node-param self :threshold))".into(),
                ),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        // Seeded energy fires at beat 1; the self-loop re-fires every quarter after.
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(samples, vec![48_000, 96_000, 144_000, 192_000]);
        assert!(out.iter().all(|e| e.node_index == 0));
        assert_eq!(scratch.graph_updates.len(), 1);
        assert_eq!(
            scratch
                .graph_updates
                .get(&manifest.id)
                .map(|update| update.source.as_str()),
            manifest.node.update_source.as_deref()
        );
    }

    #[test]
    fn graph_update_reads_input_event_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        // A self-looping node seeded (track 0, note 4) whose :update fires only when the
        // arrived event's note exceeds 1 — exercising node-input-event/event-note on the
        // real scratch VM. The carried note (4) re-emits each hop.
        let manifest = GraphManifest {
            id: 7,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
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
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some("(>= (event-note (node-input-event self)) 1)".into()),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 4.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        // The seed reaches the node at beat 1 and the self-loop re-fires every quarter;
        // each emission carries the relayed note (4, node transpose 0).
        assert!(!out.is_empty());
        assert!(out.iter().all(|e| e.event.resolved.transpose == 4.0));
    }

    #[test]
    fn graph_update_emit_shapes_velocity_per_hop_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        // A self-looping node seeded (note 10, vel 1.0). Its :update emits via the terse
        // self-less surface, relaying the note unchanged and halving velocity each hop.
        // Because the emitted payload is what rides the scatter, the decayed velocity
        // feeds the next boundary's `in-vel` — proving per-hop velocity accumulation is
        // expressible purely in the DSL (the velocity analogue of the transpose cascade).
        let manifest = GraphManifest {
            id: 31,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
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
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some("(emit :note (in-note) :vel (* (in-vel) 0.5))".into()),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 10.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert!(out.len() >= 3, "expected several hops, got {}", out.len());
        // Note relays unchanged (emit named :note (in-note), no transpose applied).
        assert!(out.iter().all(|e| e.event.resolved.transpose == 10.0));
        // Velocity halves each hop and the decayed value propagates: 0.5, 0.25, 0.125, …
        let vels: Vec<f32> = out.iter().map(|e| e.event.resolved.velocity).collect();
        assert_eq!(vels[0], 0.5);
        assert_eq!(vels[1], 0.25);
        assert_eq!(vels[2], 0.125);
    }

    #[test]
    fn graph_update_can_reset_transpose_cascade_through_vm() {
        use crate::graph::{
            GraphDurationSpec, GraphManifest, GraphPayload, NodeEval, NodeProto, ShapeSpec,
        };
        use crate::sequencer::Timebase;

        let update_source = "(emit :note (if (>= (param :transpose-reset) 1) (param :transpose) (+ (in-note) (param :transpose))) :vel (in-vel))";
        let manifest = GraphManifest {
            id: 33,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: GraphDurationSpec::Steps { value: 1.0 },
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                update_source: Some(update_source.into()),
                ..NodeProto::default()
            },
            edge_sets: vec![],
        };
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        let eval_with_reset = |transpose_reset| {
            let mut params = HashMap::new();
            params.insert("transpose".to_string(), 7.0);
            params.insert("transpose-reset".to_string(), transpose_reset);
            NodeEval {
                node_index: 0,
                input: 1.0,
                energy: 1.0,
                tick_index: 0,
                beat: 0.0,
                resolution: Timebase::Quarter,
                delay_steps: 1,
                input_event: Some(GraphPayload {
                    note: 12.0,
                    velocity: 0.5,
                    duration_beats: 0.25,
                }),
                params,
            }
        };

        let carried = scratch
            .invoke_graph_update(&manifest, &eval_with_reset(0.0))
            .expect("invoke transpose carry update");
        assert_eq!(carried.emit.and_then(|emit| emit.note), Some(19.0));

        let reset = scratch
            .invoke_graph_update(&manifest, &eval_with_reset(1.0))
            .expect("invoke transpose reset update");
        assert_eq!(reset.emit.and_then(|emit| emit.note), Some(7.0));
    }

    #[test]
    fn graph_update_emit_shapes_duration_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = GraphManifest {
            id: 33,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Sixteenth,
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some(
                    "(emit :note (in-note) :vel (in-vel) :dur (+ (steps 0.5) (* 0.25 (in-dur)) (* 0.25 (event-dur (in-event)))))"
                        .into(),
                ),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 10.0,
                velocity: 1.0,
                duration_beats: 0.75,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );

        assert!(!out.is_empty());
        assert_eq!(out[0].event.resolved.duration, 0.5);
    }

    #[test]
    fn graph_update_emit_shapes_swing_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = GraphManifest {
            id: 34,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Sixteenth,
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some(
                    "(emit :note (in-note) :vel (in-vel) :swing (swing 75 :16))".into(),
                ),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 10.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );

        assert!(!out.is_empty());
        assert_eq!(out[0].sample_time, 18_000);
    }

    #[test]
    fn graph_update_can_request_graph_state_reset_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphManifest, GraphPayload, NodeProto, ParamSpec, SeedFrom, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = GraphManifest {
            id: 32,
            name: "g".into(),
            shape: ShapeSpec::Line(1),
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
                seed_from: SeedFrom::Tracks(vec![0]),
                update_source: Some(
                    "(if (> (input) 0) (do (reset-graph-state) (emit :note (in-note) :vel 1)) nil)"
                        .into(),
                ),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut runtime = manifest.materialize();
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 9.0,
                velocity: 0.25,
                duration_beats: 0.25,
            },
        );
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );

        assert!(
            out.len() >= 3,
            "reset should preserve the firing's own self-loop scatter; got {out:?}"
        );
        assert!(out
            .iter()
            .all(|emission| emission.event.resolved.transpose == 9.0));
        assert!(out
            .iter()
            .all(|emission| emission.event.resolved.velocity == 1.0));
        assert_eq!(runtime.pending_count_for_node(0), Some(1));
        assert_eq!(runtime.energy(0), 0.0);
    }

    #[test]
    fn graph_update_dampen_and_recover_incoming_through_vm() {
        use crate::graph::{
            EdgeSetSpec, GraphEdge, GraphManifest, GraphPayload, NodeProto, ParamSpec, ShapeSpec,
            Topology,
        };
        use crate::sequencer::Timebase;

        let manifest = GraphManifest {
            id: 17,
            name: "g".into(),
            shape: ShapeSpec::Line(2),
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
                params: vec![
                    ParamSpec {
                        name: "threshold".into(),
                        min: 0.0,
                        max: 4.0,
                        default: 1.0,
                        is_int: false,
                    },
                    ParamSpec {
                        name: "dampening".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        is_int: false,
                    },
                    ParamSpec {
                        name: "recovery".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        is_int: false,
                    },
                ],
                update_source: Some(
                    "(if (>= (node-state self :energy) (node-param self :threshold)) (do (dampen-incoming self (node-param self :dampening)) true) (do (recover-incoming self (node-param self :recovery)) false))"
                        .into(),
                ),
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
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut seed_node = crate::graph::GraphNode {
            resolution: Timebase::Quarter,
            ..crate::graph::GraphNode::default()
        };
        seed_node.seed_track_mask = crate::graph::seed_track_mask(&[0]);
        let param_defaults = manifest
            .node
            .params
            .iter()
            .map(|param| (param.name.clone(), param.default))
            .collect::<HashMap<_, _>>();
        let mut runtime = crate::graph::GraphRuntime::new_with_config(
            17,
            "g".into(),
            vec![
                seed_node,
                crate::graph::GraphNode {
                    resolution: Timebase::Quarter,
                    ..crate::graph::GraphNode::default()
                },
            ],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
            0,
            NeuralMaxPolySelection::Deterministic,
            crate::graph::GraphDurationSpec::default(),
            crate::graph::GraphSwingSpec::default(),
            vec![param_defaults.clone(), param_defaults],
        );
        runtime.seed(0, 0.0, GraphPayload::default());
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(runtime.edge_dampening(0), Some(0.5));

        runtime.process_block(
            1.0,
            2.0,
            48_000,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut out,
        );
        assert_eq!(runtime.edge_dampening(0), Some(0.25));
    }

    fn register_graph_def_sequencer_test_native(runtime: &mut Runtime, state: Arc<SequencerState>) {
        runtime.register_native("def-sequencer", move |args, _ctx| {
            let name = match args.first() {
                Some(Value::String(s) | Value::Symbol(s) | Value::Keyword(s)) => {
                    s.trim_start_matches('@').to_string()
                }
                _ => return Err("def-sequencer expects a name".to_string()),
            };
            if !super::graph_mode_present(&args) {
                return Err("test def-sequencer native only supports graph mode".to_string());
            }
            let manifest = super::parse_graph_manifest(&args)?;
            state.publish_sequencer(PublishedSequencer {
                id: manifest.id,
                name: name.clone(),
                resolution: Timebase::Sixteenth as u8,
                tick_source: String::new(),
                graph: Some(manifest),
            });
            Ok(Value::String(name))
        });
    }

    #[test]
    fn graph_authoring_buffer_overrides_routes_and_emits_multiple_tracks() {
        use crate::graph::GraphPayload;

        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut authoring = Runtime::new();
        register_graph_def_sequencer_test_native(&mut authoring, Arc::clone(&state));
        register_graph_authoring_natives(&mut authoring, Arc::clone(&state));

        authoring
            .eval_str(
                r#"
                (def-sequencer "graph-route-e2e"
                  :shape (line 4)
                  :energy-decay 1
                  :reset-every 0
                  :seed-on-reset 0
                  :max-poly 4
                  :max-poly-selection :deterministic

                  (def-node nrn
                    :resolution :16
                    :delay 1
                    :quantize :16
                    :route 0
                    :seed-from ()
                    :reduce :sum
                    :params ((threshold :float 0 4 :default 0.5)
                             (transpose :int -48 48 :default 0))
                    :state ((energy :leak (per-step :energy-decay)))
                    :update (>= (node-state self :energy)
                                (node-param self :threshold)))

                  (edges
                    :from nrn
                    :to nrn
                    :topology (all-to-all)
                    :gather (edge :weight)
                    :params ((weight :float -1 1 :default 1))))

                (graph-node "graph-route-e2e" 0 :route 0 :seed-from 0)
                (graph-node "graph-route-e2e" 1 :route 1)
                (graph-node "graph-route-e2e" 2 :route 2)
                (graph-node "graph-route-e2e" 3 :route 3)
                "#,
            )
            .expect("evaluate graph authoring buffer");

        let published = state.published_sequencers();
        let manifest = published
            .iter()
            .find_map(|published| published.graph.clone())
            .expect("published graph manifest");
        let graph_overrides = state.current_graph_overrides();
        let overrides = graph_overrides
            .iter()
            .find(|overrides| overrides.sequencer_name == manifest.name)
            .expect("graph overrides");
        assert_eq!(overrides.node_intrinsics.len(), 4);
        assert!(matches!(
            overrides.node_intrinsics[1].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(1))
        ));
        assert!(matches!(
            overrides.node_intrinsics[2].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(2))
        ));
        assert!(matches!(
            overrides.node_intrinsics[3].route,
            Some(crate::graph::ProjectGraphRouteOverride::Track(3))
        ));

        let mut graph_runtime = manifest.materialize_with_overrides(Some(overrides));
        graph_runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        let mut emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut emissions,
        );

        let mut tracks = emissions
            .iter()
            .filter_map(|emission| emission.event.track)
            .collect::<Vec<_>>();
        tracks.sort_unstable();
        tracks.dedup();
        assert_eq!(tracks, vec![0, 1, 2, 3]);
    }

    #[test]
    fn graph_8x8_demo_ui_exposes_node_param_controls_and_weight_matrix() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
                ("track-events", Value::List(Vec::new())),
                ("track-event-current-beat", Value::Number(0.0)),
                ("track-colors", Value::List(Vec::new())),
                (
                    "track-active-notes",
                    Value::List(
                        (0..8)
                            .map(|_| Rc::new(RefCell::new(Value::List(Vec::new()))))
                            .collect(),
                    ),
                ),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer) nil)")
            .expect("install sequencer tab registration test stub");
        runtime
            .eval_str(
                r#"
                (def graph-8x8-registered-tab nil)
                (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab
                  (label buffer sequencer source-path)
                  (set! graph-8x8-registered-tab
                    (list label buffer sequencer source-path)))
                "#,
            )
            .expect("install script sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-8x8-demo.lisp"))
        .expect("read graph 8x8 demo script");
        runtime.eval_str(&source).expect("evaluate graph 8x8 demo");
        assert_eq!(
            runtime
                .eval_str("graph-8x8-registered-tab")
                .expect("read graph tab registration"),
            Some(Value::List(
                [
                    "8x8",
                    "*8x8*",
                    "neural-8x8-demo",
                    "",
                ]
                .into_iter()
                .map(|value| Rc::new(RefCell::new(Value::String(value.to_string()))))
                .collect(),
            )),
            "the script must reach tab registration after publishing its graph and UI"
        );
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the graph demo must publish graph/UI without writing pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published graph manifest");
        assert_eq!(
            manifest.shape.num_nodes(),
            8,
            "the demo matrix must cover every materialized node"
        );

        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("graph 8x8 script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((40.0, 48.0)))
            .expect("graph 8x8 widget tree should lay out");

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected editable weight matrix plus trigger/energy/dampening telemetry"
        );
        for matrix in &matrices {
            assert_measured(matrix);
        }

        let matrix = find_by_stable_key(&layout, "graph-8x8-weight-matrix")
            .expect("weight matrix stable key");
        assert_measured(matrix);
        for key in [
            "graph-8x8-trigger-matrix",
            "graph-8x8-energy-matrix",
            "graph-8x8-dampening-matrix",
            "graph-8x8-event-view",
            "graph-8x8-track-event-view",
            "graph-8x8-piano",
        ] {
            let widget =
                find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("missing widget {key}"));
            assert_measured(widget);
        }
        let mut event_views = Vec::new();
        collect_widgets(&layout, "event-view", &mut event_views);
        assert_eq!(event_views.len(), 2, "expected two event-view widgets");
        let mut piano_keyboards = Vec::new();
        collect_widgets(&layout, "piano-keyboard", &mut piano_keyboards);
        assert_eq!(
            piano_keyboards.len(),
            1,
            "expected one aggregate track piano keyboard"
        );
        assert_eq!(
            piano_keyboards[0].props.get("key-count"),
            Some(&Value::Number(80.0))
        );
        assert_eq!(
            piano_keyboards[0].props.get("tracks"),
            Some(&Value::List(
                (0..8)
                    .map(|track| Rc::new(RefCell::new(Value::Number(track as f64))))
                .collect(),
            ))
        );
        assert_eq!(
            piano_keyboards[0].props.get("overlap-mode"),
            Some(&Value::Keyword("loudest".to_string()))
        );
        let activity = |note: f64, velocity: f64| {
            Rc::new(RefCell::new(Value::Map(std::collections::HashMap::from([
                (
                    "note".to_string(),
                    Rc::new(RefCell::new(Value::Number(note))),
                ),
                (
                    "velocity".to_string(),
                    Rc::new(RefCell::new(Value::Number(velocity))),
                ),
                (
                    "trigger-id".to_string(),
                    Rc::new(RefCell::new(Value::Number(note))),
                ),
            ]))))
        };
        let active_notes = Value::List(
            (0..8)
                .map(|track| {
                    Rc::new(RefCell::new(Value::List(
                        if track == 0 {
                            vec![activity(60.0, 0.4)]
                        } else if track == 7 {
                            vec![activity(60.0, 0.9), activity(67.0, 0.7)]
                        } else {
                            Vec::new()
                        },
                    )))
                })
                .collect(),
        );
        runtime.set_reactive("SEQ", "track-active-notes", active_notes.clone());
        runtime.run_reactive_cycle();
        let updated_tree = runtime
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("active-note update should republish the graph keyboard");
        let updated_layout = runtime
            .layout_snapshot_for_tree_with_viewport(&updated_tree, Some((40.0, 48.0)))
            .expect("updated graph keyboard should lay out");
        let updated_piano = find_by_stable_key(&updated_layout, "graph-8x8-piano")
            .expect("updated piano stable key");
        assert_eq!(
            updated_piano.props.get("notes-by-track"),
            Some(&active_notes),
            "aggregate note activity must reach the piano widget reactively"
        );

        // Five number-pickers per node (delay + transpose + vel-decay + dampening +
        // recovery) plus the three top-of-panel pickers (reset-bars + max-poly +
        // piano press depth); three dropdowns per node (route + resolution + quantize).
        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            8 * 5 + 3,
            "expected per-node controls + reset-bars + max-poly + piano press depth"
        );
        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            24,
            "expected route + resolution + quantize per node"
        );
        for key in [
            "graph-8x8-reset-bars",
            "graph-8x8-max-poly",
            "graph-8x8-piano-press-depth",
        ] {
            let widget = find_by_stable_key(&layout, key)
                .unwrap_or_else(|| panic!("missing config control {key}"));
            assert_measured(widget);
        }
        for idx in 0..8 {
            for key in [
                format!("graph-8x8-route-{idx}"),
                format!("graph-8x8-delay-{idx}"),
                format!("graph-8x8-transpose-{idx}"),
                format!("graph-8x8-vel-decay-{idx}"),
                format!("graph-8x8-dampening-{idx}"),
                format!("graph-8x8-recovery-{idx}"),
                format!("graph-8x8-resolution-{idx}"),
                format!("graph-8x8-quantize-{idx}"),
            ] {
                let widget = find_by_stable_key(&layout, &key)
                    .unwrap_or_else(|| panic!("missing control {key}"));
                assert_measured(widget);
            }
        }
        let quantize_options = find_by_stable_key(&layout, "graph-8x8-quantize-0")
            .and_then(|node| node.props.get("options"))
            .expect("quantize options");
        let Value::List(options) = quantize_options else {
            panic!("quantize options should be a list");
        };
        for label in ["2T", "4T", "8T", "16T", "32T", "64T"] {
            assert!(
                options.iter().any(
                    |option| matches!(&*option.borrow(), Value::String(value) if value == label)
                ),
                "missing quantize triplet option {label}"
            );
        }

        runtime
            .eval_str("(g8-init-ring-defaults)")
            .expect("explicitly initialize graph demo defaults");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after explicit init");
        assert_eq!(
            graph.edge_params.len(),
            64,
            "explicit init should write the full ring weight matrix"
        );
        assert!(
            graph.edge_params.iter().any(|edge| {
                edge.from == 0 && edge.to == 1 && edge.param == "weight" && edge.value == 1.0
            }),
            "explicit init should write the first ring edge"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 0
                    && node.seed_from == Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0]))
            }),
            "explicit init should seed node 0 from track 0"
        );

        // transpose picker -> per-node behavioral param override.
        let transpose_change = find_by_stable_key(&layout, "graph-8x8-transpose-2")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("transpose callback");
        runtime
            .invoke(transpose_change, vec![Value::Number(7.0)])
            .expect("invoke transpose callback");
        // vel-decay picker -> per-node behavioral param override (the velocity analogue).
        let vel_change = find_by_stable_key(&layout, "graph-8x8-vel-decay-5")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("vel-decay callback");
        runtime
            .invoke(vel_change, vec![Value::Number(0.5)])
            .expect("invoke vel-decay callback");
        // resolution dropdown -> per-node intrinsic override.
        let resolution_change = find_by_stable_key(&layout, "graph-8x8-resolution-3")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("resolution callback");
        runtime
            .invoke(resolution_change, vec![Value::String("8".into())])
            .expect("invoke resolution callback");
        // route dropdown -> per-node intrinsic route override.
        let route_change = find_by_stable_key(&layout, "graph-8x8-route-4")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("route callback");
        runtime
            .invoke(route_change, vec![Value::String("Track 3".into())])
            .expect("invoke route callback");

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides");
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 2 && param.param == "transpose" && param.value == 7.0
            }),
            "transpose knob should write a node param override"
        );
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 5 && param.param == "vel-decay" && param.value == 0.5
            }),
            "vel-decay knob should write a node param override"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 3
                    && node.resolution == Some(vec![crate::sequencer::Timebase::Eighth as u8])
            }),
            "resolution dropdown should write an intrinsic override"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 4
                    && node.route == Some(crate::graph::ProjectGraphRouteOverride::Track(2))
            }),
            "route dropdown should write an intrinsic override"
        );

        {
            let mut bank = state.export_pattern_repository();
            let mut pattern = bank[0].clone();
            let graph = pattern
                .graph_overrides
                .iter_mut()
                .find(|graph| graph.sequencer_name == "neural-8x8-demo")
                .expect("cloned graph overrides");
            graph
                .node_params
                .push(crate::graph::ProjectGraphNodeParamOverride {
                    group: "nrn".to_string(),
                    instance: 2,
                    param: "transpose".to_string(),
                    value: -12.0,
                });
            graph
                .node_intrinsics
                .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 3,
                    resolution: None,
                    delay_steps: Some(6),
                    quantize: None,
                    route: None,
                    seed_from: None,
                    seed_on_reset: None,
                    duration: None,
                    swing: None,
                    neural_group: None,
                });
            graph
                .node_intrinsics
                .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 4,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: Some(crate::graph::ProjectGraphRouteOverride::Track(0)),
                    seed_from: None,
                    seed_on_reset: None,
                    duration: None,
                    swing: None,
                    neural_group: None,
                });
            graph
                .edge_params
                .push(crate::graph::ProjectGraphEdgeParamOverride {
                    group: "nrn->nrn".to_string(),
                    from: 0,
                    to: 1,
                    param: "weight".to_string(),
                    value: 0.25,
                });
            bank.push(pattern);
            state.replace_pattern_repository(bank, 1);
        }
        runtime.set_reactive("SEQ", "current-pattern", Value::Number(1.0));
        runtime.run_reactive_cycle();
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 2 :transpose))")
                .expect("read bound transpose value"),
            Some(Value::Number(-12.0)),
            "pattern switch should reload transpose control state"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 3 :delay))")
                .expect("read bound delay value"),
            Some(Value::Number(6.0)),
            "pattern switch should reload delay control state"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 4 :route g8-route-options))")
                .expect("read bound route index"),
            Some(Value::Number(0.0)),
            "pattern switch should display internal route 0 as Track 1 (index 0)"
        );
        assert_eq!(
            runtime
                .eval_str("(nth (nth g8-weights 0) 1)")
                .expect("read synced weight"),
            Some(Value::Number(0.25)),
            "pattern switch should reload matrix state"
        );
        let bank = state.export_pattern_repository();
        state.replace_pattern_repository(bank, 0);
        runtime.set_reactive("SEQ", "current-pattern", Value::Number(0.0));
        runtime.run_reactive_cycle();

        let mut graph_runtime = manifest.materialize_with_overrides(Some(graph));
        assert_eq!(graph_runtime.seed_track_mask_for_node(0), Some(1));
        let seeded = graph_runtime.seed(
            0,
            0.0,
            crate::graph::GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        assert_eq!(seeded, 1, "track 0 should seed node 0 exactly once");
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(8),
            fallback_instrument_descriptors(8),
            0,
            0,
        );
        let mut chunked_emissions = Vec::new();
        let mut start_beats = 0.0_f64;
        let mut eval_count = 0_usize;
        let mut max_input = 0.0_f64;
        let mut max_energy = 0.0_f64;
        while start_beats < 1.0 {
            let end_beats = (start_beats + 0.021_f64).min(1.0_f64);
            graph_runtime.process_block(
                start_beats,
                end_beats,
                0,
                48_000.0,
                manifest.max_poly,
                |eval| {
                    eval_count += 1;
                    max_input = max_input.max(eval.input);
                    max_energy = max_energy.max(eval.energy);
                    scratch
                        .invoke_graph_update(&manifest, eval)
                        .expect("demo graph update should evaluate")
                },
                &mut chunked_emissions,
            );
            start_beats = end_beats;
        }
        assert!(eval_count > 0, "chunked graph drive should evaluate nodes");
        assert!(
            !chunked_emissions.is_empty(),
            "track-0 seed should propagate through the ring under chunked graph drive; evals={eval_count} max_input={max_input} max_energy={max_energy} edge_overrides={}",
            graph.edge_params.len()
        );

        let matrix_cell_change = matrix
            .props
            .get("on-cell-change")
            .cloned()
            .expect("matrix cell callback");
        // A single cell edit writes exactly one edge override; (from=3, to=4) == 0.5.
        runtime
            .invoke(
                matrix_cell_change.clone(),
                vec![gv_num(3.0), gv_num(4.0), gv_num(0.5)],
            )
            .expect("invoke matrix cell callback");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after matrix edit");
        assert_eq!(graph.edge_params.len(), 64);
        assert!(graph.edge_params.iter().any(|edge| {
            edge.from == 3 && edge.to == 4 && edge.param == "weight" && edge.value == 0.5
        }));

        // Zero every weight one cell at a time (the per-cell edit path) to silence the net.
        for r in 0..8 {
            for c in 0..8 {
                runtime
                    .invoke(
                        matrix_cell_change.clone(),
                        vec![gv_num(r as f64), gv_num(c as f64), gv_num(0.0)],
                    )
                    .expect("invoke zero matrix cell callback");
            }
        }

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-demo")
            .expect("graph overrides after zero matrix edit");
        let mut graph_runtime = manifest.materialize_with_overrides(Some(graph));
        graph_runtime.seed(
            0,
            0.0,
            crate::graph::GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(8),
            fallback_instrument_descriptors(8),
            0,
            0,
        );
        let mut emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            manifest.max_poly,
            |eval| {
                scratch
                    .invoke_graph_update(&manifest, eval)
                    .unwrap_or_default()
            },
            &mut emissions,
        );
        assert!(
            emissions.is_empty(),
            "zero matrix should silence graph propagation"
        );
    }

    #[test]
    fn graph_8x8_reset_demo_ui_exposes_reset_and_global_timing_controls() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer) nil)")
            .expect("install sequencer tab registration test stub");
        runtime
            .eval_str(
                "(def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon) nil)",
            )
            .expect("install script sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-8x8-reset-demo.lisp"))
        .expect("read graph 8x8 reset demo script");
        runtime
            .eval_str(&source)
            .expect("evaluate graph 8x8 reset demo");
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the reset demo must publish graph/UI without writing pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published reset graph manifest");
        assert_eq!(manifest.name, "neural-8x8-reset-demo");
        assert_eq!(manifest.shape.num_nodes(), 8);
        assert_eq!(manifest.node.param_default("global-transpose"), Some(0.0));
        assert_eq!(manifest.node.param_default("transpose-reset"), Some(0.0));
        assert_eq!(manifest.node.param_default("dur-factor"), Some(1.0));
        assert_eq!(manifest.node.param_default("vel-reset"), Some(0.0));

        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("graph 8x8 reset script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((64.0, 52.0)))
            .expect("graph 8x8 reset widget tree should lay out");

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected editable weight matrix plus trigger/energy/dampening telemetry"
        );
        for matrix in &matrices {
            assert_measured(matrix);
        }

        let mut event_views = Vec::new();
        collect_widgets(&layout, "event-view", &mut event_views);
        assert_eq!(event_views.len(), 1, "expected graph event history view");
        assert_measured(event_views[0]);

        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            8 * 5 + 4,
            "expected per-node numeric controls plus reset/max/global transpose/duration"
        );
        let mut toggles = Vec::new();
        collect_widgets(&layout, "toggle", &mut toggles);
        assert_eq!(
            toggles.len(),
            8 * 2,
            "expected transpose-reset and vel-reset toggles per node"
        );
        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            8 * 3 + 2,
            "expected per-node route/resolution/quantize plus delay and res/q scale factors"
        );

        for key in [
            "graph-8x8-reset-global-transpose",
            "graph-8x8-reset-dur-factor",
            "graph-8x8-reset-delay-factor",
            "graph-8x8-reset-timebase-factor",
            "graph-8x8-reset-weight-matrix",
            "graph-8x8-reset-event-view",
        ] {
            let widget = find_by_stable_key(&layout, key)
                .unwrap_or_else(|| panic!("missing reset/global control {key}"));
            assert_measured(widget);
        }
        for idx in 0..8 {
            for key in [
                format!("graph-8x8-reset-transpose-reset-{idx}"),
                format!("graph-8x8-reset-vel-reset-{idx}"),
                format!("graph-8x8-reset-resolution-{idx}"),
                format!("graph-8x8-reset-quantize-{idx}"),
            ] {
                let widget = find_by_stable_key(&layout, &key)
                    .unwrap_or_else(|| panic!("missing reset fork control {key}"));
                assert_measured(widget);
            }
        }

        let global_transpose_change =
            find_by_stable_key(&layout, "graph-8x8-reset-global-transpose")
                .and_then(|node| node.props.get("on-change"))
                .cloned()
                .expect("global transpose callback");
        runtime
            .invoke(global_transpose_change, vec![Value::Number(12.0)])
            .expect("invoke global transpose callback");

        let dur_factor_change = find_by_stable_key(&layout, "graph-8x8-reset-dur-factor")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("duration factor callback");
        runtime
            .invoke(dur_factor_change, vec![Value::Number(2.0)])
            .expect("invoke duration factor callback");

        let transpose_reset_change =
            find_by_stable_key(&layout, "graph-8x8-reset-transpose-reset-2")
                .and_then(|node| node.props.get("on-change"))
                .cloned()
                .expect("transpose reset callback");
        runtime
            .invoke(transpose_reset_change, vec![Value::Bool(true)])
            .expect("invoke transpose reset callback");

        let vel_reset_change = find_by_stable_key(&layout, "graph-8x8-reset-vel-reset-3")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("velocity reset callback");
        runtime
            .invoke(vel_reset_change, vec![Value::Bool(true)])
            .expect("invoke velocity reset callback");

        let delay_factor_change = find_by_stable_key(&layout, "graph-8x8-reset-delay-factor")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("delay factor callback");
        runtime
            .invoke(delay_factor_change, vec![Value::String("2".to_string())])
            .expect("invoke delay factor callback");

        let timebase_factor_change = find_by_stable_key(&layout, "graph-8x8-reset-timebase-factor")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("timebase factor callback");
        runtime
            .invoke(timebase_factor_change, vec![Value::String("2".to_string())])
            .expect("invoke timebase factor callback");

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-8x8-reset-demo")
            .expect("reset graph overrides");
        assert_eq!(
            graph
                .node_params
                .iter()
                .filter(|param| param.param == "global-transpose" && param.value == 12.0)
                .count(),
            8,
            "global transpose should write every node param"
        );
        assert_eq!(
            graph
                .node_params
                .iter()
                .filter(|param| param.param == "dur-factor" && param.value == 2.0)
                .count(),
            8,
            "duration factor should write every node param"
        );
        assert!(graph.node_params.iter().any(|param| {
            param.instance == 2 && param.param == "transpose-reset" && param.value == 1.0
        }));
        assert!(graph.node_params.iter().any(|param| {
            param.instance == 3 && param.param == "vel-reset" && param.value == 1.0
        }));
        assert_eq!(
            graph
                .node_intrinsics
                .iter()
                .filter(|node| {
                    node.delay_steps == Some(2)
                        && node.resolution
                            == Some(vec![crate::sequencer::Timebase::ThirtySecond as u8])
                        && node.quantize
                            == Some(crate::graph::ProjectGraphQuantizeOverride::Timebase(vec![
                                crate::sequencer::Timebase::ThirtySecond as u8,
                            ]))
                })
                .count(),
            8,
            "delay and res/q factors should batch-edit all node intrinsics"
        );
    }

    #[test]
    fn graph_variable_reset_demo_tracks_active_node_count_and_dormant_overrides() {
        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn assert_number_prop(node: &eseqlisp::layout::LayoutNode, prop: &str, expected: f64) {
            assert_eq!(
                node.props.get(prop),
                Some(&Value::Number(expected)),
                "expected {} {:?} to be {expected}",
                node.widget_type,
                prop
            );
        }

        fn assert_number_prop_close(
            node: &eseqlisp::layout::LayoutNode,
            prop: &str,
            expected: f64,
        ) {
            let Some(Value::Number(actual)) = node.props.get(prop) else {
                panic!(
                    "expected {} {:?} to be number {expected}, got {:?}",
                    node.widget_type,
                    prop,
                    node.props.get(prop)
                );
            };
            assert!(
                (actual - expected).abs() < 0.001,
                "expected {} {:?} to be {expected}, got {actual}",
                node.widget_type,
                prop
            );
        }

        fn value_list(items: Vec<Value>) -> Value {
            Value::List(
                items
                    .into_iter()
                    .map(|value| Rc::new(RefCell::new(value)))
                    .collect(),
            )
        }

        fn test_track_colors() -> Value {
            let palette = [
                [0.96, 0.28, 0.52],
                [0.25, 0.56, 0.98],
                [0.28, 0.84, 0.54],
                [0.96, 0.72, 0.24],
                [0.66, 0.42, 0.96],
                [0.26, 0.78, 0.84],
                [0.98, 0.44, 0.28],
                [0.76, 0.82, 0.30],
                [0.52, 0.48, 0.98],
                [0.98, 0.36, 0.70],
                [0.38, 0.72, 0.42],
                [0.90, 0.58, 0.22],
                [0.40, 0.64, 0.90],
                [0.72, 0.46, 0.82],
                [0.30, 0.76, 0.68],
                [0.86, 0.34, 0.34],
            ];
            value_list(
                palette
                    .iter()
                    .map(|color| {
                        value_list(
                            color
                                .iter()
                                .map(|channel| Value::Number(*channel))
                                .collect(),
                        )
                    })
                    .collect(),
            )
        }

        fn assert_reactive_number(runtime: &mut Runtime, expr: &str, expected: f64) {
            assert_eq!(
                runtime.eval_str(expr).expect("read reactive number"),
                Some(Value::Number(expected)),
                "{expr}"
            );
        }

        fn latest_layout(runtime: &mut Runtime) -> std::sync::Arc<eseqlisp::layout::LayoutNode> {
            let tree = runtime
                .take_pending_buffer_widget_trees()
                .into_iter()
                .rev()
                .find_map(|pending| match pending {
                    eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                    eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
                })
                .expect("script should publish widget tree");
            runtime
                .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 64.0)))
                .expect("variable graph widget tree should lay out")
        }

        fn assert_active_layout(layout: &eseqlisp::layout::LayoutNode, count: usize) {
            for key in [
                "graph-variable-reset-node-count",
                "graph-variable-reset-max-poly-selection",
                "graph-variable-reset-threshold",
                "graph-variable-reset-route-color-0",
                "graph-variable-reset-seed-route-0",
                "graph-variable-reset-reset-seed-0",
                "graph-variable-reset-trigger-matrix",
                "graph-variable-reset-energy-matrix",
                "graph-variable-reset-weight-matrix",
                "graph-variable-reset-dampening-matrix",
            ] {
                let widget =
                    find_by_stable_key(layout, key).unwrap_or_else(|| panic!("missing {key}"));
                assert_measured(widget);
            }
            let weight = find_by_stable_key(layout, "graph-variable-reset-weight-matrix").unwrap();
            assert_number_prop(weight, "rows", count as f64);
            assert_number_prop(weight, "cols", count as f64);
            assert!(weight.props.contains_key("on-cell-press"));
            assert!(weight.props.contains_key("on-cell-release"));
            let trigger =
                find_by_stable_key(layout, "graph-variable-reset-trigger-matrix").unwrap();
            assert_number_prop(trigger, "rows", count as f64);
            assert_number_prop(trigger, "cols", 1.0);
            let energy = find_by_stable_key(layout, "graph-variable-reset-energy-matrix").unwrap();
            let expected_matrix_height = count as f64 + (count.saturating_sub(1) as f64 * 0.2);
            for matrix in [trigger, energy, weight] {
                assert_number_prop_close(matrix, "height", expected_matrix_height);
            }
            let first_row = find_by_stable_key(layout, "graph-variable-reset-transpose-0")
                .expect("missing first active row control");
            let first_row_highlight =
                find_by_stable_key(layout, "graph-variable-reset-row-0")
                    .expect("missing first active row highlight");
            assert_measured(first_row_highlight);
            let active_row_key = format!("graph-variable-reset-transpose-{}", count - 1);
            let final_row = find_by_stable_key(layout, &active_row_key)
                .unwrap_or_else(|| panic!("missing final active row control {active_row_key}"));
            let active_bar_key = format!("graph-variable-reset-route-color-{}", count - 1);
            assert!(
                find_by_stable_key(layout, &active_bar_key).is_some(),
                "missing final active route color bar {active_bar_key}"
            );
            let active_seed_route_key = format!("graph-variable-reset-seed-route-{}", count - 1);
            assert!(
                find_by_stable_key(layout, &active_seed_route_key).is_some(),
                "missing final active seed route toggle {active_seed_route_key}"
            );
            let active_reset_seed_key = format!("graph-variable-reset-reset-seed-{}", count - 1);
            assert!(
                find_by_stable_key(layout, &active_reset_seed_key).is_some(),
                "missing final active reset seed toggle {active_reset_seed_key}"
            );
            let inactive_row_key = format!("graph-variable-reset-transpose-{count}");
            assert!(
                find_by_stable_key(layout, &inactive_row_key).is_none(),
                "inactive row control {inactive_row_key} should not be visible"
            );
            let inactive_bar_key = format!("graph-variable-reset-route-color-{count}");
            assert!(
                find_by_stable_key(layout, &inactive_bar_key).is_none(),
                "inactive route color bar {inactive_bar_key} should not be visible"
            );
            let inactive_seed_route_key = format!("graph-variable-reset-seed-route-{count}");
            assert!(
                find_by_stable_key(layout, &inactive_seed_route_key).is_none(),
                "inactive seed route toggle {inactive_seed_route_key} should not be visible"
            );
            let inactive_reset_seed_key = format!("graph-variable-reset-reset-seed-{count}");
            assert!(
                find_by_stable_key(layout, &inactive_reset_seed_key).is_none(),
                "inactive reset seed toggle {inactive_reset_seed_key} should not be visible"
            );
        }

        let state = Arc::new(SequencerState::new(
            16,
            (0..16).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
                ("track-colors", test_track_colors()),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer) nil)")
            .expect("install sequencer tab registration test stub");
        runtime
            .eval_str(
                "(def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon) nil)",
            )
            .expect("install script sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-variable-reset-demo.lisp"))
        .expect("read graph variable reset demo script");
        runtime
            .eval_str(&source)
            .expect("evaluate graph variable reset demo");
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the variable demo must publish graph/UI without writing pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published variable graph manifest");
        assert_eq!(manifest.name, "neural-variable-reset-demo");
        assert_eq!(manifest.shape.num_nodes(), 8);
        assert_eq!(manifest.shape.capacity_num_nodes(), 16);

        let layout = latest_layout(&mut runtime);
        assert_active_layout(&layout, 8);
        let cell_press = find_by_stable_key(&layout, "graph-variable-reset-weight-matrix")
            .and_then(|node| node.props.get("on-cell-press"))
            .cloned()
            .expect("weight matrix cell-press callback");
        let cell_release = find_by_stable_key(&layout, "graph-variable-reset-weight-matrix")
            .and_then(|node| node.props.get("on-cell-release"))
            .cloned()
            .expect("weight matrix cell-release callback");
        runtime
            .invoke(
                cell_press,
                vec![Value::Number(2.0), Value::Number(3.0)],
            )
            .expect("select destination neuron 3 from source neuron 2");
        runtime.run_reactive_cycle();
        let layout = latest_layout(&mut runtime);
        let source_row = find_by_stable_key(&layout, "graph-variable-reset-row-2")
            .expect("source row highlight");
        let destination_row = find_by_stable_key(&layout, "graph-variable-reset-row-3")
            .expect("destination row highlight");
        assert_eq!(source_row.props.get("selected"), Some(&Value::Bool(false)));
        assert_eq!(destination_row.props.get("selected"), Some(&Value::Bool(true)));
        assert_eq!(
            destination_row.props.get("selected-background-color"),
            Some(&Value::Keyword("mixer-strip-selected-bg".to_string()))
        );
        assert_eq!(
            runtime.eval_str("gvr-selected-neuron").expect("selected neuron"),
            Some(Value::Number(3.0))
        );
        runtime
            .invoke(
                cell_release,
                vec![Value::Number(2.0), Value::Number(3.0)],
            )
            .expect("release destination neuron 3");
        runtime.run_reactive_cycle();
        let layout = latest_layout(&mut runtime);
        let released_row = find_by_stable_key(&layout, "graph-variable-reset-row-3")
            .expect("released destination row");
        assert_eq!(released_row.props.get("selected"), Some(&Value::Bool(false)));
        assert_eq!(
            runtime.eval_str("gvr-selected-neuron").expect("released neuron"),
            Some(Value::Number(-1.0))
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 0 \"active\")))",
            1.0,
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 0 \"r\")))",
            0.96,
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph gvr-name 0 :seed-route))")
                .expect("default seed route"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph gvr-name 0 :seed-on-reset))")
                .expect("default reset seed"),
            Some(Value::Number(0.0))
        );
        let route_change = find_by_stable_key(&layout, "graph-variable-reset-route-4")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("route callback");
        runtime
            .invoke(route_change.clone(), vec![Value::String("Track 3".into())])
            .expect("route node 4 to track 3");
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"active\")))",
            1.0,
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"r\")))",
            0.28,
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"g\")))",
            0.84,
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"b\")))",
            0.54,
        );
        runtime
            .invoke(route_change, vec![Value::String("Off".into())])
            .expect("route node 4 off");
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"active\")))",
            0.0,
        );
        assert_reactive_number(
            &mut runtime,
            "(reactive-value (bind \"GRAPH\" (gvr-route-color-field 4 \"r\")))",
            0.20,
        );
        let seed_route_change = find_by_stable_key(&layout, "graph-variable-reset-seed-route-1")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("seed route callback");
        runtime
            .invoke(seed_route_change.clone(), vec![Value::Bool(true)])
            .expect("enable node 1 routed seeding");
        assert_eq!(
            runtime
                .eval_str("(graph-node-value gvr-name 1 :seed-route)")
                .expect("node 1 seed route enabled"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value gvr-name 1 :seed-from)")
                .expect("node 1 seed-from routed track"),
            Some(value_list(vec![Value::Number(0.0)]))
        );
        runtime
            .invoke(seed_route_change, vec![Value::Bool(false)])
            .expect("disable node 1 routed seeding");
        assert_eq!(
            runtime
                .eval_str("(graph-node-value gvr-name 1 :seed-route)")
                .expect("node 1 seed route disabled"),
            Some(Value::Number(0.0))
        );
        let reset_seed_change = find_by_stable_key(&layout, "graph-variable-reset-reset-seed-7")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("reset seed callback");
        runtime
            .invoke(reset_seed_change, vec![Value::Bool(true)])
            .expect("enable node 7 reset seeding");
        assert_eq!(
            runtime
                .eval_str("(graph-node-value gvr-name 7 :seed-on-reset)")
                .expect("node 7 reset seed enabled"),
            Some(Value::Number(1.0))
        );
        let threshold_change = find_by_stable_key(&layout, "graph-variable-reset-threshold")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("threshold callback");
        runtime
            .invoke(threshold_change, vec![Value::Number(0.8)])
            .expect("set graph threshold");
        assert_eq!(
            runtime
                .eval_str("(graph-param-value gvr-name 0 :threshold)")
                .expect("node 0 threshold"),
            Some(Value::Number(0.8))
        );
        let selection_change =
            find_by_stable_key(&layout, "graph-variable-reset-max-poly-selection")
                .and_then(|node| node.props.get("on-change"))
                .cloned()
                .expect("max-poly-selection callback");
        runtime
            .invoke(selection_change, vec![Value::String("random".to_string())])
            .expect("set max-poly-selection");
        assert_eq!(
            runtime
                .eval_str("(graph-config-value gvr-name :max-poly-selection)")
                .expect("max-poly-selection value"),
            Some(Value::String("random".to_string()))
        );
        let node_count_change = find_by_stable_key(&layout, "graph-variable-reset-node-count")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("node-count callback");

        runtime
            .invoke(node_count_change.clone(), vec![Value::Number(16.0)])
            .expect("grow to 16");
        runtime.run_reactive_cycle();
        let layout = latest_layout(&mut runtime);
        assert_active_layout(&layout, 16);
        assert_eq!(
            runtime
                .eval_str("(graph-param-value gvr-name 14 :threshold)")
                .expect("restored capacity threshold"),
            Some(Value::Number(0.8))
        );

        runtime
            .eval_str("(graph-param gvr-name 14 :transpose 7)")
            .expect("write node 14 override");
        runtime
            .eval_str("(graph-edge gvr-name :from 14 :to 3 :weight 0.5)")
            .expect("write edge 14->3 override");

        runtime
            .invoke(node_count_change.clone(), vec![Value::Number(12.0)])
            .expect("shrink to 12");
        runtime.run_reactive_cycle();
        let layout = latest_layout(&mut runtime);
        assert_active_layout(&layout, 12);
        assert!(find_by_stable_key(&layout, "graph-variable-reset-transpose-14").is_none());

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-variable-reset-demo")
            .expect("variable graph overrides");
        assert_eq!(graph.node_count, Some(12));
        assert_eq!(
            graph.max_poly_selection,
            Some(NeuralMaxPolySelection::Random)
        );
        assert!(graph
            .node_params
            .iter()
            .any(|param| param.instance == 14 && param.param == "threshold" && param.value == 0.8));
        assert!(graph
            .node_params
            .iter()
            .any(|param| param.instance == 14 && param.param == "transpose" && param.value == 7.0));
        assert!(graph
            .edge_params
            .iter()
            .any(|edge| edge.from == 14 && edge.to == 3 && edge.value == 0.5));
        assert!(graph.node_intrinsics.iter().any(|node| {
            node.instance == 7 && node.seed_on_reset == Some(1.0) && node.seed_from.is_none()
        }));
        let shrunk = manifest.runtime_config_with_overrides(Some(graph));
        assert_eq!(shrunk.nodes.len(), 12);
        assert_eq!(shrunk.nodes[7].seed_on_reset, 1.0);
        assert!(shrunk.nodes[7].trigger_on_reset);
        assert!(shrunk
            .edges
            .iter()
            .all(|edge| edge.from < 12 && edge.to < 12));

        runtime
            .invoke(node_count_change, vec![Value::Number(16.0)])
            .expect("restore to 16");
        runtime.run_reactive_cycle();
        let layout = latest_layout(&mut runtime);
        assert_active_layout(&layout, 16);
        assert!(find_by_stable_key(&layout, "graph-variable-reset-transpose-14").is_some());
        assert_eq!(
            runtime
                .eval_str("(graph-param-value gvr-name 14 :transpose)")
                .expect("read restored node 14"),
            Some(Value::Number(7.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value gvr-name :from 14 :to 3 :weight)")
                .expect("read restored edge 14->3"),
            Some(Value::Number(0.5))
        );
    }

    #[test]
    fn graph_group_matrix_demo_loads_and_edits_group_matrix_cells() {
        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        let state = Arc::new(SequencerState::new(
            16,
            (0..16).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str(
                "(def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon) nil)",
            )
            .expect("install script sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-group-matrix-demo.lisp"))
        .expect("read graph group matrix demo script");
        runtime
            .eval_str(&source)
            .expect("evaluate graph group matrix demo");
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the group matrix demo must not write pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published group matrix graph manifest");
        assert_eq!(manifest.name, "neural-group-matrix-demo");

        // The matrix reader sees the engine's inert defaults: G all-ones, H all-zeros.
        assert_eq!(
            runtime
                .eval_str("(nth (nth (ggm-read-group-matrix \"group-gain\") 0) 1)")
                .expect("read G cell default"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            runtime
                .eval_str("(nth (nth (ggm-read-group-matrix \"group-coupling\") 0) 1)")
                .expect("read H cell default"),
            Some(Value::Number(0.0))
        );

        // The two group matrices render with the fixed 4×4 shape and cell-edit hooks.
        let tree = runtime
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 64.0)))
            .expect("group matrix widget tree should lay out");
        for key in [
            "graph-group-matrix-weight-matrix",
            "graph-group-matrix-group-gain-matrix",
            "graph-group-matrix-group-coupling-matrix",
            "graph-group-matrix-group-activity-matrix",
            "graph-group-matrix-group-suppression-matrix",
        ] {
            assert!(
                find_by_stable_key(&layout, key).is_some(),
                "missing {key}"
            );
        }
        let gain_change = find_by_stable_key(&layout, "graph-group-matrix-group-gain-matrix")
            .and_then(|node| node.props.get("on-cell-change"))
            .cloned()
            .expect("G matrix cell-change callback");
        runtime
            .invoke(
                gain_change,
                vec![Value::Number(0.0), Value::Number(1.0), Value::Number(0.25)],
            )
            .expect("edit G[A][B]");
        let coupling_change =
            find_by_stable_key(&layout, "graph-group-matrix-group-coupling-matrix")
                .and_then(|node| node.props.get("on-cell-change"))
                .cloned()
                .expect("H matrix cell-change callback");
        runtime
            .invoke(
                coupling_change,
                vec![Value::Number(1.0), Value::Number(0.0), Value::Number(-1.5)],
            )
            .expect("edit H[B][A]");

        // One drag = one persisted cell, resolved back through graph-config-value.
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-group-matrix-demo")
            .expect("group matrix overrides");
        let k = crate::graph::NEURAL_GROUP_MAX as usize;
        assert_eq!(graph.group_gain.as_ref().expect("gain written")[1], 0.25);
        assert_eq!(
            graph.group_coupling.as_ref().expect("coupling written")[k],
            -1.5
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value ggm-name :group-gain-0-1)")
                .expect("read back G cell"),
            Some(Value::Number(0.25))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value ggm-name :group-coupling-1-0)")
                .expect("read back H cell"),
            Some(Value::Number(-1.5))
        );
    }

    #[test]
    fn graph_markov_8x8_demo_loads_weight_matrix_and_node_delays() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer) nil)")
            .expect("install sequencer tab registration test stub");
        runtime
            .eval_str(
                "(def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon) nil)",
            )
            .expect("install script sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-markov-8x8-demo.lisp"))
        .expect("read markov 8x8 demo script");
        runtime.eval_str(&source).expect("evaluate markov 8x8 demo");
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the markov demo must not write pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published markov graph manifest");
        assert_eq!(manifest.name, "markov-8x8-demo");
        assert_eq!(manifest.shape.num_nodes(), 8);
        assert_eq!(
            manifest.edge_sets[0].distribution,
            crate::graph::EdgeDistribution::WeightedChoice
        );

        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("markov script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((70.0, 70.0)))
            .expect("markov widget tree should lay out");

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            3,
            "expected trigger/energy telemetry plus editable weight matrix"
        );
        for key in [
            "markov-8x8-trigger-matrix",
            "markov-8x8-energy-matrix",
            "markov-8x8-weight-matrix",
        ] {
            let widget =
                find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("missing {key}"));
            assert_measured(widget);
        }
        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            8 * 3 + 1,
            "expected delay/transpose/vel-scale per node plus max-poly"
        );
        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            24,
            "expected route + resolution + quantize per node"
        );

        runtime
            .eval_str("(m8-init-defaults)")
            .expect("explicitly initialize markov defaults");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "markov-8x8-demo")
            .expect("markov graph overrides after explicit init");
        assert_eq!(
            graph.edge_params.len(),
            64,
            "explicit init should write only the weight matrix"
        );
        assert!(graph.edge_params.iter().any(|edge| {
            edge.from == 0 && edge.to == 1 && edge.param == "weight" && edge.value == 0.65
        }));
        assert!(graph.node_intrinsics.iter().any(|node| {
            node.instance == 0
                && node.seed_from == Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0]))
        }));
        assert!(graph
            .node_intrinsics
            .iter()
            .any(|node| { node.instance == 4 && node.delay_steps == Some(3) }));
    }

    #[test]
    fn graph_16_demo_ui_exposes_all_node_controls_and_ring_defaults() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn assert_number_prop(node: &eseqlisp::layout::LayoutNode, prop: &str, expected: f64) {
            assert_eq!(
                node.props.get(prop),
                Some(&Value::Number(expected)),
                "expected {} {:?} to be {expected}",
                node.widget_type,
                prop
            );
        }

        let state = Arc::new(SequencerState::new(
            16,
            (0..16).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str(
                r#"
                (defstate eseq.seq-step-tabs/seq-registered-step-tabs '())
                (def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer)
                  (set! eseq.seq-step-tabs/seq-registered-step-tabs
                    (append
                      (filter (lambda (tab) (not (= (nth tab 1) buffer)))
                        eseq.seq-step-tabs/seq-registered-step-tabs)
                      (list (list label buffer)))))
                (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon)
                  (eseq.seq-step-tabs/seq-register-step-sequencer-tab label buffer))
                "#,
            )
            .expect("install sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-16-demo.lisp"))
        .expect("read graph 16 demo script");
        runtime.eval_str(&source).expect("evaluate graph 16 demo");
        assert_eq!(
            runtime
                .eval_str("eseq.seq-step-tabs/seq-registered-step-tabs")
                .expect("read registered step tabs"),
            Some(gv_list(vec![gv_list(vec![
                Value::String("16x16".to_string()),
                Value::String("*16x16*".to_string()),
            ])])),
            "graph 16 demo should register a step-panel tab like the 8x8 demo"
        );
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the graph demo must publish graph/UI without writing pattern overrides"
        );
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published graph manifest");
        assert_eq!(
            manifest.shape.num_nodes(),
            16,
            "the demo matrix must cover every materialized node"
        );

        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("graph 16 script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((40.0, 56.0)))
            .expect("graph 16 widget tree should lay out");

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected editable weight matrix plus trigger/energy/dampening telemetry"
        );
        for matrix in &matrices {
            assert_measured(matrix);
        }
        for key in [
            "graph-16-trigger-matrix",
            "graph-16-energy-matrix",
            "graph-16-weight-matrix",
            "graph-16-dampening-matrix",
            "graph-16-event-view",
        ] {
            let widget =
                find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("missing {key}"));
            assert_measured(widget);
        }
        let mut event_views = Vec::new();
        collect_widgets(&layout, "event-view", &mut event_views);
        assert_eq!(event_views.len(), 1, "expected one event-view");
        let trigger_matrix =
            find_by_stable_key(&layout, "graph-16-trigger-matrix").expect("trigger matrix");
        assert_number_prop(trigger_matrix, "height", 24.0);
        let energy_matrix =
            find_by_stable_key(&layout, "graph-16-energy-matrix").expect("energy matrix");
        assert_number_prop(energy_matrix, "height", 24.0);
        let weight_matrix =
            find_by_stable_key(&layout, "graph-16-weight-matrix").expect("weight matrix");
        assert_number_prop(weight_matrix, "width", 52.0);
        assert_number_prop(weight_matrix, "height", 24.0);

        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            16 * 8 + 4,
            "expected delay/transpose/reset/vel/dampening/recovery per node + reset-bars/max-poly/dur-factor/swing"
        );
        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            16 * 3,
            "expected route + resolution + quantize per node"
        );
        for idx in 0..16 {
            for key in [
                format!("graph-16-route-{idx}"),
                format!("graph-16-delay-{idx}"),
                format!("graph-16-transpose-{idx}"),
                format!("graph-16-transpose-reset-{idx}"),
                format!("graph-16-vel-decay-{idx}"),
                format!("graph-16-vel-reset-{idx}"),
                format!("graph-16-state-reset-{idx}"),
                format!("graph-16-dampening-{idx}"),
                format!("graph-16-recovery-{idx}"),
                format!("graph-16-resolution-{idx}"),
                format!("graph-16-quantize-{idx}"),
            ] {
                let widget = find_by_stable_key(&layout, &key)
                    .unwrap_or_else(|| panic!("missing control {key}"));
                assert_measured(widget);
            }
        }

        let transpose_reset_change = find_by_stable_key(&layout, "graph-16-transpose-reset-5")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("transpose-reset callback");
        runtime
            .invoke(transpose_reset_change, vec![Value::Number(1.0)])
            .expect("invoke transpose-reset callback");
        let vel_reset_change = find_by_stable_key(&layout, "graph-16-vel-reset-6")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("vel-reset callback");
        runtime
            .invoke(vel_reset_change, vec![Value::Number(1.0)])
            .expect("invoke vel-reset callback");
        let state_reset_change = find_by_stable_key(&layout, "graph-16-state-reset-7")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("state-reset callback");
        runtime
            .invoke(state_reset_change, vec![Value::Number(1.0)])
            .expect("invoke state-reset callback");
        let dur_factor_change = find_by_stable_key(&layout, "graph-16-dur-factor")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("dur-factor callback");
        runtime
            .invoke(dur_factor_change, vec![Value::Number(2.0)])
            .expect("invoke dur-factor callback");
        let swing_change = find_by_stable_key(&layout, "graph-16-swing")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("swing callback");
        runtime
            .invoke(swing_change, vec![Value::Number(64.0)])
            .expect("invoke swing callback");

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-16-demo")
            .expect("graph overrides after reset control edits");
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 5 && param.param == "transpose-reset" && param.value == 1.0
            }),
            "transpose-reset knob should write a node param override"
        );
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 6 && param.param == "vel-reset" && param.value == 1.0
            }),
            "vel-reset knob should write a node param override"
        );
        assert!(
            graph.node_params.iter().any(|param| {
                param.instance == 7 && param.param == "state-reset" && param.value == 1.0
            }),
            "state-reset knob should write a node param override"
        );
        assert!(
            (0..16).all(|idx| {
                graph.node_params.iter().any(|param| {
                    param.instance == idx && param.param == "dur-factor" && param.value == 2.0
                })
            }),
            "dur-factor global knob should write every node param override"
        );
        assert!(
            (0..16).all(|idx| {
                graph.node_params.iter().any(|param| {
                    param.instance == idx && param.param == "swing" && param.value == 64.0
                })
            }),
            "swing global knob should write every node param override"
        );

        runtime
            .eval_str("(g16-init-ring-defaults)")
            .expect("explicitly initialize graph 16 demo defaults");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-16-demo")
            .expect("graph overrides after explicit init");
        assert_eq!(
            graph.edge_params.len(),
            16 * 16,
            "explicit init should write the full ring weight matrix"
        );
        assert!(
            graph.edge_params.iter().any(|edge| {
                edge.from == 0 && edge.to == 1 && edge.param == "weight" && edge.value == 1.0
            }),
            "explicit init should write the first ring edge"
        );
        assert!(
            graph.edge_params.iter().any(|edge| {
                edge.from == 15 && edge.to == 0 && edge.param == "weight" && edge.value == 1.0
            }),
            "explicit init should wrap the ring from the final node to node 0"
        );
        assert!(
            graph.node_intrinsics.iter().any(|node| {
                node.instance == 0
                    && node.seed_from == Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0]))
            }),
            "explicit init should seed node 0 from track 0"
        );
    }

    #[test]
    fn graph_16_cycle_demo_round_trips_resolution_and_quantize_cycles() {
        let state = Arc::new(SequencerState::new(
            16,
            (0..16).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str(
                r#"
                (defstate eseq.seq-step-tabs/seq-registered-step-tabs '())
                (def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer)
                  (set! eseq.seq-step-tabs/seq-registered-step-tabs
                    (append eseq.seq-step-tabs/seq-registered-step-tabs (list (list label buffer)))))
                (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon)
                  (eseq.seq-step-tabs/seq-register-step-sequencer-tab label buffer))
                "#,
            )
            .expect("install sequencer tab registration test stub");

        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/graph-neural-16-cycle-demo.lisp"))
        .expect("read graph 16 cycle demo script");
        runtime
            .eval_str(&source)
            .expect("evaluate graph 16 cycle demo");

        // The panel must render (exercises the text-input + g16c-sync-cycles body): lay it
        // out and confirm a resolution + quantize cycle text field per node.
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }
        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }
        let tree = runtime
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("cycle demo should publish a widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((44.0, 56.0)))
            .expect("cycle demo widget tree should lay out");
        let event_view =
            find_by_stable_key(&layout, "graph-16c-event-view").expect("cycle event-view");
        assert!(
            event_view.rect.width > 0.0 && event_view.rect.height > 0.0,
            "{:?}",
            event_view.rect
        );
        let mut event_views = Vec::new();
        collect_widgets(&layout, "event-view", &mut event_views);
        assert_eq!(event_views.len(), 1, "expected one event-view");
        let mut text_inputs = Vec::new();
        collect_widgets(&layout, "text-input", &mut text_inputs);
        assert_eq!(
            text_inputs.len(),
            16 * 2,
            "expected a resolution + quantize cycle text field per node"
        );
        for idx in 0..16 {
            for key in [
                format!("graph-16c-resolution-{idx}"),
                format!("graph-16c-quantize-{idx}"),
            ] {
                let widget = find_by_stable_key(&layout, &key)
                    .unwrap_or_else(|| panic!("missing cycle field {key}"));
                assert!(widget.rect.width > 0.0 && widget.rect.height > 0.0);
            }
        }

        // Loading must not write overrides (matches the other demos).
        assert!(
            state.current_graph_overrides().is_empty(),
            "loading the cycle demo must not write pattern overrides"
        );

        // Explicit init writes the showcase cycles onto nodes 0 and 1.
        runtime
            .eval_str("(script-init-fn)")
            .expect("initialize cycle demo defaults");

        let read_cycle = |runtime: &mut Runtime, node: usize, field: &str| {
            runtime
                .eval_str(&format!("(graph-node-value g16c-name {node} {field})"))
                .expect("read cycle")
        };
        assert_eq!(
            read_cycle(&mut runtime, 0, ":resolution-cycle"),
            Some(Value::String("16 16 16 16 16 4".to_string())),
            "node 0 should round-trip the showcase resolution cycle"
        );
        assert_eq!(
            read_cycle(&mut runtime, 1, ":resolution-cycle"),
            Some(Value::String("16 8 16".to_string())),
            "node 1 should round-trip its 3-slot lurch cycle"
        );

        // The stored override is a list of timebase indices (16->Sixteenth=4, 4->Quarter=2).
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "neural-16-cycle-demo")
            .expect("cycle demo graph overrides");
        let node0 = graph
            .node_intrinsics
            .iter()
            .find(|node| node.instance == 0)
            .expect("node 0 intrinsic override");
        assert_eq!(
            node0.resolution,
            Some(vec![4, 4, 4, 4, 4, 2]),
            "resolution override should store the full cycle as timebase indices"
        );

        // The UI edit path (g16c-edit-cycle -> graph-node string parse) is lenient: extra
        // whitespace collapses and unparseable tokens drop, then it re-serializes canonically.
        runtime
            .eval_str(r#"(g16c-edit-cycle 2 :resolution "16  4 garbage 8")"#)
            .expect("edit node 2 resolution cycle");
        assert_eq!(
            read_cycle(&mut runtime, 2, ":resolution-cycle"),
            Some(Value::String("16 4 8".to_string())),
            "lenient parse drops junk tokens and collapses whitespace"
        );

        // Quantize accepts a cycle too; a whole-field "off" collapses to a single off slot.
        runtime
            .eval_str(r#"(g16c-edit-cycle 4 :quantize "16 8 16")"#)
            .expect("edit node 4 quantize cycle");
        assert_eq!(
            read_cycle(&mut runtime, 4, ":quantize-cycle"),
            Some(Value::String("16 8 16".to_string())),
            "quantize cycle should round-trip"
        );
        runtime
            .eval_str(r#"(g16c-edit-cycle 4 :quantize "off")"#)
            .expect("clear node 4 quantize cycle");
        assert_eq!(
            read_cycle(&mut runtime, 4, ":quantize-cycle"),
            Some(Value::String("off".to_string())),
            "a whole-field off collapses to a single off slot"
        );
    }

    #[test]
    fn graph_8x8_demo_scratch_load_preserves_saved_overrides() {
        let state = Arc::new(SequencerState::new(
            8,
            (0..8).map(|_| default_empty_effect_chain()).collect(),
        ));
        let expected = crate::graph::ProjectGraphOverrides {
            sequencer_id: super::stable_sequencer_id("neural-8x8-demo"),
            sequencer_name: "neural-8x8-demo".to_string(),
            node_intrinsics: vec![
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 0,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: None,
                    seed_from: Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0])),
                    seed_on_reset: None,
                    duration: None,
                    swing: None,
                    neural_group: None,
                },
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 3,
                    resolution: None,
                    delay_steps: Some(6),
                    quantize: None,
                    route: None,
                    seed_from: None,
                    seed_on_reset: None,
                    duration: None,
                    swing: None,
                    neural_group: None,
                },
                crate::graph::ProjectGraphNodeIntrinsicOverride {
                    group: "nrn".to_string(),
                    instance: 4,
                    resolution: None,
                    delay_steps: None,
                    quantize: None,
                    route: Some(crate::graph::ProjectGraphRouteOverride::Track(0)),
                    seed_from: None,
                    seed_on_reset: None,
                    duration: None,
                    swing: None,
                    neural_group: None,
                },
            ],
            node_params: vec![crate::graph::ProjectGraphNodeParamOverride {
                group: "nrn".to_string(),
                instance: 2,
                param: "transpose".to_string(),
                value: -12.0,
            }],
            edge_params: vec![crate::graph::ProjectGraphEdgeParamOverride {
                group: "nrn->nrn".to_string(),
                from: 0,
                to: 1,
                param: "weight".to_string(),
                value: 0.25,
            }],
            reset_every_beats: None,
            max_poly: None,
            max_poly_selection: None,
            node_count: None,
            group_gain: None,
            group_coupling: None,
            group_trace_decay: None,
        };
        state
            .edit_current_graph_overrides(|overrides| {
                *overrides = vec![expected.clone()];
                Ok(())
            })
            .unwrap();

        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("graph-visualizations", Value::List(Vec::new())),
            ],
            true,
        );
        register_graph_def_sequencer_test_native(&mut runtime, Arc::clone(&state));
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|crates_dir| crates_dir.parent())
            .expect("sequencer crate should live under workspace crates dir")
            .join(".eseqlisp-scratch");
        let report = runtime.eval_source_transactional(
            Some(workspace_root),
            r#"
            (def eseq.seq-step-tabs/seq-register-step-sequencer-tab (label buffer) nil)
            (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer icon) nil)
            (load "content/scripts/sequencers/graph-neural-8x8-demo.lisp")
            "#,
            Vec::new(),
        );
        assert!(
            report.success,
            "scratch-style load failed: {}",
            report.failure_message()
        );
        assert_eq!(
            runtime.eval_str("g8-name").expect("read loaded graph name"),
            Some(Value::String("neural-8x8-demo".to_string())),
            "scratch-style load should define the graph demo UI state"
        );

        assert!(
            state
                .published_sequencers()
                .into_iter()
                .any(|published| published.name == "neural-8x8-demo" && published.graph.is_some()),
            "scratch load should republish the graph manifest"
        );
        assert_eq!(
            state.current_graph_overrides(),
            vec![expected],
            "scratch load must not clobber saved graph overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 2 :transpose))")
                .expect("read bound transpose value"),
            Some(Value::Number(-12.0)),
            "loaded UI should sync node params from saved overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 3 :delay))")
                .expect("read bound delay value"),
            Some(Value::Number(6.0)),
            "loaded UI should sync node intrinsics from saved overrides"
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph g8-name 4 :route g8-route-options))")
                .expect("read bound route index"),
            Some(Value::Number(0.0)),
            "loaded UI should display saved internal route 0 as Track 1 (index 0)"
        );
        assert_eq!(
            runtime
                .eval_str("(nth (nth g8-weights 0) 1)")
                .expect("read synced weight"),
            Some(Value::Number(0.25)),
            "loaded UI should sync matrix weights from saved overrides"
        );
    }

    #[test]
    fn graph_authoring_natives_write_current_pattern_overrides() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 123,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 2,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str(
                "(graph-node \"neural\" 1 :delay 3 :route 0 :seed-from :route :seed-on-reset 1 :duration (beats :16) :swing (swing 60 :16))",
            )
            .expect("graph-node");
        runtime
            .eval_str("(graph-param \"neural\" 1 :threshold 0.75)")
            .expect("graph-param");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :weight 0.5)")
            .expect("graph-edge");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :delay 7)")
            .expect("graph-edge delay");

        let overrides = state.current_graph_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].node_intrinsics[0].delay_steps, Some(3));
        assert_eq!(
            overrides[0].node_intrinsics[0].seed_from,
            Some(crate::graph::ProjectGraphSeedFrom::Route)
        );
        assert_eq!(overrides[0].node_intrinsics[0].seed_on_reset, Some(1.0));
        assert_eq!(
            overrides[0].node_intrinsics[0].duration,
            Some(crate::graph::GraphDurationSpec::Beats { value: 0.25 })
        );
        assert_eq!(
            overrides[0].node_intrinsics[0].swing,
            Some(crate::graph::GraphSwingSpec::new(60.0, 0))
        );
        assert_eq!(overrides[0].node_params[0].value, 0.75);
        assert!(overrides[0]
            .edge_params
            .iter()
            .any(|edge| edge.param == "weight" && edge.value == 0.5));
        assert!(overrides[0]
            .edge_params
            .iter()
            .any(|edge| edge.param == "delay" && edge.value == 7.0));
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :delay)")
                .expect("graph-node-value delay"),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :route)")
                .expect("graph-node-value route"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :seed-route)")
                .expect("graph-node-value seed route"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 1 :seed-on-reset))")
                .expect("bound seed-on-reset"),
            Some(Value::Number(1.0))
        );
        runtime
            .eval_str("(graph-node \"neural\" 1 :group 2)")
            .expect("graph-node group");
        assert_eq!(
            state.current_graph_overrides()[0].node_intrinsics[0].neural_group,
            Some(2)
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :group)")
                .expect("graph-node-value group"),
            Some(Value::Number(2.0))
        );
        // Unassigned nodes stay in group A, and the numeric bind path serves the value.
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 0 :group))")
                .expect("bound group default"),
            Some(Value::Number(0.0))
        );
        runtime
            .eval_str("(graph-node \"neural\" 1 :seed-from :off :seed-on-reset 0)")
            .expect("disable seeding");
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :seed-route)")
                .expect("graph-node-value seed route off"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :seed-from)")
                .expect("graph-node-value seed-from off"),
            Some(Value::List(Vec::new()))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-node-value \"neural\" 1 :seed-on-reset)")
                .expect("graph-node-value seed-on-reset off"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-param-value \"neural\" 1 :threshold)")
                .expect("graph-param-value threshold"),
            Some(Value::Number(0.75))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"neural\" :from 0 :to 1 :weight)")
                .expect("graph-edge-value keyword syntax"),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"neural\" 0 1 :weight)")
                .expect("graph-edge-value positional syntax"),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"neural\" 0 1 :delay)")
                .expect("graph-edge-value delay"),
            Some(Value::Number(7.0))
        );
    }

    #[test]
    fn bind_graph_seeds_reactive_slots_and_keys_round_trip() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 77,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 2,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "transpose".into(),
                    min: -48.0,
                    max: 48.0,
                    default: 0.0,
                    is_int: true,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(graph-node \"neural\" 1 :delay 4 :route 2)")
            .expect("graph-node");
        runtime
            .eval_str("(graph-param \"neural\" 1 :transpose -7)")
            .expect("graph-param");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :weight 0.5)")
            .expect("graph-edge");
        runtime
            .eval_str("(graph-edge \"neural\" :from 0 :to 1 :delay 6)")
            .expect("graph-edge delay");

        // Numeric intrinsic + param bind-graph handles read the resolved value from the slot.
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 1 :delay))")
                .expect("bind-graph delay"),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"neural\" 1 :transpose))")
                .expect("bind-graph transpose"),
            Some(Value::Number(-7.0))
        );
        // Enum intrinsic binds to the dropdown index within the supplied options
        // (route 2 -> "Track 3" -> index 2).
        assert_eq!(
            runtime
                .eval_str(
                    "(reactive-value (bind-graph \"neural\" 1 :route \
                     (list \"Track 1\" \"Track 2\" \"Track 3\" \"Off\")))"
                )
                .expect("bind-graph route index"),
            Some(Value::Number(2.0))
        );
        // Edge handle.
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-edge \"neural\" 0 1 :weight))")
                .expect("bind-graph-edge weight"),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-edge \"neural\" 0 1 :delay))")
                .expect("bind-graph-edge delay"),
            Some(Value::Number(6.0))
        );

        // graph-key / graph-edge-key name the exact slot a reactive-set dirties, so a
        // plain `bind` to that key observes the new value (this is the edit-writeback path).
        runtime
            .eval_str("(reactive-set \"GRAPH\" (graph-key \"neural\" 1 :delay) 9)")
            .expect("reactive-set node key");
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind \"GRAPH\" (graph-key \"neural\" 1 :delay)))")
                .expect("read node slot"),
            Some(Value::Number(9.0))
        );
        runtime
            .eval_str("(reactive-set \"GRAPH\" (graph-edge-key \"neural\" 0 1 :weight) 0.2)")
            .expect("reactive-set edge key");
        assert_eq!(
            runtime
                .eval_str(
                    "(reactive-value (bind \"GRAPH\" (graph-edge-key \"neural\" 0 1 :weight)))"
                )
                .expect("read edge slot"),
            Some(Value::Number(0.2))
        );
    }

    #[test]
    fn graph_config_overrides_round_trip_and_reach_runtime() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 88,
            name: "neural".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 16.0, // 4 bars
            seed_on_reset: 0.0,
            max_poly: 4,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest.clone()),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));

        // Manifest defaults reported in UI units (16 beats / 4 = 4 bars; cap 4).
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :reset-bars)")
                .expect("reset-bars default"),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :max-poly)")
                .expect("max-poly default"),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :max-poly-selection)")
                .expect("max-poly-selection default"),
            Some(Value::String("deterministic".to_string()))
        );

        // Override scalar and enum config; reset-bars persists as beats (2 bars -> 8 beats).
        runtime
            .eval_str("(graph-config \"neural\" :reset-bars 2)")
            .expect("set reset-bars");
        runtime
            .eval_str("(graph-config \"neural\" :max-poly 1)")
            .expect("set max-poly");
        runtime
            .eval_str("(graph-config \"neural\" :max-poly-selection :random)")
            .expect("set max-poly-selection");
        let overrides = state.current_graph_overrides();
        assert_eq!(overrides[0].reset_every_beats, Some(8.0));
        assert_eq!(overrides[0].max_poly, Some(1));
        assert_eq!(
            overrides[0].max_poly_selection,
            Some(NeuralMaxPolySelection::Random)
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :reset-bars)")
                .expect("reset-bars override"),
            Some(Value::Number(2.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-config \"neural\" :max-poly))")
                .expect("bound max-poly"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :max-poly-selection)")
                .expect("max-poly-selection override"),
            Some(Value::String("random".to_string()))
        );
        assert_eq!(
            runtime
                .eval_str(
                    "(reactive-value (bind-graph-config \"neural\" :max-poly-selection \
                     (list \"deterministic\" \"propagation\" \"random\")))"
                )
                .expect("bound max-poly-selection"),
            Some(Value::Number(2.0))
        );

        // The overrides actually reach the materialized runtime config.
        let config = manifest.runtime_config_with_overrides(Some(&overrides[0]));
        assert_eq!(config.reset_interval_beats, 8.0);
        assert_eq!(config.max_poly, 1);
        assert_eq!(config.max_poly_selection, NeuralMaxPolySelection::Random);

        // Group matrices (neural-groups spec §3.2): unset cells read their inert
        // defaults, one graph-config write persists one cell, and values clamp to the
        // declared cell ranges.
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :group-gain-0-1)")
                .expect("group-gain default"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :group-coupling-0-1)")
                .expect("group-coupling default"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"neural\" :group-trace-decay)")
                .expect("group-trace-decay default"),
            Some(Value::Number(crate::graph::GROUP_TRACE_DECAY_DEFAULT))
        );
        runtime
            .eval_str("(graph-config \"neural\" :group-gain-0-1 0.25)")
            .expect("set group-gain cell");
        runtime
            .eval_str("(graph-config \"neural\" :group-coupling-1-0 9)")
            .expect("set group-coupling cell (clamps)");
        runtime
            .eval_str("(graph-config \"neural\" :group-trace-decay 0.8)")
            .expect("set group-trace-decay");
        let overrides = state.current_graph_overrides();
        let k = crate::graph::NEURAL_GROUP_MAX as usize;
        let gain = overrides[0].group_gain.as_ref().expect("gain persisted");
        assert_eq!(gain[1], 0.25); // cell 0-1
        assert_eq!(gain[k], 1.0); // untouched cell 1-0 keeps its default
        let coupling = overrides[0]
            .group_coupling
            .as_ref()
            .expect("coupling persisted");
        assert_eq!(coupling[k], crate::graph::GROUP_COUPLING_MAX);
        assert_eq!(overrides[0].group_trace_decay, Some(0.8));
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-config \"neural\" :group-gain-0-1))")
                .expect("bound group-gain cell"),
            Some(Value::Number(0.25))
        );
        let config = manifest.runtime_config_with_overrides(Some(&overrides[0]));
        assert_eq!(config.group_gain[1], 0.25);
        assert_eq!(config.group_coupling[k], crate::graph::GROUP_COUPLING_MAX);
        assert_eq!(config.group_trace_decay, 0.8);
        // An out-of-range cell coordinate is rejected: nothing is written.
        let _ = runtime.eval_str("(graph-config \"neural\" :group-gain-9-0 1)");
        let overrides = state.current_graph_overrides();
        let gain = overrides[0].group_gain.as_ref().expect("gain persisted");
        assert_eq!(gain.len(), crate::graph::NEURAL_GROUP_CELLS);
        assert_eq!(gain[1], 0.25);
    }

    #[test]
    fn graph_config_node_count_round_trips_and_restores_dormant_overrides() {
        use crate::graph::{EdgeSetSpec, GraphManifest, NodeProto, ParamSpec, ShapeSpec, Topology};
        use crate::sequencer::{PublishedSequencer, Timebase};

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let manifest = GraphManifest {
            id: 188,
            name: "variable".into(),
            shape: ShapeSpec::VariableLine {
                default: 8,
                min: 1,
                max: 16,
            },
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 4,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
            node: NodeProto {
                name: "nrn".into(),
                params: vec![ParamSpec {
                    name: "threshold".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 1.0,
                    is_int: false,
                }],
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "nrn".into(),
                to: "nrn".into(),
                topology: Topology::AllToAll,
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    is_int: false,
                }],
            }],
        };
        let fixed = GraphManifest {
            id: 189,
            name: "fixed".into(),
            shape: ShapeSpec::Line(8),
            ..manifest.clone()
        };
        state.publish_sequencer(PublishedSequencer {
            id: manifest.id,
            name: manifest.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest.clone()),
        });
        state.publish_sequencer(PublishedSequencer {
            id: fixed.id,
            name: fixed.name.clone(),
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(fixed),
        });

        let mut runtime = Runtime::new();
        register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
        assert_eq!(
            runtime
                .eval_str("(graph-config-value \"variable\" :node-count)")
                .expect("node-count default"),
            Some(Value::Number(8.0))
        );
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph-config \"variable\" :node-count))")
                .expect("bound node-count default"),
            Some(Value::Number(8.0))
        );
        let before_fixed_reject = state.current_graph_overrides();
        let fixed_result = runtime
            .eval_str("(graph-config \"fixed\" :node-count 4)")
            .expect("fixed node-count diagnostic should not abort the VM");
        assert_ne!(fixed_result, Some(Value::Bool(true)));
        assert_eq!(state.current_graph_overrides(), before_fixed_reject);

        let before = state.scheduler_snapshot_version();
        runtime
            .eval_str("(graph-config \"variable\" :node-count 12)")
            .expect("set node-count");
        assert!(state.scheduler_snapshot_version() > before);
        runtime
            .eval_str("(graph-param \"variable\" 14 :threshold 0.75)")
            .expect("write dormant node param");
        runtime
            .eval_str("(graph-edge \"variable\" :from 14 :to 3 :weight 0.5)")
            .expect("write dormant edge");
        let inactive_bind = runtime
            .eval_str("(reactive-value (bind-graph \"variable\" 14 :threshold))")
            .expect("inactive bind diagnostic should not abort the VM");
        assert_ne!(inactive_bind, Some(Value::Number(0.75)));

        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "variable")
            .expect("variable graph overrides");
        assert_eq!(graph.node_count, Some(12));
        assert!(graph
            .node_params
            .iter()
            .any(|param| param.instance == 14 && param.value == 0.75));
        assert!(graph
            .edge_params
            .iter()
            .any(|edge| edge.from == 14 && edge.to == 3 && edge.value == 0.5));
        assert_eq!(
            manifest
                .runtime_config_with_overrides(Some(graph))
                .nodes
                .len(),
            12
        );

        runtime
            .eval_str("(graph-config \"variable\" :node-count 16)")
            .expect("grow node-count");
        assert_eq!(
            runtime
                .eval_str("(reactive-value (bind-graph \"variable\" 14 :threshold))")
                .expect("read restored dormant node"),
            Some(Value::Number(0.75))
        );
        assert_eq!(
            runtime
                .eval_str("(graph-edge-value \"variable\" :from 14 :to 3 :weight)")
                .expect("read restored dormant edge"),
            Some(Value::Number(0.5))
        );

        runtime
            .eval_str("(graph-config \"variable\" :node-count 99)")
            .expect("clamp node-count high");
        let overrides = state.current_graph_overrides();
        let graph = overrides
            .iter()
            .find(|graph| graph.sequencer_name == "variable")
            .expect("variable graph overrides after clamp");
        assert_eq!(graph.node_count, Some(16));
    }

    #[test]
    fn parse_graph_manifest_requires_shape_and_node() {
        let no_shape = vec![gv_sym("g"), gv_list(vec![gv_sym("def-node"), gv_sym("n")])];
        assert!(super::parse_graph_manifest(&no_shape)
            .unwrap_err()
            .contains(":shape"));

        let no_node = vec![
            gv_sym("g"),
            gv_kw("shape"),
            gv_list(vec![gv_sym("grid"), gv_num(2.0), gv_num(2.0)]),
        ];
        assert!(super::parse_graph_manifest(&no_node)
            .unwrap_err()
            .contains("def-node"));
    }

    static CAPTURED_DGEN_SAMPLE_RATE_BITS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn capture_dgen_sample_rate_process(
        _inp: *const *const f32,
        _out: *const *mut f32,
        _nframes: u32,
        _state: *mut std::ffi::c_void,
        context: *const super::DGenProcessContextV1,
        _host: *const super::DGenHostServicesV1,
    ) {
        let sample_rate = if context.is_null()
            || (*context).abi_version != super::DGEN_ABI_VERSION_V1
            || ((*context).struct_size as usize)
                < std::mem::size_of::<super::DGenProcessContextV1>()
        {
            0.0
        } else {
            (*context).sample_rate
        };
        CAPTURED_DGEN_SAMPLE_RATE_BITS.store(sample_rate.to_bits(), Ordering::SeqCst);
    }

    unsafe extern "C" fn write_one_process(
        _inp: *const *const f32,
        out: *const *mut f32,
        nframes: u32,
        _state: *mut std::ffi::c_void,
        _context: *const super::DGenProcessContextV1,
        _host: *const super::DGenHostServicesV1,
    ) {
        for frame in 0..nframes as usize {
            *(*out).add(frame) = 1.0;
        }
    }

    unsafe extern "C" fn write_two_process(
        _inp: *const *const f32,
        out: *const *mut f32,
        nframes: u32,
        _state: *mut std::ffi::c_void,
        _context: *const super::DGenProcessContextV1,
        _host: *const super::DGenHostServicesV1,
    ) {
        for frame in 0..nframes as usize {
            *(*out).add(frame) = 2.0;
        }
    }

    fn descriptors_with_filter(track_count: usize) -> Vec<Vec<EffectDescriptor>> {
        (0..track_count)
            .map(|_| {
                let mut chain = EffectDescriptor::default_full_chain();
                chain[0] = EffectDescriptor::builtin_filter();
                chain
            })
            .collect()
    }

    fn bind_filter_slot(state: &SequencerState) {
        state.pattern.effect_chains[0][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
    }

    #[test]
    fn folder_instrument_dsp_path_maps_to_instrument_name() {
        let path =
            std::path::Path::new("instruments/monomachine/fmplus/monomachine-fmplus/dsp.lisp");
        assert_eq!(
            super::instrument_name_from_source_path(path).as_deref(),
            Some("monomachine/fmplus/monomachine-fmplus/")
        );
        assert_eq!(
            super::source_name_from_path(&eseqlisp::CompileKind::Instrument, path).as_deref(),
            Some("monomachine/fmplus/monomachine-fmplus/")
        );
    }

    #[test]
    fn parse_manifest_uses_dgen_param_span_metadata() {
        let manifest = parse_manifest(
            r#"{
                "processAbi": "dgen-host-abi-v1",
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [
                    {"name": "implicit_scalar", "cellId": 2, "default": 0.1},
                    {"name": "scalar", "cellId": 4, "cellSpan": 1, "default": 0.25},
                    {"name": "vector", "cellId": 8, "vectorWidth": 4, "default": 0.5}
                ]
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.params[0].cell_span, 1);
        assert_eq!(manifest.params[1].cell_span, 1);
        assert_eq!(manifest.params[2].cell_span, 4);
        assert!(manifest.params.iter().all(|param| param.group.is_none()));
        assert!(manifest.params.iter().all(|param| param.env.is_none()));
        assert!(manifest.params.iter().all(|param| param.role.is_none()));
        assert!(manifest.groups.is_empty());
        assert!(manifest.envelopes.is_empty());
    }

    #[test]
    fn parse_manifest_reads_ui_metadata() {
        let manifest = parse_manifest(
            r#"{
                "processAbi": "dgen-host-abi-v1",
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [
                    {
                        "name": "amp_attack",
                        "cellId": 2,
                        "default": 0.01,
                        "min": 0,
                        "max": 2,
                        "group": "amp",
                        "env": "amp_env",
                        "role": "attack"
                    },
                    {
                        "name": "cutoff",
                        "cellId": 3,
                        "default": 1000,
                        "min": 20,
                        "max": 20000,
                        "group": "filter"
                    }
                ],
                "groups": [
                    { "name": "amp" },
                    { "name": "filter" }
                ],
                "envelopes": [
                    {
                        "name": "amp_env",
                        "group": "amp",
                        "roles": {
                            "attack": "amp_attack",
                            "decay": "amp_decay",
                            "sustain": "amp_sustain",
                            "release": "amp_release"
                        }
                    }
                ]
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.params[0].group.as_deref(), Some("amp"));
        assert_eq!(manifest.params[0].env.as_deref(), Some("amp_env"));
        assert_eq!(manifest.params[0].role.as_deref(), Some("attack"));
        assert_eq!(manifest.params[1].group.as_deref(), Some("filter"));
        assert_eq!(manifest.params[1].env, None);
        assert_eq!(
            manifest
                .groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amp", "filter"]
        );
        assert_eq!(manifest.envelopes.len(), 1);
        assert_eq!(manifest.envelopes[0].name, "amp_env");
        assert_eq!(manifest.envelopes[0].group.as_deref(), Some("amp"));
        assert_eq!(
            manifest.envelopes[0].roles.attack.as_deref(),
            Some("amp_attack")
        );
        assert_eq!(
            manifest.envelopes[0].roles.release.as_deref(),
            Some("amp_release")
        );
    }

    #[test]
    fn dgen_init_message_honors_param_span() {
        let manifest = super::DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 2,
            process_abi: "dgen-host-abi-v1".to_string(),
            total_memory_slots: 16,
            params: vec![
                DGenParam {
                    name: "scalar".to_string(),
                    cell_id: 4,
                    cell_span: 1,
                    default: 0.25,
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                    hidden: false,
                    group: None,
                    env: None,
                    role: None,
                },
                DGenParam {
                    name: "vector".to_string(),
                    cell_id: 8,
                    cell_span: 4,
                    default: 0.5,
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                    hidden: false,
                    group: None,
                    env: None,
                    role: None,
                },
            ],
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 2,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        };

        let init = super::build_init_message_for_voice(0, &manifest, 0);
        let entries = init[10..]
            .chunks_exact(2)
            .map(|entry| (entry[0] as usize, entry[1]))
            .collect::<Vec<_>>();

        assert!(entries.contains(&(4, 0.25)));
        assert!(!entries.contains(&(5, 0.25)));
        assert!(entries.contains(&(8, 0.5)));
        assert!(entries.contains(&(11, 0.5)));
    }

    #[test]
    fn voice_init_message_round_trips_through_dgenlisp_init() {
        let total_memory_slots = 16;
        let manifest = super::DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 2,
            process_abi: "dgen-host-abi-v1".to_string(),
            total_memory_slots,
            params: vec![DGenParam {
                name: "scalar".to_string(),
                cell_id: 4,
                cell_span: 1,
                default: 0.25,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 2,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: Some(7),
        };

        let voice_index = 3usize;
        let init_msg = super::build_init_message_for_voice(0, &manifest, voice_index);
        let mut state = vec![0.0_f32; super::dgen_total_state_slots(total_memory_slots)];
        unsafe {
            super::dgenlisp_init(
                state.as_mut_ptr().cast(),
                48_000,
                128,
                init_msg.as_ptr().cast(),
            );
        }

        assert_eq!(state[1] as usize, total_memory_slots);
        assert_eq!(state[2].to_bits(), super::HEADER_CANARY.to_bits());
        let mem = &state[super::HEADER_SLOTS..super::HEADER_SLOTS + total_memory_slots];
        assert_eq!(mem[4], 0.25, "param default must land in its memory cell");
        assert_eq!(
            mem[7], voice_index as f32,
            "voice cell must carry the voice index"
        );
    }

    #[test]
    fn dgenlisp_init_writes_host_sample_rate_without_shifting_compact_entries() {
        let total_memory_slots = 16;
        let mut state = vec![0.0_f32; super::dgen_total_state_slots(total_memory_slots)];
        let initial_state = [
            7.0,
            total_memory_slots as f32,
            super::HEADER_CANARY,
            2.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            2.0,
            4.0,
            0.25,
            8.0,
            0.5,
        ];

        unsafe {
            super::dgenlisp_init(
                state.as_mut_ptr() as *mut std::ffi::c_void,
                48_000,
                128,
                initial_state.as_ptr() as *const std::ffi::c_void,
            );
        }

        assert_eq!(state[super::DGEN_HOST_SAMPLE_RATE_IDX], 48_000.0);
        assert_eq!(state[super::HEADER_SLOTS + 4], 0.25);
        assert_eq!(state[super::HEADER_SLOTS + 8], 0.5);
    }

    #[test]
    fn dgen_total_state_slots_is_header_plus_single_span_plus_redzone() {
        // ABI v1: one memory span (the old split read/write pair is gone).
        assert_eq!(
            super::dgen_total_state_slots(16),
            super::HEADER_SLOTS + 16 + super::DGEN_STATE_REDZONE_SLOTS
        );
    }

    #[test]
    fn vendored_dgen_abi_header_matches_staged_toolchain_header() {
        use sha2::{Digest, Sha256};

        let sequencer_dir = crate::paths::sequencer_dir().expect("sequencer dir");
        let vendored_path = sequencer_dir.join("audiograph/dgen_abi_v1.h");
        let vendored = std::fs::read_to_string(&vendored_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", vendored_path.display()));
        let recorded_sha = vendored
            .lines()
            .find_map(|line| line.trim().strip_prefix("* Source-sha256: "))
            .map(str::trim)
            .expect("dgen_abi_v1.h must record a `Source-sha256:` line");

        let staged_path = crate::app_paths::app_paths()
            .dgen_toolchain_root()
            .join("include/dgen_runtime.h");
        let staged = std::fs::read(&staged_path).unwrap_or_else(|e| {
            panic!(
                "read staged toolchain header {}: {e} (run rebuild_dgenlisp_tool.sh \
                 to stage the vendored toolchain)",
                staged_path.display()
            )
        });
        let staged_sha = format!("{:x}", Sha256::digest(&staged));
        assert_eq!(
            staged_sha, recorded_sha,
            "staged include/dgen_runtime.h drifted from the recorded hash — \
             re-vendor audiograph/dgen_abi_v1.h from the staged header's ABI \
             section and update its Source-sha256 comment"
        );
    }

    #[test]
    fn dgenlisp_wrapper_passes_header_sample_rate_to_generated_process() {
        let total_memory_slots = 4;
        let mut state = vec![0.0_f32; super::dgen_total_state_slots(total_memory_slots)];
        state[0] = 17.0;
        state[1] = total_memory_slots as f32;
        state[2] = super::HEADER_CANARY;
        state[3] = 1.0;
        state[super::DGEN_ENABLED_PARAM_IDX] = 1.0;
        state[super::DGEN_HOST_SAMPLE_RATE_IDX] = 48_000.0;
        let process_fn_chunks = super::process_fn_pointer_chunks(capture_dgen_sample_rate_process);
        for (chunk, value) in process_fn_chunks.into_iter().enumerate() {
            state[super::DGEN_PROCESS_FN_START_IDX + chunk] = value;
        }

        CAPTURED_DGEN_SAMPLE_RATE_BITS.store(0, Ordering::SeqCst);

        let mut input = vec![0.0_f32; 8];
        let mut output = vec![0.0_f32; 8];
        let inputs = [input.as_mut_ptr()];
        let outputs = [output.as_mut_ptr()];
        unsafe {
            super::dgenlisp_wrapper_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                8,
                state.as_mut_ptr() as *mut std::ffi::c_void,
                std::ptr::null_mut(),
            );
        }
        assert_eq!(
            f32::from_bits(CAPTURED_DGEN_SAMPLE_RATE_BITS.load(Ordering::SeqCst)),
            48_000.0
        );
    }

    #[test]
    fn dgenlisp_nodes_keep_distinct_process_identity_while_coexisting() {
        fn state_for(process_fn: super::DGenProcessFn) -> Vec<f32> {
            let mut state = vec![0.0_f32; super::dgen_total_state_slots(1)];
            state[1] = 1.0;
            state[2] = super::HEADER_CANARY;
            state[3] = 1.0;
            state[super::DGEN_ENABLED_PARAM_IDX] = 1.0;
            state[super::DGEN_HOST_SAMPLE_RATE_IDX] = 48_000.0;
            for (chunk, value) in super::process_fn_pointer_chunks(process_fn)
                .into_iter()
                .enumerate()
            {
                state[super::DGEN_PROCESS_FN_START_IDX + chunk] = value;
            }
            state
        }

        let mut old_state = state_for(write_one_process);
        let mut new_state = state_for(write_two_process);
        let mut input = [0.0_f32; 4];
        let inputs = [input.as_mut_ptr()];
        let mut old_output = [0.0_f32; 4];
        let mut new_output = [0.0_f32; 4];
        let old_outputs = [old_output.as_mut_ptr()];
        let new_outputs = [new_output.as_mut_ptr()];

        unsafe {
            super::dgenlisp_wrapper_process(
                inputs.as_ptr(),
                old_outputs.as_ptr(),
                4,
                old_state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
            super::dgenlisp_wrapper_process(
                inputs.as_ptr(),
                new_outputs.as_ptr(),
                4,
                new_state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }

        assert_eq!(old_output, [1.0; 4]);
        assert_eq!(new_output, [2.0; 4]);
    }

    #[test]
    fn parse_manifest_reads_process_abi_and_tensor_source_sample_rate() {
        let manifest = parse_manifest(
            r#"{
                "version": 3,
                "processAbi": "dgen-host-abi-v1",
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "params": [],
                "tensors": [
                    {
                        "name": "sample",
                        "cellOffset": 4,
                        "shape": [8],
                        "kind": "audio",
                        "mutable": false,
                        "sourceFile": "sample.wav",
                        "sourceSampleRate": 48000
                    }
                ],
                "tensorInitData": []
            }"#,
        )
        .expect("manifest parses");

        assert_eq!(manifest.version, 3);
        assert_eq!(manifest.process_abi, super::DGEN_PROCESS_ABI_V1);
        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].source_sample_rate, Some(48_000));
    }

    #[test]
    fn parse_manifest_rejects_missing_or_mismatched_process_abi() {
        let missing = parse_manifest(r#"{ "dylib": "test.dylib", "totalMemorySlots": 4 }"#)
            .map(|_| ())
            .expect_err("manifest without processAbi must be rejected");
        assert!(
            missing.contains("missing 'processAbi'"),
            "unexpected error: {missing}"
        );

        let stale = parse_manifest(
            r#"{
                "processAbi": "dgen-c-v2-host-sample-rate",
                "dylib": "test.dylib",
                "totalMemorySlots": 4
            }"#,
        )
        .map(|_| ())
        .expect_err("pre-v1 manifest must be rejected");
        assert!(
            stale.contains("dgen-c-v2-host-sample-rate") && stale.contains("dgen-host-abi-v1"),
            "unexpected error: {stale}"
        );
    }

    #[test]
    fn built_in_instrument_dsp_files_do_not_hardcode_44100_sample_rate() {
        fn visit(dir: &std::path::Path, failures: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, failures);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("lisp") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (idx, line) in source.lines().enumerate() {
                    let code = line.split(';').next().unwrap_or("");
                    if code.contains("44100") || code.contains("44.1") {
                        failures.push(format!("{}:{}", path.display(), idx + 1));
                    }
                }
            }
        }

        let mut failures = Vec::new();
        visit(&crate::app_paths::app_paths().instruments_dir(), &mut failures);

        assert!(
            failures.is_empty(),
            "hardcoded 44.1kHz timing constants found in Lisp DSP files:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn parse_manifest_reads_mod_active_flag_and_depth_lanes() {
        let json = r#"
        {
          "processAbi": "dgen-host-abi-v1",
          "totalMemorySlots": 128,
          "params": [
            { "name": "gain", "cellId": 10, "default": 0.5, "min": 0, "max": 1 }
          ],
          "inputs": [],
          "outputs": [],
          "modulators": [
            { "slot": 1, "inputChannel": 4, "name": "mod1" },
            { "slot": 2, "inputChannel": 5, "name": "mod2" }
          ],
          "modDestinations": [
            {
              "name": "gain",
              "paramCellId": 10,
              "activeCellId": 20,
              "depthLanes": [
                { "slot": 1, "depthCellId": 21 },
                { "slot": 2, "depthCellId": 22 }
              ],
              "mode": "additive",
              "min": 0,
              "max": 1
            }
          ],
          "tensors": [],
          "tensorInitData": []
        }
        "#;

        let manifest = parse_manifest(json).expect("manifest parses");
        assert_eq!(manifest.mod_destinations.len(), 1);
        let dest = &manifest.mod_destinations[0];
        assert_eq!(dest.active_cell_id, 20);
        assert_eq!(
            dest.depth_lanes
                .iter()
                .map(|lane| (lane.slot, lane.depth_cell_id))
                .collect::<Vec<_>>(),
            vec![(1, 21), (2, 22)]
        );
    }

    #[test]
    fn parse_manifest_reads_modulation_outputs() {
        let json = r#"
        {
          "processAbi": "dgen-host-abi-v1",
          "totalMemorySlots": 128,
          "inputs": [],
          "outputs": [{ "channel": 0, "name": "audio" }],
          "modOutputs": [
            { "slot": 1, "channel": 2, "name": "macro-a", "range": "unipolar" },
            { "slot": 2, "channel": 3, "name": "macro-b", "range": "unipolar" }
          ],
          "tensors": [],
          "tensorInitData": []
        }
        "#;

        let manifest = parse_manifest(json).expect("manifest parses");
        assert_eq!(manifest.n_outputs, 1);
        assert_eq!(manifest.mod_outputs.len(), 2);
        assert_eq!(
            manifest
                .mod_outputs
                .iter()
                .map(|output| (
                    output.slot,
                    output.channel,
                    output.name.as_str(),
                    output.range.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![(1, 2, "macro-a", "unipolar"), (2, 3, "macro-b", "unipolar")]
        );
    }

    #[test]
    fn parse_manifest_defaults_missing_modulation_outputs_to_empty() {
        let manifest = parse_manifest(
            r#"{
                "processAbi": "dgen-host-abi-v1",
                "totalMemorySlots": 16,
                "inputs": [],
                "outputs": [{ "channel": 0, "name": "audio" }]
            }"#,
        )
        .expect("manifest parses");

        assert!(manifest.mod_outputs.is_empty());
        assert_eq!(manifest.n_outputs, 1);
    }

    #[test]
    fn effect_host_modulation_controls_use_effect_local_bank() {
        let manifest = parse_manifest(
            r#"
            {
              "processAbi": "dgen-host-abi-v1",
              "totalMemorySlots": 128,
              "params": [
                { "name": "gain", "cellId": 10, "default": 0.5, "min": 0, "max": 1 }
              ],
              "inputs": [],
              "outputs": [{ "channel": 0, "name": "out" }],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "mod1" },
                { "slot": 2, "inputChannel": 3, "name": "mod2" }
              ],
              "modDestinations": [
                {
                  "name": "gain",
                  "paramCellId": 10,
                  "activeCellId": 20,
                  "depthLanes": [
                    { "slot": 1, "depthCellId": 21 },
                    { "slot": 2, "depthCellId": 22 }
                  ],
                  "mode": "additive",
                  "min": 0,
                  "max": 1
                }
              ],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        let mut desc = EffectDescriptor::from_lisp_manifest(
            "MODDED_GAIN",
            &manifest.params,
            manifest.n_inputs,
            manifest.n_outputs,
        );
        super::append_effect_host_modulation_controls(&mut desc, &manifest);

        assert!(super::effect_has_host_modulation(&manifest));
        assert_eq!(
            desc.instrument_modulators.len(),
            crate::instruments::voice_modulator::SLOT_COUNT
        );
        let mod1_source = desc
            .params
            .iter()
            .find(|param| param.name == "mod1_source")
            .expect("effect descriptor should expose Mod 1 source");
        assert_eq!(mod1_source.default, 0.0);
        assert!(mod1_source.node_param_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE);

        let depth = desc
            .params
            .iter()
            .find(|param| param.name == "mod gain slot 1 amt")
            .expect("effect descriptor should expose DGen depth param");
        assert_eq!(depth.node_param_idx, (super::HEADER_SLOTS + 21) as u32);
        assert_eq!(
            desc.instrument_modulation_targets
                .iter()
                .map(|target| {
                    (
                        desc.params[target.base_param_idx].name.as_str(),
                        target.modulator_slot,
                        desc.params[target.depth_param_idx].name.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("gain", 1, "mod gain slot 1 amt"),
                ("gain", 2, "mod gain slot 2 amt"),
            ]
        );
    }

    #[test]
    fn legacy_effect_modulator_inputs_are_sidechain_controls_without_host_modulation() {
        let manifest = parse_manifest(
            r#"
            {
              "processAbi": "dgen-host-abi-v1",
              "totalMemorySlots": 128,
              "params": [],
              "inputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" },
                { "channel": 2, "name": "signal" }
              ],
              "outputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" }
              ],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "signal" }
              ],
              "modDestinations": [],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        assert_eq!(
            effect_sidechain_inputs(&manifest),
            vec![DGenSidechainInput {
                input_channel: 2,
                name: "sidechain signal".to_string(),
            }]
        );
    }

    #[test]
    fn effect_host_modulation_can_coexist_with_named_sidechain_input() {
        let manifest = parse_manifest(
            r#"
            {
              "processAbi": "dgen-host-abi-v1",
              "totalMemorySlots": 128,
              "params": [
                { "name": "threshold", "cellId": 10, "default": -20, "min": -80, "max": -2 }
              ],
              "inputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" },
                { "channel": 2, "name": "mod1" },
                { "channel": 3, "name": "mod2" },
                { "channel": 4, "name": "mod3" },
                { "channel": 5, "name": "mod4" },
                { "channel": 6, "name": "sidechain" }
              ],
              "outputs": [
                { "channel": 0, "name": "left" },
                { "channel": 1, "name": "right" }
              ],
              "modulators": [
                { "slot": 1, "inputChannel": 2, "name": "mod1" },
                { "slot": 2, "inputChannel": 3, "name": "mod2" },
                { "slot": 3, "inputChannel": 4, "name": "mod3" },
                { "slot": 4, "inputChannel": 5, "name": "mod4" }
              ],
              "modDestinations": [
                {
                  "name": "threshold",
                  "paramCellId": 10,
                  "activeCellId": 20,
                  "depthLanes": [
                    { "slot": 1, "depthCellId": 21 }
                  ],
                  "mode": "additive",
                  "min": -80,
                  "max": -2
                }
              ],
              "tensors": [],
              "tensorInitData": []
            }
            "#,
        )
        .expect("manifest parses");

        assert!(effect_has_host_modulation(&manifest));
        assert_eq!(
            effect_sidechain_inputs(&manifest),
            vec![DGenSidechainInput {
                input_channel: 6,
                name: "sidechain".to_string(),
            }]
        );
    }

    use std::sync::Arc;

    fn neural_test_runtime(track_count: usize) -> (Arc<SequencerState>, Runtime) {
        let state = Arc::new(SequencerState::new(
            track_count,
            (0..track_count)
                .map(|_| default_empty_effect_chain())
                .collect(),
        ));
        let mut runtime = Runtime::new();
        runtime.register_reactive(
            "SEQ",
            vec![
                ("current-pattern", Value::Number(0.0)),
                ("neural-networks", Value::List(Vec::new())),
                ("neural-energy-matrix", Value::List(Vec::new())),
                ("neural-trigger-matrix", Value::List(Vec::new())),
                ("neural-dampening-matrix", Value::List(Vec::new())),
                ("selected-neural-neurons", Value::List(Vec::new())),
            ],
            true,
        );
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(track_count),
                fallback_instrument_descriptors(track_count),
            ),
        );
        (state, runtime)
    }

    fn scene_slot_test_runtime() -> (Arc<SequencerState>, Runtime) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", vec![("current-pattern", Value::Number(0.0))], true);
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );
        (state, runtime)
    }

    /// The UI VM (`ui::natives::init_runtime`) does not install the full
    /// sequencer native set; it registers the scene-slot natives on their own.
    /// Guard that this registration stands alone, since a `defscene` read or
    /// `set!` in a UI script has no other lowering target.
    #[test]
    fn scene_slot_natives_register_without_the_rest_of_the_sequencer_natives() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        super::register_scene_slot_natives(&mut runtime, Arc::clone(&state));

        assert_eq!(
            runtime
                .eval_str("(defscene rate 0.5)\nrate")
                .expect("declare and read a scene slot in a bare runtime"),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime.eval_str("(set! rate 0.75)").expect("write"),
            Some(Value::Number(0.75))
        );
        assert_eq!(
            state.current_scene_slots().get("rate"),
            Some(&crate::process::ProcessLiteral::Number(0.75))
        );
    }

    #[test]
    fn scene_slot_authoring_write_queues_stable_history_payload() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let scene = state.current_scene_id().expect("current scene identity");
        let mut runtime = Runtime::new();
        super::register_scene_slot_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime
            .eval_str("(defscene amount 0.1)\n(set! amount 0.75)")
            .expect("authoring write");
        let mut editor = eseqlisp::Editor::new(runtime, eseqlisp::EditorConfig::default());
        let commands = editor.drain_host_commands();
        let payload = commands
            .iter()
            .find_map(|command| match command {
                eseqlisp::HostCommand::Custom { name, payload }
                    if name == "scene-slot-history-write" => Some(payload),
                _ => None,
            })
            .expect("scene-slot history command");
        let Value::Map(payload) = payload else {
            panic!("history payload must be a map");
        };
        assert_eq!(
            &*payload["scene-id"].borrow(),
            &Value::String(scene.0.to_string())
        );
        assert_eq!(&*payload["slot"].borrow(), &Value::String("amount".to_string()));
        assert_eq!(&*payload["old-present"].borrow(), &Value::Bool(false));
        assert_eq!(&*payload["new"].borrow(), &Value::Number(0.75));
    }

    #[test]
    fn defscene_bare_read_and_set_lower_to_current_pattern_slot_storage() {
        let (state, mut runtime) = scene_slot_test_runtime();

        assert_eq!(
            runtime
                .eval_str("(defscene figures '(1 2))\nfigures")
                .expect("declare and read scene slot"),
            Some(lisp_list(vec![Value::Number(1.0), Value::Number(2.0)]))
        );
        assert_eq!(
            runtime
                .eval_str("(set! figures '(3 5))")
                .expect("write scene slot"),
            Some(lisp_list(vec![Value::Number(3.0), Value::Number(5.0)]))
        );
        assert_eq!(
            state.current_scene_slots().get("figures"),
            Some(&crate::process::ProcessLiteral::List(vec![
                crate::process::ProcessLiteral::Number(3.0),
                crate::process::ProcessLiteral::Number(5.0),
            ]))
        );

        runtime
            .eval_str("(defscene figures '(8 13))")
            .expect("rebind declaration default");
        assert_eq!(
            runtime.eval_str("figures").expect("read retained override"),
            Some(lisp_list(vec![Value::Number(3.0), Value::Number(5.0)])),
            "re-evaluating a declaration must not overwrite a stored pattern override"
        );
        assert_eq!(
            runtime
                .eval_str("(set! figures (lambda () 1))")
                .expect("native authoring diagnostics return false"),
            Some(Value::Bool(false)),
            "scene-slot writes must reject non-portable values"
        );
        assert_eq!(
            state.current_scene_slots().get("figures"),
            Some(&crate::process::ProcessLiteral::List(vec![
                crate::process::ProcessLiteral::Number(3.0),
                crate::process::ProcessLiteral::Number(5.0),
            ])),
            "a rejected value must not alter the stored override"
        );
    }

    #[test]
    fn defscene_render_reads_invalidate_by_slot_and_by_namespace_sweep() {
        let (_state, mut runtime) = scene_slot_test_runtime();
        runtime
            .eval_str(
                r#"
                (defscene alpha 1)
                (defscene beta 2)
                (effect-buffer "*scene-slot-reactivity*"
                  (h-stack
                    (subtree :key "alpha" (label (str alpha)))
                    (subtree :key "alpha-mirror" (label (str alpha)))
                    (subtree :key "beta" (label (str beta)))
                    (subtree :key "unrelated" (label "static"))))
                "#,
            )
            .expect("render scene-slot readers");
        let initial = runtime.take_pending_buffer_widget_trees();
        assert!(
            matches!(
                initial.as_slice(),
                [eseqlisp::vm::PendingUiUpdate::FullTree(_)]
            ),
            "initial render should publish exactly one full tree, got {} updates",
            initial.len()
        );

        // Every scene-slot invalidation must arrive as targeted subtree
        // replacements: a full-tree update here is the whole-UI-repaint
        // failure mode this seam exists to prevent.
        let replaced_roots = |updates: Vec<eseqlisp::vm::PendingUiUpdate>, label: &str| {
            let mut roots = updates
                .into_iter()
                .map(|update| match update {
                    eseqlisp::vm::PendingUiUpdate::ReplaceSubtree {
                        subtree_root_id, ..
                    } => subtree_root_id,
                    eseqlisp::vm::PendingUiUpdate::FullTree(_) => {
                        panic!("{label} must not repaint the full tree")
                    }
                })
                .collect::<Vec<_>>();
            roots.sort_unstable();
            roots
        };

        runtime.eval_str("(set! alpha 3)").expect("write alpha");
        let alpha_roots = replaced_roots(runtime.take_pending_buffer_widget_trees(), "alpha write");
        assert_eq!(
            alpha_roots.len(),
            2,
            "a write must replace every subtree reading that slot and nothing else"
        );

        runtime.eval_str("(set! beta 4)").expect("write beta");
        let beta_roots = replaced_roots(runtime.take_pending_buffer_widget_trees(), "beta write");
        assert_eq!(
            beta_roots.len(),
            1,
            "a write must not touch subtrees reading a different slot"
        );
        assert!(
            !alpha_roots.contains(&beta_roots[0]),
            "slot writes must target disjoint readers"
        );

        runtime
            .eval_str("(set! alpha 3)")
            .expect("repeat equal alpha write");
        assert_eq!(
            replaced_roots(
                runtime.take_pending_buffer_widget_trees(),
                "equal alpha write",
            ),
            alpha_roots,
            "every authored write advances the slot generation, and re-rendered \
             subtrees keep their injected dependency"
        );

        // What a pattern sync does: sweep the namespace, handing every
        // subscribed slot the newly-current scene's generation.
        runtime.queue_reactive_namespace_invalidation(
            super::SCENE_SLOT_REACTIVE_NAMESPACE,
            |_| Value::String("scene-2".to_string()),
        );
        runtime.run_reactive_cycle();
        let mut expected = alpha_roots.clone();
        expected.extend_from_slice(&beta_roots);
        expected.sort_unstable();
        assert_eq!(
            replaced_roots(runtime.take_pending_buffer_widget_trees(), "pattern switch"),
            expected,
            "pattern switch should replace every scene-slot reader and no unrelated subtree"
        );

        assert_eq!(
            runtime
                .eval_str("(set! alpha (lambda () 9))")
                .expect("rejected write reports false"),
            Some(Value::Bool(false))
        );
        assert!(
            runtime.take_pending_buffer_widget_trees().is_empty(),
            "a rejected write must not invalidate the slot"
        );
    }

    #[test]
    fn defscene_non_render_reads_resolve_without_retaining_dependencies() {
        let (_state, mut runtime) = scene_slot_test_runtime();
        assert_eq!(
            runtime
                .eval_str("(defscene immediate 1)\nimmediate")
                .expect("plain read"),
            Some(Value::Number(1.0))
        );
        runtime
            .eval_str("(set! immediate 2)")
            .expect("plain write");
        assert!(
            runtime.take_pending_buffer_widget_trees().is_empty(),
            "non-render reads must not create reactive UI work"
        );
    }

    #[test]
    fn defscene_uses_defstate_module_qualification_and_declaration_order() {
        let (state, mut runtime) = scene_slot_test_runtime();
        runtime
            .eval_str("(def figures 41)\n(def before figures)\n(defscene figures 2)")
            .expect("compile declaration-order fixture");
        assert_eq!(runtime.eval_str("before").unwrap(), Some(Value::Number(41.0)));
        assert_eq!(runtime.eval_str("figures").unwrap(), Some(Value::Number(2.0)));

        runtime
            .eval_str(
                "(module test.scene-slots)\n(defscene rate 0.5)\n(set! rate 0.75)",
            )
            .expect("declare module-qualified scene slot");
        assert_eq!(
            state.current_scene_slots().get("test.scene-slots/rate"),
            Some(&crate::process::ProcessLiteral::Number(0.75))
        );
        assert_eq!(
            runtime
                .eval_str("test.scene-slots/rate")
                .expect("qualified read"),
            Some(Value::Number(0.75))
        );
    }

    #[test]
    fn neural_lisp_create_list_describe_delete() {
        let (state, mut runtime) = neural_test_runtime(1);

        let created = runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();
        assert!(matches!(created, Some(Value::Map(_))));

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].id, 1);
        assert_eq!(networks[0].name, "drums");
        assert_eq!(networks[0].num_neurons, 3);
        assert_eq!(networks[0].neurons.len(), 3);
        assert_eq!(networks[0].weights, vec![vec![0.0; 3]; 3]);

        let listed = runtime.eval_str("(neural-list)").unwrap();
        match listed {
            Some(Value::List(items)) => assert_eq!(items.len(), 1),
            other => panic!("expected neural-list to return list, got {other:?}"),
        }

        let described = runtime.eval_str("(neural-describe \"drums\")").unwrap();
        assert!(matches!(described, Some(Value::Map(_))));

        let deleted = runtime.eval_str("(neural-delete \"drums\")").unwrap();
        assert_eq!(deleted, Some(Value::Bool(true)));
        assert!(state.current_neural_networks().is_empty());
    }

    #[test]
    fn neural_lisp_enable_set_and_neuron_edit() {
        let (state, mut runtime) = neural_test_runtime(2);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 2 :enabled false)")
            .unwrap();

        let enabled = runtime.eval_str("(neural-enable \"drums\" true)").unwrap();
        assert!(matches!(enabled, Some(Value::Map(_))));

        runtime
            .eval_str(
                "(neural-set \"drums\" :reset-bars 2 :energy-decay 0.5 :max-poly 4 :max-poly-selection :random :name \"kit\")",
            )
            .unwrap();
        runtime
            .eval_str(
                "(neural-neuron \"kit\" 1 :route 1 :resolution :8 :threshold 0.75 :delay 3 :quantize :16 :transpose -12 :dampening 0.2 :recovery 0.9)",
            )
            .unwrap();

        let networks = state.current_neural_networks();
        let network = &networks[0];
        assert_eq!(network.name, "kit");
        assert!(network.enabled);
        assert_eq!(network.reset_interval_bars, 2.0);
        assert_eq!(network.energy_decay, 0.5);
        assert_eq!(network.max_poly, 4);
        assert_eq!(network.max_poly_selection, NeuralMaxPolySelection::Random);

        let neuron = &network.neurons[1];
        assert_eq!(neuron.route, Some(1));
        assert_eq!(
            neuron.resolution_timebase(),
            crate::sequencer::Timebase::Eighth
        );
        assert_eq!(neuron.threshold, 0.75);
        assert_eq!(neuron.delay_steps, 3);
        assert_eq!(
            neuron.quantize_timebase(),
            Some(crate::sequencer::Timebase::Sixteenth)
        );
        assert_eq!(neuron.transpose, -12.0);
        assert_eq!(neuron.dampening_amount, 0.2);
        assert_eq!(neuron.dampening_recovery, 0.9);

        runtime
            .eval_str("(neural-set \"kit\" :max-poly-selection :propagation)")
            .unwrap();
        assert_eq!(
            state.current_neural_networks()[0].max_poly_selection,
            NeuralMaxPolySelection::Propagation
        );
    }

    #[test]
    fn neural_lisp_network_edits_do_not_bump_pattern_epoch() {
        let (state, mut runtime) = neural_test_runtime(2);
        let created = runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap()
            .unwrap();
        let Value::Map(created) = created else {
            panic!("expected created network map");
        };
        let id = match created.get("id").map(|value| value.borrow().clone()) {
            Some(Value::Number(id)) => id as u64,
            other => panic!("expected created network id, got {other:?}"),
        };
        let epoch_before = state.transport.pattern_epoch.load(Ordering::Relaxed);

        runtime
            .eval_str(&format!(
                "(neural-neuron {id} 1 :route 1 :delay 3 :dampening 0.5)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!("(neural-weight {id} :from 0 :to 1 :value 0.75)"))
            .unwrap();
        runtime
            .eval_str(&format!("(neural-set {id} :reset-bars 2 :max-poly 4)"))
            .unwrap();

        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before,
            "neural network authoring should publish a scheduler snapshot without forcing a pattern-epoch reset"
        );
    }

    #[test]
    fn neural_lisp_weights_matrix_and_single_cell() {
        let (state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();

        runtime
            .eval_str("(neural-weights \"drums\" '((0 0.5 0) (0 0 0.25) (1 0 0)))")
            .unwrap();
        let networks = state.current_neural_networks();
        assert_eq!(
            networks[0].weights,
            vec![
                vec![0.0, 0.5, 0.0],
                vec![0.0, 0.0, 0.25],
                vec![1.0, 0.0, 0.0],
            ]
        );

        let updated = runtime
            .eval_str("(neural-weight \"drums\" :from 0 :to 2 :value 0.9)")
            .unwrap();
        assert!(matches!(updated, Some(Value::Map(_))));
        assert_eq!(state.current_neural_networks()[0].weights[0][2], 0.9);
    }

    #[test]
    fn neural_lisp_selects_and_clears_neuron_selection() {
        let (_state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"drums\" :neurons 3)")
            .unwrap();

        let selected = runtime
            .eval_str("(neural-select-neuron \"drums\" 2)")
            .unwrap()
            .expect("selection list");
        let Value::List(items) = selected else {
            panic!("expected selected neuron list");
        };
        assert_eq!(items.len(), 1);
        let Value::Map(selected) = &*items[0].borrow() else {
            panic!("expected selected neuron map");
        };
        assert_eq!(
            selected.get("pattern").map(|value| value.borrow().clone()),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            selected
                .get("network-id")
                .map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            selected.get("neuron").map(|value| value.borrow().clone()),
            Some(Value::Number(2.0))
        );
        assert_eq!(
            runtime.eval_str("(neural-neuron-selected? 1 2)").unwrap(),
            Some(Value::Bool(true))
        );

        let cleared = runtime.eval_str("(neural-clear-selection)").unwrap();
        assert!(matches!(cleared, Some(Value::List(items)) if items.is_empty()));
        assert_eq!(
            runtime.eval_str("(neural-neuron-selected? 1 2)").unwrap(),
            Some(Value::Bool(false))
        );

        runtime
            .eval_str("(neural-select-neuron \"drums\" 1)")
            .unwrap();
        runtime.eval_str("(neural-delete \"drums\")").unwrap();
        assert_eq!(
            runtime.eval_str("(neural-selected-neurons)").unwrap(),
            Some(Value::List(vec![]))
        );
    }

    #[test]
    fn selected_neural_instrument_plock_helper_records_current_pattern_selection() {
        let (state, mut runtime) = neural_test_runtime(2);
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();

        let mut selection = BTreeSet::new();
        selection.insert(SelectedNeuralNeuron {
            pattern_idx: 0,
            network_id: 1,
            neuron_idx: 0,
        });

        let wrote =
            set_selected_neural_instrument_plocks(&state, &selection, 1, speed_param_idx, 1.25)
                .unwrap();
        assert!(wrote);
        assert_eq!(
            selected_neural_instrument_plock_value(&state, &selection, 1, speed_param_idx),
            Some(1.25)
        );
        assert_eq!(
            state.current_neural_networks()[0].neurons[0]
                .output_overrides
                .instrument[0]
                .target_track,
            1
        );
    }

    #[test]
    fn neural_plock_clear_helpers_remove_single_network_entry() {
        let (state, mut runtime) = neural_test_runtime(2);
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {speed_param_idx} 1.5)"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-plock-effect \"router\" 0 1 0 0 800.0)")
            .unwrap();

        assert!(
            clear_neural_instrument_plock_by_network_id(&state, 1, 0, 1, speed_param_idx).unwrap()
        );
        assert!(clear_neural_effect_plock_by_network_id(&state, 1, 0, 1, 0, 0).unwrap());
        assert!(
            !clear_neural_instrument_plock_by_network_id(&state, 1, 0, 1, speed_param_idx).unwrap()
        );

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert!(neuron.output_overrides.instrument.is_empty());
        assert!(neuron.output_overrides.effects.is_empty());
    }

    #[test]
    fn neural_lisp_plock_authoring_targets_tracks_and_devices() {
        let (state, mut runtime) = neural_test_runtime(2);
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

        runtime
            .eval_str("(neural-create :name \"router\" :neurons 2)")
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {sampler_speed_param_idx} 1.5)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-plock-instrument \"router\" 0 1 {sampler_speed_param_idx} 2.0)"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-plock-effect \"router\" 0 1 0 0 800.0)")
            .unwrap();

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert_eq!(
            neuron.output_overrides.instrument,
            vec![crate::neural::ProjectParamOverride {
                target_track: 1,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: sampler_speed_node_param_idx,
                },
                param_index: sampler_speed_param_idx,
                value: 2.0,
            }]
        );
        assert_eq!(
            neuron.output_overrides.effects,
            vec![crate::neural::ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: EffectDescriptor::builtin_filter().params[0].node_param_idx,
                },
                param_index: 0,
                value: 800.0,
            }]
        );

        runtime
            .eval_str(&format!(
                "(neural-clear-instrument-plock \"router\" 0 1 {sampler_speed_param_idx})"
            ))
            .unwrap();
        runtime
            .eval_str("(neural-clear-effect-plock \"router\" 0 1 0 0)")
            .unwrap();

        let networks = state.current_neural_networks();
        let neuron = &networks[0].neurons[0];
        assert!(neuron.output_overrides.instrument.is_empty());
        assert!(neuron.output_overrides.effects.is_empty());
    }

    #[test]
    fn neural_lisp_track_router_script_is_idempotent_and_routes_tracks() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/neural-8x8-track-router.lisp"))
        .expect("read neural router script");

        let first = runtime.eval_str(&source).unwrap();
        let first_status = runtime.take_status_message();
        assert!(
            matches!(first, Some(Value::Map(_))),
            "expected first script eval to return map, got {first:?}; status {first_status:?}"
        );
        let second = runtime.eval_str(&source).unwrap();
        assert!(
            matches!(second, Some(Value::Map(_))),
            "expected second script eval to return map, got {second:?}"
        );

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        let network = &networks[0];
        assert_eq!(network.name, "8x8-track-router2");
        assert_eq!(network.num_neurons, 8);
        assert_eq!(network.reset_interval_bars, 4.0);
        assert_eq!(network.energy_decay, 0.994);
        assert_eq!(network.max_poly, 2);
        assert_eq!(
            network.max_poly_selection,
            NeuralMaxPolySelection::Deterministic
        );
        assert_eq!(
            network.weights,
            vec![
                vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ]
        );
        let routes = network
            .neurons
            .iter()
            .map(|neuron| neuron.route)
            .collect::<Vec<_>>();
        assert_eq!(
            routes,
            vec![
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(6),
                Some(7),
            ]
        );
        assert_eq!(
            network
                .neurons
                .iter()
                .map(|neuron| neuron.delay_steps)
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1, 1, 1, 1]
        );
        assert!(network
            .neurons
            .iter()
            .all(|neuron| neuron.quantize_timebase().is_none()));
        assert!(network.neurons.iter().all(|neuron| neuron.transpose == 0.0));
        assert!(network.neurons.iter().all(|neuron| neuron.threshold == 1.0));
        assert!(network
            .neurons
            .iter()
            .all(|neuron| neuron.dampening_amount == 0.0));
        assert!(network
            .neurons
            .iter()
            .all(|neuron| (neuron.dampening_recovery - 0.98).abs() < f32::EPSILON));
        assert!(!state.pattern.neural_reset_patterns[0].is_active(0));
    }

    #[test]
    fn neural_lisp_track_router_route_dropdown_supports_track_16() {
        let (state, mut runtime) = neural_test_runtime(16);
        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/neural-8x8-track-router.lisp"))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let options = runtime
            .eval_str("neural-8x8-track-router-route-options")
            .unwrap()
            .expect("route options");
        let Value::List(options) = options else {
            panic!("expected route options list, got {options:?}");
        };
        assert!(
            options.iter().any(
                |option| matches!(&*option.borrow(), Value::String(value) if value == "Track 16")
            ),
            "route dropdown should include Track 16"
        );
        assert_eq!(
            runtime
                .eval_str("(neural-8x8-track-router-route-index \"Track 16\")")
                .unwrap(),
            Some(Value::Number(15.0))
        );
        assert_eq!(
            runtime
                .eval_str("(neural-8x8-track-router-route-label 15)")
                .unwrap(),
            Some(Value::String("Track 16".to_string()))
        );

        runtime
            .eval_str(
                "(do
                  (set! neural-8x8-track-router-route-0 \"Track 16\")
                  (neural-8x8-track-router-apply-neuron-0))",
            )
            .unwrap();
        assert_eq!(
            state.current_neural_networks()[0].neurons[0].route,
            Some(15)
        );
    }

    #[test]
    fn neural_lisp_track_router_reuses_existing_named_network() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/neural-8x8-track-router.lisp"))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let initial = state.current_neural_networks();
        assert_eq!(initial.len(), 1);
        let id = initial[0].id;

        runtime
            .eval_str(&format!("(neural-weight {id} :from 0 :to 1 :value 0.25)"))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-set {id} :reset-bars 2 :energy-decay 0.5 :max-poly 4 :max-poly-selection :random)"
            ))
            .unwrap();
        runtime
            .eval_str(&format!(
                "(neural-neuron {id} 1 :route 7 :threshold 1.5 :delay 4 :quantize :4 :transpose -7 :dampening 0.25 :recovery 0.75)"
            ))
            .unwrap();

        let second = runtime.eval_str(&source).unwrap();
        assert!(
            matches!(second, Some(Value::Map(_))),
            "expected router script to describe reused network, got {second:?}"
        );

        let networks = state.current_neural_networks();
        assert_eq!(networks.len(), 1);
        let network = &networks[0];
        assert_eq!(network.id, id);
        assert_eq!(network.name, "8x8-track-router2");
        assert_eq!(network.reset_interval_bars, 2.0);
        assert_eq!(network.energy_decay, 0.5);
        assert_eq!(network.max_poly, 4);
        assert_eq!(network.max_poly_selection, NeuralMaxPolySelection::Random);
        assert_eq!(network.weights[0][1], 0.25);
        assert_eq!(network.neurons[1].route, Some(7));
        assert_eq!(network.neurons[1].threshold, 1.5);
        assert_eq!(network.neurons[1].delay_steps, 4);
        assert_eq!(
            network.neurons[1].quantize_timebase(),
            Some(crate::sequencer::Timebase::Quarter)
        );
        assert_eq!(network.neurons[1].transpose, -7.0);
        assert_eq!(network.neurons[1].dampening_amount, 0.25);
        assert_eq!(network.neurons[1].dampening_recovery, 0.75);
    }

    #[test]
    fn neural_lisp_track_router_reactive_refresh_loads_model_state() {
        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/neural-8x8-track-router.lisp"))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let id = state.current_neural_networks()[0].id;
        let _ = runtime.take_pending_buffer_widget_trees();

        state
            .edit_current_neural_networks(|networks| {
                let network = networks
                    .iter_mut()
                    .find(|network| network.id == id)
                    .expect("router network");
                network.reset_interval_bars = 3.0;
                network.energy_decay = 0.5;
                network.max_poly = 5;
                network.max_poly_selection = NeuralMaxPolySelection::Random;
                network.weights[0][1] = 0.75;
                network.neurons[0].threshold = 1.75;
                network.neurons[1].route = Some(6);
                network.neurons[1].threshold = 2.5;
                network.neurons[1].delay_steps = 5;
                network.neurons[1].quantize = Some(crate::sequencer::Timebase::Eighth as u8);
                network.neurons[1].transpose = 12.0;
                network.neurons[1].dampening_amount = 0.33;
                network.neurons[1].dampening_recovery = 0.44;
                Ok(())
            })
            .unwrap();

        let epoch_before_refresh = state.transport.pattern_epoch.load(Ordering::Relaxed);
        let outcome = runtime.set_reactive(
            "SEQ",
            "neural-networks",
            Value::List(vec![Rc::new(RefCell::new(Value::Number(id as f64)))]),
        );
        assert!(
            outcome.effects_dirty,
            "router panel should subscribe to SEQ.neural-networks"
        );
        runtime.run_reactive_cycle();
        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before_refresh,
            "reactive panel refresh should not write back unchanged network data"
        );

        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-reset-bars")
                .unwrap(),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-energy-decay")
                .unwrap(),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-max-poly")
                .unwrap(),
            Some(Value::Number(5.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-max-poly-selection")
                .unwrap(),
            Some(Value::String("random".to_string()))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-threshold")
                .unwrap(),
            Some(Value::Number(1.75))
        );
        assert_eq!(
            runtime.eval_str("neural-8x8-track-router-route-1").unwrap(),
            Some(Value::String("Track 7".to_string()))
        );
        assert_eq!(
            runtime.eval_str("neural-8x8-track-router-delay-1").unwrap(),
            Some(Value::Number(5.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-quantize-1")
                .unwrap(),
            Some(Value::String("8".to_string()))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-transpose-1")
                .unwrap(),
            Some(Value::Number(12.0))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-dampening-1")
                .unwrap(),
            Some(Value::Number(0.33_f32 as f64))
        );
        assert_eq!(
            runtime
                .eval_str("neural-8x8-track-router-recovery-1")
                .unwrap(),
            Some(Value::Number(0.44_f32 as f64))
        );
        assert!(
            !runtime.take_pending_buffer_widget_trees().is_empty(),
            "reactive refresh should rebuild the matrix buffer"
        );
    }

    #[test]
    fn neural_lisp_track_router_controls_align_with_matrix_rows() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_width(node: &eseqlisp::layout::LayoutNode, expected: f32, label: &str) {
            assert!(
                (node.rect.width - expected).abs() <= 0.05,
                "{label} should measure to width {expected}, got {:?}",
                node.rect
            );
        }

        let (state, mut runtime) = neural_test_runtime(8);
        let source = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("sequencers/neural-8x8-track-router.lisp"))
        .expect("read neural router script");

        runtime.eval_str(&source).unwrap();
        let pending = runtime.take_pending_buffer_widget_trees();
        let tree = pending
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("router script should publish widget tree");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 18.0)))
            .expect("router widget tree should lay out");

        for (key, text, width) in [
            ("neural-router-column-label-route", "route", 7.68),
            ("neural-router-column-label-delay", "delay", 5.04),
            ("neural-router-column-label-quantize", "quant", 5.76),
            ("neural-router-column-label-transpose", "transp", 5.04),
            ("neural-router-column-label-dampening", "damp", 5.04),
            ("neural-router-column-label-recovery", "recov", 5.04),
        ] {
            let label = find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(label.widget_type, "label", "{key}");
            assert_eq!(
                label.props.get("text"),
                Some(&Value::String(text.to_string())),
                "{key}"
            );
            assert_measured(label);
            assert_width(label, width, key);
        }

        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(
            matrices.len(),
            4,
            "expected trigger, energy, weight, and dampening matrix widgets"
        );
        matrices.sort_by(|left, right| left.rect.col.total_cmp(&right.rect.col));
        for matrix in &matrices {
            assert_measured(matrix);
        }
        let matrix = matrices[2];
        for (idx, visualization_matrix) in matrices.iter().enumerate() {
            assert!(
                (matrix.rect.row - visualization_matrix.rect.row).abs() <= 0.05
                    && (matrix.rect.height - visualization_matrix.rect.height).abs() <= 0.05,
                "visualization matrix {idx} should align with weight matrix; weight={:?} visualization={:?}",
                matrix.rect,
                visualization_matrix.rect
            );
        }

        let label_3 =
            find_by_stable_key(&layout, "neural-router-row-label-3").expect("row 3 label");
        let label_click = label_3
            .props
            .get("on-click")
            .cloned()
            .expect("row label on-click");
        runtime
            .invoke(label_click, vec![Value::Bool(true)])
            .expect("invoke row label click");
        assert_eq!(
            runtime
                .eval_str("(neural-neuron-selected? neural-8x8-track-router-id 2)")
                .unwrap(),
            Some(Value::Bool(true))
        );
        let row_3 = find_by_stable_key(&layout, "neural-router-row-3").expect("selected row 3");
        let mut row_3_dropdowns = Vec::new();
        collect_widgets(row_3, "dropdown", &mut row_3_dropdowns);
        assert_eq!(
            row_3_dropdowns.len(),
            2,
            "row 3 should contain route and quantize dropdowns"
        );
        row_3_dropdowns.sort_by(|left, right| left.rect.col.total_cmp(&right.rect.col));
        assert_width(row_3_dropdowns[0], 7.68, "row route dropdown");
        assert_width(row_3_dropdowns[1], 5.76, "row quantize dropdown");

        let mut row_3_pickers = Vec::new();
        collect_widgets(row_3, "number-picker", &mut row_3_pickers);
        assert_eq!(
            row_3_pickers.len(),
            4,
            "row 3 should contain delay, transpose, dampening, and recovery pickers"
        );
        for picker in row_3_pickers {
            assert_width(picker, 5.04, "row number picker");
        }

        let expected_selected_field = format!(
            "neural-neuron-selected-0-{}-2",
            state.current_neural_networks()[0].id
        );
        assert!(
            matches!(
                row_3.props.get("selected"),
                Some(Value::ReactiveRef { namespace, field, .. })
                    if namespace == "SEQ" && field == &expected_selected_field
            ),
            "row 3 should bind selected state to its targeted neural selection field"
        );
        assert_eq!(
            row_3.props.get("selected-background-color"),
            Some(&Value::Keyword("fx-panel-header-selected-bg".to_string()))
        );

        let clear_callback = layout
            .props
            .get("on-click")
            .cloned()
            .expect("outer panel click clears selection");
        runtime
            .invoke(clear_callback, vec![Value::Bool(true)])
            .expect("invoke outer panel click");
        assert_eq!(
            runtime.eval_str("(neural-selected-neurons)").unwrap(),
            Some(Value::List(vec![]))
        );

        let mut dropdowns = Vec::new();
        collect_widgets(&layout, "dropdown", &mut dropdowns);
        assert_eq!(
            dropdowns.len(),
            17,
            "expected one global max-poly dropdown plus route and quantize dropdowns"
        );
        for dropdown in &dropdowns {
            assert_measured(dropdown);
        }

        let mut pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut pickers);
        assert_eq!(
            pickers.len(),
            36,
            "expected four global pickers and four pickers per neuron"
        );
        for picker in &pickers {
            assert_measured(picker);
        }

        let mut row_pickers = pickers
            .into_iter()
            .filter(|picker| {
                let center = picker.rect.row + picker.rect.height * 0.5;
                center >= matrix.rect.row && center <= matrix.rect.row + matrix.rect.height
            })
            .collect::<Vec<_>>();
        assert_eq!(
            row_pickers.len(),
            32,
            "expected four row-aligned pickers per neuron"
        );
        row_pickers.sort_by(|left, right| {
            left.rect
                .row
                .total_cmp(&right.rect.row)
                .then(left.rect.col.total_cmp(&right.rect.col))
        });

        let matrix_row_height = matrix.rect.height / 8.0;
        for (idx, picker) in row_pickers.iter().enumerate() {
            let row_idx = idx / 4;
            let expected_center = matrix.rect.row + matrix_row_height * (row_idx as f32 + 0.5);
            let actual_center = picker.rect.row + picker.rect.height * 0.5;
            assert!(
                (actual_center - expected_center).abs() <= 0.05,
                "row picker {idx} center {actual_center} should align with matrix row center {expected_center}; picker={:?} matrix={:?}",
                picker.rect,
                matrix.rect
            );
        }
    }

    #[test]
    fn neural_lisp_reset_step_sets_dedicated_flag() {
        let (state, mut runtime) = neural_test_runtime(1);

        let enabled = runtime
            .eval_str("(neural-reset-step :track 0 :step 4 true)")
            .unwrap();
        assert_eq!(enabled, Some(Value::Bool(true)));
        assert!(state.pattern.neural_reset_patterns[0].is_active(4));

        let disabled = runtime.eval_str("(neural-reset-step 0 4 false)").unwrap();
        assert_eq!(disabled, Some(Value::Bool(false)));
        assert!(!state.pattern.neural_reset_patterns[0].is_active(4));
    }

    #[test]
    fn neural_lisp_rejects_bad_matrix_shape() {
        let (state, mut runtime) = neural_test_runtime(1);

        let result = runtime
            .eval_str("(neural-create :name \"bad\" :neurons 2 :weights '((0 1)))")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(false)));
        assert!(state.current_neural_networks().is_empty());
    }

    #[test]
    fn neural_lisp_rejects_ambiguous_name_lookup() {
        let (state, mut runtime) = neural_test_runtime(1);
        runtime
            .eval_str("(neural-create :name \"same\" :neurons 1)")
            .unwrap();
        runtime
            .eval_str("(neural-create :name \"same\" :neurons 1)")
            .unwrap();

        let result = runtime.eval_str("(neural-describe \"same\")").unwrap();

        assert_eq!(result, Some(Value::Bool(false)));
        assert_eq!(state.current_neural_networks().len(), 2);
    }

    #[test]
    fn seq_step_returns_map_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step 0)").unwrap();
        assert!(matches!(result, Some(Value::Map(_))));
    }

    #[test]
    fn sequencer_authoring_forms_document_contextual_completion_keywords() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let metadata = runtime
            .symbol_metadata()
            .get("def-sequencer")
            .expect("def-sequencer completion metadata");

        assert!(metadata.keyword_args.iter().any(|keyword| keyword == ":tick"));
        assert!(metadata.keyword_args.iter().any(|keyword| keyword == ":shape"));
        assert!(!metadata.keyword_args.iter().any(|keyword| keyword == ":16"));

        let seq_emit = runtime
            .symbol_metadata()
            .get("seq-emit")
            .expect("seq-emit completion metadata");
        assert!(seq_emit.keyword_args.iter().any(|keyword| keyword == ":track"));
        assert!(seq_emit.keyword_args.iter().any(|keyword| keyword == ":quantize"));
        assert!(!seq_emit.keyword_args.iter().any(|keyword| keyword == ":now"));
    }

    #[test]
    fn def_process_documents_contextual_completion_keywords() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_published_process_authoring_natives(
            &mut runtime,
            state,
            Arc::new(AtomicUsize::new(0)),
        );

        let metadata = runtime
            .symbol_metadata()
            .get("def-process")
            .expect("def-process completion metadata");

        for expected in [":doc", ":in", ":targets", ":listen", ":run"] {
            assert!(
                metadata.keyword_args.iter().any(|keyword| keyword == expected),
                "def-process should offer {expected}"
            );
        }
    }

    #[test]
    fn seq_track_steps_returns_list_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-track-steps)").unwrap();
        assert!(matches!(result, Some(Value::List(_))));
    }

    #[test]
    fn seq_set_current_track_updates_context_for_following_calls() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(2),
                fallback_instrument_descriptors(2),
            ),
        );

        let result = runtime.eval_str("(seq-set-current-track 1)").unwrap();
        assert_eq!(result, Some(Value::Number(1.0)));

        let result = runtime.eval_str("(seq-current-track)").unwrap();
        assert_eq!(result, Some(Value::Number(1.0)));

        let result = runtime.eval_str("(seq-toggle-step 0)").unwrap();
        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[1].is_active(0));
        assert!(!state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn seq_step_on_activates_step_without_toggle_semantics() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step-on 2)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[0].is_active(2));
    }

    #[test]
    fn seq_step_off_clears_payload_and_deactivates_step() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.8);
        state.pattern.effect_chains[0][0].set_plock(2, 0, 0.25);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-step-off 2)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.effect_chains[0][0].plocks.get(2, 0), None);
    }

    #[test]
    fn seq_rotate_track_rotates_full_pattern() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 7.0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );
        let result = runtime.eval_str("(seq-rotate-track 1)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 7.0);
    }

    #[test]
    fn seq_plock_step_sets_step_param() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime
            .eval_str("(seq-plock-step 1 :velocity 0.7)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Velocity), 0.7);
    }

    #[test]
    fn seq_plock_timebase_sets_timebase_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-plock-timebase 2 :8t)").unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.timebase_plocks[0].get(2),
            Some(crate::sequencer::Timebase::EighthTriplet)
        );
    }

    #[test]
    fn seq_plock_effect_normalizes_slot_param_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        bind_filter_slot(&state);
        let mut runtime = Runtime::new();
        let effect_descriptors = descriptors_with_filter(1);
        let expected = effect_descriptors[0][0].params[2].denormalize(0.5);
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(effect_descriptors, fallback_instrument_descriptors(1)),
        );

        let result = runtime
            .eval_str("(seq-plock-effect 0 FILTER.cutoff 0.5)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(0, 2),
            Some(expected)
        );
    }

    #[test]
    fn seq_plock_effect_raw_preserves_stored_value() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        bind_filter_slot(&state);
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime
            .eval_str("(seq-plock-effect-raw 0 0 2 440.0)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(0, 2),
            Some(440.0)
        );
    }

    #[test]
    fn seq_effect_param_name_returns_effect_param_name() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-effect-param-name 0 2)").unwrap();

        assert_eq!(result, Some(Value::String("cutoff".to_string())));
    }

    #[test]
    fn seq_effect_param_names_returns_effect_param_name_list() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("(seq-effect-param-names 0)").unwrap();

        match result {
            Some(Value::List(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::String(name) => name.clone(),
                        other => panic!("expected string, got {other:?}"),
                    })
                    .collect();
                assert!(names.starts_with(&[
                    "enabled".to_string(),
                    "mode".to_string(),
                    "cutoff".to_string(),
                    "resonance".to_string(),
                ]));
                assert!(names.contains(&"drive".to_string()));
                assert!(names.contains(&"lfo amt".to_string()));
                assert!(names.contains(&"env amt".to_string()));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn seq_effect_globals_expose_slot_and_param_refs() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(
                descriptors_with_filter(1),
                fallback_instrument_descriptors(1),
            ),
        );

        let result = runtime.eval_str("FILTER.cutoff").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0, 2.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn seq_instrument_globals_expose_param_refs() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let custom_desc = EffectDescriptor::from_lisp_manifest(
            "MINIMOOG",
            &[DGenParam {
                name: "cutoff".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            0,
            0,
        );
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![custom_desc]),
        );

        let result = runtime.eval_str("MINIMOOG.cutoff").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn scratch_runtime_with_fallbacks_uses_state_published_descriptors() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let custom_desc = EffectDescriptor::from_lisp_manifest(
            "MODUM_DELAY",
            &[DGenParam {
                name: "max1".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.0,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            2,
            2,
        );
        let mut effect_descriptors = fallback_effect_descriptors(1);
        effect_descriptors[0][0] = custom_desc;
        state.set_scratch_runtime_descriptors(
            effect_descriptors,
            fallback_instrument_descriptors(1),
        );

        let mut runtime = scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
        let result = runtime.eval("MODUM_DELAY.max1").unwrap();

        match result {
            Some(Value::List(items)) => {
                let values: Vec<f64> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::Number(value) => *value,
                        other => panic!("expected numeric ref component, got {other:?}"),
                    })
                    .collect();
                assert_eq!(values, vec![0.0, 0.0]);
            }
            other => panic!("expected ref list, got {other:?}"),
        }
    }

    #[test]
    fn scratch_runtime_accepts_source_tab_script_contract_as_noop() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);

        runtime
            .eval(r#"(eseq.seq-script-picker/seq-register-script-source-tab "Source Only")"#)
            .expect("source-tab script contract should be a scratch-runtime no-op");
    }

    #[test]
    fn seq_plock_instrument_normalizes_slot_param_override() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::from_lisp_manifest(
            "MINIMOOG",
            &[DGenParam {
                name: "cutoff".to_string(),
                cell_id: 0,
                cell_span: 4,
                default: 0.5,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: None,
                env: None,
                role: None,
            }],
            0,
            0,
        );
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);
        let expected = instrument_desc.params[0].denormalize(0.25);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime
            .eval_str("(seq-plock-instrument 0 MINIMOOG.cutoff 0.25)")
            .unwrap();

        assert_eq!(result, Some(Value::Bool(true)));
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(0, 0),
            Some(expected)
        );
    }

    #[test]
    fn seq_instrument_param_name_returns_instrument_param_name() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::builtin_delay();
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime.eval_str("(seq-instrument-param-name 2)").unwrap();

        assert_eq!(result, Some(Value::String("time".to_string())));
    }

    #[test]
    fn seq_instrument_param_names_returns_instrument_param_name_list() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let instrument_desc = EffectDescriptor::builtin_delay();
        state.pattern.instrument_slots[0].apply_descriptor(&instrument_desc, 0);

        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            Arc::clone(&state),
            new_eval_context(0, 0),
            shared_native_metadata(fallback_effect_descriptors(1), vec![instrument_desc]),
        );

        let result = runtime.eval_str("(seq-instrument-param-names)").unwrap();

        match result {
            Some(Value::List(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .map(|item| match &*item.borrow() {
                        Value::String(name) => name.clone(),
                        other => panic!("expected string, got {other:?}"),
                    })
                    .collect();
                assert_eq!(
                    names,
                    vec!["wet", "synced", "time", "feedback", "dampening", "width"]
                );
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn seq_step_shows_value_through_editor_eval_binding() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let init_src = read_eseqlisp_init_source();
        let mut runtime = Runtime::new();
        register_sequencer_natives(
            &mut runtime,
            state,
            new_eval_context(0, 0),
            shared_native_metadata(
                fallback_effect_descriptors(1),
                fallback_instrument_descriptors(1),
            ),
        );
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init_src),
                ..EditorConfig::default()
            },
        );
        editor.open_scratch_buffer_with_mode("*scratch*", "(seq-step 0)", BufferMode::ESeqLisp);
        editor.active_buffer_mut().cursor = (0, "(seq-step 0)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        let minibuffer = editor.minibuffer.unwrap_or_default();
        assert!(minibuffer.contains("step"), "minibuffer was: {minibuffer}");
    }

    #[test]
    fn scratch_control_runtime_can_invoke_exported_closure() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );

        let callback = runtime
            .eval("(lambda () (seq-toggle-step 0))")
            .unwrap()
            .unwrap();
        runtime.set_global_value("__hook_test", callback);
        let result = runtime.eval("(__hook_test)").unwrap().unwrap();

        assert_eq!(result, Value::Bool(true));
        assert!(state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn scratch_control_runtime_source_path_eval_resolves_relative_loads() {
        let unique = format!(
            "eseq-scratch-source-path-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("create temp scratch source dir");
        let loaded_path = dir.join("loaded.lisp");
        std::fs::write(
            &loaded_path,
            r#"
            (def-accumulator "loaded-relative"
              (acc-add-step-param :transpose acc-value))
            (scheduler-should-ignore-ui-only-tail)
            "#,
        )
        .expect("write loaded scratch source");

        let mut runtime = ScratchControlRuntime::new(
            Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()])),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        assert!(
            runtime
                .eval_source_at_path(dir.join(".eseqlisp-scratch"), r#"(load "loaded.lisp")"#)
                .is_err(),
            "scheduler scratch should report the UI-only tail error"
        );

        let _ = std::fs::remove_file(loaded_path);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(
            runtime.accumulator_names(),
            vec!["loaded-relative".to_string()]
        );
    }

    #[test]
    fn scratch_control_runtime_runs_source_hooks_with_dynamic_track_context() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.set_position(1, 0);
        let result = runtime.eval("(seq-toggle-step 0)").unwrap().unwrap();

        assert_eq!(result, Value::Bool(true));
        assert!(state.pattern.patterns[1].is_active(0));
        assert!(!state.pattern.patterns[0].is_active(0));
    }

    #[test]
    fn scratch_control_runtime_registers_and_invokes_accumulator_callbacks() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "test-acc"
                  (do
                     (acc-add-step-param :transpose acc-value)
                     (acc-scale-step-param :velocity 0.5)
                     (acc-set-step-param :pan 0.25)
                     (acc-add-effect-param FILTER.cutoff 0.25)
                     (acc-add-instrument-param 0 0.25)))
                "#,
            )
            .unwrap();

        assert_eq!(runtime.accumulator_names(), vec!["test-acc".to_string()]);

        let effect_desc = EffectDescriptor::builtin_filter();
        let effect_initial = effect_desc.params[2].denormalize(0.5);
        let effect_expected = effect_desc.params[2].denormalize(0.75);
        let instrument_desc = fallback_instrument_descriptors(1)[0].clone();
        let instrument_initial = instrument_desc.params[0].denormalize(0.5);
        let instrument_expected = instrument_desc.params[0].denormalize(0.75);

        let output = runtime
            .invoke_accumulator(
                0,
                3,
                3.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 2.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![1.0, 1.0, 1.0],
                2.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 2,
                    value: effect_initial,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: instrument_initial,
                }],
            )
            .unwrap();

        assert_eq!(output.resolved.transpose, 5.0);
        assert_eq!(output.resolved.velocity, 0.5);
        assert_eq!(output.resolved.pan, 0.25);
        assert!(output.effect_params.iter().any(|param| {
            param.logical_id == 42
                && param.idx == 2
                && (param.value - effect_expected).abs() < 0.001
        }));
        assert!(output.instrument_params.iter().any(|param| param.target
            == ScheduledInstrumentParamTarget::Synth
            && param.idx == 0
            && (param.value - instrument_expected).abs() < 0.001));
    }

    #[test]
    fn scratch_control_runtime_clamps_normalized_accumulator_param_adds() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            descriptors_with_filter(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "clip-acc"
                  (do
                     (acc-add-effect-param FILTER.cutoff 0.75)
                     (acc-add-instrument-param 0 0.75)))
                "#,
            )
            .unwrap();

        let effect_desc = EffectDescriptor::builtin_filter();
        let effect_initial = effect_desc.params[2].denormalize(0.5);
        let effect_expected = effect_desc.params[2].denormalize(1.0);
        let instrument_desc = fallback_instrument_descriptors(1)[0].clone();
        let instrument_initial = instrument_desc.params[0].denormalize(0.5);
        let instrument_expected = instrument_desc.params[0].denormalize(1.0);

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                1.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(&effect_desc, 42)],
                EffectSlotSnapshot::new_default(&instrument_desc, 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 2,
                    value: effect_initial,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: instrument_initial,
                }],
            )
            .unwrap();

        assert!(output.effect_params.iter().any(|param| {
            param.logical_id == 42
                && param.idx == 2
                && (param.value - effect_expected).abs() < 0.001
        }));
        assert!(output.instrument_params.iter().any(|param| {
            param.target == ScheduledInstrumentParamTarget::Synth
                && param.idx == 0
                && (param.value - instrument_expected).abs() < 0.001
        }));
    }

    #[test]
    fn scratch_control_runtime_accumulator_can_emit_arp_events() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp"
                  (do
                     (acc-suppress)
                     (acc-emit 0 :note 0 :vel 0.9)
                     (acc-emit 1 :note 4 :vel 0.8)
                     (acc-emit :8t 1 :note 7 :track 0)))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![1.0, 1.0, 1.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(output.suppressed);
        assert_eq!(output.emitted.len(), 3);
        assert_eq!(output.emitted[0].offset_beats, 0.0);
        assert_eq!(output.emitted[0].resolved.transpose, 0.0);
        assert_eq!(output.emitted[0].resolved.velocity, 0.9);
        assert!(output.emitted[0].chord.is_empty());
        assert_eq!(output.emitted[1].offset_beats, 0.25);
        assert_eq!(output.emitted[1].resolved.transpose, 4.0);
        assert!((output.emitted[2].offset_beats - (1.0 / 3.0)).abs() < 0.0001);
        assert_eq!(output.emitted[2].track, Some(0));
    }

    #[test]
    fn scratch_control_runtime_rejects_invalid_def_accumulator_forms() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        assert_eq!(
            runtime
                .eval(r#"(def-accumulator "missing-body")"#)
                .expect("native validation should return a rejection value"),
            Some(Value::Bool(false))
        );
        assert_eq!(
            runtime
                .eval(
                    r#"
                (def-accumulator "multi-body"
                  (acc-suppress)
                  (acc-emit 0))
                "#
                )
                .expect("native validation should return a rejection value"),
            Some(Value::Bool(false))
        );
        assert!(runtime.accumulators.lock().unwrap().is_empty());
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_follow_chord_durations() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 6.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![6.0, 6.0, 6.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert_eq!(notes, vec![0.0, 4.0, 7.0, 0.0, 4.0, 7.0]);
        assert_eq!(output.emitted.len(), 6);
        assert_eq!(output.emitted[5].offset_beats, 1.25);
        assert_eq!(output.emitted[5].resolved.duration, 1.0);
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_fall_back_to_step_duration() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 6.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![0.0, 0.0, 0.0],
                0.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert_eq!(output.emitted.len(), 6);
    }

    #[test]
    fn scratch_control_runtime_arp_helpers_use_joined_note_pool() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-held"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_accumulator(
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![
                    AccumulatorNoteSpan {
                        transpose: 0.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 4.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 7.0,
                        start_beats: 1.0,
                        end_beats: 2.0,
                    },
                ]),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert_eq!(notes, vec![0.0, 4.0, 0.0, 4.0, 4.0, 7.0, 0.0, 4.0]);
        assert_eq!(output.emitted.len(), 8);
    }

    #[test]
    fn scratch_control_runtime_arp_source_assigns_track() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-accumulator "arp-16"
                  (do
                    (acc-suppress)
                    (for-each |i|
                      (acc-arp-emit :16 i :vel 0.8)
                      (range 0 (acc-arp-count :16)))))

                (seq-use-accumulator 0 "arp-16")
                "#,
            )
            .unwrap();

        let params = &state.pattern.track_params[0];
        assert_eq!(params.script_accumulator_name(), Some("arp-16".to_string()));
        assert_eq!(
            params.get_accumulator_idx(),
            crate::accumulator::ACCUMULATOR_REGISTRY.len()
        );
    }

    #[test]
    fn scratch_control_runtime_midi_fx_source_assigns_track_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "arp-16"
                  (do
                    (fx-suppress)
                    (for-each |i|
                      (fx-arp-emit :16 i :vel 0.8)
                      (range 0 (fx-arp-count :16)))))

                (seq-use-midi-fx 0 "arp-16")
                "#,
            )
            .unwrap();

        let params = &state.pattern.track_params[0];
        assert_eq!(runtime.midi_fx_names(), vec!["arp-16".to_string()]);
        assert_eq!(params.midi_fx_chain(), vec!["arp-16".to_string()]);
        assert_eq!(
            params.get_midi_fx_position(),
            crate::sequencer::MidiFxPosition::PostAccumulator
        );
    }

    #[test]
    fn midi_fx_control_lisp_read_runs_module_alias_preflight() {
        let dir = std::env::temp_dir().join(format!(
            "eseq-midi-fx-alias-preflight-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dsp.lisp");
        let source = "(def helper () (eseq.seq-layout/apply-fx-layout))\n";
        std::fs::write(&path, source).unwrap();
        assert_eq!(super::read_midi_fx_lisp(&path).unwrap(), source);
        assert!(
            eseqlisp::module_alias_migration::warn_on_old_module_aliases(&path, source).is_none(),
            "MIDI-FX control source must be preflighted before concatenation"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn builtin_midi_fx_source_resolves_crate_local_library() {
        let manifest_root = crate::app_paths::app_paths().midi_fx_dir()
            .canonicalize()
            .expect("crate-local midi-fx directory");
        assert!(
            super::midi_fx_library_root_candidates()
                .iter()
                .any(|candidate| candidate == &manifest_root),
            "builtin MIDI FX roots should include the crate-local library"
        );

        let source = super::load_midi_fx_library_source();
        assert!(source.contains("(def-midi-fx \"arp\""));
        assert!(source.contains("(def-midi-fx \"beat-repeat\""));
        assert!(source.contains("(def-midi-fx \"quantizer\""));
        assert!(source.contains("(def-midi-fx \"spatial-harmonic-delay\""));
        assert!(source.contains("(def-midi-fx \"trigger-to-track\""));
        assert!(source.contains("(def-midi-fx \"transpose-range\""));
    }

    #[test]
    fn folder_midi_fx_registers_params_and_syncs_track_slot() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(&super::midi_fx_library_source_with_user_source(
                r#"
                (seq-use-midi-fx 0 "arp")
                (seq-set-midi-fx-param 0 "rate" 3)
                (seq-plock-midi-fx 0 0 "rate" 9)
                "#,
            ))
            .unwrap();

        let params = &state.pattern.track_params[0];
        let slot = &state.pattern.midi_fx_slots[0][0];
        assert!(runtime.midi_fx_names().iter().any(|name| name == "arp"));
        let arp_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "arp")
            .expect("arp descriptor");
        assert_eq!(
            arp_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            Some("clock-rate")
        );
        let descriptors = runtime.midi_fx_descriptors();
        let beat_repeat_desc = descriptors
            .iter()
            .find(|desc| desc.name == "beat-repeat")
            .expect("beat-repeat descriptor");
        assert_eq!(
            beat_repeat_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            Some("clock-rate")
        );
        let spatial_desc = descriptors
            .iter()
            .find(|desc| desc.name == "spatial-harmonic-delay")
            .expect("spatial-harmonic-delay descriptor");
        assert_eq!(
            spatial_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            None
        );
        assert_eq!(params.midi_fx_chain(), vec!["arp".to_string()]);
        assert_eq!(slot.num_params.load(Ordering::Relaxed), 6);
        assert_eq!(slot.defaults.get(0), 3.0);
        assert_eq!(slot.plocks.get(0, 0), Some(9.0));
    }

    #[test]
    fn folder_midi_fx_transpose_range_wraps_notes_into_configured_octaves() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let descriptors = runtime.midi_fx_descriptors();
        let transpose_range_idx = descriptors
            .iter()
            .position(|desc| desc.name == "transpose-range")
            .expect("transpose-range MIDI FX is registered");
        let transpose_range_desc = descriptors
            .get(transpose_range_idx)
            .expect("transpose-range descriptor");
        assert_eq!(transpose_range_desc.params.len(), 3);
        assert_eq!(transpose_range_desc.params[0].name, "min");
        assert_eq!(transpose_range_desc.params[1].name, "max");
        assert_eq!(transpose_range_desc.params[2].name, "enabled");

        let mut slot = EffectSlotSnapshot::new_default(transpose_range_desc, 0);
        slot.defaults[0] = -12.0;
        slot.defaults[1] = 12.0;

        let output = runtime
            .invoke_midi_fx(
                transpose_range_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 4.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![
                    AccumulatorNoteSpan {
                        transpose: -24.0,
                        start_beats: 0.0,
                        end_beats: 0.25,
                    },
                    AccumulatorNoteSpan {
                        transpose: -13.0,
                        start_beats: 0.25,
                        end_beats: 0.5,
                    },
                    AccumulatorNoteSpan {
                        transpose: 5.0,
                        start_beats: 0.5,
                        end_beats: 0.75,
                    },
                    AccumulatorNoteSpan {
                        transpose: 13.0,
                        start_beats: 0.75,
                        end_beats: 1.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 24.0,
                        start_beats: 1.0,
                        end_beats: 1.25,
                    },
                ]),
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        let offsets = output
            .emitted
            .iter()
            .map(|event| event.offset_beats)
            .collect::<Vec<_>>();
        let durations = output
            .emitted
            .iter()
            .map(|event| event.resolved.duration)
            .collect::<Vec<_>>();
        assert!(output.suppressed);
        assert_eq!(notes, vec![-12.0, -1.0, 5.0, 1.0, 12.0]);
        assert_eq!(offsets, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(durations, vec![1.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn folder_midi_fx_spatial_harmonic_delay_emits_configured_taps_and_passes_source() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let descriptors = runtime.midi_fx_descriptors();
        let delay_idx = descriptors
            .iter()
            .position(|desc| desc.name == "spatial-harmonic-delay")
            .expect("spatial-harmonic-delay MIDI FX is registered");
        let delay_desc = descriptors
            .get(delay_idx)
            .expect("spatial-harmonic-delay descriptor");
        assert_eq!(delay_desc.params.len(), 27);
        assert_eq!(delay_desc.params[0].name, "rate");
        assert_eq!(delay_desc.params[1].name, "taps");
        assert_eq!(delay_desc.params[26].name, "enabled");

        let mut slot = EffectSlotSnapshot::new_default(delay_desc, 0);
        slot.defaults[1] = 2.0;
        slot.defaults[2] = 1.0;
        slot.defaults[3] = 0.0;
        slot.defaults[4] = 0.5;
        slot.defaults[5] = -0.5;
        slot.defaults[6] = 2.0;
        slot.defaults[7] = 12.0;
        slot.defaults[8] = 0.25;
        slot.defaults[9] = 0.5;

        let output = runtime
            .invoke_midi_fx(
                delay_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 4.0,
                    velocity: 0.8,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![AccumulatorNoteSpan {
                    transpose: 5.0,
                    start_beats: 0.125,
                    end_beats: 0.375,
                }]),
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(!output.suppressed);
        assert_eq!(output.resolved.transpose, 5.0);
        assert_eq!(output.resolved.velocity, 0.8);
        assert_eq!(output.emitted.len(), 2);

        let first = &output.emitted[0];
        assert_eq!(first.offset_beats, 0.375);
        assert_eq!(first.resolved.transpose, 5.0);
        assert!((first.resolved.velocity - 0.4).abs() < 1e-6);
        assert_eq!(first.resolved.pan, -0.5);
        assert_eq!(first.resolved.duration, 1.0);

        let second = &output.emitted[1];
        assert_eq!(second.offset_beats, 0.625);
        assert_eq!(second.resolved.transpose, 17.0);
        assert!((second.resolved.velocity - 0.2).abs() < 1e-6);
        assert_eq!(second.resolved.pan, 0.5);
        assert_eq!(second.resolved.duration, 1.0);
    }

    #[test]
    fn folder_midi_fx_beat_repeat_suppresses_source_and_emits_clock_window_note() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let descriptors = runtime.midi_fx_descriptors();
        let repeat_idx = descriptors
            .iter()
            .position(|desc| desc.name == "beat-repeat")
            .expect("beat-repeat MIDI FX is registered");
        let repeat_desc = descriptors.get(repeat_idx).expect("beat-repeat descriptor");
        assert_eq!(repeat_desc.params[0].name, "rate");
        assert_eq!(
            repeat_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            Some("clock-rate")
        );

        let mut slot = EffectSlotSnapshot::new_default(repeat_desc, 0);
        slot.defaults[0] = 4.0;
        slot.defaults[1] = 0.5;
        slot.defaults[2] = 0.75;

        let output = runtime
            .invoke_midi_fx(
                repeat_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 0.8,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.25,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![AccumulatorNoteSpan {
                    transpose: 5.0,
                    start_beats: 0.0,
                    end_beats: 0.25,
                }]),
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(output.suppressed);
        assert_eq!(output.emitted.len(), 1);
        let repeat = &output.emitted[0];
        assert_eq!(repeat.offset_beats, 0.0);
        assert_eq!(repeat.resolved.transpose, 5.0);
        assert!((repeat.resolved.velocity - 0.6).abs() < 1e-6);
        assert_eq!(repeat.resolved.pan, 0.25);
        assert_eq!(repeat.resolved.duration, 0.5);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_can_emit_joined_arp_events() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "arp-held"
                  (do
                    (fx-suppress)
                    (for-each |i|
                      (fx-arp-emit :16 i :vel 0.8)
                      (range 0 (fx-arp-count :16)))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                Some(vec![
                    AccumulatorNoteSpan {
                        transpose: 0.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 4.0,
                        start_beats: 0.0,
                        end_beats: 2.0,
                    },
                    AccumulatorNoteSpan {
                        transpose: 7.0,
                        start_beats: 1.0,
                        end_beats: 2.0,
                    },
                ]),
                EffectSlotSnapshot::new_empty(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert!(output.suppressed);
        assert_eq!(notes, vec![0.0, 4.0, 0.0, 4.0, 4.0, 7.0, 0.0, 4.0]);
        assert_eq!(output.emitted.len(), 8);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_arp_octave_expands_note_pool() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let arp_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "arp")
            .expect("arp descriptor");
        let mut slot = EffectSlotSnapshot::new_default(&arp_desc, 0);
        slot.defaults[2] = 2.0;

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 8.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0, 4.0, 7.0],
                vec![8.0, 8.0, 8.0],
                0.0,
                None,
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let notes = output
            .emitted
            .iter()
            .map(|event| event.resolved.transpose)
            .collect::<Vec<_>>();
        assert!(output.suppressed);
        assert_eq!(notes, vec![0.0, 4.0, 7.0, 12.0, 16.0, 19.0, 0.0, 4.0]);
    }

    #[test]
    fn folder_midi_fx_trigger_to_track_emits_to_selected_target_and_ignores_self() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        runtime.eval(&super::load_midi_fx_library_source()).unwrap();
        let descriptors = runtime.midi_fx_descriptors();
        let trigger_idx = descriptors
            .iter()
            .position(|desc| desc.name == "trigger-to-track")
            .expect("trigger-to-track MIDI FX is registered");
        let trigger_desc = descriptors
            .get(trigger_idx)
            .expect("trigger-to-track descriptor");
        assert_eq!(trigger_desc.params.len(), 2);
        assert_eq!(trigger_desc.params[0].name, "track");
        assert_eq!(trigger_desc.params[1].name, "enabled");

        let mut slot = EffectSlotSnapshot::new_default(trigger_desc, 0);
        slot.defaults[0] = 2.0;
        let output = runtime
            .invoke_midi_fx(
                trigger_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0],
                vec![1.0],
                0.0,
                None,
                slot.clone(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        assert!(!output.suppressed);
        assert_eq!(output.emitted.len(), 1);
        assert_eq!(output.emitted[0].track, Some(1));
        assert_eq!(output.emitted[0].offset_beats, 0.0);
        assert_eq!(output.emitted[0].resolved.transpose, 5.0);

        slot.defaults[0] = 1.0;
        let self_target_output = runtime
            .invoke_midi_fx(
                trigger_idx,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 5.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                vec![0.0],
                vec![1.0],
                0.0,
                None,
                slot,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        assert!(!self_target_output.suppressed);
        assert!(self_target_output.emitted.is_empty());
    }

    #[test]
    fn scratch_control_runtime_midi_fx_uses_beat_timing_helpers() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "timing"
                  (do
                    (fx-suppress)
                    (fx-emit :beats (fx-time :8t 1))
                    (fx-emit :beats (fx-source-time 2))))
                "#,
            )
            .unwrap();

        let output = runtime
            .invoke_midi_fx(
                0,
                0,
                0,
                0.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                0.0,
                None,
                EffectSlotSnapshot::new_empty(),
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        assert!(output.suppressed);
        assert!((output.emitted[0].offset_beats - (1.0 / 3.0)).abs() < 0.0001);
        assert_eq!(output.emitted[1].offset_beats, 0.5);
    }

    #[test]
    fn registered_sequencer_tick_emits_event() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(__register-sequencer "chord"
                     :resolution :1
                     :tick (lambda () (seq-emit :track 0 :at :now :vel 0.9 :chord (list 0 4 7))))"#,
            )
            .expect("register sequencer");

        let defs = runtime.sequencer_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "chord");
        assert!((defs[0].resolution_beats - 4.0).abs() < 1e-9); // :1 = whole = 4 beats

        let result = runtime
            .invoke_sequencer_tick(
                0,
                crate::generator::GeneratorTickInput {
                    id: defs[0].id,
                    generator_index: 0,
                    tick_index: 0,
                    beat: 0.0,
                    resolution_beats: defs[0].resolution_beats,
                    samples_per_quarter: 48_000.0,
                    random_state: 1,
                    state: Default::default(),
                },
            )
            .expect("tick");

        assert_eq!(result.emitted.len(), 1);
        let event = &result.emitted[0];
        assert_eq!(event.track, Some(0));
        assert_eq!(event.offset_beats, 0.0);
        assert!((event.resolved.velocity - 0.9).abs() < 1e-6);
        assert_eq!(event.chord, vec![0.0, 4.0, 7.0]);
    }

    #[test]
    fn chan_get_reads_channel_snapshot_inside_generator_tick() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(__register-sequencer "chan-reader"
                     :resolution :16
                     :tick (lambda () (seq-emit :track 0 :at :now :vel (chan-get "warp" 0.25))))"#,
            )
            .expect("register sequencer");
        let defs = runtime.sequencer_defs();
        let invoke = |runtime: &mut ScratchControlRuntime, tick_index| {
            runtime
                .invoke_sequencer_tick(
                    0,
                    crate::generator::GeneratorTickInput {
                        id: defs[0].id,
                        generator_index: 0,
                        tick_index,
                        beat: tick_index as f64 * 0.25,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: Default::default(),
                    },
                )
                .expect("tick")
        };

        // No snapshot published yet: the channel is unset, the default wins.
        assert!((invoke(&mut runtime, 0).emitted[0].resolved.velocity - 0.25).abs() < 1e-6);

        runtime.set_generator_channel_values(
            1,
            HashMap::from([("warp".to_string(), Value::Number(0.9))]),
        );
        assert!((invoke(&mut runtime, 1).emitted[0].resolved.velocity - 0.9).abs() < 1e-6);

        // An unset channel still falls through to the default.
        runtime.set_generator_channel_values(2, HashMap::new());
        assert!((invoke(&mut runtime, 2).emitted[0].resolved.velocity - 0.25).abs() < 1e-6);
    }

    #[test]
    fn chan_get_outside_generator_tick_errors() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let result = runtime.eval(r#"(chan-get "warp")"#).expect("eval");
        assert_eq!(result, Some(Value::Bool(false)));
        let status = runtime.take_status_message().unwrap_or_default();
        assert!(status.contains("outside a generator tick"), "{status}");
    }

    #[test]
    fn published_sequencer_tick_compiles_once_and_hot_reloads_without_stale_code() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let id = 42;
        let source = "(seq-emit :track 0 :at :now :vel 0.25)";
        runtime
            .register_published_sequencer(
                id,
                "published".to_string(),
                Timebase::Sixteenth,
                source.to_string(),
            )
            .expect("compile initial tick");
        assert_eq!(runtime.sequencer_tick_compile_count, 1);

        let invoke = |runtime: &mut ScratchControlRuntime, tick_index| {
            runtime
                .invoke_sequencer_tick(
                    0,
                    crate::generator::GeneratorTickInput {
                        id,
                        generator_index: 0,
                        tick_index,
                        beat: tick_index as f64 * 0.25,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: Default::default(),
                    },
                )
                .expect("invoke published tick")
        };
        assert!((invoke(&mut runtime, 0).emitted[0].resolved.velocity - 0.25).abs() < 1e-6);
        assert!((invoke(&mut runtime, 1).emitted[0].resolved.velocity - 0.25).abs() < 1e-6);
        assert_eq!(runtime.sequencer_tick_compile_count, 1);

        runtime
            .register_published_sequencer(
                id,
                "published".to_string(),
                Timebase::Sixteenth,
                "(seq-emit :track 0 :at :now :vel 0.75)".to_string(),
            )
            .expect("compile replacement tick");
        assert_eq!(runtime.sequencer_tick_compile_count, 2);
        assert!((invoke(&mut runtime, 2).emitted[0].resolved.velocity - 0.75).abs() < 1e-6);
        assert_eq!(runtime.sequencer_tick_compile_count, 2);

        let error = runtime
            .register_published_sequencer(
                id,
                "published".to_string(),
                Timebase::Sixteenth,
                "(seq-emit".to_string(),
            )
            .expect_err("malformed replacement must fail");
        assert!(error.contains("failed to compile sequencer tick 42"), "{error}");
        assert_eq!(runtime.sequencer_tick_compile_count, 3);
        assert!(runtime.sequencer_defs().is_empty(), "stale tick remained registered");

        // The exact failed source is cached too, so a caller cannot accidentally
        // move parsing malformed input into the steady-state scheduler path.
        assert!(runtime
            .register_published_sequencer(
                id,
                "published".to_string(),
                Timebase::Sixteenth,
                "(seq-emit".to_string(),
            )
            .is_err());
        assert_eq!(runtime.sequencer_tick_compile_count, 3);
    }

    #[test]
    fn def_sequencer_drives_generator_runtime_end_to_end() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(def-sequencer "chord"
                     :resolution :1
                     :tick (seq-emit :track 0 :at :now :vel 0.8 :chord (list 0 4 7)))"#,
            )
            .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&runtime.sequencer_defs(), 0.0);
        assert_eq!(generators.len(), 1);

        // Drive exactly as the scheduler does: tick the generator runtime, routing
        // each boundary through the scheduler-side VM's :tick closure.
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                runtime
                    .invoke_sequencer_tick(input.generator_index, input)
                    .expect("tick")
            },
            &mut out,
        );

        // :1 = whole note = 4 beats; one boundary at beat 4.0 within (0, 4].
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.track, Some(0));
        assert_eq!(out[0].event.chord, vec![0.0, 4.0, 7.0]);
        assert_eq!(out[0].sample_time, 192_000); // 4 beats * 48000 spq
    }

    // ── jaki: pure-Lisp evaluator core + pattern surface + generator wiring ──
    // (content/packages/alez.jaki, spec docs/jaki-sequencer-spec.md)

    fn jaki_runtime() -> ScratchControlRuntime {
        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        runtime
            .eval("(import alez.jaki.surface :refer (jak))")
            .expect("import Jaki package");
        runtime
    }

    fn jaki_authoring_runtime(state: Arc<SequencerState>) -> Runtime {
        let mut runtime = Runtime::new();
        let paths = crate::app_paths::app_paths();
        runtime.set_load_root(paths.factory_root());
        runtime.set_scoped_module_load_path(paths.module_load_roots().0);
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            ui_epoch,
        );
        runtime.register_native_with_docs_and_keywords(
            "def-sequencer",
            crate::lisp_host::DEF_SEQUENCER_SIGNATURE,
            crate::lisp_host::DEF_SEQUENCER_DOCS,
            crate::lisp_host::DEF_SEQUENCER_KEYWORDS.iter().copied(),
            move |args, _ctx| {
                let published = crate::lisp_host::published_sequencer_from_def_args(&args)?;
                let name = published.name.clone();
                state.publish_sequencer(published);
                Ok(Value::String(name))
            },
        );
        runtime
    }

    fn jaki_nums(value: &Value) -> Vec<f64> {
        let Value::List(items) = value else {
            panic!("expected a list of numbers, got {value:?}");
        };
        items
            .iter()
            .map(|cell| match &*cell.borrow() {
                Value::Number(n) => *n,
                other => panic!("expected number, got {other:?}"),
            })
            .collect()
    }

    fn jaki_eval_nums(runtime: &mut ScratchControlRuntime, code: &str) -> Vec<f64> {
        let source = format!("(import alez.jaki.core :as jaki)\n{code}");
        let value = runtime
            .eval(&source)
            .expect("eval jaki snippet")
            .expect("value");
        jaki_nums(&value)
    }

    fn assert_close(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-6, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn jaki_hand_derivation_alternates_dots_and_keeps_dash_hand() {
        let mut rt = jaki_runtime();
        // (. . - .) → hits L R L L R at unit offsets 0..4 (spec §6.1)
        let hands = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . - .) 0 :left jaki/default-state)))
                 (map (lambda (e) (if (= (get e :hand) :left) 0 1)) (get r :events)))"#,
        );
        assert_eq!(hands, vec![0.0, 1.0, 0.0, 0.0, 1.0]);
        let offs = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . - .) 0 :left jaki/default-state)))
                 (map (lambda (e) (/ (nth (get e :off) 0) (nth (get e :off) 1)))
                      (get r :events)))"#,
        );
        assert_eq!(offs, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn jaki_hands_derive_after_order_transforms() {
        let mut rt = jaki_runtime();
        // (rev) runs first, hands derive over the reversed events (spec §6.2):
        // (- . .) → dash L,L then R, L
        let hands = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . - (rev)) 0 :left jaki/default-state)))
                 (map (lambda (e) (if (= (get e :hand) :left) 0 1)) (get r :events)))"#,
        );
        assert_eq!(hands, vec![0.0, 0.0, 1.0, 0.0]);
        let vels = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . - (rev)) 0 :left jaki/default-state)))
                 (map (lambda (e) (get e :vel)) (get r :events)))"#,
        );
        // dash base, dash-decay, post-dash accent, then dot decay from the accent
        assert_close(&vels, &[0.8, 0.72, 0.92, 0.782]);
    }

    #[test]
    fn jaki_velocity_decays_and_threads_across_cycles() {
        let mut rt = jaki_runtime();
        let vels = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . . .)))
                 (let ((r0 (jaki/eval-at p 0 :left jaki/default-state)))
                   (let ((r1 (jaki/eval-at p 1 (get r0 :end-hand) (get r0 :end-st))))
                     (append (map (lambda (e) (get e :vel)) (get r0 :events))
                             (map (lambda (e) (get e :vel)) (get r1 :events))))))"#,
        );
        // dot streak decays straight through the cycle boundary, clamped at
        // min-vel 0.3 (spec §5: no per-step reset)
        assert_close(
            &vels,
            &[
                0.8,
                0.68,
                0.578,
                0.49130000000000007,
                0.417605,
                0.35496425,
                0.30171961249999996,
                0.3,
            ],
        );
    }

    #[test]
    fn jaki_every_swap_exchanges_hands_on_matching_cycles() {
        let mut rt = jaki_runtime();
        let hands = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . (every 2 swap))))
                 (append
                   (map (lambda (e) (if (= (get e :hand) :left) 0 1))
                        (get (jaki/eval-at p 0 :left jaki/default-state) :events))
                   (map (lambda (e) (if (= (get e :hand) :left) 0 1))
                        (get (jaki/eval-at p 1 :left jaki/default-state) :events))))"#,
        );
        // (every 2 ...) fires when (cycle+1) % 2 == 0 → cycle 1 swaps
        assert_eq!(hands, vec![0.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn jaki_fit_produces_exact_rational_offsets() {
        let mut rt = jaki_runtime();
        // (. . . -)(% 4): five hits scaled by 4/5 (spec §8.3) — offsets are
        // exact (num den) pairs, no float rounding
        let pairs = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . . - (% 4)) 0 :left jaki/default-state)))
                 (reduce (lambda (acc e)
                           (append acc (list (nth (get e :off) 0) (nth (get e :off) 1))))
                         (list) (get r :events)))"#,
        );
        assert_eq!(pairs, vec![0.0, 1.0, 4.0, 5.0, 8.0, 5.0, 12.0, 5.0, 16.0, 5.0]);
        let len = jaki_eval_nums(
            &mut rt,
            r#"(get (jaki/eval-at (jaki/pat . . . - (% 4)) 0 :left jaki/default-state) :len)"#,
        );
        assert_eq!(len, vec![4.0, 1.0]);
    }

    #[test]
    fn jaki_cyc_time_mod_alternates_per_cycle() {
        let mut rt = jaki_runtime();
        let offs = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . (* (cyc 1 2)))))
                 (append
                   (map (lambda (e) (/ (nth (get e :off) 0) (nth (get e :off) 1)))
                        (get (jaki/eval-at p 0 :left jaki/default-state) :events))
                   (map (lambda (e) (/ (nth (get e :off) 0) (nth (get e :off) 1)))
                        (get (jaki/eval-at p 1 :left jaki/default-state) :events))))"#,
        );
        // cycle 0: two hits on the grid; cycle 1: (* 2) doubles density in the
        // same two units
        assert_eq!(offs, vec![0.0, 1.0, 0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn jaki_hand_filter_keeps_offsets_and_extends_gates_legato() {
        let mut rt = jaki_runtime();
        let nums = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/filter (jaki/pat . . - .) '(:hand :left))
                                      0 :left jaki/default-state)))
                 (reduce (lambda (acc e)
                           (append acc (list (/ (nth (get e :off) 0) (nth (get e :off) 1))
                                             (/ (nth (get e :gate) 0) (nth (get e :gate) 1)))))
                         (list) (get r :events)))"#,
        );
        // left-hand events at 0, 2, 3; gates run legato to the next survivor,
        // the last to cycle end (spec §7)
        assert_eq!(nums, vec![0.0, 2.0, 2.0, 1.0, 3.0, 2.0]);
    }

    #[test]
    fn jaki_accent_filter_extends_to_next_unfiltered_event() {
        let mut rt = jaki_runtime();
        let nums = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/filter (jaki/pat - .) '(:accent true))
                                      0 :left jaki/default-state)))
                 (reduce (lambda (acc e)
                           (append acc (list (/ (nth (get e :off) 0) (nth (get e :off) 1))
                                             (/ (nth (get e :gate) 0) (nth (get e :gate) 1))
                                             (get e :vel))))
                         (list) (get r :events)))"#,
        );
        // only the post-dash dot is accented; its gate extends to cycle end
        assert_close(&nums, &[2.0, 1.0, 0.92]);
    }

    #[test]
    fn jaki_align_pad_threads_hand_and_velocity_through_padding() {
        let mut rt = jaki_runtime();
        let nums = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . . (align 4 :pad)) 0 :left jaki/default-state)))
                 (append
                   (map (lambda (e) (if (= (get e :hand) :left) 0 1)) (get r :events))
                   (map (lambda (e) (get e :vel)) (get r :events))
                   (list (/ (nth (get r :len) 0) (nth (get r :len) 1))
                         (if (= (get r :end-hand) :left) 0 1))))"#,
        );
        // three dots pad to four units; the pad dot continues the alternation
        // (R) and the dot-decay streak, and the ending hand threads past it
        assert_close(
            &nums,
            &[
                0.0, 1.0, 0.0, 1.0, // hands L R L R
                0.8, 0.68, 0.578, 0.49130000000000007, // pad continues decay
                4.0, // padded length
                0.0, // ending hand back to :left
            ],
        );
    }

    #[test]
    fn jaki_stac_and_ghost_transforms() {
        let mut rt = jaki_runtime();
        let gates = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat . . (stac)) 0 :left jaki/default-state)))
                 (reduce (lambda (acc e)
                           (append acc (list (nth (get e :gate) 0) (nth (get e :gate) 1))))
                         (list) (get r :events)))"#,
        );
        assert_eq!(gates, vec![1.0, 4.0, 1.0, 4.0]);
        let nums = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/pat - . (ghost)) 0 :left jaki/default-state)))
                 (reduce (lambda (acc e)
                           (append acc (list (/ (nth (get e :off) 0) (nth (get e :off) 1))
                                             (get e :vel))))
                         (list) (get r :events)))"#,
        );
        // ghost drops the dash's first hit; the pickup keeps dash-decay as its
        // velocity and the following dot still accents
        assert_close(&nums, &[1.0, 0.9, 2.0, 0.92]);
    }

    #[test]
    fn jaki_shift_rotates_offsets_within_the_cycle() {
        let mut rt = jaki_runtime();
        let vels = jaki_eval_nums(
            &mut rt,
            r#"(let ((r (jaki/eval-at (jaki/shift (jaki/pat . . . .) 1)
                                      0 :left jaki/default-state)))
                 (map (lambda (e) (get e :vel)) (get r :events)))"#,
        );
        // rotate right by one unit: the last (quietest) hit wraps to offset 0
        assert_close(&vels, &[0.49130000000000007, 0.8, 0.68, 0.578]);
    }

    #[test]
    fn jaki_cycle_index_closed_form_over_variable_length_super_cycle() {
        let mut rt = jaki_runtime();
        let cycles = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . . . (% (cyc 4 6)))))
                 (map (lambda (pos) (jaki/cycle-index p pos)) (list 0 3 4 9 10 23)))"#,
        );
        // lengths alternate 4, 6 (super-cycle 10): closed form, no state
        assert_eq!(cycles, vec![0.0, 0.0, 1.0, 1.0, 2.0, 4.0]);
        let starts = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . . . (% (cyc 4 6)))))
                 (map (lambda (pos) (nth (jaki/locate p pos) 1)) (list 0 3 4 9 10 23)))"#,
        );
        assert_eq!(starts, vec![0.0, 0.0, 4.0, 4.0, 10.0, 20.0]);
    }

    #[test]
    fn jaki_per_cycle_memo_reuses_results_and_stays_bounded() {
        let mut rt = jaki_runtime();
        let nums = jaki_eval_nums(
            &mut rt,
            r#"(let ((p (jaki/pat . . - .)))
                 (do
                   (map (lambda (k) (jaki/eval-cycle p k :left jaki/default-state))
                        (range 0 20))
                   (let ((a (jaki/eval-cycle p 3 :left jaki/default-state))
                         (b (jaki/eval-cycle p 3 :left jaki/default-state)))
                     (list (len jaki/memo-store)
                           (if (= a b) 1 0)
                           (len (get a :events))))))"#,
        );
        // the assoc memo caps at 16 entries and repeated lookups agree
        assert_eq!(nums, vec![16.0, 1.0, 5.0]);
    }

    #[test]
    fn jaki_payload_channel_rekeys_only_the_cycle_memo() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.core :as jaki)
               (def payload-pattern (jaki/pat - (dashdecay (chan "decay" 0.5))))
               (__register-sequencer "jaki-payload-channel"
                 :resolution :16
                 :tick (lambda ()
                   (do
                     (jaki/locate payload-pattern 0)
                     (let ((r (jaki/eval-cycle payload-pattern 0 :left jaki/default-state)))
                       (seq-emit
                         :track 0
                         :at :now
                         :vel (get (nth (get r :events) 1) :vel)
                         :note (len jaki/len-memo)
                         :speed (len jaki/lens-memo)
                         :pan (/ (len jaki/memo-store) 10))))))"#,
        )
        .expect("register payload-channel sequencer");
        let definition = rt.sequencer_defs().remove(0);
        let invoke = |runtime: &mut ScratchControlRuntime| {
            runtime
                .invoke_sequencer_tick(
                    0,
                    crate::generator::GeneratorTickInput {
                        id: definition.id,
                        generator_index: 0,
                        tick_index: 0,
                        beat: 0.0,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: Default::default(),
                    },
                )
                .expect("tick")
                .emitted
                .remove(0)
                .resolved
        };

        let before = invoke(&mut rt);
        assert!((before.velocity - 0.4).abs() < 1e-6);
        assert_eq!(before.transpose, 1.0);
        assert_eq!(before.speed, 1.0);
        assert!((before.pan - 0.1).abs() < 1e-6);

        rt.set_generator_channel_values(
            1,
            HashMap::from([("decay".to_string(), Value::Number(0.4))]),
        );
        let after = invoke(&mut rt);
        assert!((after.velocity - 0.32).abs() < 1e-6, "{after:?}");
        assert_eq!(after.transpose, 1.0, "len-memo must survive payload writes");
        assert_eq!(after.speed, 1.0, "lens-memo must survive payload writes");
        assert!((after.pan - 0.2).abs() < 1e-6, "eval-cycle must be rekeyed");
    }

    #[test]
    fn jaki_def_sequencer_free_runs_and_threads_velocity_across_cycles() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(def-sequencer "jaki-t1"
                 :resolution :16
                 :tick (do
                   (alez.jaki.core/init :16)
                   (alez.jaki.core/emit (alez.jaki.core/pat . . . .) 0)))"#,
        )
        .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            2.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        // 8 sixteenth boundaries = two 4-unit cycles; the dot-decay streak
        // crosses the cycle boundary via the generator state cells
        assert_eq!(out.len(), 8);
        let vels: Vec<f64> = out.iter().map(|e| e.event.resolved.velocity as f64).collect();
        assert_close(
            &vels,
            &[
                0.8,
                0.68,
                0.578,
                0.49130000000000007,
                0.417605,
                0.35496425,
                0.30171961249999996,
                0.3,
            ],
        );
        assert_eq!(out[0].sample_time, 12_000);
        assert!(out.iter().all(|e| e.event.track == Some(0)));
    }

    #[test]
    fn jaki_def_sequencer_emits_fractional_offsets_exactly_once() {
        let mut rt = jaki_runtime();
        // (. -)(* 2) → hits at 0, 1/2, 1, 3/2, 2, 5/2 units; the quoted
        // pattern also exercises the quote-preserving tick capture round-trip
        rt.eval(
            r#"(def-sequencer "jaki-t2"
                 :resolution :16
                 :tick (do
                   (alez.jaki.core/init :16)
                   (alez.jaki.core/emit (alez.jaki.core/pat . - (* 2)) 0)))"#,
        )
        .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        // each sub-unit hit is emitted exactly once from the tick window that
        // owns it, at its exact fractional beat offset (:16 unit = 0.25 beats)
        let mut samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        samples.sort_unstable();
        // the 3-unit cycle wraps mid-block: tick 3 owns cycle 1's [0,1) window
        // and emits both of its hits (0 and 1/2 units past beat 1.0)
        assert_eq!(
            samples,
            vec![12_000, 18_000, 24_000, 30_000, 36_000, 42_000, 48_000, 54_000]
        );
    }

    #[test]
    fn jaki_def_sequencer_fans_out_one_pattern_to_multiple_tracks() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(def-sequencer "jaki-t3"
                 :resolution :16
                 :tick (do
                   (alez.jaki.core/init :16)
                   (let ((base (alez.jaki.core/pat . . - .)))
                     (do
                       (alez.jaki.core/emit base 0)
                       (alez.jaki.core/emit (alez.jaki.core/shift base 1) 1)
                       (alez.jaki.core/emit (alez.jaki.core/filter base '(:hand :left)) 2)))))"#,
        )
        .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.25,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        // one 5-unit cycle: full pattern on 0 and 1, the left hand's three
        // hits on 2 with legato gates (2, 1, 2 units → 0.5, 0.25, 0.5 beats)
        let track = |t: usize| -> Vec<&crate::generator::GeneratorEmission> {
            out.iter().filter(|e| e.event.track == Some(t)).collect()
        };
        assert_eq!(track(0).len(), 5);
        assert_eq!(track(1).len(), 5);
        let left = track(2);
        assert_eq!(left.len(), 3);
        let durations: Vec<f64> = left
            .iter()
            .map(|e| e.event.resolved.duration as f64)
            .collect();
        assert_close(&durations, &[0.5, 0.25, 0.5]);
        let base_vels: Vec<f64> = track(0)
            .iter()
            .map(|e| e.event.resolved.velocity as f64)
            .collect();
        assert_close(&base_vels, &[0.8, 0.68, 0.8, 0.72, 0.92]);
    }

    #[test]
    fn jaki_surface_flat_route_grammar_fans_out() {
        // tier-2 surface: same routing as the plumbed fan-out test above, but
        // authored through the bare `jak` macro and the `->` route grammar
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "kit" :16
                 . . - .
                 -> 0
                 -> 1 (shift 1)
                 -> 2 left)"#,
        )
        .expect("jaki surface macro");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.25,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        let track = |t: usize| -> Vec<&crate::generator::GeneratorEmission> {
            out.iter().filter(|e| e.event.track == Some(t)).collect()
        };
        assert_eq!(track(0).len(), 5);
        assert_eq!(track(1).len(), 5);
        let left = track(2);
        assert_eq!(left.len(), 3);
        let durations: Vec<f64> = left
            .iter()
            .map(|e| e.event.resolved.duration as f64)
            .collect();
        assert_close(&durations, &[0.5, 0.25, 0.5]);
        let base_vels: Vec<f64> = track(0)
            .iter()
            .map(|e| e.event.resolved.velocity as f64)
            .collect();
        assert_close(&base_vels, &[0.8, 0.68, 0.8, 0.72, 0.92]);
    }

    #[test]
    fn jaki_surface_channel_widgets_rewrite_declare_and_reseed_idempotently() {
        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut rt = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        rt.eval("(import alez.jaki.surface :refer (jak))")
            .expect("import Jaki package");
        let source = r#"(import alez.jaki.surface :refer (jak))
                         (jak "hit" :16
                           - .
                           (dashdecay (~slider 0.294 :chan "hit.dashdecay"))
                           (dotdecay (~knob 0.177 :chan true)))"#;
        rt.eval(source).expect("channel widget surface");

        let authored = rt.process_authoring_snapshot();
        assert_eq!(authored.channels.len(), 2);
        assert_eq!(authored.channels[0].name.as_deref(), Some("hit.dashdecay"));
        assert_eq!(authored.channels[0].initial, Some(Value::Number(0.294)));
        assert_eq!(authored.channels[1].name.as_deref(), Some("hit#1"));
        assert_eq!(authored.channels[1].initial, Some(Value::Number(0.177)));
        assert!(authored.channels.iter().all(|channel| !channel.message_only));
        let handles: Vec<_> = authored.channels.iter().map(|channel| channel.handle_id).collect();
        assert!(state.take_process_channel_writes().is_empty());

        rt.eval(source).expect("unchanged re-evaluation");
        let unchanged = rt.process_authoring_snapshot();
        assert_eq!(unchanged.channels.len(), 2, "declarations are upserts by name");
        assert_eq!(
            unchanged
                .channels
                .iter()
                .map(|channel| channel.handle_id)
                .collect::<Vec<_>>(),
            handles,
            "re-evaluation must preserve channel handles"
        );
        assert!(
            state.take_process_channel_writes().is_empty(),
            "an unchanged literal must not stomp the runtime value"
        );

        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "hit" :16
                 - .
                 (dashdecay (~slider 0.5 :chan "hit.dashdecay"))
                 (dotdecay (~knob 0.177 :chan true)))"#,
        )
        .expect("edited channel seed");
        let edited = rt.process_authoring_snapshot();
        assert_eq!(edited.channels.len(), 2);
        assert_eq!(edited.channels[0].initial, Some(Value::Number(0.5)));
        assert_eq!(
            state.take_process_channel_writes(),
            vec![(
                "hit.dashdecay".to_string(),
                crate::process::ProcessLiteral::Number(0.5),
            )],
            "a text edit must enter the normal scheduler write queue"
        );

        // The scheduler source contains `(chan ...)`, not a widget fallback:
        // changing its channel snapshot changes the payload without re-eval.
        rt.set_generator_channel_values(
            1,
            HashMap::from([("hit.dashdecay".to_string(), Value::Number(0.4))]),
        );
        let definition = rt.sequencer_defs().remove(0);
        let emitted = rt
            .invoke_sequencer_tick(
                0,
                crate::generator::GeneratorTickInput {
                    id: definition.id,
                    generator_index: 0,
                    tick_index: 1,
                    beat: 0.25,
                    resolution_beats: 0.25,
                    samples_per_quarter: 48_000.0,
                    random_state: 1,
                    state: Default::default(),
                },
            )
            .expect("tick")
            .emitted;
        assert_eq!(emitted.len(), 1);
        assert!((emitted[0].resolved.velocity - 0.32).abs() < 1e-6);
    }

    #[test]
    fn jaki_surface_channel_declarations_reject_message_only_collisions_atomically() {
        let mut rt = jaki_runtime();
        rt.eval("(defchan ping)").expect("message-only channel");
        let before = rt.process_authoring_snapshot().channels;
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].name.as_deref(), Some("ping"));
        assert!(before[0].message_only);
        let result = rt
            .eval(
                r#"(import alez.jaki.surface :refer (jak))
                   (jak "bad" :16
                     -
                     (dotdecay (~slider 0.2 :chan true))
                     (dashdecay (~slider 0.3 :chan "ping")))"#,
            )
            .expect("native errors use the host status plus a false result");
        assert_eq!(result, Some(Value::Bool(false)));
        assert!(rt.sequencer_defs().is_empty(), "the invalid body must not publish");
        let channels = rt.process_authoring_snapshot().channels;
        assert_eq!(channels.len(), 1, "validation must precede all mutations");
        assert_eq!(channels[0].name.as_deref(), Some("ping"));
        assert!(channels[0].message_only);
    }

    #[test]
    fn jaki_surface_channel_widgets_bind_named_and_anonymous_handles_by_source() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = jaki_authoring_runtime(Arc::clone(&state));
        let init_source = r#"
            (def eval-buffer-command () (eval-current-buffer))
            (bind-key "C-x C-b" "eval-buffer-command")
        "#
        .to_string();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init_source),
                ..EditorConfig::default()
            },
        );
        let source_path = std::env::temp_dir().join(format!(
            "eseq-jaki-channel-bindings-{}.lisp",
            std::process::id()
        ));
        std::fs::write(
            &source_path,
            r#"(import alez.jaki.surface :refer (jak))
               (jak "live" :16
                 . -
                 (dotdecay (~slider 0.3 :min 0 :max 1 :chan "live.decay"))
                 (dashdecay (~knob 0.4 :min 0 :max 1 :chan true)))"#,
        )
        .expect("write Jaki source fixture");
        editor
            .open_file_buffer_with_mode(&source_path, BufferMode::ESeqLisp)
            .expect("open Jaki source fixture");
        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        editor.refresh_runtime_side_effects();
        let _ = std::fs::remove_file(&source_path);

        let bindings = editor.active_buffer().inline_widget_runtime_bindings();
        assert_eq!(
            bindings.len(),
            2,
            "widgets={} buffer={} minibuffer={:?}",
            editor.active_buffer().inline_code_widgets().len(),
            editor.active_buffer().name,
            editor.minibuffer,
        );
        for (_, target, inlet) in bindings {
            assert_eq!(inlet, "set");
            editor
                .runtime_mut()
                .invoke(
                    target,
                    vec![Value::Keyword(inlet), Value::Number(0.75)],
                )
                .expect("write through inline channel target");
        }
        assert_eq!(
            state.take_process_channel_writes(),
            vec![
                (
                    "live.decay".to_string(),
                    crate::process::ProcessLiteral::Number(0.75),
                ),
                (
                    "live#1".to_string(),
                    crate::process::ProcessLiteral::Number(0.75),
                ),
            ]
        );

        state.publish_process_channel_values(HashMap::from([(
            "live.decay".to_string(),
            crate::process::ProcessLiteral::Number(0.9),
        )]));
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 120, 30);
        let live_values = editor
            .active_buffer()
            .inline_code_widgets()
            .iter()
            .filter_map(|inline| match &inline.widget {
                Value::Map(map) => map.get("value").map(|value| value.borrow().clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            live_values.contains(&Value::Number(0.9)),
            "a render frame must poll the scheduler mirror into the slider: {live_values:?}"
        );

        // The sequencer UI commonly leaves the source visible in an inactive
        // tile. Runtime polling must update that tile too; waiting until a
        // click activates it is the visible "stuck, then snap" regression.
        let source_buffer_name = editor.active_buffer().name.clone();
        editor.open_scratch_buffer("*other*", "");
        let other_buffer_idx = editor.active_buffer_idx();
        editor
            .runtime_mut()
            .eval_str(&format!("(switch-to-buffer {source_buffer_name:?})"))
            .expect("request source buffer switch");
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.active_buffer().name, source_buffer_name);
        let other_tile = editor
            .split_active_tile(eseqlisp::tile::SplitDir::Vertical, other_buffer_idx)
            .expect("split source and other buffer");
        editor.switch_active_tile(other_tile);
        state.publish_process_channel_values(HashMap::from([(
            "live.decay".to_string(),
            crate::process::ProcessLiteral::Number(0.6),
        )]));
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 120, 30);
        assert!(editor.switch_active_tile_to_buffer_named(&source_buffer_name));
        let inactive_live_values = editor
            .active_buffer()
            .inline_code_widgets()
            .iter()
            .filter_map(|inline| match &inline.widget {
                Value::Map(map) => map.get("value").map(|value| value.borrow().clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            inactive_live_values.contains(&Value::Number(0.6)),
            "a visible inactive source tile must poll the scheduler mirror: {inactive_live_values:?}"
        );
    }

    #[test]
    fn shipped_scene_references_resolve_from_the_selected_scheduler_snapshot() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut authoring = Runtime::new();
        super::register_scene_slot_natives(&mut authoring, Arc::clone(&state));
        let publish_state = Arc::clone(&state);
        authoring.register_native("def-sequencer", move |args, _ctx| {
            let published = super::published_sequencer_from_def_args(&args)?;
            let name = published.name.clone();
            publish_state.publish_sequencer(published);
            Ok(Value::String(name))
        });
        authoring
            .eval_str(
                r#"(defscene figures 0.25)
                   (def-sequencer "scene-reader" :resolution :16
                     :tick (seq-emit :track 0 :vel figures))
                   (def-sequencer "shadow-reader" :resolution :16
                     :tick ((lambda (figures)
                              (seq-emit :track 0 :vel figures))
                            0.7))
                   (set! figures 0.8)"#,
            )
            .expect("publish scene-driven sequencers");

        let published = state.published_sequencers();
        let scene_reader = published
            .iter()
            .find(|definition| definition.name == "scene-reader")
            .expect("scene reader publication");
        assert!(
            scene_reader
                .tick_source
                .contains("(__defscene-resolve \"figures\")"),
            "a free scene reference must ship by canonical name: {}",
            scene_reader.tick_source
        );
        let shadow_reader = published
            .iter()
            .find(|definition| definition.name == "shadow-reader")
            .expect("shadow reader publication");
        assert!(
            !shadow_reader.tick_source.contains("__defscene-resolve"),
            "a lambda parameter must shadow the scene declaration: {}",
            shadow_reader.tick_source
        );

        let mut scheduler = ScratchControlRuntime::new_scheduler(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scheduler
            .eval("(defscene figures 0.25)")
            .expect("register the declaration default in the scheduler VM");
        for definition in &published {
            scheduler
                .register_published_sequencer(
                    definition.id,
                    definition.name.clone(),
                    Timebase::from_index(definition.resolution as u32),
                    definition.tick_source.clone(),
                )
                .expect("compile published sequencer");
        }
        let invoke_velocity = |scheduler: &mut ScratchControlRuntime, name: &str| {
            let definitions = scheduler.sequencer_defs();
            let index = definitions
                .iter()
                .position(|definition| definition.name == name)
                .expect("registered sequencer");
            let result = scheduler
                .invoke_sequencer_tick(
                    index,
                    crate::generator::GeneratorTickInput {
                        id: definitions[index].id,
                        generator_index: index,
                        tick_index: 0,
                        beat: 0.0,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: HashMap::new(),
                    },
                )
                .expect("invoke sequencer tick");
            result.emitted[0].resolved.velocity
        };

        assert!((invoke_velocity(&mut scheduler, "scene-reader") - 0.8).abs() < 1e-6);
        assert!((invoke_velocity(&mut scheduler, "shadow-reader") - 0.7).abs() < 1e-6);

        authoring
            .eval_str("(set! figures 0.4)")
            .expect("publish a newer scene-slot snapshot");
        assert!(
            (invoke_velocity(&mut scheduler, "scene-reader") - 0.8).abs() < 1e-6,
            "a callback must retain its selected boundary snapshot"
        );
        scheduler.set_scene_slot_snapshot(state.latest_scheduler_snapshot().scene_slots.clone());
        assert!(
            (invoke_velocity(&mut scheduler, "scene-reader") - 0.4).abs() < 1e-6,
            "the next boundary must observe the newly published snapshot"
        );
    }

    #[test]
    fn shipped_scene_writes_ship_as_by_name_sets() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut authoring = Runtime::new();
        super::register_scene_slot_natives(&mut authoring, Arc::clone(&state));
        let publish_state = Arc::clone(&state);
        authoring.register_native("def-sequencer", move |args, _ctx| {
            let published = super::published_sequencer_from_def_args(&args)?;
            let name = published.name.clone();
            publish_state.publish_sequencer(published);
            Ok(Value::String(name))
        });
        authoring
            .eval_str(
                r#"(defscene counter 0.25)
                   (def-sequencer "counting-reader" :resolution :16
                     :tick (do (set! counter (+ counter 0.25))
                               (seq-emit :track 0 :vel counter)))"#,
            )
            .expect("publish a scene-writing sequencer");

        let published = state.published_sequencers();
        let writer = published
            .iter()
            .find(|definition| definition.name == "counting-reader")
            .expect("writer publication");
        assert!(
            writer
                .tick_source
                .contains("(__defscene-set \"counter\" (+ (__defscene-resolve \"counter\") 0.25))"),
            "a scene write must ship as a by-name set: {}",
            writer.tick_source
        );

        let mut scheduler = ScratchControlRuntime::new_scheduler(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scheduler
            .eval("(defscene counter 0.25)")
            .expect("register the declaration default in the scheduler VM");
        scheduler
            .register_published_sequencer(
                writer.id,
                writer.name.clone(),
                Timebase::from_index(writer.resolution as u32),
                writer.tick_source.clone(),
            )
            .expect("compile the published writer");

        let invoke = |scheduler: &mut ScratchControlRuntime| {
            let definitions = scheduler.sequencer_defs();
            let index = definitions
                .iter()
                .position(|definition| definition.name == "counting-reader")
                .expect("registered sequencer");
            scheduler
                .invoke_sequencer_tick(
                    index,
                    crate::generator::GeneratorTickInput {
                        id: definitions[index].id,
                        generator_index: index,
                        tick_index: 0,
                        beat: 0.0,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: HashMap::new(),
                    },
                )
                .expect("invoke sequencer tick")
                .emitted[0]
                .resolved
                .velocity
        };

        // The write lands in the live slot bank; the read still observes the
        // boundary snapshot until the next boundary republishes it.
        assert!((invoke(&mut scheduler) - 0.25).abs() < 1e-6);
        scheduler.set_scene_slot_snapshot(state.latest_scheduler_snapshot().scene_slots.clone());
        assert!((invoke(&mut scheduler) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn jaki_surface_regular_authoring_runtime_publishes_to_scheduler() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut authoring = jaki_authoring_runtime(Arc::clone(&state));

        authoring
            .eval_str(
                r#"(import alez.jaki.surface :refer (jak))
                   (jak "hit" :16
                     . . -
                     -> 0 left)"#,
            )
            .expect("evaluate Jaki in the editor authoring runtime");
        let published = state.published_sequencers();
        assert_eq!(published.len(), 1, "jak must publish, not register editor-locally");
        assert!(!published[0].tick_source.is_empty());

        let mut scheduler = ScratchControlRuntime::new_scheduler(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scheduler
            .eval("(import alez.jaki.surface :refer (jak))")
            .expect("load Jaki on the scheduler VM");
        scheduler
            .register_published_sequencer(
                published[0].id,
                published[0].name.clone(),
                Timebase::from_index(published[0].resolution as u32),
                published[0].tick_source.clone(),
            )
            .expect("compile the published Jaki tick on the scheduler VM");
        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&scheduler.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            |input| scheduler.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );
        assert!(!out.is_empty(), "the published Jaki definition must trigger");
        assert!(out.iter().all(|event| event.event.track == Some(0)));
    }

    // ── sig: signal-pipeline sugar over def-process + channels ──
    // (content/packages/alez.sig, sibling of alez.jaki)

    #[test]
    fn sig_surface_registers_process_and_channel() {
        let mut rt = jaki_runtime();
        let handle = rt
            .eval(
                r#"(import alez.sig.surface :refer (sig))
                   (sig "hello" :over (bars 4) :rate :16
                     (-> phase (* tau) sin (scale -1 1 0 1)))"#,
            )
            .expect("sig surface macro")
            .expect("sig must return the started process handle");
        assert!(
            matches!(handle, Value::HostHandle { ref kind, .. } if kind == "process"),
            "expected a process handle, got {handle:?}"
        );
        rt.eval("(channel-handle \"hello\")")
            .expect("sig must declare its value channel");
    }

    #[test]
    fn sig_and_jaki_macros_expand_to_captured_forms_without_source_string_paths() {
        let mut rt = jaki_runtime();
        let jaki = rt
            .eval(
                r#"(source (macroexpand
                     '(alez.jaki.surface/jak "kit" :16 . - -> 0)))"#,
            )
            .expect("expand jak")
            .expect("jak expansion");
        let Value::String(jaki) = jaki else {
            panic!("expected jak source, got {jaki:?}");
        };
        assert!(jaki.contains("def-sequencer"), "{jaki}");
        assert!(jaki.contains(":tick "), "{jaki}");
        assert!(!jaki.contains(":tick-source"), "{jaki}");
        assert!(!jaki.contains("channel-register"), "{jaki}");

        let sig = rt
            .eval(
                r#"(import alez.sig.surface :refer (sig))
                   (source (macroexpand
                     '(alez.sig.surface/sig "ramp" :over (beats 4) :rate :16 phase)))"#,
            )
            .expect("expand sig")
            .expect("sig expansion");
        let Value::String(sig) = sig else {
            panic!("expected sig source, got {sig:?}");
        };
        assert!(sig.contains("def-process"), "{sig}");
        assert!(!sig.contains("sig-register"), "{sig}");
        assert!(!sig.contains("(eval "), "{sig}");

        let pat = rt
            .eval("(source (macroexpand '(alez.jaki.core/pat . -)))")
            .expect("expand pat")
            .expect("pat expansion");
        let Value::String(pat) = pat else {
            panic!("expected pat source, got {pat:?}");
        };
        assert!(pat.contains("alez.jaki.core/from-list"), "{pat}");
        assert!(!pat.contains("alez.jaki.core/pat"), "{pat}");
    }

    #[test]
    fn jaki_builder_uses_one_scene_driven_publication_and_plain_slot_edits() {
        fn find_by_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children.iter().find_map(|child| find_by_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
            assert!(node.rect.row >= 0.0 && node.rect.row < 18.0, "{:?}", node.rect);
            assert!(node.rect.col >= 0.0 && node.rect.col < 34.0, "{:?}", node.rect);
        }

        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.replace_pattern_repository(
            vec![
                crate::sequencer::PatternSnapshot::new_default(1, &[]),
                crate::sequencer::PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let mut runtime = jaki_authoring_runtime(Arc::clone(&state));
        super::register_scene_slot_authoring_natives(&mut runtime, Arc::clone(&state));
        runtime.register_reactive(
            "SEQ",
            vec![("current-pattern", Value::Number(0.0))],
            true,
        );
        runtime
            .eval_str(
                "(def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab \
                   (label buffer sequencer source-path) nil)",
            )
            .expect("install script-tab test stub");
        let source = std::fs::read_to_string(
            crate::app_paths::app_paths()
                .scripts_dir()
                .join("sequencers/jaki-builder-demo.lisp"),
        )
        .expect("read Jaki builder script");
        runtime.eval_str(&source).expect("evaluate Jaki builder script");

        let published = state.published_sequencers();
        assert_eq!(published.len(), 1, "the builder publishes exactly once at load");
        assert!(
            published[0]
                .tick_source
                .contains("(alez.jaki.core/run (__defscene-resolve \"jb-figures\"))"),
            "the tick must late-resolve body data by scene: {}",
            published[0].tick_source,
        );

        let tree = runtime
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("builder should publish its panel");
        let layout = runtime
            .layout_snapshot_for_tree_with_viewport(&tree, Some((34.0, 18.0)))
            .expect("builder panel should lay out");
        for key in [
            "jaki-builder-shape-0",
            "jaki-builder-mod-0",
            "jaki-builder-add",
            "jaki-builder-bake",
            "jaki-builder-code",
        ] {
            assert_measured(find_by_key(&layout, key).unwrap_or_else(|| panic!("missing {key}")));
        }

        let mod_change = find_by_key(&layout, "jaki-builder-mod-0")
            .and_then(|node| node.props.get("on-change"))
            .cloned()
            .expect("modifier edit callback");
        runtime
            .invoke(mod_change, vec![Value::String("stac".to_string())])
            .expect("edit first scene through plain set!");
        assert_eq!(state.published_sequencers().len(), 1, "editing must not republish");
        assert!(
            runtime
                .eval_str("(source jb-figures)")
                .expect("read first scene body")
                .is_some_and(|value| matches!(value, Value::String(source) if source.contains("(stac)"))),
            "the first scene must retain its modifier",
        );
        let first_scheduler_slots = state.latest_scheduler_snapshot().scene_slots.clone();

        let snapshots = state.export_pattern_repository();
        state.replace_pattern_repository(snapshots, 1);
        runtime
            .eval_str("(jb-set-shape 0 \"- . . .\")")
            .expect("edit second scene shape");
        assert!(
            runtime
                .eval_str("(source jb-figures)")
                .expect("read second scene body")
                .is_some_and(|value| matches!(value, Value::String(source) if source.contains("(- . . .)"))),
            "the second scene must resolve its own body",
        );
        let second_scheduler_slots = state.latest_scheduler_snapshot().scene_slots.clone();
        let snapshots = state.export_pattern_repository();
        state.replace_pattern_repository(snapshots, 0);
        assert!(
            runtime
                .eval_str("(source jb-figures)")
                .expect("return to first scene body")
                .is_some_and(|value| matches!(value, Value::String(source) if source.contains("(stac)"))),
            "switching back must recover the first scene without republishing",
        );

        let mut scheduler = ScratchControlRuntime::new_scheduler(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scheduler
            .eval("(import alez.jaki.surface) (defscene jb-figures '())")
            .expect("load Jaki and declare the scheduler slot");
        scheduler
            .register_published_sequencer(
                published[0].id,
                published[0].name.clone(),
                Timebase::from_index(published[0].resolution as u32),
                published[0].tick_source.clone(),
            )
            .expect("compile the one published builder tick");
        let invoke_duration = |scheduler: &mut ScratchControlRuntime| {
            scheduler
                .invoke_sequencer_tick(
                    0,
                    crate::generator::GeneratorTickInput {
                        id: published[0].id,
                        generator_index: 0,
                        tick_index: 0,
                        beat: 0.0,
                        resolution_beats: 0.25,
                        samples_per_quarter: 48_000.0,
                        random_state: 1,
                        state: HashMap::new(),
                    },
                )
                .expect("run scene-driven Jaki tick")
                .emitted[0]
                .resolved
                .duration
        };
        scheduler.set_scene_slot_snapshot(first_scheduler_slots);
        let first_duration = invoke_duration(&mut scheduler);
        scheduler.set_scene_slot_snapshot(second_scheduler_slots);
        let second_duration = invoke_duration(&mut scheduler);
        assert!((first_duration - 0.0625).abs() < 1e-6, "stac scene: {first_duration}");
        assert!((second_duration - 0.2).abs() < 1e-6, "plain scene: {second_duration}");

        runtime.eval_str("(jb-bake)").expect("bake current body");
        let baked = runtime
            .eval_str("jb-baked-code")
            .expect("read baked source")
            .expect("baked source value");
        assert!(
            matches!(&baked, Value::String(source) if source.starts_with("(jak \"jaki-builder\" :16") && source.contains("(stac)")),
            "bake must export an authorable (jak ...) form, got {baked:?}",
        );

        let mut editor = eseqlisp::Editor::new(runtime, eseqlisp::EditorConfig::default());
        assert!(
            editor.drain_host_commands().iter().any(|command| {
                matches!(command, eseqlisp::HostCommand::Custom { name, .. }
                    if name == "scene-slot-history-write")
            }),
            "the panel's set! must enter the scene-slot undo path",
        );
    }

    #[test]
    fn sig_surface_derives_transport_phase_per_tick() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"(import alez.sig.surface :refer (sig))
                   (sig "ramp" :over (beats 4) :rate (beats 1) :from 0.5)"#,
            )
            .expect("sig surface macro");

        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(scratch.process_authoring_snapshot(), 0.0);

        let mut ticks = 0usize;
        for beat in 0..6u64 {
            let invocations =
                processes.process_block(beat as f64, beat as f64 + 1.0, beat * 48_000, 48_000.0);
            for invocation in invocations {
                let tick_beat = invocation.beat;
                let result = scratch
                    .invoke_process_run(invocation)
                    .expect("sig process tick");
                let sent = result
                    .outputs
                    .iter()
                    .find(|output| output.name == "__chan:ramp")
                    .expect("sig tick must send its channel");
                let Value::Number(value) = sent.value else {
                    panic!("expected a numeric channel value, got {:?}", sent.value);
                };
                let expected = (tick_beat / 4.0 + 0.5).fract();
                assert!(
                    (value - expected).abs() < 1e-9,
                    "beat {tick_beat}: sent {value}, expected derived phase {expected}"
                );
                processes.apply_run_result(result);
                ticks += 1;
            }
        }
        assert!(ticks >= 4, "expected at least four sig ticks, got {ticks}");
    }

    #[test]
    fn jaki_surface_no_routes_defaults_to_track_zero() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "kick" :16 . . . .)"#,
        )
        .expect("jaki surface macro");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            2.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        assert_eq!(out.len(), 8);
        assert!(out.iter().all(|e| e.event.track == Some(0)));
        let vels: Vec<f64> = out.iter().map(|e| e.event.resolved.velocity as f64).collect();
        assert_close(
            &vels,
            &[
                0.8,
                0.68,
                0.578,
                0.49130000000000007,
                0.417605,
                0.35496425,
                0.30171961249999996,
                0.3,
            ],
        );
    }

    #[test]
    fn jaki_surface_fast_route_word_retimes_one_route_conditionally() {
        let mut rt = jaki_runtime();
        // track 0 plays the pattern straight; track 1 doubles up every other
        // cycle via the (fast (cyc …)) route word. Per-pattern threading state
        // keeps the two routes from fighting over the shared cycle counter.
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "snare" :16
                 . . - .
                 -> 0
                 -> 1 (fast (cyc 1 2)))"#,
        )
        .expect("jaki surface macro");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        // two 5-unit cycles of the base pattern = 2.5 beats
        generators.process_block(
            0.0,
            2.5,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        let count = |t: usize| out.iter().filter(|e| e.event.track == Some(t)).count();
        assert_eq!(count(0), 10);
        // fast route: cycle 0 matches the straight route, cycle 1 is denser
        let cycle_boundary = 66_000; // cycle 0 spans samples 12k..=60k (first boundary at 0.25 beats)
        let t1_times: Vec<u64> = out
            .iter()
            .filter(|e| e.event.track == Some(1))
            .map(|e| e.sample_time)
            .collect();
        let t1_first = t1_times.iter().filter(|t| **t < cycle_boundary).count();
        assert_eq!(t1_first, 5, "track1 sample times: {t1_times:?}");
        assert!(count(1) > count(0), "fast cycle should add events");
    }

    #[test]
    fn jaki_surface_vel_route_word_scales_velocity() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "soft" :16 . . . . -> 0 (vel 0.5))"#,
        )
        .expect("jaki surface macro");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        // :vel-scale applies at emission, on top of the threaded velocities
        let vels: Vec<f64> = out.iter().map(|e| e.event.resolved.velocity as f64).collect();
        assert_close(&vels, &[0.4, 0.34, 0.289, 0.24565000000000003]);
    }

    #[test]
    fn jaki_route_stac_applies_in_word_order_with_filters() {
        // Route-word stac is a post op so it composes with the gate-extending
        // hand/accent filters in authored order: `left stac` caps the extended
        // gates, `stac left` lets the filter's legato win.
        let cases: [(&str, &str, &[f32]); 4] = [
            ("plain", r#"(jak "a" :16 . . - .)"#, &[0.2, 0.2, 0.2, 0.2, 0.2]),
            ("stac", r#"(jak "b" :16 . . - . -> 0 stac)"#, &[0.0625; 5]),
            (
                "left-stac",
                r#"(jak "c" :16 . . - . -> 0 left stac)"#,
                &[0.0625; 3],
            ),
            (
                "stac-left",
                r#"(jak "d" :16 . . - . -> 0 stac left)"#,
                &[0.5, 0.25, 0.5],
            ),
        ];
        for (label, src, expected) in cases {
            let mut rt = jaki_runtime();
            rt.eval(&format!(
                "(import alez.jaki.surface :refer (jak))\n{src}"
            ))
            .expect("jaki surface macro");
            let mut generators = crate::generator::GeneratorRuntime::default();
            generators.sync_definitions(&rt.sequencer_defs(), 0.0);
            let mut out = Vec::new();
            generators.process_block(
                0.0,
                1.25,
                0,
                48_000.0,
                |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
                &mut out,
            );
            let durs: Vec<f32> = out.iter().map(|e| e.event.resolved.duration).collect();
            assert_eq!(durs, expected, "{label}");
        }
    }

    #[test]
    fn jaki_route_every_wraps_post_words_cycle_gated() {
        // (every n w) with a post-op word (stac/shift/filters) lowers to a
        // cycle-gated post op in authored order. Pattern `. . - .` = 5 units
        // = 1.25 beats at :16; run two cycles.
        let run = |src: &str| {
            let mut rt = jaki_runtime();
            rt.eval(&format!(
                "(import alez.jaki.surface :refer (jak))\n{src}"
            ))
            .expect("jaki surface macro");
            let mut generators = crate::generator::GeneratorRuntime::default();
            generators.sync_definitions(&rt.sequencer_defs(), 0.0);
            let mut out = Vec::new();
            generators.process_block(
                0.0,
                2.5,
                0,
                48_000.0,
                |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
                &mut out,
            );
            out
        };

        // stac on every 2nd cycle: cycle 0 normal 80% gates, cycle 1 capped.
        let out = run(r#"(jak "a" :16 . . - . -> 0 (every 2 stac))"#);
        let durs: Vec<f32> = out.iter().map(|e| e.event.resolved.duration).collect();
        assert_eq!(durs, [[0.2; 5], [0.0625; 5]].concat(), "every-stac");

        // stac survives a hand filter when written after it, still cycle-gated.
        let out = run(r#"(jak "b" :16 . . - . -> 0 left (every 2 stac))"#);
        let durs: Vec<f32> = out.iter().map(|e| e.event.resolved.duration).collect();
        assert_eq!(
            durs,
            vec![0.5, 0.25, 0.5, 0.0625, 0.0625, 0.0625],
            "left-every-stac"
        );

        // (every 2 (shift 1)) was a silent no-op through the xf path; as a
        // post op it rotates cycle 1's left-hand events by one unit. Left-hand
        // events sit at units [0,2,3] per cycle; the block clock's first
        // boundary is beat 0.25 (sample 12000), so cycle 0 (unshifted) lands
        // at [12000,36000,48000] and cycle 1 (shifted +1 unit) at
        // [84000,108000,120000] instead of [72000,96000,108000].
        let out = run(r#"(jak "c" :16 . . - . -> 0 left (every 2 (shift 1)))"#);
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(
            samples,
            vec![12_000, 36_000, 48_000, 84_000, 108_000, 120_000],
            "left-every-shift"
        );
    }

    #[test]
    fn jaki_route_destination_and_note_take_cyc_args() {
        // `-> (cyc 0 1)` bounces the route between tracks per pattern cycle;
        // `(note (cyc 0 4 12))` cycles the emitted transpose. Pattern `. . . .`
        // = 4 units = 1 beat per cycle; run three cycles.
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "cycdest" :16 . . . . -> (cyc 0 1) (note (cyc 0 4 12)))"#,
        )
        .expect("jaki surface macro");
        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            3.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );
        let tracks: Vec<usize> = out.iter().map(|e| e.event.track.unwrap()).collect();
        assert_eq!(tracks, [[0; 4], [1; 4], [0; 4]].concat(), "cyc destination");
        let notes: Vec<f32> = out.iter().map(|e| e.event.resolved.transpose).collect();
        assert_eq!(notes, [[0.0; 4], [4.0; 4], [12.0; 4]].concat(), "cyc note");
    }

    #[test]
    fn jaki_route_quant_snaps_offsets_to_straight_and_triplet_grids() {
        let run = |src: &str| {
            let mut rt = jaki_runtime();
            rt.eval(&format!(
                "(import alez.jaki.surface :refer (jak))\n{src}"
            ))
            .expect("jaki surface macro");
            let mut generators = crate::generator::GeneratorRuntime::default();
            generators.sync_definitions(&rt.sequencer_defs(), 0.0);
            let mut out = Vec::new();
            generators.process_block(
                0.0,
                1.0,
                0,
                48_000.0,
                |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
                &mut out,
            );
            out
        };

        // A 3-into-4 tuplet (offsets 0, 4/3, 8/3 units): the quantized arm
        // snaps to units [0,1,3], the raw arm keeps the tuplet placement.
        // Boundary k = sample 12000*(k+1); fractional offsets add within the
        // tick window (1/3 unit = 4000 samples).
        let out = run(r#"(jak "qs" :16 (fig (. . .) (% 4)) -> 0 (quant :16) -> 1)"#);
        let track = |t: usize| -> Vec<u64> {
            out.iter()
                .filter(|e| e.event.track == Some(t))
                .map(|e| e.sample_time)
                .collect()
        };
        assert_eq!(track(0), vec![12_000, 24_000, 48_000], "tuplet -> straight");
        assert_eq!(track(1), vec![12_000, 28_000, 44_000], "raw tuplet arm");

        // Straight 16ths pushed onto a triplet grid (q = 2/3 unit):
        // units [0,1,2,3] snap to [0, 4/3, 2, 10/3].
        let out = run(r#"(jak "qt" :16 . . . . -> 0 (quant :16t))"#);
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(
            samples,
            vec![12_000, 28_000, 36_000, 52_000],
            "straight -> triplet"
        );
    }

    #[test]
    fn jaki_surface_multi_voice_lines_each_carry_their_own_routes() {
        let mut rt = jaki_runtime();
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "kit" :16
                 (. . - . -> 0)
                 (. . - . -> 1 (shift 1) -> 2 left))"#,
        )
        .expect("jaki surface macro");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        generators.process_block(
            0.0,
            1.25,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );

        let track = |t: usize| -> Vec<&crate::generator::GeneratorEmission> {
            out.iter().filter(|e| e.event.track == Some(t)).collect()
        };
        assert_eq!(track(0).len(), 5);
        assert_eq!(track(1).len(), 5);
        let left = track(2);
        assert_eq!(left.len(), 3);
        let durations: Vec<f64> = left
            .iter()
            .map(|e| e.event.resolved.duration as f64)
            .collect();
        assert_close(&durations, &[0.5, 0.25, 0.5]);
    }

    #[test]
    #[ignore = "timing probe, run manually"]
    fn jaki_perf_probe() {
        let mut rt = jaki_runtime();
        // production-shaped runtime: full midi-fx + process libraries resident
        let midi_fx = super::load_midi_fx_library_source();
        if !midi_fx.trim().is_empty() {
            rt.eval(&midi_fx).expect("midi-fx library");
        }
        let processes = super::load_process_library_source();
        if !processes.trim().is_empty() {
            rt.eval(&processes).expect("process library");
        }
        // real-world shape: multi-fig, cyc'd velocity params, every/align, 5 routes
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "hit" :16
                 (fig . - . .)
                 (fig . - - (minvel 0) (dashdecay 0.1) (dotdecay (cyc 0.1 0)) (/ 2))
                 (fig . - - (every 4 rev) (minvel 0.0) (dotdecay 0.1)
                            (dashdecay (cyc 0.1 0.8 0)) (align 16))
                 -> 0 left
                 -> 1 (shift 3) right
                 -> 2 (shift 2) left)"#,
        )
        .expect("jaki surface macro");
        rt.eval(
            r#"(import alez.jaki.surface :refer (jak))
               (jak "hats" :16
                 . . - . - . . (dashdecay 0) (minvel 0.1)
                 (dotdecay (cyc 0.1 0.8)) (/ (cyc 1 2))
                 -> 3 left
                 -> 4 (shift 4))"#,
        )
        .expect("jaki surface macro 2");
        // simulate a session's worth of stale renamed sequencers still registered
        let stale: usize = std::env::var("JAKI_PROBE_STALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for i in 0..stale {
            rt.eval(&format!(
                r#"(import alez.jaki.surface :refer (jak))
                   (jak "stale-{i}" :16 . . - . -> 0)"#,
            ))
                .expect("stale def");
        }

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&rt.sequencer_defs(), 0.0);
        let mut out = Vec::new();
        let mut invocations: usize;
        // warm-up cycle
        generators.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| rt.invoke_sequencer_tick(input.generator_index, input).expect("tick"),
            &mut out,
        );
        let mut pos = 4.0f64;
        for window in 0..4 {
            let started = std::time::Instant::now();
            invocations = 0;
            let end = pos + 128.0;
            while pos < end {
                generators.process_block(
                    pos,
                    pos + 0.5,
                    0,
                    48_000.0,
                    |input| {
                        invocations += 1;
                        rt.invoke_sequencer_tick(input.generator_index, input).expect("tick")
                    },
                    &mut out,
                );
                pos += 0.5;
            }
            let elapsed = started.elapsed();
            eprintln!(
                "window {window}: {} ticks in {:?} => {:.3} ms/tick",
                invocations,
                elapsed,
                elapsed.as_secs_f64() * 1_000.0 / invocations.max(1) as f64,
            );
            out.clear();
        }
    }

    #[test]
    fn def_sequencer_state_cells_persist_across_ticks() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(def-sequencer "counter"
                     :resolution :4
                     :tick (do
                       (state-set! "n" (+ 1 (state-get "n" 0)))
                       (seq-emit :track 0 :at :now :note (state-get "n"))))"#,
            )
            .expect("def-sequencer");

        let mut generators = crate::generator::GeneratorRuntime::default();
        generators.sync_definitions(&runtime.sequencer_defs(), 0.0);

        let mut out = Vec::new();
        generators.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                runtime
                    .invoke_sequencer_tick(input.generator_index, input)
                    .expect("tick")
            },
            &mut out,
        );

        // :4 = quarter = 1 beat; boundaries at 1,2,3,4 -> the counter persists across
        // ticks, so transpose climbs 1,2,3,4.
        let transposes: Vec<f32> = out.iter().map(|e| e.event.resolved.transpose).collect();
        assert_eq!(transposes, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn processes_form_attaches_chain_slots_with_lanes_and_knobs() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
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

                (def climb
                  (processes :track 0
                    (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0))))
                "#,
            )
            .expect("attach process chain");

        let chain = state.track_process_chain(0).expect("track 0 chain");
        assert_eq!(chain.slots.len(), 1);
        assert_eq!(chain.slots[0].class_name, "sparse-transpose");
        assert_eq!(
            chain.slots[0].lanes.get("amount").map(|lane| &lane.values),
            Some(&vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0])
        );
        assert!(chain.slots[0].inlets.is_empty());
        assert!(state
            .track_process_chain(1)
            .expect("track 1 chain")
            .slots
            .is_empty());

        // lane! rewrites the attached lane in place via the returned handle.
        scratch
            .eval("(lane! climb :amount 0 2 0 0 2 0 0 0)")
            .expect("lane! rewrite");
        let chain = state.track_process_chain(0).expect("track 0 chain");
        assert_eq!(
            chain.slots[0].lanes.get("amount").map(|lane| &lane.values),
            Some(&vec![0.0, 2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0])
        );

        // :track :all is deprecated: the native errors (status + false) and no
        // track chain is stamped; :project is the attach-everywhere surface.
        scratch
            .eval("(processes :track :all (sparse-transpose :amount (lane 1 1)))")
            .expect("eval itself should not fail");
        assert!(
            state
                .track_process_chain(1)
                .expect("track 1 chain")
                .slots
                .is_empty(),
            "deprecated :all must not stamp track chains"
        );
        scratch.eval("(processes :track 0)").expect("clear chain");
        assert!(state
            .track_process_chain(0)
            .expect("track 0 chain")
            .slots
            .is_empty());

        // (lane ...) on a non-lane inlet is rejected: the native errors (status +
        // false, per plain-native convention) and the chain is left untouched.
        scratch
            .eval("(processes :track 0 (sparse-transpose :range (lane 1 2)))")
            .expect("eval itself should not fail");
        assert!(
            state
                .track_process_chain(0)
                .expect("track 0 chain")
                .slots
                .is_empty(),
            "rejected attach must not modify the chain"
        );
    }

    #[test]
    fn processes_project_form_declares_shared_layer_composed_ahead_of_track_chains() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
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

                (def ext (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0)))
                (processes :project ext)

                (def climb
                  (processes :track 0
                    (sparse-transpose :amount (lane 1 1 1 1))))
                "#,
            )
            .expect("attach project layer and track chain");

        // The layer lives in its own store: track chains are untouched.
        let project_chain = state.project_process_chain();
        assert_eq!(project_chain.slots.len(), 1);
        assert!(project_chain.slots[0].project_layer);
        assert_eq!(project_chain.slots[0].class_name, "sparse-transpose");
        assert_eq!(
            project_chain.slots[0]
                .lanes
                .get("amount")
                .map(|lane| &lane.values),
            Some(&vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0])
        );
        assert_eq!(
            state.track_process_chain(0).expect("track 0").slots.len(),
            1,
            "track chain composes on top, never clobbered"
        );
        assert!(state
            .track_process_chain(1)
            .expect("track 1")
            .slots
            .is_empty());

        // Every track's effective chain runs project slots first.
        for track in 0..2 {
            let composed = state
                .composed_track_process_chain(track)
                .expect("composed chain");
            assert!(
                composed.slots[0].project_layer,
                "track {track} composed chain must start with the project slot"
            );
        }
        assert_eq!(
            state
                .composed_track_process_chain(0)
                .expect("composed")
                .slots
                .len(),
            2
        );
        assert_eq!(
            state
                .composed_track_process_chain(1)
                .expect("composed")
                .slots
                .len(),
            1
        );

        // lane! on the project handle edits the single shared lane.
        scratch
            .eval("(lane! ext :amount 0 2 0 0)")
            .expect("lane! on project slot");
        assert_eq!(
            state.project_process_chain().slots[0]
                .lanes
                .get("amount")
                .map(|lane| &lane.values),
            Some(&vec![0.0, 2.0, 0.0, 0.0])
        );

        // Re-evaluating the same block preserves the edited lane (idempotent
        // whole-layer replace, matching the track-chain reconciliation rules:
        // named instances keep their pattern-owned lane edits).
        scratch
            .eval(
                r#"
                (def ext (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0)))
                (processes :project ext)
                "#,
            )
            .expect("re-eval project layer");
        assert_eq!(state.project_process_chain().slots.len(), 1);
        assert_eq!(
            state.project_process_chain().slots[0]
                .lanes
                .get("amount")
                .map(|lane| &lane.values),
            Some(&vec![0.0, 2.0, 0.0, 0.0]),
            "re-eval must not erase the edited shared lane"
        );

        // Empty form clears the whole layer.
        scratch.eval("(processes :project)").expect("clear layer");
        assert!(state.project_process_chain().slots.is_empty());
        assert_eq!(
            state
                .composed_track_process_chain(0)
                .expect("composed")
                .slots
                .len(),
            1,
            "clearing the layer leaves track chains alone"
        );
    }

    #[test]
    fn process_handle_call_writes_through_to_attached_chain_slot() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process knobbed-transpose
                  :target (step-param :transpose)
                  :in ((limit :float -24 24 :default 3 :doc "scalar limit")
                       (amount :float -12 12 :lane true :default 0))
                  :run (target-add! limit))

                (def climb
                  (processes :track 0
                    (knobbed-transpose :limit 3 :amount (lane 0 1 0 0))))

                (climb :limit 6)
                (climb :amount (lane 0 2 0 0))
                "#,
            )
            .expect("attach and mutate process chain");

        let chain = state.track_process_chain(0).expect("track 0 chain");
        assert_eq!(chain.slots.len(), 1);
        assert_eq!(
            chain.slots[0].inlets.get("limit"),
            Some(&crate::process::ProcessLiteral::Number(6.0))
        );
        assert_eq!(
            chain.slots[0].lanes.get("amount").map(|lane| &lane.values),
            Some(&vec![0.0, 2.0, 0.0, 0.0])
        );
        assert_eq!(
            scratch
                .eval("(climb :__inline-read :limit)")
                .expect("read current pattern-scoped inlet"),
            Some(Value::Number(6.0))
        );
    }

    #[test]
    fn inline_code_widgets_demo_evaluates_and_attaches_its_process_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        runtime
            .eval_str("(def eseq.seq-script-picker/seq-register-script-source-tab (label) nil)")
            .expect("install source-tab stub");
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );
        let source = include_str!("../../../../content/scripts/ui/inline-code-widgets-demo.lisp");

        runtime
            .eval_str(source)
            .expect("evaluate inline code widgets demo");

        let chain = state.track_process_chain(0).expect("track 0 chain");
        assert_eq!(chain.slots.len(), 1);
        assert_eq!(chain.slots[0].class_name, "inline-demo-transpose");
        assert_eq!(
            chain.slots[0].inlets.get("limit"),
            Some(&crate::process::ProcessLiteral::Number(7.0))
        );
        assert_eq!(
            chain.slots[0].lanes.get("amount").map(|lane| &lane.values),
            Some(&vec![0.0, 2.0, 0.0, 0.0, -4.0, 0.0, 7.0, 0.0])
        );

        let published_version = state.published_process_authoring_version();
        let published_ui_epoch = ui_epoch.load(Ordering::Acquire);
        for _ in 0..8 {
            assert_eq!(
                runtime
                    .eval_str("(inline-demo-process-h :__inline-read :limit)")
                    .expect("poll inline process control"),
                Some(Value::Number(7.0))
            );
        }
        assert_eq!(
            state.published_process_authoring_version(),
            published_version,
            "visible inline controls must not republish process authoring while polling values"
        );
        assert_eq!(
            ui_epoch.load(Ordering::Acquire),
            published_ui_epoch,
            "read-only inline polling must not invalidate the UI or scheduler"
        );
    }

    #[test]
    fn process_chain_demo_publishes_from_regular_runtime() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(8);
        let mut authoring = Runtime::new();
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-chain-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read process chain demo script");
        authoring
            .eval_source_at_path(script_path.into(), &source)
            .expect("evaluate process chain demo in regular runtime");

        assert!(
            ui_epoch.load(Ordering::Acquire) > 0,
            "regular runtime process chain demo should publish process authoring"
        );
        let chain = state.track_process_chain(0).expect("track 0 chain");
        assert_eq!(chain.slots.len(), 1);
        assert_eq!(chain.slots[0].class_name, "sparse-transpose");
        assert_eq!(
            chain.slots[0].lanes.get("amount").map(|lane| &lane.values),
            Some(&vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0])
        );

        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(state.published_process_authoring().to_runtime(), 0.0);
        let first_cycle = (0..8)
            .map(|step| processes.step_process_writes(&chain.slots[0], step, 0, 8)[0].value)
            .collect::<Vec<_>>();
        let second_cycle = (0..8)
            .map(|step| processes.step_process_writes(&chain.slots[0], step, 1, 8)[0].value)
            .collect::<Vec<_>>();
        assert_eq!(first_cycle, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
        assert_eq!(second_cycle, vec![2.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0, 4.0]);
    }

    #[test]
    fn def_process_named_instance_preserves_state_across_scheduler_ticks() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process counter
                  :in ((step :float 0 12 :default 1))
                  :out ((value :float))
                  :state ((x 0))
                  :every (beats 1)
                  :run (do
                    (set! x (+ x (in :step)))
                    (out :value x)))

                (def wander (counter :step 2))
                (start wander)
                "#,
            )
            .expect("def-process");

        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(scratch.process_authoring_snapshot(), 0.0);

        let first = processes.process_block(0.0, 1.0, 0, 48_000.0);
        assert_eq!(first.len(), 1);
        let first_result = scratch
            .invoke_process_run(first[0].clone())
            .expect("first process tick");
        assert_eq!(first_result.outputs[0].value, Value::Number(2.0));
        processes.apply_run_result(first_result);

        let second = processes.process_block(1.0, 2.0, 48_000, 48_000.0);
        assert_eq!(second.len(), 1);
        let second_result = scratch
            .invoke_process_run(second[0].clone())
            .expect("second process tick");
        assert_eq!(second_result.outputs[0].value, Value::Number(4.0));
    }

    #[test]
    fn process_transpose_wander_demo_loads_and_ticks() {
        let state = Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-transpose-wander-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read process transpose demo script");
        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate process transpose demo script");

        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(scratch.process_authoring_snapshot(), 0.0);

        let mut outlet_values = Vec::new();
        let mut emitted_marker_count = 0;
        for beat in 0..4 {
            let invocations =
                processes.process_block(beat as f64, beat as f64 + 1.0, beat * 48_000, 48_000.0);
            assert_eq!(
                invocations.len(),
                1,
                "expected one process tick at beat {beat}"
            );
            let result = scratch
                .invoke_process_run(invocations[0].clone())
                .expect("invoke process demo tick");
            assert_eq!(
                result.outputs.len(),
                2,
                "demo should publish one outlet and one channel send"
            );
            outlet_values.extend(result.outputs.iter().find_map(|output| {
                (output.name == "value").then_some(match &output.value {
                    Value::Number(value) => *value,
                    _ => panic!("value outlet should be numeric"),
                })
            }));
            if !result.emissions.is_empty() {
                emitted_marker_count += result.emissions.len();
                assert_eq!(result.emissions[0].track, Some(0));
            }
            processes.apply_run_result(result);
        }

        assert_eq!(outlet_values, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(processes.global_transpose(), 4.0);
        assert_eq!(emitted_marker_count, 1);
        let due = processes.take_due_emissions(4.0);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].beat, 4.0);
    }

    #[test]
    fn process_transpose_wander_demo_publishes_from_regular_runtime() {
        let state = Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut authoring = Runtime::new();
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-transpose-wander-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read process transpose demo script");
        authoring
            .eval_source_at_path(script_path.into(), &source)
            .expect("evaluate process demo in regular runtime");
        assert!(
            ui_epoch.load(Ordering::Acquire) > 0,
            "regular runtime process authoring should publish"
        );

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(state.published_process_authoring().to_runtime(), 0.0);

        let mut outlet_values = Vec::new();
        for beat in 0..4 {
            let invocations =
                processes.process_block(beat as f64, beat as f64 + 1.0, beat * 48_000, 48_000.0);
            assert_eq!(
                invocations.len(),
                1,
                "expected one process tick at beat {beat}"
            );
            let result = scratch
                .invoke_process_run(invocations[0].clone())
                .expect("invoke regular-runtime published process tick");
            outlet_values.extend(result.outputs.iter().find_map(|output| {
                (output.name == "value").then_some(match &output.value {
                    Value::Number(value) => *value,
                    _ => panic!("value outlet should be numeric"),
                })
            }));
            processes.apply_run_result(result);
        }

        assert_eq!(outlet_values, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(processes.global_transpose(), 4.0);
        assert_eq!(processes.take_due_emissions(4.0).len(), 1);
    }

    #[test]
    fn def_process_preserves_inlet_metadata_and_auto_binds_inlets() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process lane-transpose
                  :target (step-param :transpose)
                  :seed :locked
                  :in ((delta :float -12 12 :default 0 :lane true :doc "Delta"))
                  :run (target-add! delta))
                "#,
            )
            .expect("define process with inlet metadata");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "lane-transpose")
            .expect("process definition");
        assert_eq!(def.seed_policy, crate::process::ProcessSeedPolicy::Locked);
        assert_eq!(
            def.ports,
            vec![crate::process::ProcessPortDef::default_with_target(
                crate::process::ProcessTargetHint::StepParam {
                    param: "transpose".to_string()
                }
            )]
        );
        let inlet = &def.inlets[0];
        assert_eq!(inlet.name, "delta");
        assert_eq!(inlet.kind, crate::process::ProcessInletKind::Float);
        assert_eq!(inlet.min, Some(-12.0));
        assert_eq!(inlet.max, Some(12.0));
        assert!(inlet.lane);
        assert_eq!(inlet.doc.as_deref(), Some("Delta"));

        let roundtrip = authored
            .to_published()
            .expect("publish process authoring")
            .to_runtime();
        let roundtrip_def = roundtrip
            .defs
            .iter()
            .find(|def| def.name == "lane-transpose")
            .expect("round-tripped process definition");
        assert_eq!(roundtrip_def.inlets[0], *inlet);

        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 77,
                source: def.run_source.clone().expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::from([("delta".to_string(), Value::Number(5.0))]),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect("invoke process");
        assert_eq!(
            result.target_writes,
            vec![crate::process::ProcessTargetWrite {
                port: crate::process::DEFAULT_PROCESS_PORT.to_string(),
                target: Some(crate::process::ProcessTargetHint::StepParam {
                    param: "transpose".to_string(),
                }),
                op: crate::process::ProcessTargetOp::Add,
                value: 5.0,
            }]
        );
    }

    #[test]
    fn process_run_scene_references_ship_by_name() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new_scheduler(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"(defscene amount 2)
                   (def-process scene-process
                     :target (step-param :transpose)
                     :run (target-set! amount))"#,
            )
            .expect("define scene-driven process");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "scene-process")
            .expect("scene process definition");
        let source = def.run_source.expect("run source");
        assert!(
            source.contains("(__defscene-resolve \"amount\")"),
            "process body must carry the scene reference by name: {source}"
        );

        state
            .write_current_scene_slot("amount", crate::process::ProcessLiteral::Number(5.0))
            .expect("write scene override");
        scratch.set_scene_slot_snapshot(state.latest_scheduler_snapshot().scene_slots.clone());
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 92,
                source,
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .expect("invoke scene-driven process");
        assert_eq!(result.target_writes[0].value, 5.0);
    }

    #[test]
    fn process_run_source_is_expanded_before_shipping() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (defmacro cached-process-write () `(target-set! 1))
                (def-process cached-macro-process
                  :target (step-param :transpose)
                  :run (cached-process-write))
                "#,
            )
            .expect("define cached macro process");

        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "cached-macro-process")
            .expect("cached macro process definition");
        let invocation = crate::process::ProcessRunInvocation {
            runtime_id: 91,
            source: def.run_source.expect("run source"),
            beat: 0.0,
            sample_time: 0,
            inlets: HashMap::new(),
            state: HashMap::new(),
            event: None,
            step_context: None,
            ports: def.ports,
            reads: crate::process::ProcessReadSnapshot::default(),
            seed: 1,
        };
        assert!(
            !invocation.source.contains("cached-process-write"),
            "shipped source must contain only expansion residue: {}",
            invocation.source
        );
        assert!(
            invocation.source.contains("(target-set! 1)"),
            "shipped source must contain the expanded kernel form: {}",
            invocation.source
        );
        assert!(
            !invocation.source.contains("__source-origin"),
            "shipped source must not contain authoring provenance: {}",
            invocation.source
        );
        let first = scratch
            .invoke_process_run(invocation.clone())
            .expect("invoke expanded process source");
        assert_eq!(first.target_writes[0].value, 1.0);

        scratch
            .eval("(defmacro cached-process-write () `(target-set! 2))")
            .expect("redefine authoring-side macro");
        let unchanged = scratch
            .invoke_process_run(invocation)
            .expect("invoke already-shipped process after macro redefinition");
        assert_eq!(unchanged.target_writes[0].value, 1.0);
    }

    #[test]
    fn process_declaration_clauses_parse_expanded_macros_without_authoring_provenance() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def c (defchan c 0))
                (defmacro process-rate () `(beats 5))
                (defmacro process-target () `(step-param :transpose))
                (defmacro process-listens () `((event (channel c))))
                (def-process expanded-declarations
                  :every (process-rate)
                  :target (process-target)
                  :listen (process-listens)
                  :on-event (lambda (value) value)
                  :run (target-set! 1))
                "#,
            )
            .expect("define process with expanded declaration clauses");

        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "expanded-declarations")
            .expect("expanded process definition");
        assert_eq!(def.every, Some(crate::process::ProcessTimeExpr::Beats(5.0)));
        assert_eq!(
            def.ports,
            vec![crate::process::ProcessPortDef::default_with_target(
                crate::process::ProcessTargetHint::StepParam {
                    param: "transpose".to_string(),
                },
            )],
        );
        assert_eq!(def.listens.len(), 1);
        assert_eq!(def.listens[0].name, "event");
    }

    #[test]
    fn def_process_parses_named_targets_and_named_target_writes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process multi-port
                  :targets '((pitch (step-param :transpose))
                             (gate (midi-fx-target :beat-repeat :gate)))
                  :run (do
                    (target-add! :pitch 7)
                    (target-set! :gate 0.25)))
                "#,
            )
            .expect("define named-port process");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "multi-port")
            .expect("process definition");
        assert_eq!(
            def.ports,
            vec![
                crate::process::ProcessPortDef::with_target(
                    "pitch",
                    crate::process::ProcessTargetHint::StepParam {
                        param: "transpose".to_string(),
                    },
                ),
                crate::process::ProcessPortDef::with_target(
                    "gate",
                    crate::process::ProcessTargetHint::MidiFxParam {
                        fx: "beat-repeat".to_string(),
                        param: "gate".to_string(),
                    },
                ),
            ]
        );

        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 78,
                source: def.run_source.clone().expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect("invoke named-port process");
        assert_eq!(
            result.target_writes,
            vec![
                crate::process::ProcessTargetWrite {
                    port: "pitch".to_string(),
                    target: Some(crate::process::ProcessTargetHint::StepParam {
                        param: "transpose".to_string(),
                    }),
                    op: crate::process::ProcessTargetOp::Add,
                    value: 7.0,
                },
                crate::process::ProcessTargetWrite {
                    port: "gate".to_string(),
                    target: Some(crate::process::ProcessTargetHint::MidiFxParam {
                        fx: "beat-repeat".to_string(),
                        param: "gate".to_string(),
                    }),
                    op: crate::process::ProcessTargetOp::Set,
                    value: 0.25,
                },
            ]
        );
    }

    #[test]
    fn def_process_parses_mappable_target_ports() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process mappable-port
                  :targets '((shape :mappable :device-param)
                             (cutoff :mappable (param-tag :cutoff))
                             (pitch (step-param :transpose)))
                  :run (do
                    (target-set! :shape 0.5)
                    (target-set! :cutoff 0.25)
                    (target-add! :pitch 7)))
                "#,
            )
            .expect("define mappable process ports");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "mappable-port")
            .expect("process definition");
        assert_eq!(
            def.ports,
            vec![
                crate::process::ProcessPortDef::mappable(
                    "shape",
                    Some(crate::process::ProcessTargetKind::DeviceParam),
                    None,
                ),
                crate::process::ProcessPortDef::mappable(
                    "cutoff",
                    None,
                    Some(crate::process::ProcessTargetHint::ParamTag {
                        tag: "cutoff".to_string(),
                    }),
                ),
                crate::process::ProcessPortDef::with_target(
                    "pitch",
                    crate::process::ProcessTargetHint::StepParam {
                        param: "transpose".to_string(),
                    },
                ),
            ]
        );

        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 80,
                source: def.run_source.clone().expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect("invoke mappable-port process");
        assert_eq!(result.target_writes.len(), 3);
        assert_eq!(result.target_writes[0].port, "shape");
        assert_eq!(result.target_writes[0].target, None);
        assert_eq!(
            result.target_writes[1].target,
            Some(crate::process::ProcessTargetHint::ParamTag {
                tag: "cutoff".to_string(),
            })
        );
        assert_eq!(
            result.target_writes[2].target,
            Some(crate::process::ProcessTargetHint::StepParam {
                param: "transpose".to_string(),
            })
        );
    }

    #[test]
    fn process_inlet_ports_reject_parameter_mappable_declarations() {
        let error = parse_process_port_def(&[
            Value::Symbol("out".to_string()),
            Value::Keyword("mappable".to_string()),
            Value::Keyword("process-inlet".to_string()),
        ])
        .expect_err(":mappable :process-inlet must be rejected");

        assert_eq!(
            error,
            "process-inlet ports use (name :process-inlet) and connect!, not :mappable"
        );
    }

    #[test]
    fn def_accumulator_parses_mappable_target_with_kind_and_hint() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-accumulator filter-rise
                  :target :mappable
                  :target-kind :device-param
                  :target-hint (param-tag :cutoff)
                  :amount (amount :float 0 1 :lane true)
                  :range (0 1)
                  :mode :clip)
                "#,
            )
            .expect("define mappable accumulator");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "filter-rise")
            .expect("accumulator definition");
        assert_eq!(
            def.ports,
            vec![crate::process::ProcessPortDef::default_mappable(
                Some(crate::process::ProcessTargetKind::DeviceParam),
                Some(crate::process::ProcessTargetHint::ParamTag {
                    tag: "cutoff".to_string(),
                }),
            )]
        );
        assert!(def.inlets[0].lane);
    }

    #[test]
    fn def_process_rejects_legacy_untyped_target_port_placeholder() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        let result = scratch
            .eval(
                r#"
                (def-process old-placeholder
                  :targets '((aux :float))
                  :run (target-set! :aux 1))
                "#,
            )
            .expect("legacy untyped placeholder should return false");
        assert_eq!(result, Some(Value::Bool(false)));
        assert!(
            scratch
                .process_authoring_snapshot()
                .defs
                .iter()
                .all(|def| def.name != "old-placeholder"),
            "rejected placeholder process should not be registered"
        );
    }

    #[test]
    fn process_target_write_errors_on_unknown_named_port() {
        let process_eval = Arc::new(std::sync::Mutex::new(Some(super::ProcessEvalContext {
            runtime_id: 79,
            beat: 0.0,
            inlets: HashMap::new(),
            state: HashMap::new(),
            event: None,
            step_context: None,
            ports: vec![crate::process::ProcessPortDef::with_target(
                "pitch",
                crate::process::ProcessTargetHint::StepParam {
                    param: "transpose".to_string(),
                },
            )],
            outputs: Vec::new(),
            reads: crate::process::ProcessReadSnapshot::default(),
            conductor_observe_tracks: Vec::new(),
            conductor_play_tracks: Vec::new(),
            emissions: Vec::new(),
            commands: Vec::new(),
            target_writes: Vec::new(),
            transpose: None,
            random_state: 123,
            scope: super::ProcessEvalScope::Run,
        })));

        let err = super::push_process_target_write(
            &process_eval,
            crate::process::ProcessTargetOp::Add,
            Some("missing".to_string()),
            1.0,
        )
        .expect_err("unknown target port should fail");
        assert!(
            err.contains("unknown process target port 'missing'"),
            "{err}"
        );
    }

    fn test_process_step_context() -> crate::process::ProcessStepEventContext {
        crate::process::ProcessStepEventContext {
            track: 0,
            step: 0,
            cycle: 0,
            beat: 0.0,
            sample_time: 0,
            step_beats: 0.25,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 5.0,
                pan: 0.0,
                chop: 1.0,
            },
        }
    }

    #[test]
    fn process_run_collects_ordered_veto_ratchet_and_target_write_commands() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process command-check
                  :target (step-param :transpose)
                  :run (do
                    (target-add! 2)
                    (veto!)
                    (ratchet! :times 2 :mode :subdivide :span 0.5)))
                "#,
            )
            .expect("define command-check process");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "command-check")
            .expect("process definition");

        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 701,
                source: def.run_source.expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: Some(test_process_step_context()),
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect("invoke command-check process");

        assert_eq!(result.target_writes.len(), 1);
        assert_eq!(result.commands.len(), 3);
        assert!(matches!(
            result.commands[0],
            crate::process::ProcessRunCommand::TargetWrite(_)
        ));
        assert!(matches!(
            result.commands[1],
            crate::process::ProcessRunCommand::VetoBaseEvent
        ));
        match &result.commands[2] {
            crate::process::ProcessRunCommand::Ratchet(request) => {
                assert_eq!(request.times, 2);
                assert_eq!(request.mode, crate::process::ProcessRatchetMode::Subdivide);
                assert_eq!(request.span_beats, Some(0.5));
                assert!(request.shape.is_none());
                assert_eq!(request.shape_context.step_context.step_beats, 0.25);
            }
            other => panic!("expected ratchet command, got {other:?}"),
        }
    }

    #[test]
    fn process_ratchet_shape_mutates_event_handle_in_invocation_order() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process shaped-ratchet
                  :in ((boost :float 0 24 :default 0))
                  :run (ratchet! :times 1
                                  :mode :repeat
                                  :span (step-length)
                                  :shape (lambda (i ev)
                                           (do
                                             (note! ev (+ (note ev) (in :boost) i))
                                             (vel! ev (* (vel ev) 0.5))
                                             (nudge! ev 0.125)
                                             ev))))
                "#,
            )
            .expect("define shaped-ratchet process");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "shaped-ratchet")
            .expect("process definition");
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 702,
                source: def.run_source.expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::from([("boost".to_string(), Value::Number(3.0))]),
                state: HashMap::new(),
                event: None,
                step_context: Some(test_process_step_context()),
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect("invoke shaped-ratchet process");
        let crate::process::ProcessRunCommand::Ratchet(mut request) =
            result.commands.into_iter().next().expect("ratchet command")
        else {
            panic!("expected ratchet command");
        };
        let shape = request.shape.clone().expect("shape closure");
        let shaped = scratch
            .invoke_process_ratchet_shape(
                &mut request.shape_context,
                &shape,
                2,
                crate::process::ProcessRatchetEvent {
                    offset_beats: 0.0,
                    resolved: test_process_step_context().resolved,
                },
            )
            .expect("invoke ratchet shape");
        assert_eq!(shaped.offset_beats, 0.125);
        assert_eq!(shaped.resolved.transpose, 10.0);
        assert!((shaped.resolved.velocity - 0.4).abs() < 1e-6);
    }

    #[test]
    fn process_veto_and_ratchet_require_step_event_context() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process invalid-verdicts
                  :run (do
                    (veto!)
                    (ratchet! :times 1)))
                "#,
            )
            .expect("define invalid-verdicts process");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "invalid-verdicts")
            .expect("process definition");
        let err = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 703,
                source: def.run_source.expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 123,
            })
            .expect_err("veto! outside step context should fail");
        assert!(
            err.contains("requires a scheduler step event context"),
            "{err}"
        );
    }

    #[test]
    fn process_builtin_library_loads_all_shipped_processes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(&super::load_process_library_source())
            .expect("builtin process library should eval");
        let defs = scratch.process_authoring_snapshot().defs;
        let names = defs.iter().map(|def| def.name.clone()).collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "prob-mask"), "{names:?}");
        assert!(names.iter().any(|name| name == "repeater"), "{names:?}");
        assert!(names.iter().any(|name| name == "dice"), "{names:?}");
        assert!(names.iter().any(|name| name == "echo-track"), "{names:?}");
        assert!(names.iter().any(|name| name == "wrap-crash"), "{names:?}");
        assert!(
            names.iter().any(|name| name == "follow-harmony"),
            "{names:?}"
        );
        assert!(defs.iter().all(|def| {
            def.source_path
                .as_deref()
                .is_some_and(|path| path.ends_with("processes/builtin.lisp"))
        }));
    }

    #[test]
    fn process_read_family_resolves_tracks_process_values_and_channels() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process read-family-probe
                  :target (step-param :transpose)
                  :run (target-add!
                         (+ (read (track 0 :transpose))
                            (read (track 0 :transpose :steps-ago 1))
                            (read (track 0 :transpose :trigs-ago 0))
                            (read (track 0 :fire-count :window (bars 1)))
                            (read (process :brain :value))
                            (read :channel :density))))
                "#,
            )
            .expect("define read-family probe");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "read-family-probe")
            .expect("read-family definition");
        let mut current = std::array::from_fn(|index| StepParam::ALL[index].default_value());
        current[StepParam::Transpose.index()] = 2.0;
        let mut step_zero = current;
        step_zero[StepParam::Transpose.index()] = 3.0;
        let mut step_one = current;
        step_one[StepParam::Transpose.index()] = 4.0;
        let mut trig = current;
        trig[StepParam::Transpose.index()] = 5.0;
        let reads = crate::process::ProcessReadSnapshot {
            tracks: Arc::new(vec![crate::process::ProcessTrackReadSnapshot {
                current,
                steps: vec![step_zero, step_one],
                trigs: vec![trig, trig, trig],
                trig_beats: vec![0.9, 0.5, -3.1],
            }]),
            process_values: HashMap::from([(
                "brain".to_string(),
                HashMap::from([("value".to_string(), Value::Number(6.0))]),
            )]),
            channels: HashMap::from([("density".to_string(), Value::Number(7.0))]),
            fields: HashMap::new(),
            conductor_observe_tracks: Vec::new(),
            conductor_play_tracks: Vec::new(),
        };
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 1,
                source: def.run_source.expect("run source"),
                beat: 1.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports,
                reads,
                seed: 1,
            })
            .expect("invoke read-family probe");
        assert_eq!(result.target_writes[0].value, 26.0);
    }

    #[test]
    fn graph_homeostat_nudges_use_process_commands_and_immediate_host_queue() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut publisher = Runtime::new();
        register_graph_def_sequencer_test_native(&mut publisher, Arc::clone(&state));
        publisher
            .eval_str(
                r#"
                (def-sequencer "homeostat-test"
                  :shape (line 2)
                  :energy-decay 1
                  :reset-every 0
                  :seed-on-reset 0
                  :max-poly 2
                  (def-node nrn
                    :resolution :16
                    :delay 2
                    :route 0
                    :seed-from ()
                    :params ((threshold :float 0 4 :default 1)
                             (transpose :int -48 48 :default 0))
                    :state ((energy :leak (per-step :energy-decay)))
                    :update nil)
                  (edges
                    :from nrn :to nrn :topology (all-to-all)
                    :gather (edge :weight)
                    :params ((weight :float -1 1 :default 0)
                             (dampening :float 0 1 :default 0))))
                "#,
            )
            .expect("publish graph");

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process homeostat-command-probe
                  :run (do
                         (graph-clear-deltas! "homeostat-test")
                         (graph-nudge-param! "homeostat-test" 0 :transpose 1.5)
                         (graph-nudge-node! "homeostat-test" 0 :delay -0.5)
                         (graph-nudge-edge! "homeostat-test"
                           :from 0 :to 1 :weight 0.2)))
                "#,
            )
            .expect("define graph homeostat command probe");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "homeostat-command-probe")
            .expect("homeostat command definition");
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 1,
                source: def.run_source.expect("run source"),
                beat: 1.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .expect("invoke homeostat command probe");
        assert_eq!(result.commands.len(), 4);
        assert!(matches!(
            result.commands.first(),
            Some(crate::process::ProcessRunCommand::Graph(
                crate::graph::GraphControlCommand::Clear { .. }
            ))
        ));
        assert!(result.commands.iter().skip(1).all(|command| matches!(
            command,
            crate::process::ProcessRunCommand::Graph(
                crate::graph::GraphControlCommand::Nudge(_)
            )
        )));
        assert!(state.current_graph_overrides().is_empty());

        scratch
            .eval("(graph-clear-deltas! \"homeostat-test\")")
            .expect("queue immediate clear");
        assert!(matches!(
            state.drain_graph_control_commands().as_slice(),
            [crate::graph::GraphControlCommand::Clear { .. }]
        ));

        let demo = std::fs::read_to_string(crate::app_paths::app_paths().scripts_dir().join("processes/graph-homeostat-demo.lisp"))
        .expect("read graph homeostat demo");
        scratch.eval(&demo).expect("evaluate graph homeostat demo");
        let names = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "graph-homeostat"));
        assert!(names.iter().any(|name| name == "graph-restructurer"));

        scratch
            .eval(
                r#"
                (def-process invalid-homeostat-commit
                  :run (graph-commit-deltas! "homeostat-test"))
                "#,
            )
            .expect("define invalid commit probe");
        let invalid = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "invalid-homeostat-commit")
            .expect("invalid commit definition");
        assert!(scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 2,
                source: invalid.run_source.expect("run source"),
                beat: 1.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: invalid.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .is_err());
        scratch
            .eval("(graph-clear-deltas! \"homeostat-test\")")
            .expect("process error must not poison later host actions");
        assert!(matches!(
            state.drain_graph_control_commands().as_slice(),
            [crate::graph::GraphControlCommand::Clear { .. }]
        ));
    }

    #[test]
    fn graph_commit_deltas_promotes_one_authored_edit_then_queues_clear() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut publisher = Runtime::new();
        register_graph_def_sequencer_test_native(&mut publisher, Arc::clone(&state));
        publisher
            .eval_str(
                r#"
                (def-sequencer "commit-test"
                  :shape (line 1)
                  :energy-decay 1
                  :reset-every 0
                  :seed-on-reset 0
                  :max-poly 1
                  (def-node nrn
                    :resolution :16 :delay 4 :route 0 :seed-from ()
                    :params ((transpose :int -48 48 :default 0))
                    :state ((energy :leak (per-step :energy-decay)))
                    :update nil)
                  (edges
                    :from nrn :to nrn :topology (all-to-all)
                    :gather (edge :weight)
                    :params ((weight :float -1 1 :default 0))))
                "#,
            )
            .expect("publish graph");
        let manifest = state
            .published_sequencers()
            .into_iter()
            .find_map(|published| published.graph)
            .expect("published graph manifest");
        state.set_graph_visualizations(vec![crate::graph::GraphVisualizationSnapshot {
            id: manifest.id,
            name: manifest.name.clone(),
            deltas: vec![
                crate::graph::GraphDeltaEntry {
                    key: crate::graph::GraphDeltaKey::NodeDelay { node: 0 },
                    delta: -1.5,
                },
                crate::graph::GraphDeltaEntry {
                    key: crate::graph::GraphDeltaKey::NodeParam {
                        node: 0,
                        param: "transpose".to_string(),
                    },
                    delta: 2.75,
                },
            ],
            ..crate::graph::GraphVisualizationSnapshot::default()
        }]);

        let mut runtime = Runtime::new();
        register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::new(AtomicUsize::new(0)),
        );
        runtime
            .eval_str("(graph-commit-deltas! \"commit-test\")")
            .expect("commit graph deltas");

        let overrides = state.current_graph_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].node_intrinsics[0].delay_steps, Some(3));
        assert_eq!(overrides[0].node_params[0].value, 2.0);
        assert!(matches!(
            state.drain_graph_control_commands().as_slice(),
            [crate::graph::GraphControlCommand::Clear { .. }]
        ));
    }

    #[test]
    fn suggest_normalizes_scalar_gate_and_pitch_field_domains() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-process typed-field-publisher
                  :run (do
                         (suggest :density 0.25)
                         (suggest :accent true)
                         (suggest :harmony
                           (pitch-field (list 0 4 7) :root 0 :weight 0.8))))
                "#,
            )
            .expect("define typed field publisher");
        let def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "typed-field-publisher")
            .expect("typed field publisher definition");
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 1,
                source: def.run_source.expect("run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: HashMap::new(),
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: def.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .expect("invoke typed field publisher");

        let domains = result
            .outputs
            .iter()
            .map(|output| {
                (
                    output.name.as_str(),
                    super::process_field_domain(&output.value).expect("typed field domain"),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(
            domains.get("__field:density").map(String::as_str),
            Some("scalar")
        );
        assert_eq!(
            domains.get("__field:accent").map(String::as_str),
            Some("gate")
        );
        assert_eq!(
            domains.get("__field:harmony").map(String::as_str),
            Some("pitch-field")
        );
    }

    #[test]
    fn midi_fx_param_tags_are_parsed_alongside_role() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (midi-fx-param "gate"
                  :default 0.5
                  :min 0
                  :max 1
                  :role :level
                  :tags :gate "probability")
                (def-midi-fx "tagged-gate" (fx-emit 0))
                "#,
            )
            .expect("define tagged MIDI FX");

        let desc = scratch
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "tagged-gate")
            .expect("tagged MIDI FX descriptor");
        let gate_param = desc
            .params
            .iter()
            .find(|param| param.name == "gate")
            .expect("declared gate param");
        let metadata = gate_param
            .ui_metadata
            .as_ref()
            .expect("tagged param metadata");
        assert_eq!(metadata.role.as_deref(), Some("level"));
        assert_eq!(
            metadata.tags,
            vec!["gate".to_string(), "probability".to_string()]
        );
        assert!(gate_param.has_tag_or_name("gate"));
        assert!(gate_param.has_tag_or_name("probability"));
    }

    #[test]
    fn process_midi_fx_target_helper_does_not_shadow_midi_fx_param_declaration() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process midi-target-writer
                  :target (midi-fx-target :beat-repeat :gate)
                  :run (target-set! 1))

                (midi-fx-param "gate"
                  :default 0.5
                  :min 0
                  :max 1)
                (def-midi-fx "shadow-check" (fx-emit 0))
                "#,
            )
            .expect("define process target and MIDI FX param in one runtime");

        let process_def = scratch
            .process_authoring_snapshot()
            .defs
            .into_iter()
            .find(|def| def.name == "midi-target-writer")
            .expect("process definition");
        assert_eq!(
            process_def.ports[0].target,
            Some(crate::process::ProcessTargetHint::MidiFxParam {
                fx: "beat-repeat".to_string(),
                param: "gate".to_string(),
            })
        );

        let desc = scratch
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "shadow-check")
            .expect("MIDI FX descriptor");
        assert!(
            desc.params.iter().any(|param| param.name == "gate"),
            "{:?}",
            desc.params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn process_phase3a_ports_demo_loads_and_attaches_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase3a-ports-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read Phase 3A process demo");

        scratch
            .eval(&super::load_midi_fx_library_source())
            .expect("load builtin MIDI FX library");
        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate Phase 3A process demo");

        assert!(scratch
            .midi_fx_names()
            .iter()
            .any(|name| name == "beat-repeat"));
        assert!(state.pattern.track_params[0].midi_fx_chain().is_empty());
        let chain = state.track_process_chain(0).expect("track 0 process chain");
        assert_eq!(chain.slots.len(), 1);
        assert_eq!(chain.slots[0].class_name, "phase3a-port-writer");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "phase3a-port-writer")
            .expect("Phase 3A process definition");
        assert_eq!(def.ports.len(), 3);
        assert!(def.ports.iter().any(|port| {
            port.name == "pitch"
                && port.target
                    == Some(crate::process::ProcessTargetHint::StepParam {
                        param: "transpose".to_string(),
                    })
        }));
        assert!(def.ports.iter().any(|port| {
            port.name == "gate"
                && port.target
                    == Some(crate::process::ProcessTargetHint::InstrumentParam {
                        param: "release".to_string(),
                    })
                && port.is_mappable()
        }));
        assert!(def.ports.iter().any(|port| {
            port.name == "speed"
                && port.target
                    == Some(crate::process::ProcessTargetHint::InstrumentParam {
                        param: "speed".to_string(),
                    })
                && port.is_mappable()
        }));
    }

    #[test]
    fn process_phase3b_mappable_demo_loads_and_marks_only_mappable_ports() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase3b-mappable-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read Phase 3B mappable demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate Phase 3B mappable demo");

        let chain = state.track_process_chain(0).expect("track 0 process chain");
        assert_eq!(chain.slots.len(), 1);
        let slot = &chain.slots[0];
        assert_eq!(slot.class_name, "phase3b-mappable-writer");
        assert!(slot.lanes.contains_key("amount"));
        assert!(slot.lanes.contains_key("pitch"));

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "phase3b-mappable-writer")
            .expect("Phase 3B process definition");
        let pitch = def
            .ports
            .iter()
            .find(|port| port.name == "pitch")
            .expect("pitch port");
        assert_eq!(
            pitch.target,
            Some(crate::process::ProcessTargetHint::StepParam {
                param: "transpose".to_string(),
            })
        );
        assert!(!pitch.is_mappable());

        let shape = def
            .ports
            .iter()
            .find(|port| port.name == "shape")
            .expect("shape port");
        assert!(shape.is_mappable());
        assert_eq!(
            shape.target_kind,
            Some(crate::process::ProcessTargetKind::InstrumentParam)
        );
        assert_eq!(shape.target, None);

        let color = def
            .ports
            .iter()
            .find(|port| port.name == "color")
            .expect("color port");
        assert!(color.is_mappable());
        assert_eq!(
            color.target_kind,
            Some(crate::process::ProcessTargetKind::DeviceParam)
        );
        assert_eq!(color.target, None);
    }

    #[test]
    fn process_phase4_verdict_ratchet_demo_loads_and_attaches_all_lanes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase4-verdict-ratchet-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read Phase 4 verdict/ratchet demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate Phase 4 verdict/ratchet demo");

        let chain = state.track_process_chain(0).expect("track 0 process chain");
        assert_eq!(chain.slots.len(), 1);
        let slot = &chain.slots[0];
        assert_eq!(slot.class_name, "phase4-verdict-ratchet");
        for lane_name in [
            "veto",
            "times",
            "mode",
            "span",
            "note-delta",
            "vel-scale",
            "dur-scale",
            "speed-scale",
            "pan-delta",
            "chop-scale",
            "nudge",
        ] {
            assert!(
                slot.lanes.contains_key(lane_name),
                "missing lane {lane_name}; lanes={:?}",
                slot.lanes.keys().collect::<Vec<_>>()
            );
        }

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "phase4-verdict-ratchet")
            .expect("Phase 4 process definition");
        assert_eq!(def.ports.len(), 0);
        assert_eq!(def.inlets.len(), 11);
        assert!(def.inlets.iter().all(|inlet| inlet.lane));
    }

    #[test]
    fn process_phase7_reads_demo_loads_all_sources_and_attaches_reader_chain() {
        let state = Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase7-reads-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read Phase 7 reads demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate Phase 7 reads demo");

        let chain = state.track_process_chain(1).expect("track 1 reader chain");
        assert_eq!(chain.slots.len(), 7);
        assert_eq!(
            chain
                .slots
                .iter()
                .map(|slot| slot.class_name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "phase7-demo-current",
                "phase7-demo-steps",
                "phase7-demo-trigs",
                "phase7-demo-state",
                "phase7-demo-outlet",
                "phase7-demo-channel",
                "phase7-demo-steps",
            ]
        );
        assert!(chain
            .slots
            .iter()
            .all(|slot| slot.lanes.contains_key("amount")));

        let authored = scratch.process_authoring_snapshot();
        assert!(authored
            .instances
            .iter()
            .any(|instance| instance.name.as_deref() == Some("phase7-demo-brain")));
        assert!(authored
            .channels
            .iter()
            .any(|channel| channel.name.as_deref() == Some("phase7-demo-density")));
    }

    #[test]
    fn process_fields_band_demo_loads_publisher_and_independent_followers() {
        let state = Arc::new(SequencerState::new(
            3,
            (0..3).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(3),
            fallback_instrument_descriptors(3),
            0,
            0,
        );
        scratch
            .eval(&super::load_process_library_source())
            .expect("load builtin process library");
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-fields-band-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read fields band demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate fields band demo");

        assert_eq!(
            state.track_process_chain(0).expect("publisher chain").slots[0].class_name,
            "fields-band-publisher"
        );
        for track in [1, 2] {
            let chain = state.track_process_chain(track).expect("follower chain");
            assert_eq!(chain.slots.len(), 1);
            assert_eq!(chain.slots[0].class_name, "follow-harmony");
        }
    }

    #[test]
    fn process_conductor_demo_loads_one_multi_track_attachment() {
        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-conductor-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read conductor demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate conductor demo");

        let authored = scratch.process_authoring_snapshot();
        assert_eq!(authored.conductors.len(), 1);
        assert_eq!(authored.conductors[0].observe_tracks, vec![0, 1]);
        assert_eq!(authored.conductors[0].play_tracks, vec![2, 3]);
        let handle = authored.conductors[0].process_handle_id;
        assert_eq!(
            authored
                .instances
                .iter()
                .find(|instance| instance.handle_id == handle)
                .and_then(|instance| instance.name.as_deref()),
            Some("call-response-conductor-h")
        );
        let players = state.track_process_chain(0).expect("response players");
        assert_eq!(players.slots.len(), 2);
        assert!(players
            .slots
            .iter()
            .all(|slot| slot.class_name == "suggestion-response-player"));
    }

    #[test]
    fn process_project_layer_demo_loads_and_declares_the_shared_layer() {
        let state = Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-project-layer-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read project layer demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate project layer demo");

        let project_chain = state.project_process_chain();
        assert_eq!(project_chain.slots.len(), 2);
        assert!(project_chain.slots.iter().all(|slot| slot.project_layer));
        assert_eq!(project_chain.slots[0].class_name, "project-prob-mask");
        assert_eq!(project_chain.slots[1].class_name, "project-climb");
        assert!(project_chain.slots[0].lanes.contains_key("prob"));
        assert!(project_chain.slots[1].lanes.contains_key("delta"));
        // No track chain is stamped; composition happens at fire time.
        for track in 0..2 {
            assert!(state
                .track_process_chain(track)
                .expect("track chain")
                .slots
                .is_empty());
            assert_eq!(
                state
                    .composed_track_process_chain(track)
                    .expect("composed chain")
                    .slots
                    .len(),
                2
            );
        }
    }

    #[test]
    fn process_inlet_connections_attach_inline_and_via_connect_bang() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process dice-like
                  :targets ((out :process-inlet))
                  :run (target-set! :out 3))

                (def-process repeater-like
                  :in ((times :int 0 8 :default 0 :lane true))
                  :run nil)

                (def inline-src
                  (dice-like :connect '((out (process-inlet :repeater-like :times)))))
                (def mapped-src (dice-like))
                (def mapped-sink (repeater-like))

                (processes :track 0 inline-src mapped-src mapped-sink)
                (connect! mapped-src :out (inlet mapped-sink :times))
                "#,
            )
            .expect("define process-inlet connections");

        let chain = state.track_process_chain(0).expect("track 0 process chain");
        assert_eq!(chain.slots.len(), 3);
        let authored = scratch.process_authoring_snapshot();
        let dice = authored
            .defs
            .iter()
            .find(|def| def.name == "dice-like")
            .expect("dice-like process definition");
        assert_eq!(dice.ports.len(), 1);
        assert!(dice.ports[0].is_connectable());
        assert!(!dice.ports[0].is_mappable());
        assert_eq!(
            chain.slots[0].bindings.get("out"),
            Some(&Some(crate::process::ParamTarget::ProcessInlet {
                process: "repeater-like".to_string(),
                inlet: "times".to_string(),
                instance_id: None,
            }))
        );
        assert_eq!(
            chain.slots[1].bindings.get("out"),
            Some(&Some(crate::process::ParamTarget::ProcessInlet {
                process: "repeater-like".to_string(),
                inlet: "times".to_string(),
                instance_id: Some(chain.slots[2].instance_id),
            }))
        );
    }

    #[test]
    fn process_inlet_patch_demo_loads_and_wires_two_processes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-inlet-patch-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read process-inlet demo");

        scratch
            .eval_source_at_path(script_path, &source)
            .expect("evaluate process-inlet patch demo");

        let chain = state.track_process_chain(0).expect("track 0 process chain");
        assert_eq!(chain.slots.len(), 2);
        assert_eq!(chain.slots[0].class_name, "process-inlet-demo-dice");
        assert_eq!(chain.slots[1].class_name, "process-inlet-demo-repeater");
        assert_eq!(
            chain.slots[0].bindings.get("out"),
            Some(&Some(crate::process::ParamTarget::ProcessInlet {
                process: "process-inlet-demo-repeater".to_string(),
                inlet: "times".to_string(),
                instance_id: Some(chain.slots[1].instance_id),
            }))
        );
        assert!(chain.slots[0].lanes.contains_key("roll"));
        assert!(chain.slots[1].lanes.contains_key("enabled"));
    }

    #[test]
    fn process_chain_re_eval_preserves_named_slot_scene_lanes_and_mappings() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = Runtime::new();
        register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::new(AtomicUsize::new(0)),
        );

        let source = |amount_lane: &str, pitch_lane: &str| {
            format!(
                r#"
                (def-process reload-writer
                  :in ((amount :float 0 1 :default 0 :lane true)
                       (pitch :float -12 12 :default 0 :lane true))
                  :targets '((pitch (step-param :transpose))
                             (shape :mappable :instrument-param)
                             (color :mappable :device-param))
                  :run (do
                    (target-add! :pitch (in :pitch))
                    (target-set! :shape (in :amount))
                    (target-set! :color (in :amount))))

                (def reload-writer-h
                  (reload-writer
                    :amount (lane {amount_lane})
                    :pitch (lane {pitch_lane})))
                (def reload-demo (processes :track 0 reload-writer-h))
                "#
            )
        };

        runtime
            .eval_str(&source("0.1 0.2", "1 2"))
            .expect("initial script evaluation");

        let first_chain = state.track_process_chain(0).expect("initial process chain");
        let first_slot = first_chain.slots.first().expect("initial process slot");
        let first_instance_id = first_slot.instance_id;
        assert_eq!(first_slot.instance_name.as_deref(), Some("reload-writer-h"));
        assert_eq!(
            first_slot
                .lanes
                .get("amount")
                .map(|lane| lane.values.as_slice()),
            Some(&[0.1, 0.2][..])
        );

        assert!(state.set_process_port_binding(
            0,
            first_instance_id,
            "shape",
            crate::process::ParamTarget::InstrumentParam {
                param: "release".to_string(),
                param_id: None,
            },
        ));
        assert_eq!(
            state.set_process_lane_values(first_instance_id, "amount", vec![0.9, 0.8, 0.7]),
            1
        );

        runtime
            .eval_str(&source("0.3 0.4 0.5", "9 10 11"))
            .expect("same buffer re-evaluation should preserve scene-owned state");

        let chain = state
            .track_process_chain(0)
            .expect("process chain after re-evaluation");
        let slot = chain
            .slots
            .first()
            .expect("process slot after re-evaluation");
        assert_ne!(
            slot.instance_id, first_instance_id,
            "same-runtime re-eval should allocate a fresh handle"
        );
        assert_eq!(slot.instance_name.as_deref(), Some("reload-writer-h"));
        assert_eq!(
            slot.lanes.get("amount").map(|lane| lane.values.as_slice()),
            Some(&[0.9, 0.8, 0.7][..]),
            "edited scene lane values should win over script defaults"
        );
        assert_eq!(
            slot.lanes.get("pitch").map(|lane| lane.values.as_slice()),
            Some(&[1.0, 2.0][..]),
            "existing scene lane defaults should not be replaced by a script re-eval"
        );
        assert_eq!(
            slot.bindings.get("shape"),
            Some(&Some(crate::process::ParamTarget::InstrumentParam {
                param: "release".to_string(),
                param_id: None,
            }))
        );
        assert_eq!(slot.bindings.get("color"), Some(&None));
    }

    #[test]
    fn scheduler_scratch_load_does_not_replace_ui_authored_process_chain_slots() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase3a-ports-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read Phase 3A process demo");

        let mut ui_runtime = Runtime::new();
        ui_runtime
            .eval_str("(def eseq.seq-script-picker/seq-register-script-source-tab (label) nil)")
            .expect("install source-tab stub");
        register_published_process_authoring_natives(
            &mut ui_runtime,
            Arc::clone(&state),
            Arc::new(AtomicUsize::new(0)),
        );
        ui_runtime
            .eval_str(&source)
            .expect("evaluate demo in UI runtime");

        let ui_instance_id = state
            .track_process_chain(0)
            .expect("track 0 process chain")
            .slots
            .first()
            .expect("UI-authored process slot")
            .instance_id;
        assert!(
            ui_instance_id.0 >= UI_PROCESS_HANDLE_BASE,
            "UI-authored process slot should use the UI handle range, got {ui_instance_id:?}"
        );

        let mut scheduler_runtime =
            scheduler_scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
        scheduler_runtime
            .eval(&source)
            .expect("scheduler scratch should accept the demo source");

        assert_eq!(
            state
                .track_process_chain(0)
                .expect("track 0 process chain")
                .slots
                .first()
                .expect("process slot after scheduler eval")
                .instance_id,
            ui_instance_id,
            "scheduler scratch eval must not replace UI-authored chain slots with scheduler-local handles"
        );
    }

    #[test]
    fn project_scratch_reattach_preserves_saved_process_slot_settings() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-phase3a-ports-demo.lisp");
        let source = std::fs::read_to_string(&script_path).expect("read Phase 3A process demo");

        let eval_demo = |state: Arc<SequencerState>| {
            let mut runtime = Runtime::new();
            runtime
                .eval_str("(def eseq.seq-script-picker/seq-register-script-source-tab (label) nil)")
                .expect("install source-tab stub");
            register_published_process_authoring_natives(
                &mut runtime,
                state,
                Arc::new(AtomicUsize::new(0)),
            );
            runtime
                .eval_str(&source)
                .expect("evaluate demo in UI runtime");
        };

        eval_demo(Arc::clone(&state));
        let instance_id = state
            .track_process_chain(0)
            .expect("initial process chain")
            .slots
            .first()
            .expect("initial process slot")
            .instance_id;
        assert!(state.set_process_port_binding(
            0,
            instance_id,
            "gate",
            crate::process::ParamTarget::InstrumentParam {
                param: "release".to_string(),
                param_id: None,
            },
        ));
        assert_eq!(
            state.set_process_lane_values(instance_id, "gate", vec![0.0, 0.25, 0.75, 1.0]),
            1
        );
        assert_eq!(
            state.set_process_lane_values(instance_id, "speed", vec![1.0, 0.75, 0.5, 0.25]),
            1
        );
        assert!(state.set_track_process_slot_enabled(0, instance_id, false));

        // Project load restores the saved chain first, then evaluates project
        // scratch in a fresh UI runtime. The demo source reattaches the same
        // stable UI handle; that must not reset saved pattern-owned lane/binding
        // state back to the script defaults.
        eval_demo(Arc::clone(&state));

        let chain = state
            .track_process_chain(0)
            .expect("process chain after scratch reattach");
        let slot = chain.slots.first().expect("process slot after reattach");
        assert_eq!(slot.instance_id, instance_id);
        assert!(
            !slot.enabled,
            "scratch re-evaluation must preserve bypass state"
        );
        assert_eq!(
            slot.lanes.get("gate").map(|lane| lane.values.as_slice()),
            Some(&[0.0, 0.25, 0.75, 1.0][..])
        );
        assert_eq!(
            slot.lanes.get("speed").map(|lane| lane.values.as_slice()),
            Some(&[1.0, 0.75, 0.5, 0.25][..])
        );
        assert_eq!(
            slot.bindings.get("gate"),
            Some(&Some(crate::process::ParamTarget::InstrumentParam {
                param: "release".to_string(),
                param_id: None,
            }))
        );
    }

    #[test]
    fn process_chain_reconciliation_preserves_pattern_order_and_appends_new_slots() {
        let slot = |id: u64, class_name: &str| crate::process::TrackProcessSlot {
            instance_id: crate::process::ProcessInstanceId(id),
            instance_name: None,
            class_name: class_name.to_string(),
            enabled: true,
            project_layer: false,
            inlets: std::collections::BTreeMap::new(),
            lanes: std::collections::BTreeMap::new(),
            bindings: std::collections::BTreeMap::new(),
        };
        let mut bypassed = slot(3, "third");
        bypassed.enabled = false;
        let existing = crate::process::TrackProcessChain {
            slots: vec![bypassed, slot(1, "first"), slot(2, "second")],
        };
        let mut replacement = crate::process::TrackProcessChain {
            slots: vec![
                slot(1, "first"),
                slot(2, "second"),
                slot(3, "third"),
                slot(4, "new"),
            ],
        };

        super::preserve_process_slot_state(&[], &existing, &mut replacement);

        assert_eq!(
            replacement
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![3, 1, 2, 4]
        );
        assert!(!replacement.slots[0].enabled);
    }

    #[test]
    fn process_vm_helpers_and_rand_are_deterministic_per_seed() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        scratch
            .eval(
                r#"
                (def-process helper-check
                  :target (step-param :transpose)
                  :seed :locked
                  :run (target-set!
                    (+ (clip 15 0 10)
                       (wrap -1 0 8)
                       (bounce 10 0 6)
                       (floor (pow 2 3))
                       (if (gate? 0.75) 1 0)
                       (rand))))
                "#,
            )
            .expect("define helper process");

        let authored = scratch.process_authoring_snapshot();
        let def = authored
            .defs
            .iter()
            .find(|def| def.name == "helper-check")
            .expect("process definition");
        let invoke = |scratch: &mut ScratchControlRuntime, seed| {
            scratch
                .invoke_process_run(crate::process::ProcessRunInvocation {
                    runtime_id: 99,
                    source: def.run_source.clone().expect("run source"),
                    beat: 0.0,
                    sample_time: 0,
                    inlets: HashMap::new(),
                    state: HashMap::new(),
                    event: None,
                    step_context: None,
                    ports: def.ports.clone(),
                    reads: crate::process::ProcessReadSnapshot::default(),
                    seed,
                })
                .expect("invoke helper process")
                .target_writes[0]
                .value
        };

        let first = invoke(&mut scratch, 1234);
        let second = invoke(&mut scratch, 1234);
        let different_seed = invoke(&mut scratch, 4321);
        assert_eq!(first, second);
        assert_ne!(first, different_seed);
        assert!(
            (28.0..29.0).contains(&first),
            "helper sum plus rand: {first}"
        );
    }

    #[test]
    fn process_ui_control_demo_publishes_inlet_changes_from_regular_runtime() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn inlet_number(instance: &crate::process::AuthoredProcessInstance, name: &str) -> f64 {
            match instance.inlets.get(name) {
                Some(crate::process::ProcessInletValue::Literal(Value::Number(value))) => *value,
                other => panic!("expected numeric inlet {name}, got {other:?}"),
            }
        }

        fn inlet_bool(instance: &crate::process::AuthoredProcessInstance, name: &str) -> bool {
            match instance.inlets.get(name) {
                Some(crate::process::ProcessInletValue::Literal(Value::Bool(value))) => *value,
                other => panic!("expected bool inlet {name}, got {other:?}"),
            }
        }

        let state = Arc::new(SequencerState::new(
            16,
            (0..16).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut authoring = Runtime::new();
        authoring.register_reactive(
            "SEQ",
            vec![
                ("track-events", Value::List(Vec::new())),
                ("track-event-current-beat", Value::Number(0.0)),
                ("track-colors", Value::List(Vec::new())),
            ],
            true,
        );
        authoring
            .eval_str(
                r#"
                (defstate eseq.seq-step-tabs/seq-registered-step-tabs '())
                (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer source-path)
                  (set! eseq.seq-step-tabs/seq-registered-step-tabs
                    (append
                      (filter (lambda (tab) (not (= (nth tab 1) buffer)))
                        eseq.seq-step-tabs/seq-registered-step-tabs)
                      (list (list label buffer sequencer source-path)))))
                "#,
            )
            .expect("install sequencer tab registration test stub");

        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        let script_path = crate::app_paths::app_paths().scripts_dir().join("processes/process-ui-control-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read process UI control demo script");
        authoring
            .eval_source_at_path(script_path.into(), &source)
            .expect("evaluate process UI demo in regular runtime");
        assert!(
            ui_epoch.load(Ordering::Acquire) > 0,
            "regular runtime process UI demo should publish"
        );
        assert_eq!(
            authoring
                .eval_str("script-buffer-name")
                .expect("read script buffer name"),
            Some(Value::String("*process-ui*".to_string()))
        );
        assert_eq!(
            authoring
                .eval_str("script-tab-label")
                .expect("read script tab label"),
            Some(Value::String("Process UI".to_string()))
        );
        assert_eq!(
            authoring
                .eval_str("eseq.seq-step-tabs/seq-registered-step-tabs")
                .expect("read registered step tabs"),
            Some(gv_list(vec![gv_list(vec![
                Value::String("Process UI".to_string()),
                Value::String("*process-ui*".to_string()),
                Value::String(String::new()),
                Value::String(String::new()),
            ])]))
        );

        let tree = authoring
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("process UI demo should publish a widget tree");
        let layout = authoring
            .layout_snapshot_for_tree_with_viewport(&tree, Some((56.0, 20.0)))
            .expect("process UI demo widget tree should lay out");
        for key in [
            "process-ui-step",
            "process-ui-range",
            "process-ui-period",
            "process-ui-marker-every",
            "process-ui-track",
            "process-ui-enabled",
            "process-ui-markers",
            "process-ui-track-event-view",
        ] {
            let widget =
                find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("missing {key}"));
            assert_measured(widget);
        }
        let mut number_pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut number_pickers);
        assert_eq!(
            number_pickers.len(),
            4,
            "expected four numeric process controls"
        );
        let mut toggles = Vec::new();
        collect_widgets(&layout, "toggle", &mut toggles);
        assert_eq!(toggles.len(), 2, "expected run/emit toggles");
        let mut event_views = Vec::new();
        collect_widgets(&layout, "event-view", &mut event_views);
        assert_eq!(event_views.len(), 1, "expected one track event view");

        authoring
            .eval_str(
                r#"
                (process-ui-set-step 3.5)
                (process-ui-set-range 12)
                (process-ui-set-period 0.5)
                (process-ui-set-marker-every 2)
                (process-ui-set-track "Track 4")
                (process-ui-set-enabled false)
                (process-ui-set-markers false)
                "#,
            )
            .expect("process UI setters should update process inlets");

        let snapshot = state.published_process_authoring().to_runtime();
        let instance = snapshot
            .instances
            .iter()
            .find(|instance| instance.name.as_deref() == Some("process-ui-wander"))
            .expect("named process-ui-wander instance");
        assert_eq!(instance.class_name, "process-ui-bounce");
        assert!(instance.running, "demo process should be running");
        assert_eq!(inlet_number(instance, "step"), 3.5);
        assert_eq!(inlet_number(instance, "range"), 12.0);
        assert_eq!(inlet_number(instance, "period"), 0.5);
        assert_eq!(inlet_number(instance, "marker-every"), 2.0);
        assert_eq!(inlet_number(instance, "track"), 3.0);
        assert!(!inlet_bool(instance, "enabled"));
        assert!(!inlet_bool(instance, "markers"));
    }

    #[test]
    fn band_coupling_matrix_demo_publishes_ui_and_attaches_voice_ear_chains() {
        fn collect_widgets<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
            out: &mut Vec<&'a eseqlisp::layout::LayoutNode>,
        ) {
            if node.widget_type == widget_type {
                out.push(node);
            }
            for child in &node.children {
                collect_widgets(child, widget_type, out);
            }
        }

        fn find_by_stable_key<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            key: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_by_stable_key(child, key))
        }

        fn assert_measured(node: &eseqlisp::layout::LayoutNode) {
            assert!(node.rect.row.is_finite(), "{:?}", node.rect);
            assert!(node.rect.col.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width.is_finite(), "{:?}", node.rect);
            assert!(node.rect.height.is_finite(), "{:?}", node.rect);
            assert!(node.rect.width > 0.0, "{:?}", node.rect);
            assert!(node.rect.height > 0.0, "{:?}", node.rect);
        }

        fn inlet_number(instance: &crate::process::AuthoredProcessInstance, name: &str) -> f64 {
            match instance.inlets.get(name) {
                Some(crate::process::ProcessInletValue::Literal(Value::Number(value))) => *value,
                other => panic!("expected numeric inlet {name}, got {other:?}"),
            }
        }

        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut authoring = Runtime::new();
        authoring.register_reactive(
            "SEQ",
            vec![
                ("track-events", Value::List(Vec::new())),
                ("track-event-current-beat", Value::Number(0.0)),
                ("track-colors", Value::List(Vec::new())),
                ("track-process-slots", Value::List(Vec::new())),
            ],
            true,
        );
        authoring
            .eval_str(
                r#"
                (defstate eseq.seq-step-tabs/seq-registered-step-tabs '())
                (def eseq.seq-step-tabs/seq-register-script-step-sequencer-tab (label buffer sequencer source-path)
                  (set! eseq.seq-step-tabs/seq-registered-step-tabs
                    (append
                      (filter (lambda (tab) (not (= (nth tab 1) buffer)))
                        eseq.seq-step-tabs/seq-registered-step-tabs)
                      (list (list label buffer sequencer source-path)))))
                "#,
            )
            .expect("install sequencer tab registration test stub");

        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        let script_path = crate::app_paths::app_paths().scripts_dir().join("sequencers/band-coupling-matrix-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read band coupling matrix demo script");
        authoring
            .eval_source_at_path(script_path.into(), &source)
            .expect("evaluate band coupling matrix demo in regular runtime");
        assert!(
            ui_epoch.load(Ordering::Acquire) > 0,
            "band coupling matrix demo should publish"
        );

        // Loading attaches one voice + one ear per track, all cells silent.
        for track in 0..4 {
            let chain = state.track_process_chain(track).expect("band chain");
            assert_eq!(chain.slots.len(), 2, "track {track} chain length");
            assert_eq!(chain.slots[0].class_name, "band-voice");
            assert_eq!(chain.slots[1].class_name, "band-ear");
        }

        // The matrix cell setter writes the listener ear's per-source inlet.
        authoring
            .eval_str("(band-set-cell 0 2 0.9) (band-set-lag 1 4) (band-set-coupling 1.5)")
            .expect("band setters should update process inlets");

        let snapshot = state.published_process_authoring().to_runtime();
        let ear_2 = snapshot
            .instances
            .iter()
            .find(|instance| instance.name.as_deref() == Some("band-ear-2-h"))
            .expect("named band-ear-2-h instance");
        assert_eq!(ear_2.class_name, "band-ear");
        assert_eq!(inlet_number(ear_2, "a0"), 0.9);
        assert_eq!(inlet_number(ear_2, "coupling"), 1.5);
        let voice_1 = snapshot
            .instances
            .iter()
            .find(|instance| instance.name.as_deref() == Some("band-voice-1-h"))
            .expect("named band-voice-1-h instance");
        assert_eq!(inlet_number(voice_1, "lag"), 4.0);

        // Presets rewrite all sixteen cells; ring couples each listener to the
        // previous track at 0.8.
        authoring
            .eval_str("(band-apply-matrix (band-ring-matrix))")
            .expect("apply ring preset");
        let snapshot = state.published_process_authoring().to_runtime();
        let ear_1 = snapshot
            .instances
            .iter()
            .find(|instance| instance.name.as_deref() == Some("band-ear-1-h"))
            .expect("named band-ear-1-h instance");
        assert_eq!(inlet_number(ear_1, "a0"), 0.8);
        assert_eq!(inlet_number(ear_1, "a1"), 0.0);

        let tree = authoring
            .take_pending_buffer_widget_trees()
            .into_iter()
            .rev()
            .find_map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(update) => Some(update.tree),
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree { tree, .. } => Some(tree),
            })
            .expect("band coupling matrix demo should publish a widget tree");
        let layout = authoring
            .layout_snapshot_for_tree_with_viewport(&tree, Some((90.0, 24.0)))
            .expect("band coupling matrix demo widget tree should lay out");
        for key in [
            "band-coupling",
            "band-grace",
            "band-cell-matrix",
            "band-preset-clear",
            "band-preset-ring",
            "band-preset-hub",
            "band-preset-mesh",
            "band-attach-button",
            "band-detach-button",
            "band-weight-0",
            "band-lag-3",
            "band-memory-2",
            "band-track-event-view",
        ] {
            let widget =
                find_by_stable_key(&layout, key).unwrap_or_else(|| panic!("missing {key}"));
            assert_measured(widget);
        }
        let mut matrices = Vec::new();
        collect_widgets(&layout, "matrix", &mut matrices);
        assert_eq!(matrices.len(), 1, "expected the 4x4 coupling matrix");
        let mut number_pickers = Vec::new();
        collect_widgets(&layout, "number-picker", &mut number_pickers);
        assert_eq!(
            number_pickers.len(),
            14,
            "expected coupling + grace + four rows of weight/lag/memory"
        );
    }

    #[test]
    fn band_coupling_demo_run_bodies_publish_field_and_stay_inert_by_default() {
        let state = Arc::new(SequencerState::new(
            4,
            (0..4).map(|_| default_empty_effect_chain()).collect(),
        ));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(4),
            fallback_instrument_descriptors(4),
            0,
            0,
        );
        let script_path = crate::app_paths::app_paths().scripts_dir().join("sequencers/band-coupling-matrix-demo.lisp");
        let source =
            std::fs::read_to_string(&script_path).expect("read band coupling matrix demo script");
        // The script defines the processes before any UI form; only the
        // process layer is needed to probe the run bodies.
        let process_section = source
            .split(";; ── UI state mirrors")
            .next()
            .expect("process section");
        scratch
            .eval(process_section)
            .expect("evaluate band coupling process layer");

        let defs = scratch.process_authoring_snapshot().defs;

        // band-voice with its default inlet values publishes a pitch-field on
        // :band-0 even before any source trig has fired (reads are
        // defaults-inert). The scheduler resolves inlet defaults before
        // invoking, so the probe passes them explicitly.
        let voice = defs
            .iter()
            .find(|def| def.name == "band-voice")
            .expect("band-voice definition")
            .clone();
        let voice_inlets = HashMap::from([
            ("chan".to_string(), Value::Keyword("band-0".to_string())),
            ("self".to_string(), Value::Number(0.0)),
            ("weight".to_string(), Value::Number(1.0)),
            ("lag".to_string(), Value::Number(0.0)),
            ("memory".to_string(), Value::Number(3.0)),
        ]);
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 1,
                source: voice.run_source.expect("voice run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: voice_inlets,
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: voice.ports,
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .expect("invoke band-voice run");
        let field = result
            .outputs
            .iter()
            .find(|output| output.name == "__field:band-0")
            .expect("band-voice should suggest on :band-0");
        assert_eq!(
            super::process_field_domain(&field.value).expect("voice field domain"),
            "pitch-field"
        );

        // Keep the voice's published policy field around: with all reads at
        // their defaults the policy is the single pitch class 0.
        let policy = field.value.clone();

        // band-ear with no publishers and zero amounts writes nothing at all:
        // coupling is fully inert until cells are raised.
        let ear = defs
            .iter()
            .find(|def| def.name == "band-ear")
            .expect("band-ear definition")
            .clone();
        let ear_inlets = HashMap::from([
            ("a0".to_string(), Value::Number(0.0)),
            ("a1".to_string(), Value::Number(0.0)),
            ("a2".to_string(), Value::Number(0.0)),
            ("a3".to_string(), Value::Number(0.0)),
            ("coupling".to_string(), Value::Number(1.0)),
            ("grace".to_string(), Value::Number(0.0)),
        ]);
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 2,
                source: ear.run_source.clone().expect("ear run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: ear_inlets,
                state: HashMap::new(),
                event: None,
                step_context: None,
                ports: ear.ports.clone(),
                reads: crate::process::ProcessReadSnapshot::default(),
                seed: 1,
            })
            .expect("invoke band-ear run");
        assert!(
            result.target_writes.is_empty(),
            "silent matrix must not write transpose"
        );

        // With a full-amount cell from source 1 the ear conforms the current
        // note to the heard policy: a whole-snap pitch-class delta, never a
        // scaled interpolation. Policy {0} heard from transpose 1 => -1.
        let mut reads = crate::process::ProcessReadSnapshot::default();
        reads.fields.insert("band-1".to_string(), policy);
        let ear_inlets = HashMap::from([
            ("a0".to_string(), Value::Number(0.0)),
            ("a1".to_string(), Value::Number(1.0)),
            ("a2".to_string(), Value::Number(0.0)),
            ("a3".to_string(), Value::Number(0.0)),
            ("coupling".to_string(), Value::Number(1.0)),
            ("grace".to_string(), Value::Number(0.0)),
        ]);
        let step_context = crate::process::ProcessStepEventContext {
            track: 0,
            step: 0,
            cycle: 0,
            beat: 0.0,
            sample_time: 0,
            step_beats: 0.25,
            resolved: crate::accumulator::ResolvedStep {
                duration: 1.0,
                velocity: 1.0,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 1.0,
                pan: 0.0,
                chop: 0.0,
            },
        };
        let result = scratch
            .invoke_process_run(crate::process::ProcessRunInvocation {
                runtime_id: 3,
                source: ear.run_source.expect("ear run source"),
                beat: 0.0,
                sample_time: 0,
                inlets: ear_inlets,
                state: HashMap::new(),
                event: None,
                step_context: Some(step_context),
                ports: ear.ports,
                reads,
                seed: 1,
            })
            .expect("invoke band-ear conform run");
        assert_eq!(result.target_writes.len(), 1, "one conform write");
        assert_eq!(result.target_writes[0].value, -1.0);
    }

    #[test]
    fn channel_handle_resolves_an_existing_channel_without_redeclaring_it() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval("(defchan shared 0.25)")
            .expect("declare channel");
        let original = runtime.process_authoring_snapshot().channels[0].handle_id;

        runtime
            .eval("((channel-handle \"shared\") :set 0.75)")
            .expect("write through resolved handle");

        let channels = runtime.process_authoring_snapshot().channels;
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].handle_id, original);
        assert_eq!(
            state.take_process_channel_writes(),
            vec![(
                "shared".to_string(),
                crate::process::ProcessLiteral::Number(0.75),
            )]
        );
    }

    /// docs/jaki-live-channel-widgets-spec.md 7: handle writes queue for the
    /// scheduler in call order and are handed over exactly once. The value
    /// deliberately does not ride the authoring snapshot, where
    /// `sync_channels` would prefer the existing runtime value and drop it.
    #[test]
    fn channel_handle_set_queues_writes_for_the_scheduler_in_order() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval("(def warp (defchan warp 0.25))")
            .expect("declare channel");
        assert!(
            state.take_process_channel_writes().is_empty(),
            "declaring a channel is not a write"
        );

        // An inline widget bound to the channel sees the initial until the
        // author moves it, then its own last value.
        assert_eq!(
            runtime.eval("(warp :__inline-read :set)").expect("read"),
            Some(Value::Number(0.25))
        );
        runtime.eval("(warp :set 0.6)").expect("first write");
        runtime.eval("(warp :set 0.9)").expect("second write");
        assert_eq!(
            runtime.eval("(warp :__inline-read :set)").expect("read"),
            Some(Value::Number(0.9))
        );

        assert_eq!(
            state.take_process_channel_writes(),
            vec![
                ("warp".to_string(), crate::process::ProcessLiteral::Number(0.6)),
                ("warp".to_string(), crate::process::ProcessLiteral::Number(0.9)),
            ]
        );
        assert!(
            state.take_process_channel_writes().is_empty(),
            "a drained write must not be replayed"
        );

        // The authoring snapshot still carries only the declared initial.
        let channels = runtime.process_authoring_snapshot().channels;
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].initial, Some(Value::Number(0.25)));
    }

    #[test]
    fn process_tap_from_regular_runtime_publishes_listener_process() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut authoring = Runtime::new();
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        let result = authoring
            .eval_str("(def c (defchan c 0)) (tap c (lambda (v) (transpose! v)))")
            .expect("tap evaluation should publish listener process");

        assert!(matches!(result, Some(Value::HostHandle { kind, .. }) if kind == "process"));
        assert!(
            ui_epoch.load(Ordering::Acquire) > 0,
            "regular runtime tap should publish"
        );

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(state.published_process_authoring().to_runtime(), 0.0);

        let invocations = processes.send_channel_at("c", Value::Number(5.0), 2.0, 96_000);
        assert_eq!(invocations.len(), 1);
        let result = scratch
            .invoke_process_run(invocations[0].clone())
            .expect("invoke tap listener");
        assert!(processes.apply_run_result(result).is_empty());
        assert_eq!(processes.global_transpose(), 5.0);
    }

    #[test]
    fn process_on_from_regular_runtime_publishes_channel_listener_process() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut authoring = Runtime::new();
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        register_published_process_authoring_natives(
            &mut authoring,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );

        authoring
            .eval_str("(def c (defchan c 0)) (on (channel c) (lambda (v) (transpose! (+ v 1))))")
            .expect("on evaluation should publish listener process");

        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(state.published_process_authoring().to_runtime(), 0.0);

        let invocations = processes.send_channel_at("c", Value::Number(4.0), 2.0, 96_000);
        assert_eq!(invocations.len(), 1);
        let result = scratch
            .invoke_process_run(invocations[0].clone())
            .expect("invoke on listener");
        assert!(processes.apply_run_result(result).is_empty());
        assert_eq!(processes.global_transpose(), 5.0);
    }

    #[test]
    fn def_process_listen_handler_receives_channel_event_and_preserves_state() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut scratch = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def c (defchan c 0))
                (def-process counter
                  :out ((seen :float))
                  :state ((total 0))
                  :listen ((event (channel c)))
                  :on-event (lambda (v)
                    (do
                      (set! total (+ total v))
                      (out :seen total))))
                (def listener (counter))
                (start listener)
                "#,
            )
            .expect("define listener process");

        let mut processes = crate::process::ProcessRuntime::default();
        processes.sync_authoring(scratch.process_authoring_snapshot(), 0.0);

        let first = processes.send_channel_at("c", Value::Number(2.0), 1.0, 48_000);
        assert_eq!(first.len(), 1);
        let first_result = scratch
            .invoke_process_run(first[0].clone())
            .expect("invoke first listener event");
        assert_eq!(
            first_result
                .outputs
                .iter()
                .find(|output| output.name == "seen")
                .map(|output| output.value.clone()),
            Some(Value::Number(2.0))
        );
        assert!(processes.apply_run_result(first_result).is_empty());

        let second = processes.send_channel_at("c", Value::Number(3.0), 2.0, 96_000);
        assert_eq!(second.len(), 1);
        let second_result = scratch
            .invoke_process_run(second[0].clone())
            .expect("invoke second listener event");
        assert_eq!(
            second_result
                .outputs
                .iter()
                .find(|output| output.name == "seen")
                .map(|output| output.value.clone()),
            Some(Value::Number(5.0))
        );
    }

    #[test]
    fn seq_emit_quantize_snaps_offset_to_grid() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );
        runtime
            .eval(
                r#"(__register-sequencer "q"
                     :resolution :16
                     :tick (lambda () (seq-emit :track 0 :at :now :quantize :4)))"#,
            )
            .expect("register sequencer");

        // Boundary at beat 0.30 with :4 (quarter, 1 beat) quantize -> snap up to 1.0,
        // so offset = 1.0 - 0.30 = 0.70 beats.
        let result = runtime
            .invoke_sequencer_tick(
                0,
                crate::generator::GeneratorTickInput {
                    id: 0,
                    generator_index: 0,
                    tick_index: 0,
                    beat: 0.30,
                    resolution_beats: 0.25,
                    samples_per_quarter: 48_000.0,
                    random_state: 1,
                    state: Default::default(),
                },
            )
            .expect("tick");
        assert_eq!(result.emitted.len(), 1);
        assert!((result.emitted[0].offset_beats - 0.70).abs() < 1e-5);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_arp_phase_rotates_live_notes() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        runtime.eval(&super::load_midi_fx_library_source()).unwrap();

        let invoke = |runtime: &mut ScratchControlRuntime, arp_phase_beats| {
            runtime
                .invoke_midi_fx_with_arp_phase_beats(
                    0,
                    0,
                    0,
                    0.0,
                    ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    vec![0.0, 4.0, 7.0],
                    vec![1.0, 1.0, 1.0],
                    0.0,
                    Some(vec![
                        AccumulatorNoteSpan {
                            transpose: 0.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                        AccumulatorNoteSpan {
                            transpose: 4.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                        AccumulatorNoteSpan {
                            transpose: 7.0,
                            start_beats: 0.0,
                            end_beats: 0.25,
                        },
                    ]),
                    EffectSlotSnapshot::new_empty(),
                    arp_phase_beats,
                    0.25,
                    16,
                    vec![EffectSlotSnapshot::new_default(
                        &EffectDescriptor::builtin_filter(),
                        42,
                    )],
                    EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap()
        };

        assert_eq!(invoke(&mut runtime, 0.0).emitted[0].resolved.transpose, 0.0);
        assert_eq!(
            invoke(&mut runtime, 0.25).emitted[0].resolved.transpose,
            4.0
        );
        assert_eq!(invoke(&mut runtime, 0.5).emitted[0].resolved.transpose, 7.0);
    }

    #[test]
    fn scratch_control_runtime_midi_fx_state_persists_per_track() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(2),
            fallback_instrument_descriptors(2),
            0,
            0,
        );

        runtime
            .eval(
                r#"
                (def-midi-fx "every-two-octave"
                  (do
                    (if (= (fx-state-get :count 0) 1)
                      (do
                        (fx-state-set :count 0)
                        (fx-emit 0 :transpose (+ 12 (fx-note 0))))
                      (fx-state-set :count 1))))
                "#,
            )
            .unwrap();

        let invoke = |runtime: &mut ScratchControlRuntime, track| {
            runtime
                .invoke_midi_fx(
                    0,
                    track,
                    0,
                    0.0,
                    ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    vec![0.0],
                    vec![1.0],
                    0.0,
                    None,
                    EffectSlotSnapshot::new_empty(),
                    0.25,
                    16,
                    vec![EffectSlotSnapshot::new_default(
                        &EffectDescriptor::builtin_filter(),
                        42,
                    )],
                    EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(2)[track], 7),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap()
        };

        assert_eq!(invoke(&mut runtime, 0).emitted.len(), 0);
        assert_eq!(invoke(&mut runtime, 0).emitted[0].resolved.transpose, 12.0);
        assert_eq!(invoke(&mut runtime, 1).emitted.len(), 0);
        assert_eq!(invoke(&mut runtime, 1).emitted[0].resolved.transpose, 12.0);
    }

    #[test]
    fn scratch_control_runtime_can_register_closure_accumulator_directly() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let mut runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        );

        let result = runtime
            .eval(
                r#"
                (__register-accumulator "closure-acc"
                  (lambda (step value)
                    (do
                      (acc-add-step-param :transpose value)
                      (acc-scale-step-param :velocity 0.5)
                      (acc-set-step-param :pan 0.25)
                      (acc-set-effect-param 0 1 value)
                      (acc-set-instrument-param 0 0.75))))
                "#,
            )
            .unwrap();
        let status = runtime.take_status_message();

        assert_eq!(result, Some(Value::Bool(true)), "status: {status:?}");
        assert_eq!(
            runtime.accumulator_names(),
            vec!["closure-acc".to_string()],
            "status: {status:?}"
        );

        runtime
            .invoke_accumulator(
                0,
                3,
                3.0,
                ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 2.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                Vec::new(),
                Vec::new(),
                2.0,
                None,
                0.25,
                16,
                vec![EffectSlotSnapshot::new_default(
                    &EffectDescriptor::builtin_filter(),
                    42,
                )],
                EffectSlotSnapshot::new_default(&fallback_instrument_descriptors(1)[0], 7),
                vec![ScheduledEffectParam {
                    logical_id: 42,
                    idx: 1,
                    value: 0.0,
                }],
                vec![ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 0,
                    span: 1,
                    value: 0.1,
                }],
            )
            .unwrap();
    }

    #[test]
    fn scratch_runtime_editor_loads_init_bindings_for_eval() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = ScratchControlRuntime::new(
            Arc::clone(&state),
            fallback_effect_descriptors(1),
            fallback_instrument_descriptors(1),
            0,
            0,
        )
        .into_parts()
        .0;
        let init_src = read_eseqlisp_init_source();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init_src),
                ..EditorConfig::default()
            },
        );
        editor.open_scratch_buffer_with_mode("*scratch*", "(+ 1 1)", BufferMode::ESeqLisp);
        editor.active_buffer_mut().cursor = (0, "(+ 1 1)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        assert_eq!(editor.minibuffer.unwrap_or_default(), "2");
    }

    #[test]
    fn parse_manifest_reads_wavetable_tensor_metadata() {
        let manifest = parse_manifest(
            r#"{
              "version": 1,
              "processAbi": "dgen-host-abi-v1",
              "dylib": "test.dylib",
              "totalMemorySlots": 16,
              "params": [],
              "inputs": [],
              "outputs": [{"channel": 0, "name": "audio"}],
              "modulators": [],
              "modDestinations": [],
              "tensors": [
                {
                  "name": "waves",
                  "cellOffset": 4,
                  "shape": [2, 4],
                  "kind": "wavetable",
                  "mutable": false,
                  "sourceFile": "waves/tiny.json"
                }
              ],
              "tensorInitData": [
                {"offset": 4, "data": [0.0, 0.25, 0.5, 0.75, 1.0, 0.5, 0.0, -0.5]}
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].name, "waves");
        assert_eq!(manifest.tensors[0].cell_offset, 4);
        assert_eq!(manifest.tensors[0].shape, vec![2, 4]);
        assert_eq!(manifest.tensors[0].kind, "wavetable");
        assert!(!manifest.tensors[0].mutable);
        assert_eq!(
            manifest.tensors[0].source_file.as_deref(),
            Some("waves/tiny.json")
        );
        assert_eq!(manifest.tensor_init_data[0].offset, 4);
        assert_eq!(manifest.tensor_init_data[0].data.len(), 8);
    }

    #[test]
    fn compile_instrument_passes_asset_base_for_wavetable_files() {
        let root = std::env::temp_dir().join(format!(
            "sequencer-wavetable-asset-test-{}",
            std::process::id()
        ));
        let waves = root.join("waves");
        std::fs::create_dir_all(&waves).unwrap();
        std::fs::write(
            waves.join("tiny.json"),
            r#"{"shape":[4,2],"data":[0.0,1.0,0.25,0.5,0.5,0.0,0.75,-0.5]}"#,
        )
        .unwrap();

        let source = r#"
            (def gate (in 1 @name gate))
            (def pitch (in 2 @name pitch))
            (def velocity (in 3 @name velocity))
            (def trigger (in 4 @name trigger))
            (def waves (tensor @shape [4 2] @file "waves/tiny.json"))
            (out (* (peek waves 1 0) gate velocity) 1 @name audio)
        "#;

        let json = compile_instrument_with_asset_base(source, 44_100, Some(&root)).unwrap();
        let manifest = parse_manifest(&json).unwrap();
        assert_eq!(manifest.tensors.len(), 1);
        assert_eq!(manifest.tensors[0].name, "waves");
        assert_eq!(manifest.tensors[0].shape, vec![4, 2]);
        assert_eq!(manifest.tensor_init_data[0].data.len(), 8);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiled_tensor_param_manifest_derives_instrument_tensor_descriptor() {
        let root = std::env::temp_dir().join(format!(
            "sequencer-tensor-param-asset-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("strike-mask.json"), "[0.1,0.2,0.3,0.4]").unwrap();

        let source = r#"
            (def strike-mask
              (tensor-param @shape [2 2] @name strike_mask @default-file "strike-mask.json"))
            (param gain @default 1.0 @min 0.0 @max 1.0)
            (out (* 0.0 gain) 1 @name audio)
        "#;

        let json = compile_instrument_with_asset_base(source, 44_100, Some(&root)).unwrap();
        let manifest = parse_manifest(&json).unwrap();
        let desc = super::instrument_descriptor_from_manifest("tensor-test", &manifest);

        assert_eq!(desc.tensor_params.len(), 1);
        let tensor = &desc.tensor_params[0];
        assert_eq!(tensor.name, "strike_mask");
        assert_eq!(tensor.shape, vec![2, 2]);
        assert_eq!(tensor.rows(), 2);
        assert_eq!(tensor.cols(), 2);
        assert_eq!(tensor.default, vec![0.1, 0.2, 0.3, 0.4]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compile_instrument_manifest_includes_modulation_outputs_from_out_forms() {
        let source = r#"
            (out (phasor 0.25) 2 @name macro-a @modulator 1)
            (out (phasor 50.0) 1 @name audio)
        "#;

        let json = compile_instrument(source, 44_100).expect("instrument compiles");
        let manifest = parse_manifest(&json).expect("manifest parses");

        assert_eq!(manifest.mod_outputs.len(), 1);
        let output = &manifest.mod_outputs[0];
        assert_eq!(output.slot, 1);
        assert_eq!(output.channel, 1);
        assert_eq!(output.name, "macro-a");
        assert_eq!(output.range, "unipolar");
    }

    #[test]
    fn compile_instrument_hoists_macro_body_mod_params() {
        // A `@mod true` param declared inside a defmacro body with a `(mod ...)`
        // reference: DGenLisp's validator only sees top-level params, so the
        // compile pipeline hoists it (hoist_defmacro_params).
        let source = r#"
            (def gate (in 1 @name gate))
            (def mod1 (in 6 @name mod1 @modulator 1))
            (defmacro reverb123 (input)
              (param xyz @min 0.3 @max 8 @mod true @mod-mode additive)
              (def m (mod xyz))
              (* input m))
            (out (reverb123 gate) 1)
        "#;

        let json = compile_instrument(source, 44_100)
            .expect("param+mod inside a macro body should compile via the hoist");
        let manifest = parse_manifest(&json).expect("manifest parses");
        assert!(
            manifest.params.iter().any(|param| param.name == "xyz"),
            "hoisted macro param should appear in the manifest"
        );
    }

    #[test]
    fn patcher_writeback_for_real_instrument_compiles() {
        let path = crate::app_paths::app_paths().instruments_dir().join("bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted = eseqlisp::widget_render::patcher::emit_patch_writeback_source(
            &source,
            eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
        )
        .expect("patcher writeback should emit source");

        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-emitted instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn patcher_insert_unity_gain_before_real_instrument_output_compiles() {
        let path = crate::app_paths::app_paths().instruments_dir().join("bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_inserted_node_before_first_output(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "* 1",
            )
            .expect("patcher writeback should insert unity gain node");

        assert!(
            emitted.contains("(* "),
            "emitted source should contain an inserted multiply node:\n{emitted}"
        );
        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-edited instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn patcher_edit_gemini_piano_svf_literal_compiles() {
        let path = crate::app_paths::app_paths().instruments_dir().join("wips/gemini-piano/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read gemini-piano dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_first_node_text_edit(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "svf",
                "svf knock_cutoff 1.40 1",
            )
            .expect("patcher writeback should edit svf node text");

        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| {
                panic!("patcher-edited gemini-piano svf source should compile:\n{error}\n{emitted}")
            },
        );
    }

    #[test]
    fn patcher_insert_created_phasor_multiply_before_real_instrument_output_compiles() {
        let path = crate::app_paths::app_paths().instruments_dir().join("bass/bad-subbass1/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read bad-subbass1 dsp source");
        let emitted =
            eseqlisp::widget_render::patcher::emit_patch_writeback_with_created_phasor_multiply_before_first_output(
                &source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                "5",
            )
            .expect("patcher writeback should insert created phasor multiply chain");

        // Interaction-created ids never persist as bindings; the chain is
        // emitted under deterministic op-derived names.
        assert!(
            emitted.contains("(phasor ") && emitted.contains("(* "),
            "emitted source should contain the inserted phasor multiply chain:\n{emitted}"
        );
        assert!(
            !emitted.contains("(def created-"),
            "interaction-created ids must not leak into generated source:\n{emitted}"
        );
        // The frequency the interaction typed has to reach the emission. It
        // lands as its own value binding (`(def value 5)`), so match on the
        // binding rather than a bare "5.0" literal — but still require the
        // created phasor to read *that* binding, or an emission of
        // `(phasor 0)` would slip through.
        let frequency_bindings = emitted
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("(def ")?;
                let (name, value) = rest.split_once(' ')?;
                (value.trim_end_matches(')').trim() == "5").then(|| name.to_string())
            })
            .collect::<Vec<_>>();
        assert!(
            frequency_bindings
                .iter()
                .any(|name| emitted.contains(&format!("(phasor {name})"))),
            "the created phasor should read a binding holding the requested frequency 5, \
             found bindings {frequency_bindings:?}:\n{emitted}"
        );
        compile_instrument_with_asset_base(&emitted, 44_100, path.parent()).unwrap_or_else(
            |error| panic!("patcher-edited instrument source should compile:\n{error}\n{emitted}"),
        );
    }

    #[test]
    fn save_folder_instrument_writes_dsp_lisp_even_when_folder_is_new() {
        let name = format!("__test-agent-folder-{}/", std::process::id());
        let folder = crate::app_paths::app_paths().user_instruments_dir().join(name.trim_end_matches('/'));
        let legacy_file = crate::app_paths::app_paths().user_instruments_dir()
            .join(format!("{}.lisp", name.trim_end_matches('/')));
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&legacy_file);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_ui(&name, "(defsynth-ui (label \"ok\"))").unwrap();

        assert!(folder.join("dsp.lisp").exists());
        assert!(folder.join("ui.lisp").exists());
        assert!(
            !legacy_file.exists(),
            "folder-style saves must not fall back to legacy single-file instruments"
        );

        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&legacy_file);
    }

    #[test]
    fn missing_instrument_metadata_defaults_to_instrument_run_mode() {
        let name = format!("__test-run-mode-missing-{}/", std::process::id());
        let folder = crate::app_paths::app_paths().user_instruments_dir().join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();

        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::Instrument
        );
        assert!(!folder.join("instrument.json").exists());

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn folder_instrument_run_mode_roundtrips_to_instrument_json() {
        let name = format!("__test-run-mode-folder-{}/", std::process::id());
        let folder = crate::app_paths::app_paths().user_instruments_dir().join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::FreePatch).unwrap();

        let metadata_path = folder.join("instrument.json");
        assert_eq!(
            super::instrument_metadata_path(&name).unwrap(),
            metadata_path
        );
        assert!(metadata_path.exists());
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::FreePatch
        );
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::Instrument).unwrap();
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::Instrument
        );

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn legacy_file_instrument_run_mode_roundtrips_to_sidecar_json() {
        let name = format!("__test-run-mode-legacy-{}", std::process::id());
        let root = crate::app_paths::app_paths().user_instruments_dir();
        let source_path = root.join(format!("{name}.lisp"));
        let metadata_path = root.join(format!("{name}.instrument.json"));
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&metadata_path);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        super::save_instrument_run_mode(&name, super::CustomInstrumentRunMode::FreePatch).unwrap();

        assert_eq!(
            super::instrument_metadata_path(&name).unwrap(),
            metadata_path
        );
        assert!(metadata_path.exists());
        assert_eq!(
            super::load_instrument_run_mode(&name).unwrap(),
            super::CustomInstrumentRunMode::FreePatch
        );

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&metadata_path);
    }

    #[test]
    fn invalid_instrument_run_mode_reports_error() {
        let name = format!("__test-run-mode-invalid-{}/", std::process::id());
        let folder = crate::app_paths::app_paths().user_instruments_dir().join(name.trim_end_matches('/'));
        let _ = std::fs::remove_dir_all(&folder);

        super::save_instrument(&name, "(out 0 1 @name audio)").unwrap();
        std::fs::write(
            folder.join("instrument.json"),
            r#"{ "version": 1, "run_mode": "forever_note" }"#,
        )
        .unwrap();

        let error = super::load_instrument_run_mode(&name).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("invalid instrument run_mode"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn moved_folder_instrument_resolves_by_unique_leaf_name() {
        let leaf = format!("__test-moved-folder-{}", std::process::id());
        let folder = crate::app_paths::app_paths().user_instruments_dir()
            .join("__test-resolve-category")
            .join(&leaf);
        let direct_folder = crate::app_paths::app_paths().user_instruments_dir().join(&leaf);
        let legacy_file = crate::app_paths::app_paths().user_instruments_dir().join(format!("{leaf}.lisp"));
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
        let _ = std::fs::remove_dir_all(&direct_folder);
        let _ = std::fs::remove_file(&legacy_file);

        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();
        std::fs::write(folder.join("ui.lisp"), "(defsynth-ui (label \"ok\"))").unwrap();

        assert_eq!(
            super::instrument_source_path(&format!("{leaf}/")).unwrap(),
            folder.join("dsp.lisp")
        );
        assert_eq!(
            super::load_instrument_source(&format!("{leaf}/")).unwrap(),
            "(out 0 1 @name audio)"
        );
        assert_eq!(
            super::instrument_ui_path(&format!("{leaf}/")).unwrap(),
            folder.join("ui.lisp")
        );
        assert_eq!(
            super::load_instrument_ui_source(&format!("{leaf}/")).unwrap(),
            "(defsynth-ui (label \"ok\"))"
        );

        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
        let _ = std::fs::remove_dir_all(&direct_folder);
        let _ = std::fs::remove_file(&legacy_file);
    }

    #[test]
    fn moved_folder_instrument_leaf_match_requires_unique_source() {
        let leaf = format!("__test-ambiguous-folder-{}", std::process::id());
        let root = crate::app_paths::app_paths().user_instruments_dir();
        let first = root.join("__test-ambiguous-a").join(&leaf);
        let second = root.join("__test-ambiguous-b").join(&leaf);
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-a"));
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-b"));

        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();
        std::fs::write(second.join("dsp.lisp"), "(out 0 1 @name audio)").unwrap();

        let error = super::instrument_source_path(&format!("{leaf}/")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains("Ambiguous instrument"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-a"));
        let _ = std::fs::remove_dir_all(root.join("__test-ambiguous-b"));
    }

    #[test]
    fn instrument_preamble_uses_runtime_sample_rate_context() {
        let preamble = super::instrument_preamble(48_000);
        assert!(preamble.contains("runtime host sample-rate"));
        assert!(!preamble.contains("(def samplerate"));
        assert!(!preamble.contains("__SAMPLE_RATE__"));
    }

    #[test]
    fn adsrexp_runtime_shapes_attack_and_falling_segments_independently() {
        let source = r#"
            (def gate (in 1 @name gate))
            (def trigger (in 4 @name trigger))
            (out (adsrexp gate trigger 3.0 3.0 0.25 3.0 1.0 2.0) 1 @name audio)
        "#;
        let report = super::render_instrument_source_for_test(
            source,
            None,
            &super::InstrumentRenderOptions {
                sample_rate: 1_000,
                block_size: 4,
                frames: 12,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 8,
                voice_index: 0,
                param_overrides: Vec::new(),
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .expect("adsrexp should compile and render through the instrument host");

        assert_eq!(report.non_finite_samples, 0, "report={report:?}");
        for (frame, expected) in [
            (0, 0.25),
            (1, 0.5),
            (3, 1.0),
            (7, 0.25),
            (8, 0.140625),
        ] {
            assert!(
                (report.first_samples[frame] - expected).abs() < 0.0001,
                "frame {frame} should be {expected}, report={report:?}",
            );
        }
    }

    #[test]
    fn adsrexp_restarts_full_attack_after_completed_release() {
        let source = r#"
            (def frame (accum 1.0 0.0 0.0 1000.0))
            (def first_gate (* (gte frame 1.0) (lt frame 7.0)))
            (def second_gate (* (gte frame 14.0) (lt frame 22.0)))
            (def test_gate (+ first_gate second_gate))
            (def env (adsrexp test_gate 0.0 3.0 1.0 0.25 1.0 1.0 2.0))
            (out env 1 @name audio)
        "#;
        let report = super::render_instrument_source_for_test(
            source,
            None,
            &super::InstrumentRenderOptions {
                sample_rate: 1_000,
                block_size: 4,
                frames: 28,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 28,
                voice_index: 0,
                param_overrides: Vec::new(),
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .expect("repeated adsrexp notes should compile and render");

        assert_eq!(report.non_finite_samples, 0, "report={report:?}");
        for offset in 0..6 {
            let first = report.first_samples[1 + offset];
            let second = report.first_samples[14 + offset];
            assert!(
                (first - second).abs() < 0.0001,
                "second note should repeat the full first attack at offset {offset}, report={report:?}",
            );
        }
        assert!(report.first_samples[17] > 0.999, "report={report:?}");
    }

    #[test]
    fn effect_compile_injects_shared_preamble_helpers() {
        let source = r#"
            (def in_l (in 1 @name left))
            (def in_r (in 2 @name right))
            (param cutoff @default 1200 @min 40 @max 12000)
            (param q @default 0.8 @min 0.5 @max 4.0)
            (def filtered_l (svf in_l cutoff q 0))
            (def filtered_r (svf in_r cutoff q 0))
            (out filtered_l 1 @name left)
            (out filtered_r 2 @name right)
        "#;

        let result = super::compile_and_load(source, 44_100)
            .expect("effect compiler should inject shared preamble helpers");
        assert_eq!(result.manifest.n_inputs, 2);
        assert_eq!(result.manifest.n_outputs, 2);
    }

    #[test]
    fn spectral_effect_renders_finite_nonzero_audio_through_host_services() {
        // Stereo adaptation of dgen's toolchain/fixtures/spectral-effect.lisp:
        // partitioned convolution exercises the full DGenHostServicesV1 table
        // (fft_setup_create, forward/inverse FFT, complex MAC) that
        // audiograph/dgen_host_services.c implements over Accelerate.
        let source = r#"
            (def dry_l (in 1 @name Left))
            (def dry_r (in 2 @name Right))
            (def impulse (tensor @shape [32] @data [
              1 0 0 0 0 0 0 0
              0.35 0 0 0 0 0 0 0
              0.15 0 0 0 0 0 0 0
              0.05 0 0 0 0 0 0 0]))
            (out (partitioned-convolve dry_l impulse @N 16 @hop 8 @gain 0.5) 1 @name Left)
            (out (partitioned-convolve dry_r impulse @N 16 @hop 8 @gain 0.5) 2 @name Right)
        "#;

        let render = || {
            super::render_effect_source_for_test(
                source,
                &super::EffectRenderOptions {
                    sample_rate: 48_000,
                    block_size: 128,
                    frames: 4096,
                    param_overrides: Vec::new(),
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: Vec::new(),
                    input_overrides: Vec::new(),
                },
            )
            .expect("spectral effect should compile and render")
        };

        let first = render();
        assert!(first.peak.is_finite(), "peak must be finite");
        assert!(first.rms.is_finite(), "rms must be finite");
        assert!(
            first.nonzero_frames > 0,
            "spectral output must be nonzero (peak={}, rms={})",
            first.peak,
            first.rms
        );

        let second = render();
        assert_eq!(
            first.first_samples, second.first_samples,
            "spectral render must be deterministic across runs"
        );
        assert_eq!(first.peak, second.peak);
        assert_eq!(first.rms, second.rms);
    }

    #[test]
    fn prewarm_gate_follows_generated_code_fft_usage() {
        // The load-time warm-up exists only to create FFT setups off the audio
        // thread, and it costs a full state allocation plus 2048 rendered
        // frames — so it must run for spectral dylibs and not for the rest.
        let spectral = r#"
            (def dry_l (in 1 @name Left))
            (def impulse (tensor @shape [16] @data [1 0 0 0 0 0 0 0 0.25 0 0 0 0 0 0 0]))
            (out (partitioned-convolve dry_l impulse @N 16 @hop 8 @gain 0.5) 1 @name Left)
            (out dry_l 2 @name Right)
        "#;
        let plain = r#"
            (def dry_l (in 1 @name Left))
            (def dry_r (in 2 @name Right))
            (param gain @default 0.5 @min 0.0 @max 1.0)
            (out (* dry_l gain) 1 @name Left)
            (out (* dry_r gain) 2 @name Right)
        "#;

        let spectral = super::compile_and_load(spectral, 48_000)
            .expect("spectral effect should compile and load");
        assert!(
            super::generated_code_uses_host_fft(&spectral.manifest.dylib_path),
            "spectral generated code calls the host FFT hook and must be warmed up"
        );

        let plain =
            super::compile_and_load(plain, 48_000).expect("plain effect should compile and load");
        assert!(
            !super::generated_code_uses_host_fft(&plain.manifest.dylib_path),
            "non-spectral generated code must not pay for the warm-up render"
        );
    }

    #[test]
    fn prewarm_renders_against_seeded_init_state() {
        // Regression: the warm-up used to render against an all-zero span, so
        // any DSP gated behind a param that defaults non-zero never reached its
        // FFT setup (and zero-valued divisors produced NaN indices).
        let source = r#"
            (def dry_l (in 1 @name Left))
            (def dry_r (in 2 @name Right))
            (param gain @default 0.75 @min 0.0 @max 1.0)
            (out (* dry_l gain) 1 @name Left)
            (out (* dry_r gain) 2 @name Right)
        "#;

        let compiled =
            super::compile_and_load(source, 48_000).expect("effect should compile and load");
        let param = compiled
            .manifest
            .params
            .iter()
            .find(|param| param.name == "gain")
            .expect("manifest should expose the gain param");
        assert_eq!(param.default, 0.75);

        let state = super::prewarm_dgen_process(&compiled.manifest, &compiled.lib);
        assert_eq!(
            state[param.cell_id], 0.75,
            "warm-up scratch span must carry the param defaults a live node inits with"
        );
        assert!(
            state.iter().all(|value| value.is_finite()),
            "warm-up must not leave NaN/Inf in the state span"
        );
    }

    #[test]
    fn custom_effect_mod_input_changes_mod_accessor_output_when_active() {
        let source = r#"
            (def input_l (in 1 @name Left))
            (def input_r (in 2 @name Right))
            (def mod1 (in 3 @name mod1 @modulator 1))
            (def mod2 (in 4 @name mod2 @modulator 2))
            (def mod3 (in 5 @name mod3 @modulator 3))
            (def mod4 (in 6 @name mod4 @modulator 4))
            (param xyz @default 0.25 @min 0.0 @max 1.0 @mod true @mod-mode additive)
            (def amount (mod xyz))
            (out (* input_l amount) 1 @name Left)
            (out (* input_r amount) 2 @name Right)
        "#;

        let render = |mod_value: f32| {
            super::render_effect_source_for_test(
                source,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 128,
                    frames: 2048,
                    param_overrides: vec![
                        ("__dgen_mod_active__xyz".to_string(), 1.0),
                        ("mod xyz slot 1 amt".to_string(), 0.5),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: Vec::new(),
                    input_overrides: vec![(2, mod_value)],
                },
            )
            .expect("effect should compile and render")
        };

        let unmodulated = render(0.0);
        let modulated = render(1.0);
        let diff = unmodulated
            .first_samples
            .iter()
            .zip(modulated.first_samples.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);

        assert!(
            diff > 0.01,
            "expected mod1 input to affect (mod xyz), diff={diff}"
        );
    }

    #[test]
    fn unnamed_custom_effect_modulator_inputs_are_manifest_modulators() {
        let source = r#"
            (def input_l (in 1 @name Left))
            (def input_r (in 2 @name Right))
            (def in3 (in 3 @modulator 1))
            (param xyz @default 0.25 @min 0.0 @max 1.0 @mod true @mod-mode additive)
            (out (* input_l (mod xyz)) 1 @name Left)
            (out (* input_r (mod xyz)) 2 @name Right)
        "#;

        let compiled = super::compile_and_load(source, 44_100)
            .expect("effect with unnamed modulator input should compile");

        assert_eq!(compiled.manifest.modulators.len(), 1);
        assert_eq!(compiled.manifest.modulators[0].slot, 1);
        assert_eq!(compiled.manifest.modulators[0].input_channel, 2);
    }

    #[test]
    fn spectral_cumsum_soothe_amount_zero_full_wet_preserves_stereo_energy() {
        let path = crate::app_paths::app_paths().effects_dir().join("spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        for block_size in [256, 512] {
            let report = super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size,
                    frames: 8192,
                    param_overrides: vec![
                        ("amount".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: Vec::new(),
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render");
            println!("spectral-cumsum-soothe amount=0 mix=1 block={block_size} report: {report:?}");

            assert!(
                report.left_rms > 0.01,
                "left channel should not collapse at amount=0/mix=1 block={block_size}, report={report:?}"
            );
            assert!(
                report.right_rms > 0.01,
                "right channel should not collapse at amount=0/mix=1 block={block_size}, report={report:?}"
            );
            let ratio = report.left_rms / report.right_rms.max(1.0e-9);
            assert!(
                (0.25..4.0).contains(&ratio),
                "stereo energy should stay within a plausible range at block={block_size}, ratio={ratio}, report={report:?}"
            );
        }
    }

    #[test]
    fn spectral_cumsum_soothe_is_listed_and_ui_validates() {
        let effect_name = "spectral-cumsum-soothe";
        let listed = super::list_saved_effects();
        assert!(
            listed.iter().any(|name| name == effect_name),
            "effect picker list should include {effect_name:?}; listed={listed:?}"
        );

        let path = crate::app_paths::app_paths().effects_dir().join("spectral-cumsum-soothe/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral cumsum soothe effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let ui_source = std::fs::read_to_string(
            crate::app_paths::app_paths().effects_dir().join("spectral-cumsum-soothe/ui.lisp"),
        )
        .expect("read spectral cumsum soothe ui");
        crate::agent::ui_validate::validate_effect_ui_source(&ui_source, &compiled.manifest)
            .expect("effect ui should validate");
    }

    #[test]
    fn spectral_notch_phaser_is_listed_and_ui_validates() {
        let effect_name = "spectral-notch-phaser";
        let listed = super::list_saved_effects();
        assert!(
            listed.iter().any(|name| name == effect_name),
            "effect picker list should include {effect_name:?}; listed={listed:?}"
        );

        let path = crate::app_paths::app_paths().effects_dir().join("spectral-notch-phaser/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral notch phaser effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let ui_source = std::fs::read_to_string(
            crate::app_paths::app_paths().effects_dir().join("spectral-notch-phaser/ui.lisp"),
        )
        .expect("read spectral notch phaser ui");
        crate::agent::ui_validate::validate_effect_ui_source(&ui_source, &compiled.manifest)
            .expect("effect ui should validate");
    }

    #[test]
    fn spectral_notch_phaser_depth_changes_signal() {
        let path = crate::app_paths::app_paths().effects_dir().join("spectral-notch-phaser/dsp.lisp");
        let source = std::fs::read_to_string(&path).expect("read spectral notch phaser effect");
        let asset_base = path.parent();
        let compiled = super::compile_and_load_with_asset_base(&source, 44_100, asset_base)
            .expect("effect should compile");
        let render = |depth: f32| {
            super::render_loaded_effect_for_test(
                &compiled.manifest,
                &compiled.lib,
                &super::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("depth".to_string(), depth),
                        ("lowkeep".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                        ("output".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: Vec::new(),
                    input_overrides: vec![],
                },
            )
            .expect("effect should compile and render")
        };

        let bypass = render(0.0);
        let active = render(1.0);
        println!("spectral-notch-phaser bypass={bypass:?} active={active:?}");

        assert!(
            active.peak < bypass.peak * 0.9,
            "deep notches should reduce peak level, bypass={bypass:?}, active={active:?}"
        );
        assert!(
            active.left_rms > 0.001 && active.right_rms > 0.001,
            "active processing should not collapse either channel, active={active:?}"
        );
    }

    #[test]
    fn dpro_wave_v2_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-wave-v2/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: Vec::new(),
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn digitone_bellington_high_note_remains_finite() {
        let name = "emulations/digitone/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let compile = super::compile_and_load_instrument_with_asset_base(
            &source,
            44_100,
            asset_base.as_deref(),
        )
        .unwrap();
        let preset = super::load_instrument_presets(name)
            .unwrap()
            .into_iter()
            .find(|preset| preset.name == "bellington")
            .expect("bellington preset should exist");
        let param_overrides = compile
            .manifest
            .params
            .iter()
            .filter_map(|param| {
                preset
                    .params
                    .get(&param.name)
                    .map(|value| (param.name.clone(), value.clamp(param.min, param.max)))
            })
            .collect();
        let report = super::render_loaded_instrument_for_test(
            &compile.manifest,
            &compile.lib,
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 44_100,
                midi_note: 153.0 + preset.base_note_offset,
                velocity: 1.0,
                gate_frames: 44_100,
                voice_index: 0,
                param_overrides,
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(
            report.non_finite_samples, 0,
            "expected finite output samples, got report: {report:?}"
        );
        assert_eq!(
            report.non_finite_state_slots, 0,
            "expected finite instrument state, got report: {report:?}"
        );
        assert!(
            report.peak.is_finite() && report.rms.is_finite() && report.mean_abs.is_finite(),
            "expected finite signal stats, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_ddrw_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-ddrw-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("wav1".to_string(), 4.0),
                    ("wav2".to_string(), 40.0),
                    ("mix".to_string(), 0.5),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_dens_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-dens-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("wave".to_string(), 16.0),
                    ("pch2".to_string(), 4.0),
                    ("pch3".to_string(), 7.0),
                    ("pch4".to_string(), 12.0),
                    ("chrl".to_string(), 0.35),
                    ("chrw".to_string(), 0.4),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn dpro_bbox_v1_renders_audible_signal() {
        let name = "emulations/monomachine-dpro-bbox-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 48.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("ptch".to_string(), 0.0),
                    ("start".to_string(), 0.0),
                    ("rtrg".to_string(), 0.0),
                    ("rtim".to_string(), 72.0),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn fmplus_stat_v1_renders_audible_signal() {
        let name = "emulations/monomachine-fmplus-stat-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("op1_frq".to_string(), 15.0),
                    ("op1_fin".to_string(), 0.0),
                    ("op1_fb".to_string(), 0.18),
                    ("op1_env".to_string(), 0.62),
                    ("op2_frq".to_string(), 19.0),
                    ("op2_vol".to_string(), 0.38),
                    ("tone".to_string(), 0.64),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn fmplus_par_v1_renders_audible_signal() {
        let name = "emulations/monomachine-fmplus-par-v1/";
        let source = super::load_instrument_source(name).unwrap();
        let asset_base = super::instrument_source_path(name)
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
        let report = super::render_instrument_source_for_test(
            &source,
            asset_base.as_deref(),
            &super::InstrumentRenderOptions {
                sample_rate: 44_100,
                block_size: 128,
                frames: 4096,
                midi_note: 69.0,
                velocity: 1.0,
                gate_frames: 4096,
                voice_index: 0,
                param_overrides: vec![
                    ("op1_frq".to_string(), 15.0),
                    ("op1_env".to_string(), 0.55),
                    ("op2_frq".to_string(), 19.0),
                    ("op2_env".to_string(), 0.42),
                    ("op3_frq".to_string(), 23.0),
                    ("op3_env".to_string(), 0.30),
                    ("op1_wave".to_string(), 18.0),
                    ("op1_mix".to_string(), 0.35),
                    ("op2_wave".to_string(), 34.0),
                    ("op2_mix".to_string(), 0.28),
                    ("op3_wave".to_string(), 51.0),
                    ("op3_mix".to_string(), 0.22),
                    ("car_wave".to_string(), 9.0),
                    ("car_mix".to_string(), 0.18),
                    ("tone".to_string(), 0.62),
                ],
                param_events: Vec::new(),
                input_overrides: Vec::new(),
            },
        )
        .unwrap();

        assert!(
            report.peak > 0.01,
            "expected audible peak, got report: {report:?}"
        );
        assert!(
            report.rms > 0.001,
            "expected audible rms, got report: {report:?}"
        );
    }

    #[test]
    fn process_target_parser_accepts_stable_rack_macro_identifier() {
        let target = super::process_list([
            super::EValue::Symbol("rack-macro".to_string()),
            super::EValue::Keyword("macro_1".to_string()),
        ]);
        assert_eq!(
            super::parse_process_target_hint(&target).unwrap(),
            crate::process::ProcessTargetHint::RackMacroParam { macro_id: 0 }
        );
    }
