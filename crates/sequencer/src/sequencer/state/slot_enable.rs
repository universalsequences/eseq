//! Scene-owned swap state (docs/rig-spec.md §3, eseq-ka5t.12).
//!
//! A pattern keeps **what plays** (notes, p-locks, each slot's params and a
//! sampler slot's sample); the scene keeps **who plays it**:
//! `Scene::slot_enable` holds, per track, the set of Instrument Rack slot ids
//! enabled in that scene. A track with no entry falls back to the pattern's
//! stored `RackSlotSnapshot::enabled` flags, which are only a creation /
//! legacy default: every funnel that creates a rack or appends a slot writes
//! entries for every scene.
//!
//! Resolution is a pure view. The pool patterns and the live mirror
//! (`PatternState::rack_tracks`) keep the STORED flags; whatever needs what
//! actually plays — the scheduler snapshot the audio thread reads (which
//! derives FX gates and solo mutes from it), the host fields the rack panel
//! reads — resolves the slot through the playing scene
//! (`ProjectScenes::current_scene`). Keeping the mirror unresolved means no
//! save-back can ever bake one scene's device set into a pattern another
//! scene shares.
//!
//! Slots are named by a stable id (`RackSlotSnapshot::id`), unique within the
//! track and shared by the slot in every pattern of the track. Ids are
//! assigned at the funnels (load, rack creation, append, replace keeps it);
//! readers never assign one.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::track_delete_remap::remap_optional_track_after_delete;
use super::*;

/// The enabled slot ids of one track in one scene (rig spec §3.2).
pub type SlotEnable = BTreeSet<u64>;

/// The project-file shape of every scene's `slot_enable`: outer index scene,
/// entries `(track index, enabled slot ids)`.
pub type SceneSlotEnableWire = Vec<Vec<(usize, Vec<u64>)>>;

/// The per-scene "who plays" state a new scene inherits (rig spec §3.3).
/// Every field a scene owns for swapping lives here, so a scene-owned field
/// added later (eseq-ka5t.13's `parked_kits`) is inherited by every creation
/// path by adding it to `of` / `apply_to` alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SceneSwapState {
    pub slot_enable: BTreeMap<usize, SlotEnable>,
    /// The kits switched off (eseq-ka5t.13, `super::kit_parking`).
    pub parked_kits: super::ParkedKits,
    /// The project-owned instances switched off (docs/rig-spec.md §10,
    /// `super::instance_parking`).
    pub parked_instances: super::ParkedInstances,
}

impl SceneSwapState {
    pub fn of(scene: &Scene) -> Self {
        Self {
            slot_enable: scene.slot_enable.clone(),
            parked_kits: scene.parked_kits.clone(),
            parked_instances: scene.parked_instances.clone(),
        }
    }

    pub fn apply_to(self, scene: &mut Scene) {
        scene.slot_enable = self.slot_enable;
        scene.parked_kits = self.parked_kits;
        scene.parked_instances = self.parked_instances;
    }
}

/// Where a new scene takes its swap state from (rig spec §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSwapInheritance {
    /// `clone-scene!` / `copy-scene-to-new-bank!`: the scene it is made from.
    Source(usize),
    /// A blank `+` scene: the scene before it in its bank or, when the bank
    /// is empty, the last scene of the bank before it. Banks are contiguous,
    /// so both are the scene right before the insertion point.
    BankNeighbour,
}

/// Whether `slot` plays under `entry`, a scene's `slot_enable` for its track
/// (`None`: the track has no entry, the pattern's stored flag decides).
pub fn resolve_slot_enabled(slot: &RackSlotSnapshot, entry: Option<&SlotEnable>) -> bool {
    entry.map_or(slot.enabled, |set| set.contains(&slot.id))
}

/// Overwrite every slot's `enabled` with what `entry` says plays (rig spec
/// §3.2 resolution). No entry leaves the stored flags as they are.
pub(crate) fn resolve_rack_slot_enable(rack: &mut RackTrackSnapshot, entry: Option<&SlotEnable>) {
    let Some(set) = entry else {
        return;
    };
    for slot in &mut rack.slots {
        slot.enabled = set.contains(&slot.id);
    }
}

impl RackTrackSnapshot {
    /// The largest slot id this rack holds (0: none assigned).
    pub fn max_slot_id(&self) -> u64 {
        self.slots.iter().map(|slot| slot.id).max().unwrap_or(0)
    }
}

/// Give every id-0 slot of `slots` an id: the one `reference` holds at its
/// position when this rack does not use it yet, else a fresh one from `next`.
/// A duplicate id (a damaged file) keeps its first holder.
pub(super) fn assign_slot_ids(
    slots: &mut [RackSlotSnapshot],
    reference: &[u64],
    next: &mut u64,
) -> bool {
    let mut used: BTreeSet<u64> = BTreeSet::new();
    for slot in slots.iter_mut() {
        if slot.id != 0 && !used.insert(slot.id) {
            slot.id = 0;
        }
    }
    let mut changed = false;
    for (position, slot) in slots.iter_mut().enumerate() {
        if slot.id != 0 {
            continue;
        }
        let id = match reference.get(position).copied() {
            Some(id) if id != 0 && !used.contains(&id) => id,
            _ => {
                *next += 1;
                *next
            }
        };
        used.insert(id);
        slot.id = id;
        changed = true;
    }
    changed
}

impl ProjectScenes {
    /// Scene `scene`'s `slot_enable` entry for `track`.
    pub fn scene_slot_enable(&self, scene: usize, track: usize) -> Option<&SlotEnable> {
        self.scenes.get(scene)?.slot_enable.get(&track)
    }

