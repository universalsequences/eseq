use super::super::*;

impl SequencerState {
    pub fn scratch_source(&self) -> String {
        self.scratch_source.lock().unwrap().clone()
    }
    pub fn scratch_source_version(&self) -> u64 {
        self.scratch_source_version.load(Ordering::Acquire)
    }
    pub fn set_scratch_source(&self, source: impl Into<String>) {
        *self.scratch_source.lock().unwrap() = source.into();
        self.scratch_source_version.fetch_add(1, Ordering::AcqRel);
    }
    /// Publish (upsert by id) a UI-authored generator definition for the scheduler.
    pub fn publish_sequencer(&self, sequencer: PublishedSequencer) {
        {
            let mut list = self.published_sequencers.lock().unwrap();
            if let Some(existing) = list.iter_mut().find(|s| s.id == sequencer.id) {
                *existing = sequencer;
            } else {
                list.push(sequencer);
            }
        }
        self.published_sequencers_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn unpublish_sequencer_by_name(&self, name: &str) -> bool {
        let removed = {
            let mut list = self.published_sequencers.lock().unwrap();
            let before = list.len();
            list.retain(|sequencer| sequencer.name != name);
            list.len() != before
        };
        if removed {
            self.published_sequencers_version
                .fetch_add(1, Ordering::AcqRel);
        }
        removed
    }
    pub fn published_sequencers(&self) -> Vec<PublishedSequencer> {
        self.published_sequencers.lock().unwrap().clone()
    }
    pub fn published_sequencers_version(&self) -> u64 {
        self.published_sequencers_version.load(Ordering::Acquire)
    }
    /// Publish the complete UI-authored process/channel authoring snapshot.
    pub fn publish_process_authoring(
        &self,
        snapshot: crate::process::PublishedProcessAuthoringSnapshot,
    ) {
        *self.published_process_authoring.lock().unwrap() = snapshot;
        self.published_process_authoring_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn published_process_authoring(&self) -> crate::process::PublishedProcessAuthoringSnapshot {
        self.published_process_authoring.lock().unwrap().clone()
    }
    pub fn published_process_authoring_version(&self) -> u64 {
        self.published_process_authoring_version
            .load(Ordering::Acquire)
    }
    pub fn track_process_chain(&self, track: usize) -> Option<crate::process::TrackProcessChain> {
        if track >= self.active_track_count() {
            return None;
        }
        self.pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
    }
    pub fn set_track_process_chain(
        &self,
        track: usize,
        chain: crate::process::TrackProcessChain,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let mut chains = self.pattern.process_chains.lock().unwrap();
        let Some(slot) = chains.get_mut(track) else {
            return false;
        };
        *slot = chain;
        drop(chains);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    fn publish_process_chain_edit(&self) {
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }
    /// A track's effective chain as the scheduler sees it: project-layer
    /// slots ahead of the track's own slots. UI surfaces read this composed
    /// view so project slots appear (badged) in every track's process column.
    pub fn composed_track_process_chain(
        &self,
        track: usize,
    ) -> Option<crate::process::TrackProcessChain> {
        let track_chain = self.track_process_chain(track)?;
        let mut project_chain = self.project_process_chain();
        if let Some(overrides) = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
        {
            crate::process::apply_project_lane_overrides(&mut project_chain, overrides);
        }
        Some(crate::process::compose_effective_process_chain(
            &project_chain,
            &track_chain,
        ))
    }
    /// The project-level default process chain for the current scene. Every
    /// track — present and future — runs these slots ahead of its own chain.
    pub fn project_process_chain(&self) -> crate::process::TrackProcessChain {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_project_process_chain()
    }
    /// Whole-layer replace of the project process chain (`(processes :project ...)`).
    pub fn set_project_process_chain(&self, chain: crate::process::TrackProcessChain) -> bool {
        let identities = chain
            .slots
            .iter()
            .map(crate::process::project_slot_identity_id)
            .collect::<std::collections::BTreeSet<_>>();
        let updated = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|current| {
                    *current = chain;
                    Ok(())
                })
                .is_ok()
        };
        if updated {
            for overrides in self
                .pattern
                .project_process_lane_overrides
                .lock()
                .unwrap()
                .iter_mut()
            {
                overrides.retain(|identity, _| identities.contains(identity));
            }
            self.publish_process_chain_edit();
        }
        updated
    }
    fn edit_project_process_chain_slot<R>(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        edit: impl FnOnce(&mut crate::process::TrackProcessSlot) -> R,
    ) -> Option<R> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes
            .edit_current_project_process_chain(|chain| {
                Ok(chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == instance_id)
                    .map(edit))
            })
            .ok()
            .flatten()
    }
    /// Enable or bypass one pattern-scoped process-chain slot.
    ///
    /// Returns `false` only when the track or instance does not exist. A
    /// repeated write of the current value is a successful no-op.
    pub fn set_track_process_slot_enabled(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        enabled: bool,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    let changed = slot.enabled != enabled;
                    slot.enabled = enabled;
                    Some(changed)
                }
                None => None,
            }
        };
        // Project-layer slots are editable from any track's panel; the toggle
        // lands on the one shared object.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.enabled != enabled;
                slot.enabled = enabled;
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Move a slot before another instance, or to the end when `before` is
    /// `None`. Instance ids make this stable across reactive UI refreshes and
    /// avoid index-shift ambiguity while dragging downward.
    pub fn move_track_process_slot_before(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        before: Option<crate::process::ProcessInstanceId>,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        fn move_slot_within_chain(
            chain: &mut crate::process::TrackProcessChain,
            instance_id: crate::process::ProcessInstanceId,
            before: Option<crate::process::ProcessInstanceId>,
        ) -> Option<bool> {
            let source_index = chain
                .slots
                .iter()
                .position(|slot| slot.instance_id == instance_id)?;
            if before == Some(instance_id) {
                return Some(false);
            }
            if before
                .is_some_and(|target| !chain.slots.iter().any(|slot| slot.instance_id == target))
            {
                return None;
            }
            let previous_order = chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>();
            let slot = chain.slots.remove(source_index);
            let target_index = before
                .and_then(|target| {
                    chain
                        .slots
                        .iter()
                        .position(|slot| slot.instance_id == target)
                })
                .unwrap_or(chain.slots.len());
            chain.slots.insert(target_index, slot);
            Some(
                previous_order
                    != chain
                        .slots
                        .iter()
                        .map(|slot| slot.instance_id)
                        .collect::<Vec<_>>(),
            )
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            move_slot_within_chain(chain, instance_id, before)
        };
        // Reordering a project slot moves it within the project layer only.
        let changed = changed.or_else(|| {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    Ok(move_slot_within_chain(chain, instance_id, before))
                })
                .ok()
                .flatten()
        });
        let Some(changed) = changed else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Detach one slot from one track in the current pattern.
    pub fn remove_track_process_slot(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let removed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            let previous_len = chain.slots.len();
            chain.slots.retain(|slot| slot.instance_id != instance_id);
            chain.slots.len() != previous_len
        };
        // A project slot has no per-track detach: removing it from any track's
        // panel removes the shared slot from the project layer.
        let mut removed_identity = None;
        let removed = removed || {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    removed_identity = chain
                        .slots
                        .iter()
                        .find(|slot| slot.instance_id == instance_id)
                        .map(crate::process::project_slot_identity_id);
                    let previous_len = chain.slots.len();
                    chain.slots.retain(|slot| slot.instance_id != instance_id);
                    Ok(chain.slots.len() != previous_len)
                })
                .unwrap_or(false)
        };
        if removed {
            if let Some(identity) = removed_identity {
                for overrides in self
                    .pattern
                    .project_process_lane_overrides
                    .lock()
                    .unwrap()
                    .iter_mut()
                {
                    overrides.remove(&identity);
                }
            }
            self.publish_process_chain_edit();
        }
        removed
    }
    pub fn set_process_lane_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: impl Into<String>,
        step: usize,
        value: f32,
    ) -> bool {
        if track >= self.active_track_count() || step >= MAX_STEPS {
            return false;
        }
        let inlet_name = inlet_name.into();
        let write_lane_step = |slot: &mut crate::process::TrackProcessSlot| {
            let lane = slot.lanes.entry(inlet_name.clone()).or_default();
            if lane.values.len() <= step {
                lane.values.resize(step + 1, 0.0);
            }
            lane.values[step] = value;
        };
        let updated = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    write_lane_step(slot);
                    true
                }
                None => false,
            }
        };
        if !updated {
            let project_slot = self
                .project_process_chain()
                .slots
                .into_iter()
                .find(|slot| slot.instance_id == instance_id);
            let Some(project_slot) = project_slot else {
                return false;
            };
            let identity = crate::process::project_slot_identity_id(&project_slot);
            let mut overrides = self.pattern.project_process_lane_overrides.lock().unwrap();
            let Some(track_overrides) = overrides.get_mut(track) else {
                return false;
            };
            let lane = track_overrides
                .entry(identity)
                .or_default()
                .entry(inlet_name.clone())
                .or_insert_with(|| {
                    project_slot
                        .lanes
                        .get(&inlet_name)
                        .cloned()
                        .unwrap_or_default()
                });
            if lane.values.len() <= step {
                lane.values.resize(step + 1, 0.0);
            }
            lane.values[step] = value;
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    pub fn clear_project_process_lane_override(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> bool {
        let Some(slot) = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id)
        else {
            return false;
        };
        let identity = crate::process::project_slot_identity_id(&slot);
        let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
        let Some(track_overrides) = all.get_mut(track) else {
            return false;
        };
        let removed = track_overrides
            .get_mut(&identity)
            .is_some_and(|lanes| lanes.remove(inlet_name).is_some());
        if track_overrides
            .get(&identity)
            .is_some_and(|lanes| lanes.is_empty())
        {
            track_overrides.remove(&identity);
        }
        drop(all);
        if removed {
            self.publish_process_chain_edit();
        }
        removed
    }
    pub fn has_project_process_lane_override(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> bool {
        let Some(slot) = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id)
        else {
            return false;
        };
        self.pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
            .and_then(|overrides| overrides.get(&crate::process::project_slot_identity_id(&slot)))
            .is_some_and(|lanes| lanes.contains_key(inlet_name))
    }
    /// Replace a scalar inlet on every current-pattern chain slot owned by
    /// `instance_id`. This is the durable counterpart to authoring-handle knob
    /// edits like `(climb :limit 6)`: it updates pattern-scoped attachment
    /// state without touching step data or p-lock storage.
    pub fn set_process_inlet_value(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.inlets.insert(inlet_name.to_string(), value.clone());
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    /// Replace a scalar inlet on one track attachment. UI slot editors use
    /// this track-local form; authored process handles intentionally retain
    /// the all-attachments behavior of `set_process_inlet_value` above.
    pub fn set_track_process_inlet_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| {
                    let changed = slot.inlets.get(inlet_name) != Some(&value);
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    changed
                })
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.inlets.get(inlet_name) != Some(&value);
                slot.inlets.insert(inlet_name.to_string(), value.clone());
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    pub fn set_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let apply = |slot: &mut crate::process::TrackProcessSlot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        };
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(apply)
        };
        let Some(changed) =
            changed.or_else(|| self.edit_project_process_chain_slot(instance_id, apply))
        else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Bind a process target port on every current-pattern chain slot owned by
    /// `instance_id`. This mirrors `set_process_lane_values`: authored Lisp and
    /// UI interactions both update the pattern-owned slots currently attached to
    /// tracks without touching step/plock storage.
    pub fn set_process_port_binding_for_instance(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> usize {
        let mut updated = 0;
        let mut changed = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    updated += 1;
                    let current = slot.bindings.get(port_name);
                    if !matches!(current, Some(Some(existing)) if existing == &target) {
                        slot.bindings
                            .insert(port_name.to_string(), Some(target.clone()));
                        changed = true;
                    }
                }
            }
        }
        if let Some(project_changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        }) {
            updated += 1;
            changed |= project_changed;
        }
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn clear_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| slot.bindings.remove(port_name).is_some())
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                slot.bindings.remove(port_name).is_some()
            })
        }) else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Replace a lane wholesale on every current-pattern chain slot owned by
    /// `instance_id` (a handle can be attached to several tracks). Returns the
    /// number of slots updated; publishes only when at least one matched.
    pub fn set_process_lane_values(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        values: Vec<f32>,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.lanes.insert(
                        inlet_name.to_string(),
                        crate::process::ProcessLane {
                            values: values.clone(),
                        },
                    );
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.lanes.insert(
                    inlet_name.to_string(),
                    crate::process::ProcessLane {
                        values: values.clone(),
                    },
                );
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn process_instance_attachment_count(
        &self,
        instance_id: crate::process::ProcessInstanceId,
    ) -> usize {
        let track_attachments: usize = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .iter()
            .map(|chain| {
                chain
                    .slots
                    .iter()
                    .filter(|slot| slot.instance_id == instance_id)
                    .count()
            })
            .sum();
        let project_attachments = self
            .project_process_chain()
            .slots
            .iter()
            .filter(|slot| slot.instance_id == instance_id)
            .count();
        track_attachments + project_attachments
    }
    pub fn process_inlet_value(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> Option<crate::process::ProcessLiteral> {
        self.pattern
            .process_chains
            .lock()
            .unwrap()
            .iter()
            .flat_map(|chain| chain.slots.iter())
            .find(|slot| slot.instance_id == instance_id)
            .and_then(|slot| slot.inlets.get(inlet_name))
            .cloned()
    }
    pub fn scratch_runtime_descriptors(
        &self,
    ) -> (Vec<Vec<EffectDescriptor>>, Vec<EffectDescriptor>) {
        (
            self.scratch_effect_descriptors.lock().unwrap().clone(),
            self.scratch_instrument_descriptors.lock().unwrap().clone(),
        )
    }
    pub fn set_scratch_runtime_descriptors(
        &self,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
    ) {
        {
            *self.scratch_effect_descriptors.lock().unwrap() = effect_descriptors;
        }
        {
            *self.scratch_instrument_descriptors.lock().unwrap() = instrument_descriptors;
        }
        self.publish_scheduler_snapshot();
    }
    pub fn process_trace_enabled(&self) -> bool {
        self.process_trace_enabled.load(Ordering::Relaxed)
    }
    pub fn set_process_trace_enabled(&self, enabled: bool) {
        let previous = self.process_trace_enabled.swap(enabled, Ordering::Relaxed);
        if previous != enabled {
            self.publish_scheduler_snapshot();
        }
    }
    pub fn request_accumulator_reset(&self, track: usize) {
        if track < MAX_TRACKS {
            self.pending_accumulator_reset_tracks[track].store(true, Ordering::Release);
        }
    }
    pub fn request_all_accumulator_resets(&self) {
        self.pending_accumulator_reset_all
            .store(true, Ordering::Release);
    }
    pub fn take_accumulator_reset_requests(&self) -> (bool, [bool; MAX_TRACKS]) {
        let all = self
            .pending_accumulator_reset_all
            .swap(false, Ordering::AcqRel);
        let mut tracks = [false; MAX_TRACKS];
        for (idx, flag) in tracks.iter_mut().enumerate() {
            *flag = self.pending_accumulator_reset_tracks[idx].swap(false, Ordering::AcqRel);
        }
        (all, tracks)
    }
}
