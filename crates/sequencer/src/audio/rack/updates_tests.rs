use super::*;
use crate::effects::EffectDescriptor;
use crate::sequencer::{default_rack_macros, RackMacroCurve, RackMacroMapping, RackMacroTarget};

fn mapped_rack() -> RackTrackSnapshot {
    let mut slot = off_step_solo_tests::slot();
    slot.instrument_slot = EffectSlotSnapshot::new_default(&EffectDescriptor::builtin_sampler(), 46);
    slot.effect_slots[0] = EffectSlotSnapshot::new_default(&EffectDescriptor::builtin_filter(), 47);
    let mut rack = RackTrackSnapshot::new(vec![slot.clone(), slot], default_rack_macros());
    for (id, rack_macro) in rack.macros.iter_mut().enumerate() {
        rack_macro.value = (id + 1) as f32 / 9.0;
        if id % 2 == 0 { rack_macro.plocks[4] = Some(0.8); }
        for slot in 0..2 {
            let targets = RackSlotParam::ALL.into_iter().map(|param| RackMacroTarget::SlotParam {
                slot, param: param.name().into(),
            }).chain((0..rack.slots[slot].instrument_slot.num_params as usize).map(|param_index| {
                RackMacroTarget::SlotInstrumentParam { slot, param: "test".into(), param_index }
            })).chain((0..rack.slots[slot].effect_slots[0].num_params as usize).map(|param_index| {
                RackMacroTarget::SlotEffectParam { slot, effect_slot: 0, param: "test".into(), param_index }
            }));
            for target in targets {
                rack_macro.mappings.push(RackMacroMapping {
                    target, range_min: -0.2, range_max: 1.7,
                    curve: [RackMacroCurve::Linear, RackMacroCurve::Exp, RackMacroCurve::Log][id % 3],
                });
            }
        }
    }
    rack.slots[0].param_plocks.rows[4][RackSlotParam::Gain.index()] = Some(0.37);
    rack.slots[1].param_plocks.rows[4][RackSlotParam::Solo.index()] = Some(0.0);
    for slot in &mut rack.slots {
        slot.instrument_slot.set_plock(4, 0, 42.0);
        slot.instrument_slot.set_plock(4, crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 1.0);
        slot.effect_slots[0].set_plock(4, 0, 333.0);
        // Stale identity: the macro must not turn an invalid stored lock
        // into a different default, nor dispatch the lock to a new node.
        slot.effect_slots[0].set_plock(5, 1, 0.2);
        slot.effect_slots[0].plock_param_ids[5][1] = None;
    }
    rack
}