    /// The playing scene's entry for every track (`None`: no entry).
    pub fn current_scene_slot_enables(&self) -> Vec<Option<SlotEnable>> {
        (0..self.track_pools.len())
            .map(|track| self.scene_slot_enable(self.current_scene, track).cloned())
            .collect()
    }

    /// Resolve a track's pattern data against scene `scene` (rig spec §3.2):
    /// the rack slots' `enabled` become what the scene plays.
    pub fn resolve_scene_track_pattern(
        &self,
        scene: usize,
        track: usize,
        data: &mut TrackPatternData,
    ) {
        if let Some(rack) = data.rack_track.as_mut() {
            resolve_rack_slot_enable(rack, self.scene_slot_enable(scene, track));
        }
    }

    /// Every rack a track's pool holds (one per Patch entity; take chunks
    /// share theirs), keyed by entity so iteration is deterministic.
    fn track_racks(&self, track: usize) -> BTreeMap<PatchId, &RackTrackSnapshot> {
        self.track_pools
            .get(track)
            .map(|pool| {
                pool.sounds
                    .patches
                    .iter()
                    .filter_map(|(id, patch)| Some((*id, patch.rack_track.as_ref()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The largest slot id any pattern of `track` holds (0: none assigned).
    pub fn max_rack_slot_id(&self, track: usize) -> u64 {
        self.track_racks(track)
            .values()
            .map(|rack| rack.max_slot_id())
            .max()
            .unwrap_or(0)
    }

    /// Assign stable ids to every slot of `track` that has none (`id == 0`),
    /// in every pattern and in `live` (the live mirror's rack, when given).
    /// Slots that were matched by position keep matching: a slot takes the
    /// id the other patterns hold at its position, so a legacy track gets one
    /// id per row. Returns whether anything changed.
    pub fn normalize_rack_slot_ids(
        &mut self,
        track: usize,
        live: Option<&mut RackTrackSnapshot>,
    ) -> bool {
        let mut next = self
            .max_rack_slot_id(track)
            .max(live.as_ref().map_or(0, |rack| rack.max_slot_id()));
        // The id each position already holds somewhere (first holder wins),
        // live first: the live rack is what the user is looking at.
        let mut reference: Vec<u64> = Vec::new();
        let mut note = |slots: &[RackSlotSnapshot]| {
            for (position, slot) in slots.iter().enumerate() {
                if reference.len() <= position {
                    reference.resize(position + 1, 0);
                }
                if reference[position] == 0 {
                    reference[position] = slot.id;
                }
            }
        };
        if let Some(rack) = live.as_ref() {
            note(&rack.slots);
        }
        for rack in self.track_racks(track).values() {
            note(&rack.slots);
        }
        // Positions nobody has an id for yet get one fresh id shared by all.
        for id in reference.iter_mut().filter(|id| **id == 0) {
            next += 1;
            *id = next;
        }
        let mut changed = false;
        if let Some(rack) = live {
            changed |= assign_slot_ids(&mut rack.slots, &reference, &mut next);
        }
        let Some(pool) = self.track_pools.get_mut(track) else {
            return changed;
        };
        let needs: Vec<PatchId> = pool
            .sounds
            .patches
            .iter()
            .filter(|(_, patch)| {
                patch.rack_track.as_ref().is_some_and(|rack| {
                    let mut seen = BTreeSet::new();
                    rack.slots
                        .iter()
                        .any(|slot| slot.id == 0 || !seen.insert(slot.id))
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in needs {
            if let Some(rack) = pool
                .sounds
                .patches
                .get_mut(&id)
                .map(Arc::make_mut)
                .and_then(|patch| patch.rack_track.as_mut())
            {
                changed |= assign_slot_ids(&mut rack.slots, &reference, &mut next);
            }
        }
        changed
    }

    /// The rack scene `scene` plays on `track` as stored: its composed cell's
    /// rack, else (a bare cell) the track sound's.
    fn scene_stored_rack(&self, scene: usize, track: usize) -> Option<&RackTrackSnapshot> {
        let pool = self.track_pools.get(track)?;
        let patch_of = |id: PatternId| {
            pool.refs(id)
                .and_then(|refs| pool.sounds.patches.get(&refs.patch))
                .and_then(|patch| patch.rack_track.as_ref())
        };
        self.composed_scene_cell(scene, track)
            .and_then(patch_of)
            .or_else(|| self.track_sound_pattern(track).and_then(patch_of))
    }

    /// The slot ids scene `scene` enables on `track` from its pattern's
    /// stored flags (`None`: the scene plays no rack there).
    fn stored_slot_enable(&self, scene: usize, track: usize) -> Option<SlotEnable> {
        self.scene_stored_rack(scene, track).map(|rack| {
            rack.slots
                .iter()
                .filter(|slot| slot.enabled)
                .map(|slot| slot.id)
                .collect()
        })
    }

    /// Whether scene `scene` plays slot `slot_id` of `track`: its entry, else
    /// its pattern's stored flag (`None`: no entry and no such slot there).
    pub fn slot_enabled_in_scene(&self, scene: usize, track: usize, slot_id: u64) -> Option<bool> {
        if let Some(set) = self.scene_slot_enable(scene, track) {
            return Some(set.contains(&slot_id));
        }
        self.scene_stored_rack(scene, track)?
            .slots
            .iter()
            .find(|slot| slot.id == slot_id)
            .map(|slot| slot.enabled)
    }

    /// Give every scene that plays a rack on `track` an explicit entry,
    /// written from what it plays now, so a write to some scenes never
    /// leaves others falling back to flags a shared pattern carries.
    pub fn materialize_track_slot_enable(&mut self, track: usize) -> bool {
        let mut changed = false;
        for scene in 0..self.scenes.len() {
            if self.scenes[scene].slot_enable.contains_key(&track) {
                continue;
            }
            if let Some(set) = self.stored_slot_enable(scene, track) {
                self.scenes[scene].slot_enable.insert(track, set);
                changed = true;
            }
        }
        changed
    }

    /// Switch slot `slot_id` of `track` per scene: `decide(scene index,
    /// scene id)` says on (`Some(true)`), off (`Some(false)`) or untouched
    /// (`None`). Every scene is materialized once first, so untouched scenes
    /// keep playing exactly what they played. Returns whether anything
    /// changed. The drop-to-swap write primitive (eseq-ka5t.9).
    pub fn set_rack_slot_enabled_where(
        &mut self,
        track: usize,
        slot_id: u64,
        mut decide: impl FnMut(usize, SceneId) -> Option<bool>,
    ) -> bool {
        if track >= self.track_pools.len() || slot_id == 0 {
            return false;
        }
        let mut changed = self.materialize_track_slot_enable(track);
        for (index, scene) in self.scenes.iter_mut().enumerate() {
            let Some(enabled) = decide(index, scene.id) else {
                continue;
            };
            let set = scene.slot_enable.entry(track).or_default();
            changed |= if enabled {
                set.insert(slot_id)
            } else {
                set.remove(&slot_id)
            };
        }
        changed
    }

    /// `set_rack_slot_enabled_where` for every scene of `scenes`.
    pub fn set_rack_slot_enabled_in_scenes(
        &mut self,
        track: usize,
        slot_id: u64,
        enabled: bool,
        scenes: Range<usize>,
    ) -> bool {
        self.set_rack_slot_enabled_where(track, slot_id, |scene, _| {
            scenes.contains(&scene).then_some(enabled)
        })
    }

    /// Whether `set_rack_slot_enabled_in_scenes` would change what any of
    /// `scenes` plays (a dry run: nothing is written).
    pub fn rack_slot_enable_would_change(
        &self,
        track: usize,
        slot_id: u64,
        enabled: bool,
        scenes: Range<usize>,
    ) -> bool {
        scenes.into_iter().any(|scene| {
            self.slot_enabled_in_scene(scene, track, slot_id)
                .is_some_and(|on| on != enabled)
        })
    }

    /// The scenes that enable slot `slot_id` of `track` (entry, else stored
    /// flags), in scene order.
    pub(crate) fn scenes_enabling_rack_slot(&self, track: usize, slot_id: u64) -> Vec<usize> {
        (0..self.scenes.len())
            .filter(|scene| self.slot_enabled_in_scene(*scene, track, slot_id) == Some(true))
            .collect()
    }

    /// The banks holding a scene that enables slot `slot_id` of `track`, in
    /// bank order ("Kept Piano for banks A, B", rig spec §4).
    pub(crate) fn banks_enabling_rack_slot(&self, track: usize, slot_id: u64) -> Vec<SceneBankId> {
        let mut offset = 0;
        let mut banks = Vec::new();
        for bank in &self.banks {
            let range = offset..offset + bank.len;
            offset += bank.len;
            if range
                .into_iter()
                .any(|scene| self.slot_enabled_in_scene(scene, track, slot_id) == Some(true))
            {
                banks.push(bank.id);
            }
        }
        banks
    }

    /// Whether any scene outside `range` enables slot `slot_id` of `track`
    /// (rig spec §4 "replace for this bank": overwrite in place only when
    /// this is false).
    pub(crate) fn rack_slot_enabled_outside(
        &self,
        track: usize,
        slot_id: u64,
        range: Range<usize>,
    ) -> bool {
        (0..self.scenes.len())
            .filter(|scene| !range.contains(scene))
            .any(|scene| self.slot_enabled_in_scene(scene, track, slot_id) == Some(true))
    }

    /// Drop a deleted slot's id from every scene, so a later slot that reuses
    /// the number does not inherit its scenes.
    pub fn forget_rack_slot_id(&mut self, track: usize, slot_id: u64) {
        for scene in &mut self.scenes {
            if let Some(set) = scene.slot_enable.get_mut(&track) {
                set.remove(&slot_id);
            }
        }
    }

    /// Track `track` is gone: drop its entries and shift higher tracks down.
    pub(super) fn remove_track_slot_enable(&mut self, track: usize) {
        for scene in &mut self.scenes {
            scene.slot_enable = std::mem::take(&mut scene.slot_enable)
                .into_iter()
                .filter_map(|(owner, set)| {
                    Some((remap_optional_track_after_delete(owner, track)?, set))
                })
                .collect();
        }
    }

    /// Track `from` moved to index `to` (the other tracks shift to make
    /// room, like `Vec::remove(from)` + `Vec::insert(to)`).
    pub(super) fn move_track_slot_enable(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let remap = |track: usize| {
            if track == from {
                to
            } else if from < to && track > from && track <= to {
                track - 1
            } else if to < from && track >= to && track < from {
                track + 1
            } else {
                track
            }
        };
        for scene in &mut self.scenes {
            scene.slot_enable = std::mem::take(&mut scene.slot_enable)
                .into_iter()
                .map(|(track, set)| (remap(track), set))
                .collect();
        }
    }

    /// `track`'s entries, keyed by scene identity (undo state survives a
    /// scene reorder between capture and replay).
    pub(crate) fn track_slot_enable(&self, track: usize) -> Vec<(SceneId, SlotEnable)> {
        self.scenes
            .iter()
            .filter_map(|scene| Some((scene.id, scene.slot_enable.get(&track)?.clone())))
            .collect()
    }

    /// Put back `track`'s entries exactly as `track_slot_enable` captured
    /// them: a scene not listed loses its entry.
    pub(crate) fn restore_track_slot_enable(
        &mut self,
        track: usize,
        entries: &[(SceneId, SlotEnable)],
    ) {
        for scene in &mut self.scenes {
            match entries.iter().find(|(id, _)| *id == scene.id) {
                Some((_, set)) => {
                    scene.slot_enable.insert(track, set.clone());
                }
                None => {
                    scene.slot_enable.remove(&track);
                }
            }
        }
    }

    /// The swap state a scene inserted at `insert_at` inherits (rig spec
    /// §3.3). `insert_at` is the new scene's index before insertion.
    pub(super) fn inherited_scene_swap_state(
        &self,
        inheritance: SceneSwapInheritance,
        insert_at: usize,
    ) -> SceneSwapState {
        let source = match inheritance {
            SceneSwapInheritance::Source(scene) => Some(scene),
            SceneSwapInheritance::BankNeighbour => insert_at.checked_sub(1),
        };
        source
            .and_then(|scene| self.scenes.get(scene))
            .map(SceneSwapState::of)
            .unwrap_or_default()
    }

    /// Every scene's `slot_enable` in the project-file shape, tracks below
    /// `track_count` only.
    pub fn slot_enable_to_wire(&self, track_count: usize) -> SceneSlotEnableWire {
        self.scenes
            .iter()
            .map(|scene| {
                scene
                    .slot_enable
                    .iter()
                    .filter(|(track, _)| **track < track_count)
                    .map(|(track, set)| (*track, set.iter().copied().collect()))
                    .collect()
            })
            .collect()
    }

    /// Install `slot_enable_to_wire`'s shape, scene by scene; entries for
    /// tracks the project does not have are dropped.
    pub fn slot_enable_from_wire(&mut self, wire: &[Vec<(usize, Vec<u64>)>]) {
        let track_count = self.track_pools.len();
        for (scene, entries) in self.scenes.iter_mut().zip(wire) {
            scene.slot_enable = entries
                .iter()
                .filter(|(track, _)| *track < track_count)
                .map(|(track, ids)| (*track, ids.iter().copied().collect()))
                .collect();
        }
    }

    /// Legacy migration (rig spec §3.4), for projects saved before slots had
    /// stable ids and scenes owned the device set. Per track with slots:
    ///
    /// 1. ids are assigned by position (`normalize_rack_slot_ids`), so one
    ///    row is one slot in every pattern;
    /// 2. a pattern missing a row (a shorter rack) gets it as a disabled copy
    ///    from a pattern that has it;
    /// 3. every scene gets an explicit `slot_enable` with exactly the slots
    ///    its pattern stored as enabled.
    ///
    /// Rows are never split. A sampler slot's sample is pattern content (a
    /// kit pad's per-clip sample lives in the slot snapshot), not device
    /// identity. A row can hold a sampler in one pattern and an instrument
    /// in another only in a session (a per-pattern replace); a saved file
    /// cannot carry it, because the loader rebinds every pattern's slot to
    /// the track's one built device (`rebind_project_rack_tracks_to_graph`:
    /// instrument type and engine come from the graph), and this runs after
    /// that rebind. So on load a row is always one device kind.
    ///
    /// Returns the number of tracks that got scene state.
    pub fn migrate_legacy_rack_slot_scene_state(&mut self) -> usize {
        (0..self.track_pools.len())
            .filter(|track| self.migrate_legacy_track_slots(*track))
            .count()
    }

    fn migrate_legacy_track_slots(&mut self, track: usize) -> bool {
        self.normalize_rack_slot_ids(track, None);
        let racks = self.track_racks(track);
        if racks.is_empty() {
            return false;
        }
        let slot_count = racks
            .values()
            .map(|rack| rack.slots.len())
            .max()
            .unwrap_or(0);
        let fill: Vec<Option<RackSlotSnapshot>> = (0..slot_count)
            .map(|position| {
                racks.values().find_map(|rack| {
                    let mut slot = rack.slots.get(position)?.clone();
                    slot.enabled = false;
                    Some(slot)
                })
            })
            .collect();
        debug_assert!(
            racks
                .values()
                .all(|rack| rack.slots.iter().zip(&fill).all(|(slot, fill)| {
                    fill.as_ref()
                        .is_none_or(|fill| fill.instrument_type == slot.instrument_type)
                })),
            "track {track}: a loaded row holds two device kinds; the load rebind forbids it"
        );
        let short: Vec<PatchId> = racks
            .iter()
            .filter(|(_, rack)| rack.slots.len() < slot_count)
            .map(|(id, _)| *id)
            .collect();
        drop(racks);
        let pool = &mut self.track_pools[track];
        for patch_id in short {
            let Some(rack) = pool
                .sounds
                .patches
                .get_mut(&patch_id)
                .map(Arc::make_mut)
                .and_then(|patch| patch.rack_track.as_mut())
            else {
                continue;
            };
            for slot in fill.iter().skip(rack.slots.len()).flatten() {
                rack.slots.push(slot.clone());
            }
        }
        for scene in 0..self.scenes.len() {
            if let Some(set) = self.stored_slot_enable(scene, track) {
                self.scenes[scene].slot_enable.insert(track, set);
            }
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn normalize_all_rack_slot_ids(&mut self) -> bool {
        let mut changed = false;
        for track in 0..self.track_pools.len() {
            changed |= self.normalize_rack_slot_ids(track, None);
        }
        changed
    }
}

impl SequencerState {
    /// The playing scene: `ProjectScenes::current_scene`, the one index every
    /// scene-owned read (graph overrides, process chain, slot enables) uses.
    pub fn playing_scene(&self) -> usize {
        self.pattern.scenes.lock().unwrap().current_scene
    }

    /// Advances on every write to the project scenes (slot enables and the
    /// playing scene included), for readers that cache scene-resolved views.
    pub fn scenes_revision(&self) -> u64 {
        self.pattern.scenes.revision()
    }

    /// The project process chain, every track's slot enables and the kits
    /// switched off (eseq-ka5t.13) of the playing scene, under one scenes
    /// lock (scheduler capture; read before the rack lock: scene launches
    /// lock scenes then racks).
    pub(crate) fn capture_scene_track_inputs(
        &self,
    ) -> (crate::process::TrackProcessChain, Vec<Option<SlotEnable>>, super::SceneParking) {
        let scenes = self.pattern.scenes.lock().unwrap();
        (
            scenes.current_project_process_chain(),
            scenes.current_scene_slot_enables(),
            scenes.current_scene_parking(),
        )
    }

    /// The playing scene's `slot_enable` entry for `track`.
    pub fn current_scene_slot_enable(&self, track: usize) -> Option<SlotEnable> {
        let scenes = self.pattern.scenes.lock().unwrap();
        scenes
            .scene_slot_enable(scenes.current_scene, track)
            .cloned()
    }

    /// Read the live rack of `track` together with the playing scene's
    /// `slot_enable` entry for it, without cloning either. Locks scenes then
    /// racks (the launch order); the caller holds neither.
    pub fn with_resolved_live_rack<R>(
        &self,
        track: usize,
        read: impl FnOnce(&RackTrackSnapshot, Option<&SlotEnable>) -> R,
    ) -> Option<R> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let entry = scenes.scene_slot_enable(scenes.current_scene, track);
        let racks = self.pattern.rack_tracks.lock().unwrap();
        let rack = racks.get(track)?.as_ref()?;
        Some(read(rack, entry))
    }

    /// The live rack of `track` with each slot's `enabled` resolved through
    /// the playing scene: what the audio thread plays and the panel shows.
    pub fn resolved_live_rack_track(&self, track: usize) -> Option<RackTrackSnapshot> {
        self.with_resolved_live_rack(track, |rack, entry| {
            let mut rack = rack.clone();
            resolve_rack_slot_enable(&mut rack, entry);
            rack
        })
    }

    /// Live slot `slot_idx` of `track`, `enabled` resolved (one clone).
    pub fn resolved_live_rack_slot(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> Option<RackSlotSnapshot> {
        self.with_resolved_live_rack(track, |rack, entry| {
            let mut slot = rack.slots.get(slot_idx)?.clone();
            slot.enabled = resolve_slot_enabled(&slot, entry);
            Some(slot)
        })
        .flatten()
    }

    /// Whether live slot `slot_idx` of `track` plays in the playing scene.
    pub fn live_rack_slot_enabled(&self, track: usize, slot_idx: usize) -> Option<bool> {
        self.with_resolved_live_rack(track, |rack, entry| {
            rack.slots
                .get(slot_idx)
                .map(|slot| resolve_slot_enabled(slot, entry))
        })
        .flatten()
    }

    /// The stable id of live slot `slot_idx` of `track`. Pure: the funnels
    /// assign ids, so an unassigned (0) id is a bug and reads as `None`.
    pub fn live_rack_slot_id(&self, track: usize, slot_idx: usize) -> Option<u64> {
        let id = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)?
            .as_ref()?
            .slots
            .get(slot_idx)?
            .id;
        debug_assert_ne!(id, 0, "track {track} slot {slot_idx} has no stable id");
        (id != 0).then_some(id)
    }

    /// Switch slot `slot_id` of `track` on or off in every scene of
    /// `scenes`. Unrecorded and unpublished: the caller records the step
    /// and publishes the scheduler snapshot once.
    pub fn set_rack_slot_enabled_in_scenes(
        &self,
        track: usize,
        slot_id: u64,
        enabled: bool,
        scenes: Range<usize>,
    ) -> bool {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .set_rack_slot_enabled_in_scenes(track, slot_id, enabled, scenes)
    }

    /// The scenes that enable slot `slot_id` of `track` (eseq-ka5t.9).
    pub(crate) fn scenes_enabling_rack_slot(&self, track: usize, slot_id: u64) -> Vec<usize> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes_enabling_rack_slot(track, slot_id)
    }

    /// The banks holding a scene that enables slot `slot_id` of `track`.
    pub(crate) fn banks_enabling_rack_slot(&self, track: usize, slot_id: u64) -> Vec<SceneBankId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .banks_enabling_rack_slot(track, slot_id)
    }

    /// Whether a scene outside `range` enables slot `slot_id` of `track`.
    pub(crate) fn rack_slot_enabled_outside(
        &self,
        track: usize,
        slot_id: u64,
        range: Range<usize>,
    ) -> bool {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .rack_slot_enabled_outside(track, slot_id, range)
    }

    /// Assign ids to every slot of every track (pools and live mirror) that
    /// has none. The load funnel.
    pub fn normalize_rack_slot_ids(&self) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let mut changed = false;
        for track in 0..scenes.track_pools.len() {
            let live = racks.get_mut(track).and_then(Option::as_mut);
            changed |= scenes.normalize_rack_slot_ids(track, live);
        }
        changed
    }

    /// Legacy load migration (rig spec §3.4,
    /// `ProjectScenes::migrate_legacy_rack_slot_scene_state`). Run on the
    /// installed scenes before the current scene is restored into the live
    /// mirror. Returns the number of tracks that got scene state.
    pub fn migrate_legacy_rack_slot_scene_state(&self) -> usize {
        let migrated = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .migrate_legacy_rack_slot_scene_state();
        self.normalize_rack_slot_ids();
        migrated
    }

    /// `ProjectScenes::restore_track_slot_enable` under the scenes lock.
    pub(crate) fn restore_track_slot_enable(
        &self,
        track: usize,
        entries: &[(SceneId, SlotEnable)],
    ) {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .restore_track_slot_enable(track, entries);
    }

    /// Install per-scene `slot_enable` read from a project file.
    pub fn install_scene_slot_enable(&self, wire: &[Vec<(usize, Vec<u64>)>]) {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .slot_enable_from_wire(wire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(instrument_type: InstrumentType) -> RackSlotSnapshot {
        RackSlotSnapshot {
            id: 0,
            instrument_type,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony: 2,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_empty(),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: None,
        }
    }

    fn sampler(name: &str) -> RackSlotSnapshot {
        let mut slot = slot(InstrumentType::Sampler);
        slot.sample_id = Some((-1, name.to_string(), 44_100));
        slot
    }

    /// A custom slot holding `instrument` on its own engine (Digi Syn and
    /// Digi FM are different instruments, so different engines).
    fn custom(instrument: &str, engine: usize) -> RackSlotSnapshot {
        let mut slot = slot(InstrumentType::Custom);
        slot.track_sound_state = TrackSoundState {
            engine_id: Some(engine),
            loaded_preset: Some(instrument.to_string()),
            dirty: false,
        };
        slot
    }

    fn rack(slots: Vec<RackSlotSnapshot>) -> RackTrackSnapshot {
        RackTrackSnapshot::new(slots, default_rack_macros())
    }

    /// One rack track, one private pattern per scene, each holding `racks[i]`
    /// as stored before this change (ids 0).
    fn legacy_state(racks: Vec<RackTrackSnapshot>) -> SequencerState {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshots = racks
            .into_iter()
            .map(|rack| {
                let mut snapshot = PatternSnapshot::new_default(1, &[]);
                snapshot.instrument_types[0] = InstrumentType::Rack;
                snapshot.rack_tracks[0] = Some(rack);
                snapshot
            })
            .collect();
        state.replace_pattern_repository(snapshots, 0);
        state.restore_current_pattern_from_repository().unwrap();
        state
    }

    fn launch(state: &SequencerState, scene: usize) {
        state
            .launch_scene(
                scene,
                1,
                &[-1],
                &[44_100],
                &[String::from("rack")],
                &[InstrumentType::Rack],
            )
            .expect("scene launches");
    }

    /// What the audio thread plays: the published scheduler snapshot's rack.
    fn published(state: &SequencerState) -> RackTrackSnapshot {
        state.latest_scheduler_snapshot().tracks[0]
            .rack_track
            .clone()
            .expect("a rack track")
    }

    /// What a playing slot sounds like in a test: its sample, or its engine.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum RackSlotDevice {
        Sampler(String),
        Engine(Option<usize>),
    }

    fn rack_slot_device(slot: &RackSlotSnapshot) -> RackSlotDevice {
        match &slot.sample_id {
            Some((_, name, _)) => RackSlotDevice::Sampler(name.clone()),
            None => RackSlotDevice::Engine(slot.track_sound_state.engine_id),
        }
    }

    /// The device of every slot that plays (what triggers reach).
    fn playing_devices(rack: &RackTrackSnapshot) -> Vec<RackSlotDevice> {
        let mut devices: Vec<_> = rack
            .slots
            .iter()
            .filter(|slot| slot.enabled)
            .map(rack_slot_device)
            .collect();
        devices.sort();
        devices
    }

    /// Every scene's playing devices as `launch` makes them audible.
    fn scene_devices(state: &SequencerState) -> Vec<Vec<RackSlotDevice>> {
        (0..state.scene_count())
            .map(|scene| {
                launch(state, scene);
                playing_devices(&published(state))
            })
            .collect()
    }

    #[test]
    fn two_scenes_sharing_a_pattern_play_their_own_slots() {
        let state = legacy_state(vec![rack(Vec::new()), rack(Vec::new())]);
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            rack(vec![sampler("piano"), sampler("synth")]),
        );
        let ids: Vec<u64> = state
            .live_rack_track_snapshot(0)
            .unwrap()
            .slots
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![1, 2], "a new rack's slots get ids by position");
        state.with_scenes_mut(|scenes| {
            let shared = scenes.scenes[0].cells[0].expect("scene 0 pattern");
            assert!(scenes.set_cell(1, 0, shared));
            scenes.scenes[0]
                .slot_enable
                .insert(0, SlotEnable::from([1]));
            scenes.scenes[1]
                .slot_enable
                .insert(0, SlotEnable::from([2]));
        });

        launch(&state, 0);
        assert_eq!(
            playing_devices(&published(&state)),
            vec![RackSlotDevice::Sampler("piano".into())]
        );
        assert_eq!(state.live_rack_slot_enabled(0, 0), Some(true));
        assert_eq!(state.live_rack_slot_enabled(0, 1), Some(false));
        launch(&state, 1);
        assert_eq!(
            playing_devices(&published(&state)),
            vec![RackSlotDevice::Sampler("synth".into())],
            "the scene, not the shared pattern, picks the slot"
        );
        assert_eq!(state.live_rack_slot_enabled(0, 1), Some(true));
        // The pattern's stored flags are untouched: both still say "on".
        let stored = state.live_rack_track_snapshot(0).unwrap();
        assert!(stored.slots.iter().all(|slot| slot.enabled));
        launch(&state, 0);
        assert_eq!(
            playing_devices(&published(&state)),
            vec![RackSlotDevice::Sampler("piano".into())]
        );
    }

    #[test]
    fn a_per_track_launch_keeps_the_playing_scenes_slots() {
        let state = legacy_state(vec![rack(Vec::new()), rack(Vec::new())]);
        state.restore_current_pattern_from_repository().unwrap();
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            rack(vec![sampler("piano"), sampler("synth")]),
        );
        let other = state.with_scenes_mut(|scenes| {
            scenes.scenes[0]
                .slot_enable
                .insert(0, SlotEnable::from([1]));
            scenes.scenes[1]
                .slot_enable
                .insert(0, SlotEnable::from([2]));
            scenes.scenes[1].cells[0].unwrap()
        });
        launch(&state, 0);
        assert!(state.launch_track_pattern(
            0,
            other,
            1,
            &[-1],
            &[44_100],
            &[String::from("rack")],
            &[InstrumentType::Rack],
        ));
        assert_eq!(
            playing_devices(&published(&state)),
            vec![RackSlotDevice::Sampler("piano".into())],
            "launching scene 1's clip on its own keeps scene 0's device set"
        );
    }

    #[test]
    fn legacy_migration_keeps_rows_and_what_every_scene_plays() {
        // Three scenes: row 1's sample swapped per pattern (pattern content,
        // a kit pad's per-clip sample), row 2 parked in one pattern, and a
        // shorter rack missing row 2.
        let mut parked = sampler("snare");
        parked.enabled = false;
        let state = legacy_state(vec![
            rack(vec![sampler("kick-a"), sampler("snare")]),
            rack(vec![sampler("kick-b"), parked]),
            rack(vec![sampler("kick-a")]),
        ]);
        let before = scene_devices(&state);
        assert_eq!(before[1], vec![RackSlotDevice::Sampler("kick-b".into())]);

        assert_eq!(state.migrate_legacy_rack_slot_scene_state(), 1);
        state.restore_current_pattern_from_repository().unwrap();
        assert_eq!(
            scene_devices(&state),
            before,
            "each scene plays its own pattern's sample"
        );
        state.with_project_scenes(|scenes| {
            for rack in scenes.track_racks(0).values() {
                let ids: Vec<u64> = rack.slots.iter().map(|slot| slot.id).collect();
                assert_eq!(ids, vec![1, 2], "no split; every pattern holds every row");
            }
            assert_eq!(
                scenes.scene_slot_enable(0, 0),
                Some(&SlotEnable::from([1, 2]))
            );
            assert_eq!(scenes.scene_slot_enable(1, 0), Some(&SlotEnable::from([1])));
            assert_eq!(scenes.scene_slot_enable(2, 0), Some(&SlotEnable::from([1])));
        });
    }

    #[test]
    fn bank_api_reports_and_writes_scene_ranges() {
        let state = legacy_state(vec![rack(vec![sampler("piano")]); 4]);
        state.with_scenes_mut(|scenes| {
            scenes.normalize_all_rack_slot_ids();
            let bank = scenes.create_scene_bank().unwrap();
            // Move scenes 2 and 3 into bank B (each move re-indexes A's tail).
            for _ in 0..2 {
                scenes.move_scene_to_bank(2, bank).unwrap();
            }
        });
        let banks = state.scene_banks();
        assert_eq!(banks.len(), 2);
        let (a, b) = (banks[0].id, banks[1].id);
        assert_eq!(state.scene_bank_range(a), Some(0..2));
        assert_eq!(state.scene_bank_range(b), Some(2..4));
        state.restore_current_pattern_from_repository().unwrap();

        assert!(state.set_rack_slot_enabled_in_scenes(0, 1, false, 2..4));
        assert_eq!(state.scenes_enabling_rack_slot(0, 1), vec![0, 1]);
        assert_eq!(state.banks_enabling_rack_slot(0, 1), vec![a]);
        assert!(!state.rack_slot_enabled_outside(0, 1, 0..2));
        assert!(state.rack_slot_enabled_outside(0, 1, 2..4));
        assert!(
            !state.set_rack_slot_enabled_in_scenes(0, 1, false, 2..4),
            "a repeated write changes nothing"
        );
        state.with_project_scenes(|scenes| {
            assert!(!scenes.rack_slot_enable_would_change(0, 1, false, 2..4));
            assert!(scenes.rack_slot_enable_would_change(0, 1, false, 0..4));
        });
    }

    #[test]
    fn deleting_a_track_shifts_slot_enable_with_it() {
        let state = legacy_state(vec![rack(vec![sampler("piano")]); 2]);
        state.with_scenes_mut(|scenes| {
            scenes.scenes[0]
                .slot_enable
                .insert(0, SlotEnable::from([1]));
            scenes.scenes[0]
                .slot_enable
                .insert(2, SlotEnable::from([5]));
            scenes.scenes[1]
                .slot_enable
                .insert(1, SlotEnable::from([7]));
            scenes.remove_track_slot_enable(1);
            assert_eq!(
                scenes.scenes[0].slot_enable,
                BTreeMap::from([(0, SlotEnable::from([1])), (1, SlotEnable::from([5]))])
            );
            assert!(
                scenes.scenes[1].slot_enable.is_empty(),
                "the deleted track's entry goes"
            );
            scenes.move_track_slot_enable(1, 0);
            assert_eq!(
                scenes.scenes[0].slot_enable,
                BTreeMap::from([(1, SlotEnable::from([1])), (0, SlotEnable::from([5]))])
            );
        });
    }

    #[test]
    fn new_scenes_inherit_swap_state() {
        let state = legacy_state(vec![rack(vec![sampler("piano"), sampler("synth")]); 2]);
        let (a, b) = state.with_scenes_mut(|scenes| {
            scenes.normalize_all_rack_slot_ids();
            let b = scenes.create_scene_bank().unwrap();
            scenes.move_scene_to_bank(1, b).unwrap();
            scenes.scenes[0]
                .slot_enable
                .insert(0, SlotEnable::from([1]));
            scenes.scenes[1]
                .slot_enable
                .insert(0, SlotEnable::from([2]));
            (scenes.scene_banks()[0].id, b)
        });
        state.restore_current_pattern_from_repository().unwrap();
        let args = (
            1,
            [-1],
            [44_100],
            [String::from("rack")],
            [InstrumentType::Rack],
        );
        // clone-scene!: from its source (scene 0, bank A).
        let clone = state
            .clone_pattern_in_scene_bank(
                a,
                SceneSwapInheritance::Source(0),
                args.0,
                &args.1,
                &args.2,
                &args.3,
                &args.4,
            )
            .unwrap();
        // `+` in bank B while scene `clone` (bank A) plays: bank B's own.
        let plus = state
            .clone_pattern_in_scene_bank(
                b,
                SceneSwapInheritance::BankNeighbour,
                args.0,
                &args.1,
                &args.2,
                &args.3,
                &args.4,
            )
            .unwrap();
        // copy-scene-to-new-bank!: an empty new bank, from its source.
        let c = state.create_scene_bank().unwrap();
        let copied = state
            .clone_pattern_in_scene_bank(
                c,
                SceneSwapInheritance::Source(clone),
                args.0,
                &args.1,
                &args.2,
                &args.3,
                &args.4,
            )
            .unwrap();
        // `+` into another empty bank: the last scene of the bank before.
        let d = state.create_scene_bank().unwrap();
        let plus_empty = state
            .clone_pattern_in_scene_bank(
                d,
                SceneSwapInheritance::BankNeighbour,
                args.0,
                &args.1,
                &args.2,
                &args.3,
                &args.4,
            )
            .unwrap();
        state.with_project_scenes(|scenes| {
            assert_eq!(
                scenes.scene_slot_enable(clone, 0),
                Some(&SlotEnable::from([1]))
            );
            assert_eq!(
                scenes.scene_slot_enable(plus, 0),
                Some(&SlotEnable::from([2]))
            );
            assert_eq!(
                scenes.scene_slot_enable(copied, 0),
                Some(&SlotEnable::from([1]))
            );
            assert_eq!(
                scenes.scene_slot_enable(plus_empty, 0),
                scenes.scene_slot_enable(copied, 0),
                "an empty bank's first scene sounds like the bank before it"
            );
        });
    }

    #[test]
    fn slot_ids_survive_the_wire_and_appended_slots_play_everywhere() {
        let state = legacy_state(vec![rack(vec![sampler("piano")]); 2]);
        state.restore_current_pattern_from_repository().unwrap();
        state.normalize_rack_slot_ids();
        state.with_scenes_mut(|scenes| {
            scenes.materialize_track_slot_enable(0);
        });
        state.append_rack_slot_for_all_pattern_snapshots(0, sampler("synth"));
        state.with_project_scenes(|scenes| {
            for scene in 0..2 {
                assert_eq!(
                    scenes.scene_slot_enable(scene, 0),
                    Some(&SlotEnable::from([1, 2]))
                );
            }
        });
        let saved = state.with_project_scenes(|scenes| scenes.slot_enable_to_wire(1));
        assert_eq!(saved, vec![vec![(0, vec![1, 2])], vec![(0, vec![1, 2])]]);
        let live = state.live_rack_track_snapshot(0).unwrap();
        let wire = crate::project::ProjectRackSlotPattern::from(live.slots[1].clone());
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["id"], 2);
        let back = RackSlotSnapshot::from(
            serde_json::from_value::<crate::project::ProjectRackSlotPattern>(json).unwrap(),
        );
        assert_eq!(back.id, 2);
        state.with_scenes_mut(|scenes| scenes.scenes[1].slot_enable.clear());
        state.install_scene_slot_enable(&[vec![(0, vec![2])], vec![(0, vec![1]), (9, vec![1])]]);
        state.with_project_scenes(|scenes| {
            assert_eq!(scenes.scene_slot_enable(0, 0), Some(&SlotEnable::from([2])));
            assert_eq!(scenes.scene_slot_enable(1, 0), Some(&SlotEnable::from([1])));
            assert!(
                scenes.scene_slot_enable(1, 9).is_none(),
                "out-of-range tracks are dropped"
            );
        });
    }
}
