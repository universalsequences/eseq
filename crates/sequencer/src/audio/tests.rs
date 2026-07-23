use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{
    apply_rack_macros_at_step, bus_gate_target_at, clear_active_keyboard_note_by_lid,
    collect_rack_choke_group_voice_releases, for_each_custom_voice_route_update,
    free_patch_transport_route_cache_is_fresh, free_patch_transport_route_target,
    instrument_sound_fingerprint, key_locked_live_instrument_params, mix_metronome,
    mute_group_winner_for_block_events, rack_slot_matches_routing,
    rack_slot_playback_transpose, resolve_live_instrument_defaults,
    resolve_live_keyboard_transpose, resolve_snapshot_instrument_defaults,
    resolved_chord_transpose, resolved_slot_param_value, sampler_warp_runtime,
    select_output_channels, select_output_config, store_active_keyboard_note,
    swing_delay_samples, take_active_keyboard_note, track_accepts_scheduled_trigger,
    ActiveKeyboardNote, ActiveKeyboardVoice, ActiveKeyboardVoiceTarget, BlockEvent,
    BlockEventKind, ChopEvent, CountdownEvent, CountdownEventKind, CustomEnginePool,
    FreePatchTransportRouteState, FreePatchTransportRouteTarget, GateOffEvent, GateOffTarget,
    MetronomeState, OutputDeviceConfig, OutputFormatRange, RackSlotNoteOff,
    FALLBACK_SAMPLE_RATE,
};
use crate::accumulator::{AccumulatorRuntimeState, ResolvedStep};
use crate::analysis::{pack_ptr, OnsetTableShared};
use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, EffectSlotState, ParamDescriptor, ParamKind,
    ParamScaling, TensorParamDescriptor,
};
use crate::scheduled_event::{
    ScheduledChordData, ScheduledEvent, ScheduledEventKind, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledInstrumentTensorParams, ScheduledSamplerParams,
};
use crate::sequencer::{
    default_rack_macros, CustomInstrumentRunMode, InstrumentType, RackMacroCurve,
    RackMacroMapping, RackMacroTarget, RackRouting, RackSlotParam, RackSlotParamPlocks,
    RackSlotSnapshot, RackTrackSnapshot, SequencerState, SwingResolution, TrackSoundState,
};
use crate::voice::VoicePool;

fn active_keyboard_notes_fixture(
) -> [[Option<ActiveKeyboardNote>; crate::voice::MAX_VOICES]; crate::sequencer::MAX_TRACKS]
{
    [[None; crate::voice::MAX_VOICES]; crate::sequencer::MAX_TRACKS]
}

#[test]
fn metronome_mix_emits_a_click_at_a_quarter_boundary() {
    let mut output = vec![0.0; 128 * 2];
    let mut metronome = MetronomeState::default();
    mix_metronome(&mut metronome, &mut output, 2, 48_000.0, 0.0, 120.0);
    assert!(
        output.iter().any(|sample| sample.abs() > 1.0e-6),
        "a metronome block starting on a beat should contain a click"
    );
}

#[test]
fn rack_effect_plock_resolution_requires_matching_parameter_identity() {
    let descriptor = EffectDescriptor::builtin_filter();
    let mut slot = EffectSlotSnapshot::new_default(&descriptor, 42);
    let default = slot.defaults[1];
    assert!(slot.set_plock(3, 1, 0.75));
    assert_eq!(resolved_slot_param_value(&slot, 3, 1, default), 0.75);

    slot.plock_param_ids[3][1] = None;
    assert_eq!(resolved_slot_param_value(&slot, 3, 1, default), default);
}

fn rack_routing_test_slot(pad_note: Option<i32>) -> RackSlotSnapshot {
    RackSlotSnapshot {
        instrument_type: InstrumentType::Sampler,
        instrument_run_mode: CustomInstrumentRunMode::Instrument,
        instrument_base_note_offset: 0.0,
        pad_note,
        choke_group: None,
        gain: 1.0,
        pan: 0.0,
        mute: false,
        solo: false,
        max_polyphony: 1,
        param_plocks: RackSlotParamPlocks::new(),
        instrument_slot: EffectSlotSnapshot::new_empty(),
        effect_slots: RackSlotSnapshot::empty_effect_slots(),
        effect_descriptors: EffectDescriptor::default_full_chain(),
        custom_effect_names: RackSlotSnapshot::empty_effect_names(),
        track_sound_state: TrackSoundState::default(),
        sample_id: None,
    }
}

#[test]
fn rack_macro_is_effective_default_beneath_target_plock() {
    let mut rack = RackTrackSnapshot::new(
        RackRouting::Broadcast,
        vec![rack_routing_test_slot(None)],
        default_rack_macros(),
    );
    rack.macros[0].value = 0.75;
    rack.macros[0].mappings.push(RackMacroMapping {
        target: RackMacroTarget::SlotParam {
            slot: 0,
            param: "gain".to_string(),
        },
        range_min: 0.0,
        range_max: 2.0,
        curve: RackMacroCurve::Linear,
    });
    apply_rack_macros_at_step(&mut rack, 2, [None; crate::sequencer::RACK_MACRO_COUNT]);
    assert_eq!(rack.slots[0].gain, 1.5);

    rack.macros[0].plocks[2] = Some(0.25);
    apply_rack_macros_at_step(&mut rack, 2, [None; crate::sequencer::RACK_MACRO_COUNT]);
    assert_eq!(rack.slots[0].gain, 0.5);

    rack.slots[0].param_plocks.set(2, RackSlotParam::Gain, 0.25);
    apply_rack_macros_at_step(&mut rack, 2, [None; crate::sequencer::RACK_MACRO_COUNT]);
    assert_eq!(
        rack.slots[0].param_value_at_step(RackSlotParam::Gain, 2),
        0.25
    );
}

#[test]
fn published_rack_snapshot_observes_live_macro_defaults_and_plocks() {
    let state = SequencerState::new(1, vec![Vec::new()]);
    let mut rack = RackTrackSnapshot::new(
        RackRouting::Broadcast,
        vec![rack_routing_test_slot(None)],
        default_rack_macros(),
    );
    rack.macros[0].mappings.push(RackMacroMapping {
        target: RackMacroTarget::SlotParam {
            slot: 0,
            param: "gain".to_string(),
        },
        range_min: 0.0,
        range_max: 2.0,
        curve: RackMacroCurve::Linear,
    });
    state.set_rack_track_for_all_pattern_snapshots(0, rack);
    state.publish_scheduler_snapshot();

    let snapshot = state.latest_scheduler_snapshot();
    let mut published_rack = snapshot.tracks[0]
        .rack_track
        .clone()
        .expect("published rack snapshot");
    let macro_id = crate::sequencer::RackMacroId::from_index(0).expect("macro id");

    state.set_live_rack_macro_default(0, macro_id, 0.75);
    apply_rack_macros_at_step(
        &mut published_rack,
        2,
        [None; crate::sequencer::RACK_MACRO_COUNT],
    );
    assert_eq!(published_rack.slots[0].gain, 1.5);

    assert!(state.set_rack_macro_plocks_in_current_pattern(0, macro_id, &[2], 0.25));
    apply_rack_macros_at_step(
        &mut published_rack,
        2,
        [None; crate::sequencer::RACK_MACRO_COUNT],
    );
    assert_eq!(published_rack.slots[0].gain, 0.5);
}

