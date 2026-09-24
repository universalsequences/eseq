//! Host-owned instances of package kinds (`docs/instance-kinds-spec.md` §5).
//!
//! A package defines kinds with `def-kind`; the project owns instances of
//! them. Each instance has a host-assigned id that is stable within the
//! project and IS its published sequencer id, so graph overrides (keyed by
//! `sequencer_id`) belong to exactly one instance. Create, delete, duplicate
//! and rename are recorded, undoable project edits
//! (`EditPatch::InstanceStructure`); delete, duplicate and move also carry
//! the graph overrides keyed by the instances they touch (never the whole
//! scene bank, so replay cannot revert other sequencers' override edits,
//! which are not recorded).
//!
//! The Lisp records (`self`) are the UI's to mirror: every change bumps
//! `ProjectInstances::revision`, and the UI syncs its VM with
//! `lisp_host::sync_instance_records` when that moves.

use super::edit::finish_active_gesture;
use super::history::{
    EditPatch, GraphOverrideSite, InstanceOverridesState, InstanceStructurePatch,
    InstanceStructureState, ScratchImportState,
};
use super::*;
use crate::graph::ProjectGraphOverrides;
use crate::project::{ProjectInstance, ProjectInstanceOwner, ProjectInstances};
use crate::sequencer::ProjectScenes;

/// Every graph-override list in the scene bank with its site: each scene's
/// own list, then every clip of every rack bank, pointed at or not.
/// Instance edits that key on an id (delete, duplicate, move, kit load) walk
/// these directly rather than composing per scene, which only reaches the
/// clip each scene points at.
pub(super) fn for_each_override_list(
    scenes: &mut ProjectScenes,
    mut f: impl FnMut(GraphOverrideSite, &mut Vec<ProjectGraphOverrides>),
) {
    for scene in &mut scenes.scenes {
        f(GraphOverrideSite::Scene(scene.id), &mut scene.graph_overrides);
    }
    for bank in &mut scenes.rack_banks {
        let group_id = bank.group_id;
        for clip in &mut bank.clips {
            f(GraphOverrideSite::Clip { group_id, clip_id: clip.id }, &mut clip.graph_overrides);
        }
    }
}

/// Every graph override the scene bank holds (see [`for_each_override_list`]).
pub(super) fn all_override_entries(
    scenes: &ProjectScenes,
) -> impl Iterator<Item = &ProjectGraphOverrides> {
    scenes
        .scenes
        .iter()
        .flat_map(|scene| scene.graph_overrides.iter())
        .chain(
            scenes
                .rack_banks
                .iter()
                .flat_map(|bank| bank.clips.iter())
                .flat_map(|clip| clip.graph_overrides.iter()),
        )
}

impl App {
    /// `<kind name> <n>` with the smallest `n` no instance label uses yet.
    pub fn default_instance_label(&self, kind_id: &str) -> String {
        let name = crate::lisp_host::kind_name_of(kind_id);
        (1..)
            .map(|n| format!("{name} {n}"))
            .find(|label| !self.instances.list.iter().any(|instance| &instance.label == label))
            .expect("an unused label")
    }

    /// The next free instance id: never one an instance or a published
    /// sequencer already uses, and never one that still keys graph overrides
    /// in any scene or any rack clip (launched or not). Overrides are not
    /// instance edits, so undoing a create can leave overrides behind under
    /// the undone id; a later create must not inherit them.
    pub(super) fn allocate_instance_id(&mut self) -> u64 {
        let mut taken: HashSet<u64> = self
            .state
            .published_sequencers()
            .iter()
            .map(|sequencer| sequencer.id)
            .collect();
        self.state.with_scenes(|scenes| {
            taken.extend(all_override_entries(scenes).map(|graph| graph.sequencer_id));
        });
        let mut id = self.instances.next_id.max(1);
        while self.instances.contains(id) || taken.contains(&id) {
            id += 1;
        }
        self.instances.next_id = id + 1;
        id
    }

    fn validate_instance_owner(&self, owner: ProjectInstanceOwner) -> Result<(), String> {
        let Some(group_id) = owner.rack() else {
            return Ok(());
        };
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if group.rack.is_none() {
            return Err(format!("Track group {group_id} is not a drum rack"));
        }
        Ok(())
    }

    /// Publish (or re-publish) every instance whose kind is registered and
    /// has a `:sequencer` slot, with sequencer id == instance id. Idempotent:
    /// an unchanged instance is not re-published. An instance whose kind is
    /// not registered yet (package not attached, or the project is still
    /// replaying its scratch) stays unpublished until the kind registers.
    /// Returns how many sequencers were (un)published.
    pub fn publish_instance_sequencers(&self) -> usize {
        let published = self.state.published_sequencers();
        let mut changed = 0;
        for instance in &self.instances.list {
            let Some(kind) = crate::lisp_host::registered_kind(&instance.kind) else {
                continue;
            };
            match crate::lisp_host::instance_published_sequencer(
                &kind,
                instance.id,
                instance.owner.rack(),
            ) {
                Some(sequencer) => {
                    if !published.iter().any(|existing| *existing == sequencer) {
                        self.state.publish_sequencer(sequencer);
                        changed += 1;
                    }
                }
                // The kind lost its `:sequencer` slot on reload.
                None => {
                    if self.state.unpublish_sequencer_by_id(instance.id) {
                        changed += 1;
                    }
                }
            }
        }
        changed
    }

    /// Replace the whole instance list (project open / new project): the
    /// previous project's instance sequencers are unpublished first, and the
    /// generation bump makes the UI drop every record (and its view state)
    /// before mirroring the new list.
    pub fn replace_instances(&mut self, instances: ProjectInstances) {
        for instance in &self.instances.list {
            if !instances.contains(instance.id) {
                self.state.unpublish_sequencer_by_id(instance.id);
            }
        }
        let revision = self.instances.revision;
        let generation = self.instances.generation;
        self.instances = instances;
        self.instances.revision = revision + 1;
        self.instances.generation = generation + 1;
        self.publish_instance_sequencers();
    }

    fn capture_instance_structure_state(
        &self,
        overrides_of: Option<&[u64]>,
        scratch_import: Option<&str>,
    ) -> InstanceStructureState {
        InstanceStructureState {
            instances: self.instances.clone(),
            overrides: overrides_of.map(|ids| self.capture_instance_overrides(ids.to_vec())),
            scratch_import: scratch_import.map(|module| ScratchImportState {
                module: module.to_string(),
                present: import_modules(&self.state.scratch_source()).contains(module),
            }),
        }
    }

    /// The overrides keyed by `ids`, at every site that holds any.
    fn capture_instance_overrides(&self, ids: Vec<u64>) -> InstanceOverridesState {
        let mut sites = Vec::new();
        self.state.with_scenes_mut(|scenes| {
            for_each_override_list(scenes, |site, graphs| {
                let kept: Vec<ProjectGraphOverrides> = graphs
                    .iter()
                    .filter(|graph| ids.contains(&graph.sequencer_id))
                    .cloned()
                    .collect();
                if !kept.is_empty() {
                    sites.push((site, kept));
                }
            });
        });
        InstanceOverridesState { ids, sites }
    }

    /// Splice `state` back: drop every override keyed by one of its ids,
    /// wherever it is, and re-insert the recorded ones at their sites (a
    /// site that no longer exists is skipped). Every other sequencer's
    /// overrides are untouched.
    fn restore_instance_overrides(&self, state: &InstanceOverridesState) {
        self.state.with_scenes_mut(|scenes| {
            for_each_override_list(scenes, |site, graphs| {
                graphs.retain(|graph| !state.ids.contains(&graph.sequencer_id));
                for (recorded, entries) in &state.sites {
                    if *recorded == site {
                        graphs.extend(entries.iter().cloned());
                    }
                }
            });
        });
        self.state.publish_scheduler_snapshot();
    }

    /// Apply an id-keyed edit to every override list (see
    /// [`for_each_override_list`]); republishes when `edit` reports a change.
    pub(super) fn edit_instance_override_lists(
        &self,
        mut edit: impl FnMut(GraphOverrideSite, &mut Vec<ProjectGraphOverrides>) -> bool,
    ) -> bool {
        let mut changed = false;
        self.state.with_scenes_mut(|scenes| {
            for_each_override_list(scenes, |site, graphs| changed |= edit(site, graphs));
        });
        if changed {
            self.state.publish_scheduler_snapshot();
        }
        changed
    }