#[test]
fn borrowed_rack_values_match_macro_lock_and_live_precedence() {
    let rack = mapped_rack();
    for mode in 0..4 {
        for step in [0, 4, 5] {
            for printing in [false, true] {
                let mut print = [None; crate::sequencer::RACK_MACRO_COUNT];
                if printing { print[7] = Some(0.5); }
                let mut process = [None; crate::sequencer::RACK_MACRO_COUNT];
                process[7] = Some(0.9);
                let mut reference = rack.clone();
                let view = match mode {
                    0 => {
                        apply_rack_macros_at_step(&mut reference, step, process, print);
                        RackParams::at_step(&rack, step, process, print)
                    }
                    1 => {
                        apply_rack_macros_live(&mut reference, print);
                        RackParams::live(&rack, print)
                    }
                    _ => {
                        let step = (mode == 2).then_some(step);
                        apply_rack_macros_for_update(&mut reference, step, print);
                        RackParams::for_update(&rack, step, print)
                    }
                };
                for (slot_idx, expected) in reference.slots.iter().enumerate() {
                    assert_eq!(view.slot_params(slot_idx), resolve_rack_slot_params_for_update(expected, view.step));
                    let instrument = view.step.map_or_else(
                        || resolve_rack_slot_instrument_defaults(&expected.instrument_slot),
                        |step| resolve_rack_slot_instrument_params(&expected.instrument_slot, step));
                    assert_eq!(view.instrument_params(slot_idx), instrument);
                    let sampler = view.step.map_or_else(
                        || resolve_rack_slot_sampler_defaults(&expected.instrument_slot),
                        |step| resolve_rack_slot_sampler_params(&expected.instrument_slot, step));
                    assert_eq!(view.sampler_params(slot_idx), sampler);
                    for (effect_idx, effect) in expected.effect_slots.iter().enumerate() {
                        for param in 0..effect.num_params as usize {
                            let value = view.step.map_or(effect.defaults[param], |step| {
                                resolved_slot_param_value(effect, step, param, effect.defaults[param])
                            });
                            assert_eq!(view.effect(slot_idx, effect_idx).value(param, 0.0), value);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rack_resolution_does_not_allocate_or_free_even_at_maximum_parameter_count() {
    let mut rack = mapped_rack();
    let slot = &mut rack.slots[0].instrument_slot;
    slot.num_params = MAX_SLOT_PARAMS as u32;
    slot.defaults = vec![0.4; MAX_SLOT_PARAMS];
    // Reverse order and duplicate addresses exercise stable tie ordering.
    slot.param_node_indices = (0..MAX_SLOT_PARAMS as u32).rev().map(|idx| idx / 2).collect();
    slot.param_node_spans = vec![1; MAX_SLOT_PARAMS];
    let (_, reference) = crate::test_alloc::measure(|| drop(std::hint::black_box(rack.clone())));
    assert!(reference.allocations > 100 && reference.deallocations > 100,
        "negative control must detect the former deep copy: {reference:?}");
    let (_, counts) = crate::test_alloc::measure(|| {
        for view in [
            RackParams::at_step(&rack, 4, [None; 8], [None; 8]),
            RackParams::live(&rack, [None; 8]),
            RackParams::for_update(&rack, Some(4), [None; 8]),
            RackParams::for_update(&rack, None, [Some(0.7); 8]),
        ] {
            for slot in 0..rack.slots.len() {
                std::hint::black_box(view.slot_params(slot));
                std::hint::black_box(view.sampler_params(slot));
                std::hint::black_box(view.instrument_params(slot));
                let device = view.instrument(slot);
                std::hint::black_box(resolve_rack_slot_instrument_updates(
                    device.slot, view.step, |param| device.macro_value(param)));
            }
        }
    });
    assert_eq!(counts, crate::test_alloc::Counts::default());
}

#[test]
fn rack_update_and_release_dispatch_do_not_allocate_or_free() {
    use crate::audiograph as graph;
    use crate::effects::{filter, stereo_panner as pan};
    use crate::instruments::sampler;
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let lg = engine.lg_ptr.0;
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut data = new_audio_callback_data(
        lg, Arc::clone(&engine.state), 48_000, 2, 512,
        Arc::clone(&engine.master_recorder), rx,
        Arc::clone(&engine.buses.bus_effect_runtime),
        Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
    );
    let mut rack = mapped_rack();
    for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
        let add = |vtable, cells, inputs, outputs| unsafe {
            graph::add_node(lg, vtable, cells * 4, c"rack-allocation-test".as_ptr(),
                inputs, outputs, std::ptr::null(), 0) as u32
        };
        let sampler_id = add(sampler::sampler_vtable(), sampler::SAMPLER_STATE_SIZE,
            crate::instruments::voice_modulator::SLOT_COUNT as i32, 2);
        let filter_id = add(filter::filter_vtable(), filter::FILTER_STATE_SIZE, 6, 2);
        let pan_id = add(pan::stereo_panner_vtable(), pan::STEREO_PANNER_STATE_SIZE, 2, 2);
        unsafe {
            for channel in 0..2 {
                assert!(graph::graph_connect(lg, sampler_id as i32, channel, filter_id as i32, channel));
                assert!(graph::graph_connect(lg, filter_id as i32, channel, pan_id as i32, channel));
                assert!(graph::graph_connect(lg, pan_id as i32, channel, 0, channel));
            }
        }
        let pool = rack_slot_pool_index(0, slot_idx).unwrap();
        data.voice_pools[pool].add_voice(sampler_id as u64, 0);
        data.voice_pools[pool].allocate_voice(0.0);
        data.state.runtime.sampler_lids[pool].store(sampler_id as u64, Ordering::Release);
        data.state.runtime.rack_slot_pan_lids[0][slot_idx].store(pan_id as u64, Ordering::Release);
        slot.instrument_slot.sync_to_descriptor(&EffectDescriptor::builtin_sampler(), sampler_id);
        slot.effect_slots[0].sync_to_descriptor(&EffectDescriptor::builtin_filter(), filter_id);
        slot.effect_slots[0].set_plock(4, 0, 333.0);
    }
    let filter = rack.slots[0].effect_slots[0].node_id;
    let cutoff = rack.slots[0].effect_slots[0].node_param_idx(0).unwrap();
    unsafe { assert!(graph::add_node_to_watchlist(lg, filter as i32)); }
    data.state.transport.num_tracks.store(1, Ordering::Release);
    data.state.publish_scheduler_snapshot();
    data.scheduler_snapshot = data.state.latest_scheduler_snapshot();
    let snapshot = Arc::make_mut(&mut data.scheduler_snapshot);
    Arc::make_mut(&mut snapshot.tracks[0]).rack_track = Some(rack);
    let mut output = vec![0.0; 1024];
    // The graph publishes watch snapshots every four blocks, independently
    // of when parameter messages are applied. Advance that fixed cadence
    // before reading the monitor, rather than testing a stale snapshot.
    let render_for_watch = |output: &mut [f32]| {
        for _ in 0..4 {
            unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
        }
    };
    render_for_watch(&mut output);

    let (_, counts) = crate::test_alloc::measure(|| apply_rack_params_off_step(&mut data, 0, 4));
    assert_eq!(counts, crate::test_alloc::Counts::default());
    render_for_watch(&mut output);
    // The lock still reaches the real DSP node after the borrowed update.
    let mut values = [0.0_f32; filter::FILTER_STATE_SIZE];
    let mut size = 0;
    unsafe { assert!(graph::get_node_state_into(lg, filter as i32,
        values.as_mut_ptr().cast(), std::mem::size_of_val(&values), &mut size)); }
    assert_eq!(values[cutoff as usize], 333.0);

    let id = crate::sequencer::RackMacroId::from_index(7).unwrap();
    data.state.take_rack_macro_override.set(0, id, 0.5);
    let (_, counts) = crate::test_alloc::measure(|| apply_take_rack_macro_updates(&mut data));
    assert_eq!(counts, crate::test_alloc::Counts::default());
    render_for_watch(&mut output);
    unsafe { assert!(graph::get_node_state_into(lg, filter as i32,
        values.as_mut_ptr().cast(), std::mem::size_of_val(&values), &mut size)); }
    let expected = -0.2 + 1.9 * rack_macro_curve_value(RackMacroCurve::Exp, 0.5);
    assert!((values[cutoff as usize] - expected).abs() < 1e-6);

    let snapshot = Arc::clone(&data.scheduler_snapshot);
    let rack = snapshot.tracks[0].rack_track.as_ref().unwrap();
    let trigger = KeyboardTrigger {
        generation: 1, source: None, track: 0, transpose: 0.0, velocity: 0.7, note_off: false,
    };
    let (fired, counts) = crate::test_alloc::measure(|| {
        fire_live_keyboard_rack_note(&mut data, 0, &trigger, 0.0, rack)
    });
    assert!(fired);
    assert_eq!(counts, crate::test_alloc::Counts::default());

    let (_, counts) = crate::test_alloc::measure(|| release_rack_active_voices(&mut data, 0, 1024, 0));
    assert_eq!(counts, crate::test_alloc::Counts::default());
    for slot in 0..2 {
        assert!(!data.voice_pools[rack_slot_pool_index(0, slot).unwrap()].voices[0].active);
    }
    drop(snapshot);
    drop(data);
    unsafe { engine.destroy(); }
}

#[test]
fn borrowed_rack_note_reads_live_macro_edits_without_republishing() {
    let state = SequencerState::new(1, vec![Vec::new()]);
    let rack = mapped_rack();
    state.set_rack_track_for_all_pattern_snapshots(0, rack);
    state.publish_scheduler_snapshot();
    let snapshot = state.latest_scheduler_snapshot();
    let rack = snapshot.tracks[0].rack_track.as_ref().unwrap();
    let id = crate::sequencer::RackMacroId::from_index(7).unwrap();
    for value in [0.0, 0.25, 0.75, 1.0] {
        state.set_live_rack_macro_default(0, id, value);
        let (_, counts) = crate::test_alloc::measure(|| {
            let live = RackParams::live(rack, [None; 8]);
            let expected = RackSlotParam::Gain.clamp(-0.2 + 1.9 * value * value);
            assert!((live.slot_params(0).gain - expected).abs() < 1e-6);
        });
        assert_eq!(counts, crate::test_alloc::Counts::default());
    }
}

/// eseq-bw9v: a disabled slot is a parked instrument. It must not take a
/// voice from a trigger, and its FX chain must be bypassed on the node so a
/// silent chain is not rendered block after block.
#[test]
fn disabled_rack_slot_takes_no_voice_and_bypasses_its_effects() {
    use crate::audiograph as graph;
    use crate::effects::{filter, stereo_panner as pan};
    use crate::instruments::sampler;
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let lg = engine.lg_ptr.0;
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut data = new_audio_callback_data(
        lg, Arc::clone(&engine.state), 48_000, 2, 512,
        Arc::clone(&engine.master_recorder), rx,
        Arc::clone(&engine.buses.bus_effect_runtime),
        Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
    );
    let mut slot = off_step_solo_tests::slot();
    slot.instrument_slot = EffectSlotSnapshot::new_default(&EffectDescriptor::builtin_sampler(), 46);
    slot.effect_slots[0] = EffectSlotSnapshot::new_default(&EffectDescriptor::builtin_filter(), 47);
    // The bypass push finds each effect's `enabled` through the descriptor
    // the graph build pairs with the slot, so keep the two aligned.
    slot.effect_descriptors[0] = EffectDescriptor::builtin_filter();
    let mut rack = RackTrackSnapshot::new(vec![slot.clone(), slot], default_rack_macros());
    rack.slots[1].enabled = false;
    let mut filters = Vec::new();
    for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
        let add = |vtable, cells, inputs, outputs| unsafe {
            graph::add_node(lg, vtable, cells * 4, c"rack-enabled-test".as_ptr(),
                inputs, outputs, std::ptr::null(), 0) as u32
        };
        let sampler_id = add(sampler::sampler_vtable(), sampler::SAMPLER_STATE_SIZE,
            crate::instruments::voice_modulator::SLOT_COUNT as i32, 2);
        let filter_id = add(filter::filter_vtable(), filter::FILTER_STATE_SIZE, 6, 2);
        let pan_id = add(pan::stereo_panner_vtable(), pan::STEREO_PANNER_STATE_SIZE, 2, 2);
        unsafe {
            for channel in 0..2 {
                assert!(graph::graph_connect(lg, sampler_id as i32, channel, filter_id as i32, channel));
                assert!(graph::graph_connect(lg, filter_id as i32, channel, pan_id as i32, channel));
                assert!(graph::graph_connect(lg, pan_id as i32, channel, 0, channel));
            }
            assert!(graph::add_node_to_watchlist(lg, filter_id as i32));
        }
        let pool = rack_slot_pool_index(0, slot_idx).unwrap();
        data.voice_pools[pool].add_voice(sampler_id as u64, 0);
        data.state.runtime.sampler_lids[pool].store(sampler_id as u64, Ordering::Release);
        data.state.runtime.rack_slot_pan_lids[0][slot_idx].store(pan_id as u64, Ordering::Release);
        slot.instrument_slot.sync_to_descriptor(&EffectDescriptor::builtin_sampler(), sampler_id);
        slot.effect_slots[0].sync_to_descriptor(&EffectDescriptor::builtin_filter(), filter_id);
        filters.push(filter_id as i32);
    }
    data.state.transport.num_tracks.store(1, Ordering::Release);
    data.state.publish_scheduler_snapshot();
    data.scheduler_snapshot = data.state.latest_scheduler_snapshot();
    let snapshot = Arc::make_mut(&mut data.scheduler_snapshot);
    Arc::make_mut(&mut snapshot.tracks[0]).rack_track = Some(rack);
    let mut output = vec![0.0; 1024];
    let render_for_watch = |output: &mut [f32]| {
        for _ in 0..4 {
            unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
        }
    };
    // A built chain starts with every effect running, as the graph build
    // pushes the slot's stored `enabled` on creation.
    for &filter_id in &filters {
        unsafe {
            graph::params_push_wrapper(lg, graph::ParamMsg {
                idx: filter::FILTER_PARAM_ENABLED,
                logical_id: filter_id as u64,
                fvalue: 1.0,
            });
        }
    }
    render_for_watch(&mut output);

    // Off-step param push: the enabled slot's filter keeps its stored
    // enabled value, the disabled slot's filter is bypassed.
    apply_rack_params_off_step(&mut data, 0, 4);
    render_for_watch(&mut output);
    let filter_enabled = |filter_id: i32| {
        let mut values = [0.0_f32; filter::FILTER_STATE_SIZE];
        let mut size = 0;
        unsafe { assert!(graph::get_node_state_into(lg, filter_id,
            values.as_mut_ptr().cast(), std::mem::size_of_val(&values), &mut size)); }
        values[filter::FILTER_PARAM_ENABLED as usize]
    };
    assert_eq!(filter_enabled(filters[0]), 1.0, "enabled slot keeps its effect running");
    assert_eq!(filter_enabled(filters[1]), 0.0, "disabled slot bypasses its effect");

    // A live keyboard note only reaches the enabled slot.
    let snapshot = Arc::clone(&data.scheduler_snapshot);
    let rack = snapshot.tracks[0].rack_track.as_ref().unwrap();
    let trigger = KeyboardTrigger {
        generation: 1, source: None, track: 0, transpose: 0.0, velocity: 0.7, note_off: false,
    };
    // Simulate the enabled slot having been parked before a pattern switch:
    // its filter is still bypassed on the node.
    unsafe {
        graph::params_push_wrapper(lg, graph::ParamMsg {
            idx: filter::FILTER_PARAM_ENABLED,
            logical_id: filters[0] as u64,
            fvalue: 0.0,
        });
    }
    render_for_watch(&mut output);
    assert!(fire_live_keyboard_rack_note(&mut data, 0, &trigger, 0.0, rack));
    assert!(data.voice_pools[rack_slot_pool_index(0, 0).unwrap()].voices[0].active);
    assert!(
        !data.voice_pools[rack_slot_pool_index(0, 1).unwrap()].voices[0].active,
        "disabled slot must not take a voice"
    );
    render_for_watch(&mut output);
    assert_eq!(filter_enabled(filters[0]), 1.0, "a live key restores the enabled slot's FX");
    assert_eq!(filter_enabled(filters[1]), 0.0);
    drop(snapshot);

    // Parking the sounding slot releases its voice rather than leaving it
    // ringing dry through the bypassed chain.
    release_newly_disabled_rack_slots(&mut data, 0);
    assert!(data.voice_pools[rack_slot_pool_index(0, 0).unwrap()].voices[0].active,
        "an unchanged enabled slot keeps its voice");
    let snapshot = Arc::make_mut(&mut data.scheduler_snapshot);
    Arc::make_mut(&mut snapshot.tracks[0]).rack_track.as_mut().unwrap().slots[0].enabled = false;
    release_newly_disabled_rack_slots(&mut data, 0);
    assert!(!data.voice_pools[rack_slot_pool_index(0, 0).unwrap()].voices[0].active,
        "a newly disabled slot releases its sounding voice");
    drop(data);
    unsafe { engine.destroy(); }
}

/// eseq-bw9v: a parked slot never sounds, so its solo must not mute the
/// enabled slots beside it.
#[test]
fn disabled_soloed_slot_does_not_mute_enabled_slots() {
    let mut rack = RackTrackSnapshot::new(
        vec![off_step_solo_tests::slot(), off_step_solo_tests::slot()],
        default_rack_macros(),
    );
    rack.slots[0].solo = true;
    assert!(rack_has_enabled_solo(&RackParams::live(&rack, [None; 8])));
    rack.slots[0].enabled = false;
    assert!(!rack_has_enabled_solo(&RackParams::live(&rack, [None; 8])));
    assert!(!rack_has_enabled_solo(&RackParams::at_step(&rack, 0, [None; 8], [None; 8])));
}

/// eseq-bw9v: `enabled` is a per-pattern authoring value like mute/solo, so
/// history replay and "copy current values to all scenes" carry it.
#[test]
fn slot_enabled_flag_survives_history_replay_and_scene_copy() {
    let mut slot = off_step_solo_tests::slot();
    let before = slot.authoring_values();
    slot.enabled = false;
    let after = slot.authoring_values();
    slot.apply_authoring_values(&before).unwrap();
    assert!(slot.enabled, "undo restores the enable flag");
    slot.apply_authoring_values(&after).unwrap();
    assert!(!slot.enabled, "redo restores the enable flag");

    let mut target = off_step_solo_tests::slot();
    target.copy_scene_values_from(&slot);
    assert!(!target.enabled, "copy to all scenes carries the enable flag");
}