fn audio_test_param(name: &str, default: f32, node_param_idx: u32) -> ParamDescriptor {
    ParamDescriptor {
        name: name.to_string(),
        min: -20_000.0,
        max: 20_000.0,
        default,
        kind: ParamKind::Continuous { unit: None },
        scaling: ParamScaling::Linear,
        node_param_idx,
        node_param_span: 1,
        host_control: None,
        ui_metadata: None,
    }
}

fn param_value(params: &ScheduledInstrumentParams, idx: u64) -> Option<f32> {
    params
        .iter()
        .find(|param| param.target == ScheduledInstrumentParamTarget::Synth && param.idx == idx)
        .map(|param| param.value)
}

#[test]
fn custom_pitch_midi_note_uses_c4_zero_and_base_offset() {
    assert_eq!(super::custom_pitch_midi_note(0.0, 0.0), 60);
    assert_eq!(super::custom_pitch_midi_note(2.0, 0.0), 62);
    assert_eq!(super::custom_pitch_midi_note(0.0, 12.0), 72);
}

#[test]
fn key_locked_live_instrument_params_apply_per_note_after_base_offset() {
    let state = SequencerState::new(1, Vec::new());
    let desc = EffectDescriptor {
        name: "key-lock-test".to_string(),
        input_channels: 0,
        output_channels: 2,
        instrument_modulators: Vec::new(),
        instrument_modulation_targets: Vec::new(),
        tensor_params: Vec::new(),
        params: vec![
            audio_test_param("cutoff", 100.0, 0),
            audio_test_param("detune", 0.0, 1),
        ],
    };
    let slot = &state.pattern.instrument_slots[0];
    slot.apply_descriptor(&desc, 42);
    slot.defaults.set(0, 100.0);
    slot.defaults.set(1, 0.0);
    slot.set_key_lock(60, 1, -12.0);
    slot.set_key_lock(62, 1, -7.0);
    slot.set_key_lock(72, 1, 7.0);

    let base_params = resolve_live_instrument_defaults(&state, 0);
    let c4_params = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, None, &base_params);
    let d4_params = key_locked_live_instrument_params(&state, 0, 2.0, 0.0, None, &base_params);
    let offset_params =
        key_locked_live_instrument_params(&state, 0, 0.0, 12.0, None, &base_params);

    assert_eq!(param_value(&c4_params, 1), Some(-12.0));
    assert_eq!(param_value(&d4_params, 1), Some(-7.0));
    assert_eq!(param_value(&offset_params, 1), Some(7.0));
}

#[test]
fn live_keyboard_defaults_use_macro_effective_scheduler_snapshot() {
    let desc = EffectDescriptor::builtin_filter();
    let state = SequencerState::new(1, vec![crate::sequencer::default_empty_effect_chain()]);
    state.pattern.instrument_slots[0].apply_descriptor(&desc, 42);
    let param_id = state.pattern.instrument_slots[0].param_node_id(0);
    let mut macros = crate::macro_engine::MacroEngine::default();
    let macro_id = macros
        .create_macro("keyboard", crate::macro_engine::MacroKind::Mapped)
        .expect("macro");
    macros
        .add_mapping(
            macro_id,
            crate::macro_engine::MacroMapping::new_resolved(
                0,
                crate::process::ParamTarget::InstrumentParam {
                    param: desc.params[0].name.clone(),
                    param_id,
                },
                Some(0),
                100.0,
                900.0,
                crate::macro_engine::MacroCurve::Linear,
            )
            .expect("mapping"),
        )
        .expect("mapped");

    let snapshot = state.publish_macro_overrides(macros.override_snapshot());
    let defaults = resolve_snapshot_instrument_defaults(&snapshot, 0);

    assert_eq!(
        param_value(&defaults, desc.params[0].node_param_idx as u64),
        Some(100.0),
        "a macro at zero must initialize a new keyboard voice at its mapped minimum"
    );
}

#[test]
fn key_locked_live_instrument_params_keep_step_plocks_per_param() {
    let state = SequencerState::new(1, Vec::new());
    let desc = EffectDescriptor {
        name: "key-lock-precedence-test".to_string(),
        input_channels: 0,
        output_channels: 2,
        instrument_modulators: Vec::new(),
        instrument_modulation_targets: Vec::new(),
        tensor_params: Vec::new(),
        params: vec![
            audio_test_param("cutoff", 100.0, 0),
            audio_test_param("detune", 0.0, 1),
        ],
    };
    let slot = &state.pattern.instrument_slots[0];
    slot.apply_descriptor(&desc, 42);
    slot.defaults.set(0, 100.0);
    slot.defaults.set(1, 0.0);
    slot.set_key_lock(60, 0, 3_000.0);
    slot.set_key_lock(60, 1, -40.0);
    slot.set_plock(5, 0, 2_000.0);

    let mut base_params = resolve_live_instrument_defaults(&state, 0);
    for param in &mut base_params {
        if param.target == ScheduledInstrumentParamTarget::Synth && param.idx == 0 {
            param.value = 2_000.0;
        }
    }

    let merged = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, Some(5), &base_params);

    assert_eq!(
        param_value(&merged, 0),
        Some(2_000.0),
        "valid step p-lock should win for the same param"
    );
    assert_eq!(
        param_value(&merged, 1),
        Some(-40.0),
        "step p-lock on cutoff must not suppress detune's key lock"
    );
}

#[test]
fn key_locked_live_instrument_params_drop_stale_key_lock_identity() {
    let state = SequencerState::new(1, Vec::new());
    let desc = EffectDescriptor {
        name: "key-lock-stale-id-test".to_string(),
        input_channels: 0,
        output_channels: 2,
        instrument_modulators: Vec::new(),
        instrument_modulation_targets: Vec::new(),
        tensor_params: Vec::new(),
        params: vec![audio_test_param("cutoff", 100.0, 0)],
    };
    let slot = &state.pattern.instrument_slots[0];
    slot.apply_descriptor(&desc, 42);
    slot.defaults.set(0, 100.0);
    slot.key_locks.set(60, 0, 3_000.0);

    let base_params = resolve_live_instrument_defaults(&state, 0);
    let merged = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, None, &base_params);

    assert_eq!(param_value(&merged, 0), Some(100.0));
}

