use super::*;

#[derive(Clone, Debug)]
pub struct BusPatternSnapshot {
    pub id: BusId,
    pub gate_sequence: BusGateSequence,
    pub effect_plocks: Vec<Vec<Vec<Option<f32>>>>,
    /// Per-scene base (non-plocked) effect parameter values, indexed
    /// `[slot][param]`. Recalled on scene switch so a bus effect knob can
    /// hold different values per scene. Empty for legacy snapshots.
    pub effect_defaults: Vec<Vec<f32>>,
}

impl BusPatternSnapshot {
    pub(super) fn remap_effect_slots(&mut self, new_to_old: &[Option<usize>]) {
        let old_plocks = std::mem::take(&mut self.effect_plocks);
        let old_defaults = std::mem::take(&mut self.effect_defaults);
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;

        self.effect_plocks = new_to_old
            .iter()
            .copied()
            .take(slot_count)
            .map(|source| {
                source
                    .and_then(|slot| old_plocks.get(slot).cloned())
                    .unwrap_or_default()
            })
            .collect();
        self.effect_defaults = new_to_old
            .iter()
            .copied()
            .take(slot_count)
            .map(|source| {
                source
                    .and_then(|slot| old_defaults.get(slot).cloned())
                    .unwrap_or_default()
            })
            .collect();

        self.effect_plocks.resize_with(slot_count, Vec::new);
        self.effect_defaults.resize_with(slot_count, Vec::new);
    }

    pub(super) fn replace_effect_slot(
        &mut self,
        slot_idx: usize,
        defaults: Vec<f32>,
        plocks: Vec<Vec<Option<f32>>>,
    ) {
        if slot_idx >= crate::lisp_host::MAX_CUSTOM_FX {
            return;
        }
        self.effect_plocks.resize_with(slot_idx + 1, Vec::new);
        self.effect_defaults.resize_with(slot_idx + 1, Vec::new);
        self.effect_plocks[slot_idx] = plocks;
        self.effect_defaults[slot_idx] = defaults;
    }
}

#[derive(Clone, Debug)]
pub struct BusGateSequence {
    pub steps: [bool; MAX_STEPS],
    pub velocities: [f32; MAX_STEPS],
    pub durations: [f32; MAX_STEPS],
    pub syncs: [f32; MAX_STEPS],
    pub num_steps: usize,
    pub timebase: Timebase,
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub timebase_plocks: [Option<Timebase>; MAX_STEPS],
    pub swing_plocks: [Option<f32>; MAX_STEPS],
    pub swing_resolution_plocks: [Option<SwingResolution>; MAX_STEPS],
}

impl Default for BusGateSequence {
    fn default() -> Self {
        Self {
            steps: [true; MAX_STEPS],
            velocities: [1.0; MAX_STEPS],
            durations: [1.0; MAX_STEPS],
            syncs: [0.0; MAX_STEPS],
            num_steps: 16,
            timebase: Timebase::Sixteenth,
            swing: 50.0,
            swing_resolution: SwingResolution::Sixteenth,
            timebase_plocks: [None; MAX_STEPS],
            swing_plocks: [None; MAX_STEPS],
            swing_resolution_plocks: [None; MAX_STEPS],
        }
    }
}

impl BusGateSequence {
    pub fn toggle_step(&mut self, step: usize) -> Option<bool> {
        let value = self.steps.get_mut(step)?;
        *value = !*value;
        Some(*value)
    }

    pub fn set_step_velocity(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.velocities.get_mut(step)?;
        *slot = value.clamp(0.0, 1.0);
        Some(*slot)
    }

    pub fn set_step_duration(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.durations.get_mut(step)?;
        *slot = value.clamp(0.1, 2.0);
        Some(*slot)
    }

    pub fn set_step_sync(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.syncs.get_mut(step)?;
        *slot = value
            .round()
            .clamp(0.0, (crate::sequencer::SYNC_COUNT - 1) as f32);
        Some(*slot)
    }

    pub fn set_num_steps(&mut self, value: usize) {
        self.num_steps = value.clamp(1, MAX_STEPS);
    }

    pub fn has_step_plock(&self, step: usize) -> bool {
        step < MAX_STEPS
            && (self.timebase_plocks[step].is_some()
                || self.swing_plocks[step].is_some()
                || self.swing_resolution_plocks[step].is_some())
    }
}
