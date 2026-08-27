    use super::*;
    use crate::effects::{
        EffectDescriptor, EffectSlotSnapshot, HostControl, ParamDescriptor, ParamKind,
        ParamScaling, TensorParamDescriptor, BUILTIN_SLOT_COUNT,
    };
    use crate::neural::ParamNodeId;
    use crate::plock_variants::{PlockVariantEntry, PlockVariantRegistryEntry};
    use crate::process::{ParamTarget, ProcessInstanceId, TrackProcessSlot};
    use crate::sequencer::ModDestination;

    #[test]
    fn record_position_uses_the_track_timebase_not_global_sixteenths() {
        let state = SequencerState::new(1, vec![vec![]]);
        state.pattern.track_params[0].set_timebase(Timebase::Eighth);
        assert_eq!(
            state.record_position_at_beat(0, 0.75),
            Some(RecordPosition {
                step: 1,
                phase: 0.5,
            })
        );
    }

    #[test]
    fn stop_invalidates_the_record_clock_anchor_until_republished() {
        use std::time::{Duration, Instant};
        let state = SequencerState::new(1, vec![vec![]]);
        // First publish initializes the monotonic clock origin; the real
        // anchor sits 1 ms later so its origin-relative timestamp is
        // non-zero (a zero anchor reads as "no anchor yet").
        let now = Instant::now();
        state.transport.record_clock.publish(0.0, now);
        let anchor = now + Duration::from_millis(1);
        state.transport.record_clock.publish(8.0, anchor);
        let press = anchor + Duration::from_millis(2);
        assert!(state.record_beats_at_instant(press).is_some());

        // Stop freezes nothing: the stale anchor must read as absent, so a
        // press racing the next Play's first audio block falls back to the
        // reset playheads instead of the previous run's beat.
        state.stop_playback();
        assert_eq!(state.record_beats_at_instant(press), None);
        assert_eq!(state.record_position_at_instant(0, press), None);

        // The next playing audio block re-arms the clock.
        state.start_playback();
        state.transport.record_clock.publish(0.0, press);
        assert!(state.record_beats_at_instant(press).is_some());
    }

    #[test]
    fn active_notes_merge_scheduled_expirations_with_live_note_state() {
        let state = SequencerState::new(1, vec![vec![]]);
        state.set_audio_rendered_sample(100);
        state.mark_scheduled_note_active_until(0, 60, 200, 0.4);
        state.mark_live_note_trigger(0, 64);
        state.replace_live_notes(0, [(64, 0.7)]);
        assert_eq!(state.active_notes(0), vec![60, 64]);
        assert_eq!(
            state.active_note_activity(0),
            vec![
                ActiveNoteActivity {
                    note: 60,
                    velocity: 0.4,
                    trigger_id: 1,
                },
                ActiveNoteActivity {
                    note: 64,
                    velocity: 0.7,
                    trigger_id: 2,
                },
            ]
        );

        state.set_audio_rendered_sample(200);
        assert_eq!(state.active_notes(0), vec![64]);

        state.mark_scheduled_note_active_until(0, 64, 300, 0.5);
        state.replace_live_notes(0, []);
        assert_eq!(state.active_notes(0), vec![64]);
        assert_eq!(state.active_note_activity(0)[0].trigger_id, 3);

        state.set_audio_rendered_sample(300);
        assert!(state.active_notes(0).is_empty());
    }

    fn sample_track_params(id: usize) -> TrackParamsSnapshot {
        TrackParamsSnapshot {
            gate: id % 2 == 0,
            attack_ms: 10.0 + id as f32,
            release_ms: 20.0 + id as f32,
            swing: 0.1 * id as f32,
            swing_resolution: SwingResolution::Quarter,
            num_steps: 8 + id,
            volume: 0.2 * id as f32,
            pan: -1.0 + id as f32,
            mute: id % 2 == 0,
            send: 0.3 * id as f32,
            output: crate::sequencer::TrackOutput::Mix,
            sends: vec![crate::sequencer::TrackSendSnapshot {
                destination: crate::sequencer::BusId::DEFAULT_A,
                amount: 0.1 * id as f32,
            }],
            polyphonic: id % 2 == 1,
            max_polyphony: 1 + (id % 12),
            timebase: Timebase::Quarter,
            accumulator_idx: id,
            script_accumulator_name: Some(format!("acc-{id}")),
            midi_fx_chain: vec![format!("fx-{id}")],
            midi_fx_position: crate::sequencer::MidiFxPosition::PostAccumulator,
            accum_limit: (1 + id) as f32,
            accum_mode: id as u32,
            fts_scale: id + 1,
            mute_group: (id % 9) as u8,
            global_transpose: id % 2 == 0,
        }
    }

    fn sample_effect_slot_snapshot(id: usize) -> EffectSlotSnapshot {
        EffectSlotSnapshot {
            node_id: 100 + id as u32,
            modulator_node_id: 0,
            num_params: 2,
            defaults: vec![id as f32, id as f32 + 0.5],
            plocks: (0..MAX_STEPS)
                .map(|step| {
                    if step == id {
                        vec![Some(id as f32), None]
                    } else {
                        vec![None, None]
                    }
                })
                .collect(),
            plock_param_ids: vec![vec![None, None]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![id as u32, id as u32 + 10],
            param_node_spans: vec![1, 1],
            transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
            tensor_params: Vec::new(),
            ir: None,
            table: None,
            sampler_slice_edits: None,
        }
    }

    fn instrument_swap_process_chain() -> crate::process::TrackProcessChain {
        crate::process::TrackProcessChain {
            slots: vec![TrackProcessSlot {
                instance_id: ProcessInstanceId(1),
                instance_name: Some("swap-test".to_string()),
                class_name: "swap-test".to_string(),
                enabled: true,
                project_layer: false,
                inlets: Default::default(),
                lanes: Default::default(),
                bindings: std::collections::BTreeMap::from([
                    (
                        "survives".to_string(),
                        Some(ParamTarget::EffectParam {
                            slot: 0,
                            effect: "filter".to_string(),
                            param: "cutoff".to_string(),
                            param_id: Some(ParamNodeId {
                                logical_id: 55,
                                node_param_idx: 2,
                            }),
                        }),
                    ),
                    (
                        "resolves".to_string(),
                        Some(ParamTarget::InstrumentParam {
                            param: "cutoff".to_string(),
                            param_id: Some(ParamNodeId {
                                logical_id: 11,
                                node_param_idx: 7,
                            }),
                        }),
                    ),
                    (
                        "drops".to_string(),
                        Some(ParamTarget::InstrumentParam {
                            param: "removed-param".to_string(),
                            param_id: Some(ParamNodeId {
                                logical_id: 11,
                                node_param_idx: 8,
                            }),
                        }),
                    ),
                ]),
            }],
        }
    }

    fn composite_instrument_effect_variant_registry() -> PlockVariantRegistry {
        let key = PlockVariantKey::new(vec![
            PlockVariantEntry {
                domain: PlockVariantDomain::Instrument,
                slot: 0,
                param: 0,
                cell: None,
                value_bits: 0.2f32.to_bits(),
            },
            PlockVariantEntry {
                domain: PlockVariantDomain::Effect,
                slot: 0,
                param: 0,
                cell: None,
                value_bits: 0.8f32.to_bits(),
            },
        ])
        .unwrap();
        PlockVariantRegistry {
            entries: vec![PlockVariantRegistryEntry {
                key: key.clone(),
                label: "A".to_string(),
                color: [0.1, 0.2, 0.3],
                name: Some("effect identity".to_string()),
                color_index: 0,
            }],
            previous_step_keys: vec![Some(key)],
        }
    }

    #[test]
    fn track_mute_group_defaults_and_clamps() {
        let params = TrackParams::new();
        assert_eq!(params.get_mute_group(), 0);

        params.set_mute_group(4);
        assert_eq!(params.get_mute_group(), 4);

        params.set_mute_group(42);
        assert_eq!(params.get_mute_group(), 8);
    }

    #[test]
    fn key_lock_variant_stamp_copies_key_lock_signature_to_selected_notes() {
        let desc = EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let mode_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mode")
            .expect("filter descriptor should include mode");
        let state = SequencerState::new(1, vec![]);
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 100);
        state.pattern.instrument_slots[0].set_key_lock(60, cutoff_idx, 900.0);
        state.pattern.instrument_slots[0].set_key_lock(60, mode_idx, 2.0);

        let assignments = state.reconcile_key_lock_variant_registry_for_track(0);
        let assignment = assignments[60]
            .clone()
            .expect("source note should be assigned to a key-lock variant");
        assert_eq!(assignment.label, "A");
        assert_eq!(assignment.param_count, 2);

        assert!(state.stamp_key_lock_variant_key_to_notes(0, &assignment.key, &[62, 64]));
        let slot = &state.pattern.instrument_slots[0];
        assert_eq!(slot.key_locks.get(62, cutoff_idx), Some(900.0));
        assert_eq!(slot.key_locks.get(62, mode_idx), Some(2.0));
        assert_eq!(slot.key_locks.get(64, cutoff_idx), Some(900.0));
        assert_eq!(slot.key_locks.get(64, mode_idx), Some(2.0));

        let assignments = state.reconcile_key_lock_variant_registry_for_track(0);
        assert_eq!(
            assignments[60].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );
        assert_eq!(
            assignments[62].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );
        assert_eq!(
            assignments[64].as_ref().map(|item| item.label.as_str()),
            Some("A")
        );

        assert!(state.clear_key_lock_variant_locks_for_notes(0, &[62]));
        assert_eq!(slot.key_locks.get(62, cutoff_idx), None);
        assert_eq!(slot.key_locks.get(62, mode_idx), None);
        assert_eq!(slot.key_locks.get(64, cutoff_idx), Some(900.0));
    }

    #[test]
    fn step_variant_stamp_and_clear_preserve_step_expression() {
        let state = make_state_with_tracks(1);
        state.pattern.timebase_plocks[0].set(2, Timebase::Eighth);
        state.pattern.swing_plocks[0].set(2, 63.0);
        state.pattern.swing_resolution_plocks[0].set(2, SwingResolution::Eighth);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.42);
        state.pattern.step_data[0].set(5, StepParam::Velocity, 0.77);

        let assignment = state.reconcile_plock_variant_registry_for_track(0)[2]
            .clone()
            .expect("track locks should produce a variant");
        assert_eq!(assignment.param_count, 3);
        assert!(state.stamp_variant_key_to_steps(0, &assignment.key, &[5]));
        assert_eq!(
            state.pattern.timebase_plocks[0].get(5),
            Some(Timebase::Eighth)
        );
        assert_eq!(state.pattern.swing_plocks[0].get(5), Some(63.0));
        assert_eq!(
            state.pattern.swing_resolution_plocks[0].get(5),
            Some(SwingResolution::Eighth)
        );
        assert_eq!(state.pattern.step_data[0].get(5, StepParam::Velocity), 0.77);

        assert!(state.clear_variant_locks_for_steps(0, &[5]));
        assert_eq!(state.pattern.timebase_plocks[0].get(5), None);
        assert_eq!(state.pattern.swing_plocks[0].get(5), None);
        assert_eq!(state.pattern.swing_resolution_plocks[0].get(5), None);
        assert_eq!(state.pattern.step_data[0].get(5, StepParam::Velocity), 0.77);
    }

    fn sample_pattern_snapshot(num_tracks: usize) -> PatternSnapshot {
        PatternSnapshot {
            track_bits: (0..num_tracks)
                .map(|track| {
                    let mut bits = [0u64; TRACK_PATTERN_WORDS];
                    bits[0] = (track as u64) + 1;
                    bits
                })
                .collect(),
            neural_reset_bits: vec![[0u64; TRACK_PATTERN_WORDS]; num_tracks],
            step_data: (0..num_tracks)
                .map(|track| {
                    let mut steps = vec![[0.0; NUM_PARAMS]; MAX_STEPS];
                    steps[0][0] = track as f32 + 0.25;
                    steps[1][1] = track as f32 + 0.5;
                    steps
                })
                .collect(),
            track_params: (0..num_tracks).map(sample_track_params).collect(),
            effect_slots: (0..num_tracks)
                .map(|track| vec![sample_effect_slot_snapshot(track)])
                .collect(),
            midi_fx_slots: (0..num_tracks)
                .map(|_| vec![EffectSlotSnapshot::new_empty(); crate::lisp_host::MAX_MIDI_FX_SLOTS])
                .collect(),
            instrument_slots: (0..num_tracks)
                .map(|track| sample_effect_slot_snapshot(track + 10))
                .collect(),
            instrument_base_note_offsets: (0..num_tracks)
                .map(|track| track as f32 - 12.0)
                .collect(),
            instrument_run_modes: (0..num_tracks)
                .map(|track| {
                    if track % 2 == 0 {
                        CustomInstrumentRunMode::Instrument
                    } else {
                        CustomInstrumentRunMode::FreePatch
                    }
                })
                .collect(),
            track_sound_states: (0..num_tracks)
                .map(|track| TrackSoundState {
                    loaded_preset: Some(format!("preset-{track}")),
                    dirty: track % 2 == 0,
                    engine_id: Some(track),
                })
                .collect(),
            sample_ids: (0..num_tracks)
                .map(|track| (track as i32, format!("track-{track}"), 44_100))
                .collect(),
            chord_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut chord = ChordSnapshot::new_default();
                    chord.steps[0] = vec![track as f32, track as f32 + 7.0];
                    chord
                })
                .collect(),
            timebase_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[0] = Some(track as u32);
                    arr
                })
                .collect(),
            swing_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[1] = Some((track as u32) + 10);
                    arr
                })
                .collect(),
            swing_resolution_plock_snapshots: (0..num_tracks)
                .map(|track| {
                    let mut arr = [None; MAX_STEPS];
                    arr[2] = Some((track as u32) + 20);
                    arr
                })
                .collect(),
            track_send_plock_snapshots: vec![vec![Vec::new(); MAX_STEPS]; num_tracks],
            instrument_types: (0..num_tracks)
                .map(|track| {
                    if track % 2 == 0 {
                        InstrumentType::Sampler
                    } else {
                        InstrumentType::Custom
                    }
                })
                .collect(),
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: vec![None; num_tracks],
            process_chains: vec![crate::process::TrackProcessChain::default(); num_tracks],
            project_process_lane_overrides: vec![Default::default(); num_tracks],
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: vec![PlockVariantRegistry::default(); num_tracks],
            key_lock_variant_registries: vec![PlockVariantRegistry::default(); num_tracks],
        }
    }

    #[test]
    fn group_flat_track_to_rack_migrates_every_pattern_without_rekeying_scene_mappings() {
        let mut first = sample_pattern_snapshot(1);
        first.instrument_types[0] = InstrumentType::Custom;
        first.instrument_slots[0].defaults[0] = 0.11;
        first.effect_slots[0][0].defaults[0] = 0.21;
        let mut second = first.clone();
        second.instrument_slots[0].defaults[0] = 0.32;
        second.effect_slots[0][0].defaults[0] = 0.42;

        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(vec![first, second], 0);
        let (scene_pattern_ids_before, override_pattern_id, launched) = {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            let override_id = scenes.fork_track_pattern(0).expect("track pattern fork");
            assert!(scenes.track_pools[0].edit(override_id, |data| {
                data.instrument_slot.defaults[0] = 0.53;
                data.effect_slots[0].defaults[0] = 0.63;
            }));
            let launched = scenes.track_pools[0]
                .get(override_id)
                .expect("forked pattern");
            (
                scenes
                    .scenes
                    .iter()
                    .map(|scene| scene.cells[0])
                    .collect::<Vec<_>>(),
                override_id,
                launched,
            )
        };
        launched.restore_to(&state, 0);

        state
            .group_flat_track_to_rack(
                0,
                InstrumentType::Custom,
                CustomInstrumentRunMode::Instrument,
                Some(7),
                &[EffectDescriptor::builtin_filter()],
                &[Some("filter".to_string())],
            )
            .expect("flat track should migrate");

        let scenes = state.pattern.scenes.lock().unwrap();
        assert_eq!(
            scenes
                .scenes
                .iter()
                .map(|scene| scene.cells[0])
                .collect::<Vec<_>>(),
            scene_pattern_ids_before,
            "scene cells must retain their stable pattern ids"
        );
        assert_eq!(scenes.track_overrides[0], Some(override_pattern_id));
        // 2 scene cells + 1 override fork + the track-sound carrier
        // (track-sound spec §2.1).
        assert_eq!(scenes.track_pools[0].patterns.len(), 4);

        let expected = [
            (scene_pattern_ids_before[0].unwrap(), 0.11, 0.21),
            (scene_pattern_ids_before[1].unwrap(), 0.32, 0.42),
            (override_pattern_id, 0.53, 0.63),
        ];
        for (pattern_id, instrument_default, effect_default) in expected {
            let pattern = scenes.track_pools[0]
                .get(pattern_id)
                .expect("migrated pattern");
            assert_eq!(pattern.instrument_type, InstrumentType::Rack);
            assert_eq!(pattern.instrument_slot.node_id, 0);
            assert!(pattern.effect_slots.iter().all(|slot| slot.node_id == 0));
            let slot = &pattern.rack_track.as_ref().expect("rack pattern").slots[0];
            assert_eq!(slot.instrument_slot.defaults[0], instrument_default);
            assert_eq!(slot.effect_slots[0].defaults[0], effect_default);
            assert_eq!(slot.track_sound_state.engine_id, Some(7));
        }
    }

    #[test]
    fn instrument_swap_reset_clears_only_instrument_state_in_every_pattern() {
        let descriptor = EffectDescriptor::builtin_filter();
        let mut snapshots = (0..3)
            .map(|scene| {
                let mut snapshot = sample_pattern_snapshot(1);
                snapshot.track_bits[0][0] = 10 + scene as u64;
                snapshot.step_data[0][0][0] = 0.25 + scene as f32;
                snapshot.track_params[0].volume = 0.4 + scene as f32 * 0.1;
                snapshot.effect_slots[0][0].plocks[scene][0] = Some(100.0 + scene as f32);
                snapshot.instrument_slots[0].plocks[scene][0] = Some(0.1 + scene as f32);
                snapshot.instrument_slots[0]
                    .key_locks
                    .insert(60 + scene as u8, vec![Some(0.5), None]);
                snapshot.instrument_base_note_offsets[0] = -7.0;
                snapshot.instrument_types[0] = InstrumentType::Custom;
                snapshot.instrument_run_modes[0] = CustomInstrumentRunMode::FreePatch;
                snapshot.track_sound_states[0] = TrackSoundState {
                    engine_id: Some(2),
                    loaded_preset: Some(format!("preset-{scene}")),
                    dirty: true,
                };
                let param_id = crate::neural::ParamNodeId {
                    logical_id: 100 + scene as u64,
                    node_param_idx: 0,
                };
                let mut network = ProjectNeuralNetwork::default();
                network.neurons[0].output_overrides.instrument.push(
                    crate::neural::ProjectParamOverride {
                        target_track: 0,
                        param_id,
                        param_index: 0,
                        value: 0.25,
                    },
                );
                network.neurons[0].output_overrides.effects.push(
                    crate::neural::ProjectEffectParamOverride {
                        target_track: 0,
                        slot_index: 0,
                        param_id,
                        param_index: 0,
                        value: 0.75,
                    },
                );
                snapshot.neural_networks = vec![network];
                snapshot.process_chains[0] = instrument_swap_process_chain();
                snapshot.plock_variant_registries[0] =
                    composite_instrument_effect_variant_registry();
                snapshot.key_lock_variant_registries[0] = PlockVariantRegistry {
                    entries: vec![PlockVariantRegistryEntry {
                        key: PlockVariantKey::new(vec![PlockVariantEntry {
                            domain: PlockVariantDomain::InstrumentKeyLock,
                            slot: 0,
                            param: 0,
                            cell: None,
                            value_bits: 0.5f32.to_bits(),
                        }])
                        .unwrap(),
                        label: "K".to_string(),
                        color: [0.4, 0.5, 0.6],
                        name: None,
                        color_index: 1,
                    }],
                    previous_step_keys: Vec::new(),
                };
                snapshot
            })
            .collect::<Vec<_>>();
        let preserved = snapshots.clone();
        let state = SequencerState::new(1, vec![vec![EffectSlotState::new(&descriptor, 90)]]);
        state.replace_pattern_repository(std::mem::take(&mut snapshots), 0);
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 11);
        state.pattern.instrument_slots[0].set_plock(0, 0, 0.9);
        state.pattern.instrument_slots[0].set_key_lock(60, 0, 0.6);
        state.pattern.instrument_base_note_offsets[0].store((-7.0f32).to_bits(), Ordering::Relaxed);
        state.pattern.process_chains.lock().unwrap()[0] = instrument_swap_process_chain();
        state.pattern.plock_variant_registries.lock().unwrap()[0] =
            composite_instrument_effect_variant_registry();

        let summary = state
            .reset_instrument_slot_all_patterns(
                0,
                &descriptor,
                700,
                701,
                9,
                CustomInstrumentRunMode::Instrument,
            )
            .expect("valid custom track should reset");

        // 3 patterns + the track-sound carrier (it swaps with the track).
        assert_eq!(summary.patterns_reset, 4);
        assert_eq!(summary.patterns_with_cleared_locks, 4);
        assert_eq!(summary.neural_overrides_dropped, 3);
        assert_eq!(
            f32::from_bits(state.pattern.instrument_base_note_offsets[0].load(Ordering::Relaxed)),
            -7.0
        );
        assert!(
            !EffectSlotSnapshot::capture(&state.pattern.instrument_slots[0])
                .plocks
                .iter()
                .flatten()
                .any(Option::is_some)
        );
        assert!(
            EffectSlotSnapshot::capture(&state.pattern.instrument_slots[0])
                .key_locks
                .is_empty()
        );

        let reset = state.export_pattern_repository();
        for (scene, (actual, before)) in reset.iter().zip(preserved.iter()).enumerate() {
            assert_eq!(actual.track_bits[0], before.track_bits[0]);
            assert_eq!(actual.step_data[0], before.step_data[0]);
            assert_eq!(actual.track_params[0].volume, before.track_params[0].volume);
            assert_eq!(actual.track_params[0].pan, before.track_params[0].pan);
            assert_eq!(actual.track_params[0].sends, before.track_params[0].sends);
            assert_eq!(
                actual.effect_slots[0][0].plocks,
                before.effect_slots[0][0].plocks
            );
            assert_eq!(
                actual.instrument_base_note_offsets[0],
                before.instrument_base_note_offsets[0]
            );

            let slot = &actual.instrument_slots[0];
            assert_eq!(slot.node_id, 700, "scene {scene}");
            assert_eq!(slot.modulator_node_id, 701, "scene {scene}");
            assert_eq!(
                slot.defaults,
                descriptor
                    .params
                    .iter()
                    .map(|param| param.default)
                    .collect::<Vec<_>>()
            );
            assert!(slot.plocks.iter().flatten().all(Option::is_none));
            assert!(slot.key_locks.is_empty());
            assert!(slot
                .tensor_params
                .iter()
                .all(|tensor| tensor.plocks.is_empty()));
            assert_eq!(actual.instrument_types[0], InstrumentType::Custom);
            assert_eq!(actual.sample_ids[0], (-1, String::new(), 44_100));
            assert_eq!(
                actual.instrument_run_modes[0],
                CustomInstrumentRunMode::Instrument
            );
            assert_eq!(actual.track_sound_states[0].engine_id, Some(9));
            assert_eq!(actual.track_sound_states[0].loaded_preset, None);
            assert!(!actual.track_sound_states[0].dirty);

            let variants = &actual.plock_variant_registries[0];
            assert_eq!(variants.entries.len(), 1);
            assert_eq!(variants.entries[0].name.as_deref(), Some("effect identity"));
            assert!(variants.entries[0]
                .key
                .entries
                .iter()
                .all(|entry| entry.domain == PlockVariantDomain::Effect));
            assert!(actual.key_lock_variant_registries[0].entries.is_empty());

            let bindings = &actual.process_chains[0].slots[0].bindings;
            assert!(bindings["drops"].is_none());
            let Some(ParamTarget::InstrumentParam { param_id, .. }) = &bindings["resolves"] else {
                panic!("surviving instrument binding should retain its kind");
            };
            assert_eq!(param_id.unwrap().logical_id, 700);
            assert!(matches!(
                bindings["survives"],
                Some(ParamTarget::EffectParam { .. })
            ));
            assert!(actual.neural_networks[0].neurons[0]
                .output_overrides
                .instrument
                .is_empty());
            assert_eq!(
                actual.neural_networks[0].neurons[0]
                    .output_overrides
                    .effects
                    .len(),
                1,
                "effect neural overrides should survive scene {scene}"
            );
        }
    }

    #[test]
    fn sampler_conversion_sets_sample_identity_and_type_in_every_pattern() {
        let descriptor = EffectDescriptor::builtin_sampler();
        let snapshots = (0..3)
            .map(|scene| {
                let mut snapshot = sample_pattern_snapshot(1);
                snapshot.track_bits[0][0] = 10 + scene;
                snapshot.instrument_types[0] = InstrumentType::Custom;
                snapshot.instrument_run_modes[0] = CustomInstrumentRunMode::FreePatch;
                snapshot.track_sound_states[0] = TrackSoundState {
                    engine_id: Some(4),
                    loaded_preset: Some(format!("preset-{scene}")),
                    dirty: true,
                };
                snapshot.rack_tracks[0] = Some(sample_rack_track_snapshot());
                snapshot
            })
            .collect::<Vec<_>>();
        let state = SequencerState::new(1, vec![vec![EffectSlotState::new(&descriptor, 90)]]);
        state.replace_pattern_repository(snapshots, 0);
        state.pattern.rack_tracks.lock().unwrap()[0] = Some(sample_rack_track_snapshot());

        let summary = state
            .reset_sampler_slot_all_patterns(
                0,
                &descriptor,
                800,
                801,
                (42, "snare".to_string(), 48_000),
            )
            .expect("valid track should convert to sampler");

        // 3 patterns + the track-sound carrier (it converts with the track).
        assert_eq!(summary.patterns_reset, 4);
        assert_eq!(
            state.runtime.instrument_type_flags[0].load(Ordering::Acquire),
            InstrumentType::Sampler.runtime_flag()
        );
        assert_eq!(
            state.runtime.track_engine_ids[0].load(Ordering::Acquire),
            u32::MAX
        );
        assert!(state.pattern.rack_tracks.lock().unwrap()[0].is_none());
        for (scene, snapshot) in state.export_pattern_repository().iter().enumerate() {
            assert_eq!(snapshot.track_bits[0][0], 10 + scene as u64);
            assert_eq!(snapshot.instrument_types[0], InstrumentType::Sampler);
            assert_eq!(
                snapshot.instrument_run_modes[0],
                CustomInstrumentRunMode::Instrument
            );
            assert_eq!(snapshot.sample_ids[0], (42, "snare".to_string(), 48_000));
            assert_eq!(snapshot.track_sound_states[0].engine_id, None);
            assert_eq!(snapshot.track_sound_states[0].loaded_preset, None);
            assert!(!snapshot.track_sound_states[0].dirty);
            assert!(snapshot.rack_tracks[0].is_none());
            assert_eq!(snapshot.instrument_slots[0].node_id, 800);
            assert_eq!(snapshot.instrument_slots[0].modulator_node_id, 801);
        }
    }

    #[test]
    fn seeding_unset_sample_ids_fills_only_patterns_without_a_real_sample() {
        let snapshots = (0..3)
            .map(|scene| {
                let mut snapshot = sample_pattern_snapshot(1);
                snapshot.track_bits[0][0] = 10 + scene;
                snapshot.sample_ids[0] = if scene == 1 {
                    (7, "kick".to_string(), 44_100)
                } else {
                    (-1, String::new(), 44_100)
                };
                snapshot
            })
            .collect::<Vec<_>>();
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(snapshots, 0);

        let seeded =
            state.seed_unset_pattern_sample_ids(0, (42, "eight-oh-eight".to_string(), 48_000));

        // 2 unset scene patterns + the track-sound carrier (unset sample).
        assert_eq!(seeded, 3);
        let exported = state.export_pattern_repository();
        assert_eq!(
            exported[0].sample_ids[0],
            (42, "eight-oh-eight".to_string(), 48_000)
        );
        assert_eq!(exported[1].sample_ids[0], (7, "kick".to_string(), 44_100));
        assert_eq!(
            exported[2].sample_ids[0],
            (42, "eight-oh-eight".to_string(), 48_000)
        );

        assert_eq!(
            state.seed_unset_pattern_sample_ids(0, (-1, String::new(), 44_100)),
            0
        );
    }

    fn sample_rack_track_snapshot() -> RackTrackSnapshot {
        RackTrackSnapshot::new(
            vec![RackSlotSnapshot {
                instrument_type: InstrumentType::Custom,
                instrument_run_mode: CustomInstrumentRunMode::Instrument,
                instrument_base_note_offset: 7.0,
                choke_group: None,
                gain: 0.8,
                pan: -0.2,
                mute: false,
                solo: true,
                max_polyphony: 3,
                param_plocks: RackSlotParamPlocks::new(),
                instrument_slot: sample_effect_slot_snapshot(77),
                effect_slots: RackSlotSnapshot::empty_effect_slots(),
                effect_descriptors: EffectDescriptor::default_full_chain(),
                custom_effect_names: RackSlotSnapshot::empty_effect_names(),
                track_sound_state: TrackSoundState {
                    loaded_preset: Some("rack-lead".to_string()),
                    dirty: true,
                    engine_id: Some(12),
                },
                sample_id: None,
            }],
            default_rack_macros(),
        )
    }

    #[test]
    fn pattern_restore_keeps_live_rack_macro_names() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(vec![sample_pattern_snapshot(1)], 0);
        let mut live_rack = sample_rack_track_snapshot();
        live_rack.macros[0].name = "Release".to_string();
        state.pattern.rack_tracks.lock().unwrap()[0] = Some(live_rack);

        let launched = {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            let id = scenes.fork_track_pattern(0).expect("track pattern fork");
            let mut stored_rack = sample_rack_track_snapshot();
            stored_rack.macros[0].value = 0.75;
            assert!(scenes.track_pools[0].edit(id, |data| {
                data.rack_track = Some(stored_rack.clone());
            }));
            scenes.track_pools[0].get(id).expect("forked pattern")
        };
        launched.restore_to(&state, 0);

        let racks = state.pattern.rack_tracks.lock().unwrap();
        let rack = racks[0].as_ref().expect("rack restored");
        assert_eq!(
            rack.macros[0].name, "Release",
            "live macro renames must survive scene restore"
        );
        assert_eq!(
            rack.macros[0].value, 0.75,
            "macro values still restore from the stored patch"
        );
    }

    fn sample_sampler_rack_slot(
        buffer_id: i32,
        sample_name: &str,
        sample_rate: u32,
        instrument_slot_id: usize,
    ) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: -12.0,
            choke_group: None,
            gain: 0.5,
            pan: 0.25,
            mute: false,
            solo: false,
            max_polyphony: 2,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: sample_effect_slot_snapshot(instrument_slot_id),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((buffer_id, sample_name.to_string(), sample_rate)),
        }
    }

    fn sample_sidechain_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            name: "duck".to_string(),
            params: vec![ParamDescriptor {
                name: "sidechain".to_string(),
                min: 0.0,
                max: 3.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec![
                        "off".to_string(),
                        "track-a".to_string(),
                        "track-b".to_string(),
                        "track-c".to_string(),
                    ],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: u32::MAX,
                node_param_span: 1,
                host_control: Some(HostControl::FxSidechain { input_channel: 0 }),
                ui_metadata: None,
            }],
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        }
    }

    fn step_tensor_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            name: "step tensor fixture".to_string(),
            params: vec![ParamDescriptor {
                name: "amount".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: 0,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }],
            input_channels: 2,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "matrix".to_string(),
                shape: vec![2],
                cell_offset: 1,
                default: vec![0.0, 0.0],
                min: 0.0,
                max: 1.0,
            }],
        }
    }

    #[test]
    fn step_snapshot_clear_and_restore_cover_every_live_lock_domain() {
        let descriptor = step_tensor_descriptor();
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(&descriptor, 10)]],
        );
        let step = 7;
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&descriptor, 20);
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 30);
        state.pattern.midi_fx_slots[0][0].set_plock(step, 0, 0.11);
        state.pattern.midi_fx_slots[0][0]
            .tensor_params
            .set_plock(step, 0, &[0.12, 0.13]);
        state.pattern.effect_chains[0][0].set_plock(step, 0, 0.21);
        state.pattern.effect_chains[0][0]
            .tensor_params
            .set_plock(step, 0, &[0.22, 0.23]);
        state.pattern.instrument_slots[0].set_plock(step, 0, 0.31);
        state.pattern.instrument_slots[0]
            .tensor_params
            .set_plock(step, 0, &[0.32, 0.33]);

        let mut rack = sample_rack_track_snapshot();
        rack.macros[0].plocks[step] = Some(0.41);
        rack.slots[0]
            .param_plocks
            .set(step, RackSlotParam::Gain, 0.42);
        rack.slots[0].instrument_slot = EffectSlotSnapshot::new_default(&descriptor, 40);
        rack.slots[0].instrument_slot.set_plock(step, 0, 0.43);
        rack.slots[0]
            .instrument_slot
            .set_tensor_plock(step, 0, vec![0.44, 0.45]);
        rack.slots[0].effect_slots[0] = EffectSlotSnapshot::new_default(&descriptor, 50);
        rack.slots[0].effect_slots[0].set_plock(step, 0, 0.46);
        rack.slots[0].effect_slots[0]
            .set_tensor_plock(step, 0, vec![0.47, 0.48]);
        state.set_rack_track_for_all_pattern_snapshots(0, rack);

        let captured = state.capture_step_snapshot(0, step);
        assert_eq!(captured.midi_fx_plocks[0].params[0], Some(0.11));
        assert_eq!(captured.midi_fx_plocks[0].tensor_params[0], Some(vec![0.12, 0.13]));
        assert_eq!(captured.effect_plocks[0].tensor_params[0], Some(vec![0.22, 0.23]));
        assert_eq!(captured.instrument_plocks.tensor_params[0], Some(vec![0.32, 0.33]));
        assert_eq!(captured.rack_macro_plocks[0], Some(0.41));
        assert_eq!(captured.rack_slot_instrument_plocks[0].tensor_params[0], Some(vec![0.44, 0.45]));
        assert_eq!(captured.rack_slot_effect_plocks[0][0].tensor_params[0], Some(vec![0.47, 0.48]));

        state.clear_step_payload(0, step);
        let cleared = state.capture_step_snapshot(0, step);
        assert!(cleared.midi_fx_plocks[0].params.iter().all(Option::is_none));
        assert!(cleared.midi_fx_plocks[0].tensor_params.iter().all(Option::is_none));
        assert!(cleared.effect_plocks[0].tensor_params.iter().all(Option::is_none));
        assert!(cleared.instrument_plocks.tensor_params.iter().all(Option::is_none));
        assert!(cleared.rack_macro_plocks.iter().all(Option::is_none));
        assert!(cleared.rack_slot_instrument_plocks[0].tensor_params.iter().all(Option::is_none));
        assert!(cleared.rack_slot_effect_plocks[0][0].tensor_params.iter().all(Option::is_none));

        state.restore_step_snapshot(0, step, &captured);
        let restored = state.capture_step_snapshot(0, step);
        assert_eq!(restored.midi_fx_plocks[0].tensor_params[0], Some(vec![0.12, 0.13]));
        assert_eq!(restored.effect_plocks[0].tensor_params[0], Some(vec![0.22, 0.23]));
        assert_eq!(restored.instrument_plocks.tensor_params[0], Some(vec![0.32, 0.33]));
        assert_eq!(restored.rack_macro_plocks[0], Some(0.41));
        assert_eq!(restored.rack_slot_instrument_plocks[0].tensor_params[0], Some(vec![0.44, 0.45]));
        assert_eq!(restored.rack_slot_effect_plocks[0][0].tensor_params[0], Some(vec![0.47, 0.48]));

        state.pattern.patterns[0].set_step_active(step, true);
        state.pattern.step_data[0].set(step, StepParam::Velocity, 0.73);
        let velocity_bits = state.pattern.step_data[0]
            .get(step, StepParam::Velocity)
            .to_bits();
        state.toggle_step_and_clear_plocks_no_publish(0, step);
        let toggled_off = state.capture_step_snapshot(0, step);
        assert!(!toggled_off.active);
        assert_eq!(toggled_off.params[StepParam::Velocity.index()].to_bits(), velocity_bits);
        assert!(toggled_off.midi_fx_plocks[0].tensor_params.iter().all(Option::is_none));
        assert!(toggled_off.effect_plocks[0].tensor_params.iter().all(Option::is_none));
        assert!(toggled_off.instrument_plocks.tensor_params.iter().all(Option::is_none));
        assert!(toggled_off.rack_slot_effect_plocks[0][0]
            .tensor_params
            .iter()
            .all(Option::is_none));
    }

    #[test]
    fn pattern_repository_ownership_stays_inside_state_module() {
        fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read source dir") {
                let entry = entry.expect("read source entry");
                let path = entry.path();
                if path.is_dir() {
                    collect_rs_files(&path, out);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source_dir = manifest_dir.join("src");
        let state_dir = source_dir.join("sequencer").join("state");
        let mut files = Vec::new();
        collect_rs_files(&source_dir, &mut files);

        let forbidden = [
            ".pattern.pattern_bank",
            ".pattern.current_pattern",
            ".pattern.num_patterns",
            ".bus_pattern_bank",
            ".edit_pattern_repository",
            ".edit_non_current_pattern_snapshots",
            ".edit_all_pattern_snapshots",
        ];
        let mut violations = Vec::new();
        for file in files {
            // SequencerState's facade and repository implementation now live
            // across this directory; the ownership boundary is the state
            // module, not one monolithic state.rs file.
            if file.starts_with(&state_dir) {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("read Rust source");
            let normalized: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
            for pattern in forbidden {
                if normalized.contains(pattern) {
                    violations.push(format!(
                        "{} contains direct {} access",
                        file.strip_prefix(&manifest_dir).unwrap_or(&file).display(),
                        pattern
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "pattern repository access must go through SequencerState facade:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn repository_effect_slot_insert_applies_to_other_patterns_for_one_track_only() {
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        let descriptor_lane =
            vec![EffectDescriptor::empty_custom_slot(); crate::lisp_host::MAX_CUSTOM_FX];
        let slot_descriptors = vec![descriptor_lane.clone(), descriptor_lane];
        let mut current = PatternSnapshot::new_default(2, &slot_descriptors);
        let mut other = PatternSnapshot::new_default(2, &slot_descriptors);
        current.effect_slots[0][BUILTIN_SLOT_COUNT].node_id = 7;
        other.effect_slots[0][BUILTIN_SLOT_COUNT].node_id = 42;
        other.effect_slots[0][BUILTIN_SLOT_COUNT + 1].node_id = 43;
        other.effect_slots[1][BUILTIN_SLOT_COUNT].node_id = 99;
        state.replace_pattern_repository(vec![current, other], 0);

        state.insert_effect_slot_in_other_track_patterns(0, BUILTIN_SLOT_COUNT);

        let after = state.export_pattern_repository();
        assert_eq!(after[0].effect_slots[0][BUILTIN_SLOT_COUNT].node_id, 7);
        assert_eq!(after[1].effect_slots[1][BUILTIN_SLOT_COUNT].node_id, 99);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT].node_id, 0);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT].num_params, 0);
        assert_eq!(after[1].effect_slots[0][BUILTIN_SLOT_COUNT + 1].node_id, 42);
    }

    #[test]
    fn topology_edit_preserves_shared_track_pattern_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let shared = state.scene_track_pattern_id(0, 0).unwrap();
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            assert!(scenes.set_cell(1, 0, shared));
        }

        state.insert_effect_slot_in_other_track_patterns(0, BUILTIN_SLOT_COUNT);

        let scenes = state.pattern.scenes.lock().unwrap();
        assert_eq!(scenes.scenes[0].cells[0], Some(shared));
        assert_eq!(scenes.scenes[1].cells[0], Some(shared));
    }

    #[test]
    fn track_effect_chain_values_restore_each_pattern_by_stable_pattern_id() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let slot_descriptors = vec![EffectDescriptor::default_full_chain()];
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &slot_descriptors),
                PatternSnapshot::new_default(1, &slot_descriptors),
            ],
            0,
        );
        let first = state.scene_track_pattern_id(0, 0).unwrap();
        let second = state.scene_track_pattern_id(1, 0).unwrap();
        let descriptor = EffectDescriptor::builtin_filter();
        let mut first_slot = EffectSlotSnapshot::new_default(&descriptor, 901);
        first_slot.defaults[0] = 0.21;
        let mut second_slot = EffectSlotSnapshot::new_default(&descriptor, 901);
        second_slot.defaults[0] = 0.79;
        let empty = EffectSlotSnapshot::new_empty().authoring_values();
        let mut pattern_values = [(first, first_slot.authoring_values()), (second, second_slot.authoring_values())]
            .into_iter()
            .map(|(pattern, first_value)| {
                let mut values = vec![empty.clone(); crate::lisp_host::MAX_CUSTOM_FX];
                values[0] = first_value;
                (pattern, values)
            })
            .collect::<Vec<_>>();
        // The track-sound carrier (track-sound spec §2.1) is a pool pattern
        // too: chain history spans it like any other.
        pattern_values.push((state.track_sound_pattern_id(0).expect("track sound"), {
            let mut values = vec![empty.clone(); crate::lisp_host::MAX_CUSTOM_FX];
            values[0] = first_slot.authoring_values();
            values
        }));
        let mut descriptors = vec![descriptor; 1];
        descriptors.resize_with(
            crate::lisp_host::MAX_CUSTOM_FX,
            EffectDescriptor::empty_custom_slot,
        );
        let mut nodes = vec![(901, 0)];
        nodes.resize(crate::lisp_host::MAX_CUSTOM_FX, (0, 0));

        state
            .restore_track_effect_chain_values(
                0,
                BUILTIN_SLOT_COUNT,
                &descriptors,
                &nodes,
                &pattern_values,
            )
            .unwrap();

        let captured = state
            .capture_track_effect_chain_values(
                0,
                BUILTIN_SLOT_COUNT,
                crate::lisp_host::MAX_CUSTOM_FX,
            )
            .unwrap();
        let first_values = captured.iter().find(|(id, _)| *id == first).unwrap();
        let second_values = captured.iter().find(|(id, _)| *id == second).unwrap();
        assert_eq!(first_values.1[0].defaults[0], 0.21);
        assert_eq!(second_values.1[0].defaults[0], 0.79);
    }

    #[test]
    fn track_midi_fx_chain_values_restore_each_pattern_by_stable_pattern_id() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let slot_descriptors = vec![EffectDescriptor::default_full_chain()];
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &slot_descriptors),
                PatternSnapshot::new_default(1, &slot_descriptors),
            ],
            0,
        );
        let first = state.scene_track_pattern_id(0, 0).unwrap();
        let second = state.scene_track_pattern_id(1, 0).unwrap();
        let descriptor = crate::lisp_host::load_midi_fx_descriptor("arp").unwrap();
        let mut first_slot = EffectSlotSnapshot::new_default(&descriptor, 0);
        first_slot.defaults[0] = 0.18;
        let mut second_slot = EffectSlotSnapshot::new_default(&descriptor, 0);
        second_slot.defaults[0] = 0.83;
        let empty = EffectSlotSnapshot::new_empty().authoring_values();
        let mut pattern_values = [(first, first_slot.authoring_values()), (second, second_slot.authoring_values())]
            .into_iter()
            .map(|(pattern, first_value)| {
                let mut values = vec![empty.clone(); crate::lisp_host::MAX_MIDI_FX_SLOTS];
                values[0] = first_value;
                (pattern, values)
            })
            .collect::<Vec<_>>();
        // The track-sound carrier (track-sound spec §2.1) is a pool pattern
        // too: chain history spans it like any other.
        pattern_values.push((state.track_sound_pattern_id(0).expect("track sound"), {
            let mut values = vec![empty.clone(); crate::lisp_host::MAX_MIDI_FX_SLOTS];
            values[0] = first_slot.authoring_values();
            values
        }));

        state
            .restore_track_midi_fx_chain_values(
                0,
                &["arp".to_string()],
                &[descriptor],
                &pattern_values,
            )
            .unwrap();

        let captured = state.capture_track_midi_fx_chain_values(0).unwrap();
        let first_values = captured.iter().find(|(id, _)| *id == first).unwrap();
        let second_values = captured.iter().find(|(id, _)| *id == second).unwrap();
        assert_eq!(first_values.1[0].defaults[0], 0.18);
        assert_eq!(second_values.1[0].defaults[0], 0.83);
    }

    #[test]
    fn repository_midi_fx_insert_applies_to_other_patterns_for_one_track_only() {
        let state = SequencerState::new(2, (0..2).map(|_| default_empty_effect_chain()).collect());
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ],
            0,
        );
        let before = state.export_pattern_repository();
        let mut descriptor = EffectDescriptor::empty_custom_slot();
        descriptor.name = "arp".to_string();

        state.insert_midi_fx_slot_in_other_track_patterns(0, 0, "arp".to_string(), &descriptor);

        let after = state.export_pattern_repository();
        assert_eq!(
            after[0].track_params[0].midi_fx_chain,
            before[0].track_params[0].midi_fx_chain
        );
        assert_eq!(
            after[1].track_params[1].midi_fx_chain,
            before[1].track_params[1].midi_fx_chain
        );
        assert_eq!(
            after[1].track_params[0].midi_fx_chain,
            vec!["arp".to_string()]
        );
    }

    fn sample_bus_pattern_snapshot(marker: f32) -> Vec<BusPatternSnapshot> {
        vec![BusPatternSnapshot {
            id: BusId::DEFAULT_A,
            effect_plocks: vec![
                vec![vec![Some(marker)]],
                vec![vec![Some(marker + 1.0)]],
                vec![vec![Some(marker + 2.0)]],
            ],
            effect_defaults: vec![vec![marker], vec![marker + 1.0], vec![marker + 2.0]],
        }]
    }

    #[test]
    fn bus_pattern_repository_clone_and_delete_are_state_owned() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let first = sample_bus_pattern_snapshot(0.25);
        let second = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![first.clone(), second], &first);

        let new_scene = state.clone_pattern(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );
        let cloned = state.clone_bus_pattern_snapshot(0, new_scene, &first);
        assert_eq!(cloned[0].effect_defaults[0][0], 0.25);
        assert_eq!(
            state.bus_pattern_snapshot_or_default(new_scene, &first)[0].effect_defaults[0][0],
            0.25
        );

        state
            .switch_pattern(
                0,
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .unwrap();
        state
            .delete_pattern(
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .unwrap();
        let restored = state.delete_bus_pattern_snapshot(0, 0, &first);
        assert_eq!(restored[0].effect_defaults[0][0], 0.75);
    }

    #[test]
    fn bus_effect_slot_topology_updates_other_scene_bus_patterns_only() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let current = sample_bus_pattern_snapshot(0.25);
        let other = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![current.clone(), other], &current);

        state.insert_bus_effect_slot_in_other_scene_patterns(0, 1, &current);

        let current_after = state.bus_pattern_snapshot_or_default(0, &current);
        let other_after = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(
            current_after[0].effect_plocks[1][0][0],
            Some(1.25),
            "current scene bus plocks should not be touched"
        );
        assert!(
            other_after[0].effect_plocks[1].is_empty(),
            "other scene should receive an empty inserted bus effect slot"
        );
        assert_eq!(other_after[0].effect_plocks[2][0][0], Some(1.75));
        assert!(
            other_after[0].effect_defaults[1].is_empty(),
            "the inserted slot must also be empty in the parallel defaults lane"
        );
        assert_eq!(
            other_after[0].effect_defaults[2],
            vec![1.75],
            "existing base values must follow their effect when its slot shifts"
        );

        state.move_bus_effect_slot_in_other_scene_patterns(0, 2, 0, &current);

        let current_after_move = state.bus_pattern_snapshot_or_default(0, &current);
        let other_after_move = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(
            current_after_move[0].effect_defaults[0],
            vec![0.25],
            "current scene bus defaults should not be touched"
        );
        assert_eq!(other_after_move[0].effect_plocks[0][0][0], Some(1.75));
        assert_eq!(other_after_move[0].effect_defaults[0], vec![1.75]);
        assert_eq!(other_after_move[0].effect_plocks[1][0][0], Some(0.75));
        assert_eq!(other_after_move[0].effect_defaults[1], vec![0.75]);
    }

    #[test]
    fn bus_effect_slot_topology_uses_final_forward_destination_and_handles_sparse_chains() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let current = sample_bus_pattern_snapshot(0.25);
        let other = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![current.clone(), other], &current);

        state.move_bus_effect_slot_in_other_scene_patterns(0, 0, 2, &current);

        let moved = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(moved[0].effect_defaults[0], vec![1.75]);
        assert_eq!(moved[0].effect_defaults[1], vec![2.75]);
        assert_eq!(moved[0].effect_defaults[2], vec![0.75]);
        assert_eq!(moved[0].effect_plocks[2][0][0], Some(0.75));

        state.remap_bus_effect_slots_in_other_scene_patterns(
            0,
            &[Some(0), Some(2), None],
            &current,
        );

        let compacted = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(compacted[0].effect_defaults[0], vec![1.75]);
        assert_eq!(compacted[0].effect_defaults[1], vec![0.75]);
        assert!(compacted[0].effect_defaults[2].is_empty());
        assert_eq!(compacted[0].effect_plocks[1][0][0], Some(0.75));
    }

    #[test]
    fn bus_effect_slot_initialization_and_clear_update_both_scene_value_lanes() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let current = sample_bus_pattern_snapshot(0.25);
        let other = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![current.clone(), other], &current);
        let initialized = sample_bus_pattern_snapshot(10.0);

        state.replace_bus_effect_slot_in_other_scene_patterns(0, 1, &initialized);

        let current_after = state.bus_pattern_snapshot_or_default(0, &current);
        let other_after = state.bus_pattern_snapshot_or_default(1, &current);
        assert_eq!(current_after[0].effect_defaults[1], vec![1.25]);
        assert_eq!(other_after[0].effect_defaults[1], vec![11.0]);
        assert_eq!(other_after[0].effect_plocks[1][0][0], Some(11.0));

        state.clear_bus_effect_slot_in_other_scene_patterns(0, 1, &current);

        let cleared = state.bus_pattern_snapshot_or_default(1, &current);
        assert!(cleared[0].effect_defaults[1].is_empty());
        assert!(cleared[0].effect_plocks[1].is_empty());
    }

    #[test]
    fn copy_current_device_values_updates_every_pattern_without_copying_locks() {
        let descriptor = EffectDescriptor::builtin_filter();
        let state = SequencerState::new(1, vec![vec![EffectSlotState::new(&descriptor, 101)]]);
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&descriptor, 201);
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 301);

        let mut first = sample_pattern_snapshot(1);
        let mut second = sample_pattern_snapshot(1);
        first.midi_fx_slots[0][0] = sample_effect_slot_snapshot(20);
        second.midi_fx_slots[0][0] = sample_effect_slot_snapshot(21);
        first.rack_tracks[0] = Some(sample_rack_track_snapshot());
        second.rack_tracks[0] = Some(sample_rack_track_snapshot());
        second.effect_slots[0][0].plocks[5][1] = Some(555.0);
        second.midi_fx_slots[0][0].plocks[6][1] = Some(666.0);
        second.instrument_slots[0]
            .key_locks
            .insert(60, vec![Some(777.0), None]);
        second.rack_tracks[0].as_mut().unwrap().slots[0]
            .instrument_slot
            .plocks[7][1] = Some(888.0);
        state.replace_pattern_repository(vec![first, second], 0);

        state.pattern.effect_chains[0][0].defaults.set(0, 91.0);
        state.pattern.effect_chains[0][0].defaults.set(1, 92.0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 93.0);
        state.pattern.midi_fx_slots[0][0].defaults.set(1, 94.0);
        state.pattern.instrument_slots[0].defaults.set(0, 95.0);
        state.pattern.instrument_slots[0].defaults.set(1, 96.0);
        state.pattern.instrument_base_note_offsets[0].store(12.0f32.to_bits(), Ordering::Relaxed);

        let mut live_rack = sample_rack_track_snapshot();
        live_rack.slots[0].instrument_slot.defaults[0] = 97.0;
        live_rack.slots[0].instrument_slot.defaults[1] = 98.0;
        live_rack.slots[0].instrument_base_note_offset = 24.0;
        state.pattern.rack_tracks.lock().unwrap()[0] = Some(live_rack);

        // 2 other scene patterns + the track-sound carrier.
        assert_eq!(
            state.copy_current_effect_values_to_all_track_patterns(0, 0),
            3
        );
        assert_eq!(
            state.copy_current_midi_fx_values_to_all_track_patterns(0, 0),
            3
        );
        assert_eq!(
            state.copy_current_instrument_values_to_all_track_patterns(0),
            3
        );
        assert_eq!(
            state.copy_current_rack_slot_instrument_values_to_all_track_patterns(0, 0),
            3
        );

        let patterns = state.export_pattern_repository();
        for pattern in &patterns {
            assert_eq!(&pattern.effect_slots[0][0].defaults[..2], &[91.0, 92.0]);
            assert_eq!(&pattern.midi_fx_slots[0][0].defaults[..2], &[93.0, 94.0]);
            assert_eq!(&pattern.instrument_slots[0].defaults[..2], &[95.0, 96.0]);
            assert_eq!(pattern.instrument_base_note_offsets[0], 12.0);
            let rack_slot = &pattern.rack_tracks[0].as_ref().unwrap().slots[0];
            assert_eq!(&rack_slot.instrument_slot.defaults[..2], &[97.0, 98.0]);
            assert_eq!(rack_slot.instrument_base_note_offset, 24.0);
        }
        assert_eq!(patterns[1].effect_slots[0][0].plocks[5][1], Some(555.0));
        assert_eq!(patterns[1].midi_fx_slots[0][0].plocks[6][1], Some(666.0));
        assert_eq!(
            patterns[1].instrument_slots[0].key_locks[&60][0],
            Some(777.0)
        );
        assert_eq!(
            patterns[1].rack_tracks[0].as_ref().unwrap().slots[0]
                .instrument_slot
                .plocks[7][1],
            Some(888.0)
        );
    }

    #[test]
    fn copy_current_bus_effect_values_updates_all_scenes_without_copying_plocks() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        let current = sample_bus_pattern_snapshot(0.25);
        let other = sample_bus_pattern_snapshot(0.75);
        state.replace_bus_pattern_repository(vec![current.clone(), other], &current);
        let source = sample_bus_pattern_snapshot(10.0);

        assert_eq!(
            state.copy_bus_effect_values_to_all_scene_patterns(0, 1, &source),
            2
        );

        let first = state.bus_pattern_snapshot_or_default(0, &source);
        let second = state.bus_pattern_snapshot_or_default(1, &source);
        assert_eq!(first[0].effect_defaults[1], vec![11.0]);
        assert_eq!(second[0].effect_defaults[1], vec![11.0]);
        assert_eq!(first[0].effect_plocks[1][0][0], Some(1.25));
        assert_eq!(second[0].effect_plocks[1][0][0], Some(1.75));
    }

    #[test]
    fn track_pattern_data_extracts_one_complete_lane() {
        let snapshot = sample_pattern_snapshot(3);

        let data = snapshot.track_pattern_data(2).unwrap();

        assert_eq!(data.track_bits[0], 3);
        assert_eq!(data.neural_reset_bits, [0u64; TRACK_PATTERN_WORDS]);
        assert_eq!(data.step_data[0][0], 2.25);
        assert_eq!(data.track_params.num_steps, 10);
        assert_eq!(data.effect_slots[0].node_id, 102);
        assert_eq!(data.instrument_slot.node_id, 112);
        assert_eq!(data.instrument_base_note_offset, -10.0);
        assert_eq!(
            data.track_sound_state.loaded_preset.as_deref(),
            Some("preset-2")
        );
        assert_eq!(data.sample_id, (2, "track-2".to_string(), 44_100));
        assert_eq!(data.chord_snapshot.steps[0], vec![2.0, 9.0]);
        assert_eq!(data.timebase_plock_snapshot[0], Some(2));
        assert_eq!(data.swing_plock_snapshot[1], Some(12));
        assert_eq!(data.swing_resolution_plock_snapshot[2], Some(22));
        assert_eq!(data.instrument_type, InstrumentType::Sampler);
        assert_eq!(
            data.instrument_run_mode,
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn track_pattern_data_round_trips_rack_track_lane() {
        let mut source = sample_pattern_snapshot(2);
        source.rack_tracks[1] = Some(sample_rack_track_snapshot());
        let data = source.track_pattern_data(1).unwrap();
        let rack = data.rack_track.as_ref().unwrap();
        assert_eq!(rack.slots.len(), 1);
        assert_eq!(rack.slots[0].instrument_base_note_offset, 7.0);
        assert_eq!(rack.slots[0].gain, 0.8);
        assert!(rack.slots[0].solo);

        let mut target = PatternSnapshot::new_default(1, &[]);
        target.set_track_pattern_data(0, data);

        let restored = target.rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.slots[0].instrument_type, InstrumentType::Custom);
        assert_eq!(
            restored.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(restored.slots[0].instrument_slot.node_id, 177);
    }

    #[test]
    fn append_rack_slot_preserves_existing_rack_slots_in_pattern_pool() {
        let state = SequencerState::new(1, Vec::new());
        let initial = sample_rack_track_snapshot();
        state.set_rack_track_for_all_pattern_snapshots(0, initial.clone());
        let appended = sample_sampler_rack_slot(123, "layer", 48_000, 88);

        state.append_rack_slot_for_all_pattern_snapshots(0, appended);

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 2);
        assert_eq!(
            live.slots[0].track_sound_state.loaded_preset,
            Some("rack-lead".to_string())
        );
        assert_eq!(live.slots[1].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let restored = repository[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.slots.len(), 2);
        assert_eq!(
            restored.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(restored.slots[1].instrument_base_note_offset, -12.0);
        assert_eq!(restored.slots[1].max_polyphony, 2);
    }

    #[test]
    fn append_rack_slot_to_current_pattern_does_not_mutate_other_patterns() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());

        let appended = sample_sampler_rack_slot(123, "current-layer", 48_000, 88);

        assert!(state.append_rack_slot_to_current_pattern(0, appended));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 2);
        assert_eq!(live.slots[1].sample_id.as_ref().unwrap().1, "current-layer");

        let repository = state.export_pattern_repository();
        let current = repository[0].rack_tracks[0].as_ref().unwrap();
        let other = repository[1].rack_tracks[0].as_ref().unwrap();
        assert_eq!(current.slots.len(), 2);
        assert_eq!(
            current.slots[1].sample_id.as_ref().unwrap().1,
            "current-layer"
        );
        assert_eq!(other.slots.len(), 1);
        assert_eq!(
            other.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert!(other.slots[0].sample_id.is_none());
    }

    #[test]
    fn replace_rack_slot_source_preserves_pad_controls_and_slot_plocks() {
        let state = SequencerState::new(1, Vec::new());
        let mut initial = sample_rack_track_snapshot();
        initial.slots[0].choke_group = Some(3);
        initial.slots[0].instrument_base_note_offset = 5.0;
        initial.slots[0].gain = 0.7;
        initial.slots[0].pan = -0.4;
        initial.slots[0].mute = true;
        initial.slots[0].solo = false;
        initial.slots[0].max_polyphony = 6;
        let rack_fx = EffectDescriptor::builtin_ott();
        initial.slots[0].effect_descriptors[0] = rack_fx.clone();
        initial.slots[0].effect_slots[0] = EffectSlotSnapshot::new_default(&rack_fx, 701);
        initial.slots[0].custom_effect_names[0] = Some("builtin:OTT".to_string());
        assert!(initial.slots[0]
            .param_plocks
            .set(4, RackSlotParam::Gain, 0.25));
        state.set_rack_track_for_all_pattern_snapshots(0, initial);

        let replacement = RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: -12.0,
            choke_group: Some(8),
            gain: 0.2,
            pan: 0.5,
            mute: false,
            solo: true,
            max_polyphony: 1,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: sample_effect_slot_snapshot(88),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: Some((123, "replacement".to_string(), 48_000)),
        };

        assert!(state.replace_rack_slot_source_for_all_pattern_snapshots(0, 0, replacement));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        let live_slot = &live.slots[0];
        assert_eq!(live_slot.instrument_type, InstrumentType::Sampler);
        assert_eq!(live_slot.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(live_slot.instrument_slot.node_id, 188);
        assert_eq!(live_slot.choke_group, Some(3));
        assert_eq!(live_slot.instrument_base_note_offset, 5.0);
        assert_eq!(live_slot.gain, 0.7);
        assert_eq!(live_slot.pan, -0.4);
        assert!(live_slot.mute);
        assert!(!live_slot.solo);
        assert_eq!(live_slot.max_polyphony, 6);
        assert_eq!(
            live_slot.param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );

        let repository = state.export_pattern_repository();
        let restored = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(restored.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(restored.choke_group, Some(3));
        assert_eq!(restored.instrument_base_note_offset, 5.0);
        assert_eq!(
            restored.param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );
    }

    #[test]
    fn replace_rack_slot_source_in_current_pattern_keeps_other_patterns_unchanged() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();

        let mut initial = sample_rack_track_snapshot();
        initial.slots[0].choke_group = Some(4);
        initial.slots[0].instrument_base_note_offset = 5.0;
        initial.slots[0].gain = 0.7;
        initial.slots[0].pan = -0.4;
        initial.slots[0].mute = true;
        initial.slots[0].solo = false;
        initial.slots[0].max_polyphony = 6;
        let rack_fx = EffectDescriptor::builtin_ott();
        initial.slots[0].effect_descriptors[0] = rack_fx.clone();
        initial.slots[0].effect_slots[0] = EffectSlotSnapshot::new_default(&rack_fx, 701);
        initial.slots[0].custom_effect_names[0] = Some("builtin:OTT".to_string());
        assert!(initial.slots[0]
            .param_plocks
            .set(4, RackSlotParam::Gain, 0.25));
        state.set_rack_track_for_all_pattern_snapshots(0, initial);

        let mut replacement = sample_sampler_rack_slot(321, "replacement", 48_000, 88);
        replacement.choke_group = Some(8);
        replacement.instrument_base_note_offset = -24.0;
        replacement.gain = 0.2;
        replacement.pan = 0.5;
        replacement.mute = false;
        replacement.solo = true;
        replacement.max_polyphony = 1;

        assert!(state.replace_rack_slot_source_in_current_pattern(0, 0, replacement));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(live.slots[0].choke_group, Some(4));
        assert_eq!(live.slots[0].instrument_base_note_offset, 5.0);
        assert_eq!(live.slots[0].gain, 0.7);
        assert_eq!(live.slots[0].pan, -0.4);
        assert!(live.slots[0].mute);
        assert!(!live.slots[0].solo);
        assert_eq!(live.slots[0].max_polyphony, 6);
        assert_eq!(live.slots[0].effect_slots[0].node_id, 701);
        assert_eq!(live.slots[0].effect_descriptors[0].name, "OTT");
        assert_eq!(
            live.slots[0].custom_effect_names[0].as_deref(),
            Some("builtin:OTT")
        );
        assert_eq!(
            live.slots[0].param_plocks.get(4, RackSlotParam::Gain),
            Some(0.25)
        );

        let repository = state.export_pattern_repository();
        let current = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        let other = &repository[1].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(current.sample_id.as_ref().unwrap().1, "replacement");
        assert_eq!(current.choke_group, Some(4));
        assert_eq!(current.param_plocks.get(4, RackSlotParam::Gain), Some(0.25));
        assert!(other.sample_id.is_none());
        assert_eq!(
            other.track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(other.choke_group, Some(4));
        assert_eq!(other.param_plocks.get(4, RackSlotParam::Gain), Some(0.25));
    }

    #[test]
    fn sync_rack_slot_bindings_for_current_pattern_does_not_rebind_other_patterns() {
        let state = make_state_with_tracks(1);
        let mut current = PatternSnapshot::new_default(1, &[]);
        let mut other = PatternSnapshot::new_default(1, &[]);
        let mut current_rack = sample_rack_track_snapshot();
        let mut other_rack = sample_rack_track_snapshot();
        current_rack.slots[0].instrument_slot = sample_effect_slot_snapshot(11);
        other_rack.slots[0].instrument_slot = sample_effect_slot_snapshot(22);
        current.instrument_types[0] = InstrumentType::Rack;
        other.instrument_types[0] = InstrumentType::Rack;
        current.rack_tracks[0] = Some(current_rack);
        other.rack_tracks[0] = Some(other_rack);
        state.replace_pattern_repository(vec![current, other], 0);
        state.restore_current_pattern_from_repository().unwrap();

        let descriptor = EffectDescriptor::builtin_sampler();
        assert!(state
            .sync_rack_slot_instrument_bindings_for_current_pattern(0, &[(descriptor, 999, 1000)]));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].instrument_slot.node_id, 999);
        assert_eq!(live.slots[0].instrument_slot.modulator_node_id, 1000);

        let repository = state.export_pattern_repository();
        let current_slot = &repository[0].rack_tracks[0].as_ref().unwrap().slots[0];
        let other_slot = &repository[1].rack_tracks[0].as_ref().unwrap().slots[0];
        assert_eq!(current_slot.instrument_slot.node_id, 999);
        assert_eq!(current_slot.instrument_slot.modulator_node_id, 1000);
        assert_eq!(other_slot.instrument_slot.node_id, 122);
        assert_eq!(other_slot.instrument_slot.modulator_node_id, 0);
    }

    #[test]
    fn launch_scene_restores_pattern_locked_rack_sources() {
        let state = make_state_with_tracks(1);
        let mut first = PatternSnapshot::new_default(1, &[]);
        let mut second = PatternSnapshot::new_default(1, &[]);
        first.instrument_types[0] = InstrumentType::Rack;
        second.instrument_types[0] = InstrumentType::Rack;
        first.rack_tracks[0] = Some(RackTrackSnapshot::new(
            vec![sample_sampler_rack_slot(101, "pattern-one", 44_100, 11)],
            default_rack_macros(),
        ));
        second.rack_tracks[0] = Some(RackTrackSnapshot::new(
            vec![sample_sampler_rack_slot(202, "pattern-two", 44_100, 22)],
            default_rack_macros(),
        ));
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-one");

        state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &[String::from("rack")],
                &[InstrumentType::Rack],
            )
            .unwrap();
        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-two");

        state
            .launch_scene(
                0,
                1,
                &[-1],
                &[44_100],
                &[String::from("rack")],
                &[InstrumentType::Rack],
            )
            .unwrap();
        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "pattern-one");
    }

    #[test]
    fn remove_rack_slot_from_current_pattern_does_not_mutate_other_patterns() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());
        state.append_rack_slot_for_all_pattern_snapshots(
            0,
            sample_sampler_rack_slot(123, "layer", 48_000, 88),
        );

        assert!(state.remove_rack_slot_from_current_pattern(0, 0));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 1);
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let current = repository[0].rack_tracks[0].as_ref().unwrap();
        let other = repository[1].rack_tracks[0].as_ref().unwrap();
        assert_eq!(current.slots.len(), 1);
        assert_eq!(current.slots[0].sample_id.as_ref().unwrap().1, "layer");
        assert_eq!(other.slots.len(), 2);
        assert_eq!(
            other.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("rack-lead")
        );
        assert_eq!(other.slots[1].sample_id.as_ref().unwrap().1, "layer");
    }

    #[test]
    fn remove_rack_slot_updates_live_and_pattern_pool_slots() {
        let state = SequencerState::new(1, Vec::new());
        let initial = sample_rack_track_snapshot();
        state.set_rack_track_for_all_pattern_snapshots(0, initial);
        let appended = sample_sampler_rack_slot(123, "layer", 48_000, 88);
        state.append_rack_slot_for_all_pattern_snapshots(0, appended);

        assert!(state.remove_rack_slot_from_all_pattern_snapshots(0, 0));

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(live.slots.len(), 1);
        assert_eq!(live.slots[0].sample_id.as_ref().unwrap().1, "layer");

        let repository = state.export_pattern_repository();
        let restored = repository[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored.slots.len(), 1);
        assert_eq!(restored.slots[0].sample_id.as_ref().unwrap().1, "layer");
        assert_eq!(restored.slots[0].instrument_base_note_offset, -12.0);
    }

    #[test]
    fn normalize_track_count_extends_missing_rack_lane_for_legacy_snapshots() {
        let mut snapshot = sample_pattern_snapshot(2);
        snapshot.rack_tracks.clear();

        snapshot.normalize_track_count(2, &[]);

        assert_eq!(snapshot.rack_tracks.len(), 2);
        assert!(snapshot.rack_tracks.iter().all(Option::is_none));
        assert!(snapshot.track_lane_count_is_consistent());
    }

    #[test]
    fn set_track_pattern_data_round_trips_one_lane() {
        let source = sample_pattern_snapshot(3);
        let data = source.track_pattern_data(2).unwrap();
        let mut target = PatternSnapshot::new_default(1, &[]);

        target.set_track_pattern_data(0, data);

        assert_eq!(target.track_bits[0][0], 3);
        assert_eq!(target.step_data[0][0][0], 2.25);
        assert_eq!(target.track_params[0].num_steps, 10);
        assert_eq!(target.effect_slots[0][0].node_id, 102);
        assert_eq!(target.instrument_slots[0].node_id, 112);
        assert_eq!(target.sample_ids[0], (2, "track-2".to_string(), 44_100));
        assert_eq!(target.chord_snapshots[0].steps[0], vec![2.0, 9.0]);
        assert_eq!(target.timebase_plock_snapshots[0][0], Some(2));
        assert_eq!(target.instrument_types[0], InstrumentType::Sampler);
        assert_eq!(
            target.instrument_run_modes[0],
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn project_scenes_identity_mapping_splits_patterns_into_track_pools() {
        let first = sample_pattern_snapshot(2);
        let mut second = sample_pattern_snapshot(2);
        second.track_bits[0][0] = 99;
        second.track_bits[1][0] = 199;
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        second.mod_connections.push(route);

        let scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 1);

        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_pools.len(), 2);
        // 2 scene cells + the track-sound carrier (track-sound spec §2.1).
        assert_eq!(scenes.track_pools[0].patterns.len(), 3);
        assert_eq!(scenes.track_pools[1].patterns.len(), 3);
        assert_eq!(scenes.scenes.len(), 2);
        assert_eq!(scenes.track_overrides, vec![None, None]);

        let first_track_zero = scenes.scenes[0].cells[0].unwrap();
        let second_track_zero = scenes.scenes[1].cells[0].unwrap();
        assert_ne!(first_track_zero, second_track_zero);
        assert_eq!(
            scenes.track_pools[0]
                .get(first_track_zero)
                .unwrap()
                .track_bits[0],
            1
        );
        assert_eq!(
            scenes.track_pools[0]
                .get(second_track_zero)
                .unwrap()
                .track_bits[0],
            99
        );

        let second_track_one = scenes.scenes[1].cells[1].unwrap();
        assert_eq!(
            scenes.track_pools[1]
                .get(second_track_one)
                .unwrap()
                .track_bits[0],
            199
        );
        assert_eq!(scenes.scenes[1].mod_connections, vec![route]);
        assert!(scenes.scenes[0].mod_connections.is_empty());
    }

    #[test]
    fn project_scenes_effective_pattern_prefers_track_override() {
        let first = sample_pattern_snapshot(2);
        let mut second = sample_pattern_snapshot(2);
        second.track_bits[1][0] = 42;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);

        let scene_pattern = scenes.scenes[0].cells[1].unwrap();
        let override_pattern = scenes.scenes[1].cells[1].unwrap();
        assert_eq!(scenes.effective_pattern_id(1), Some(scene_pattern));

        scenes.track_overrides[1] = Some(override_pattern);

        assert_eq!(scenes.effective_pattern_id(1), Some(override_pattern));
    }

    #[test]
    fn project_scenes_new_scene_forks_current_effective_pattern_per_track() {
        let first = sample_pattern_snapshot(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 3,
        };
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        scenes.scenes[0].mod_connections.push(route);
        let track_zero_original = scenes.scenes[0].cells[0].unwrap();
        let track_one_original = scenes.scenes[0].cells[1].unwrap();

        let track_one_override = scenes.fork_track_pattern(1).unwrap();
        Arc::make_mut(
            scenes.track_pools[1]
                .patterns
                .get_mut(&track_one_override)
                .unwrap(),
        )
        .seq
        .track_bits[0] = 77;

        let new_scene = scenes.new_scene();

        assert_eq!(new_scene, 1);
        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_overrides, vec![None, None]);
        // Counts include each track's track-sound carrier.
        assert_eq!(scenes.track_pools[0].patterns.len(), 3);
        assert_eq!(scenes.track_pools[1].patterns.len(), 4);
        assert_eq!(scenes.scenes[1].mod_connections, vec![route]);

        let track_zero_new = scenes.scenes[1].cells[0].unwrap();
        let track_one_new = scenes.scenes[1].cells[1].unwrap();
        assert_ne!(track_zero_original, track_zero_new);
        assert_ne!(track_one_original, track_one_new);
        assert_ne!(track_one_override, track_one_new);
        assert_eq!(
            scenes.track_pools[0]
                .get(track_zero_new)
                .unwrap()
                .track_bits[0],
            scenes.track_pools[0]
                .get(track_zero_original)
                .unwrap()
                .track_bits[0]
        );
        assert_eq!(
            scenes.track_pools[1].get(track_one_new).unwrap().track_bits[0],
            77
        );
    }

    #[test]
    fn project_scenes_set_cell_shares_pool_entry_and_fork_diverges() {
        let first = sample_pattern_snapshot(1);
        let mut second = sample_pattern_snapshot(1);
        second.track_bits[0][0] = 42;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 1);

        let shared = scenes.scenes[0].cells[0].unwrap();
        assert!(scenes.set_cell(1, 0, shared));
        Arc::make_mut(scenes.track_pools[0].patterns.get_mut(&shared).unwrap()).seq.track_bits[0] = 123;

        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            123
        );

        let forked = scenes.fork_track_pattern(0).unwrap();
        Arc::make_mut(scenes.track_pools[0].patterns.get_mut(&forked).unwrap()).seq.track_bits[0] = 999;

        assert_eq!(
            scenes.track_pools[0].get(shared).unwrap().track_bits[0],
            123
        );
        assert_eq!(
            scenes.track_pools[0].get(forked).unwrap().track_bits[0],
            999
        );
        assert_eq!(scenes.effective_pattern_id(0), Some(forked));
    }

    #[test]
    fn project_scenes_clear_cell_keeps_orphan_re_shareable() {
        let first = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        let id = scenes.scenes[0].cells[0].unwrap();
        scenes.track_overrides[0] = Some(id);

        assert_eq!(scenes.clear_cell(0, 0), Some(id));

        assert_eq!(scenes.scenes[0].cells[0], None);
        assert_eq!(scenes.track_overrides[0], None);
        assert!(scenes.track_pools[0].contains(id));
        assert!(scenes.set_cell(0, 0, id));
        assert_eq!(scenes.scenes[0].cells[0], Some(id));
    }

    #[test]
    fn project_scenes_launch_scene_clears_overrides_and_preserves_empty_cells() {
        let first = sample_pattern_snapshot(2);
        let second = sample_pattern_snapshot(2);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let override_id = scenes.scenes[1].cells[0].unwrap();
        scenes.track_overrides[0] = Some(override_id);
        scenes.clear_cell(1, 1);

        let launched = scenes.launch_scene(1).unwrap();

        assert_eq!(scenes.current_scene, 1);
        assert_eq!(scenes.track_overrides, vec![None, None]);
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0].as_ref().unwrap().track_bits[0], 1);
        assert!(launched[1].is_none());
    }

    #[test]
    fn project_scenes_launch_track_pattern_sets_override_and_returns_restore_data() {
        let first = sample_pattern_snapshot(1);
        let mut second = sample_pattern_snapshot(1);
        second.track_bits[0][0] = 88;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let id = scenes.scenes[1].cells[0].unwrap();

        let data = scenes.launch_track_pattern(0, id).unwrap();

        assert_eq!(data.track_bits[0], 88);
        assert_eq!(scenes.track_overrides[0], Some(id));
        assert_eq!(scenes.effective_pattern_id(0), Some(id));
    }

    #[test]
    fn project_scenes_save_effective_track_pattern_makes_edits_durable_across_launches() {
        let first = sample_pattern_snapshot(1);
        let second = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let original_id = scenes.scenes[0].cells[0].unwrap();
        let other_id = scenes.scenes[1].cells[0].unwrap();
        let mut edited = scenes.effective_track_pattern(0).unwrap();
        edited.track_bits[0] = 321;

        assert!(scenes.save_effective_track_pattern(0, edited));
        scenes.launch_track_pattern(0, other_id).unwrap();
        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            scenes.track_pools[0].get(other_id).unwrap().track_bits[0]
        );
        scenes.launch_scene(0).unwrap();

        assert_eq!(scenes.effective_pattern_id(0), Some(original_id));
        assert_eq!(
            scenes.effective_track_pattern(0).unwrap().track_bits[0],
            321
        );
    }

    #[test]
    fn project_scenes_remove_track_drops_pool_scene_column_and_override() {
        let first = sample_pattern_snapshot(3);
        let mut second = sample_pattern_snapshot(3);
        second.track_bits[2][0] = 44;
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let track_two_id = scenes.scenes[1].cells[2].unwrap();
        scenes.track_overrides[2] = Some(track_two_id);

        assert!(scenes.remove_track(1));

        assert_eq!(scenes.track_pools.len(), 2);
        assert_eq!(scenes.track_overrides.len(), 2);
        assert_eq!(scenes.scenes[0].cells.len(), 2);
        assert_eq!(scenes.scenes[1].cells.len(), 2);
        assert_eq!(scenes.track_overrides[1], Some(track_two_id));
        assert_eq!(scenes.scenes[1].cells[1], Some(track_two_id));
        assert_eq!(
            scenes.track_pools[1]
                .get(scenes.scenes[1].cells[1].unwrap())
                .unwrap()
                .track_bits[0],
            44
        );
    }

    #[test]
    fn project_scenes_move_track_take_pool_keeps_take_pools_parallel() {
        let first = sample_pattern_snapshot(2);
        let second = sample_pattern_snapshot(2);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let chunk = scenes.scenes[1].cells[1].unwrap();
        let sound = scenes.track_pools[1].refs(chunk).expect("chunk refs");
        let take = scenes.take_pools[1].insert(None, vec![chunk], 64, sound);

        // Undo of "delete track 0" re-appends the track at the end and moves
        // its lane back to index 0; every per-track vector must follow.
        scenes.move_track_take_pool(1, 0);

        assert!(scenes.take_pools[0].contains(take));
        assert!(scenes.take_pools[1].takes.is_empty());
        assert_eq!(scenes.take_pools.len(), scenes.track_pools.len());
    }

    #[test]
    fn project_scenes_purge_unused_track_patterns_removes_only_unreferenced_orphans() {
        let first = sample_pattern_snapshot(1);
        let second = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first, second], 0);
        let scene_zero_id = scenes.scenes[0].cells[0].unwrap();
        let scene_one_id = scenes.scenes[1].cells[0].unwrap();
        let override_only_id = scenes.fork_track_pattern(0).unwrap();
        let orphan_id = scenes.clear_cell(1, 0).unwrap();

        assert_eq!(orphan_id, scene_one_id);
        assert_eq!(scenes.purge_unused_track_patterns(), 1);

        assert!(scenes.track_pools[0].contains(scene_zero_id));
        assert!(scenes.track_pools[0].contains(override_only_id));
        assert!(!scenes.track_pools[0].contains(orphan_id));
        assert_eq!(scenes.track_overrides[0], Some(override_only_id));
    }

    #[test]
    fn pattern_snapshot_remove_track_compacts_all_track_lanes() {
        let mut snapshot = sample_pattern_snapshot(3);

        snapshot.remove_track(1);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.step_data.len(), 2);
        assert_eq!(snapshot.track_params.len(), 2);
        assert_eq!(snapshot.effect_slots.len(), 2);
        assert_eq!(snapshot.instrument_slots.len(), 2);
        assert_eq!(snapshot.instrument_base_note_offsets.len(), 2);
        assert_eq!(snapshot.track_sound_states.len(), 2);
        assert_eq!(snapshot.sample_ids.len(), 2);
        assert_eq!(snapshot.chord_snapshots.len(), 2);
        assert_eq!(snapshot.timebase_plock_snapshots.len(), 2);
        assert_eq!(snapshot.swing_plock_snapshots.len(), 2);
        assert_eq!(snapshot.swing_resolution_plock_snapshots.len(), 2);
        assert_eq!(snapshot.instrument_types.len(), 2);
        assert_eq!(snapshot.instrument_run_modes.len(), 2);

        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1][0], 3);
        assert_eq!(snapshot.step_data[0][0][0], 0.25);
        assert_eq!(snapshot.step_data[1][0][0], 2.25);
        assert_eq!(snapshot.track_params[0].num_steps, 8);
        assert_eq!(snapshot.track_params[1].num_steps, 10);
        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[1][0].node_id, 102);
        assert_eq!(snapshot.instrument_slots[0].node_id, 110);
        assert_eq!(snapshot.instrument_slots[1].node_id, 112);
        assert_eq!(snapshot.instrument_base_note_offsets, vec![-12.0, -10.0]);
        assert_eq!(
            snapshot.track_sound_states[0].loaded_preset.as_deref(),
            Some("preset-0")
        );
        assert_eq!(
            snapshot.track_sound_states[1].loaded_preset.as_deref(),
            Some("preset-2")
        );
        assert_eq!(snapshot.sample_ids[0], (0, "track-0".to_string(), 44_100));
        assert_eq!(snapshot.sample_ids[1], (2, "track-2".to_string(), 44_100));
        assert_eq!(snapshot.chord_snapshots[0].steps[0], vec![0.0, 7.0]);
        assert_eq!(snapshot.chord_snapshots[1].steps[0], vec![2.0, 9.0]);
        assert_eq!(snapshot.timebase_plock_snapshots[0][0], Some(0));
        assert_eq!(snapshot.timebase_plock_snapshots[1][0], Some(2));
        assert_eq!(snapshot.swing_plock_snapshots[0][1], Some(10));
        assert_eq!(snapshot.swing_plock_snapshots[1][1], Some(12));
        assert_eq!(snapshot.swing_resolution_plock_snapshots[0][2], Some(20));
        assert_eq!(snapshot.swing_resolution_plock_snapshots[1][2], Some(22));
        assert_eq!(snapshot.instrument_types[0], InstrumentType::Sampler);
        assert_eq!(snapshot.instrument_types[1], InstrumentType::Sampler);
        assert_eq!(
            snapshot.instrument_run_modes,
            vec![
                CustomInstrumentRunMode::Instrument,
                CustomInstrumentRunMode::Instrument
            ]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_shifts_first_track() {
        let mut snapshot = sample_pattern_snapshot(3);

        snapshot.remove_track(0);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.track_bits[0][0], 2);
        assert_eq!(snapshot.track_bits[1][0], 3);
        assert_eq!(snapshot.sample_ids[0], (1, "track-1".to_string(), 44_100));
        assert_eq!(snapshot.sample_ids[1], (2, "track-2".to_string(), 44_100));
    }

    #[test]
    fn pattern_snapshot_remove_track_remaps_mod_connections() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.mod_connections = vec![
            ModConnection {
                source_track: 1,
                destination: ModDestination::Track(3),
                dest_input: 2,
            },
            ModConnection {
                source_track: 0,
                destination: ModDestination::Track(2),
                dest_input: 1,
            },
            ModConnection {
                source_track: 2,
                destination: ModDestination::Track(1),
                dest_input: 3,
            },
        ];

        snapshot.remove_track(1);

        assert_eq!(
            snapshot.mod_connections,
            vec![ModConnection {
                source_track: 0,
                destination: ModDestination::Track(1),
                dest_input: 1,
            }]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_preserves_bus_mod_destinations() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.mod_connections = vec![
            ModConnection {
                source_track: 2,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 1,
            },
            ModConnection {
                source_track: 1,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 2,
            },
        ];

        snapshot.remove_track(1);

        assert_eq!(
            snapshot.mod_connections,
            vec![ModConnection {
                source_track: 1,
                destination: ModDestination::Bus(BusId(42)),
                dest_input: 1,
            }]
        );
    }

    #[test]
    fn pattern_snapshot_remove_track_remaps_neural_routes() {
        let mut snapshot = sample_pattern_snapshot(4);
        let mut network = crate::neural::ProjectNeuralNetwork::default();
        network.neurons[0].route = Some(0);
        network.neurons[1].route = Some(1);
        network.neurons[2].route = Some(3);
        snapshot.neural_networks = vec![network];

        snapshot.remove_track(1);

        let neurons = &snapshot.neural_networks[0].neurons;
        assert_eq!(neurons[0].route, Some(0));
        assert_eq!(neurons[1].route, None);
        assert_eq!(neurons[2].route, Some(2));
    }

    #[test]
    fn pattern_snapshot_normalize_fills_missing_loaded_project_lanes() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.step_data[0].truncate(3);
        snapshot.normalize_track_count(3, &[]);

        assert!(snapshot.track_lane_count_is_consistent());
        assert_eq!(snapshot.track_bits.len(), 3);
        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1], [0; TRACK_PATTERN_WORDS]);
        assert_eq!(snapshot.track_bits[2], [0; TRACK_PATTERN_WORDS]);
        assert_eq!(snapshot.step_data[0].len(), MAX_STEPS);
        assert_eq!(
            snapshot.step_data[0][3][StepParam::Velocity.index()],
            StepParam::Velocity.default_value()
        );
        assert_eq!(
            snapshot.track_params[1].num_steps,
            TrackParamsSnapshot::default().num_steps
        );
        assert_eq!(snapshot.sample_ids[2], (-1, String::new(), 44_100));
        assert_eq!(snapshot.instrument_types[1], InstrumentType::Sampler);
        assert_eq!(
            snapshot.instrument_run_modes[1],
            CustomInstrumentRunMode::Instrument
        );
    }

    #[test]
    fn pattern_snapshot_normalize_truncates_extra_loaded_project_lanes() {
        let mut snapshot = sample_pattern_snapshot(4);
        snapshot.normalize_track_count(2, &[]);

        assert!(snapshot.track_lane_count_is_consistent());
        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.track_bits[0][0], 1);
        assert_eq!(snapshot.track_bits[1][0], 2);
        assert_eq!(
            snapshot.sample_ids,
            vec![
                (0, "track-0".to_string(), 44_100),
                (1, "track-1".to_string(), 44_100)
            ]
        );
    }

    #[test]
    fn pattern_snapshot_remove_effect_slot_compacts_slot_plocks() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
        ];

        snapshot.remove_effect_slot(0, 1);

        assert_eq!(snapshot.effect_slots[0].len(), 3);
        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 102);
        assert_eq!(snapshot.effect_slots[0][1].defaults, vec![2.0, 2.5]);
        assert_eq!(snapshot.effect_slots[0][1].plocks[2][0], Some(2.0));
        assert_eq!(snapshot.effect_slots[0][2].node_id, 0);
        assert_eq!(snapshot.effect_slots[0][2].num_params, 0);
        assert!(snapshot.effect_slots[0][2].defaults.is_empty());
        assert!(snapshot.effect_slots[0][2].plocks[2].is_empty());
    }

    #[test]
    fn pattern_snapshot_insert_empty_effect_slot_shifts_existing_slots() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
            EffectSlotSnapshot::new_empty(),
        ];

        snapshot.insert_empty_effect_slot(0, 1);

        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 0);
        assert_eq!(snapshot.effect_slots[0][2].node_id, 101);
        assert_eq!(snapshot.effect_slots[0][3].node_id, 102);
    }

    #[test]
    fn pattern_snapshot_move_effect_slot_reorders_without_losing_payload() {
        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![
            sample_effect_slot_snapshot(0),
            sample_effect_slot_snapshot(1),
            sample_effect_slot_snapshot(2),
            EffectSlotSnapshot::new_empty(),
        ];

        snapshot.move_effect_slot_to(0, 2, 1);

        assert_eq!(snapshot.effect_slots[0][0].node_id, 100);
        assert_eq!(snapshot.effect_slots[0][1].node_id, 102);
        assert_eq!(snapshot.effect_slots[0][1].defaults, vec![2.0, 2.5]);
        assert_eq!(snapshot.effect_slots[0][2].node_id, 101);
        assert_eq!(snapshot.effect_slots[0][3].node_id, 0);
    }

    #[test]
    fn pattern_snapshot_remove_track_ignores_out_of_range_index() {
        let mut snapshot = sample_pattern_snapshot(2);
        let original = snapshot.clone();

        snapshot.remove_track(5);

        assert_eq!(snapshot.track_bits, original.track_bits);
        assert_eq!(snapshot.step_data, original.step_data);
        assert_eq!(snapshot.track_params.len(), original.track_params.len());
        assert_eq!(snapshot.sample_ids, original.sample_ids);
    }

    #[test]
    fn pattern_snapshot_remove_track_tolerates_sparse_legacy_lanes() {
        let mut snapshot = sample_pattern_snapshot(3);
        snapshot.swing_plock_snapshots.clear();
        snapshot.swing_resolution_plock_snapshots.truncate(1);
        snapshot.instrument_types.truncate(2);
        snapshot.instrument_run_modes.truncate(2);

        snapshot.remove_track(1);

        assert_eq!(snapshot.track_bits.len(), 2);
        assert_eq!(snapshot.step_data.len(), 2);
        assert_eq!(snapshot.track_params.len(), 2);
        assert_eq!(snapshot.swing_plock_snapshots.len(), 0);
        assert_eq!(snapshot.swing_resolution_plock_snapshots.len(), 1);
        assert_eq!(snapshot.instrument_types.len(), 1);
        assert_eq!(snapshot.instrument_run_modes.len(), 1);
    }

    #[test]
    fn remap_sidechain_selection_resets_deleted_source_to_off() {
        let remapped = remap_sidechain_selection_after_track_delete(0, 2, 2, 4);
        assert_eq!(remapped, 0);
    }

    #[test]
    fn remap_sidechain_selection_shifts_source_above_deleted_track() {
        let remapped = remap_sidechain_selection_after_track_delete(0, 3, 1, 4);
        assert_eq!(remapped, 2);
    }

    #[test]
    fn remap_snapshot_sidechain_references_updates_defaults_and_plocks() {
        let mut snapshot = PatternSnapshot {
            track_bits: vec![[0; TRACK_PATTERN_WORDS]; 4],
            neural_reset_bits: vec![[0; TRACK_PATTERN_WORDS]; 4],
            step_data: vec![vec![[0.0; NUM_PARAMS]; MAX_STEPS]; 4],
            track_params: vec![TrackParamsSnapshot::default(); 4],
            effect_slots: vec![
                vec![EffectSlotSnapshot::new_empty()],
                vec![EffectSlotSnapshot {
                    node_id: 1,
                    modulator_node_id: 0,
                    num_params: 1,
                    defaults: vec![3.0],
                    plocks: {
                        let mut plocks = (0..MAX_STEPS).map(|_| vec![None]).collect::<Vec<_>>();
                        plocks[0][0] = Some(2.0);
                        plocks
                    },
                    plock_param_ids: vec![vec![None]; MAX_STEPS],
                    key_locks: std::collections::BTreeMap::new(),
                    key_lock_param_ids: std::collections::BTreeMap::new(),
                    param_node_indices: vec![0],
                    param_node_spans: vec![1],
                    transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
                    tensor_params: Vec::new(),
                    ir: None,
                    table: None,
                    sampler_slice_edits: None,
                }],
                vec![EffectSlotSnapshot::new_empty()],
                vec![EffectSlotSnapshot::new_empty()],
            ],
            midi_fx_slots: vec![
                vec![
                    EffectSlotSnapshot::new_empty();
                    crate::lisp_host::MAX_MIDI_FX_SLOTS
                ];
                4
            ],
            instrument_slots: vec![EffectSlotSnapshot::new_empty(); 4],
            instrument_base_note_offsets: vec![0.0; 4],
            track_sound_states: vec![TrackSoundState::default(); 4],
            sample_ids: vec![(-1, String::new(), 44_100); 4],
            chord_snapshots: (0..4).map(|_| ChordSnapshot::new_default()).collect(),
            timebase_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            swing_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            swing_resolution_plock_snapshots: vec![[None; MAX_STEPS]; 4],
            track_send_plock_snapshots: vec![vec![Vec::new(); MAX_STEPS]; 4],
            instrument_types: vec![InstrumentType::Sampler; 4],
            instrument_run_modes: vec![CustomInstrumentRunMode::Instrument; 4],
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: vec![None; 4],
            process_chains: vec![crate::process::TrackProcessChain::default(); 4],
            project_process_lane_overrides: vec![Default::default(); 4],
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: vec![PlockVariantRegistry::default(); 4],
            key_lock_variant_registries: vec![PlockVariantRegistry::default(); 4],
        };
        let descriptors = vec![
            vec![EffectDescriptor::builtin_filter()],
            vec![sample_sidechain_descriptor()],
            vec![EffectDescriptor::builtin_filter()],
            vec![EffectDescriptor::builtin_filter()],
        ];

        remap_snapshot_sidechain_references_after_track_delete(&mut snapshot, &descriptors, 2, 4);

        assert_eq!(snapshot.effect_slots[1][0].defaults[0], 2.0);
        assert_eq!(snapshot.effect_slots[1][0].plocks[0][0], Some(0.0));
    }

    #[test]
    fn move_step_range_preserves_chords_and_step_plocks() {
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(
                &EffectDescriptor::builtin_filter(),
                1,
            )]],
        );
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.instrument_slots[0].apply_descriptor(&EffectDescriptor::builtin_delay(), 2);

        state.pattern.patterns[0].toggle_step(1);
        state.pattern.step_data[0].set(1, StepParam::Velocity, 0.6);
        state.pattern.chord_data[0].add_note(1, 0.0);
        state.pattern.chord_data[0].add_note(1, 4.0);
        state.pattern.chord_data[0].add_note(1, 7.0);
        state.pattern.timebase_plocks[0].set(1, Timebase::Eighth);
        state.pattern.effect_chains[0][0].set_plock(1, 2, 440.0);
        state.pattern.instrument_slots[0].set_plock(1, 0, 0.75);

        state.pattern.patterns[0].toggle_step(2);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.3);
        state.pattern.chord_data[0].add_note(2, 12.0);
        state.pattern.timebase_plocks[0].set(2, Timebase::QuarterTriplet);
        state.pattern.effect_chains[0][0].set_plock(2, 2, 880.0);
        state.pattern.instrument_slots[0].set_plock(2, 0, 0.25);

        state.move_step_range(0, 1, 2, 2);

        assert!(!state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.chord_data[0].count(1), 0);
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.timebase_plocks[0].get(1), None);
        assert_eq!(state.pattern.effect_chains[0][0].plocks.get(1, 2), None);
        assert_eq!(state.pattern.instrument_slots[0].plocks.get(1, 0), None);

        assert!(state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Velocity), 0.6);
        assert_eq!(state.pattern.chord_data[0].count(2), 3);
        assert_eq!(state.pattern.chord_data[0].get(2, 0), 0.0);
        assert_eq!(state.pattern.chord_data[0].get(2, 1), 4.0);
        assert_eq!(state.pattern.chord_data[0].get(2, 2), 7.0);
        assert_eq!(
            state.pattern.timebase_plocks[0].get(2),
            Some(Timebase::Eighth)
        );
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(2, 2),
            Some(440.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(2, 0),
            Some(0.75)
        );

        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Velocity), 0.3);
        assert_eq!(state.pattern.chord_data[0].count(3), 1);
        assert_eq!(state.pattern.chord_data[0].get(3, 0), 12.0);
        assert_eq!(
            state.pattern.timebase_plocks[0].get(3),
            Some(Timebase::QuarterTriplet)
        );
        assert_eq!(
            state.pattern.effect_chains[0][0].plocks.get(3, 2),
            Some(880.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[0].plocks.get(3, 0),
            Some(0.25)
        );
    }

    fn make_state_with_instrument() -> SequencerState {
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(
                &EffectDescriptor::builtin_filter(),
                1,
            )]],
        );
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.instrument_slots[0].apply_descriptor(&EffectDescriptor::builtin_delay(), 2);
        state
    }

    fn make_state_with_rack_slot() -> SequencerState {
        let state = make_state_with_instrument();
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());
        state
    }

    fn make_state_with_tracks(num_tracks: usize) -> SequencerState {
        SequencerState::new(
            num_tracks,
            (0..num_tracks)
                .map(|_| default_empty_effect_chain())
                .collect(),
        )
    }

    fn sample_process_chain() -> crate::process::TrackProcessChain {
        crate::process::TrackProcessChain {
            slots: vec![crate::process::TrackProcessSlot {
                instance_id: crate::process::ProcessInstanceId(7),
                instance_name: Some("sparse-h".to_string()),
                class_name: "sparse".to_string(),
                enabled: true,
                project_layer: false,
                inlets: std::collections::BTreeMap::new(),
                lanes: std::collections::BTreeMap::from([(
                    "amount".to_string(),
                    crate::process::ProcessLane {
                        values: vec![0.0, 1.0],
                    },
                )]),
                bindings: std::collections::BTreeMap::new(),
            }],
        }
    }

    fn effect_process_chain(
        port: &str,
        effect: &str,
        param: &str,
        param_id: ParamNodeId,
    ) -> crate::process::TrackProcessChain {
        crate::process::TrackProcessChain {
            slots: vec![crate::process::TrackProcessSlot {
                instance_id: crate::process::ProcessInstanceId(8),
                instance_name: Some("phase3b-writer-h".to_string()),
                class_name: "phase3b-mappable-writer".to_string(),
                enabled: true,
                project_layer: false,
                inlets: std::collections::BTreeMap::new(),
                lanes: std::collections::BTreeMap::new(),
                bindings: std::collections::BTreeMap::from([(
                    port.to_string(),
                    Some(crate::process::ParamTarget::EffectParam {
                        slot: 0,
                        effect: effect.to_string(),
                        param: param.to_string(),
                        param_id: Some(param_id),
                    }),
                )]),
            }],
        }
    }

    fn effect_binding_param_id(
        chain: &crate::process::TrackProcessChain,
        port: &str,
    ) -> Option<ParamNodeId> {
        let binding = chain.slots.first()?.bindings.get(port)?.as_ref()?;
        match binding {
            crate::process::ParamTarget::EffectParam { param_id, .. } => *param_id,
            _ => None,
        }
    }

    #[test]
    fn process_binding_param_ids_refresh_to_restored_effect_slot() {
        let desc = EffectDescriptor::builtin_insert("Str8 Delay").expect("Str8 Delay descriptor");
        let wet_idx = desc
            .params
            .iter()
            .position(|param| param.name == "wet")
            .expect("Str8 Delay should expose wet");
        let fresh_slot = EffectSlotSnapshot::new_default(&desc, 130);
        let expected = ParamNodeId::from_slot_param(
            fresh_slot.node_id,
            fresh_slot.modulator_node_id,
            fresh_slot.param_node_indices[wet_idx],
        )
        .expect("wet should have a live node identity");
        let stale = ParamNodeId {
            logical_id: 79,
            node_param_idx: expected.node_param_idx,
        };

        let mut snapshot = sample_pattern_snapshot(1);
        snapshot.effect_slots[0] = vec![fresh_slot];
        snapshot.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);

        snapshot.refresh_process_binding_param_ids(&[vec![desc]], &[]);

        assert_eq!(
            effect_binding_param_id(&snapshot.process_chains[0], "color"),
            Some(expected)
        );
    }

    #[test]
    fn process_binding_param_ids_refresh_when_scene_restores_stored_chain() {
        let desc = EffectDescriptor::builtin_insert("Str8 Delay").expect("Str8 Delay descriptor");
        let wet_idx = desc
            .params
            .iter()
            .position(|param| param.name == "wet")
            .expect("Str8 Delay should expose wet");
        let first_slot = EffectSlotSnapshot::new_default(&desc, 130);
        let second_slot = EffectSlotSnapshot::new_default(&desc, 131);
        let first_expected = ParamNodeId::from_slot_param(
            first_slot.node_id,
            first_slot.modulator_node_id,
            first_slot.param_node_indices[wet_idx],
        )
        .expect("first wet should have a live node identity");
        let second_expected = ParamNodeId::from_slot_param(
            second_slot.node_id,
            second_slot.modulator_node_id,
            second_slot.param_node_indices[wet_idx],
        )
        .expect("second wet should have a live node identity");
        let stale = ParamNodeId {
            logical_id: 79,
            node_param_idx: first_expected.node_param_idx,
        };

        let state = make_state_with_tracks(1);
        state.set_scratch_runtime_descriptors(
            vec![vec![desc.clone()]],
            vec![EffectDescriptor::builtin_sampler()],
        );

        let mut first = sample_pattern_snapshot(1);
        first.effect_slots[0] = vec![first_slot];
        first.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);
        let mut second = sample_pattern_snapshot(1);
        second.effect_slots[0] = vec![second_slot];
        second.process_chains[0] = effect_process_chain("color", &desc.name, "wet", stale);

        let buffer_ids = [-1];
        let sample_rates = [44_100];
        let names = [String::new()];
        let instrument_types = [InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(first_expected)
        );

        state
            .switch_pattern(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(second_expected)
        );
        {
            let scheduler_snapshot = state.latest_scheduler_snapshot();
            assert_eq!(
                effect_binding_param_id(&scheduler_snapshot.tracks[0].process_chain, "color"),
                Some(second_expected)
            );
            assert_eq!(
                scheduler_snapshot.tracks[0].effect_descriptors[0].name,
                desc.name
            );
        }

        state
            .switch_pattern(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(
            state
                .track_process_chain(0)
                .and_then(|chain| effect_binding_param_id(&chain, "color")),
            Some(first_expected)
        );
        {
            let scheduler_snapshot = state.latest_scheduler_snapshot();
            assert_eq!(
                effect_binding_param_id(&scheduler_snapshot.tracks[0].process_chain, "color"),
                Some(first_expected)
            );
            assert_eq!(
                scheduler_snapshot.tracks[0].effect_descriptors[0].name,
                desc.name
            );
        }
    }

    #[test]
    fn process_chain_and_lane_values_survive_snapshots_and_project_save() {
        let state = make_state_with_tracks(1);
        let mut expected = sample_process_chain();

        assert!(state.set_track_process_chain(0, expected.clone()));
        assert!(state.set_process_lane_value(
            0,
            crate::process::ProcessInstanceId(7),
            "amount",
            4,
            2.0,
        ));
        expected.slots[0]
            .lanes
            .get_mut("amount")
            .unwrap()
            .values
            .resize(5, 0.0);
        expected.slots[0].lanes.get_mut("amount").unwrap().values[4] = 2.0;

        assert_eq!(state.track_process_chain(0), Some(expected.clone()));
        assert_eq!(
            SequencerSnapshot::capture(&state).tracks[0].process_chain,
            expected
        );

        let snapshot = PatternSnapshot::capture(
            &state,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        );
        assert_eq!(snapshot.process_chains[0], expected);
        let project_pattern = crate::project::ProjectPattern::from_snapshot(
            &snapshot,
            vec![None],
            vec![String::new()],
            Vec::new(),
        );
        assert_eq!(project_pattern.process_chains[0], expected);

        assert!(state.set_track_process_chain(0, crate::process::TrackProcessChain::default()));
        assert_eq!(
            state.track_process_chain(0),
            Some(crate::process::TrackProcessChain::default())
        );
        assert!(snapshot.restore_track(&state, 0));
        assert_eq!(state.track_process_chain(0), Some(expected));
    }

    #[test]
    fn process_chain_slot_edits_are_track_scoped_and_snapshot_visible() {
        let state = make_state_with_tracks(2);
        let mut first = sample_process_chain().slots.remove(0);
        let mut second = first.clone();
        second.instance_id = crate::process::ProcessInstanceId(8);
        second.class_name = "second".to_string();
        let mut third = first.clone();
        third.instance_id = crate::process::ProcessInstanceId(9);
        third.class_name = "third".to_string();
        first.inlets.insert(
            "depth".to_string(),
            crate::process::ProcessLiteral::Number(1.0),
        );
        second.inlets = first.inlets.clone();
        third.inlets = first.inlets.clone();
        let chain = crate::process::TrackProcessChain {
            slots: vec![first, second, third],
        };
        assert!(state.set_track_process_chain(0, chain));

        assert!(state.set_track_process_slot_enabled(
            0,
            crate::process::ProcessInstanceId(8),
            false,
        ));
        assert!(state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(9),
            Some(crate::process::ProcessInstanceId(7)),
        ));
        assert!(state.set_track_process_inlet_value(
            0,
            crate::process::ProcessInstanceId(8),
            "depth",
            crate::process::ProcessLiteral::Number(4.0),
        ));

        let edited = state.track_process_chain(0).expect("track 1 process chain");
        assert_eq!(
            edited
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![9, 7, 8]
        );
        assert!(!edited.slots[2].enabled);
        assert_eq!(
            edited.slots[2].inlets.get("depth"),
            Some(&crate::process::ProcessLiteral::Number(4.0))
        );
        assert!(
            state
                .track_process_chain(1)
                .expect("track 2 process chain")
                .slots
                .is_empty(),
            "slot edits must not leak to another track"
        );
        assert_eq!(
            state.latest_scheduler_snapshot().tracks[0].process_chain,
            edited,
            "every successful slot edit must publish to the scheduler snapshot"
        );

        assert!(state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(7),
            None,
        ));
        assert_eq!(
            state
                .track_process_chain(0)
                .unwrap()
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![9, 8, 7]
        );
        assert!(state.remove_track_process_slot(0, crate::process::ProcessInstanceId(9)));
        assert_eq!(
            state
                .track_process_chain(0)
                .unwrap()
                .slots
                .iter()
                .map(|slot| slot.instance_id.0)
                .collect::<Vec<_>>(),
            vec![8, 7]
        );
        assert!(!state.remove_track_process_slot(0, crate::process::ProcessInstanceId(99)));
        assert!(!state.move_track_process_slot_before(
            0,
            crate::process::ProcessInstanceId(99),
            None,
        ));
    }

    #[test]
    fn process_chain_and_lane_values_survive_scene_switching() {
        let state = make_state_with_tracks(1);
        let mut first = PatternSnapshot::new_default(1, &[]);
        first.process_chains[0] = sample_process_chain();
        let mut second = PatternSnapshot::new_default(1, &[]);
        second.process_chains[0] = sample_process_chain();
        second.process_chains[0].slots[0].instance_id = crate::process::ProcessInstanceId(99);
        second.process_chains[0].slots[0].class_name = "second".to_string();
        second.process_chains[0].slots[0]
            .lanes
            .get_mut("amount")
            .unwrap()
            .values = vec![4.0, 5.0, 6.0];

        let buffer_ids = [-1];
        let sample_rates = [44_100];
        let names = [String::new()];
        let instrument_types = [InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first.clone(), second.clone()], 0);
        state.restore_current_pattern_from_repository().unwrap();
        assert_eq!(
            state.track_process_chain(0),
            Some(first.process_chains[0].clone())
        );

        state
            .switch_pattern(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert_eq!(
            state.track_process_chain(0),
            Some(second.process_chains[0].clone())
        );

        state
            .switch_pattern(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(
            state.track_process_chain(0),
            Some(first.process_chains[0].clone())
        );
    }

    #[test]
    fn project_process_chain_is_scene_scoped_and_survives_scene_switching() {
        let state = make_state_with_tracks(2);
        let mut project_chain = sample_process_chain();
        project_chain.slots[0].project_layer = true;

        let mut first = PatternSnapshot::new_default(2, &[]);
        first.project_process_chain = project_chain.clone();
        let second = PatternSnapshot::new_default(2, &[]);

        let buffer_ids = [-1, -1];
        let sample_rates = [44_100, 44_100];
        let names = [String::new(), String::new()];
        let instrument_types = [InstrumentType::Sampler, InstrumentType::Sampler];
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();

        assert_eq!(state.project_process_chain(), project_chain);
        // Every track's effective chain starts with the shared project slot.
        for track in 0..2 {
            let composed = state
                .composed_track_process_chain(track)
                .expect("composed chain");
            assert_eq!(composed.slots.len(), 1);
            assert!(composed.slots[0].project_layer);
        }
        // The scheduler snapshot sees the composed chain on every track.
        let snapshot = state.publish_scheduler_snapshot();
        for track in 0..2 {
            assert_eq!(snapshot.tracks[track].process_chain.slots.len(), 1);
            assert!(snapshot.tracks[track].process_chain.slots[0].project_layer);
        }

        // Settings are pattern-scoped: scene 2 has its own (empty) layer.
        state
            .switch_pattern(1, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch to scene 2");
        assert!(state.project_process_chain().slots.is_empty());
        let snapshot = state.latest_scheduler_snapshot();
        assert!(snapshot.tracks[0].process_chain.slots.is_empty());

        state
            .switch_pattern(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .expect("switch back to scene 1");
        assert_eq!(state.project_process_chain(), project_chain);

        // Whole-layer replace + export roundtrip.
        assert!(state.set_project_process_chain(crate::process::TrackProcessChain::default()));
        assert!(state.project_process_chain().slots.is_empty());
        assert!(state.set_project_process_chain(project_chain.clone()));
        let bank = state.export_pattern_repository();
        assert_eq!(bank[0].project_process_chain, project_chain);
        assert!(bank[1].project_process_chain.slots.is_empty());
    }

    #[test]
    fn project_lane_ui_edits_fork_per_track_and_can_revert_to_live_template() {
        let state = make_state_with_tracks(2);
        let mut project_chain = sample_process_chain();
        let slot = &mut project_chain.slots[0];
        slot.project_layer = true;
        slot.instance_name = Some("shared".to_string());
        slot.lanes.get_mut("amount").unwrap().values = vec![0.1, 0.2, 0.3];
        let instance_id = slot.instance_id;
        assert!(state.set_project_process_chain(project_chain));

        assert!(state.set_process_lane_value(0, instance_id, "amount", 1, 9.0));
        assert_eq!(
            state.composed_track_process_chain(0).unwrap().slots[0].lanes["amount"].values,
            vec![0.1, 9.0, 0.3]
        );
        assert_eq!(
            state.composed_track_process_chain(1).unwrap().slots[0].lanes["amount"].values,
            vec![0.1, 0.2, 0.3]
        );
        assert_eq!(
            state.project_process_chain().slots[0].lanes["amount"].values,
            vec![0.1, 0.2, 0.3]
        );
        assert!(state.has_project_process_lane_override(0, instance_id, "amount"));

        assert_eq!(
            state.set_process_lane_values(instance_id, "amount", vec![4.0, 5.0]),
            1
        );
        assert_eq!(
            state.composed_track_process_chain(0).unwrap().slots[0].lanes["amount"].values,
            vec![0.1, 9.0, 0.3]
        );
        assert_eq!(
            state.composed_track_process_chain(1).unwrap().slots[0].lanes["amount"].values,
            vec![4.0, 5.0]
        );

        assert!(state.clear_project_process_lane_override(0, instance_id, "amount"));
        assert_eq!(
            state.composed_track_process_chain(0).unwrap().slots[0].lanes["amount"].values,
            vec![4.0, 5.0]
        );
    }

    #[test]
    fn project_process_chain_slot_edits_share_structure_and_fork_track_lanes() {
        let state = make_state_with_tracks(2);
        let mut project_chain = sample_process_chain();
        project_chain.slots[0].project_layer = true;
        let instance_id = project_chain.slots[0].instance_id;
        assert!(state.set_project_process_chain(project_chain));

        // Structural track-scoped mutators fall back to the shared project slot
        // when the instance is not in that track's own chain.
        assert!(state.set_track_process_slot_enabled(1, instance_id, false));
        assert!(!state.project_process_chain().slots[0].enabled);
        assert!(state.set_track_process_slot_enabled(0, instance_id, true));
        assert!(state.project_process_chain().slots[0].enabled);

        assert!(state.set_process_lane_value(1, instance_id, "amount", 2, 7.0));
        assert_eq!(
            state.project_process_chain().slots[0]
                .lanes
                .get("amount")
                .map(|lane| lane.values.clone()),
            Some(vec![0.0, 1.0])
        );
        assert_eq!(
            state.composed_track_process_chain(0).unwrap().slots[0].lanes["amount"].values,
            vec![0.0, 1.0]
        );
        assert_eq!(
            state.composed_track_process_chain(1).unwrap().slots[0].lanes["amount"].values,
            vec![0.0, 1.0, 7.0]
        );
        assert!(state.has_project_process_lane_override(1, instance_id, "amount"));

        // Instance-wide mutators update the live project template without
        // overwriting a track's explicit lane override.
        assert_eq!(
            state.set_process_lane_values(instance_id, "amount", vec![0.0, 3.0]),
            1
        );
        assert_eq!(
            state.composed_track_process_chain(0).unwrap().slots[0].lanes["amount"].values,
            vec![0.0, 3.0]
        );
        assert_eq!(
            state.composed_track_process_chain(1).unwrap().slots[0].lanes["amount"].values,
            vec![0.0, 1.0, 7.0]
        );
        assert!(state.clear_project_process_lane_override(1, instance_id, "amount"));
        assert_eq!(
            state.composed_track_process_chain(1).unwrap().slots[0].lanes["amount"].values,
            vec![0.0, 3.0]
        );
        assert_eq!(state.process_instance_attachment_count(instance_id), 1);

        // Removing from any track's panel removes the shared slot.
        assert!(state.remove_track_process_slot(0, instance_id));
        assert!(state.project_process_chain().slots.is_empty());
        assert!(!state.remove_track_process_slot(0, instance_id));
    }

    #[test]
    fn clone_pattern_preserves_mod_connections_on_source_and_clone() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();

        let cloned_idx = state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        let bank = state.export_pattern_repository();
        assert_eq!(cloned_idx, 1);
        assert_eq!(bank[0].mod_connections, vec![route]);
        assert_eq!(bank[1].mod_connections, vec![route]);
    }

    #[test]
    fn switch_pattern_publishes_snapshot_after_releasing_pattern_bank() {
        let state = make_state_with_tracks(2);
        state.pattern.patterns[0].toggle_step(5);
        state.pattern.step_data[0].set(5, StepParam::Velocity, 0.75);
        state.pattern.chord_data[0].add_note(5, 7.0);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 3,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();
        state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );
        state
            .edit_current_mod_connections(|routes| {
                routes.clear();
                Ok(())
            })
            .unwrap();

        let sample_ids = state.switch_pattern(
            0,
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        assert!(sample_ids.is_some());
        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.transport.current_pattern, 0);
        assert_eq!(snapshot.mod_connections, vec![route]);
        assert!(snapshot.tracks[0].steps[5].active);
        assert_eq!(
            snapshot.tracks[0].steps[5].params[StepParam::Velocity.index()],
            0.75
        );
        assert_eq!(snapshot.tracks[0].steps[5].chord, vec![7.0]);
    }

    #[test]
    fn delete_pattern_preserves_remaining_pattern_mod_connections() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 1,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();
        state.clone_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        let sample_ids = state.delete_pattern(
            2,
            &[-1, -1],
            &[44_100, 44_100],
            &[String::from("mod"), String::from("synth")],
            &[InstrumentType::Modulator, InstrumentType::Custom],
        );

        assert!(sample_ids.is_ok());
        let bank = state.export_pattern_repository();
        assert_eq!(bank.len(), 1);
        assert_eq!(bank[0].mod_connections, vec![route]);
    }

    fn launch_test_args() -> (Vec<i32>, Vec<u32>, Vec<String>, Vec<InstrumentType>) {
        (
            vec![-1, -1],
            vec![44_100, 44_100],
            vec![String::from("one"), String::from("two")],
            vec![InstrumentType::Sampler, InstrumentType::Sampler],
        )
    }

    fn snapshot_with_active_step(track_count: usize, track: usize, step: usize) -> PatternSnapshot {
        let mut snapshot = PatternSnapshot::new_default(track_count, &[]);
        snapshot.track_bits[track][step / 64] |= 1u64 << (step % 64);
        snapshot
    }

    #[test]
    fn stable_step_cell_restore_updates_inactive_pool_without_redirecting_to_current_scene() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 7);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let first_id = state.scene_track_pattern_id(0, 0).unwrap();

        let (before_cells, before_registry) = state
            .capture_pattern_step_cells(0, first_id, &[3])
            .expect("capture active target");
        state.pattern.patterns[0].set_step_active(3, true);
        let (after_cells, after_registry) = state
            .capture_pattern_step_cells(0, first_id, &[3])
            .expect("capture edited target");
        state
            .restore_pattern_step_cells_no_publish(
                0,
                first_id,
                &[(3, after_cells[0].clone())],
                &after_registry,
            )
            .expect("synchronize edited target");

        state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .expect("launch second scene");
        assert!(state.pattern.patterns[0].is_active(7));
        assert!(!state.pattern.patterns[0].is_active(3));

        let touched_live = state
            .restore_pattern_step_cells_no_publish(
                0,
                first_id,
                &[(3, before_cells[0].clone())],
                &before_registry,
            )
            .expect("restore inactive target");
        assert!(!touched_live);
        assert!(state.pattern.patterns[0].is_active(7));

        state
            .launch_scene(
                0,
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
            )
            .expect("return to first scene");
        assert!(!state.pattern.patterns[0].is_active(3));
        assert!(!state.pattern.patterns[0].is_active(7));

        assert!(state
            .restore_pattern_step_cells_no_publish(
                0,
                first_id,
                &[(3, after_cells[0].clone())],
                &after_registry,
            )
            .expect("redo active target"));
        assert!(state.pattern.patterns[0].is_active(3));
    }

    #[test]
    fn launch_track_pattern_changes_only_requested_track_and_scene_launch_clears_override() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let second = snapshot_with_active_step(2, 0, 3);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let pattern_id = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.launch_track_pattern(
            0,
            pattern_id,
            2,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));

        assert!(state.pattern.patterns[0].is_active(3));
        assert!(!state.pattern.patterns[1].is_active(3));
        assert_eq!(state.current_scene_index(), 0);

        state
            .launch_scene(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(
            !state.pattern.patterns[0].is_active(3),
            "scene launch should clear the per-track override"
        );
    }

    /// Boundary-launch preflight (quantized session launches): the target
    /// scene resolves to a complete snapshot without launching, saving, or
    /// publishing anything — the launch may still be canceled.
    #[test]
    fn preflight_pattern_launch_snapshot_resolves_the_target_scene_read_only() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let second = snapshot_with_active_step(2, 0, 3);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let version_before = state.scheduler_snapshot_version();
        let epoch_before = state.transport.pattern_epoch.load(Ordering::Relaxed);

        let snapshot = state
            .preflight_pattern_launch_snapshot(
                &crate::quantized_launch::PatternLaunchTarget::Scene { scene: 1 },
            )
            .expect("target scene resolves");
        assert!(snapshot.transport.playing, "stamped playing for the clock");
        assert_eq!(snapshot.transport.current_pattern, 1);
        assert_eq!(snapshot.tracks.len(), 2);
        assert!(
            snapshot.tracks[0].steps[3].active,
            "carries scene 1's pool content, not the live scene 0 state"
        );
        assert!(!snapshot.tracks[0].scene_silenced);

        // Read-only: nothing launched, saved, or published.
        assert_eq!(state.current_scene_index(), 0);
        assert!(!state.pattern.patterns[0].is_active(3));
        assert_eq!(state.scheduler_snapshot_version(), version_before);
        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            epoch_before
        );

        // Unresolvable targets fall back to the legacy control-side apply.
        assert!(state
            .preflight_pattern_launch_snapshot(
                &crate::quantized_launch::PatternLaunchTarget::Scene { scene: 5 },
            )
            .is_none());
    }

    #[test]
    fn masked_scene_launch_validates_first_and_updates_only_selected_tracks() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let mut second = snapshot_with_active_step(2, 0, 3);
        second.track_bits[1][5 / 64] |= 1u64 << (5 % 64);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.launch_scene_tracks(
            1,
            &[1],
            2,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(!state.pattern.patterns[0].is_active(3));
        assert!(state.pattern.patterns[1].is_active(5));
        assert_eq!(state.current_scene_index(), 0);

        let before_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
        assert!(!state.launch_scene_tracks(
            1,
            &[2],
            2,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert_eq!(
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            before_epoch,
            "a rejected mask must not partially mutate or publish"
        );
    }

    #[test]
    fn track_pattern_cells_report_assigned_active_and_override_state() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![sample_pattern_snapshot(1), sample_pattern_snapshot(1)],
            1,
        );
        let scene_zero_id = state.scene_track_pattern_id(0, 0).unwrap();
        let scene_one_id = state.scene_track_pattern_id(1, 0).unwrap();

        let cells = state.track_pattern_cells(0);
        assert_eq!(cells.len(), 2);
        let scene_one_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_one_id)
            .unwrap();
        assert!(scene_one_cell.assigned_to_current_scene);
        assert!(scene_one_cell.active_effective);
        assert!(!scene_one_cell.overridden);

        assert!(state.launch_track_pattern(
            0,
            scene_zero_id,
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        ));

        let cells = state.track_pattern_cells(0);
        let override_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_zero_id)
            .unwrap();
        assert!(!override_cell.assigned_to_current_scene);
        assert!(override_cell.active_effective);
        assert!(override_cell.overridden);
        let assigned_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == scene_one_id)
            .unwrap();
        assert!(assigned_cell.assigned_to_current_scene);
        assert!(!assigned_cell.active_effective);
        assert!(assigned_cell.overridden);
    }

    #[test]
    fn set_current_scene_cell_restores_shared_pattern_without_override() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 4);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.set_scene_cell(
            0,
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));

        assert!(state.pattern.patterns[0].is_active(4));
        let cells = state.track_pattern_cells(0);
        let shared_cell = cells.iter().find(|cell| cell.pattern_id == shared).unwrap();
        assert!(shared_cell.assigned_to_current_scene);
        assert!(shared_cell.active_effective);
        assert!(!shared_cell.overridden);
        assert!(!state.is_scene_silenced(0));
    }

    #[test]
    fn set_current_scene_cell_clears_override_and_persists_after_scene_return() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 4);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.launch_track_pattern(
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(state
            .track_pattern_cells(0)
            .into_iter()
            .any(|cell| cell.pattern_id == shared && cell.active_effective && cell.overridden));

        assert!(state.set_scene_cell(
            0,
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(state.track_pattern_cells(0).into_iter().any(|cell| {
            cell.pattern_id == shared
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));

        state
            .launch_scene(1, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        state
            .launch_scene(0, 1, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();

        assert_eq!(state.scene_track_pattern_id(0, 0), Some(shared));
        assert!(state.pattern.patterns[0].is_active(4));
        assert!(state.track_pattern_cells(0).into_iter().any(|cell| {
            cell.pattern_id == shared
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn queued_scene_cell_defers_restore_and_pins_outgoing_override() {
        let state = make_state_with_tracks(1);
        let first = PatternSnapshot::new_default(1, &[]);
        let second = snapshot_with_active_step(1, 0, 4);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let outgoing = state.scene_track_pattern_id(0, 0).unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        assert!(state.set_scene_cell_queued(
            0,
            0,
            shared,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));

        // The edit landed (cell assigned) but nothing sounded yet: the live
        // grid still plays the outgoing pattern via its pinned override.
        assert_eq!(state.scene_track_pattern_id(0, 0), Some(shared));
        assert!(!state.pattern.patterns[0].is_active(4));
        let cells = state.track_pattern_cells(0);
        let outgoing_cell = cells
            .iter()
            .find(|cell| cell.pattern_id == outgoing)
            .unwrap();
        assert!(outgoing_cell.active_effective);
        assert!(outgoing_cell.overridden);
        let assigned_cell = cells.iter().find(|cell| cell.pattern_id == shared).unwrap();
        assert!(assigned_cell.assigned_to_current_scene);
        assert!(!assigned_cell.active_effective);

        // Boundary apply: the launch's own masked save-back must self-write
        // the outgoing pattern, never clone the outgoing live content over
        // the newly assigned one.
        assert!(state.launch_scene_tracks(
            0,
            &[0],
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ));
        assert!(state.pattern.patterns[0].is_active(4));
        assert!(state
            .track_pattern_cells(0)
            .into_iter()
            .any(|cell| cell.pattern_id == shared && cell.active_effective));
    }

    #[test]
    fn clone_current_scene_track_pattern_commits_new_pattern_id() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        state.replace_pattern_repository(vec![first], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let original = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cloned = state
            .clone_current_scene_track_pattern(
                0,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();

        assert_ne!(cloned, original);
        assert_eq!(state.scene_track_pattern_id(0, 0), Some(cloned));
        assert!(state.pattern.patterns[0].is_active(2));
        let cells = state.track_pattern_cells(0);
        assert!(cells.iter().any(|cell| cell.pattern_id == original
            && !cell.assigned_to_current_scene
            && !cell.active_effective));
        assert!(cells.iter().any(|cell| cell.pattern_id == cloned
            && cell.assigned_to_current_scene
            && cell.active_effective
            && !cell.overridden));
    }

    #[test]
    fn clone_selected_track_pattern_id_commits_that_source_into_current_scene() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        let second = snapshot_with_active_step(1, 0, 7);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let source = state.scene_track_pattern_id(1, 0).unwrap();
        let original = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cloned = state
            .clone_track_pattern_id_into_current_scene(
                0,
                source,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();

        assert_ne!(cloned, source);
        assert_ne!(cloned, original);
        assert_eq!(state.scene_track_pattern_id(0, 0), Some(cloned));
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(7));
        assert!(state.track_pattern_cells(0).iter().any(|cell| {
            cell.pattern_id == cloned
                && cell.assigned_to_current_scene
                && cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn delete_track_pattern_clears_scene_cells_and_silences_if_effective() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        let second = snapshot_with_active_step(1, 0, 5);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let first_id = state.scene_track_pattern_id(0, 0).unwrap();
        let second_id = state.scene_track_pattern_id(1, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        state
            .delete_track_pattern(
                0,
                second_id,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();
        assert_eq!(state.scene_track_pattern_id(1, 0), None);
        assert!(!state.is_scene_silenced(0));
        assert!(state
            .track_pattern_cells(0)
            .iter()
            .all(|cell| cell.pattern_id != second_id));

        state
            .delete_track_pattern(
                0,
                first_id,
                1,
                &buffer_ids,
                &sample_rates,
                &names,
                &instrument_types,
            )
            .unwrap();
        assert_eq!(state.scene_track_pattern_id(0, 0), None);
        assert!(state.is_scene_silenced(0));
        assert!(state.track_pattern_cells(0).is_empty());
    }

    #[test]
    fn clear_current_scene_cell_silences_without_deleting_pattern() {
        let state = make_state_with_tracks(1);
        let first = snapshot_with_active_step(1, 0, 2);
        state.replace_pattern_repository(vec![first], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let assigned = state.scene_track_pattern_id(0, 0).unwrap();
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        let cleared = state.clear_scene_cell(
            0,
            0,
            1,
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        );

        assert_eq!(cleared, Some(assigned));
        assert!(state.is_scene_silenced(0));
        // Clearing the last cell blanks the live lane (track-sound spec
        // §2.3): leftover steps would otherwise survive as ghosts.
        assert!(!state.pattern.patterns[0].is_active(2));
        assert!(state.track_pattern_cells(0).iter().any(|cell| {
            cell.pattern_id == assigned
                && !cell.assigned_to_current_scene
                && !cell.active_effective
                && !cell.overridden
        }));
    }

    #[test]
    fn launch_scene_with_empty_cell_silences_track_and_blanks_the_live_lane() {
        let state = make_state_with_tracks(1);
        state.clone_pattern(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );
        assert_eq!(state.current_scene_index(), 1);
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            assert!(scenes.clear_cell(0, 0).is_some());
        }
        state.pattern.patterns[0].set_step_active(3, true);
        state.pattern.track_params[0].set_num_steps(7);

        let sample_ids = state.launch_scene(
            0,
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        );

        assert!(sample_ids.is_some());
        assert!(state.is_scene_silenced(0));
        assert!(state.latest_scheduler_snapshot().tracks[0].scene_silenced);
        // An empty cell presents an EMPTY step grid (takes spec 11.1): the
        // previous scene's live notes must not show through. Track params
        // (length, timebase) stay — only note content is blanked.
        assert!(!state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.track_params[0].get_num_steps(), 7);
        assert_eq!(state.current_scene_index(), 0);
    }

    #[test]
    fn saving_scene_snapshot_preserves_empty_scene_cells() {
        let first = sample_pattern_snapshot(1);
        let mut scenes = ProjectScenes::from_pattern_snapshots(&[first], 0);
        let orphan = scenes.clear_cell(0, 0).unwrap();
        let mut live = sample_pattern_snapshot(1);
        live.track_bits[0][0] = 99;

        assert!(scenes.save_scene_snapshot(0, live));

        assert_eq!(scenes.scenes[0].cells[0], None);
        assert!(scenes.track_pools[0].contains(orphan));
        assert_eq!(
            scenes.track_pools[0].get(orphan).unwrap().track_bits[0],
            1,
            "capturing while an empty cell is active must not overwrite orphan data"
        );
    }

    #[test]
    fn install_project_arrangement_restores_bare_cells_and_take_pools() {
        // Two tracks, two scenes, freshly rebuilt as the project loader does:
        // per-track pool ids are 1..=2 with scene j's cell holding id j+1.
        let state = make_state_with_tracks(2);
        state.replace_pattern_repository(
            vec![snapshot_with_active_step(2, 0, 3), snapshot_with_active_step(2, 0, 5)],
            0,
        );

        // Simulated file data: scene 1's cell for track 1 was bare, and
        // track 0 owned one 300-step take of two chunks (stable id 3).
        let presence = vec![vec![true, true], vec![true, false]];
        let mut chunk_a = sample_pattern_snapshot(1).track_pattern_data(0).unwrap();
        chunk_a.track_bits[0] = 0b1010;
        let mut chunk_b = chunk_a.clone();
        chunk_b.track_bits[0] = 0b0111;
        state.install_project_arrangement(
            &presence,
            vec![
                (7, vec![(3, "Take 4".to_string(), 300, vec![chunk_a, chunk_b])]),
                (0, Vec::new()),
            ],
        );

        let scenes = state.pattern.scenes.lock().unwrap();
        // Bare cell restored: the loader-materialized pattern is gone.
        assert_eq!(scenes.scenes[1].cells[1], None);
        assert!(!scenes.track_pools[1].contains(PatternId(2)));
        assert!(scenes.track_pools[1].contains(PatternId(1)));
        // Take rebuilt with its stable id, chunk content, and allocator.
        let take = scenes.take_pools[0].get(TakeId(3)).expect("take restored");
        assert_eq!(take.name, "Take 4");
        assert_eq!(take.total_len_steps, 300);
        assert_eq!(take.chunks.len(), 2);
        let chunk_bits: Vec<u64> = take
            .chunks
            .iter()
            .map(|id| scenes.track_pools[0].get(*id).unwrap().track_bits[0])
            .collect();
        assert_eq!(chunk_bits, vec![0b1010, 0b0111]);
        assert_eq!(scenes.take_pools[0].next_take_id, 7);
        // Chunks are claimed (hidden from the grid) and scene cells for
        // track 0 are untouched.
        assert!(scenes.take_pools[0].is_claimed(take.chunks[0]));
        assert_eq!(scenes.scenes[0].cells[0], Some(PatternId(1)));
    }

    #[test]
    fn reorder_scene_moves_only_scene_references_and_follows_the_current_scene() {
        let first = snapshot_with_active_step(1, 0, 2);
        let second = snapshot_with_active_step(1, 0, 7);
        let third = snapshot_with_active_step(1, 0, 11);
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(vec![first, second, third], 1);

        let original_scene_ids = (0..3)
            .map(|scene| state.scene_track_pattern_id(scene, 0).unwrap())
            .collect::<Vec<_>>();
        let original_stable_scene_ids = {
            let scenes = state.pattern.scenes.lock().unwrap();
            scenes.scenes.iter().map(|scene| scene.id).collect::<Vec<_>>()
        };
        let pool_before = {
            let scenes = state.pattern.scenes.lock().unwrap();
            let mut entries = scenes.track_pools[0]
                .patterns
                .iter()
                .map(|(id, data)| (id.0, data.seq.track_bits))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(id, _)| *id);
            (entries, scenes.track_pools[0].next_id)
        };

        assert_eq!(state.reorder_scene(0, 2), Some(0));
        assert_eq!(state.current_scene_index(), 0);
        assert_eq!(
            state.pattern.scenes.lock().unwrap().scenes
                .iter()
                .map(|scene| scene.id)
                .collect::<Vec<_>>(),
            vec![
                original_stable_scene_ids[1],
                original_stable_scene_ids[2],
                original_stable_scene_ids[0],
            ],
            "stable scene identities must move with scenes rather than stay at an index"
        );
        assert_eq!(
            (0..3)
                .map(|scene| state.scene_track_pattern_id(scene, 0).unwrap())
                .collect::<Vec<_>>(),
            vec![
                original_scene_ids[1],
                original_scene_ids[2],
                original_scene_ids[0]
            ]
        );

        let pool_after = {
            let scenes = state.pattern.scenes.lock().unwrap();
            let mut entries = scenes.track_pools[0]
                .patterns
                .iter()
                .map(|(id, data)| (id.0, data.seq.track_bits))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(id, _)| *id);
            (entries, scenes.track_pools[0].next_id)
        };
        assert_eq!(
            pool_after, pool_before,
            "track pattern pools must be untouched"
        );

        assert_eq!(state.reorder_scene(2, 0), Some(1));
        assert_eq!(state.current_scene_index(), 1);
        assert_eq!(
            state.pattern.scenes.lock().unwrap().scenes
                .iter()
                .map(|scene| scene.id)
                .collect::<Vec<_>>(),
            original_stable_scene_ids
        );
        assert_eq!(
            (0..3)
                .map(|scene| state.scene_track_pattern_id(scene, 0).unwrap())
                .collect::<Vec<_>>(),
            original_scene_ids
        );
    }

    #[test]
    fn reorder_scene_rejects_out_of_range_indices_without_changes() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                snapshot_with_active_step(1, 0, 3),
                snapshot_with_active_step(1, 0, 9),
            ],
            0,
        );
        let ids_before = (0..2)
            .map(|scene| state.scene_track_pattern_id(scene, 0))
            .collect::<Vec<_>>();

        assert_eq!(state.reorder_scene(0, 2), None);
        assert_eq!(state.reorder_scene(2, 0), None);
        assert_eq!(state.current_scene_index(), 0);
        assert_eq!(
            (0..2)
                .map(|scene| state.scene_track_pattern_id(scene, 0))
                .collect::<Vec<_>>(),
            ids_before
        );
    }

    #[test]
    fn deleted_scene_identity_is_not_reused_by_a_new_scene() {
        let snapshots = vec![
            snapshot_with_active_step(1, 0, 3),
            snapshot_with_active_step(1, 0, 9),
        ];
        let mut scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
        let deleted = scenes.scene_id(1).unwrap();
        assert_eq!(scenes.delete_scene(1), Some(0));
        let new_scene = scenes.new_scene();
        assert_ne!(scenes.scene_id(new_scene), Some(deleted));
    }

    #[test]
    fn launch_scene_captures_live_edits_before_switching() {
        let state = make_state_with_tracks(2);
        let first = PatternSnapshot::new_default(2, &[]);
        let second = snapshot_with_active_step(2, 1, 7);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        state.pattern.patterns[0].set_step_active(5, true);
        let (buffer_ids, sample_rates, names, instrument_types) = launch_test_args();

        state
            .launch_scene(1, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(state.pattern.patterns[1].is_active(7));

        state
            .launch_scene(0, 2, &buffer_ids, &sample_rates, &names, &instrument_types)
            .unwrap();
        assert!(state.pattern.patterns[0].is_active(5));
    }

    fn populate_step(state: &SequencerState, track: usize, step: usize) {
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.neural_reset_patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Velocity, 0.75);
        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);
        state.pattern.timebase_plocks[track].set(step, Timebase::Eighth);
        state.pattern.effect_chains[track][0].set_plock(step, 0, 440.0);
        state.pattern.instrument_slots[track].set_plock(step, 0, 0.5);
    }

    fn assert_step_matches_populated(state: &SequencerState, track: usize, step: usize) {
        assert!(
            state.pattern.patterns[track].is_active(step),
            "step {step} should be active"
        );
        assert!(
            state.pattern.neural_reset_patterns[track].is_active(step),
            "step {step} should carry neural reset"
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Velocity),
            0.75
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            7.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 0.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 4.0);
        assert_eq!(
            state.pattern.timebase_plocks[track].get(step),
            Some(Timebase::Eighth)
        );
        assert_eq!(
            state.pattern.effect_chains[track][0].plocks.get(step, 0),
            Some(440.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[track].plocks.get(step, 0),
            Some(0.5)
        );
    }

    fn assert_step_is_default(state: &SequencerState, track: usize, step: usize) {
        assert!(
            !state.pattern.patterns[track].is_active(step),
            "step {step} should be inactive"
        );
        assert!(
            !state.pattern.neural_reset_patterns[track].is_active(step),
            "step {step} should not carry neural reset"
        );
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 0);
        assert_eq!(state.pattern.timebase_plocks[track].get(step), None);
        assert_eq!(
            state.pattern.effect_chains[track][0].plocks.get(step, 0),
            None
        );
        assert_eq!(
            state.pattern.instrument_slots[track].plocks.get(step, 0),
            None
        );
    }

    fn rack_slot_plock_value(
        state: &SequencerState,
        track: usize,
        slot_idx: usize,
        step: usize,
        param_idx: usize,
    ) -> Option<f32> {
        state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .and_then(|slot| slot.instrument_slot.plocks.get(step))
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
    }

    fn rack_slot_param_plock_value(
        state: &SequencerState,
        track: usize,
        slot_idx: usize,
        step: usize,
        param: RackSlotParam,
    ) -> Option<f32> {
        state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .and_then(|slot| slot.param_plocks.get(step, param))
    }

    #[test]
    fn step_snapshot_capture_clear_restore_preserves_rack_slot_instrument_plocks() {
        let state = make_state_with_rack_slot();
        let macro_id = RackMacroId::from_index(0).expect("macro 1 id");
        assert!(
            state.update_rack_macro_in_current_pattern(0, macro_id, |rack_macro| {
                rack_macro.plocks[2] = Some(0.25);
                rack_macro.plocks[4] = Some(0.75);
            })
        );
        {
            let mut racks = state.pattern.rack_tracks.lock().unwrap();
            let slot = &mut racks[0].as_mut().unwrap().slots[0];
            assert!(slot.param_plocks.set(2, RackSlotParam::Gain, 0.42));
            assert!(slot.param_plocks.set(4, RackSlotParam::Gain, 0.84));
            assert!(slot.instrument_slot.set_plock(2, 0, 0.42));
            assert!(slot.instrument_slot.set_plock(4, 0, 0.84));
        }

        let snap = state.capture_step_snapshot(0, 2);
        assert_eq!(
            snap.rack_slot_param_plocks[0].params[RackSlotParam::Gain.index()],
            Some(0.42)
        );
        assert_eq!(snap.rack_slot_instrument_plocks[0].params[0], Some(0.42));
        assert_eq!(snap.rack_macro_plocks[0], Some(0.25));

        state.clear_step_payload(0, 2);
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 2, RackSlotParam::Gain),
            None
        );
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 4, RackSlotParam::Gain),
            Some(0.84)
        );
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 2, 0), None);
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 4, 0), Some(0.84));
        assert_eq!(
            state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .macros[0]
                .plocks[2],
            None
        );

        state.restore_step_snapshot(0, 5, &snap);
        assert_eq!(
            rack_slot_param_plock_value(&state, 0, 0, 5, RackSlotParam::Gain),
            Some(0.42)
        );
        assert_eq!(rack_slot_plock_value(&state, 0, 0, 5, 0), Some(0.42));
        assert_eq!(
            state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .macros[0]
                .plocks[5],
            Some(0.25)
        );
    }

    #[test]
    fn rack_macro_plocks_participate_in_variant_stamp_and_clear() {
        let state = make_state_with_rack_slot();
        let macro_id = RackMacroId::from_index(0).expect("macro 1 id");
        assert!(
            state.update_rack_macro_in_current_pattern(0, macro_id, |rack_macro| {
                rack_macro.plocks[2] = Some(0.65);
            })
        );
        let key = crate::plock_variants::live_track_variant_key(&state, 0, 2)
            .expect("rack macro variant key");

        assert!(state.stamp_variant_key_to_steps(0, &key, &[5]));
        assert_eq!(
            state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .macros[0]
                .plocks[5],
            Some(0.65)
        );

        assert!(state.clear_variant_locks_for_steps(0, &[5]));
        assert_eq!(
            state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .macros[0]
                .plocks[5],
            None
        );
    }

    #[test]
    fn rack_macro_plock_batch_updates_live_and_current_pattern_atomically() {
        let state = make_state_with_rack_slot();
        let macro_id = RackMacroId::from_index(0).expect("macro 1 id");

        assert!(state.set_rack_macro_plocks_in_current_pattern(
            0,
            macro_id,
            &[1, 3, 5, MAX_STEPS],
            1.25,
        ));

        let live_plocks = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .macros[0]
            .plocks
            .clone();
        let scenes = state.pattern.scenes.lock().unwrap();
        let pattern_id = scenes.effective_pattern_id(0).expect("current pattern");
        let persisted = scenes.track_pools[0]
            .get(pattern_id)
            .expect("current pattern data");
        let persisted_plocks = &persisted
            .rack_track
            .as_ref()
            .expect("current rack")
            .macros[0]
            .plocks;

        assert_eq!(live_plocks, *persisted_plocks);
        for step in [1, 3, 5] {
            assert_eq!(live_plocks[step], Some(1.0));
        }
        assert!(live_plocks
            .iter()
            .enumerate()
            .all(|(step, value)| [1, 3, 5].contains(&step) || value.is_none()));
    }

    #[test]
    fn sampler_runtime_arrays_cover_rack_slot_pool_domain() {
        let state = make_state_with_tracks(1);
        let last_rack_pool = crate::sequencer::rack_slot_pool_index(
            MAX_TRACKS - 1,
            crate::sequencer::MAX_RACK_SLOTS - 1,
        )
        .expect("last rack slot pool should exist");
        assert_eq!(last_rack_pool + 1, MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.sampler_lids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.voice_counts.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.voice_lids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(state.runtime.synth_node_ids.len(), MAX_SAMPLER_POOLS);
        assert_eq!(
            state.runtime.sampler_analysis_status.len(),
            MAX_SAMPLER_POOLS
        );
        assert_eq!(
            state.runtime.sampler_onset_ptr_lo.len(),
            MAX_SAMPLER_POOLS
        );
        assert_eq!(
            state.runtime.sampler_onset_ptr_hi.len(),
            MAX_SAMPLER_POOLS
        );
        assert_eq!(
            state.runtime.sampler_gatepitch_node_ids.len(),
            MAX_SAMPLER_POOLS
        );
        assert_eq!(
            state.runtime.sampler_modulator_node_ids.len(),
            MAX_SAMPLER_POOLS
        );

        state.runtime.sampler_lids[last_rack_pool].store(123, Ordering::Relaxed);
        state.runtime.voice_counts[last_rack_pool].store(1, Ordering::Relaxed);
        state.runtime.voice_lids[last_rack_pool][0].store(456, Ordering::Relaxed);
        state.runtime.synth_node_ids[last_rack_pool][0].store(789, Ordering::Relaxed);
        state.runtime.sampler_gatepitch_node_ids[last_rack_pool][0].store(101, Ordering::Relaxed);
        state.runtime.sampler_modulator_node_ids[last_rack_pool][0].store(112, Ordering::Relaxed);

        assert_eq!(
            state.runtime.sampler_lids[last_rack_pool].load(Ordering::Relaxed),
            123
        );
    }

    #[test]
    fn remove_track_compacts_live_state_and_runtime_bindings() {
        let state = make_state_with_tracks(3);
        let names = vec!["kick".to_string(), "snare".to_string(), "hat".to_string()];
        let buffer_ids = vec![10, 20, 30];
        let instrument_types = vec![
            InstrumentType::Sampler,
            InstrumentType::Custom,
            InstrumentType::Sampler,
        ];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 3];

        for track in 0..3 {
            state.pattern.track_params[track].set_num_steps(8 + track);
            state.pattern.track_params[track].set_volume(0.2 * (track + 1) as f32);
            state.pattern.track_params[track].set_accumulator_idx(track);
            state.pattern.track_params[track]
                .set_script_accumulator_name(Some(format!("acc-{track}")));
            state.pattern.track_params[track].set_accum_limit(10.0 + track as f32);
            state.pattern.track_params[track].set_fts_scale(track + 1);
            state.pattern.track_params[track].set_mute_group(track as u8);
            state.pattern.patterns[track].set_step_active(track, true);
            state.pattern.step_data[track].set(
                track,
                StepParam::Velocity,
                0.1 * (track + 1) as f32,
            );
            state.pattern.chord_data[track].add_note(track, track as f32 + 0.5);
            state.pattern.timebase_plocks[track].set(track, Timebase::Eighth);
            state.pattern.swing_plocks[track].set(track, 55.0 + track as f32);
            state.pattern.swing_resolution_plocks[track].set(track, SwingResolution::Quarter);
            state.pattern.effect_chains[track][0]
                .node_id
                .store((100 + track) as u32, Ordering::Relaxed);
            state.pattern.effect_chains[track][0]
                .num_params
                .store(1, Ordering::Relaxed);
            state.pattern.effect_chains[track][0]
                .defaults
                .set(0, track as f32 + 1.0);
            state.pattern.effect_chains[track][0].set_plock(track, 0, 300.0 + track as f32);
            state.pattern.instrument_slots[track]
                .node_id
                .store((200 + track) as u32, Ordering::Relaxed);
            state.pattern.instrument_slots[track]
                .num_params
                .store(1, Ordering::Relaxed);
            state.pattern.instrument_slots[track].set_plock(track, 0, 0.25 + track as f32);
            state.pattern.instrument_base_note_offsets[track]
                .store((track as f32 + 12.0).to_bits(), Ordering::Relaxed);
            let run_mode = if track == 2 {
                CustomInstrumentRunMode::FreePatch
            } else {
                CustomInstrumentRunMode::Instrument
            };
            state.pattern.instrument_run_modes[track]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            state.pattern.track_sound_state.lock().unwrap()[track] = TrackSoundState {
                engine_id: Some(track),
                loaded_preset: Some(format!("preset-{track}")),
                dirty: track % 2 == 0,
            };

            state.transport.track_playheads[track].store((track * 4) as u32, Ordering::Relaxed);
            state.transport.trigger_flash[track].store((track * 10) as u32, Ordering::Relaxed);
            state.runtime.sampler_lids[track].store((track as u64) + 10, Ordering::Relaxed);
            state.runtime.pan_lids[track].store((track as u64) + 20, Ordering::Relaxed);
            state.runtime.delay_lids[track].store((track as u64) + 30, Ordering::Relaxed);
            state.runtime.send_lids[track].store((track as u64) + 40, Ordering::Relaxed);
            state.runtime.voice_counts[track].store((track + 1) as u32, Ordering::Relaxed);
            state.runtime.instrument_type_flags[track].store((track % 2) as u32, Ordering::Relaxed);
            state.runtime.instrument_run_mode_flags[track]
                .store(run_mode.runtime_flag(), Ordering::Relaxed);
            state.runtime.track_engine_ids[track].store((track as u32) + 50, Ordering::Relaxed);
            state.runtime.voice_lids[track][0].store((track as u64) + 60, Ordering::Relaxed);
            state.runtime.synth_node_ids[track][0].store((track as u32) + 70, Ordering::Relaxed);
            state.pending_accumulator_reset_tracks[track].store(track == 2, Ordering::Relaxed);
        }

        assert!(state.remove_track(
            1,
            &buffer_ids,
            &[44_100, 44_100, 44_100],
            &names,
            &instrument_types,
            &effect_descriptors
        ));

        assert_eq!(state.active_track_count(), 2);
        assert_eq!(state.pattern.track_params[1].get_num_steps(), 10);
        assert_eq!(state.pattern.track_params[1].get_volume(), 0.6);
        assert_eq!(state.pattern.track_params[1].get_accumulator_idx(), 2);
        assert_eq!(
            state.pattern.track_params[1]
                .script_accumulator_name()
                .as_deref(),
            Some("acc-2")
        );
        assert_eq!(state.pattern.track_params[1].get_accum_limit(), 12.0);
        assert_eq!(state.pattern.track_params[1].get_fts_scale(), 3);
        assert_eq!(state.pattern.track_params[1].get_mute_group(), 2);
        assert!(state.pattern.patterns[1].is_active(2));
        assert_eq!(state.pattern.step_data[1].get(2, StepParam::Velocity), 0.3);
        assert_eq!(state.pattern.chord_data[1].get(2, 0), 2.5);
        assert_eq!(
            state.pattern.timebase_plocks[1].get(2),
            Some(Timebase::Eighth)
        );
        assert_eq!(state.pattern.swing_plocks[1].get(2), Some(57.0));
        assert_eq!(
            state.pattern.swing_resolution_plocks[1].get(2),
            Some(SwingResolution::Quarter)
        );
        assert_eq!(
            state.pattern.effect_chains[1][0]
                .node_id
                .load(Ordering::Relaxed),
            102
        );
        assert_eq!(state.pattern.effect_chains[1][0].defaults.get(0), 3.0);
        assert_eq!(
            state.pattern.effect_chains[1][0].plocks.get(2, 0),
            Some(302.0)
        );
        assert_eq!(
            state.pattern.instrument_slots[1]
                .node_id
                .load(Ordering::Relaxed),
            202
        );
        assert_eq!(
            state.pattern.instrument_slots[1].plocks.get(2, 0),
            Some(2.25)
        );
        assert_eq!(
            f32::from_bits(state.pattern.instrument_base_note_offsets[1].load(Ordering::Relaxed)),
            14.0
        );
        assert_eq!(
            state.pattern.track_sound_state.lock().unwrap()[1]
                .loaded_preset
                .as_deref(),
            Some("preset-2")
        );
        assert_eq!(
            state.transport.track_playheads[1].load(Ordering::Relaxed),
            8
        );
        assert_eq!(state.transport.trigger_flash[1].load(Ordering::Relaxed), 20);
        assert_eq!(state.runtime.sampler_lids[1].load(Ordering::Relaxed), 12);
        assert_eq!(state.runtime.pan_lids[1].load(Ordering::Relaxed), 22);
        assert_eq!(state.runtime.delay_lids[1].load(Ordering::Relaxed), 32);
        assert_eq!(state.runtime.send_lids[1].load(Ordering::Relaxed), 42);
        assert_eq!(state.runtime.voice_counts[1].load(Ordering::Relaxed), 3);
        assert_eq!(
            state.runtime.instrument_type_flags[1].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[1].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::FreePatch
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.runtime.instrument_run_mode_flags[1].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::FreePatch
        );
        assert_eq!(
            state.runtime.track_engine_ids[1].load(Ordering::Relaxed),
            52
        );
        assert_eq!(state.runtime.voice_lids[1][0].load(Ordering::Relaxed), 62);
        assert_eq!(
            state.runtime.synth_node_ids[1][0].load(Ordering::Relaxed),
            72
        );
        assert!(state.pending_accumulator_reset_tracks[1].load(Ordering::Relaxed));
    }

    #[test]
    fn remove_track_clears_old_trailing_lane() {
        let state = make_state_with_tracks(3);
        let names = vec!["kick".to_string(), "snare".to_string(), "hat".to_string()];
        let buffer_ids = vec![10, 20, 30];
        let instrument_types = vec![InstrumentType::Sampler; 3];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 3];

        state.pattern.patterns[2].set_step_active(0, true);
        state.pattern.step_data[2].set(0, StepParam::Velocity, 0.9);
        state.pattern.chord_data[2].add_note(0, 7.0);
        state.pattern.timebase_plocks[2].set(0, Timebase::Quarter);
        state.pattern.swing_plocks[2].set(0, 60.0);
        state.pattern.swing_resolution_plocks[2].set(0, SwingResolution::Eighth);
        state.pattern.effect_chains[2][0]
            .node_id
            .store(999, Ordering::Relaxed);
        state.pattern.effect_chains[2][0]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.effect_chains[2][0].set_plock(0, 0, 123.0);
        state.pattern.instrument_slots[2]
            .node_id
            .store(888, Ordering::Relaxed);
        state.pattern.instrument_slots[2]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.instrument_slots[2].set_plock(0, 0, 0.75);
        state.pattern.instrument_run_modes[2].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.transport.track_playheads[2].store(12, Ordering::Relaxed);
        state.runtime.sampler_lids[2].store(77, Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[2].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[2].store(66, Ordering::Relaxed);

        assert!(state.remove_track(
            1,
            &buffer_ids,
            &[44_100, 44_100, 44_100],
            &names,
            &instrument_types,
            &effect_descriptors
        ));

        assert!(!state.pattern.patterns[2].is_active(0));
        assert_eq!(
            state.pattern.step_data[2].get(0, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.pattern.chord_data[2].count(0), 0);
        assert_eq!(state.pattern.timebase_plocks[2].get(0), None);
        assert_eq!(state.pattern.swing_plocks[2].get(0), None);
        assert_eq!(state.pattern.swing_resolution_plocks[2].get(0), None);
        assert_eq!(
            state.pattern.effect_chains[2][0]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.pattern.effect_chains[2][0].plocks.get(0, 0), None);
        assert_eq!(
            state.pattern.instrument_slots[2]
                .node_id
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.pattern.instrument_slots[2].plocks.get(0, 0), None);
        assert_eq!(
            state.transport.track_playheads[2].load(Ordering::Relaxed),
            0
        );
        assert_eq!(state.runtime.sampler_lids[2].load(Ordering::Relaxed), 0);
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[2].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::Instrument
        );
        assert_eq!(
            CustomInstrumentRunMode::from_runtime_flag(
                state.runtime.instrument_run_mode_flags[2].load(Ordering::Relaxed)
            ),
            CustomInstrumentRunMode::Instrument
        );
        assert_eq!(
            state.runtime.track_engine_ids[2].load(Ordering::Relaxed),
            u32::MAX
        );
    }

    #[test]
    fn pattern_restore_preserves_live_runtime_engine_binding() {
        let state = make_state_with_tracks(1);
        state.runtime.track_engine_ids[0].store(77, Ordering::Relaxed);
        state.pattern.track_sound_state.lock().unwrap()[0] = TrackSoundState {
            engine_id: Some(12),
            loaded_preset: Some("pad".to_string()),
            dirty: false,
        };

        let snapshot = PatternSnapshot::capture(
            &state,
            1,
            &[0],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Custom],
        );

        state.runtime.track_engine_ids[0].store(91, Ordering::Relaxed);
        snapshot.restore(&state);

        assert_eq!(
            state.runtime.track_engine_ids[0].load(Ordering::Relaxed),
            91
        );
        assert_eq!(
            state.pattern.track_sound_state.lock().unwrap()[0].engine_id,
            Some(77)
        );
    }

    #[test]
    fn pattern_restore_track_only_changes_requested_track() {
        let state = make_state_with_tracks(2);
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.patterns[1].set_step_active(1, true);
        state.pattern.step_data[0].set(0, StepParam::Duration, 2.0);
        state.pattern.step_data[1].set(0, StepParam::Duration, 3.0);
        state.pattern.track_params[0].set_num_steps(5);
        state.pattern.track_params[1].set_num_steps(6);

        let mut snapshot = PatternSnapshot::new_default(2, &[]);
        snapshot.track_bits[1][0] = 1 << 7;
        snapshot.step_data[1][0][StepParam::Duration.index()] = 9.0;
        snapshot.track_params[1].num_steps = 12;
        snapshot.instrument_base_note_offsets[1] = 7.0;
        snapshot.timebase_plock_snapshots[1][0] = Some(Timebase::Eighth as u32);

        assert!(snapshot.restore_track(&state, 1));

        assert!(state.pattern.patterns[0].is_active(0));
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Duration), 2.0);
        assert_eq!(state.pattern.track_params[0].get_num_steps(), 5);

        assert!(!state.pattern.patterns[1].is_active(1));
        assert!(state.pattern.patterns[1].is_active(7));
        assert_eq!(state.pattern.step_data[1].get(0, StepParam::Duration), 9.0);
        assert_eq!(state.pattern.track_params[1].get_num_steps(), 12);
        assert_eq!(
            f32::from_bits(state.pattern.instrument_base_note_offsets[1].load(Ordering::Relaxed)),
            7.0
        );
        assert_eq!(
            state.pattern.timebase_plocks[1].get(0),
            Some(Timebase::Eighth)
        );
    }

    #[test]
    fn set_step_param_transpose_shifts_chord_notes() {
        let state = make_state_with_instrument();
        let track = 0;
        let step = 2;

        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);

        state.set_step_param(track, step, StepParam::Transpose, 10.0);

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            10.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 3.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 7.0);
    }

    #[test]
    fn adjust_step_param_transpose_shifts_chord_notes() {
        let state = make_state_with_instrument();
        let track = 0;
        let step = 2;

        state.pattern.step_data[track].set(step, StepParam::Transpose, 7.0);
        state.pattern.chord_data[track].add_note(step, 0.0);
        state.pattern.chord_data[track].add_note(step, 4.0);

        state.adjust_step_param(track, step, StepParam::Transpose, -2.0);

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            5.0
        );
        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), -2.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 2.0);
    }

    // ── copy / paste (capture_step_snapshot + restore_step_snapshot) ──

    #[test]
    fn copy_paste_preserves_all_fields() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 2);

        let snap = state.capture_step_snapshot(0, 2);
        state.restore_step_snapshot(0, 5, &snap);

        assert_step_matches_populated(&state, 0, 5);
        // Source step is unchanged
        assert_step_matches_populated(&state, 0, 2);
    }

    #[test]
    fn copy_paste_multi_step_with_offsets() {
        // Simulates Ctrl+C on steps 1,2 then Ctrl+V at step 4.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 1);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.3);

        let anchor = 1usize;
        let clipboard: Vec<(usize, StepSnapshot)> = [1usize, 2]
            .iter()
            .map(|&s| (s - anchor, state.capture_step_snapshot(0, s)))
            .collect();

        let dest_start = 4usize;
        for (offset, snap) in &clipboard {
            state.restore_step_snapshot(0, dest_start + offset, snap);
        }

        // Step 4 (offset 0) should match original step 1
        assert_step_matches_populated(&state, 0, 4);
        // Step 5 (offset 1) should match original step 2
        assert!(state.pattern.patterns[0].is_active(5));
        assert_eq!(state.pattern.step_data[0].get(5, StepParam::Velocity), 0.3);
    }

    #[test]
    fn paste_inactive_snapshot_over_active_step_preserves_existing() {
        // An "empty" snapshot must not overwrite an active step.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 3);

        let empty_snap = state.capture_step_snapshot(0, 7); // step 7 is default/inactive
        assert!(!empty_snap.active);

        // Simulate the paste guard from Ctrl+V: skip if snapshot inactive and dest active
        let dest = 3usize;
        if !empty_snap.active && state.pattern.patterns[0].is_active(dest) {
            // correctly skipped
        } else {
            state.restore_step_snapshot(0, dest, &empty_snap);
            panic!("should not overwrite active step with empty snapshot");
        }

        assert_step_matches_populated(&state, 0, 3);
    }

    #[test]
    fn paste_active_snapshot_over_empty_step_writes_data() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 1);

        let snap = state.capture_step_snapshot(0, 1);
        assert!(snap.active);

        // Dest step 5 is empty — paste guard should allow the write
        let dest = 5usize;
        assert!(!state.pattern.patterns[0].is_active(dest));
        // Guard passes (snap.active == true), so we restore
        state.restore_step_snapshot(0, dest, &snap);

        assert_step_matches_populated(&state, 0, 5);
    }

    #[test]
    fn paste_out_of_bounds_offsets_are_skipped() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 0);
        let ns = state.pattern.track_params[0].get_num_steps(); // 8

        let snap = state.capture_step_snapshot(0, 0);
        // dest_start=6, offsets 0..4 → destinations 6,7,8,9; 8 and 9 exceed ns
        let dest_start = 6usize;
        for offset in 0..4 {
            let dest = dest_start + offset;
            if dest >= ns {
                continue; // bounds guard — no write, no panic
            }
            state.restore_step_snapshot(0, dest, &snap);
        }

        assert!(state.pattern.patterns[0].is_active(6));
        assert!(state.pattern.patterns[0].is_active(7));
    }

    // ── rotate_steps ──

    #[test]
    fn rotate_steps_left_wraps_first_to_last() {
        // A B C _ at steps 0,1,2,3  →  B C _ A
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 1.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 2.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 3.0);
        // step 3 stays empty

        state.rotate_steps(0, &[0, 1, 2, 3], -1);

        assert!(state.pattern.patterns[0].is_active(0));
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Transpose), 2.0);
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 3.0);
        assert!(!state.pattern.patterns[0].is_active(2)); // formerly empty step 3
        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Transpose), 1.0);
    }

    #[test]
    fn rotate_steps_right_wraps_last_to_first() {
        // A B C _ at steps 0,1,2,3  →  _ A B C
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 1.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 2.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 3.0);
        // step 3 stays empty

        state.rotate_steps(0, &[0, 1, 2, 3], 1);

        assert!(!state.pattern.patterns[0].is_active(0)); // formerly empty step 3
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Transpose), 1.0);
        assert!(state.pattern.patterns[0].is_active(2));
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Transpose), 2.0);
        assert!(state.pattern.patterns[0].is_active(3));
        assert_eq!(state.pattern.step_data[0].get(3, StepParam::Transpose), 3.0);
    }

    #[test]
    fn rotate_steps_preserves_plocks_and_chords() {
        // step 0 has full data; step 1 is empty. Rotate left: step 1 gets step 0's data.
        let state = make_state_with_instrument();
        populate_step(&state, 0, 0);

        state.rotate_steps(0, &[0, 1], -1);

        assert_step_is_default(&state, 0, 0);
        assert_step_matches_populated(&state, 0, 1);
    }

    #[test]
    fn rotate_steps_two_left_equals_rotate_by_two() {
        // A B C → (left) → B C A → (left) → C A B
        let state = make_state_with_instrument();
        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 10.0);
        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 20.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 30.0);

        state.rotate_steps(0, &[0, 1, 2], -1);
        state.rotate_steps(0, &[0, 1, 2], -1);

        assert_eq!(
            state.pattern.step_data[0].get(0, StepParam::Transpose),
            30.0
        );
        assert_eq!(
            state.pattern.step_data[0].get(1, StepParam::Transpose),
            10.0
        );
        assert_eq!(
            state.pattern.step_data[0].get(2, StepParam::Transpose),
            20.0
        );
    }

    // ── clear_step_payload ──

    #[test]
    fn clear_step_payload_removes_all_data_including_plocks() {
        let state = make_state_with_instrument();
        populate_step(&state, 0, 3);

        state.clear_step_payload(0, 3);

        assert_step_is_default(&state, 0, 3);
    }

    #[test]
    fn clear_step_payload_on_inactive_step_is_safe() {
        let state = make_state_with_instrument();
        // step 4 was never populated — clearing it should not panic
        state.clear_step_payload(0, 4);
        assert_step_is_default(&state, 0, 4);
    }

    #[test]
    fn published_scheduler_snapshot_reflects_initial_state() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );

        let snapshot = state.latest_scheduler_snapshot();

        assert_eq!(state.scheduler_snapshot_version(), 1);
        assert_eq!(snapshot.transport.num_tracks, 2);
        assert_eq!(snapshot.transport.bpm, DEFAULT_BPM);
        assert_eq!(snapshot.tracks.len(), 2);
        assert_eq!(snapshot.tracks[0].params.num_steps, 16);
    }

    #[test]
    fn transport_starts_stopped_and_toggle_resets_playheads() {
        let state = SequencerState::new(2, vec![default_empty_effect_chain()]);

        assert!(!state.is_playing());
        assert!(!state.latest_scheduler_snapshot().transport.playing);

        state.transport.playhead.store(9, Ordering::Relaxed);
        state.transport.track_playheads[0].store(3, Ordering::Relaxed);
        state.transport.track_playheads[1].store(7, Ordering::Relaxed);
        state.transport.sampler_playheads[0].store(0.5_f32.to_bits(), Ordering::Relaxed);

        assert!(state.toggle_play());
        assert!(state.is_playing());
        assert!(state.latest_scheduler_snapshot().transport.playing);
        assert_eq!(state.transport.playhead.load(Ordering::Relaxed), 0);
        assert_eq!(
            state.transport.track_playheads[0].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            state.transport.track_playheads[1].load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            f32::from_bits(state.transport.sampler_playheads[0].load(Ordering::Relaxed)),
            0.0
        );

        state.transport.playhead.store(4, Ordering::Relaxed);
        state.transport.track_playheads[0].store(2, Ordering::Relaxed);

        assert!(!state.toggle_play());
        assert!(!state.is_playing());
        assert!(!state.latest_scheduler_snapshot().transport.playing);
        assert_eq!(state.transport.playhead.load(Ordering::Relaxed), 0);
        assert_eq!(
            state.transport.track_playheads[0].load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn published_scheduler_snapshot_updates_on_step_mutation() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let before = state.scheduler_snapshot_version();

        state.toggle_step_and_clear_plocks(0, 3);
        state.set_step_param(0, 3, StepParam::Transpose, 9.0);

        let snapshot = state.latest_scheduler_snapshot();
        assert!(state.scheduler_snapshot_version() > before);
        assert!(snapshot.tracks[0].steps[3].active);
        assert_eq!(
            snapshot.tracks[0].steps[3].params[StepParam::Transpose.index()],
            9.0
        );
    }

    #[test]
    fn publish_scheduler_track_recaptures_complete_track_and_reuses_other_tracks() {
        let state = SequencerState::new(
            2,
            vec![
                vec![EffectSlotState::new(&EffectDescriptor::builtin_filter(), 1)],
                default_empty_effect_chain(),
            ],
        );
        state.pattern.effect_chains[0][0]
            .apply_descriptor(&EffectDescriptor::builtin_filter(), 1);
        state.pattern.instrument_slots[0]
            .apply_descriptor(&EffectDescriptor::builtin_delay(), 2);
        let before = state.latest_scheduler_snapshot();

        state.pattern.patterns[0].set_step_active(3, true);
        state.pattern.effect_chains[0][0].set_plock(3, 0, 0.75);
        state.pattern.instrument_slots[0].set_plock(3, 0, 0.25);
        state.publish_scheduler_track(0);

        let after = state.latest_scheduler_snapshot();
        assert!(!Arc::ptr_eq(&before.tracks[0], &after.tracks[0]));
        assert!(Arc::ptr_eq(&before.tracks[1], &after.tracks[1]));
        assert!(after.tracks[0].steps[3].active);
        assert_eq!(after.tracks[0].effect_slots[0].plocks[3][0], Some(0.75));
        assert_eq!(after.tracks[0].instrument_slot.plocks[3][0], Some(0.25));
    }

    /// Bead eseq-sj01: `start_playback`/`stop_playback`/`toggle_play` mutate
    /// transport atomics only, so the published tracks are unchanged and their
    /// `Arc`s must be reused — that removes a whole-project deep capture here
    /// AND the matching whole-project deep free on whichever thread drops last.
    #[test]
    fn publish_transport_only_reuses_the_published_track_arcs() {
        let state = make_state_with_tracks(3);
        let before = state.latest_scheduler_snapshot();

        state.transport.bpm.store(140, Ordering::Relaxed);
        state.transport.playing.store(true, Ordering::Relaxed);
        state.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        let after = state.publish_transport_only();

        assert!(!Arc::ptr_eq(&before, &after), "a new snapshot is published");
        assert_eq!(before.tracks.len(), after.tracks.len());
        for (index, (old_track, new_track)) in
            before.tracks.iter().zip(after.tracks.iter()).enumerate()
        {
            assert!(
                Arc::ptr_eq(old_track, new_track),
                "track {index} must be reused, not recaptured"
            );
        }
        assert_eq!(after.transport.bpm, 140);
        assert!(after.transport.playing);
        assert_eq!(
            after.transport.pattern_epoch,
            before.transport.pattern_epoch + 1
        );
        assert!(Arc::ptr_eq(&after, &state.latest_scheduler_snapshot()));
    }

    /// The same guard `publish_scheduler_track` uses: a published track vector
    /// that disagrees with the live track count is stale and must never be
    /// republished as current.
    #[test]
    fn publish_transport_only_falls_back_to_a_full_capture_on_a_track_count_change() {
        let state = make_state_with_tracks(2);
        let before = state.latest_scheduler_snapshot();
        assert_eq!(before.tracks.len(), 2);

        state.transport.num_tracks.store(1, Ordering::Release);
        let after = state.publish_transport_only();

        assert_eq!(after.tracks.len(), 1, "the fallback recaptured the tracks");
        assert_eq!(after.transport.num_tracks, 1);
        assert!(!Arc::ptr_eq(&before.tracks[0], &after.tracks[0]));
    }

    /// Bead eseq-sj01: `coalesce_publishes` folds a multi-step transition into
    /// one publication that lands after every mutation the scope performed.
    #[test]
    fn coalesce_publishes_emits_one_publication_after_the_scope() {
        let state = make_state_with_tracks(1);
        let before_version = state.scheduler_snapshot_version();

        state.coalesce_publishes(|| {
            state.transport.bpm.store(90, Ordering::Relaxed);
            state.publish_scheduler_snapshot();
            assert_eq!(
                state.scheduler_snapshot_version(),
                before_version,
                "nothing publishes while the scope is open"
            );
            state.transport.bpm.store(91, Ordering::Relaxed);
            state.publish_scheduler_snapshot();
            state.coalesce_publishes(|| {
                state.transport.bpm.store(92, Ordering::Relaxed);
                state.publish_scheduler_snapshot();
            });
            assert_eq!(state.scheduler_snapshot_version(), before_version);
        });

        assert_eq!(state.scheduler_snapshot_version(), before_version + 1);
        assert_eq!(state.latest_scheduler_snapshot().transport.bpm, 92);
    }

    /// A scope that publishes nothing must not manufacture a publication.
    #[test]
    fn coalesce_publishes_is_a_no_op_without_a_publish() {
        let state = make_state_with_tracks(1);
        let before_version = state.scheduler_snapshot_version();
        state.coalesce_publishes(|| {});
        assert_eq!(state.scheduler_snapshot_version(), before_version);
    }

    /// Every publication reaches the audio thread through the realtime handoff
    /// ring, not through `latest_scheduler_snapshot`'s mutex (bead eseq-sj01).
    #[test]
    fn publishing_hands_the_snapshot_to_the_realtime_ring() {
        let state = make_state_with_tracks(1);
        let mut current = Arc::new(SequencerSnapshot::empty());
        let mut version = state.scheduler_snapshot_version();
        // Drain the publications made during construction.
        state.snapshot_handoff().refresh(&mut current, &mut version);

        state.transport.bpm.store(155, Ordering::Relaxed);
        let published = state.publish_scheduler_snapshot();

        assert!(state.snapshot_handoff().refresh(&mut current, &mut version));
        assert!(Arc::ptr_eq(&current, &published));
        assert_eq!(version, state.scheduler_snapshot_version());
    }

    #[test]
    fn publish_scheduler_snapshot_captures_transport_changes() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);

        state.transport.bpm.store(172, Ordering::Relaxed);
        state.publish_scheduler_snapshot();

        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.transport.bpm, 172);
    }

    #[test]
    fn publish_scheduler_snapshot_captures_current_pattern_mod_connections() {
        let state = make_state_with_tracks(2);
        let route = ModConnection {
            source_track: 0,
            destination: ModDestination::Track(1),
            dest_input: 2,
        };
        state
            .edit_current_mod_connections(|routes| {
                routes.push(route);
                Ok(())
            })
            .unwrap();

        state.publish_scheduler_snapshot();

        let snapshot = state.latest_scheduler_snapshot();
        assert_eq!(snapshot.mod_connections, vec![route]);
    }

    #[test]
    fn accumulator_reset_requests_are_consumed_once() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );

        state.request_accumulator_reset(1);
        state.request_all_accumulator_resets();

        let (all, tracks) = state.take_accumulator_reset_requests();
        assert!(all);
        assert!(tracks[1]);

        let (all_again, tracks_again) = state.take_accumulator_reset_requests();
        assert!(!all_again);
        assert!(!tracks_again[1]);
    }

    #[test]
    fn default_empty_effect_chain_has_no_builtin_nodes() {
        let chain = default_empty_effect_chain();
        assert_eq!(chain.len(), crate::lisp_host::MAX_CUSTOM_FX);
        for slot in chain {
            assert_eq!(slot.node_id.load(Ordering::Relaxed), 0);
            assert_eq!(slot.num_params.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn rack_slot_history_snapshot_restores_every_pattern_with_live_binding_ids() {
        let state = SequencerState::new(1, Vec::new());
        state.set_rack_track_for_all_pattern_snapshots(0, sample_rack_track_snapshot());
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            let source_id = scenes.effective_pattern_id(0).unwrap();
            let clone = scenes.track_pools[0].get(source_id).unwrap();
            scenes.track_pools[0].insert(clone);
        }
        state.append_rack_slot_for_all_pattern_snapshots(
            0,
            sample_sampler_rack_slot(12, "before", 48_000, 77),
        );
        let snapshot = state.capture_rack_slot_pattern_state(0, 1).unwrap();
        assert!(state.replace_rack_slot_source_in_current_pattern(
            0,
            1,
            sample_sampler_rack_slot(13, "after", 44_100, 88),
        ));
        state
            .restore_rack_slot_pattern_state(
                0,
                &snapshot,
                &EffectDescriptor::builtin_sampler(),
                901,
                902,
            )
            .unwrap();

        let live = state.pattern.rack_tracks.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .slots[1]
            .clone();
        assert_eq!(live.sample_id.as_ref().unwrap().1, "before");
        assert_eq!(live.instrument_slot.node_id, 901);
        let scenes = state.pattern.scenes.lock().unwrap();
        let pool = &scenes.track_pools[0];
        assert!(pool.patterns.values().all(|data| {
            let patch = pool.sounds.patches.get(&data.sound.patch).unwrap();
            let slot = &patch.rack_track.as_ref().unwrap().slots[1];
            slot.sample_id.as_ref().unwrap().1 == "before"
                && slot.instrument_slot.node_id == 901
                && slot.instrument_slot.modulator_node_id == 902
        }));
    }

    #[test]
    fn instrument_pattern_snapshot_restores_all_patterns_live_state_and_neural_overrides() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        {
            let mut scenes = state.pattern.scenes.lock().unwrap();
            let first_id = scenes.effective_pattern_id(0).unwrap();
            let mut first = scenes.track_pools[0].get(first_id).unwrap();
            first.instrument_base_note_offset = 11.0;
            let mut second = first.clone();
            second.instrument_base_note_offset = 22.0;
            let mut third = first.clone();
            third.instrument_base_note_offset = 33.0;
            assert!(scenes.track_pools[0].store(first_id, first));
            scenes.track_pools[0].insert(second);
            scenes.track_pools[0].insert(third);
            let mut network = crate::neural::ProjectNeuralNetwork::default();
            network.neurons[0].output_overrides.instrument.push(
                crate::neural::ProjectParamOverride {
                    target_track: 0,
                    param_id: crate::neural::ParamNodeId {
                        logical_id: 7,
                        node_param_idx: 0,
                    },
                    param_index: 0,
                    value: 0.625,
                },
            );
            scenes.scenes[0].neural_networks.push(network);
        }
        state.pattern.instrument_base_note_offsets[0]
            .store(99.0_f32.to_bits(), Ordering::Relaxed);

        let snapshot = state.capture_track_instrument_pattern_state(0).unwrap();
        let descriptor = EffectDescriptor::builtin_sampler();
        state
            .reset_sampler_slot_all_patterns(
                0,
                &descriptor,
                42,
                43,
                (9, "replacement".to_string(), 48_000),
            )
            .unwrap();
        state
            .restore_track_instrument_pattern_state(0, &snapshot, &descriptor, 42, 43)
            .unwrap();

        assert_eq!(
            f32::from_bits(
                state.pattern.instrument_base_note_offsets[0].load(Ordering::Relaxed)
            ),
            99.0
        );
        let scenes = state.pattern.scenes.lock().unwrap();
        let pool = &scenes.track_pools[0];
        let mut offsets = pool
            .patterns
            .values()
            .map(|pattern| {
                pool.sounds
                    .patches
                    .get(&pattern.sound.patch)
                    .unwrap()
                    .instrument_base_note_offset
            })
            .collect::<Vec<_>>();
        offsets.sort_by(f32::total_cmp);
        // The 0.0 is the track-sound carrier, untouched by the snapshot
        // restore (it is not part of the captured pattern set).
        assert_eq!(offsets, vec![0.0, 11.0, 22.0, 33.0]);
        let restored = &scenes.scenes[0].neural_networks[0].neurons[0]
            .output_overrides
            .instrument[0];
        assert_eq!(restored.value, 0.625);
        assert_eq!(restored.param_id.logical_id, 42);
    }

    fn song_row(
        id: u64,
        start_beat: f64,
        scene: usize,
        overrides: &[(usize, u64)],
    ) -> ProjectSongRow {
        ProjectSongRow {
            id: SongRowId(id),
            start_beat,
            scene: Some(scene),
            overrides: overrides
                .iter()
                .map(|&(track, pattern_id)| ProjectSongTrackOverride::new(track, Some(pattern_id)))
                .collect(),
        }
    }

    #[test]
    fn remap_song_overrides_after_track_delete_drops_and_shifts() {
        let mut song = ProjectSong {
            rows: vec![
                song_row(0, 0.0, 0, &[(0, 1), (1, 2), (2, 3)]),
                song_row(1, 8.0, 1, &[(2, 1)]),
            ],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 2,
        };
        remap_song_overrides_after_track_delete(&mut song, 1);
        assert_eq!(
            song.rows[0].overrides,
            vec![
                ProjectSongTrackOverride::new(0, Some(1)),
                ProjectSongTrackOverride::new(1, Some(3)),
            ]
        );
        assert_eq!(
            song.rows[1].overrides,
            vec![ProjectSongTrackOverride::new(1, Some(1))]
        );
    }

    #[test]
    fn remap_song_overrides_after_track_delete_normalizes_identical_rows() {
        // Rows differ only by the deleted track's override, so removing it
        // must fold the later row into the earlier one (earlier id wins).
        let mut song = ProjectSong {
            rows: vec![
                song_row(0, 0.0, 0, &[(0, 1)]),
                song_row(1, 8.0, 0, &[(0, 1), (1, 2)]),
                song_row(2, 16.0, 1, &[]),
            ],
            end_beat: 32.0,
            loop_enabled: false,
            next_row_id: 3,
        };
        remap_song_overrides_after_track_delete(&mut song, 1);
        let ids: Vec<u64> = song.rows.iter().map(|row| row.id.0).collect();
        assert_eq!(ids, vec![0, 2]);
        assert_eq!(song.next_row_id, 3);
    }

    #[test]
    fn remap_song_overrides_after_track_move_permutes_and_resorts() {
        let mut song = ProjectSong {
            rows: vec![song_row(0, 0.0, 0, &[(0, 1), (2, 3), (3, 4)])],
            end_beat: 8.0,
            loop_enabled: false,
            next_row_id: 1,
        };
        // Move track 3 to index 1 (the undo-restore append-then-move path).
        remap_song_overrides_after_track_move(&mut song, 3, 1);
        assert_eq!(
            song.rows[0].overrides,
            vec![
                ProjectSongTrackOverride::new(0, Some(1)),
                ProjectSongTrackOverride::new(1, Some(4)),
                ProjectSongTrackOverride::new(3, Some(3)),
            ]
        );
        // Moving it back restores the original assignment.
        remap_song_overrides_after_track_move(&mut song, 1, 3);
        assert_eq!(
            song.rows[0].overrides,
            vec![
                ProjectSongTrackOverride::new(0, Some(1)),
                ProjectSongTrackOverride::new(2, Some(3)),
                ProjectSongTrackOverride::new(3, Some(4)),
            ]
        );
    }

    #[test]
    fn remove_track_remaps_committed_song_overrides() {
        let state = SequencerState::new(3, vec![vec![], vec![], vec![]]);
        state.set_committed_song(Some(ProjectSong {
            rows: vec![song_row(0, 0.0, 0, &[(0, 1), (1, 1), (2, 1)])],
            end_beat: 8.0,
            loop_enabled: false,
            next_row_id: 1,
        }));
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(state.remove_track(
            1,
            &[-1, -1, -1],
            &[44_100, 44_100, 44_100],
            &names,
            &[InstrumentType::Sampler; 3],
            &[Vec::new(), Vec::new(), Vec::new()],
        ));
        let song = state.committed_song().expect("song survives track delete");
        assert_eq!(
            song.rows[0].overrides,
            vec![
                ProjectSongTrackOverride::new(0, Some(1)),
                ProjectSongTrackOverride::new(1, Some(1)),
            ]
        );
    }

    #[test]
    fn song_row_explicit_empty_lane_never_destroys_the_scene_pattern() {
        // Regression: apply_song_row saves the live snapshot into the
        // current scene before applying each row. A lane silenced by an
        // explicit-empty override must keep its live content — blanking it
        // would be written back over the scene cell's real pattern by the
        // next row application.
        let state = make_state_with_tracks(1);
        state.pattern.patterns[0].set_step_active(3, true);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        ));

        // Row A: same scene, explicit-empty override for track 0.
        state
            .apply_song_row(
                0,
                &[(0, None)],
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
                true,
            )
            .expect("row A applies");
        assert!(state.is_scene_silenced(0));

        // Row B: same scene, no overrides — saves current state first, then
        // restores the scene cell's pattern.
        state
            .apply_song_row(
                0,
                &[],
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
                true,
            )
            .expect("row B applies");
        assert!(!state.is_scene_silenced(0));
        assert!(
            state.pattern.patterns[0].is_active(3),
            "the scene pattern survives an explicit-empty row"
        );
        let scenes = state.pattern.scenes.lock().unwrap();
        let cell = scenes.scenes[0].cells[0].expect("scene cell");
        let data = scenes.track_pools[0].get(cell).expect("pool pattern");
        assert!(
            data.track_bits[0] & (1 << 3) != 0,
            "the pool pattern keeps its step content"
        );
    }

    #[test]
    fn song_row_take_override_never_reaches_the_session_surface() {
        // Takes spec 11.2: take chunks are invisible outside the timeline.
        // A song row whose lane resolves to a take chunk (preflight-expanded
        // take lanes carry the chunk's PatternId) must NOT paint the chunk
        // into the live grid or the session override slot — otherwise the
        // step sequencer shows the take at MAX_STEPS, and the mirror's
        // save-back writes take content over pool patterns (or mints one
        // for a bare track).
        let state = make_state_with_tracks(1);
        state.pattern.patterns[0].set_step_active(3, true);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &[String::from("track")],
            &[InstrumentType::Sampler],
        ));
        let scene_steps = state.pattern.track_params[0].get_num_steps();

        // Register a take whose single chunk is MAX_STEPS wide with step 0.
        let mut chunk = PatternSnapshot::new_default(1, &[])
            .track_pattern_data(0)
            .expect("chunk template");
        chunk.track_params.num_steps = MAX_STEPS;
        chunk.track_bits[0] |= 1;
        let take_id = state
            .register_track_take(0, None, vec![chunk], 16, None)
            .expect("take registers");
        let chunk_id = state.track_takes(0)[0].chunks[0];

        state
            .apply_song_row(
                0,
                &[(0, Some(chunk_id))],
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
                true,
            )
            .expect("take row applies");
        assert!(
            !state.is_scene_silenced(0),
            "a take lane is audibly playing — never silenced"
        );
        assert!(
            state.pattern.patterns[0].is_active(3),
            "the live grid keeps the scene pattern, not the chunk"
        );
        assert!(
            !state.pattern.patterns[0].is_active(0),
            "the chunk's content never paints the live grid"
        );
        assert_eq!(
            state.pattern.track_params[0].get_num_steps(),
            scene_steps,
            "the live pattern length must not inherit the chunk's MAX_STEPS"
        );
        {
            let scenes = state.pattern.scenes.lock().unwrap();
            assert_eq!(
                scenes.track_overrides[0], None,
                "the session override slot never holds a take chunk"
            );
        }
        // The mixer clip grid must not mark the scene's clip "playing"
        // while the lane is actually playing the take (takes spec 11.2).
        assert_eq!(state.song_take_lane_mask(), 1);
        assert!(
            state
                .track_pattern_cells(0)
                .iter()
                .all(|cell| !cell.active_effective),
            "no grid clip is active while the take plays"
        );

        // The next row's save-back must not leak take content into the pool.
        state
            .apply_song_row(
                0,
                &[],
                1,
                &[-1],
                &[44_100],
                &[String::from("track")],
                &[InstrumentType::Sampler],
                true,
            )
            .expect("follow-up row applies");
        let scenes = state.pattern.scenes.lock().unwrap();
        let cell = scenes.scenes[0].cells[0].expect("scene cell");
        let data = scenes.track_pools[0].get(cell).expect("pool pattern");
        assert!(
            data.track_bits[0] & (1 << 3) != 0,
            "the scene pattern keeps its own content"
        );
        assert_ne!(
            data.track_params.num_steps as usize, MAX_STEPS,
            "the scene pattern must not inherit the take's length"
        );
        let take_chunk = scenes.track_pools[0].get(chunk_id).expect("chunk survives");
        assert!(
            take_chunk.track_bits[0] & 1 == 1,
            "the take chunk itself is untouched"
        );
        assert_eq!(scenes.take_pools[0].takes.len(), 1);
        assert_eq!(scenes.take_pools[0].takes[0].id, take_id);
        drop(scenes);
        // The follow-up row plays the scene clip again: the grid marker
        // returns and the take-lane bit clears.
        assert_eq!(state.song_take_lane_mask(), 0);
        assert!(
            state
                .track_pattern_cells(0)
                .iter()
                .any(|cell| cell.active_effective),
            "the scene clip is active again once the take row ends"
        );
    }

    #[test]
    fn new_lane_in_captured_rows_gets_free_run_offset_stamps() {
        // An arrangement captured at unquantized beats predates a new
        // track. When the track gains its scene pattern, rows referencing
        // that scene must stamp the free-run phase for the new lane
        // (takes spec 7.2/9.4) so it plays in time; grid-aligned rows and
        // other scenes stay untouched.
        let state = SequencerState::new(2, vec![vec![], vec![]]);
        state.with_scenes_mut(|scenes| {
            let snapshots = vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ];
            *scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
        });
        state.set_committed_song(Some(ProjectSong {
            rows: vec![
                song_row(0, 0.0, 0, &[]),
                // Unquantized captured row: 4.7 beats = step 18.8 of a
                // 16-step/4-beat pattern -> free-run offset 2.8.
                song_row(1, 4.7, 1, &[]),
                song_row(2, 9.3, 0, &[]),
                // Grid-aligned row: free-run offset 0, no override needed.
                song_row(3, 12.0, 0, &[]),
            ],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 4,
        }));
        // Adding a track materializes a pattern in EVERY scene and stamps
        // the off-grid song rows of each scene with that scene's pattern.
        state.extend_all_pattern_snapshots_to_track(
            3,
            &[],
            2,
            CustomInstrumentRunMode::Instrument,
            None,
        )
        .unwrap();
        let (scene0_pattern, scene1_pattern) = state.with_project_scenes(|scenes| {
            (
                scenes.scenes[0].cells[2].expect("scene 0 cell"),
                scenes.scenes[1].cells[2].expect("scene 1 cell"),
            )
        });
        let song = state.committed_song().expect("song");
        let lane = |idx: usize| {
            song.rows[idx]
                .overrides
                .iter()
                .find(|over| over.track == 2)
                .copied()
        };
        assert_eq!(lane(0), None, "beat 0 is grid-aligned: no override");
        let stamped_scene1 = lane(1).expect("off-grid scene 1 row is stamped");
        assert_eq!(stamped_scene1.pattern_id, Some(scene1_pattern.0));
        assert!((stamped_scene1.offset_steps - (4.7 * 4.0) % 16.0).abs() < 1e-9);
        let stamped = lane(2).expect("off-grid scene 0 row is stamped");
        assert_eq!(stamped.pattern_id, Some(scene0_pattern.0));
        assert!((stamped.offset_steps - (9.3 * 4.0) % 16.0).abs() < 1e-9);
        assert_eq!(lane(3), None, "beat 12 is grid-aligned: no override");
    }

    #[test]
    fn added_track_extends_committed_arrangement_and_stamps_its_scene_spans() {
        let state = SequencerState::new(2, vec![vec![], vec![]]);
        state.with_scenes_mut(|scenes| {
            let snapshots = vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ];
            *scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
        });
        let mut arrangement = ProjectArrangement::new(2, 16.0);
        arrangement.scene_lane = vec![
            SceneEvent {
                start_beat: 0.0,
                scene: 0,
            },
            SceneEvent {
                start_beat: 4.7,
                scene: 1,
            },
            SceneEvent {
                start_beat: 9.3,
                scene: 0,
            },
        ];
        state
            .set_committed_arrangement(Some(arrangement))
            .expect("initial arrangement");

        state
            .extend_all_pattern_snapshots_to_track(
                3,
                &[],
                2,
                CustomInstrumentRunMode::Instrument,
                None,
            )
            .expect("new track extends its arrangement lane");

        let arrangement = state.committed_arrangement().expect("arrangement survives");
        assert_eq!(
            arrangement.track_lanes.len(),
            3,
            "the arrangement topology follows the project topology"
        );
        let lane = &arrangement.track_lanes[2];
        assert_eq!(lane.len(), 3, "every scene span is stamped");
        assert_eq!((lane[0].start_beat, lane[0].end_beat), (0.0, 4.7));
        assert_eq!((lane[1].start_beat, lane[1].end_beat), (4.7, 9.3));
        assert_eq!((lane[2].start_beat, lane[2].end_beat), (9.3, 16.0));
        assert_eq!(
            lane[0].pattern_id, lane[2].pattern_id,
            "spans of the same scene share that scene's pattern"
        );
        assert_ne!(
            lane[0].pattern_id, lane[1].pattern_id,
            "each scene materializes its own pattern for the new track"
        );
        assert_eq!(lane[0].offset_steps, 0.0);
        assert!(
            (lane[1].offset_steps - (4.7 * 4.0) % 16.0).abs() < 1e-9,
            "the scene 1 span keeps global free-run phase"
        );
        assert!(
            (lane[2].offset_steps - (9.3 * 4.0) % 16.0).abs() < 1e-9,
            "the later scene span keeps global free-run phase"
        );
        state.with_project_scenes(|scenes| {
            arrangement
                .validate(scenes)
                .expect("the extended arrangement validates");
        });
        assert!(
            state.committed_song().is_some(),
            "the compiled song is rebuilt with the new lane"
        );
    }

    #[test]
    fn added_track_materializes_a_pattern_in_every_scene() {
        // A scene must always resolve a pattern + sound for every track: a
        // None cell blanks the track's live mirror on launch, so its device
        // disappears (zero-param instrument slot rejects every knob edit).
        // Growing to a new track therefore seeds one private pattern per
        // scene, mirroring new_scene's fork-per-track seeding.
        let state = SequencerState::new(2, vec![vec![], vec![]]);
        state.with_scenes_mut(|scenes| {
            let snapshots = vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ];
            *scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
        });
        state.extend_all_pattern_snapshots_to_track(
            3,
            &[],
            2,
            CustomInstrumentRunMode::Instrument,
            None,
        )
        .unwrap();
        state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.track_pools[2].patterns.len(),
                3,
                "one private pattern per scene plus the track-sound carrier"
            );
            let scene0 = scenes.scenes[0].cells[2].expect("scene 0 cell");
            let scene1 = scenes.scenes[1].cells[2].expect("scene 1 cell");
            assert_ne!(scene0, scene1, "each scene owns its own pattern");
            for (scene, id) in [(0, scene0), (1, scene1)] {
                assert_eq!(
                    scenes.scenes[scene].cell_sounds[2],
                    scenes.track_pools[2].refs(id).expect("pattern refs"),
                    "the cell's sound refs follow its pattern"
                );
            }
            scenes.validate_sound_refs().expect("sound refs resolve");
        });

        // Every scene span of the timeline projection resolves.
        let song = ProjectSong {
            rows: vec![song_row(0, 0.0, 0, &[]), song_row(1, 8.0, 1, &[])],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 2,
        };
        state.with_project_scenes(|scenes| {
            let lanes = project_lanes(&song, scenes);
            assert!(lanes[2][0].pattern.is_some(), "scene 0's span resolves");
            assert!(lanes[2][1].pattern.is_some(), "scene 1's span resolves");
        });
    }

    /// Sticky bare lanes (track-sound spec §2.3): the lazy bare-track mint
    /// is GONE — leftover live-grid content never resurrects a cell (the
    /// ghost-step bug, spec §1.2). The lane's device/mixer deltas persist
    /// into the TRACK SOUND instead, and a latched lane's do not.
    #[test]
    fn bare_lane_save_back_never_mints_and_flows_to_the_track_sound() {
        let state = SequencerState::new(2, vec![vec![], vec![]]);
        state.with_scenes_mut(|scenes| {
            let snapshots = vec![PatternSnapshot::new_default(2, &[])];
            *scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
            // Track 1: fully bare — no cell; only the track-sound carrier
            // remains in the pool.
            let cell = scenes.scenes[0].cells[1].expect("cell");
            assert!(scenes.delete_track_pattern(1, cell));
        });
        let grid_patterns = |scenes: &ProjectScenes, track: usize| {
            scenes.track_pools[track]
                .patterns
                .keys()
                .filter(|id| Some(**id) != scenes.track_sounds[track])
                .count()
        };
        // A snapshot with ACTIVE STEPS on the bare lane (ghost steps from a
        // deleted clip) must NOT mint a cell — and its device deltas land in
        // the track sound.
        state.with_scenes_mut(|scenes| {
            let mut snapshot = PatternSnapshot::new_default(2, &[]);
            snapshot.track_bits[1][0] |= 1;
            snapshot.track_params[1].volume = 0.31;
            assert!(scenes.save_scene_snapshot(0, snapshot));
            assert_eq!(
                grid_patterns(scenes, 1),
                0,
                "live content never materializes a bare track (§2.3)"
            );
            assert_eq!(
                scenes.scenes[0].cells[1], None,
                "the bare cell stays bare"
            );
            let refs = scenes.track_sound_refs(1).expect("track sound resolves");
            let mix = scenes.track_pools[1].sounds.mixes[&refs.mix].clone();
            assert_eq!(
                mix.volume.to_bits(),
                0.31f32.to_bits(),
                "the lane's mixer delta persists into the track sound"
            );
        });
        // A LATCHED bare lane's mirror is the performer's, not the track's
        // own sound: its device deltas must not leak into the track sound.
        state.with_scenes_mut(|scenes| {
            let mut snapshot = PatternSnapshot::new_default(2, &[]);
            snapshot.track_params[1].volume = 0.99;
            assert!(scenes.save_scene_snapshot_masked(0, snapshot, 1 << 1, 1 << 1, 0));
            let refs = scenes.track_sound_refs(1).expect("track sound resolves");
            let mix = scenes.track_pools[1].sounds.mixes[&refs.mix].clone();
            assert_eq!(
                mix.volume.to_bits(),
                0.31f32.to_bits(),
                "a latched lane's deltas stay out of the track sound"
            );
        });
        // Cleared cell with a non-empty pool: content in the live snapshot
        // must NOT resurrect the cell either.
        state.with_scenes_mut(|scenes| {
            scenes.scenes[0].cells[0] = None;
            let mut snapshot = PatternSnapshot::new_default(2, &[]);
            snapshot.track_bits[0][0] |= 1;
            assert!(scenes.save_scene_snapshot(0, snapshot));
            assert_eq!(scenes.scenes[0].cells[0], None, "cleared cells stay cleared");
        });
        // A pool holding ONLY claimed take chunks (plus the carrier) is
        // still bare and still never mints.
        state.with_scenes_mut(|scenes| {
            let chunk_data = PatternSnapshot::new_default(2, &[])
                .track_pattern_data(1)
                .expect("chunk data");
            let chunk = scenes.track_pools[1].insert(chunk_data);
            let sound = scenes.track_pools[1].refs(chunk).expect("chunk refs");
            scenes.take_pools[1].insert(None, vec![chunk], 16, sound);

            let mut snapshot = PatternSnapshot::new_default(2, &[]);
            snapshot.track_bits[1][0] |= 1;
            assert!(scenes.save_scene_snapshot(0, snapshot));
            assert_eq!(scenes.scenes[0].cells[1], None, "no cell is minted");
            assert!(
                !scenes
                    .track_pools[1]
                    .patterns
                    .keys()
                    .any(|id| Some(*id) != scenes.track_sounds[1] && *id != chunk),
                "no grid pattern is minted for a claimed-chunk-only pool"
            );
        });
    }

    #[test]
    fn delete_track_pattern_referenced_by_song_is_rejected_with_row_positions() {
        let state = SequencerState::new(1, vec![vec![]]);
        state.set_committed_song(Some(ProjectSong {
            rows: vec![
                song_row(0, 0.0, 0, &[(0, 1)]),
                song_row(1, 8.0, 0, &[]),
                song_row(2, 16.0, 0, &[(0, 1)]),
            ],
            end_beat: 24.0,
            loop_enabled: false,
            next_row_id: 3,
        }));
        let names = vec!["a".to_string()];
        let err = state
            .delete_track_pattern(
                0,
                PatternId(1),
                1,
                &[-1],
                &[44_100],
                &names,
                &[InstrumentType::Sampler],
            )
            .unwrap_err();
        assert!(err.contains("song row(s) 1, 3"), "{err}");
        // The pattern is still in the pool.
        assert!(state.pattern.scenes.lock().unwrap().track_pools[0].contains(PatternId(1)));
    }

    #[test]
    fn delete_scene_referenced_by_song_is_rejected_and_unreferenced_delete_remaps() {
        let state = SequencerState::new(1, vec![vec![]]);
        let names = vec!["a".to_string()];
        state.with_scenes_mut(|scenes| {
            scenes.new_scene();
            scenes.new_scene();
        });
        state.pattern.current_pattern.store(2, Ordering::Relaxed);
        state.pattern.num_patterns.store(3, Ordering::Relaxed);
        // Current scene is now index 2; a song row references it.
        state.set_committed_song(Some(ProjectSong {
            rows: vec![song_row(0, 0.0, 2, &[])],
            end_beat: 8.0,
            loop_enabled: false,
            next_row_id: 1,
        }));
        let err = state
            .delete_pattern(1, &[-1], &[44_100], &names, &[InstrumentType::Sampler])
            .unwrap_err();
        assert!(err.contains("Scene 3"), "{err}");
        assert!(err.contains("song row(s) 1"), "{err}");
        assert_eq!(state.pattern.scenes.lock().unwrap().scenes.len(), 3);

        // Point the song at scene 0 instead; deleting current scene 2 is now
        // allowed and does not disturb lower scene references.
        state.set_committed_song(Some(ProjectSong {
            rows: vec![song_row(0, 0.0, 0, &[]), song_row(1, 8.0, 1, &[])],
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 2,
        }));
        state
            .delete_pattern(1, &[-1], &[44_100], &names, &[InstrumentType::Sampler])
            .expect("unreferenced scene delete succeeds");
        let song = state.committed_song().unwrap();
        assert_eq!(song.rows[0].scene, Some(0));
        assert_eq!(song.rows[1].scene, Some(1));
    }

    #[test]
    fn remove_track_shifts_live_solo_with_its_track() {
        let state = make_state_with_tracks(3);
        let names = vec!["kick".to_string(), "snare".to_string(), "hat".to_string()];
        let buffer_ids = vec![10, 20, 30];
        let instrument_types = vec![InstrumentType::Sampler; 3];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 3];

        // Solo the last track, delete the middle one: solo is live-only
        // (takes spec 17.8), outside the snapshot restore, so the bit must
        // be shifted explicitly with its track.
        state.pattern.track_params[2].set_solo(true);
        assert!(state.remove_track(
            1,
            &buffer_ids,
            &[44_100; 3],
            &names,
            &instrument_types,
            &effect_descriptors
        ));
        assert!(!state.pattern.track_params[0].is_solo());
        assert!(
            state.pattern.track_params[1].is_solo(),
            "solo follows its track down"
        );
        assert!(
            !state.pattern.track_params[2].is_solo(),
            "trailing lane is cleared"
        );

        // Deleting the soloed track itself clears the bit — it must not
        // transfer to the successor.
        let names = vec!["kick".to_string(), "hat".to_string()];
        let instrument_types = vec![InstrumentType::Sampler; 2];
        let effect_descriptors = vec![EffectDescriptor::default_full_chain(); 2];
        assert!(state.remove_track(
            1,
            &[10, 30],
            &[44_100; 2],
            &names,
            &instrument_types,
            &effect_descriptors
        ));
        assert!(!state.pattern.track_params[0].is_solo());
        assert!(!state.pattern.track_params[1].is_solo());
    }

    #[test]
    fn orphaned_entities_do_not_veto_midi_fx_replacement() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        // Every referenced patch (and the live mirror) carries one MIDI-FX
        // device.
        state.pattern.track_params[0].set_midi_fx_chain(vec!["arp".to_string()]);
        state.with_scenes_mut(|scenes| {
            let pool = &mut scenes.track_pools[0];
            let ids: Vec<PatternId> = pool.patterns.keys().copied().collect();
            for id in ids {
                assert!(pool.edit(id, |data| {
                    data.track_params.midi_fx_chain = vec!["arp".to_string()];
                }));
            }
            // A stale orphan predating the device (chain len 0): reachable
            // by nothing, it must not veto or receive the replacement.
            pool.sounds.insert_patch(Patch::new_default());
        });
        let descriptor = EffectDescriptor {
            name: "arp2".to_string(),
            params: Vec::new(),
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        };
        state
            .replace_midi_fx_slot_in_all_track_patterns(0, 0, "arp2".to_string(), &descriptor)
            .expect("an unreferenced orphan entity must not veto the replacement");
        state.with_scenes_mut(|scenes| {
            for refs in scenes.referenced_track_sounds(0) {
                let patch = &scenes.track_pools[0].sounds.patches[&refs.patch];
                assert_eq!(patch.params.midi_fx_chain, vec!["arp2".to_string()]);
            }
        });
    }

    #[test]
    fn bare_cell_sounds_receive_structural_chain_edits_and_do_not_veto_replacement() {
        let state = make_state_with_tracks(1);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        // Scene 1 goes bare BEFORE the device exists: its cell keeps a sound
        // (§17.2) whose chain is empty, and no pattern represents it.
        let bare_refs = state.with_scenes_mut(|scenes| {
            let id = scenes.scenes[1].cells[0].expect("scene 1 cell");
            assert!(scenes.delete_track_pattern(0, id));
            scenes.scenes[1].cell_sounds[0]
        });
        // Add the device the way the app does: live mirror + the structural
        // sweep over the other patterns — which must include the bare cell.
        state.pattern.track_params[0].set_midi_fx_chain(vec!["arp".to_string()]);
        state.with_scenes_mut(|scenes| {
            let id = scenes.scenes[0].cells[0].expect("scene 0 cell");
            assert!(scenes.track_pools[0].edit(id, |data| {
                data.track_params.midi_fx_chain = vec!["arp".to_string()];
            }));
            assert!(scenes.edit_other_track_patterns(0, |data| {
                data.track_params.midi_fx_chain.insert(0, "arp".to_string());
            }));
            let patch = &scenes.track_pools[0].sounds.patches[&bare_refs.patch];
            assert_eq!(
                patch.params.midi_fx_chain,
                vec!["arp".to_string()],
                "the structural edit reaches the bare cell's entity"
            );
        });
        // Replacement validates every referenced entity — a bare cell whose
        // chain drifted would veto it forever.
        let descriptor = EffectDescriptor {
            name: "arp2".to_string(),
            params: Vec::new(),
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
        };
        state
            .replace_midi_fx_slot_in_all_track_patterns(0, 0, "arp2".to_string(), &descriptor)
            .expect("a bare cell's sound must not veto the replacement");
        state.with_scenes_mut(|scenes| {
            let patch = &scenes.track_pools[0].sounds.patches[&bare_refs.patch];
            assert_eq!(patch.params.midi_fx_chain, vec!["arp2".to_string()]);
        });
    }

    #[test]
    fn scene_launch_restores_pattern_scoped_track_bus_send_baseline() {
        let mut low = PatternSnapshot::new_default(1, &[]);
        low.track_params[0].sends = vec![TrackSendSnapshot {
            destination: BusId::DEFAULT_A,
            amount: 0.15,
        }];
        let mut high = PatternSnapshot::new_default(1, &[]);
        high.track_params[0].sends = vec![TrackSendSnapshot {
            destination: BusId::DEFAULT_A,
            amount: 0.9,
        }];
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let low_live = low.clone();
        state.replace_pattern_repository(vec![low, high], 0);
        low_live.restore(&state);
        let names = vec!["Track 1".to_string()];
        let types = vec![InstrumentType::Sampler];

        state.launch_scene(1, 1, &[-1], &[44_100], &names, &types)
            .expect("launch high-send scene");
        assert_eq!(state.pattern.track_params[0].sends()[0].amount, 0.9);
        state.launch_scene(0, 1, &[-1], &[44_100], &names, &types)
            .expect("return to low-send scene");
        assert_eq!(state.pattern.track_params[0].sends()[0].amount, 0.15);
    }

    /// A legacy pooled slot carries no `param_node_indices` at all (see the
    /// "legacy pool copy carrying no param layout" path in
    /// `sequencer_state/step_edit.rs`). It therefore has no node param id, and
    /// the macro producer keys such a param positionally
    /// (`MacroParamKey::for_instrument(track, param_idx, None)`). The snapshot
    /// consumer must key it the same way instead of skipping the param, or the
    /// macro knob moves and the parameter never changes.
    #[test]
    fn macro_overrides_reach_a_legacy_slot_with_no_param_layout() {
        use crate::macro_engine::MacroParamKey;
        use crate::sequencer::snapshot::apply_slot_macro_overrides;
        use std::collections::HashMap;

        let mut slot = EffectSlotSnapshot::new_empty();
        slot.num_params = 3;
        slot.defaults = vec![0.1, 0.2, 0.3];
        // The legacy shape: no layout, hence no node param id for any param.
        assert!(slot.param_node_indices.is_empty());
        assert!(slot.node_param_idx(1).is_none());

        let mut overrides: HashMap<MacroParamKey, f32> = HashMap::new();
        overrides.insert(MacroParamKey::for_instrument(0, 1, None), 0.75);

        apply_slot_macro_overrides(&mut slot, &overrides, |param_idx, param_id| {
            MacroParamKey::for_instrument(0, param_idx, param_id)
        });

        assert_eq!(
            slot.defaults[1], 0.75,
            "macro override must reach a legacy slot with no param layout"
        );
        assert_eq!(slot.defaults[0], 0.1);
        assert_eq!(slot.defaults[2], 0.3);
    }

    /// The same for an effect slot, and the guard's original purpose still
    /// holds: a layout-less slot must never be keyed as if `param_idx` were a
    /// node param index, so a `Node` key built from that fallback must not
    /// match anything.
    #[test]
    fn legacy_effect_slot_ignores_positional_node_keys() {
        use crate::macro_engine::MacroParamKey;
        use crate::sequencer::snapshot::apply_slot_macro_overrides;
        use std::collections::HashMap;

        let mut slot = EffectSlotSnapshot::new_empty();
        slot.node_id = 42;
        slot.num_params = 2;
        slot.defaults = vec![0.0, 0.0];

        let mut overrides: HashMap<MacroParamKey, f32> = HashMap::new();
        // A key built from the old `param_idx`-as-node-index fallback.
        overrides.insert(
            MacroParamKey::for_effect(
                0,
                1,
                1,
                ParamNodeId::from_slot_param(42, 0, 1),
            ),
            0.9,
        );
        // The positional key the producer actually emits for a layout-less slot.
        overrides.insert(MacroParamKey::for_effect(0, 1, 0, None), 0.4);

        apply_slot_macro_overrides(&mut slot, &overrides, |param_idx, param_id| {
            MacroParamKey::for_effect(0, 1, param_idx, param_id)
        });

        assert_eq!(slot.defaults[0], 0.4, "positional override applies");
        assert_eq!(
            slot.defaults[1], 0.0,
            "a node key from the positional fallback must not match"
        );
    }
