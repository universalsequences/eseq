use super::*;

#[derive(Clone, Debug)]
pub struct StepSnapshot {
    pub active: bool,
    pub neural_reset: bool,
    pub params: [f32; NUM_PARAMS],
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_delays: Vec<f32>,
    pub timebase: Option<Timebase>,
    pub swing: Option<f32>,
    pub swing_resolution: Option<SwingResolution>,
    pub track_send_plocks: Vec<TrackSendSnapshot>,
    pub midi_fx_plocks: Vec<StepSlotPlocks>,
    pub effect_plocks: Vec<StepSlotPlocks>,
    pub instrument_plocks: StepSlotPlocks,
    pub rack_macro_plocks: Vec<Option<f32>>,
    pub rack_slot_param_plocks: Vec<StepSlotPlocks>,
    pub rack_slot_instrument_plocks: Vec<StepSlotPlocks>,
    pub rack_slot_effect_plocks: Vec<Vec<StepSlotPlocks>>,
}

pub type StepCellSnapshot = StepSnapshot;

impl StepSnapshot {
    pub fn without_audio_plocks(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.track_send_plocks.clear();
        for plocks in &mut snapshot.midi_fx_plocks {
            plocks.clear();
        }
        for plocks in &mut snapshot.effect_plocks {
            plocks.clear();
        }
        snapshot.instrument_plocks.clear();
        snapshot.rack_macro_plocks.fill(None);
        for plocks in &mut snapshot.rack_slot_param_plocks {
            plocks.clear();
        }
        for plocks in &mut snapshot.rack_slot_instrument_plocks {
            plocks.clear();
        }
        for slot in &mut snapshot.rack_slot_effect_plocks {
            for plocks in slot {
                plocks.clear();
            }
        }
        snapshot
    }
}

pub(super) fn capture_live_slot_step_plocks(slot: &EffectSlotState, step: usize) -> StepSlotPlocks {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let params = (0..num_params)
        .map(|param_idx| slot.plocks.get(step, param_idx))
        .collect();
    let tensor_params = (0..slot.tensor_params.num_params())
        .map(|tensor_idx| slot.tensor_params.plock_values(step, tensor_idx))
        .collect();
    StepSlotPlocks {
        params,
        tensor_params,
    }
}

pub(super) fn capture_snapshot_slot_step_plocks(
    slot: &EffectSlotSnapshot,
    step: usize,
) -> StepSlotPlocks {
    let params = (0..slot.num_params as usize)
        .map(|param_idx| {
            slot.plocks
                .get(step)
                .and_then(|row| row.get(param_idx))
                .copied()
                .flatten()
        })
        .collect();
    let tensor_params = (0..slot.tensor_params.len())
        .map(|tensor_idx| slot.tensor_plock_values(step, tensor_idx).map(<[f32]>::to_vec))
        .collect();
    StepSlotPlocks {
        params,
        tensor_params,
    }
}

pub(super) fn restore_live_slot_step_plocks(
    slot: &EffectSlotState,
    step: usize,
    saved: Option<&StepSlotPlocks>,
) {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        match saved
            .and_then(|plocks| plocks.params.get(param_idx))
            .copied()
            .flatten()
        {
            Some(value) => slot.set_plock(step, param_idx, value),
            None => slot.plocks.clear_param(step, param_idx),
        }
    }
    for tensor_idx in 0..slot.tensor_params.num_params() {
        let values = saved
            .and_then(|plocks| plocks.tensor_params.get(tensor_idx))
            .cloned()
            .flatten();
        if values.as_deref().is_none_or(|values| {
            !slot.tensor_params.set_plock(step, tensor_idx, values)
        }) {
            slot.tensor_params.clear_plock(step, tensor_idx);
        }
    }
}

pub(super) fn restore_snapshot_slot_step_plocks(
    slot: &mut EffectSlotSnapshot,
    step: usize,
    saved: Option<&StepSlotPlocks>,
) {
    for param_idx in 0..slot.num_params as usize {
        match saved
            .and_then(|plocks| plocks.params.get(param_idx))
            .copied()
            .flatten()
        {
            Some(value) => {
                slot.set_plock(step, param_idx, value);
            }
            None => {
                slot.clear_plock(step, param_idx);
            }
        }
    }
    for tensor_idx in 0..slot.tensor_params.len() {
        let values = saved
            .and_then(|plocks| plocks.tensor_params.get(tensor_idx))
            .cloned()
            .flatten();
        let restored = values
            .map(|values| slot.set_tensor_plock(step, tensor_idx, values))
            .unwrap_or(false);
        if !restored {
            slot.clear_tensor_plock(step, tensor_idx);
        }
    }
}
