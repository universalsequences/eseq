use super::*;
use crate::audiograph as graph;
use crate::effects::{gatepitch as gp, EffectDescriptor, EffectSlotSnapshot};
use crate::sequencer::{default_rack_macros, RackSlotParamPlocks, TrackSoundState};

fn slot(instrument_type: InstrumentType) -> RackSlotSnapshot {
    RackSlotSnapshot {
        instrument_type,
        instrument_run_mode: CustomInstrumentRunMode::Instrument,
        instrument_base_note_offset: 12.0,
        choke_group: None,
        gain: 1.0,
        pan: 0.0,
        mute: false,
        solo: false,
        max_polyphony: 2,
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
fn rack_retrig_schedules_every_layer_and_chord_voice_with_resolved_gates() {
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    let lg = engine.lg_ptr.0;
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut data = new_audio_callback_data(
        lg, Arc::clone(&engine.state), 48_000, 2, 512,
        Arc::clone(&engine.master_recorder), rx,
        Arc::clone(&engine.buses.bus_effect_runtime),
        Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
    );
    data.state.transport.num_tracks.store(1, Ordering::Release);
    data.state.runtime.instrument_type_flags[0].store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
    if !data.state.pattern.track_params[0].is_gate_on() {
        data.state.pattern.track_params[0].toggle_gate();
    }
    let mut custom = slot(InstrumentType::Custom);
    custom.track_sound_state.engine_id = Some(0);
    let sampler = slot(InstrumentType::Sampler);
    let pool_id = rack_slot_pool_index(0, 1).unwrap();
    for voice_idx in 0..2 {
        // Native gatepitch nodes exercise the production custom-note event ABI
        // without compiling a synth or starting an audio device.
        let id = unsafe {
            graph::add_node(lg, gp::gatepitch_vtable(), gp::GATEPITCH_STATE_SIZE * 4,
                c"rack-retrig-custom".as_ptr(), 0, gp::OUTPUT_COUNT as i32, std::ptr::null(), 0)
        };
        assert!(id > 0);
        data.custom_engine_pools[0].add_voice(id as u64);
        data.state.runtime.engine_synth_node_ids[0][voice_idx].store(id as u32, Ordering::Release);
        data.state.runtime.engine_modulator_node_ids[0][voice_idx].store(id as u32, Ordering::Release);
        let id = unsafe {
            graph::add_node(lg, crate::instruments::sampler::sampler_vtable(),
                crate::instruments::sampler::SAMPLER_STATE_SIZE * 4,
                c"rack-retrig-sampler".as_ptr(), 0, 2, std::ptr::null(), 0)
        };
        assert!(id > 0);
        data.voice_pools[pool_id].add_voice(id as u64, id);
        data.state.runtime.sampler_lids[pool_id].store(id as u64, Ordering::Release);
    }
    let mut output = vec![0.0; 1024];
    unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
    let rack = RackTrackSnapshot::new(vec![custom, sampler], default_rack_macros());
    let resolved = crate::accumulator::ResolvedStep {
        duration: 1.0, velocity: 0.7, speed: 1.5, aux_a: 0.0, aux_b: 0.0,
        transpose: 5.0, pan: 0.0, chop: 8.0, retrig: 3.0, retrig_rate: 8.0,
    };
    let mut chord = crate::scheduled_event::ScheduledChordData {
        live_origins: [None; MAX_VOICES], count: 2, notes: [0.0; MAX_VOICES],
        durations: [0.0; MAX_VOICES], delays: [0.0; MAX_VOICES], step_transpose: 2.0,
    };
    chord.notes[0] = 2.0;
    chord.notes[1] = 9.0;
    chord.durations[0] = 0.1;
    let interval = retrig_interval_samples(&resolved, data.sample_rate, data.scheduler_snapshot.transport.bpm as f64);
    let fire = |data: &mut AudioCallbackData, resolved, chord| {
        fire_rack_resolved(data, 17, 0, 0, None, 6000.0, resolved, chord,
            rack.clone(), [None; crate::sequencer::RACK_MACRO_COUNT]);
    };
    fire(&mut data, resolved, chord);
    let repeats: Vec<_> = data.countdown_events.iter().filter_map(|event| {
        if let CountdownEventKind::Retrig(hit) = event.kind {
            assert_eq!(event.repeats, 3);
            assert_eq!(event.period_samples, interval);
            assert_eq!(event.remaining_samples, 17.0 + interval - 512.0);
            Some(hit)
        } else { None }
    }).collect();
    assert_eq!(repeats.len(), 4, "both chord notes in both layers must repeat");
    for (index, hit) in repeats.iter().enumerate() {
        assert_eq!(hit.gate, if index % 2 == 0 { 600.0 } else { interval.min(6000.0) as f32 });
        let transpose = if index % 2 == 0 { 5.0 } else { 12.0 };
        match hit.target {
            RetrigTarget::Custom { voices, count, engine_id, gated, .. } => {
                assert_eq!(index / 2, 0);
                assert_eq!(count, 1);
                assert_eq!(engine_id, 0);
                assert!(gated);
                assert_eq!(voices[0].pitch_hz, custom_pitch_hz(transpose, 12.0));
                assert_eq!(voices[0].velocity, 0.7);
            }
            RetrigTarget::RackSampler(voice) => {
                assert_eq!(index / 2, 1);
                assert_eq!(voice.pool_id, pool_id);
                assert_eq!(voice.transpose, transpose + 12.0);
                assert_eq!(voice.velocity, 0.7);
                assert_eq!(voice.speed, 1.5);
            }
            _ => panic!("rack repeats must not fall through to the parent sampler"),
        }
    }
    data.countdown_events.clear();
    data.block_events.clear();
    for voice in &mut data.voice_pools[pool_id].voices[..2] { voice.active = false; }
    for voice in &mut data.custom_engine_pools[0].voices[..2] {
        voice.active = false;
        voice.release_started_sample = Some(512);
    }
    for hit in repeats {
        dispatch_retrig_event(&mut data, hit, 31);
    }
    assert!(data.voice_pools[pool_id].voices[..2].iter().all(|voice| voice.active));
    assert!(data.custom_engine_pools[0].voices[..2].iter().all(|voice| voice.active && voice.release_started_sample.is_none()));
    assert_eq!(data.countdown_events.len(), 4, "each repeat re-arms its own gate-off");
    for event in &data.countdown_events {
        assert!(matches!(event.kind, CountdownEventKind::GateOff(_)));
        assert!(event.remaining_samples >= 119.0);
    }
    unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
    assert_eq!(unsafe { graph::graph_block_event_delivery_failures(lg) }, 0);

    // Infinite bursts are replaced by the next parent hit, including a hit
    // with retrig disabled. The obsolete Chop value must not shorten its gate.
    let infinite = crate::accumulator::ResolvedStep { retrig: crate::sequencer::RETRIG_INFINITE, ..resolved };
    fire(&mut data, infinite, chord);
    assert_eq!(data.countdown_events.iter().filter(|event| matches!(event.kind, CountdownEventKind::Retrig(_)) && event.repeats == u32::MAX).count(), 4);
    let releases = collect_rack_slot_active_voice_releases(
        &mut data.voice_pools, &mut data.custom_engine_pools,
        &mut data.countdown_events, &mut data.block_events,
        0, 0, &rack.slots[0], 512,
    );
    assert_eq!(releases.len(), 2);
    let remaining: Vec<_> = data.countdown_events.iter().filter_map(|event| match event.kind {
        CountdownEventKind::Retrig(hit) => Some(hit.target),
        _ => None,
    }).collect();
    assert_eq!(remaining.len(), 2, "choking a layer cancels only its voices' bursts");
    assert!(remaining.iter().all(|target| matches!(target, RetrigTarget::RackSampler(_))));
    let single = crate::scheduled_event::ScheduledChordData { count: 0, ..chord };
    fire(&mut data, crate::accumulator::ResolvedStep { retrig: 0.0, ..resolved }, single);
    assert!(!data.countdown_events.iter().any(|event| matches!(event.kind, CountdownEventKind::Retrig(_))));
    assert!(!data.block_events.iter().any(|event| matches!(event.kind, BlockEventKind::Retrig(_))));
    assert_eq!(data.countdown_events.iter().filter(|event| matches!(event.kind, CountdownEventKind::GateOff(_)) && event.remaining_samples == 5505.0).count(), 2);
    fire(&mut data, crate::accumulator::ResolvedStep { retrig_rate: 0.0, ..resolved }, single);
    assert!(!data.countdown_events.iter().any(|event| matches!(event.kind, CountdownEventKind::Retrig(_))));

    // A mono slot steals the first chord note for the second: its abandoned
    // burst must not re-excite that same lid at two pitches on every repeat.
    let mut mono_rack = rack.clone();
    for (slot_idx, slot) in mono_rack.slots.iter_mut().enumerate() {
        collect_rack_slot_active_voice_releases(
            &mut data.voice_pools, &mut data.custom_engine_pools,
            &mut data.countdown_events, &mut data.block_events, 0, slot_idx, slot, 1024,
        );
        slot.max_polyphony = 1;
    }
    fire_rack_resolved(&mut data, 17, 0, 0, None, 6000.0, resolved, chord,
        mono_rack, [None; crate::sequencer::RACK_MACRO_COUNT]);
    assert_eq!(data.countdown_events.iter().filter(|event| matches!(event.kind, CountdownEventKind::Retrig(_))).count(), 2);
}
