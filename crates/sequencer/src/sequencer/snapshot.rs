use std::sync::atomic::Ordering;

use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::graph::ProjectGraphOverrides;
use crate::neural::ProjectNeuralNetwork;

use super::data::{
    CustomInstrumentRunMode, InstrumentType, ModConnection, StepParam, SwingResolution, Timebase,
    TrackParamsSnapshot, MAX_STEPS, NUM_PARAMS,
};
use super::state::{RackTrackSnapshot, SequencerState, TrackPatternData};

#[derive(Clone, Debug)]
pub struct SequencerTransportSnapshot {
    pub bpm: u32,
    pub playing: bool,
    pub current_pattern: usize,
    pub pattern_epoch: u64,
    pub topology_epoch: u64,
    pub num_tracks: usize,
}

#[derive(Clone, Debug)]
pub struct SequencerStepSnapshot {
    pub active: bool,
    pub neural_reset: bool,
    pub params: [f32; NUM_PARAMS],
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_delays: Vec<f32>,
    pub timebase_override: Option<Timebase>,
    pub swing_override: Option<f32>,
    pub swing_resolution_override: Option<SwingResolution>,
}

#[derive(Clone, Debug)]
pub struct SequencerTrackSnapshot {
    pub params: TrackParamsSnapshot,
    pub scene_silenced: bool,
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub instrument_base_note_offset: f32,
    pub engine_id: Option<usize>,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
    pub effect_descriptors: Vec<EffectDescriptor>,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_descriptor: EffectDescriptor,
    pub instrument_slot: EffectSlotSnapshot,
    pub steps: Vec<SequencerStepSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SequencerSnapshot {
    pub transport: SequencerTransportSnapshot,
    pub tracks: Vec<SequencerTrackSnapshot>,
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    pub process_trace: bool,
}

impl SequencerSnapshot {
    pub fn empty() -> Self {
        Self {
            transport: SequencerTransportSnapshot {
                bpm: 0,
                playing: false,
                current_pattern: 0,
                pattern_epoch: 0,
                topology_epoch: 0,
                num_tracks: 0,
            },
            tracks: Vec::new(),
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            process_trace: false,
        }
    }

    pub fn capture(state: &SequencerState) -> Self {
        let num_tracks = state.active_track_count();
        let current_pattern = state.current_scene_index();
        let transport = SequencerTransportSnapshot {
            bpm: state.transport.bpm.load(Ordering::Relaxed),
            playing: state.transport.playing.load(Ordering::Relaxed),
            current_pattern,
            pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            topology_epoch: state.transport.topology_epoch.load(Ordering::Relaxed),
            num_tracks,
        };
        let mut tracks = Vec::with_capacity(num_tracks);
        let live_rack_tracks = state.pattern.rack_tracks.lock().unwrap();
        let live_process_chains = state.pattern.process_chains.lock().unwrap();
        let (effect_descriptors_by_track, instrument_descriptors) =
            state.scratch_runtime_descriptors();

        for track_idx in 0..num_tracks {
            let tp = &state.pattern.track_params[track_idx];
            let params = TrackParamsSnapshot {
                gate: tp.is_gate_on(),
                attack_ms: tp.get_attack_ms(),
                release_ms: tp.get_release_ms(),
                swing: tp.get_swing(),
                swing_resolution: tp.get_swing_resolution(),
                num_steps: tp.get_num_steps(),
                volume: tp.get_volume(),
                pan: tp.get_pan(),
                mute: tp.is_muted(),
                solo: tp.is_solo(),
                send: tp.get_send(),
                output: tp.output(),
                sends: tp.sends(),
                polyphonic: tp.is_polyphonic(),
                max_polyphony: tp.get_max_polyphony(),
                timebase: tp.get_timebase(),
                accumulator_idx: tp.get_accumulator_idx(),
                script_accumulator_name: tp.script_accumulator_name(),
                midi_fx_chain: tp.midi_fx_chain(),
                midi_fx_position: tp.get_midi_fx_position(),
                accum_limit: tp.get_accum_limit(),
                accum_mode: tp.get_accum_mode(),
                fts_scale: tp.get_fts_scale(),
                mute_group: tp.get_mute_group(),
                global_transpose: tp.uses_global_transpose(),
            };
            let instrument_type = InstrumentType::from_runtime_flag(
                state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
            );
            let instrument_base_note_offset = f32::from_bits(
                state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
            );
            let instrument_run_mode = CustomInstrumentRunMode::from_runtime_flag(
                state.runtime.instrument_run_mode_flags[track_idx].load(Ordering::Relaxed),
            );
            let engine_id = match state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed)
            {
                u32::MAX => None,
                id => Some(id as usize),
            };
            let effect_slots = state.pattern.effect_chains[track_idx]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
            let effect_descriptors = effect_descriptors_by_track
                .get(track_idx)
                .cloned()
                .unwrap_or_else(EffectDescriptor::default_full_chain);
            let midi_fx_slots = state.pattern.midi_fx_slots[track_idx]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
            let instrument_descriptor = instrument_descriptors
                .get(track_idx)
                .cloned()
                .unwrap_or_else(EffectDescriptor::builtin_sampler);
            let instrument_slot =
                EffectSlotSnapshot::capture(&state.pattern.instrument_slots[track_idx]);

            let mut steps = Vec::with_capacity(MAX_STEPS);
            for step_idx in 0..MAX_STEPS {
                let mut step_params = [0.0f32; NUM_PARAMS];
                for param in StepParam::ALL {
                    step_params[param.index()] =
                        state.pattern.step_data[track_idx].get(step_idx, param);
                }

                let chord_count = state.pattern.chord_data[track_idx].count(step_idx);
                let mut chord = Vec::with_capacity(chord_count);
                let mut chord_durations = Vec::with_capacity(chord_count);
                let mut chord_delays = Vec::with_capacity(chord_count);
                for note_idx in 0..chord_count {
                    chord.push(state.pattern.chord_data[track_idx].get(step_idx, note_idx));
                    chord_durations
                        .push(state.pattern.chord_data[track_idx].get_duration(step_idx, note_idx));
                    chord_delays
                        .push(state.pattern.chord_data[track_idx].get_delay(step_idx, note_idx));
                }

                steps.push(SequencerStepSnapshot {
                    active: state.pattern.patterns[track_idx].is_active(step_idx),
                    neural_reset: state.pattern.neural_reset_patterns[track_idx]
                        .is_active(step_idx),
                    params: step_params,
                    chord,
                    chord_durations,
                    chord_delays,
                    timebase_override: state.pattern.timebase_plocks[track_idx].get(step_idx),
                    swing_override: state.pattern.swing_plocks[track_idx].get(step_idx),
                    swing_resolution_override: state.pattern.swing_resolution_plocks[track_idx]
                        .get(step_idx),
                });
            }

            tracks.push(SequencerTrackSnapshot {
                params,
                scene_silenced: state.is_scene_silenced(track_idx),
                instrument_type,
                instrument_run_mode,
                instrument_base_note_offset,
                engine_id,
                rack_track: live_rack_tracks.get(track_idx).cloned().unwrap_or(None),
                process_chain: live_process_chains
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_default(),
                effect_descriptors,
                effect_slots,
                midi_fx_slots,
                instrument_descriptor,
                instrument_slot,
                steps,
            });
        }

