use std::collections::BTreeMap;
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
    /// Drop one published sequencer by id. Returns whether one was removed.
    pub fn unpublish_sequencer_by_id(&self, id: u64) -> bool {
        let removed = {
            let mut list = self.published_sequencers.lock().unwrap();
            let before = list.len();
            list.retain(|sequencer| sequencer.id != id);
            list.len() != before
        };
        if removed {
            self.published_sequencers_version.fetch_add(1, Ordering::AcqRel);
        }
        removed
    }
    /// Mirror of the app's drum-rack member lists, read into every scheduler
    /// snapshot so rack-owned graph sequencers can resolve member routes.
    pub fn set_rack_memberships(&self, memberships: Vec<crate::graph::RackMembership>) {
        *self.rack_memberships.lock().unwrap() = memberships;
    }
    pub fn rack_memberships(&self) -> Vec<crate::graph::RackMembership> {
        self.rack_memberships.lock().unwrap().clone()
    }
    /// The graph overrides of every scene, by scene position.
    pub fn all_scene_graph_overrides(&self) -> Vec<Vec<ProjectGraphOverrides>> {
        let bank = self.pattern.scenes.lock().unwrap();
        bank.scenes.iter().map(|scene| scene.graph_overrides.clone()).collect()
    }
    /// Edit the graph overrides of EVERY scene (attach/detach/member-leave
    /// rewrites are structural and must hold across the whole scene bank).
    /// Republishes the scheduler snapshot when anything changed.
    pub fn edit_all_scene_graph_overrides(
        &self,
        mut edit: impl FnMut(&mut Vec<ProjectGraphOverrides>) -> bool,
    ) -> bool {
        let changed = {
            let mut bank = self.pattern.scenes.lock().unwrap();
            let mut changed = false;
            for scene in &mut bank.scenes {
                changed |= edit(&mut scene.graph_overrides);
            }
            changed
        };
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
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
    /// Publish (upsert) one `defscene` declaration so runtimes that never
    /// evaluated the declaring source — the scheduler's scratch runtime
    /// compiling a shipped tick — can resolve the slot's default by name.
    pub fn publish_scene_slot_declaration(
        &self,
        name: String,
        default: crate::process::ProcessLiteral,
    ) {
        self.published_scene_slot_declarations
            .lock()
            .unwrap()
            .insert(name, default);
    }

    /// The published default for a `defscene` declaration, if any authoring
    /// VM has evaluated it. The fallback read for scene-slot resolution in
    /// runtimes whose local declaration table misses the name.
    pub fn published_scene_slot_declaration(
        &self,
        name: &str,
    ) -> Option<crate::process::ProcessLiteral> {
        self.published_scene_slot_declarations
            .lock()
            .unwrap()
            .get(name)
            .cloned()
    }

    /// Scheduler side: report a generator tick failure. One pending entry per
    /// generator id and a hard cap keep this bounded; repeat failures of a
    /// parked generator never re-report.
    pub fn report_generator_tick_error(&self, id: u64, name: String, error: String) {
        const MAX_PENDING_TICK_ERRORS: usize = 32;
        let mut errors = self.generator_tick_errors.lock().unwrap();
        if errors.len() >= MAX_PENDING_TICK_ERRORS
            || errors.iter().any(|notice| notice.id == id)
        {
            return;
        }
        errors.push(crate::sequencer::GeneratorTickErrorNotice { id, name, error });
    }

    /// UI side: drain pending generator tick errors for display.
    pub fn drain_generator_tick_errors(&self) -> Vec<crate::sequencer::GeneratorTickErrorNotice> {
        std::mem::take(&mut *self.generator_tick_errors.lock().unwrap())
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
    /// Drop every cross-thread channel that carries project-authored jaki /
    /// process state (bead eseq-jo7.21). A project switch replaces the
    /// authoring source wholesale, so the outgoing project's generators,
    /// process graph, scene-slot declarations and channel values must not
    /// survive into the incoming one. Both version counters are bumped so the
    /// scheduler rebuilds its scratch runtime from the cleared state on its
    /// next loop.
    pub fn clear_project_authored_processes(&self) {
        self.published_sequencers.lock().unwrap().clear();
        self.published_sequencers_version
            .fetch_add(1, Ordering::AcqRel);
        self.published_scene_slot_declarations.lock().unwrap().clear();
        *self.published_process_authoring.lock().unwrap() =
            crate::process::PublishedProcessAuthoringSnapshot::default();
        self.published_process_authoring_version
            .fetch_add(1, Ordering::AcqRel);
        self.pending_process_channel_writes.lock().unwrap().clear();
        self.process_channel_values.lock().unwrap().clear();
        self.process_channel_values_version
            .fetch_add(1, Ordering::AcqRel);
        self.generator_tick_errors.lock().unwrap().clear();
    }
    /// Queue control-thread channel writes for the scheduler to apply at the
    /// top of the next lookahead chunk
    /// (docs/jaki-live-channel-widgets-spec.md 7).
    ///
    /// Nothing drains while the transport is stopped, so a long drag would
    /// otherwise grow the queue without bound. Past the cap the oldest writes
    /// are dropped: the channel still ends at the value the author left it on,
    /// which is what section 8 promises for a stopped transport.
    pub fn queue_process_channel_writes(
        &self,
        writes: impl IntoIterator<Item = (String, crate::process::ProcessLiteral)>,
    ) {
        const MAX_PENDING_CHANNEL_WRITES: usize = 512;
        let mut pending = self.pending_process_channel_writes.lock().unwrap();
        pending.extend(writes);
        let overflow = pending.len().saturating_sub(MAX_PENDING_CHANNEL_WRITES);
        if overflow > 0 {
            pending.drain(..overflow);
        }
    }
    /// Take the queued control-thread channel writes, oldest first.
    pub fn take_process_channel_writes(&self) -> Vec<(String, crate::process::ProcessLiteral)> {
        std::mem::take(&mut *self.pending_process_channel_writes.lock().unwrap())
    }
    /// Publish the scheduler's latest held channel values for read-only UI
    /// polling (docs/jaki-live-channel-widgets-spec.md 8.1).
    pub fn publish_process_channel_values(
        &self,
        values: HashMap<String, crate::process::ProcessLiteral>,
    ) {
        let mut published = self.process_channel_values.lock().unwrap();
        if *published != values {
            *published = values;
            self.process_channel_values_version
                .fetch_add(1, Ordering::Release);
        }
    }
    /// Publish the scheduler's step-process state histories for the lane
    /// strip scope. Called once per lookahead chunk that fired a process.
    pub fn publish_process_scope_values(
        &self,
        values: HashMap<u64, HashMap<String, Vec<f32>>>,
    ) {
        *self.process_scope_values.lock().unwrap() = values;
        self.process_scope_values_version
            .fetch_add(1, Ordering::Release);
    }
    /// Publish what the scheduler's process writes resolved to on one
    /// track's instrument params. Merges per `(track, param)` so tracks that
    /// did not fire this trigger keep their last value; bumps the version
    /// only when a value actually changed.
    pub fn publish_process_effective_params(
        &self,
        track: usize,
        values: &[crate::process::ProcessEffectiveParam],
    ) {
        let mut published = self.process_effective_params.lock().unwrap();
        let mut changed = false;
        for value in values {
            let key = (track, value.param_idx);
            if published.get(&key) != Some(value) {
                published.insert(key, *value);
                changed = true;
            }
        }
        if changed {
            self.process_effective_params_version
                .fetch_add(1, Ordering::Release);
        }
    }
    pub fn publish_process_effective_sends(
        &self,
        track: usize,
        values: &[crate::process::ProcessEffectiveSend],
    ) {
        let mut published = self.process_effective_sends.lock().unwrap();
        let mut changed = false;
        for value in values {
            let key = (track, value.bus);
            if published.get(&key) != Some(value) {
                published.insert(key, *value);
                changed = true;
            }
        }
        if changed {
            self.process_effective_params_version
                .fetch_add(1, Ordering::Release);
        }
    }
    pub fn process_effective_sends(
        &self,
    ) -> HashMap<(usize, u64), crate::process::ProcessEffectiveSend> {
        self.process_effective_sends.lock().unwrap().clone()
    }
    pub fn process_effective_params_version(&self) -> u64 {
        self.process_effective_params_version.load(Ordering::Acquire)
    }
    pub fn process_effective_params(
        &self,
    ) -> HashMap<(usize, usize), crate::process::ProcessEffectiveParam> {
        self.process_effective_params.lock().unwrap().clone()
    }
    pub fn process_scope_values_version(&self) -> u64 {
        self.process_scope_values_version.load(Ordering::Acquire)
    }
    pub fn process_scope_values(&self) -> HashMap<u64, HashMap<String, Vec<f32>>> {
        self.process_scope_values.lock().unwrap().clone()
    }
    pub fn process_channel_values_version(&self) -> u64 {
        self.process_channel_values_version.load(Ordering::Acquire)
    }
    /// Return the scheduler-owned value of one channel, if it currently holds
    /// a value representable on the control thread.
    pub fn process_channel_value(
        &self,
        name: &str,
    ) -> Option<crate::process::ProcessLiteral> {
        self.process_channel_values.lock().unwrap().get(name).cloned()
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
    /// One track's scene-independent roster of user-added process slots
    /// (eseq-53y7).
    pub fn track_lane_roster(&self, track: usize) -> Option<crate::process::TrackLaneRoster> {
        self.pattern.track_lane_rosters.lock().unwrap().get(track).cloned()
    }

    /// Every track's roster, in track order. The history memento for a
    /// scene-structure edit carries this beside `capture_project_scenes`
    /// (eseq-53y7.6): roster structure lives outside pattern data, so undo
    /// has to restore both halves or the next reconcile re-applies the edit.
    pub fn capture_track_lane_rosters(&self) -> Vec<crate::process::TrackLaneRoster> {
        self.pattern.track_lane_rosters.lock().unwrap().clone()
    }

    /// Restore every track's roster from a history memento. A no-op — and in
    /// particular no reconcile pass — when the stored rosters already match,
    /// so scene-structure undo for edits that never touched a roster costs
    /// nothing. Returns whether anything moved.
    pub fn restore_track_lane_rosters(
        &self,
        rosters: &[crate::process::TrackLaneRoster],
    ) -> bool {
        {
            let stored = self.pattern.track_lane_rosters.lock().unwrap();
            let unchanged = stored.iter().enumerate().all(|(track, roster)| {
                rosters.get(track).map_or(roster.is_empty(), |target| target == roster)
            });
            if unchanged {
                return false;
            }
        }
        self.install_track_lane_rosters(rosters.to_vec());
        true
    }

    /// Replace every track's roster (project load) and reconcile the result
    /// into every stored pattern chain plus every live chain.
    pub fn install_track_lane_rosters(&self, rosters: Vec<crate::process::TrackLaneRoster>) {
        {
            let mut stored = self.pattern.track_lane_rosters.lock().unwrap();
            for (track, slot) in stored.iter_mut().enumerate() {
                *slot = rosters.get(track).cloned().unwrap_or_default();
            }
        }
        // Bind the length first: a guard temporary in the `for` header would
        // live for the whole loop and deadlock the reconcile below.
        let track_count = self.pattern.track_lane_rosters.lock().unwrap().len();
        for track in 0..track_count {
            self.reconcile_track_lane_roster_everywhere(track);
        }
    }

    /// Reconcile `track`'s roster into every pattern chain the track owns
    /// (stored Patch entities, take chunks included) and into the live
    /// chain. Idempotent: reconciliation only appends missing roster slots,
    /// drops roster slots that left, and never touches pattern-owned values.
    pub(crate) fn reconcile_track_lane_roster_everywhere(&self, track: usize) {
        let roster = match self.pattern.track_lane_rosters.lock().unwrap().get(track) {
            Some(roster) => roster.clone(),
            None => return,
        };
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if let Some(pool) = scenes.track_pools.get_mut(track) {
                // Visit each Patch entity once: take chunks share one.
                for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
                    crate::process::reconcile_track_lane_roster(
                        &mut patch.process_chain,
                        &roster,
                    );
                }
            }
        }
        if let Some(chain) = self.pattern.process_chains.lock().unwrap().get_mut(track) {
            crate::process::reconcile_track_lane_roster(chain, &roster);
        }
    }

    /// Rewrite `track`'s roster so its entries follow `order` (an instance-id
    /// sequence read back off a chain). Entries `order` does not mention keep
    /// their relative order at the end. Order is roster-owned structure, so a
    /// reorder in one scene's chain lands in every scene (eseq-53y7.5).
    pub(crate) fn reorder_track_lane_roster(
        &self,
        track: usize,
        order: &[crate::process::ProcessInstanceId],
    ) -> bool {
        let mut rosters = self.pattern.track_lane_rosters.lock().unwrap();
        let Some(roster) = rosters.get_mut(track) else {
            return false;
        };
        let rank = |id: crate::process::ProcessInstanceId| {
            order.iter().position(|listed| *listed == id).unwrap_or(usize::MAX)
        };
        let before = roster
            .iter()
            .map(|entry| entry.instance_id)
            .collect::<Vec<_>>();
        // Stable, so unlisted entries keep their relative order.
        roster.sort_by_key(|entry| rank(entry.instance_id));
        before != roster.iter().map(|entry| entry.instance_id).collect::<Vec<_>>()
    }

    /// Append one user-added process slot to `track`'s roster and to every
    /// one of its pattern chains, returning the new instance id.
    ///
    /// The instance name is minted unique within the track: the bare
    /// class-derived lane name when free, else `name 2`, `name 3`, … . The
    /// default project lanes own their bare names on every track, so a
    /// track's first added grab reads `grab 2`.
    pub fn add_track_roster_slot(
        &self,
        track: usize,
        class_name: &str,
    ) -> Option<crate::process::ProcessInstanceId> {
        self.add_track_roster_slot_with_id(track, class_name, None)
    }

    /// The instance id a new roster slot would take right now. The UI native
    /// hands the new id back to Lisp before its host command has run, so the
    /// id has to be decided ahead of the edit; `add_track_roster_slot_with_id`
    /// takes it back.
    pub fn next_track_roster_slot_id(&self) -> crate::process::ProcessInstanceId {
        let rosters = self.pattern.track_lane_rosters.lock().unwrap();
        crate::process::next_track_roster_instance_id(
            rosters
                .iter()
                .flat_map(|roster| roster.iter().map(|entry| entry.instance_id)),
        )
    }

    /// `add_track_roster_slot` with a caller-chosen instance id. A `requested`
    /// id outside the roster band, or one another slot took between the mint
    /// and this call, is ignored and a fresh id minted instead.
    pub fn add_track_roster_slot_with_id(
        &self,
        track: usize,
        class_name: &str,
        requested: Option<crate::process::ProcessInstanceId>,
    ) -> Option<crate::process::ProcessInstanceId> {
        if track >= self.active_track_count() {
            return None;
        }
        let instance_id = {
            let mut rosters = self.pattern.track_lane_rosters.lock().unwrap();
            // Ids are unique across every track: a roster slot's id is its
            // runtime identity (see `track_process_slot_runtime_id`).
            let free = |id: crate::process::ProcessInstanceId,
                        rosters: &[crate::process::TrackLaneRoster]| {
                crate::process::is_track_roster_instance_id(id)
                    && !rosters
                        .iter()
                        .any(|roster| roster.iter().any(|entry| entry.instance_id == id))
            };
            let instance_id = match requested {
                Some(id) if free(id, &rosters) => id,
                _ => crate::process::next_track_roster_instance_id(
                    rosters
                        .iter()
                        .flat_map(|roster| roster.iter().map(|entry| entry.instance_id)),
                ),
            };
            let roster = rosters.get_mut(track)?;
            let instance_name = crate::process::mint_track_roster_instance_name(
                class_name,
                &crate::process::taken_track_roster_instance_names(roster),
            );
            roster.push(crate::process::TrackLaneRosterSlot {
                instance_id,
                instance_name,
                class_name: class_name.to_string(),
            });
            instance_id
        };
        self.reconcile_track_lane_roster_everywhere(track);
        self.publish_process_chain_edit();
        Some(instance_id)
    }

    /// Drop one roster slot from `track` — and therefore from every scene's
    /// chain for that track, values included.
    pub fn remove_track_roster_slot(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        {
            let mut rosters = self.pattern.track_lane_rosters.lock().unwrap();
            let Some(roster) = rosters.get_mut(track) else {
                return false;
            };
            let before = roster.len();
            roster.retain(|entry| entry.instance_id != instance_id);
            if roster.len() == before {
                return false;
            }
        }
        self.reconcile_track_lane_roster_everywhere(track);
        self.publish_process_chain_edit();
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
        // A project slot toggled from one track's UI forks that track only
        // (docs/default-process-lanes-spec.md): bypassing `prob` on track 4
        // must not silence it everywhere. `set_process_slot_enabled_all` is
        // the every-track write.
        let Some(changed) = changed.or_else(|| {
            let shared = self
                .project_process_chain()
                .slots
                .into_iter()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| slot.enabled)?;
            self.edit_project_slot_override(track, instance_id, |override_| {
                let effective = override_.enabled.unwrap_or(shared);
                // Agreeing with the shared slot needs no fork at all, so the
                // override collapses instead of pinning a redundant value.
                override_.enabled = (enabled != shared).then_some(enabled);
                effective != enabled
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Enable or bypass a slot on every track. Track attachments of the
    /// instance flip in place; a project slot flips the shared object and
    /// drops each track's `enabled` fork so "all tracks" means exactly that.
    pub fn set_process_slot_enabled_all(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        enabled: bool,
    ) -> bool {
        let mut changed = false;
        let mut matched = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for slot in chains
                .iter_mut()
                .flat_map(|chain| chain.slots.iter_mut())
                .filter(|slot| slot.instance_id == instance_id)
            {
                matched = true;
                changed |= slot.enabled != enabled;
                slot.enabled = enabled;
            }
        }
        let shared = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id);
        if let Some(shared) = shared {
            matched = true;
            let identity = crate::process::project_slot_identity_id(&shared);
            let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
            for track_overrides in all.iter_mut() {
                let Some(override_) = track_overrides.get_mut(&identity) else {
                    continue;
                };
                if let Some(forked) = override_.enabled.take() {
                    changed |= forked != enabled;
                }
                if override_.is_empty() {
                    track_overrides.remove(&identity);
                }
            }
            drop(all);
            self.edit_project_process_chain_slot(instance_id, |slot| {
                changed |= slot.enabled != enabled;
                slot.enabled = enabled;
            });
        }
        if !matched {
            return false;
        }
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
        // A roster slot's order is scene-independent structure, so moving one
        // rewrites the roster and reconciles every pattern (eseq-53y7.5).
        let roster_slot = crate::process::is_track_roster_instance_id(instance_id);
        let mut roster_order: Option<Vec<crate::process::ProcessInstanceId>> = None;
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            let changed = move_slot_within_chain(chain, instance_id, before);
            if roster_slot && changed == Some(true) {
                roster_order = Some(
                    chain
                        .slots
                        .iter()
                        .filter(|slot| crate::process::is_track_roster_slot(slot))
                        .map(|slot| slot.instance_id)
                        .collect(),
                );
            }
            changed
        };
        if let Some(order) = roster_order {
            self.reorder_track_lane_roster(track, &order);
            self.reconcile_track_lane_roster_everywhere(track);
            self.publish_process_chain_edit();
            return true;
        }
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
        self.set_process_lane_steps(track, instance_id, inlet_name, &[step], value)
    }

    /// Apply one selection edit atomically and publish the affected track once.
    pub fn set_process_lane_steps(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: impl Into<String>,
        steps: &[usize],
        value: f32,
    ) -> bool {
        if track >= self.active_track_count() || steps.is_empty()
            || steps.iter().any(|step| *step >= MAX_STEPS) || !value.is_finite()
        {
            return false;
        }
        let last_step = *steps.iter().max().unwrap();
        let inlet_name = inlet_name.into();
        let write_lane = |lane: &mut crate::process::ProcessLane| {
            if lane.values.len() <= last_step {
                lane.values.resize(last_step + 1, 0.0);
            }
            for step in steps {
                lane.values[*step] = value;
            }
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
                    write_lane(slot.lanes.entry(inlet_name.clone()).or_default());
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
                .lanes
                .entry(inlet_name.clone())
                .or_insert_with(|| {
                    project_slot
                        .lanes
                        .get(&inlet_name)
                        .cloned()
                        .unwrap_or_default()
                });
            write_lane(lane);
        }
        // Content, not topology: publish so the next fire reads the new
        // value, but do not bump `pattern_epoch`. The epoch makes the
        // playing scheduler clear its queue, re-seek and reset every
        // accumulator; per-event during a slider drag that silenced the
        // transport for as long as the mouse moved.
        self.publish_scheduler_track(track);
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
            .is_some_and(|override_| override_.lanes.remove(inlet_name).is_some());
        if track_overrides
            .get(&identity)
            .is_some_and(|override_| override_.is_empty())
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
            .is_some_and(|override_| override_.lanes.contains_key(inlet_name))
    }
    /// Edit one track's override record for a project slot. Returns None when
    /// `instance_id` is not a project slot (or the track is out of range);
    /// prunes the record when the edit leaves it empty.
    fn edit_project_slot_override<R>(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        edit: impl FnOnce(&mut crate::process::ProjectSlotOverride) -> R,
    ) -> Option<R> {
        let slot = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id)?;
        let identity = crate::process::project_slot_identity_id(&slot);
        let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
        let track_overrides = all.get_mut(track)?;
        let result = edit(track_overrides.entry(identity).or_default());
        if track_overrides
            .get(&identity)
            .is_some_and(|override_| override_.is_empty())
        {
            track_overrides.remove(&identity);
        }
        Some(result)
    }
    /// Edit a port's fan-out list. Track slots edit in place; project slots
    /// fork this track's list (starting from the effective composed list)
    /// unless `all_tracks`, which edits the shared slot and drops every
    /// track's fork of that port. Returns false when nothing matched.
    pub fn edit_process_port_fanout(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        all_tracks: bool,
        edit: impl FnOnce(&mut Vec<crate::process::ProcessPortFanout>),
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let mut edit = Some(edit);
        let edited_track_slot = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| edit_fanout_list(&mut slot.fanout, port_name, edit.take().unwrap()))
        };
        let changed = match edited_track_slot {
            Some(changed) => changed,
            None => {
                if all_tracks {
                    let Some(changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
                        edit_fanout_list(&mut slot.fanout, port_name, edit.take().unwrap())
                    }) else {
                        return false;
                    };
                    let identity = self
                        .project_process_chain()
                        .slots
                        .into_iter()
                        .find(|slot| slot.instance_id == instance_id)
                        .map(|slot| crate::process::project_slot_identity_id(&slot));
                    if let Some(identity) = identity {
                        let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
                        for track_overrides in all.iter_mut() {
                            if let Some(override_) = track_overrides.get_mut(&identity) {
                                override_.fanout.remove(port_name);
                                if override_.is_empty() {
                                    track_overrides.remove(&identity);
                                }
                            }
                        }
                    }
                    changed
                } else {
                    // Fork from what this track currently hears.
                    let current = self
                        .composed_track_process_chain(track)
                        .and_then(|chain| {
                            chain
                                .slots
                                .into_iter()
                                .find(|slot| slot.instance_id == instance_id)
                        })
                        .map(|slot| slot.fanout.get(port_name).cloned().unwrap_or_default());
                    let Some(current) = current else {
                        return false;
                    };
                    let Some(changed) = self.edit_project_slot_override(track, instance_id, |override_| {
                        let list = override_
                            .fanout
                            .entry(port_name.to_string())
                            .or_insert(current);
                        let before = list.clone();
                        (edit.take().unwrap())(list);
                        *list != before
                    }) else {
                        return false;
                    };
                    changed
                }
            }
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Remove a project port binding from the shared slot on every track.
    /// Disconnect a port outright: drop its manual binding and mute the
    /// definition's target hint so the port writes nothing on fire. Track
    /// slots edit in place; project slots fork this track only. Binding or
    /// clearing the port reconnects it.
    pub fn unbind_process_port(
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
                .map(|slot| {
                    let had_binding = slot.bindings.remove(port_name).is_some();
                    slot.unbound_ports.insert(port_name.to_string()) | had_binding
                })
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_slot_override(track, instance_id, |override_| {
                let had_binding = override_.bindings.remove(port_name).is_some();
                override_.unbound_ports.insert(port_name.to_string()) | had_binding
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Disconnect a project slot's port on every track: the shared slot is
    /// muted and every track's fork of the port is dropped.
    pub fn unbind_process_port_for_instance(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
    ) -> bool {
        let mut changed = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    changed |= slot.bindings.remove(port_name).is_some();
                    changed |= slot.unbound_ports.insert(port_name.to_string());
                }
            }
        }
        if let Some(project_changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
            slot.bindings.remove(port_name).is_some() | slot.unbound_ports.insert(port_name.to_string())
        }) {
            changed |= project_changed;
            let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
            let identity = self
                .project_process_chain()
                .slots
                .into_iter()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| crate::process::project_slot_identity_id(&slot));
            if let Some(identity) = identity {
                for track_overrides in all.iter_mut() {
                    if let Some(override_) = track_overrides.get_mut(&identity) {
                        changed |= override_.bindings.remove(port_name).is_some();
                        changed |= override_.unbound_ports.remove(port_name);
                        if override_.is_empty() {
                            track_overrides.remove(&identity);
                        }
                    }
                }
            }
        }
        if changed {
            self.publish_process_chain_edit();
        }
        changed
    }
    pub fn clear_process_port_binding_for_instance(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
    ) -> bool {
        let mut changed = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    changed |= slot.bindings.remove(port_name).is_some();
                    changed |= slot.unbound_ports.remove(port_name);
                }
            }
        }
        if let Some(project_changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
            slot.bindings.remove(port_name).is_some() | slot.unbound_ports.remove(port_name)
        }) {
            changed |= project_changed;
            // A shared clear also drops every track's own override of the
            // port, otherwise the tracks would keep their forked targets.
            let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
            let identity = self
                .project_process_chain()
                .slots
                .into_iter()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| crate::process::project_slot_identity_id(&slot));
            if let Some(identity) = identity {
                for track_overrides in all.iter_mut() {
                    if let Some(override_) = track_overrides.get_mut(&identity) {
                        changed |= override_.bindings.remove(port_name).is_some();
                        changed |= override_.unbound_ports.remove(port_name);
                        if override_.is_empty() {
                            track_overrides.remove(&identity);
                        }
                    }
                }
            }
        }
        if changed {
            self.publish_process_chain_edit();
        }
        changed
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
        // A project slot edited from one track's UI forks that track only
        // (docs/default-process-lanes-spec.md); `set_process_inlet_value`
        // is the every-track write.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_slot_override(track, instance_id, |override_| {
                let changed = override_.inlets.get(inlet_name) != Some(&value);
                override_.inlets.insert(inlet_name.to_string(), value.clone());
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
            let reconnected = slot.unbound_ports.remove(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                reconnected
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
        // Project slots fork per track here; `set_process_port_binding_for_instance`
        // is the every-track write.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_slot_override(track, instance_id, |override_| {
                let current = override_.bindings.get(port_name);
                let reconnected = override_.unbound_ports.remove(port_name);
                if matches!(current, Some(Some(existing)) if existing == &target) {
                    reconnected
                } else {
                    override_
                        .bindings
                        .insert(port_name.to_string(), Some(target.clone()));
                    true
                }
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
                    changed |= slot.unbound_ports.remove(port_name);
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
            let reconnected = slot.unbound_ports.remove(port_name);
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                reconnected
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
                .map(|slot| {
                    slot.bindings.remove(port_name).is_some() | slot.unbound_ports.remove(port_name)
                })
        };
        // Clearing a project port from one track reverts that track to the
        // shared binding; `clear_process_port_binding_for_instance` clears
        // the shared one everywhere.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_slot_override(track, instance_id, |override_| {
                override_.bindings.remove(port_name).is_some()
                    | override_.unbound_ports.remove(port_name)
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

/// Apply `edit` to `fanout[port]`, dropping the key when the list ends empty.
fn edit_fanout_list(
    fanout: &mut BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    port_name: &str,
    edit: impl FnOnce(&mut Vec<crate::process::ProcessPortFanout>),
) -> bool {
    let list = fanout.entry(port_name.to_string()).or_default();
    let before = list.clone();
    edit(list);
    let changed = *list != before;
    if list.is_empty() {
        fanout.remove(port_name);
    }
    changed
}