#[test]
fn rack_by_pitch_matches_only_the_selected_pad_and_plays_fixed_slot_pitch() {
    let c1_slot = rack_routing_test_slot(Some(0));
    let d1_slot = rack_routing_test_slot(Some(2));
    let unmapped_slot = rack_routing_test_slot(None);

    assert!(rack_slot_matches_routing(
        &c1_slot,
        RackRouting::Broadcast,
        9.0
    ));
    assert!(rack_slot_matches_routing(
        &c1_slot,
        RackRouting::ByPitch,
        0.0
    ));
    assert!(rack_slot_matches_routing(
        &d1_slot,
        RackRouting::ByPitch,
        2.0
    ));
    assert!(!rack_slot_matches_routing(
        &c1_slot,
        RackRouting::ByPitch,
        2.0
    ));
    assert!(!rack_slot_matches_routing(
        &unmapped_slot,
        RackRouting::ByPitch,
        0.0
    ));

    assert_eq!(
        rack_slot_playback_transpose(RackRouting::Broadcast, 7.0),
        7.0
    );
    assert_eq!(rack_slot_playback_transpose(RackRouting::ByPitch, 7.0), 0.0);
}

#[test]
fn rack_choke_group_releases_matching_sampler_and_custom_slots() {
    let mut released_sampler = rack_routing_test_slot(Some(0));
    released_sampler.choke_group = Some(2);
    let mut triggering_sampler = rack_routing_test_slot(Some(1));
    triggering_sampler.choke_group = Some(2);
    let mut unrelated_sampler = rack_routing_test_slot(Some(2));
    unrelated_sampler.choke_group = Some(3);

    let released_pool_id = crate::sequencer::rack_slot_pool_index(0, 0).unwrap();
    let unrelated_pool_id = crate::sequencer::rack_slot_pool_index(0, 2).unwrap();
    let mut voice_pools: Vec<VoicePool> =
        (0..=unrelated_pool_id).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> =
        (0..=6).map(|_| CustomEnginePool::new()).collect();
    let mut countdown_events = vec![CountdownEvent {
        remaining_samples: 32.0,
        period_samples: 0.0,
        repeats: 0,
        pattern_epoch: 0,
        seq: 0,
        kind: CountdownEventKind::GateOff(GateOffEvent {
            track_idx: 0,
            logical_id: 10,
            target: GateOffTarget::Sampler { gatepitch_id: 0 },
        }),
    }];
    let mut block_events = vec![BlockEvent {
        frame_offset: 4,
        seq: 1,
        kind: BlockEventKind::GateOff(GateOffEvent {
            track_idx: 0,
            logical_id: 20,
            target: GateOffTarget::Sampler { gatepitch_id: 0 },
        }),
    }];
    voice_pools[released_pool_id].add_voice(10, 100);
    voice_pools[released_pool_id].voices[0].active = true;
    voice_pools[unrelated_pool_id].add_voice(20, 200);
    voice_pools[unrelated_pool_id].voices[0].active = true;

    let sampler_rack = RackTrackSnapshot::new(
        RackRouting::ByPitch,
        vec![released_sampler, triggering_sampler, unrelated_sampler],
        crate::sequencer::default_rack_macros(),
    );
    let sampler_note_offs = collect_rack_choke_group_voice_releases(
        &mut voice_pools,
        &mut custom_engine_pools,
        &mut countdown_events,
        &mut block_events,
        0,
        &sampler_rack,
        1,
        2,
        1_000,
    );

    assert!(
        !voice_pools[released_pool_id].voices[0].active,
        "matching sampler slot should be released"
    );
    assert!(
        voice_pools[unrelated_pool_id].voices[0].active,
        "unrelated sampler choke group should remain active"
    );
    assert_eq!(
        sampler_note_offs,
        vec![RackSlotNoteOff::Sampler { logical_id: 10 }]
    );
    assert!(
        countdown_events.is_empty(),
        "released sampler gate-off countdown should be cancelled"
    );
    assert_eq!(
        block_events.len(),
        1,
        "unrelated sampler gate-off block event should remain"
    );

    let mut released_custom = rack_routing_test_slot(Some(3));
    released_custom.instrument_type = InstrumentType::Custom;
    released_custom.choke_group = Some(7);
    released_custom.track_sound_state.engine_id = Some(4);
    let mut triggering_custom = rack_routing_test_slot(Some(4));
    triggering_custom.instrument_type = InstrumentType::Custom;
    triggering_custom.choke_group = Some(7);
    triggering_custom.track_sound_state.engine_id = Some(5);
    let mut unrelated_custom = rack_routing_test_slot(Some(5));
    unrelated_custom.instrument_type = InstrumentType::Custom;
    unrelated_custom.choke_group = Some(8);
    unrelated_custom.track_sound_state.engine_id = Some(6);

    custom_engine_pools[4].add_voice(30);
    custom_engine_pools[4].voices[0].active = true;
    custom_engine_pools[4].voices[0].assigned_track = Some(0);
    custom_engine_pools[6].add_voice(40);
    custom_engine_pools[6].voices[0].active = true;
    custom_engine_pools[6].voices[0].assigned_track = Some(0);
    countdown_events.push(CountdownEvent {
        remaining_samples: 32.0,
        period_samples: 0.0,
        repeats: 0,
        pattern_epoch: 0,
        seq: 2,
        kind: CountdownEventKind::GateOff(GateOffEvent {
            track_idx: 0,
            logical_id: 30,
            target: GateOffTarget::Custom {
                engine_id: 4,
                free_patch: false,
            },
        }),
    });
    block_events.push(BlockEvent {
        frame_offset: 12,
        seq: 3,
        kind: BlockEventKind::GateOff(GateOffEvent {
            track_idx: 0,
            logical_id: 40,
            target: GateOffTarget::Custom {
                engine_id: 6,
                free_patch: false,
            },
        }),
    });

    let custom_rack = RackTrackSnapshot::new(
        RackRouting::ByPitch,
        vec![released_custom, triggering_custom, unrelated_custom],
        crate::sequencer::default_rack_macros(),
    );
    let custom_note_offs = collect_rack_choke_group_voice_releases(
        &mut voice_pools,
        &mut custom_engine_pools,
        &mut countdown_events,
        &mut block_events,
        0,
        &custom_rack,
        1,
        7,
        1_016,
    );

    assert!(
        !custom_engine_pools[4].voices[0].active,
        "matching custom slot should be released"
    );
    assert_eq!(
        custom_engine_pools[4].voices[0].release_started_sample,
        Some(1_016),
        "custom release should use the provided release sample"
    );
    assert!(
        custom_engine_pools[6].voices[0].active,
        "unrelated custom choke group should remain active"
    );
    assert_eq!(
        custom_note_offs,
        vec![RackSlotNoteOff::Custom { logical_id: 30 }]
    );
    assert!(
        countdown_events.is_empty(),
        "released custom gate-off countdown should be cancelled"
    );
    assert_eq!(
        block_events.len(),
        2,
        "unrelated sampler and custom gate-off block events should remain"
    );
}

