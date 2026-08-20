use super::*;

#[derive(Clone, Debug)]
pub struct BusPatternSnapshot {
    pub id: BusId,
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