        let (mod_connections, neural_networks, graph_overrides) = state.current_scene_metadata();

        Self {
            transport,
            tracks,
            mod_connections,
            neural_networks,
            graph_overrides,
            process_trace: state.process_trace_enabled(),
        }
    }

    pub fn capture_from_track_pattern_data(
        state: &SequencerState,
        tracks: &[TrackPatternData],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
    ) -> Self {
        let num_tracks = tracks.len();
        let transport = SequencerTransportSnapshot {
            bpm: state.transport.bpm.load(Ordering::Relaxed),
            playing: state.transport.playing.load(Ordering::Relaxed),
            current_pattern: state.current_scene_index(),
            pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            topology_epoch: state.transport.topology_epoch.load(Ordering::Relaxed),
            num_tracks,
        };
        let (effect_descriptors_by_track, instrument_descriptors) =
            state.scratch_runtime_descriptors();
        let tracks = tracks
            .iter()
            .enumerate()
            .map(|(track_idx, data)| {
                let effect_descriptors = effect_descriptors_by_track
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_else(EffectDescriptor::default_full_chain);
                let instrument_descriptor = instrument_descriptors
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_else(EffectDescriptor::builtin_sampler);
                track_snapshot_from_pattern_data(
                    data,
                    false,
                    effect_descriptors,
                    instrument_descriptor,
                )
            })
            .collect();

        Self {
            transport,
            tracks,
            mod_connections,
            neural_networks,
            graph_overrides,
            process_trace: state.process_trace_enabled(),
        }
    }
}

fn track_snapshot_from_pattern_data(
    data: &TrackPatternData,
    scene_silenced: bool,
    effect_descriptors: Vec<EffectDescriptor>,
    instrument_descriptor: EffectDescriptor,
) -> SequencerTrackSnapshot {
    let engine_id = data.track_sound_state.engine_id;
    let process_chain =
        data.refreshed_process_chain(Some(&instrument_descriptor), &effect_descriptors);
    let steps = (0..MAX_STEPS)
        .map(|step_idx| {
            let params = data
                .step_data
                .get(step_idx)
                .copied()
                .unwrap_or([0.0; NUM_PARAMS]);
            SequencerStepSnapshot {
                active: track_pattern_bit(data.track_bits, step_idx),
                neural_reset: track_pattern_bit(data.neural_reset_bits, step_idx),
                params,
                chord: data
                    .chord_snapshot
                    .steps
                    .get(step_idx)
                    .cloned()
                    .unwrap_or_default(),
                chord_durations: data
                    .chord_snapshot
                    .durations
                    .get(step_idx)
                    .cloned()
                    .unwrap_or_default(),
                chord_delays: data
                    .chord_snapshot
                    .delays
                    .get(step_idx)
                    .cloned()
                    .unwrap_or_default(),
                timebase_override: data.timebase_plock_snapshot[step_idx].map(Timebase::from_index),
                swing_override: data.swing_plock_snapshot[step_idx].map(f32::from_bits),
                swing_resolution_override: data.swing_resolution_plock_snapshot[step_idx]
                    .map(SwingResolution::from_index),
            }
        })
        .collect();

    SequencerTrackSnapshot {
        params: data.track_params.clone(),
        scene_silenced,
        instrument_type: data.instrument_type,
        instrument_run_mode: data.instrument_run_mode,
        instrument_base_note_offset: data.instrument_base_note_offset,
        engine_id,
        rack_track: data.rack_track.clone(),
        process_chain,
        effect_descriptors,
        effect_slots: data.effect_slots.clone(),
        midi_fx_slots: data.midi_fx_slots.clone(),
        instrument_descriptor,
        instrument_slot: data.instrument_slot.clone(),
        steps,
    }
}

fn track_pattern_bit(bits: [u64; super::data::TRACK_PATTERN_WORDS], step: usize) -> bool {
    let word = step / 64;
    let bit = step % 64;
    bits.get(word)
        .is_some_and(|word_bits| (word_bits & (1u64 << bit)) != 0)
}
