use super::*;

pub(super) fn option_f32_bits_equal(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn f32_slices_bits_equal(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

pub(super) fn clear_track_variant_locks(state: &SequencerState, track: usize, step: usize) -> bool {
    let mut changed = false;
    if state.pattern.timebase_plocks[track].has_plock(step) {
        state.pattern.timebase_plocks[track].clear(step);
        changed = true;
    }
    if state.pattern.swing_plocks[track].has_plock(step) {
        state.pattern.swing_plocks[track].clear(step);
        changed = true;
    }
    if state.pattern.swing_resolution_plocks[track].has_plock(step) {
        state.pattern.swing_resolution_plocks[track].clear(step);
        changed = true;
    }
    changed
}

pub(super) fn copy_track_variant_locks(
    state: &SequencerState,
    track: usize,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let source_timebase = state.pattern.timebase_plocks[track].get(source_step);
    let target_timebase = state.pattern.timebase_plocks[track].get(target_step);
    if source_timebase != target_timebase {
        match source_timebase {
            Some(value) => state.pattern.timebase_plocks[track].set(target_step, value),
            None => state.pattern.timebase_plocks[track].clear(target_step),
        }
        changed = true;
    }

    let source_swing = state.pattern.swing_plocks[track].get(source_step);
    let target_swing = state.pattern.swing_plocks[track].get(target_step);
    if !option_f32_bits_equal(source_swing, target_swing) {
        match source_swing {
            Some(value) => state.pattern.swing_plocks[track].set(target_step, value),
            None => state.pattern.swing_plocks[track].clear(target_step),
        }
        changed = true;
    }

    let source_resolution = state.pattern.swing_resolution_plocks[track].get(source_step);
    let target_resolution = state.pattern.swing_resolution_plocks[track].get(target_step);
    if source_resolution != target_resolution {
        match source_resolution {
            Some(value) => state.pattern.swing_resolution_plocks[track].set(target_step, value),
            None => state.pattern.swing_resolution_plocks[track].clear(target_step),
        }
        changed = true;
    }
    changed
}

pub(super) fn clear_live_slot_variant_locks(slot: &EffectSlotState, step: usize) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        if slot.plocks.get(step, param_idx).is_some() {
            slot.plocks.clear_param(step, param_idx);
            changed = true;
        }
    }
    for tensor_idx in 0..slot.tensor_params.num_params() {
        if slot.tensor_params.plock_values(step, tensor_idx).is_some() {
            slot.tensor_params.clear_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

pub(super) fn copy_live_slot_variant_locks(
    slot: &EffectSlotState,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        let source = slot.plocks.get(source_step, param_idx);
        let target = slot.plocks.get(target_step, param_idx);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => slot.set_plock(target_step, param_idx, value),
            None => slot.plocks.clear_param(target_step, param_idx),
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.num_params() {
        let source = slot.tensor_params.plock_values(source_step, tensor_idx);
        let target = slot.tensor_params.plock_values(target_step, tensor_idx);
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.tensor_params
                    .set_plock(target_step, tensor_idx, &values);
            }
            None => {
                slot.tensor_params.clear_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}

pub(super) fn clear_rack_slot_variant_locks(slot: &mut RackSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        if slot.param_plocks.get(step, param).is_some() {
            slot.param_plocks.clear(step, param);
            changed = true;
        }
    }
    changed |= clear_snapshot_slot_variant_locks(&mut slot.instrument_slot, step);
    changed
}

pub(super) fn copy_rack_slot_variant_locks(
    slot: &mut RackSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        let source = slot.param_plocks.get(source_step, param);
        let target = slot.param_plocks.get(target_step, param);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.param_plocks.set(target_step, param, value);
            }
            None => {
                slot.param_plocks.clear(target_step, param);
            }
        }
        changed = true;
    }
    changed |=
        copy_snapshot_slot_variant_locks(&mut slot.instrument_slot, source_step, target_step);
    changed
}

pub(super) fn clear_snapshot_slot_variant_locks(slot: &mut EffectSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    let params_to_clear = slot
        .plocks
        .get(step)
        .map(|row| {
            (0..num_params.min(row.len()))
                .filter(|param_idx| row[*param_idx].is_some())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for param_idx in params_to_clear {
        slot.clear_plock(step, param_idx);
        changed = true;
    }
    for tensor_idx in 0..slot.tensor_params.len() {
        if slot.tensor_plock_values(step, tensor_idx).is_some() {
            slot.clear_tensor_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

pub(super) fn copy_snapshot_slot_variant_locks(
    slot: &mut EffectSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    for param_idx in 0..num_params {
        let source = slot
            .plocks
            .get(source_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        let target = slot
            .plocks
            .get(target_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.set_plock(target_step, param_idx, value);
            }
            None => {
                slot.clear_plock(target_step, param_idx);
            }
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.len() {
        let source = slot
            .tensor_plock_values(source_step, tensor_idx)
            .map(|values| values.to_vec());
        let target = slot
            .tensor_plock_values(target_step, tensor_idx)
            .map(|values| values.to_vec());
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.set_tensor_plock(target_step, tensor_idx, values);
            }
            None => {
                slot.clear_tensor_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}