#[test]
fn dj_mixer_slots_carry_explicit_transport_phase_param() {
    let dj = EffectDescriptor::builtin_dj_mixer();
    let dj_slot = EffectSlotState::new(&dj, 42);
    assert_eq!(
        dj_slot.transport_phase_param_idx.load(Ordering::Relaxed),
        crate::effects::dj_mixer::DJ_MIXER_PARAM_TRANSPORT_BEAT_PHASE as u32
    );
    assert_eq!(
        EffectSlotSnapshot::capture(&dj_slot).transport_phase_param_idx,
        crate::effects::dj_mixer::DJ_MIXER_PARAM_TRANSPORT_BEAT_PHASE as u32
    );

    let str8 = EffectDescriptor::builtin_str8_delay();
    let str8_slot = EffectSlotState::new(&str8, 43);
    assert_eq!(
        str8_slot.transport_phase_param_idx.load(Ordering::Relaxed),
        crate::effects::NO_TRANSPORT_PHASE_PARAM
    );
}

#[test]
fn active_keyboard_note_stores_all_rack_slot_voices_for_one_key() {
    let mut notes = active_keyboard_notes_fixture();
    let voices = [
        ActiveKeyboardVoice {
            logical_id: 11,
            gatepitch_id: 21,
            target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 64 },
        },
        ActiveKeyboardVoice {
            logical_id: 12,
            gatepitch_id: 0,
            target: ActiveKeyboardVoiceTarget::Custom {
                engine_id: 7,
                free_patch: false,
            },
        },
    ];

    store_active_keyboard_note(&mut notes, 0, 3.0, Some(63), &voices);
    let note = take_active_keyboard_note(&mut notes, 0, 3.0).unwrap();

    assert_eq!(note.source_transpose, 3.0);
    assert_eq!(note.midi_note, Some(63));
    assert_eq!(note.voices(), &voices);
}

#[test]
fn active_keyboard_note_clear_by_lid_preserves_other_slot_voices() {
    let mut notes = active_keyboard_notes_fixture();
    let voices = [
        ActiveKeyboardVoice {
            logical_id: 11,
            gatepitch_id: 21,
            target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 64 },
        },
        ActiveKeyboardVoice {
            logical_id: 12,
            gatepitch_id: 0,
            target: ActiveKeyboardVoiceTarget::Custom {
                engine_id: 7,
                free_patch: false,
            },
        },
        ActiveKeyboardVoice {
            logical_id: 13,
            gatepitch_id: 23,
            target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 65 },
        },
    ];

    store_active_keyboard_note(&mut notes, 0, 3.0, Some(63), &voices);
    clear_active_keyboard_note_by_lid(&mut notes, 12);
    let note = take_active_keyboard_note(&mut notes, 0, 3.0).unwrap();

    assert_eq!(note.voices(), &[voices[0], voices[2]]);
}

#[test]
fn output_config_prefers_system_default_sample_rate_over_44100() {
    let ranges = [
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 4,
            min_sample_rate: 44_100,
            max_sample_rate: 96_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 44_100,
            max_sample_rate: 44_100,
            supports_f32: true,
        },
    ];

    assert_eq!(
        select_output_config(48_000, 2, ranges),
        Some(OutputDeviceConfig {
            sample_rate: 48_000,
            channels: 2,
        })
    );
}

#[test]
fn output_config_keeps_default_channels_at_selected_rate() {
    let ranges = [
        OutputFormatRange {
            channels: 6,
            min_sample_rate: 48_000,
            max_sample_rate: 96_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 96_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 1,
            min_sample_rate: 44_100,
            max_sample_rate: 44_100,
            supports_f32: true,
        },
    ];

    assert_eq!(select_output_channels(48_000, 6, ranges), Some(6));
}

#[test]
fn output_config_prefers_stereo_when_default_channels_do_not_support_selected_rate() {
    let ranges = [
        OutputFormatRange {
            channels: 6,
            min_sample_rate: 44_100,
            max_sample_rate: 44_100,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 1,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 96_000,
            supports_f32: true,
        },
    ];

    assert_eq!(select_output_channels(48_000, 6, ranges), Some(2));
}

#[test]
fn output_config_uses_default_sample_rate_without_44100() {
    let ranges = [
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: true,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 88_200,
            max_sample_rate: 96_000,
            supports_f32: true,
        },
    ];

    assert_eq!(
        select_output_config(48_000, 2, ranges),
        Some(OutputDeviceConfig {
            sample_rate: 48_000,
            channels: 2,
        })
    );
}

#[test]
fn output_config_uses_default_sample_rate_when_44100_lacks_f32() {
    let ranges = [
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 44_100,
            max_sample_rate: 44_100,
            supports_f32: false,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: true,
        },
    ];

    assert_eq!(
        select_output_config(48_000, 2, ranges),
        Some(OutputDeviceConfig {
            sample_rate: 48_000,
            channels: 2,
        })
    );
}

#[test]
fn output_config_falls_back_to_44100_when_default_rate_lacks_f32() {
    let ranges = [
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: false,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: FALLBACK_SAMPLE_RATE,
            max_sample_rate: FALLBACK_SAMPLE_RATE,
            supports_f32: true,
        },
    ];

    assert_eq!(
        select_output_config(48_000, 2, ranges),
        Some(OutputDeviceConfig {
            sample_rate: FALLBACK_SAMPLE_RATE,
            channels: 2,
        })
    );
}

#[test]
fn output_config_rejects_when_default_and_fallback_rates_lack_f32_support() {
    let ranges = [
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 44_100,
            max_sample_rate: 44_100,
            supports_f32: false,
        },
        OutputFormatRange {
            channels: 2,
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            supports_f32: false,
        },
    ];

    assert_eq!(select_output_config(48_000, 2, ranges), None);
}

fn test_block_trigger(seq: u64, track: usize) -> BlockEvent {
    BlockEvent {
        frame_offset: 128,
        seq,
        kind: BlockEventKind::Scheduled(ScheduledEvent {
            pattern_epoch: 1,
            sample_time: 128,
            kind: ScheduledEventKind::ResolvedTrigger {
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                track,
                step: 0,
                samples_per_step: 1024.0,
                resolved: ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                chord: ScheduledChordData {
                    count: 0,
                    notes: [0.0; crate::voice::MAX_VOICES],
                    durations: [0.0; crate::voice::MAX_VOICES],
                    delays: [0.0; crate::voice::MAX_VOICES],
                    step_transpose: 0.0,
                },
                effect_params: Vec::new(),
                instrument_params: ScheduledInstrumentParams::new(),
                instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                sampler_params: ScheduledSamplerParams::default(),
                instrument_fingerprint: 0,
            },
        }),
    }
}

fn test_block_network_trigger(seq: u64, track: usize) -> BlockEvent {
    BlockEvent {
        frame_offset: 128,
        seq,
        kind: BlockEventKind::Scheduled(ScheduledEvent {
            pattern_epoch: 1,
            sample_time: 128,
            kind: ScheduledEventKind::NetworkTrigger {
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                track,
                source_neuron: 0,
                seed: None,
                samples_per_step: 1024.0,
                resolved: ResolvedStep {
                    duration: 1.0,
                    velocity: 1.0,
                    speed: 1.0,
                    aux_a: 0.0,
                    aux_b: 0.0,
                    transpose: 0.0,
                    pan: 0.0,
                    chop: 1.0,
                },
                chord: ScheduledChordData {
                    count: 0,
                    notes: [0.0; crate::voice::MAX_VOICES],
                    durations: [0.0; crate::voice::MAX_VOICES],
                    delays: [0.0; crate::voice::MAX_VOICES],
                    step_transpose: 0.0,
                },
                effect_params: Vec::new(),
                instrument_params: ScheduledInstrumentParams::new(),
                instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                sampler_params: ScheduledSamplerParams::default(),
                instrument_fingerprint: 0,
            },
        }),
    }
}