    /// Drop every override keyed by one of `ids`, in every scene and every
    /// rack clip.
    pub(super) fn drop_instance_overrides(&self, ids: &[u64]) -> bool {
        self.edit_instance_override_lists(|_, graphs| {
            let before = graphs.len();
            graphs.retain(|graph| !ids.contains(&graph.sequencer_id));
            graphs.len() != before
        })
    }

    /// Undo/redo target: the instance list (and the overrides of the
    /// instances the edit touched, when it carried them). Instances that
    /// disappear are unpublished; the rest re-publish under their ids.
    pub(super) fn restore_instance_structure_state(
        &mut self,
        target: &InstanceStructureState,
    ) -> Result<(), String> {
        if let Some(overrides) = &target.overrides {
            self.restore_instance_overrides(overrides);
        }
        if let Some(import) = &target.scratch_import {
            self.set_evaluated_scratch_import(import);
        }
        for instance in &self.instances.list {
            if !target.instances.contains(instance.id) {
                self.state.unpublish_sequencer_by_id(instance.id);
            }
        }
        let revision = self.instances.revision;
        // `next_id` never moves backwards: an id handed out once may still
        // key overrides the undo did not remove.
        let next_id = self.instances.next_id.max(target.instances.next_id);
        let generation = self.instances.generation;
        self.instances = target.instances.clone();
        self.instances.revision = revision + 1;
        self.instances.next_id = next_id;
        self.instances.generation = generation;
        self.publish_instance_sequencers();
        Ok(())
    }

    /// Run `mutate` as one recorded instance edit. `overrides_of` names the
    /// existing instances whose graph overrides the edit changes (`None`:
    /// it changes none); the overrides of instances `mutate` creates are
    /// recorded along with them, so undo removes them and redo brings them
    /// back.
    pub(super) fn apply_recorded_instance_mutation<T>(
        &mut self,
        label: &'static str,
        overrides_of: Option<Vec<u64>>,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        self.apply_recorded_instance_mutation_with(label, overrides_of, None, mutate)
    }

