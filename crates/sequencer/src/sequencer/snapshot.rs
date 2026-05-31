use std::sync::atomic::Ordering;

use crate::effects::EffectSlotSnapshot;
use crate::neural::ProjectNeuralNetwork;

use super::data::{
    CustomInstrumentRunMode, InstrumentType, ModConnection, StepParam, SwingResolution, Timebase,
    TrackParamsSnapshot, MAX_STEPS, NUM_PARAMS,
};
use super::state::SequencerState;

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
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub instrument_base_note_offset: f32,
    pub engine_id: Option<usize>,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_slot: EffectSlotSnapshot,
    pub steps: Vec<SequencerStepSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SequencerSnapshot {
    pub transport: SequencerTransportSnapshot,
    pub tracks: Vec<SequencerTrackSnapshot>,
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
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
        }
    }

    pub fn capture(state: &SequencerState) -> Self {
        let num_tracks = state.active_track_count();
        let current_pattern = state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
        let transport = SequencerTransportSnapshot {
            bpm: state.transport.bpm.load(Ordering::Relaxed),
            playing: state.transport.playing.load(Ordering::Relaxed),
            current_pattern,
            pattern_epoch: state.transport.pattern_epoch.load(Ordering::Relaxed),
            topology_epoch: state.transport.topology_epoch.load(Ordering::Relaxed),
            num_tracks,
        };
        let mut tracks = Vec::with_capacity(num_tracks);

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
            let midi_fx_slots = state.pattern.midi_fx_slots[track_idx]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
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
                instrument_type,
                instrument_run_mode,
                instrument_base_note_offset,
                engine_id,
                effect_slots,
                midi_fx_slots,
                instrument_slot,
                steps,
            });
        }

        let (mod_connections, neural_networks) = state
            .pattern
            .pattern_bank
            .lock()
            .unwrap()
            .get(current_pattern)
            .map(|pattern| {
                (
                    pattern.mod_connections.clone(),
                    pattern.neural_networks.clone(),
                )
            })
            .unwrap_or_default();

        Self {
            transport,
            tracks,
            mod_connections,
            neural_networks,
        }
    }
}
