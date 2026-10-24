//! Project-owned sequencers on/off per bank (docs/rig-spec.md §10,
//! eseq-ka5t.1/.5/.6).
//!
//! `Scene::parked_instances` holds the ids of the project-owned kind
//! instances (density, jaki, neural, graph and generator kinds…) switched
//! off in that scene, the same rule as `Scene::parked_kits` (§3.2, §6) for a
//! different thing: who plays is scene state. A rack-owned instance follows
//! its kit and is never gated here, even when a stale entry names it (an
//! instance given to a rack after it was parked): `ProjectScenes::
//! instance_racks` filters those out wherever the parked set is read.
//!
//! Resolution is a gate in every scheduler snapshot, next to the kit gate
//! (`SequencerSnapshot::apply_scene_parking`): the instance's graph or generator
//! does not run in a scene that parks it (no fires, no state advance), and
//! every process slot declared with that instance as its owner
//! (`TrackProcessSlot::owner_instance`, `(processes … :owner id …)`) is
//! disabled in that snapshot, so a parked conductor's processes do not apply
//! even in a pattern another bank shares.
//!
//! The implementation is shared with kits (`super::parking`,
//! `ParkedSet::Instances`); this file holds what only instances need.
//!
//! Legacy projects park nothing: every instance stays active everywhere. A
//! project-owned instance created later is active only in the bank it was
//! created in (`App::create_instance_in_bank_recorded`).

use std::collections::BTreeSet;

use super::*;

/// The ids of the project-owned instances a scene switches off.
pub type ParkedInstances = BTreeSet<u64>;

impl ProjectScenes {
    /// Whether scene `scene` switches project-owned instance `id` off.
    pub fn instance_parked_in_scene(&self, scene: usize, id: u64) -> bool {
        self.parked_in_scene(ParkedSet::Instances, scene, id)
    }

    /// Give instance `to` the parked set `from` has, scene by scene (a
    /// duplicate plays where its source plays).
    pub fn copy_instance_parking(&mut self, from: u64, to: u64) {
        for scene in &mut self.scenes {
            if scene.parked_instances.contains(&from) {
                scene.parked_instances.insert(to);
            } else {
                scene.parked_instances.remove(&to);
            }
        }
    }

    /// Every scene's parked instances keyed by scene id, scenes parking
    /// nothing left out: an instance edit's history memento.
    pub fn capture_parked_instances(&self) -> Vec<(SceneId, Vec<u64>)> {
        self.scenes
            .iter()
            .filter(|scene| !scene.parked_instances.is_empty())
            .map(|scene| (scene.id, scene.parked_instances.iter().copied().collect()))
            .collect()
    }

    /// Restore `capture_parked_instances`' memento: scenes it does not name
    /// park nothing. Returns whether anything moved.
    pub fn restore_parked_instances(&mut self, captured: &[(SceneId, Vec<u64>)]) -> bool {
        let mut changed = false;
        for scene in &mut self.scenes {
            let wanted: ParkedInstances = captured
                .iter()
                .find(|(id, _)| *id == scene.id)
                .map(|(_, ids)| ids.iter().copied().collect())
                .unwrap_or_default();
            if scene.parked_instances != wanted {
                scene.parked_instances = wanted;
                changed = true;
            }
        }
        changed
    }
}

/// The process slots an owner gate reads (`TrackProcessSlot::
/// owner_instance`) live in every stored chain: each track's Patch entities
/// (take chunks share one), each scene's project layer, each graph
/// override's node chains (scenes and rack clips), and the live chains.
/// These walk all of them, for the instance edits that reserve, clear,
/// restore or remap owner ids.
type Chain = crate::process::TrackProcessChain;

fn graph_node_chains_mut(graphs: &mut [crate::graph::ProjectGraphOverrides]) -> impl Iterator<Item = &mut Chain> {
    graphs
        .iter_mut()
        .flat_map(|graph| graph.node_intrinsics.iter_mut())
        .filter_map(|node| node.process_chain.as_mut())
}