#[test]
fn mute_group_same_sample_uses_highest_track_as_winner() {
    let batch = vec![test_block_trigger(0, 0), test_block_trigger(1, 2)];

    let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
        0 | 2 => 1,
        _ => 0,
    });

    assert_eq!(winner, 2);
}

#[test]
fn mute_group_same_sample_considers_neural_and_step_triggers_together() {
    let batch = vec![test_block_network_trigger(0, 0), test_block_trigger(1, 2)];

    let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
        0 | 2 => 1,
        _ => 0,
    });

    assert_eq!(winner, 2);
}

#[test]
fn mute_group_winner_ignores_off_and_other_groups() {
    let batch = vec![
        test_block_trigger(0, 0),
        test_block_trigger(1, 1),
        test_block_trigger(2, 2),
    ];

    let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
        0 => 1,
        1 => 0,
        2 => 2,
        _ => 0,
    });

    assert_eq!(winner, 0);
}

#[test]
fn instrument_sound_fingerprint_changes_for_tensor_default_and_plock_values() {
    let state = SequencerState::new(1, Vec::new());
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
    state.pattern.instrument_slots[0].apply_descriptor(&desc, 42);

    let default_fingerprint = instrument_sound_fingerprint(&state, 0, 5, None);
    state.pattern.instrument_slots[0]
        .tensor_params
        .set_default_cell(0, 0, 0.75)
        .expect("default tensor edit");
    let edited_default_fingerprint = instrument_sound_fingerprint(&state, 0, 5, None);
    state.pattern.instrument_slots[0]
        .tensor_params
        .set_plock_cell(3, 0, 1, 0.95)
        .expect("tensor p-lock edit");
    let plocked_fingerprint = instrument_sound_fingerprint(&state, 0, 5, Some(3));

    assert_ne!(default_fingerprint, edited_default_fingerprint);
    assert_ne!(edited_default_fingerprint, plocked_fingerprint);
}

#[test]
fn sampler_warp_ratio_speeds_source_when_project_bpm_is_higher() {
    let state = SequencerState::new(1, Vec::new());
    state.transport.bpm.store(160, Ordering::Relaxed);
    state.runtime.sampler_analysis_status[0].store(2, Ordering::Relaxed);
    let table = OnsetTableShared {
        onsets_frames: vec![0, 22_050],
        sample_len_frames: 44_100,
    };
    let (lo, hi) = pack_ptr(&table as *const OnsetTableShared);
    state.runtime.sampler_onset_ptr_lo[0].store(lo.to_bits(), Ordering::Relaxed);
    state.runtime.sampler_onset_ptr_hi[0].store(hi.to_bits(), Ordering::Relaxed);

    let (_, _, ratio, sample_bpm, project_bpm, _, _) =
        sampler_warp_runtime(&state, 0, 1.0, 0.0, 120.0);

    assert!((ratio - (160.0 / 120.0)).abs() < 0.0001);
    assert!((sample_bpm - 120.0).abs() < 0.0001);
    assert!((project_bpm - 160.0).abs() < 0.0001);
}

#[test]
fn sampler_warp_repitch_mode_needs_no_analysis() {
    let state = SequencerState::new(1, Vec::new());
    state.transport.bpm.store(120, Ordering::Relaxed);
    // No analysis status, no onset table: re-pitch must still engage.
    let (enabled, mode, ratio, _, _, ptr_lo, ptr_hi) = sampler_warp_runtime(
        &state,
        0,
        1.0,
        crate::sampler::WARP_MODE_REPITCH as f32,
        174.0,
    );
    assert!(enabled > 0.5);
    assert_eq!(mode.round() as i32, crate::sampler::WARP_MODE_REPITCH);
    // 174 BPM sample in a 120 BPM project: the read head must consume
    // source slower, at 120/174 ≈ 0.69 source frames per host frame.
    assert!((ratio - (120.0 / 174.0)).abs() < 0.0001);
    assert_eq!((ptr_lo, ptr_hi), (0.0, 0.0));

    // Beats mode without analysis: enabled on the pure beat grid, with a
    // null onset table (Preserve=Transients degrades to the plain grid).
    let (enabled, _, ratio, _, _, ptr_lo, ptr_hi) =
        sampler_warp_runtime(&state, 0, 1.0, 0.0, 174.0);
    assert!(enabled > 0.5);
    assert!((ratio - (120.0 / 174.0)).abs() < 0.0001);
    assert_eq!((ptr_lo, ptr_hi), (0.0, 0.0));
}

#[test]
fn custom_engine_pool_reuses_inactive_same_track_voices_before_expanding() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=4 {
        pool.add_voice(lid);
    }

    let a = pool.allocate_voice(0, 0, 0.0, true, 6);
    let b = pool.allocate_voice(0, 0, 4.0, true, 6);
    assert_eq!(a.logical_id, 1);
    assert_eq!(b.logical_id, 2);
    assert!(!a.stole_active_voice);
    assert!(!b.stole_active_voice);

    pool.release_voice_by_logical_id(a.logical_id, 0);
    pool.release_voice_by_logical_id(b.logical_id, 0);
    pool.shrink_released_voices(0, 1_000, 1_000, 1);

    let c = pool.allocate_voice(0, 0, 7.0, true, 6);
    let d = pool.allocate_voice(0, 0, 11.0, true, 6);
    assert_eq!(c.logical_id, 1);
    assert_eq!(d.logical_id, 2);
    assert!(!c.stole_active_voice);
    assert!(!d.stole_active_voice);
}

#[test]
fn custom_engine_pool_uses_unassigned_voice_for_new_same_track_overlap() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=4 {
        pool.add_voice(lid);
    }

    let a = pool.allocate_voice(0, 0, 0.0, true, 6);
    let b = pool.allocate_voice(0, 0, 4.0, true, 6);

    assert_eq!(a.logical_id, 1);
    assert_eq!(b.logical_id, 2);
    assert!(!a.stole_active_voice);
    assert!(!b.stole_active_voice);
}