    /// [`Self::apply_recorded_instance_mutation`], optionally also
    /// recording whether the evaluated project scratch imports
    /// `scratch_import` (an edit that changes that import record along with
    /// the instances).
    fn apply_recorded_instance_mutation_with<T>(
        &mut self,
        label: &'static str,
        overrides_of: Option<Vec<u64>>,
        scratch_import: Option<&str>,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let mut before = self.capture_instance_structure_state(overrides_of.as_deref(), scratch_import);
        let result = mutate(self);
        // Ids the mutation minted: nothing keyed them before (the allocator
        // guarantees it), so their before-state is "no overrides".
        if let Some(overrides) = before.overrides.as_mut() {
            for instance in &self.instances.list {
                if !before.instances.contains(instance.id) && !overrides.ids.contains(&instance.id) {
                    overrides.ids.push(instance.id);
                }
            }
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                return match self.restore_instance_structure_state(&before) {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(format!(
                        "Instance edit failed ({error}); restoring its before-state also failed ({rollback})"
                    )),
                };
            }
        };
        let after = self.capture_instance_structure_state(
            before.overrides.as_ref().map(|overrides| overrides.ids.as_slice()),
            scratch_import,
        );
        for instance in &before.instances.list {
            if !after.instances.contains(instance.id) {
                self.state.unpublish_sequencer_by_id(instance.id);
            }
        }
        self.instances.revision += 1;
        self.publish_instance_sequencers();
        let patch = InstanceStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history
            .commit(label, None, EditPatch::InstanceStructure(patch), retained_bytes);
        Ok(result)
    }

    /// Create an instance of a registered kind. `label` defaults to
    /// `<kind> <n>`. Returns the new instance id.
    pub fn create_instance_recorded(
        &mut self,
        kind: &str,
        owner: ProjectInstanceOwner,
        label: Option<String>,
    ) -> Result<u64, String> {
        let definition = crate::lisp_host::registered_kind(kind).ok_or_else(|| {
            format!("Kind '{kind}' is not registered; attach its package first")
        })?;
        self.validate_instance_owner(owner)?;
        let label = label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| self.default_instance_label(&definition.id));
        self.apply_recorded_instance_mutation("Create instance", None, move |app| {
            let id = app.allocate_instance_id();
            app.instances.list.push(ProjectInstance {
                id,
                kind: definition.id,
                owner,
                label,
            });
            Ok(id)
        })
    }

    /// Delete an instance: its graph overrides (every scene, rack clips
    /// included) and its published sequencer go with it. The UI drops its
    /// state cells on the next record sync.
    pub fn delete_instance_recorded(&mut self, id: u64) -> Result<ProjectInstance, String> {
        if !self.instances.contains(id) {
            return Err(format!("Instance {id} does not exist"));
        }
        self.apply_recorded_instance_mutation("Delete instance", Some(vec![id]), move |app| {
            let position = app
                .instances
                .list
                .iter()
                .position(|instance| instance.id == id)
                .ok_or_else(|| format!("Instance {id} does not exist"))?;
            let removed = app.instances.list.remove(position);
            app.drop_instance_overrides(&[id]);
            Ok(removed)
        })
    }

    /// Delete several instances as ONE recorded edit, optionally dropping
    /// `detach_import`'s import line from the evaluated project scratch in
    /// the same edit: detaching a package deletes its instances and its
    /// import together, and one undo brings both back (spec §8.3). History
    /// records only that import's presence, never a scratch snapshot, so
    /// undo/redo re-add or re-remove that one line in whatever the scratch
    /// is by then. Ids that do not exist are ignored; returns the removed
    /// instances.
    pub fn delete_instances_recorded(
        &mut self,
        ids: &[u64],
        label: &'static str,
        detach_import: Option<&str>,
    ) -> Result<Vec<ProjectInstance>, String> {
        let ids: Vec<u64> = ids.iter().copied().filter(|id| self.instances.contains(*id)).collect();
        let detach_import = detach_import
            .filter(|module| import_modules(&self.state.scratch_source()).contains(*module));
        if ids.is_empty() && detach_import.is_none() {
            return Ok(Vec::new());
        }
        let recorded = Some(ids.clone());
        self.apply_recorded_instance_mutation_with(label, recorded, detach_import, move |app| {
            let mut removed = Vec::new();
            app.instances.list.retain(|instance| {
                if ids.contains(&instance.id) {
                    removed.push(instance.clone());
                    false
                } else {
                    true
                }
            });
            app.drop_instance_overrides(&ids);
            if let Some(module) = detach_import {
                app.set_evaluated_scratch_import(&ScratchImportState {
                    module: module.to_string(),
                    present: false,
                });
            }
            Ok(removed)
        })
    }

    /// Make the evaluated project scratch import `import.module` or not,
    /// touching only that import line. Unchanged text is not re-set (a set
    /// bumps the scratch version and rebuilds the scheduler runtime).
    fn set_evaluated_scratch_import(&self, import: &ScratchImportState) {
        let current = self.state.scratch_source();
        if import.present {
            if let Some(updated) = source_with_leading_import(&current, &import.module) {
                self.state.set_scratch_source(updated);
            }
        } else {
            let (updated, removed) = source_without_import(&current, &import.module);
            if removed {
                self.state.set_scratch_source(updated);
            }
        }
    }

    /// Duplicate an instance: same kind and owner, a fresh id and default
    /// label, and a copy of the source's graph overrides wherever it has
    /// them (every scene, every rack clip, launched or not). The copies'
    /// node process slots get fresh slot ids (and the wires into them
    /// follow), so the duplicate's stateful node processes never share
    /// runtime state with the source's. Returns the new instance id.
    pub fn duplicate_instance_recorded(&mut self, id: u64) -> Result<u64, String> {
        let source = self
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| format!("Instance {id} does not exist"))?;
        let label = self.default_instance_label(&source.kind);
        let source_id = source.id;
        self.apply_recorded_instance_mutation("Duplicate instance", Some(vec![source_id]), move |app| {
            let new_id = app.allocate_instance_id();
            let name = crate::lisp_host::instance_sequencer_name(
                crate::lisp_host::kind_name_of(&source.kind),
                new_id,
            );
            let mut reminter = app.state.with_scenes(|scenes| {
                crate::lisp_host::GraphNodeProcessReminter::new(all_override_entries(scenes))
            });
            app.edit_instance_override_lists(|_, graphs| {
                let copies: Vec<_> = graphs
                    .iter()
                    .filter(|graph| graph.sequencer_id == source.id)
                    .cloned()
                    .map(|mut graph| {
                        graph.sequencer_id = new_id;
                        graph.sequencer_name = name.clone();
                        reminter.remint(new_id, &mut graph);
                        graph
                    })
                    .collect();
                let changed = !copies.is_empty();
                graphs.extend(copies);
                changed
            });
            let position = app
                .instances
                .list
                .iter()
                .position(|instance| instance.id == source.id)
                .map(|position| position + 1)
                .unwrap_or(app.instances.list.len());
            app.instances.list.insert(
                position,
                ProjectInstance {
                    id: new_id,
                    kind: source.kind,
                    owner: source.owner,
                    label,
                },
            );
            Ok(new_id)
        })
    }

    /// Move an instance between the project and a rack (or between racks):
    /// "Move to rack ▸" / "Give back to project" (spec §5, §8.3). Its graph
    /// overrides follow: routes and seed tracks expand from the old rack's
    /// member indices to the tracks they resolve to, then contract to the new
    /// rack's member indices (a track outside the new rack becomes "off",
    /// exactly like moving a legacy sequencer into a rack), and `owner_rack`
    /// follows. The instance keeps its id, so nothing else re-keys and no
    /// script re-runs. Moving to the current owner is a quiet no-op.
    ///
    /// Where the overrides land: each scene's view of the instance (its own
    /// list, or the clip it points at in a clip-bearing old rack) is read
    /// once, before anything moves, so two scenes sharing a clip both keep
    /// it. Into a clip-bearing rack, every clip of its bank gets the entry of
    /// the first scene pointing at it, else the entry of the current scene
    /// (a clip no scene launches still plays the instance as configured).
    /// Out of a clip-bearing rack, a scene that launched no clip of it takes
    /// the current scene's entry too. Entries in the old rack's unlaunched
    /// clips have no destination and go.
    pub fn move_instance_owner_recorded(
        &mut self,
        id: u64,
        owner: ProjectInstanceOwner,
    ) -> Result<(), String> {
        let current = self
            .instances
            .get(id)
            .ok_or_else(|| format!("Instance {id} does not exist"))?
            .owner;
        if current == owner {
            return Ok(());
        }
        self.validate_instance_owner(owner)?;
        // A rack that no longer exists has no members to expand through:
        // its member routes go "off" rather than guessing.
        let from_members = current.rack().map(|group_id| {
            self.groups
                .iter()
                .find(|group| group.id == group_id)
                .map(|group| group.members.clone())
                .unwrap_or_default()
        });
        let to_members = owner
            .rack()
            .map(|group_id| self.rack_member_tracks(group_id))
            .transpose()?;
        self.apply_recorded_instance_mutation("Move instance", Some(vec![id]), move |app| {
            let current_scene = app.state.current_scene_index();
            let remap = |mut graph: ProjectGraphOverrides| {
                if let Some(members) = &from_members {
                    super::rack_sequencers::expand_member_routes_to_tracks(&mut graph, members);
                }
                if let Some(members) = &to_members {
                    super::rack_sequencers::contract_track_routes_to_members(&mut graph, members);
                }
                graph.owner_rack = owner.rack();
                graph
            };
            app.state.with_scenes_mut(|scenes| {
                let banked = |scenes: &ProjectScenes, group_id: u64| {
                    scenes.rack_bank(group_id).is_some_and(|bank| !bank.clips.is_empty())
                };
                let from_banked = current.rack().is_some_and(|group_id| banked(scenes, group_id));
                let per_scene: Vec<Option<ProjectGraphOverrides>> = (0..scenes.scenes.len())
                    .map(|scene_idx| {
                        scenes
                            .composed_graph_overrides(scene_idx)
                            .into_iter()
                            .find(|graph| graph.sequencer_id == id)
                            .map(&remap)
                    })
                    .collect();
                let fallback = per_scene
                    .get(current_scene)
                    .cloned()
                    .flatten()
                    .or_else(|| per_scene.iter().flatten().next().cloned())
                    .or_else(|| {
                        all_override_entries(scenes)
                            .find(|graph| graph.sequencer_id == id)
                            .cloned()
                            .map(&remap)
                    });
                for_each_override_list(scenes, |_, graphs| {
                    graphs.retain(|graph| graph.sequencer_id != id);
                });
                match owner.rack().filter(|group_id| banked(scenes, *group_id)) {
                    Some(group_id) => {
                        let pointers: Vec<Option<crate::sequencer::RackClipId>> = (0..scenes
                            .scenes
                            .len())
                            .map(|scene_idx| scenes.scene_rack_clip(scene_idx, group_id))
                            .collect();
                        if let Some(bank) = scenes.rack_bank_mut(group_id) {
                            for clip in &mut bank.clips {
                                let entry = pointers
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, pointer)| **pointer == Some(clip.id))
                                    .find_map(|(scene_idx, _)| per_scene[scene_idx].clone())
                                    .or_else(|| fallback.clone());
                                clip.graph_overrides.extend(entry);
                            }
                        }
                    }
                    None => {
                        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
                            let entry = per_scene[scene_idx]
                                .clone()
                                .or_else(|| fallback.clone().filter(|_| from_banked));
                            scene.graph_overrides.extend(entry);
                        }
                    }
                }
            });
            app.state.publish_scheduler_snapshot();
            app.instances
                .get_mut(id)
                .ok_or_else(|| format!("Instance {id} does not exist"))?
                .owner = owner;
            Ok(())
        })
    }

    /// The id a new track group takes: above every live group AND every
    /// rack an instance still names. Removing a rack releases its instances
    /// (`with_rack_instances_released`), but should any `Rack(gid)` outlive
    /// its rack, a new rack must never silently adopt it.
    pub(super) fn next_group_id(&self) -> u64 {
        self.groups
            .iter()
            .map(|group| group.id)
            .chain(self.instances.list.iter().filter_map(|instance| instance.owner.rack()))
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Run a rack-dissolving `edit` (the rack goes, its tracks stay) so
    /// the instances the racks `group_ids` own never outlive it as
    /// `Rack(gid)`: they move back to the project first, member routes
    /// expanded to the tracks they drove. Edits that also delete the tracks
    /// delete the instances instead (`delete_instances_recorded`). One undo
    /// entry, labelled `label`, covers the release and the edit; a failure
    /// rolls both back.
    pub(super) fn with_rack_instances_released<T>(
        &mut self,
        group_ids: &[u64],
        label: &'static str,
        edit: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        let owned: Vec<u64> = self
            .instances
            .list
            .iter()
            .filter(|instance| instance.owner.rack().is_some_and(|gid| group_ids.contains(&gid)))
            .map(|instance| instance.id)
            .collect();
        if owned.is_empty() {
            return edit(self);
        }
        let checkpoint = self.history.clone();
        let checkpoint_len = self.history.undo_len();
        let result = (|| {
            for id in &owned {
                self.move_instance_owner_recorded(*id, ProjectInstanceOwner::Project)?;
            }
            edit(self)
        })();
        match result {
            Ok(value) => {
                super::edit::squash_history_since(self, checkpoint_len, label);
                Ok(value)
            }
            Err(error) => match super::edit::rollback_history_to(self, checkpoint) {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!(
                    "{label} failed ({error}); rolling it back also failed ({rollback:?})"
                )),
            },
        }
    }

    /// Rename an instance (`(set! self.label v)` routes here). Writing back
    /// the current label is a no-op (no history entry, no error), so a view
    /// that commits its text input on blur stays quiet.
    pub fn rename_instance_recorded(&mut self, id: u64, label: &str) -> Result<(), String> {
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err("An instance label cannot be empty".to_string());
        }
        let current = self
            .instances
            .get(id)
            .ok_or_else(|| format!("Instance {id} does not exist"))?;
        if current.label == label {
            return Ok(());
        }
        self.apply_recorded_instance_mutation("Rename instance", None, move |app| {
            let instance = app
                .instances
                .get_mut(id)
                .ok_or_else(|| format!("Instance {id} does not exist"))?;
            instance.label = label;
            Ok(())
        })
    }
}

/// Project files below this version predate instances: their project-owned
/// `(import m)` of a module that now declares a kind ran that module's
/// legacy `def-sequencer`, so migration gives them one instance of the kind.
/// From this version on an absent instance means the user deleted it (or
/// never made one), and a scratch import alone creates nothing (spec §2:
/// importing registers kinds and creates nothing).
pub const INSTANCE_KINDS_PROJECT_VERSION: u32 = 14;