impl ProjectScenes {
    /// Rewrite every stored chain `touches` selects with `edit`, the scenes
    /// side of [`SequencerState::edit_every_process_chain`]. `tracks` limits
    /// the Patch entities to those tracks' pools (`None`: every track).
    fn edit_stored_process_chains(
        &mut self,
        tracks: Option<&[usize]>,
        touches: &mut impl FnMut(&Chain) -> bool,
        edit: &mut impl FnMut(&mut Chain),
    ) -> bool {
        let mut changed = false;
        for (track, pool) in self.track_pools.iter_mut().enumerate() {
            if tracks.is_some_and(|tracks| !tracks.contains(&track)) {
                continue;
            }
            for patch in pool.sounds.patches.values_mut() {
                if touches(&patch.process_chain) {
                    edit(&mut std::sync::Arc::make_mut(patch).process_chain);
                    changed = true;
                }
            }
        }
        if tracks.is_some() {
            return changed;
        }
        for scene in &mut self.scenes {
            if touches(&scene.project_process_chain) {
                edit(&mut scene.project_process_chain);
                changed = true;
            }
            for chain in graph_node_chains_mut(&mut scene.graph_overrides) {
                if touches(chain) {
                    edit(chain);
                    changed = true;
                }
            }
        }
        for bank in &mut self.rack_banks {
            for clip in &mut bank.clips {
                for chain in graph_node_chains_mut(&mut clip.graph_overrides) {
                    if touches(chain) {
                        edit(chain);
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

impl SequencerState {
    /// Rewrite every process chain the project holds that `touches` selects
    /// (stored and live; `tracks` limits it to those tracks' own chains),
    /// then publish once when anything changed. Returns whether it did.
    pub fn edit_every_process_chain(
        &self,
        tracks: Option<&[usize]>,
        mut touches: impl FnMut(&Chain) -> bool,
        mut edit: impl FnMut(&mut Chain),
    ) -> bool {
        let mut changed = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .edit_stored_process_chains(tracks, &mut touches, &mut edit);
        for (track, chain) in self.pattern.process_chains.lock().unwrap().iter_mut().enumerate() {
            if tracks.is_some_and(|tracks| !tracks.contains(&track)) {
                continue;
            }
            if touches(chain) {
                edit(chain);
                changed = true;
            }
        }
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    /// Every instance id some process slot names as its owner.
    pub fn process_slot_owner_ids(&self) -> BTreeSet<u64> {
        let mut owners = BTreeSet::new();
        // A read through the edit walker: `touches` sees every chain and
        // selects none, so nothing is written or published.
        self.edit_every_process_chain(
            None,
            |chain| {
                owners.extend(chain.slots.iter().filter_map(|slot| slot.owner_instance));
                false
            },
            |_| {},
        );
        owners
    }

    /// The slots owned by an instance in `ids`, as (slot id, owner): what a
    /// delete clears and its undo restores.
    pub fn owned_process_slots(&self, ids: &[u64]) -> Vec<(crate::process::ProcessInstanceId, u64)> {
        let mut found = BTreeSet::new();
        self.edit_every_process_chain(
            None,
            |chain| {
                for slot in &chain.slots {
                    if let Some(owner) = slot.owner_instance.filter(|owner| ids.contains(owner)) {
                        found.insert((slot.instance_id.0, owner));
                    }
                }
                false
            },
            |_| {},
        );
        found
            .into_iter()
            .map(|(slot, owner)| (crate::process::ProcessInstanceId(slot), owner))
            .collect()
    }

    /// Make the ownership by instances `ids` exactly `owned` (from
    /// [`Self::owned_process_slots`]): a slot listed there (by slot id, still
    /// unowned or owned by one of `ids`) gets its owner back; any other slot
    /// one of `ids` owns loses it. `owned` empty clears them (a delete).
    pub fn set_process_slot_owners(&self, ids: &[u64], owned: &[(crate::process::ProcessInstanceId, u64)]) -> bool {
        let wanted = |slot: &crate::process::TrackProcessSlot| -> Option<Option<u64>> {
            let recorded = owned
                .iter()
                .find(|(id, _)| *id == slot.instance_id)
                .map(|(_, owner)| *owner);
            let ours = slot.owner_instance.is_none_or(|owner| ids.contains(&owner));
            match recorded {
                Some(owner) if ours && slot.owner_instance != Some(owner) => Some(Some(owner)),
                None if slot.owner_instance.is_some_and(|owner| ids.contains(&owner)) => Some(None),
                _ => None,
            }
        };
        self.edit_every_process_chain(
            None,
            |chain| chain.slots.iter().any(|slot| wanted(slot).is_some()),
            |chain| {
                for slot in &mut chain.slots {
                    if let Some(owner) = wanted(slot) {
                        slot.owner_instance = owner;
                    }
                }
            },
        )
    }

    /// Re-key the owners on tracks `tracks` through `map` (old id -> new),
    /// an owner `map` does not name becoming none: the chains a kit load or
    /// a project import copied from another project carry that project's
    /// instance ids.
    pub fn remap_process_slot_owners(&self, tracks: &[usize], map: &std::collections::HashMap<u64, u64>) -> bool {
        self.edit_every_process_chain(
            Some(tracks),
            |chain| chain.slots.iter().any(|slot| slot.owner_instance.is_some()),
            |chain| {
                for slot in &mut chain.slots {
                    slot.owner_instance = slot.owner_instance.and_then(|owner| map.get(&owner).copied());
                }
            },
        )
    }
}

/// Disable every enabled process slot of `track` that an instance in
/// `parked` (sorted) owns. Copy-on-write: a track with none stays shared.
pub(super) fn park_owned_process_slots(
    track: &mut std::sync::Arc<crate::sequencer::SequencerTrackSnapshot>,
    parked: &[u64],
) {
    let owned = |slot: &crate::process::TrackProcessSlot| {
        slot.enabled && slot.owner_instance.is_some_and(|owner| parked.binary_search(&owner).is_ok())
    };
    if !track.process_chain.slots.iter().any(owned) {
        return;
    }
    for slot in &mut std::sync::Arc::make_mut(track).process_chain.slots {
        if owned(slot) {
            slot.enabled = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenes(count: usize) -> ProjectScenes {
        let snapshots: Vec<PatternSnapshot> = (0..count)
            .map(|_| PatternSnapshot::new_default(2, &[]))
            .collect();
        ProjectScenes::from_pattern_snapshots(&snapshots, 0)
    }

    const DENSITY: u64 = 12;
    const SET: ParkedSet = ParkedSet::Instances;

    #[test]
    fn an_instance_bank_state_reads_on_off_and_mixed() {
        let mut scenes = scenes(3);
        assert_eq!(scenes.parked_state_in_scenes(SET, DENSITY, 0..3), BankState::On);
        assert!(scenes.set_parked_in_scenes(SET, DENSITY, true, 0..1));
        assert_eq!(scenes.parked_state_in_scenes(SET, DENSITY, 0..3), BankState::Mixed);
        assert!(scenes.set_parked_in_scenes(SET, DENSITY, true, 0..3));
        assert_eq!(scenes.parked_state_in_scenes(SET, DENSITY, 0..3), BankState::Off);
        assert!(scenes.parked_in_every_scene(SET, DENSITY));
        assert!(scenes.forget_parked(SET, &[DENSITY]));
        assert!(!scenes.instance_parked_in_scene(0, DENSITY));
    }

    #[test]
    fn parked_instances_round_trip_the_wire_and_old_files_park_nothing() {
        let mut scenes = scenes(3);
        assert!(scenes.parked_to_wire(SET).is_empty(), "nothing parked writes nothing");
        scenes.park_outside(SET, DENSITY, 0..1);
        let wire = scenes.parked_to_wire(SET);
        assert_eq!(wire, vec![vec![], vec![DENSITY], vec![DENSITY]]);
        let mut loaded = self::scenes(3);
        loaded.parked_from_wire(SET, &wire);
        assert!(loaded.instance_parked_in_scene(2, DENSITY));
        loaded.parked_from_wire(SET, &[]);
        assert!((0..3).all(|scene| !loaded.instance_parked_in_scene(scene, DENSITY)));
    }

    #[test]
    fn a_rack_owned_instance_is_never_parked_by_a_stale_entry() {
        let mut scenes = scenes(1);
        scenes.set_parked_in_scenes(SET, DENSITY, true, 0..1);
        assert_eq!(scenes.scene_parking(0).instances, vec![DENSITY]);
        scenes.instance_racks.insert(DENSITY, 4);
        assert!(scenes.scene_parking(0).instances.is_empty());
    }

    #[test]
    fn the_history_memento_restores_every_scene() {
        let mut scenes = scenes(2);
        let before = scenes.capture_parked_instances();
        scenes.park_outside(SET, DENSITY, 1..2);
        assert!(scenes.instance_parked_in_scene(0, DENSITY));
        assert!(scenes.restore_parked_instances(&before));
        assert!(!scenes.instance_parked_in_scene(0, DENSITY));
    }
}