#[test]
fn custom_engine_pool_enforces_track_polyphony_cap_as_segment() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=6 {
        pool.add_voice(lid);
    }

    let a0 = pool.allocate_voice(0, 0, 0.0, true, 2);
    let a1 = pool.allocate_voice(0, 0, 4.0, true, 2);
    let b0 = pool.allocate_voice(1, 1, 0.0, true, 2);
    let b1 = pool.allocate_voice(1, 1, 4.0, true, 2);

    assert_eq!(a0.logical_id, 1);
    assert_eq!(a1.logical_id, 2);
    assert_eq!(b0.logical_id, 3);
    assert_eq!(b1.logical_id, 4);

    let capped = pool.allocate_voice(0, 0, 7.0, true, 2);
    assert!(capped.stole_active_voice);
    assert_eq!(capped.previous_track, Some(0));
    assert!(matches!(capped.logical_id, 1 | 2));

    let track_one_count = pool.voices[..pool.num_voices]
        .iter()
        .filter(|voice| voice.assigned_track == Some(1))
        .count();
    assert_eq!(track_one_count, 2);
}

#[test]
fn custom_engine_pool_tracks_polyphony_per_rack_route_consumer() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=6 {
        pool.add_voice(lid);
    }
    let first_route = crate::sequencer::rack_slot_pool_index(0, 0).expect("first rack route");
    let second_route = crate::sequencer::rack_slot_pool_index(0, 1).expect("second rack route");

    let a0 = pool.allocate_voice(0, first_route, 0.0, true, 2);
    let a1 = pool.allocate_voice(0, first_route, 4.0, true, 2);
    let b0 = pool.allocate_voice(0, second_route, 7.0, true, 2);
    let b1 = pool.allocate_voice(0, second_route, 11.0, true, 2);

    assert_eq!([a0.logical_id, a1.logical_id], [1, 2]);
    assert_eq!([b0.logical_id, b1.logical_id], [3, 4]);
    assert_eq!(
        pool.voices[..pool.num_voices]
            .iter()
            .filter(|voice| voice.assigned_route == Some(first_route))
            .count(),
        2
    );
    assert_eq!(
        pool.voices[..pool.num_voices]
            .iter()
            .filter(|voice| voice.assigned_route == Some(second_route))
            .count(),
        2
    );
}

#[test]
fn unknown_previous_consumer_closes_stale_rack_route_before_opening_track_route() {
    let state = SequencerState::new(2, Vec::new());
    let engine_id = 4;
    let voice_idx = 3;
    let rack_route = crate::sequencer::rack_slot_pool_index(0, 0).expect("rack route identity");
    state.runtime.engine_route_lids[engine_id][voice_idx][1].store(201, Ordering::Relaxed);
    state.runtime.engine_route_lids_r[engine_id][voice_idx][1].store(202, Ordering::Relaxed);
    state.runtime.rack_engine_route_engine_ids[rack_route]
        .store(engine_id as u32, Ordering::Relaxed);
    state.runtime.rack_engine_route_lids[rack_route][voice_idx].store(101, Ordering::Relaxed);
    state.runtime.rack_engine_route_lids_r[rack_route][voice_idx].store(102, Ordering::Relaxed);

    let mut updates = Vec::new();
    for_each_custom_voice_route_update(
        &state,
        engine_id,
        voice_idx,
        None,
        1,
        |logical_id, value| updates.push((logical_id, value)),
    );

    assert!(updates.contains(&(101, 0.0)));
    assert!(updates.contains(&(102, 0.0)));
    assert!(updates.contains(&(201, 1.0)));
    assert!(updates.contains(&(202, 1.0)));
    assert!(!updates.contains(&(101, 1.0)));
    assert!(!updates.contains(&(102, 1.0)));
}

#[test]
fn custom_engine_pool_reuses_releasing_same_note_before_expanding() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=6 {
        pool.add_voice(lid);
    }

    let low = pool.allocate_voice(0, 0, 0.0, true, 6);
    let low_lid = low.logical_id;
    pool.release_voice_by_logical_id(low_lid, 1_000);

    let retriggered = pool.allocate_voice(0, 0, 0.0, true, 6);

    assert_eq!(low_lid, 1);
    assert_eq!(retriggered.logical_id, low_lid);
    assert!(!retriggered.stole_active_voice);
    assert_eq!(pool.voices[0].release_started_sample, None);
}

#[test]
fn custom_engine_pool_uses_unassigned_voice_for_different_note_before_cap() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=6 {
        pool.add_voice(lid);
    }

    let low = pool.allocate_voice(0, 0, -24.0, true, 6);
    let low_lid = low.logical_id;
    pool.release_voice_by_logical_id(low_lid, 1_000);

    let mid = pool.allocate_voice(0, 0, 0.0, true, 6);
    let high = pool.allocate_voice(0, 0, 7.0, true, 6);

    assert_eq!(low_lid, 1);
    assert_eq!(mid.logical_id, 2);
    assert_eq!(high.logical_id, 3);
    assert!(!mid.stole_active_voice);
    assert!(!high.stole_active_voice);
    assert_eq!(pool.voices[0].release_started_sample, Some(1_000));
}

#[test]
fn custom_engine_pool_shrinks_only_after_release_tail_expires() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=4 {
        pool.add_voice(lid);
    }

    let low = pool.allocate_voice(0, 0, 0.0, true, 6);
    let high = pool.allocate_voice(0, 0, 7.0, true, 6);
    let low_lid = low.logical_id;
    let high_lid = high.logical_id;
    pool.enabled_voice_count = 2;

    pool.release_voice_by_logical_id(high_lid, 1_000);
    pool.shrink_released_voices(0, 1_999, 1_000, 1);
    assert_eq!(pool.enabled_voice_count, 2);

    pool.shrink_released_voices(0, 2_000, 1_000, 1);
    assert_eq!(pool.enabled_voice_count, 1);
    assert!(pool.voices[0].active);
    assert_eq!(pool.voices[0].logical_id, low_lid);
}

#[test]
fn idle_instrument_engine_disables_all_voices() {
    let engine_id = 0;
    let mut pool = CustomEnginePool::new();
    for lid in 1..=4 {
        pool.add_voice(lid);
    }
    pool.enabled_voice_count = 1;
    crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, 1);

    pool.shrink_released_voices(engine_id, 0, 1_000, 0);

    assert_eq!(pool.enabled_voice_count, 0);
    assert_eq!(
        crate::lisp_host::get_dgen_engine_enabled_voices(engine_id),
        0
    );
    crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
}

#[test]
fn custom_engine_pool_steals_same_tracks_active_voice_first() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=2 {
        pool.add_voice(lid);
    }

    let first = pool.allocate_voice(0, 0, 0.0, true, 6);
    let second = pool.allocate_voice(1, 1, 4.0, true, 6);
    assert_eq!(first.logical_id, 1);
    assert_eq!(second.logical_id, 2);

    let stolen = pool.allocate_voice(1, 1, 7.0, true, 6);
    assert!(stolen.stole_active_voice);
    assert_eq!(stolen.previous_track, Some(1));
    assert_eq!(stolen.logical_id, 2);
}

#[test]
fn custom_engine_pool_mono_reuses_same_tracks_voice_and_marks_active_steal() {
    let mut pool = CustomEnginePool::new();
    for lid in 1..=2 {
        pool.add_voice(lid);
    }

    let first = pool.allocate_voice(3, 3, 0.0, false, 6);
    let reused = pool.allocate_voice(3, 3, 12.0, false, 6);

    assert_eq!(reused.logical_id, first.logical_id);
    assert!(reused.stole_active_voice);
    assert_eq!(reused.previous_track, Some(3));
}