/// `Some(module)` when `source` is exactly one `(import module …)` form, the
/// way packages were recorded as rack sequencer sources and scratch lines.
/// Only migration reads these any more.
pub fn legacy_import_module(source: &str) -> Option<String> {
    use eseqlisp::parser::{ASTParser, Expression, Parser};
    let tokens = Parser::new(source.trim().to_string()).parse().ok()?;
    let expressions = ASTParser::new(tokens).parse().ok()?;
    let [Expression::List(items)] = expressions.as_slice() else {
        return None;
    };
    match items.as_slice() {
        [Expression::Symbol(import), Expression::Symbol(module), ..]
            if import == "import" && !module.starts_with(':') =>
        {
            Some(module.clone())
        }
        _ => None,
    }
}

/// Parse complete top-level forms only. A draft may end in an unfinished
/// form; never reinterpret its nested forms, quoted examples or string
/// contents as attachment records.
pub fn source_forms(source: &str) -> Vec<eseqlisp::parser::Expr> {
    let Ok(tokens) = eseqlisp::parser::Parser::new(source.to_string()).parse_spanned() else {
        return Vec::new();
    };
    let mut parser = eseqlisp::parser::SpannedASTParser::new(tokens);
    let mut forms = Vec::new();
    while parser.peek().is_some() {
        let Ok(form) = parser.parse_expression() else { break };
        forms.push(form);
    }
    forms
}

/// Every top-level `(import module …)` form in `source`, with its span.
pub fn import_forms(source: &str) -> Vec<(String, eseqlisp::parser::SourceSpan)> {
    use eseqlisp::parser::ExprKind;
    source_forms(source).into_iter().filter_map(|form| {
        let ExprKind::List(items) = form.kind else { return None };
        match items.as_slice() {
            [head, name, ..] if matches!(&head.kind, ExprKind::Symbol(s) if s == "import") => {
                match &name.kind {
                    ExprKind::Symbol(name) => Some((name.clone(), form.origin.primary_span)),
                    _ => None,
                }
            }
            _ => None,
        }
    }).collect()
}

pub fn import_modules(source: &str) -> HashSet<String> {
    import_forms(source).into_iter().map(|(name, _)| name).collect()
}

/// Remove only parsed top-level imports, including multiline forms. Preserve
/// neighboring code, comments and strings byte-for-byte. A form on a line of
/// its own also owns that line's indentation and newline, not surrounding
/// blank lines or comments.
pub fn source_without_import(source: &str, module: &str) -> (String, bool) {
    let mut updated = source.to_string();
    let mut removed = false;
    for (name, span) in import_forms(source).into_iter().rev() {
        if name != module { continue; }
        let mut start = span.start_byte;
        let mut end = span.end_byte;
        let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = source[end..].find('\n').map_or(source.len(), |i| end + i + 1);
        if source[line_start..start].trim().is_empty() && source[end..line_end].trim().is_empty() {
            start = line_start;
            end = line_end;
        }
        updated.replace_range(start..end, "");
        removed = true;
    }
    (updated, removed)
}

/// Append `(import module)` unless `source` already imports it. Returns the
/// text, the import's line, and whether it was already there.
pub fn source_with_import(source: &str, module: &str) -> (String, usize, bool) {
    if import_modules(source).contains(module) {
        let line = source
            .lines()
            .position(|line| line.contains(module))
            .unwrap_or(0);
        return (source.to_string(), line, true);
    }
    let mut updated = source.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    let line = updated.lines().count();
    updated.push_str(&format!("(import {module})\n"));
    (updated, line, false)
}

/// Restore `(import module)` where a detach took it from: after the
/// leading import lines (else at the top), so code below that uses the
/// module still evaluates after it. `None` when `source` already imports it.
pub fn source_with_leading_import(source: &str, module: &str) -> Option<String> {
    if import_modules(source).contains(module) {
        return None;
    }
    scratch_with_import(source, module)
}

pub(super) fn default_label_in(list: &[ProjectInstance], kind_id: &str) -> String {
    let name = crate::lisp_host::kind_name_of(kind_id);
    (1..)
        .map(|n| format!("{name} {n}"))
        .find(|label| !list.iter().any(|instance| &instance.label == label))
        .expect("an unused label")
}

