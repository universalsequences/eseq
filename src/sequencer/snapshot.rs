use std::sync::atomic::Ordering;

use crate::effects::EffectSlotSnapshot;

use super::data::{
    InstrumentType, StepParam, SwingResolution, Timebase, TrackParamsSnapshot, MAX_STEPS,
    NUM_PARAMS,
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
    pub timebase_override: Option<Timebase>,
    pub swing_override: Option<f32>,
    pub swing_resolution_override: Option<SwingResolution>,
}

#[derive(Clone, Debug)]
pub struct SequencerTrackSnapshot {
    pub params: TrackParamsSnapshot,
    pub instrument_type: InstrumentType,
    pub instrument_base_note_offset: f32,
    pub engine_id: Option<usize>,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub instrument_slot: EffectSlotSnapshot,
    pub steps: Vec<SequencerStepSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SequencerSnapshot {
    pub transport: SequencerTransportSnapshot,
    pub tracks: Vec<SequencerTrackSnapshot>,
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
        }
    }

    pub fn capture(state: &SequencerState) -> Self {
        let num_tracks = state.active_track_count();
        let transport = SequencerTransportSnapshot {
            bpm: state.transport.bpm.load(Ordering::Relaxed),
            playing: state.transport.playing.load(Ordering::Relaxed),
            current_pattern: state.pattern.current_pattern.load(Ordering::Relaxed) as usize,
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
                send: tp.get_send(),
                polyphonic: tp.is_polyphonic(),
                timebase: tp.get_timebase(),
                accumulator_idx: tp.get_accumulator_idx(),
                script_accumulator_name: tp.script_accumulator_name(),
                accum_limit: tp.get_accum_limit(),
                accum_mode: tp.get_accum_mode(),
                fts_scale: tp.get_fts_scale(),
            };
            let instrument_type =
                if state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed) == 1 {
                    InstrumentType::Custom
                } else {
                    InstrumentType::Sampler
                };
            let instrument_base_note_offset = f32::from_bits(
                state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
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
                for note_idx in 0..chord_count {
                    chord.push(state.pattern.chord_data[track_idx].get(step_idx, note_idx));
                }

                steps.push(SequencerStepSnapshot {
                    active: state.pattern.patterns[track_idx].is_active(step_idx),
                    params: step_params,
                    chord,
                    timebase_override: state.pattern.timebase_plocks[track_idx].get(step_idx),
                    swing_override: state.pattern.swing_plocks[track_idx].get(step_idx),
                    swing_resolution_override: state.pattern.swing_resolution_plocks[track_idx]
                        .get(step_idx),
                });
            }

            tracks.push(SequencerTrackSnapshot {
                params,
                instrument_type,
                instrument_base_note_offset,
                engine_id,
                effect_slots,
                instrument_slot,
                steps,
            });
        }

        Self { transport, tracks }
    }
}