#[test]
fn custom_engine_pool_free_patch_always_targets_voice_zero_without_release_tail() {
    let mut pool = CustomEnginePool::new();
    for lid in 10..=12 {
        pool.add_voice(lid);
    }

    let first = pool.allocate_free_patch_voice(2, 2, 0.0).unwrap();
    let second = pool.allocate_free_patch_voice(2, 2, 7.0).unwrap();

    assert_eq!(first.voice_idx, 0);
    assert_eq!(first.logical_id, 10);
    assert_eq!(second.voice_idx, 0);
    assert_eq!(second.logical_id, 10);
    assert!(second.stole_active_voice);

    pool.release_free_patch_voice_by_logical_id(second.logical_id);

    assert!(!pool.voices[0].active);
    assert_eq!(pool.voices[0].assigned_track, Some(2));
    assert_eq!(pool.voices[0].release_started_sample, None);
}

#[test]
fn free_patch_transport_route_target_only_matches_custom_free_patch_tracks() {
    let state = SequencerState::new(3, Vec::new());
    state.runtime.instrument_type_flags[0]
        .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
    state.runtime.instrument_run_mode_flags[0].store(
        CustomInstrumentRunMode::Instrument.runtime_flag(),
        Ordering::Relaxed,
    );
    state.runtime.track_engine_ids[0].store(10, Ordering::Relaxed);

    state.runtime.instrument_type_flags[1]
        .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
    state.runtime.instrument_run_mode_flags[1].store(
        CustomInstrumentRunMode::FreePatch.runtime_flag(),
        Ordering::Relaxed,
    );
    state.runtime.track_engine_ids[1].store(11, Ordering::Relaxed);

    state.runtime.instrument_type_flags[2]
        .store(InstrumentType::Sampler.runtime_flag(), Ordering::Relaxed);
    state.runtime.instrument_run_mode_flags[2].store(
        CustomInstrumentRunMode::FreePatch.runtime_flag(),
        Ordering::Relaxed,
    );
    state.runtime.track_engine_ids[2].store(12, Ordering::Relaxed);

    assert_eq!(free_patch_transport_route_target(&state, 0, 3, true), None);
    assert_eq!(free_patch_transport_route_target(&state, 2, 3, true), None);

    let target = free_patch_transport_route_target(&state, 1, 3, true)
        .expect("custom free-patch track should produce a route target");
    assert_eq!(target.engine_id, 11);
    assert!(target.open);

    let stopped_target = free_patch_transport_route_target(&state, 1, 3, false)
        .expect("custom free-patch track should still be tracked while stopped");
    assert_eq!(stopped_target.engine_id, 11);
    assert!(!stopped_target.open);
    assert_eq!(stopped_target.route_hash, target.route_hash);
}

#[test]
fn free_patch_transport_route_target_hash_changes_when_route_nodes_change() {
    let state = SequencerState::new(2, Vec::new());
    state.runtime.instrument_type_flags[0]
        .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
    state.runtime.instrument_run_mode_flags[0].store(
        CustomInstrumentRunMode::FreePatch.runtime_flag(),
        Ordering::Relaxed,
    );
    state.runtime.track_engine_ids[0].store(4, Ordering::Relaxed);
    state.runtime.engine_route_lids[4][0][0].store(100, Ordering::Relaxed);
    state.runtime.engine_route_lids_r[4][0][0].store(101, Ordering::Relaxed);

    let before = free_patch_transport_route_target(&state, 0, 2, false)
        .expect("route target should exist")
        .route_hash;

    state.runtime.engine_route_lids[4][0][0].store(200, Ordering::Relaxed);
    let after = free_patch_transport_route_target(&state, 0, 2, false)
        .expect("route target should exist after route-node change")
        .route_hash;

    assert_ne!(before, after);
}

#[test]
fn free_patch_transport_route_cache_does_not_suppress_stopped_mute() {
    let cached = FreePatchTransportRouteState {
        valid: true,
        engine_id: 3,
        route_hash: 99,
        open: false,
    };
    let stopped_target = FreePatchTransportRouteTarget {
        engine_id: 3,
        route_hash: 99,
        open: false,
    };
    let playing_target = FreePatchTransportRouteTarget {
        open: true,
        ..stopped_target
    };

    assert!(!free_patch_transport_route_cache_is_fresh(
        cached,
        stopped_target
    ));
    assert!(free_patch_transport_route_cache_is_fresh(
        FreePatchTransportRouteState {
            open: true,
            ..cached
        },
        playing_target
    ));
}

#[test]
fn countdown_gate_off_cancel_removes_matching_pending_lids() {
    let mut events = vec![
        CountdownEvent {
            remaining_samples: 100.0,
            period_samples: 0.0,
            repeats: 1,
            pattern_epoch: 1,
            seq: 0,
            kind: CountdownEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 10,
                target: GateOffTarget::Sampler { gatepitch_id: 100 },
            }),
        },
        CountdownEvent {
            remaining_samples: 100.0,
            period_samples: 0.0,
            repeats: 1,
            pattern_epoch: 1,
            seq: 1,
            kind: CountdownEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 20,
                target: GateOffTarget::Sampler { gatepitch_id: 200 },
            }),
        },
    ];
    let mut block_events = vec![
        BlockEvent {
            frame_offset: 12,
            seq: 2,
            kind: BlockEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 10,
                target: GateOffTarget::Sampler { gatepitch_id: 100 },
            }),
        },
        BlockEvent {
            frame_offset: 16,
            seq: 3,
            kind: BlockEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 20,
                target: GateOffTarget::Sampler { gatepitch_id: 200 },
            }),
        },
    ];

    super::cancel_gate_off_for_lid(&mut events, &mut block_events, 10);

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].kind,
        CountdownEventKind::GateOff(GateOffEvent { logical_id: 20, .. })
    ));
    assert_eq!(block_events.len(), 1);
    assert!(matches!(
        block_events[0].kind,
        BlockEventKind::GateOff(GateOffEvent { logical_id: 20, .. })
    ));
}