fn describe_kinds(kinds: &[crate::lisp_host::DeclaredKind]) -> String {
    kinds
        .iter()
        .map(|kind| format!("'{}'", crate::lisp_host::kind_name_of(&kind.id)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Add `(import module)` to a scratch source unless a line already imports
/// it: after the last leading import line, else at the top.
fn scratch_with_import(source: &str, module: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    if lines
        .iter()
        .any(|line| legacy_import_module(line).as_deref() == Some(module))
    {
        return None;
    }
    let insert_at = lines
        .iter()
        .take_while(|line| line.trim().is_empty() || legacy_import_module(line).is_some())
        .count();
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    out.insert(insert_at, format!("(import {module})"));
    let mut text = out.join("\n");
    if source.ends_with('\n') || source.is_empty() {
        text.push('\n');
    }
    Some(text)
}

/// Legacy module-sourced sequencers -> instances (instance-kinds spec §10).
/// Runs on the project file before it is applied; `declared` lists the kinds
/// a module's package manifest declares for it (no code is evaluated).
///
/// - A rack sequencer recorded as `(import m)`, where `m` declares exactly
///   one kind, becomes an instance of that kind owned by the rack that KEEPS
///   the recorded `sequencer_id`, so the saved (member-relative) overrides
///   still match; its rack record is dropped. Such a module is then imported
///   by the project scratch (the way attaching a package does), because the
///   rack record that used to import it is gone and the kind must register.
/// - In files below [`INSTANCE_KINDS_PROJECT_VERSION`], a scratch
///   `(import m)` of a module declaring exactly one kind, and that no rack
///   recorded (the old owner map made those rack-owned), becomes one
///   project-owned instance with the id the legacy `def-sequencer` published
///   under: `graph_instance_id(legacy_sequencer or kind name, None)`.
/// - A module declaring more than one kind is reported, never guessed; its
///   rack record stays a (legacy) script.
///
/// Returns human-readable notes for the open status. Idempotent: migrated
/// rack records are gone and newer files skip the scratch step.
pub fn migrate_legacy_kind_sources(
    file_version: u32,
    groups: &mut [crate::project::ProjectTrackGroup],
    scratch: &mut crate::project::ProjectScratchState,
    instances: &mut ProjectInstances,
    declared: impl Fn(&str) -> Vec<crate::lisp_host::DeclaredKind>,
) -> Vec<String> {
    let mut notes = Vec::new();
    let mut rack_modules: HashSet<String> = HashSet::new();
    let mut attach: Vec<String> = Vec::new();
    for group in groups.iter_mut() {
        let group_id = group.id;
        let group_name = group.name.clone();
        let Some(rack) = group.rack.as_mut() else { continue };
        let mut kept = Vec::with_capacity(rack.sequencers.len());
        for record in std::mem::take(&mut rack.sequencers) {
            let Some(module) = legacy_import_module(&record.source) else {
                kept.push(record);
                continue;
            };
            rack_modules.insert(module.clone());
            let kinds = declared(&module);
            match kinds.as_slice() {
                [] => kept.push(record),
                [kind] => {
                    match instances.get(record.sequencer_id) {
                        Some(existing) if existing.kind == kind.id => {}
                        Some(existing) => {
                            notes.push(format!(
                                "{group_name}: '{}' kept as a script; its id already belongs to {}",
                                record.sequencer_name, existing.label
                            ));
                            kept.push(record);
                            continue;
                        }
                        None => {
                            let label = default_label_in(&instances.list, &kind.id);
                            instances.list.push(ProjectInstance {
                                id: record.sequencer_id,
                                kind: kind.id.clone(),
                                owner: ProjectInstanceOwner::Rack(group_id),
                                label,
                            });
                        }
                    }
                    if !attach.contains(&module) {
                        attach.push(module);
                    }
                }
                many => {
                    notes.push(format!(
                        "{group_name}: {module} declares {} kinds ({}); '{}' was left as a script, \
                         create the one you want instead",
                        many.len(),
                        describe_kinds(many),
                        record.sequencer_name
                    ));
                    kept.push(record);
                }
            }
        }
        rack.sequencers = kept;
    }

    if file_version < INSTANCE_KINDS_PROJECT_VERSION {
        let evaluated = scratch.evaluated_buffer.clone().unwrap_or_else(|| scratch.buffer.clone());
        let mut seen = HashSet::new();
        for module in evaluated.lines().filter_map(legacy_import_module) {
            if rack_modules.contains(&module) || !seen.insert(module.clone()) {
                continue;
            }
            let kinds = declared(&module);
            match kinds.as_slice() {
                [] => {}
                [kind] => {
                    let name = kind
                        .legacy_sequencer
                        .clone()
                        .unwrap_or_else(|| crate::lisp_host::kind_name_of(&kind.id).to_string());
                    let id = crate::lisp_host::graph_instance_id(&name, None);
                    if !instances.contains(id) {
                        let label = default_label_in(&instances.list, &kind.id);
                        instances.list.push(ProjectInstance {
                            id,
                            kind: kind.id.clone(),
                            owner: ProjectInstanceOwner::Project,
                            label,
                        });
                    }
                }
                many => notes.push(format!(
                    "{module} declares {} kinds ({}); no instance was created for its project \
                     import, create the one you want instead",
                    many.len(),
                    describe_kinds(many)
                )),
            }
        }
    }

    for module in attach {
        let mut added = false;
        if let Some(updated) = scratch_with_import(&scratch.buffer, &module) {
            scratch.buffer = updated;
            added = true;
        }
        if let Some(evaluated) = scratch.evaluated_buffer.as_mut() {
            if let Some(updated) = scratch_with_import(evaluated, &module) {
                *evaluated = updated;
                added = true;
            }
        }
        if added {
            notes.push(format!("attached {module} so its rack instances load"));
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;

    use super::*;
    use crate::app::edit::{redo, undo};
    use crate::app::history::HistoryReplay;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{default_empty_effect_chain, PatternSnapshot, SequencerState};

    const KIND: &str = "scratch:tst";

    /// One-track, one-scene app plus a UI-style runtime that has evaluated
    /// a headerless `def-kind tst` (kind id `scratch:tst`).
    fn fixture() -> (App, Runtime) {
        fixture_with_tracks(1)
    }

    fn fixture_with_tracks(tracks: usize) -> (App, Runtime) {
        crate::lisp_host::clear_kind_registry();
        let state =
            SequencerState::new(tracks, (0..tracks).map(|_| default_empty_effect_chain()).collect());
        state.replace_pattern_repository(vec![PatternSnapshot::new_default(tracks, &[])], 0);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = (1..=tracks).map(|n| format!("Track {n}")).collect();
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(tracks).unwrap();

        let mut runtime = Runtime::new();
        crate::lisp_host::register_graph_authoring_natives(&mut runtime, Arc::clone(&app.state));
        let kind = runtime
            .eval_str(
                "(def-kind tst
                   :sequencer (:shape (line 3) :max-poly 2
                               (def-node nrn :route 0
                                 :params ((threshold :float 0 4 :default 0.5)))
                               (edges :from nrn :to nrn :topology (all-to-all)
                                 :params ((weight :float -1 1 :default 0))))
                   :state ((sel -1) (open 0))
                   :view (lambda (self) self.sel))",
            )
            .expect("def-kind evaluates");
        assert_eq!(kind, Some(Value::String(KIND.to_string())));
        (app, runtime)
    }

    fn published_ids(app: &App) -> Vec<u64> {
        let mut ids: Vec<u64> =
            app.state.published_sequencers().iter().map(|sequencer| sequencer.id).collect();
        ids.sort_unstable();
        ids
    }

    fn max_poly_overrides(app: &App) -> Vec<(u64, String, Option<u32>)> {
        let mut rows: Vec<_> = app
            .state
            .current_graph_overrides()
            .into_iter()
            .map(|graph| (graph.sequencer_id, graph.sequencer_name, graph.max_poly))
            .collect();
        rows.sort();
        rows
    }

    fn labels(app: &App) -> Vec<(u64, String)> {
        app.instances
            .list
            .iter()
            .map(|instance| (instance.id, instance.label.clone()))
            .collect()
    }

    fn applied<E: std::fmt::Debug>(replay: HistoryReplay<E>) {
        assert!(matches!(replay, HistoryReplay::Applied(_)), "{replay:?}");
    }

    #[test]
    fn def_kind_registers_the_kind_with_its_sequencer_and_state() {
        let (_app, runtime) = fixture();
        let kind = crate::lisp_host::registered_kind(KIND).expect("kind registered");
        assert_eq!(kind.name, "tst");
        assert_eq!(kind.module, None);
        assert_eq!(kind.state_fields, vec!["sel".to_string(), "open".to_string()]);
        assert!(kind.has_view);
        let manifest = kind.sequencer.expect(":sequencer parsed as a graph manifest");
        assert_eq!(manifest.max_poly, 2);
        let schema = runtime.instance_kind_schema(KIND).expect("VM schema");
        assert_eq!(schema.fields.len(), 2);
        assert!(schema.view.is_some());
    }

    #[test]
    fn create_publishes_under_the_instance_id_with_default_labels() {
        let (mut app, _runtime) = fixture();
        let a = app
            .create_instance_recorded(KIND, ProjectInstanceOwner::Project, None)
            .expect("create a");
        let b = app
            .create_instance_recorded(KIND, ProjectInstanceOwner::Project, None)
            .expect("create b");
        assert_ne!(a, b);
        assert_eq!(labels(&app), vec![(a, "tst 1".to_string()), (b, "tst 2".to_string())]);
        assert_eq!(published_ids(&app), {
            let mut ids = vec![a, b];
            ids.sort_unstable();
            ids
        });
        let published = app.state.published_sequencers();
        let manifest_a = published
            .iter()
            .find(|sequencer| sequencer.id == a)
            .and_then(|sequencer| sequencer.graph.clone())
            .expect("a publishes a graph");
        assert_eq!(manifest_a.id, a, "sequencer id == instance id");
        assert_eq!(manifest_a.owner_rack, None);
        assert_eq!(manifest_a.name, crate::lisp_host::instance_sequencer_name("tst", a));

        // Unknown kinds and non-rack owners are refused without an edit.
        assert!(app
            .create_instance_recorded("nope:missing", ProjectInstanceOwner::Project, None)
            .is_err());
        assert!(app
            .create_instance_recorded(KIND, ProjectInstanceOwner::Rack(999), None)
            .is_err());
        assert_eq!(app.instances.list.len(), 2);

        // Undo removes and unpublishes; redo brings it back under the same id.
        applied(undo(&mut app));
        assert_eq!(labels(&app), vec![(a, "tst 1".to_string())]);
        assert_eq!(published_ids(&app), vec![a]);
        applied(redo(&mut app));
        assert_eq!(labels(&app), vec![(a, "tst 1".to_string()), (b, "tst 2".to_string())]);
        assert_eq!(published_ids(&app).len(), 2);
    }

    #[test]
    fn graph_natives_take_instances_and_overrides_stay_per_instance() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.set_global_value("b", Value::Instance(b));
        assert_eq!(
            runtime.eval_str("(graph-config a :max-poly 7)").unwrap(),
            Some(Value::Bool(true)),
            "graph-config accepts an instance value as its handle"
        );
        assert_eq!(
            runtime.eval_str("(graph-config-value a :max-poly)").unwrap(),
            Some(Value::Number(7.0))
        );
        assert_eq!(
            runtime.eval_str("(graph-config-value b :max-poly)").unwrap(),
            Some(Value::Number(2.0)),
            "a second instance of the same kind does not see the first one's overrides"
        );
        let name_a = crate::lisp_host::instance_sequencer_name("tst", a);
        assert_eq!(max_poly_overrides(&app), vec![(a, name_a, Some(7))]);
    }

    #[test]
    fn undone_create_never_hands_its_id_and_overrides_to_the_next_create() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.eval_str("(graph-config a :max-poly 7)").unwrap();
        applied(undo(&mut app));
        assert!(app.instances.list.is_empty());
        assert!(app.instances.next_id > a, "undo never moves the allocator backwards");

        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        assert_ne!(b, a, "the undone id still keys overrides, so it is not reused");
        runtime.set_global_value("b", Value::Instance(b));
        assert_eq!(
            runtime.eval_str("(graph-config-value b :max-poly)").unwrap(),
            Some(Value::Number(2.0)),
            "the fresh instance starts from the kind's defaults"
        );

        // A reload drops `next_id` with an empty list; the scene bank's
        // leftover overrides still keep the id from being reused.
        applied(undo(&mut app));
        app.replace_instances(ProjectInstances::default());
        let c = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        assert_ne!(c, a);
    }

    #[test]
    fn def_kind_rejects_host_field_state_without_registering_the_kind() {
        let (_app, mut runtime) = fixture();
        let version = crate::lisp_host::kind_registry_version();
        let result = runtime.eval_str("(def-kind bad :state ((label 0)))");
        // Native errors surface as a status plus `false`, never the kind id.
        assert!(
            matches!(result, Err(_) | Ok(Some(Value::Bool(false)))),
            "{result:?}"
        );
        assert!(crate::lisp_host::registered_kind("scratch:bad").is_none());
        assert_eq!(crate::lisp_host::kind_registry_version(), version);
    }

    #[test]
    fn def_kind_template_ignores_an_ambient_rack_owner_scope() {
        let (_app, mut runtime) = fixture();
        let version = crate::lisp_host::kind_registry_version();
        let src = "(def-kind tst
                   :sequencer (:shape (line 3) :max-poly 2
                               (def-node nrn :route 0
                                 :params ((threshold :float 0 4 :default 0.5)))
                               (edges :from nrn :to nrn :topology (all-to-all)
                                 :params ((weight :float -1 1 :default 0))))
                   :state ((sel -1) (open 0))
                   :view (lambda (self) self.sel))";
        crate::lisp_host::with_graph_owner_rack(Some(42), || runtime.eval_str(src).unwrap());
        assert_eq!(
            crate::lisp_host::kind_registry_version(),
            version,
            "the same def-kind inside a rack scope registers the same definition"
        );
        let template = crate::lisp_host::registered_kind(KIND).unwrap().sequencer.unwrap();
        assert_eq!(template.owner_rack, None);
    }

    #[test]
    fn detaching_with_instances_is_one_edit_that_undo_restores_with_the_import() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let keep = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.eval_str("(graph-config a :max-poly 5)").unwrap();
        app.state.set_scratch_source("(import pkg.mod)\n(def x 1)\n".to_string());

        let removed = app
            .delete_instances_recorded(&[a, b, 999], "Detach package", Some("pkg.mod"))
            .expect("detach edit");
        assert_eq!(removed.iter().map(|instance| instance.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(labels(&app), vec![(keep, "tst 3".to_string())]);
        assert_eq!(app.state.scratch_source(), "(def x 1)\n");
        assert!(max_poly_overrides(&app).is_empty(), "their overrides go too");
        assert!(!published_ids(&app).contains(&a) && !published_ids(&app).contains(&b));

        // ONE undo brings back both instances, their overrides and the import.
        applied(undo(&mut app));
        assert_eq!(
            labels(&app),
            vec![(a, "tst 1".to_string()), (b, "tst 2".to_string()), (keep, "tst 3".to_string())]
        );
        assert_eq!(app.state.scratch_source(), "(import pkg.mod)\n(def x 1)\n");
        let name = crate::lisp_host::instance_sequencer_name("tst", a);
        assert_eq!(max_poly_overrides(&app), vec![(a, name, Some(5))]);
        assert!(published_ids(&app).contains(&a));
        applied(redo(&mut app));
        assert_eq!(labels(&app), vec![(keep, "tst 3".to_string())]);
        assert_eq!(app.state.scratch_source(), "(def x 1)\n");

        // Scratch edits made after the detach (attaching another package,
        // evaluating new code) survive undo AND redo: history re-adds or
        // re-removes only the detached module's import.
        app.state.set_scratch_source("(import pkg.other)\n(def x 1)\n(def z 3)\n".to_string());
        applied(undo(&mut app));
        assert_eq!(
            app.state.scratch_source(),
            "(import pkg.other)\n(import pkg.mod)\n(def x 1)\n(def z 3)\n",
            "the import comes back after the leading imports, before the code"
        );
        applied(redo(&mut app));
        assert_eq!(app.state.scratch_source(), "(import pkg.other)\n(def x 1)\n(def z 3)\n");
        // Undo when the user already re-imported it by hand: nothing doubles.
        app.state.set_scratch_source("(import pkg.mod)\n(import pkg.other)\n".to_string());
        applied(undo(&mut app));
        assert_eq!(app.state.scratch_source(), "(import pkg.mod)\n(import pkg.other)\n");
        applied(redo(&mut app));
        assert_eq!(app.state.scratch_source(), "(import pkg.other)\n");

        // A module the evaluated scratch does not import records no import.
        assert!(app
            .delete_instances_recorded(&[], "Detach package", Some("pkg.absent"))
            .unwrap()
            .is_empty());

        // A plain delete leaves the scratch alone on undo.
        app.state.set_scratch_source("(def y 2)\n".to_string());
        app.delete_instances_recorded(&[keep], "Delete instance", None).unwrap();
        applied(undo(&mut app));
        assert_eq!(app.state.scratch_source(), "(def y 2)\n");
        assert!(app.delete_instances_recorded(&[12345], "noop", None).unwrap().is_empty());
    }

    #[test]
    fn duplicate_copies_overrides_and_delete_drops_them_all_undoable() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.eval_str("(graph-config a :max-poly 7)").unwrap();

        let c = app.duplicate_instance_recorded(a).expect("duplicate a");
        assert!(![a, b].contains(&c));
        assert_eq!(
            labels(&app),
            vec![(a, "tst 1".to_string()), (c, "tst 3".to_string()), (b, "tst 2".to_string())],
            "the copy sits right after its source"
        );
        let name = |id| crate::lisp_host::instance_sequencer_name("tst", id);
        assert_eq!(
            max_poly_overrides(&app),
            vec![(a, name(a), Some(7)), (c, name(c), Some(7))],
            "the copy carries the source's overrides under its own id"
        );
        assert!(published_ids(&app).contains(&c));
        runtime.set_global_value("c", Value::Instance(c));
        runtime.eval_str("(graph-config c :max-poly 3)").unwrap();
        assert_eq!(
            runtime.eval_str("(graph-config-value a :max-poly)").unwrap(),
            Some(Value::Number(7.0)),
            "editing the copy leaves the source alone"
        );

        app.delete_instance_recorded(a).expect("delete a");
        assert_eq!(labels(&app), vec![(c, "tst 3".to_string()), (b, "tst 2".to_string())]);
        assert!(!published_ids(&app).contains(&a), "delete unpublishes");
        assert_eq!(max_poly_overrides(&app), vec![(c, name(c), Some(3))], "delete drops overrides");

        applied(undo(&mut app));
        assert_eq!(
            labels(&app),
            vec![(a, "tst 1".to_string()), (c, "tst 3".to_string()), (b, "tst 2".to_string())]
        );
        assert!(published_ids(&app).contains(&a), "undo re-publishes");
        assert_eq!(max_poly_overrides(&app), vec![(a, name(a), Some(7)), (c, name(c), Some(3))]);

        // `graph-config` is not an instance edit, so the next undo is the
        // duplicate itself.
        applied(undo(&mut app));
        assert!(!app.instances.contains(c), "undoing the duplicate removes the copy");
        assert!(!published_ids(&app).contains(&c));
        assert!(max_poly_overrides(&app).iter().all(|(id, _, _)| *id != c));
        applied(redo(&mut app));
        assert!(app.instances.contains(c));
        assert!(max_poly_overrides(&app).iter().any(|(id, _, poly)| *id == c && *poly == Some(7)));
    }

    /// Graph edits are not recorded, so replaying an instance edit must
    /// touch only the overrides of the instances it changed: an edit to
    /// another instance made after it survives its undo and redo.
    #[test]
    fn undoing_an_instance_edit_keeps_later_override_edits_of_other_instances() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.set_global_value("b", Value::Instance(b));
        runtime.eval_str("(graph-config a :max-poly 7)").unwrap();

        let c = app.duplicate_instance_recorded(a).expect("duplicate a");
        app.delete_instance_recorded(a).expect("delete a");
        runtime.eval_str("(graph-config b :max-poly 5)").unwrap();
        let name = |id| crate::lisp_host::instance_sequencer_name("tst", id);

        applied(undo(&mut app));
        assert_eq!(
            max_poly_overrides(&app),
            vec![(a, name(a), Some(7)), (b, name(b), Some(5)), (c, name(c), Some(7))],
            "undoing the delete brings a back and keeps b's later edit"
        );
        applied(undo(&mut app));
        assert_eq!(
            max_poly_overrides(&app),
            vec![(a, name(a), Some(7)), (b, name(b), Some(5))],
            "undoing the duplicate drops only the copy"
        );
        applied(redo(&mut app));
        applied(redo(&mut app));
        assert_eq!(
            max_poly_overrides(&app),
            vec![(b, name(b), Some(5)), (c, name(c), Some(7))]
        );
    }

    #[test]
    fn instance_ids_survive_a_save_with_no_instances_left() {
        let mut instances = ProjectInstances::default();
        assert!(instances.is_unused());
        instances.next_id = 4;
        assert!(!instances.is_unused(), "a handed-out id keeps next_id saved");
    }

    #[test]
    fn rename_is_undoable_rejects_empty_and_ignores_unchanged_labels() {
        let (mut app, _runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        app.rename_instance_recorded(a, "  Kit A  ").expect("rename");
        assert_eq!(labels(&app), vec![(a, "Kit A".to_string())]);
        assert!(app.rename_instance_recorded(a, "   ").is_err());
        let revision = app.instances.revision;
        app.rename_instance_recorded(a, "Kit A").expect("an unchanged label is a quiet no-op");
        assert_eq!(app.instances.revision, revision, "no edit recorded");
        // The no-op left no history entry: one undo reverts the real rename.
        applied(undo(&mut app));
        assert_eq!(labels(&app), vec![(a, "tst 1".to_string())]);
        // A new instance never reuses a label in use.
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        assert_eq!(app.instances.get(b).unwrap().label, "tst 2");
    }

    #[test]
    fn records_mirror_the_project_list_into_the_vm() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let b = app
            .create_instance_recorded(KIND, ProjectInstanceOwner::Project, Some("Kit B".into()))
            .unwrap();
        assert!(crate::lisp_host::sync_instance_records(&mut runtime, &app.instances));
        assert!(!crate::lisp_host::sync_instance_records(&mut runtime, &app.instances));
        assert_eq!(runtime.live_instances(), {
            let mut ids = vec![a, b];
            ids.sort_unstable();
            ids
        });
        assert_eq!(runtime.instance_field(b, "label"), Ok(Value::String("Kit B".into())));
        assert_eq!(runtime.instance_field(a, "owner"), Ok(Value::Keyword("project".into())));
        assert_eq!(runtime.instance_field(a, "sel"), Ok(Value::Number(-1.0)));
        runtime.set_global_value("a", Value::Instance(a));
        assert_eq!(runtime.eval_str("a.label").unwrap(), Some(Value::String("tst 1".into())));
        assert_eq!(runtime.eval_str("a.id").unwrap(), Some(Value::Number(a as f64)));

        app.rename_instance_recorded(a, "Kit A").unwrap();
        app.delete_instance_recorded(b).unwrap();
        assert!(crate::lisp_host::sync_instance_records(&mut runtime, &app.instances));
        assert_eq!(runtime.live_instances(), vec![a]);
        assert_eq!(runtime.eval_str("a.label").unwrap(), Some(Value::String("Kit A".into())));
        assert!(!runtime.instance_is_live(b), "the deleted instance's cells are dropped");
    }

    #[test]
    fn kind_reload_republishes_instances_and_replace_unpublishes_the_old_project() {
        let (mut app, mut runtime) = fixture();
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        let version = crate::lisp_host::kind_registry_version();
        runtime
            .eval_str(
                "(def-kind tst
                   :sequencer (:shape (line 3) :max-poly 5
                               (def-node nrn :route 0
                                 :params ((threshold :float 0 4 :default 0.5)))
                               (edges :from nrn :to nrn :topology (all-to-all)
                                 :params ((weight :float -1 1 :default 0))))
                   :state ((sel -1) (open 0)))",
            )
            .unwrap();
        assert!(crate::lisp_host::kind_registry_version() > version);
        assert_eq!(app.publish_instance_sequencers(), 1, "the changed kind re-publishes");
        assert_eq!(app.publish_instance_sequencers(), 0, "idempotent");
        let max_poly = app
            .state
            .published_sequencers()
            .into_iter()
            .find(|sequencer| sequencer.id == a)
            .and_then(|sequencer| sequencer.graph)
            .map(|manifest| manifest.max_poly);
        assert_eq!(max_poly, Some(5));

        app.replace_instances(ProjectInstances::default());
        assert!(published_ids(&app).is_empty(), "the previous project's instances unpublish");
    }

    /// Rack 1 over tracks 1 and 2 of a four-track fixture.
    fn with_rack(app: &mut App) {
        app.groups = serde_json::from_value(serde_json::json!([{
            "id": 1, "name": "Kit", "members": [1, 2], "bus_id": 0,
            "rack": {"pads": [{"pad_note": 36, "member": 0}, {"pad_note": 37, "member": 1}]}
        }]))
        .unwrap();
        app.state.set_rack_memberships(app.rack_memberships());
    }

    /// `(owner_rack, route of node 0, route of node 1)` of instance `id`'s
    /// override in the current scene.
    fn routes(
        app: &App,
        id: u64,
    ) -> (Option<u64>, Option<crate::graph::ProjectGraphRouteOverride>, Option<crate::graph::ProjectGraphRouteOverride>) {
        let graph = app
            .state
            .current_graph_overrides()
            .into_iter()
            .find(|graph| graph.sequencer_id == id)
            .expect("instance overrides");
        let route = |node: usize| {
            graph
                .node_intrinsics
                .iter()
                .find(|intrinsic| intrinsic.instance == node)
                .and_then(|intrinsic| intrinsic.route.clone())
        };
        (graph.owner_rack, route(0), route(1))
    }

    fn published_owner(app: &App, id: u64) -> Option<u64> {
        app.state
            .published_sequencers()
            .into_iter()
            .find(|sequencer| sequencer.id == id)
            .and_then(|sequencer| sequencer.graph)
            .expect("published graph")
            .owner_rack
    }

    #[test]
    fn one_rack_owns_two_independent_instances_of_one_kind() {
        use crate::graph::ProjectGraphRouteOverride::{None as Off, Track};
        let (mut app, mut runtime) = fixture_with_tracks(4);
        with_rack(&mut app);
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Rack(1), None).unwrap();
        let b = app.create_instance_recorded(KIND, ProjectInstanceOwner::Rack(1), None).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            app.rack_instances(1).iter().map(|instance| instance.id).collect::<Vec<_>>(),
            vec![a, b],
            "the rack lists both instances"
        );
        assert_eq!(published_owner(&app, a), Some(1));
        assert_eq!(published_owner(&app, b), Some(1));

        runtime.set_global_value("a", Value::Instance(a));
        runtime.set_global_value("b", Value::Instance(b));
        runtime.eval_str("(graph-node a 0 :route 1)").unwrap();
        runtime.eval_str("(graph-node b 0 :route 0)").unwrap();
        runtime.eval_str("(graph-config a :max-poly 7)").unwrap();
        assert_eq!(routes(&app, a), (Some(1), Some(Track(1)), None));
        assert_eq!(routes(&app, b), (Some(1), Some(Track(0)), None));
        assert_eq!(
            runtime.eval_str("(graph-config-value b :max-poly)").unwrap(),
            Some(Value::Number(2.0)),
            "the second instance in the same rack keeps its own overrides"
        );

        // The first member leaves: member routes of BOTH instances follow,
        // although the rack records no legacy sequencer at all.
        app.remove_track_from_group_recorded(1).expect("member 0 leaves");
        assert_eq!(routes(&app, a), (Some(1), Some(Track(0)), None));
        assert_eq!(routes(&app, b), (Some(1), Some(Off), None));
    }

    #[test]
    fn move_to_a_rack_and_back_remaps_routes_and_is_undoable() {
        use crate::graph::ProjectGraphRouteOverride::{None as Off, Track};
        let (mut app, mut runtime) = fixture_with_tracks(4);
        with_rack(&mut app);
        let a = app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        runtime.set_global_value("a", Value::Instance(a));
        runtime.eval_str("(graph-node a 0 :route 2)").unwrap();
        runtime.eval_str("(graph-node a 1 :route 3)").unwrap();
        assert_eq!(routes(&app, a), (None, Some(Track(2)), Some(Track(3))));

        app.move_instance_owner_recorded(a, ProjectInstanceOwner::Rack(1)).expect("into rack");
        assert_eq!(app.instances.get(a).unwrap().owner, ProjectInstanceOwner::Rack(1));
        assert_eq!(
            routes(&app, a),
            (Some(1), Some(Track(1)), Some(Off)),
            "track 2 is member 1; track 3 is outside the rack and goes off"
        );
        assert_eq!(published_owner(&app, a), Some(1), "republished rack-owned under the same id");
        assert!(crate::lisp_host::sync_instance_records(&mut runtime, &app.instances));
        assert_eq!(runtime.eval_str("a.owner").unwrap(), Some(Value::Number(1.0)));
        assert_eq!(app.rack_instances(1).len(), 1);

        let revision = app.instances.revision;
        app.move_instance_owner_recorded(a, ProjectInstanceOwner::Rack(1)).expect("no-op");
        assert_eq!(app.instances.revision, revision, "moving to the current owner records nothing");
        assert!(app.move_instance_owner_recorded(a, ProjectInstanceOwner::Rack(99)).is_err());

        app.move_instance_owner_recorded(a, ProjectInstanceOwner::Project).expect("back");
        assert_eq!(routes(&app, a), (None, Some(Track(2)), Some(Off)));
        assert_eq!(published_owner(&app, a), None);
        assert!(app.rack_instances(1).is_empty());

        applied(undo(&mut app));
        assert_eq!(app.instances.get(a).unwrap().owner, ProjectInstanceOwner::Rack(1));
        assert_eq!(routes(&app, a), (Some(1), Some(Track(1)), Some(Off)));
        assert_eq!(published_owner(&app, a), Some(1));
        applied(undo(&mut app));
        assert_eq!(app.instances.get(a).unwrap().owner, ProjectInstanceOwner::Project);
        assert_eq!(
            routes(&app, a),
            (None, Some(Track(2)), Some(Track(3))),
            "undo restores the routes the move turned off"
        );
        assert_eq!(published_owner(&app, a), None);
        applied(redo(&mut app));
        assert_eq!(routes(&app, a), (Some(1), Some(Track(1)), Some(Off)));
    }

    #[test]
    fn legacy_import_records_migrate_to_instances_keeping_their_sequencer_ids() {
        let (mut app, mut runtime) = fixture_with_tracks(4);
        let declared = |module: &str| -> Vec<crate::lisp_host::DeclaredKind> {
            let kind = |id: &str, legacy: Option<&str>| crate::lisp_host::DeclaredKind {
                id: id.to_string(),
                legacy_sequencer: legacy.map(str::to_string),
            };
            match module {
                "demos.rack-neural" => vec![kind(KIND, Some("variable-reset"))],
                "demos.proj-neural" => vec![kind(KIND, Some("old-project"))],
                "demos.two-kinds" => vec![kind("demos:one", None), kind("demos:two", None)],
                _ => Vec::new(),
            }
        };
        let rack_id = crate::lisp_host::graph_instance_id("variable-reset", Some(1));
        let mut groups: Vec<crate::project::ProjectTrackGroup> =
            serde_json::from_value(serde_json::json!([{
                "id": 1, "name": "Kit", "members": [1, 2], "bus_id": 0,
                "rack": {"pads": [], "sequencers": [
                    {"sequencer_id": rack_id, "sequencer_name": "variable-reset",
                     "source": "(import demos.rack-neural)"},
                    {"sequencer_id": 5, "sequencer_name": "plain",
                     "source": "(load \"@/scripts/sequencers/plain.lisp\")"},
                    {"sequencer_id": 6, "sequencer_name": "ambiguous",
                     "source": "(import demos.two-kinds)"}
                ]}
            }]))
            .unwrap();
        let mut scratch = crate::project::ProjectScratchState {
            buffer: "(import demos.proj-neural)\n(import demos.other)\n(def x 1)\n".to_string(),
            evaluated_buffer: None,
            cursor_row: 0,
            cursor_col: 0,
        };
        let mut instances = ProjectInstances::default();
        let notes = migrate_legacy_kind_sources(13, &mut groups, &mut scratch, &mut instances, declared);

        let project_id = crate::lisp_host::graph_instance_id("old-project", None);
        assert_eq!(
            instances
                .list
                .iter()
                .map(|instance| (instance.id, instance.kind.as_str(), instance.owner, instance.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (rack_id, KIND, ProjectInstanceOwner::Rack(1), "tst 1"),
                (project_id, KIND, ProjectInstanceOwner::Project, "tst 2"),
            ],
            "the rack record and the project import each became one instance with the old id"
        );
        let kept: Vec<_> = groups[0]
            .rack
            .as_ref()
            .unwrap()
            .sequencers
            .iter()
            .map(|record| record.sequencer_name.as_str())
            .collect();
        assert_eq!(kept, vec!["plain", "ambiguous"], "plain scripts and ambiguous modules stay");
        assert!(
            notes.iter().any(|note| note.contains("declares 2 kinds") && note.contains("'one', 'two'")),
            "{notes:?}"
        );
        assert!(
            scratch.buffer.starts_with("(import demos.proj-neural)\n(import demos.other)\n(import demos.rack-neural)\n(def x 1)"),
            "the rack's module is attached to the project so its kind registers: {:?}",
            scratch.buffer
        );

        // Idempotent: the migrated record is gone, and a current-version file
        // never turns a scratch import into an instance.
        let mut again = instances.clone();
        let before = scratch.buffer.clone();
        let notes = migrate_legacy_kind_sources(
            INSTANCE_KINDS_PROJECT_VERSION,
            &mut groups,
            &mut scratch,
            &mut again,
            declared,
        );
        assert_eq!(again, instances);
        assert_eq!(scratch.buffer, before);
        assert_eq!(notes.len(), 1, "only the ambiguous module is reported again: {notes:?}");
        let mut fresh = ProjectInstances::default();
        let mut groups_none: Vec<crate::project::ProjectTrackGroup> = Vec::new();
        migrate_legacy_kind_sources(
            INSTANCE_KINDS_PROJECT_VERSION,
            &mut groups_none,
            &mut scratch,
            &mut fresh,
            declared,
        );
        assert!(fresh.is_empty(), "a v14 scratch import creates nothing");

        // Loaded into the app, the rack instance picks the saved
        // member-relative overrides up by its (old) id.
        app.groups = groups;
        app.state.set_rack_memberships(app.rack_memberships());
        app.state.edit_all_scene_graph_overrides(|graphs| {
            graphs.push(crate::graph::ProjectGraphOverrides {
                sequencer_id: rack_id,
                sequencer_name: "variable-reset".to_string(),
                owner_rack: Some(1),
                max_poly: Some(7),
                ..Default::default()
            });
            true
        });
        app.replace_instances(instances);
        assert_eq!(published_owner(&app, rack_id), Some(1));
        runtime.set_global_value("r", Value::Instance(rack_id));
        assert_eq!(
            runtime.eval_str("(graph-config-value r :max-poly)").unwrap(),
            Some(Value::Number(7.0)),
            "the old overrides still match the migrated instance"
        );
        let fresh_id =
            app.create_instance_recorded(KIND, ProjectInstanceOwner::Project, None).unwrap();
        assert!(![rack_id, project_id].contains(&fresh_id));
        assert_eq!(app.instances.get(fresh_id).unwrap().label, "tst 3");
    }

    #[test]
    fn legacy_import_module_reads_only_single_import_forms() {
        assert_eq!(legacy_import_module(" (import alez.neural.variable-reset) ").as_deref(), Some("alez.neural.variable-reset"));
        assert_eq!(legacy_import_module("(import a.b :only (x))").as_deref(), Some("a.b"));
        assert_eq!(legacy_import_module("(load \"x.lisp\")"), None);
        assert_eq!(legacy_import_module("(import a) (import b)"), None);
        assert_eq!(legacy_import_module(""), None);
    }

    #[test]
    fn project_file_round_trips_instances_and_older_files_load_none() {
        let instances = ProjectInstances {
            list: vec![ProjectInstance {
                id: 4,
                kind: "alez/neural:neural".into(),
                owner: ProjectInstanceOwner::Rack(9),
                label: "neural 1".into(),
            }],
            next_id: 5,
            revision: 0,
            generation: 0,
        };
        let json = serde_json::to_value(&instances).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "list": [{"id": 4, "kind": "alez/neural:neural", "owner": {"rack": 9}, "label": "neural 1"}],
                "next_id": 5
            })
        );
        let back: ProjectInstances = serde_json::from_value(json).unwrap();
        assert_eq!(back, instances);
        let legacy: ProjectInstances =
            serde_json::from_value(serde_json::json!({"list": [{"id": 1, "kind": "k:k"}]})).unwrap();
        assert_eq!(legacy.list[0].owner, ProjectInstanceOwner::Project);
        assert_eq!(legacy.list[0].label, "");
    }
}
