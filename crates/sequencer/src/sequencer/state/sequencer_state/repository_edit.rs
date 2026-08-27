use super::super::*;

impl SequencerState {
    fn ensure_scene_bus_patterns_len_locked(
        scenes: &mut ProjectScenes,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        for scene in scenes.scenes.iter_mut().take(len) {
            if scene.bus_patterns.is_empty() {
                scene.bus_patterns = default_snapshot.to_vec();
            }
        }
    }

    pub fn save_current_bus_pattern_snapshot(&self, snapshot: Vec<BusPatternSnapshot>) {
        let current_scene = self.current_scene_index();
        let target_len = self.scene_count().max(current_scene + 1);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, &snapshot);
        scenes.scenes[current_scene].bus_patterns = snapshot;
    }

    pub fn bus_pattern_snapshot_or_default(
        &self,
        scene_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, scene_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(scene_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn ensure_bus_pattern_repository_len(
        &self,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, len, default_snapshot);
    }

    pub fn clone_bus_pattern_snapshot(
        &self,
        source_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(
            &mut scenes,
            source_idx.max(new_idx) + 1,
            default_snapshot,
        );
        let source = scenes
            .scenes
            .get(source_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec());
        if let Some(scene) = scenes.scenes.get_mut(new_idx) {
            scene.bus_patterns = source.clone();
        }
        source
    }

    pub fn delete_bus_pattern_snapshot(
        &self,
        _deleted_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, new_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(new_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn export_bus_pattern_repository(
        &self,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<Vec<BusPatternSnapshot>> {
        let target_len = self.scene_count();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect()
    }

    pub fn replace_bus_pattern_repository(
        &self,
        snapshots: Vec<Vec<BusPatternSnapshot>>,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(snapshots.len());
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene, snapshot) in scenes.scenes.iter_mut().zip(snapshots) {
            scene.bus_patterns = snapshot;
        }
    }

    pub fn insert_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;
        if slot_idx >= slot_count {
            return;
        }
        let mut new_to_old = (0..slot_count).map(Some).collect::<Vec<_>>();
        new_to_old.insert(slot_idx, None);
        new_to_old.truncate(slot_count);
        self.remap_bus_effect_slots_in_other_scene_patterns(bus_idx, &new_to_old, default_snapshot);
    }

    pub fn move_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;
        if source_slot >= slot_count || target_slot >= slot_count || source_slot == target_slot {
            return;
        }
        let mut new_to_old = (0..slot_count).map(Some).collect::<Vec<_>>();
        let source = new_to_old.remove(source_slot);
        new_to_old.insert(target_slot, source);
        self.remap_bus_effect_slots_in_other_scene_patterns(bus_idx, &new_to_old, default_snapshot);
    }

    pub fn remap_bus_effect_slots_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        new_to_old: &[Option<usize>],
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.remap_effect_slots(new_to_old);
            }
        }
    }

    pub fn replace_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        source_snapshot: &[BusPatternSnapshot],
    ) {
        let Some(source_bus) = source_snapshot.get(bus_idx) else {
            return;
        };
        let defaults = source_bus
            .effect_defaults
            .get(slot_idx)
            .cloned()
            .unwrap_or_default();
        let plocks = source_bus
            .effect_plocks
            .get(slot_idx)
            .cloned()
            .unwrap_or_default();
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, source_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.replace_effect_slot(slot_idx, defaults.clone(), plocks.clone());
            }
        }
    }

    pub fn copy_bus_effect_values_to_all_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        source_snapshot: &[BusPatternSnapshot],
    ) -> usize {
        let Some(source_defaults) = source_snapshot
            .get(bus_idx)
            .and_then(|bus| bus.effect_defaults.get(slot_idx))
            .cloned()
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(self.current_scene_index() + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, source_snapshot);
        let mut updated = 0;
        for scene in &mut scenes.scenes {
            let Some(bus) = scene.bus_patterns.get_mut(bus_idx) else {
                continue;
            };
            bus.effect_defaults.resize_with(slot_idx + 1, Vec::new);
            bus.effect_defaults[slot_idx] = source_defaults.clone();
            updated += 1;
        }
        updated
    }

    pub fn clear_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.replace_effect_slot(slot_idx, Vec::new(), Vec::new());
            }
        }
    }

    pub fn edit_pattern_repository<F, R>(&self, edit: F) -> R
    where
        F: FnOnce(&mut Vec<PatternSnapshot>, usize) -> R,
    {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut snapshots = scenes.snapshots();
        let result = edit(&mut snapshots, current_pattern);
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, current_pattern);
        rebuilt.copy_scene_bank_model_from(&scenes);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        result
    }

    pub fn edit_non_current_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, current_pattern| {
            for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
                if pattern_idx != current_pattern {
                    edit(snapshot);
                }
            }
        });
    }

    pub fn edit_all_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, _| {
            for snapshot in bank.iter_mut() {
                edit(snapshot);
            }
        });
    }

    pub fn current_scene_metadata(
        &self,
    ) -> (
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    ) {
        let current_pattern = self.current_pattern_index();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_metadata(current_pattern)
            .unwrap_or_default()
    }

    /// Live scene-slot overrides for every scene, indexed by scene position.
    ///
    /// Captured into each published snapshot so scheduler-side readers can
    /// resolve a chunk's scene without locking the bank on the scheduling
    /// thread, and without trusting a prebuilt snapshot's preflight copy.
    pub fn scene_slot_table(&self) -> Vec<std::sync::Arc<SceneSlotStore>> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .iter()
            .map(|scene| std::sync::Arc::new(scene.scene_slots.clone()))
            .collect()
    }

    pub fn current_scene_slots(&self) -> SceneSlotStore {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .get(self.current_pattern_index())
            .map(|scene| scene.scene_slots.clone())
            .unwrap_or_default()
    }

    pub fn resolve_current_scene_slot(
        &self,
        name: &str,
        declaration_default: &crate::process::ProcessLiteral,
    ) -> (crate::process::ProcessLiteral, u64, bool) {
        let slots = self.current_scene_slots();
        let resolved = slots.resolve(name, declaration_default);
        (resolved.value.clone(), resolved.epoch, resolved.overridden)
    }

    /// Persist one override into the current pattern and publish it to the
    /// scheduler. The stable scene identity and previous override are captured
    /// under the same lock as the write so an authoring host command can replay
    /// the exact pattern even if selection changes before the command drains.
    pub(crate) fn write_current_scene_slot_identified(
        &self,
        name: impl Into<String>,
        value: crate::process::ProcessLiteral,
    ) -> Result<(SceneId, Option<crate::process::ProcessLiteral>, u64), String> {
        let name = name.into();
        let (scene_id, previous, epoch) = {
            let mut scenes = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            let current = self.current_pattern_index();
            let scene = scenes
                .scenes
                .get_mut(current)
                .ok_or_else(|| "current pattern out of range".to_string())?;
            let previous = scene.scene_slots.get(&name).cloned();
            let epoch = scene.scene_slots.write_literal(name, value)?;
            (scene.id, previous, epoch)
        };
        self.publish_scheduler_snapshot();
        Ok((scene_id, previous, epoch))
    }

    pub fn write_current_scene_slot(
        &self,
        name: impl Into<String>,
        value: crate::process::ProcessLiteral,
    ) -> Result<u64, String> {
        self.write_current_scene_slot_identified(name, value)
            .map(|(_, _, epoch)| epoch)
    }

    /// Restore or replace a slot override in a stable pattern. This is the
    /// history replay seam; selection order is deliberately irrelevant.
    pub(crate) fn set_scene_slot_override(
        &self,
        scene_id: SceneId,
        name: impl Into<String>,
        value: Option<crate::process::ProcessLiteral>,
    ) -> Result<u64, String> {
        let name = name.into();
        let epoch = {
            let mut scenes = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            let scene_idx = scenes
                .scene_index(scene_id)
                .ok_or_else(|| "scene-slot pattern no longer exists".to_string())?;
            scenes.scenes[scene_idx]
                .scene_slots
                .set_override(name, value)?
        };
        self.publish_scheduler_snapshot();
        Ok(epoch)
    }

    pub fn current_mod_connections(&self) -> Vec<ModConnection> {
        self.current_scene_metadata().0
    }

    pub fn edit_current_mod_connections<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ModConnection>) -> Result<R, String>,
    {
        let current_pattern = self.current_pattern_index();
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            if bank.current_scene != current_pattern {
                bank.current_scene = current_pattern.min(bank.scene_count().saturating_sub(1));
            }
            bank.edit_current_mod_connections(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }
}