#[test]
fn chop_cancel_preserves_scheduled_triggers_for_later_mute_group_winner() {
    let scheduled_countdown = match test_block_trigger(0, 1).kind {
        BlockEventKind::Scheduled(event) => event,
        _ => unreachable!(),
    };
    let mut countdown_events = vec![
        CountdownEvent {
            remaining_samples: 32.0,
            period_samples: 0.0,
            repeats: 1,
            pattern_epoch: 1,
            seq: 0,
            kind: CountdownEventKind::Scheduled(scheduled_countdown),
        },
        CountdownEvent {
            remaining_samples: 48.0,
            period_samples: 16.0,
            repeats: 1,
            pattern_epoch: 1,
            seq: 1,
            kind: CountdownEventKind::Chop(ChopEvent {
                track_idx: 1,
                step: 0,
                chop_gate: 16.0,
            }),
        },
    ];
    let mut block_events = vec![
        test_block_trigger(2, 1),
        BlockEvent {
            frame_offset: 20,
            seq: 3,
            kind: BlockEventKind::Chop(ChopEvent {
                track_idx: 1,
                step: 0,
                chop_gate: 16.0,
            }),
        },
    ];

    super::cancel_chops_for_track(&mut countdown_events, &mut block_events, 1);

    assert_eq!(countdown_events.len(), 1);
    assert!(matches!(
        countdown_events[0].kind,
        CountdownEventKind::Scheduled(_)
    ));
    assert_eq!(block_events.len(), 1);
    assert!(matches!(block_events[0].kind, BlockEventKind::Scheduled(_)));
}

#[test]
fn swing_delay_uses_full_pair_offset() {
    let delay = swing_delay_samples(48_000.0, 120.0, 75.0, SwingResolution::Sixteenth);
    assert_eq!(delay, 3_000.0);

    let straight = swing_delay_samples(48_000.0, 120.0, 50.0, SwingResolution::Sixteenth);
    assert_eq!(straight, 0.0);
}

#[test]
fn bus_gate_default_sequence_stays_open_across_steps() {
    let sequence = crate::sequencer::BusGateSequence::default();
    assert_eq!(bus_gate_target_at(&sequence, 0.0), 1.0);
    assert_eq!(bus_gate_target_at(&sequence, 0.24), 1.0);
    assert_eq!(bus_gate_target_at(&sequence, 0.25), 1.0);
    assert_eq!(bus_gate_target_at(&sequence, 1.99), 1.0);
}

#[test]
fn bus_gate_sequence_follows_step_activity_and_duration() {
    let mut sequence = crate::sequencer::BusGateSequence::default();
    sequence.num_steps = 4;
    sequence.timebase = crate::sequencer::Timebase::Quarter;
    sequence.steps = [false; crate::sequencer::MAX_STEPS];
    sequence.steps[1] = true;
    sequence.velocities[1] = 0.5;
    sequence.durations[1] = 0.5;

    assert_eq!(bus_gate_target_at(&sequence, 0.25), 0.0);
    assert_eq!(bus_gate_target_at(&sequence, 1.10), 0.5);
    assert_eq!(bus_gate_target_at(&sequence, 1.60), 0.0);
    assert_eq!(bus_gate_target_at(&sequence, 2.10), 0.0);
}

#[test]
fn bus_gate_sync_steps_snap_boundaries_to_grid() {
    let mut sequence = crate::sequencer::BusGateSequence::default();
    sequence.num_steps = 4;
    sequence.timebase = crate::sequencer::Timebase::Sixteenth;
    sequence.steps = [false; crate::sequencer::MAX_STEPS];
    sequence.steps[1] = true;
    sequence.syncs[1] = 3.0; // 1/4

    assert_eq!(bus_gate_target_at(&sequence, 0.50), 0.0);
    assert_eq!(bus_gate_target_at(&sequence, 1.01), 1.0);
    assert_eq!(bus_gate_target_at(&sequence, 1.30), 0.0);
}

#[test]
fn sound_fingerprint_changes_when_step_sound_changes() {
    let state = Arc::new(SequencerState::new(1, Vec::new()));
    state.runtime.track_engine_ids[0].store(2, Ordering::Relaxed);
    state.pattern.instrument_slots[0]
        .num_params
        .store(2, Ordering::Relaxed);
    state.pattern.instrument_slots[0].defaults.set(0, 0.2);
    state.pattern.instrument_slots[0].defaults.set(1, 0.4);

    let base = instrument_sound_fingerprint(&state, 0, 2, Some(3));
    state.pattern.instrument_slots[0].set_plock(3, 1, 0.9);
    let changed = instrument_sound_fingerprint(&state, 0, 2, Some(3));

    assert_ne!(base, changed);
}

#[test]
fn sound_fingerprint_changes_when_base_note_changes() {
    let state = Arc::new(SequencerState::new(1, Vec::new()));
    state.runtime.track_engine_ids[0].store(5, Ordering::Relaxed);
    state.pattern.instrument_slots[0]
        .num_params
        .store(1, Ordering::Relaxed);
    state.pattern.instrument_slots[0].defaults.set(0, 0.5);

    let base = instrument_sound_fingerprint(&state, 0, 5, None);
    state.pattern.instrument_base_note_offsets[0].store(12.0f32.to_bits(), Ordering::Relaxed);
    let changed = instrument_sound_fingerprint(&state, 0, 5, None);

    assert_ne!(base, changed);
}

#[test]
fn invalidating_custom_voice_cache_clears_all_fingerprints() {
    let mut pool = CustomEnginePool::new();
    pool.add_voice(10);
    pool.add_voice(20);
    pool.voices[0].fingerprint = 123;
    pool.voices[1].fingerprint = 456;

    pool.invalidate_sound_cache();

    assert_eq!(pool.voices[0].fingerprint, 0);
    assert_eq!(pool.voices[1].fingerprint, 0);
}

#[test]
fn resolved_chord_transpose_applies_accumulator_offset() {
    assert_eq!(resolved_chord_transpose(7.0, 0.0, 5.0), 12.0);
    assert_eq!(resolved_chord_transpose(7.0, 2.0, 8.0), 13.0);
}

#[test]
fn live_keyboard_transpose_applies_current_transpose_ramp_state() {
    let state = SequencerState::new(1, Vec::new());
    state.pattern.track_params[0].set_accumulator_idx(1);

    let resolved = resolve_live_keyboard_transpose(
        &state,
        AccumulatorRuntimeState {
            value: 5.0,
            reversed: false,
        },
        0,
        2.0,
    );

    assert_eq!(resolved, 7.0);
}

#[test]
fn live_keyboard_transpose_quantizes_after_transpose_ramp_offset() {
    let state = SequencerState::new(1, Vec::new());
    state.pattern.track_params[0].set_accumulator_idx(1);
    state.pattern.track_params[0].set_fts_scale(1);

    let resolved = resolve_live_keyboard_transpose(
        &state,
        AccumulatorRuntimeState {
            value: 1.0,
            reversed: false,
        },
        0,
        2.6,
    );

    assert_eq!(resolved, 4.0);
}

#[test]
fn scheduled_triggers_respect_track_mute() {
    let state = SequencerState::new(2, Vec::new());
    state.pattern.track_params[1].set_mute(true);

    assert!(track_accepts_scheduled_trigger(&state, 0));
    assert!(!track_accepts_scheduled_trigger(&state, 1));
}

#[test]
fn scheduled_triggers_respect_solo_mutes() {
    let state = SequencerState::new(3, Vec::new());
    state.pattern.track_params[0].set_solo(true);

    assert!(track_accepts_scheduled_trigger(&state, 0));
    assert!(!track_accepts_scheduled_trigger(&state, 1));
    assert!(!track_accepts_scheduled_trigger(&state, 2));
}
