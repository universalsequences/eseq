//! Position-shifting rack slot edits for drop-to-swap (docs/rig-spec.md §4,
//! eseq-ka5t.9): "replace for this bank" inserts a new slot directly after
//! the one it replaces, so everything that names a slot by its position on
//! the track is shifted here, in the pools and the live mirror together.

use std::ops::Range;

use super::super::*;

use crate::process::{port_rack_slots_shift, shift_port_rack_slots, RackSlotShift};

/// One process port set (`bindings`, `fanout`) of a chain slot or a per-track
/// project-lane override, for the shared rack-slot visitor.
trait PortSet {
    fn ports(
        &self,
    ) -> (
        &std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    );
    fn ports_mut(
        &mut self,
    ) -> (
        &mut std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &mut std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    );
}

impl PortSet for crate::process::TrackProcessSlot {
    fn ports(
        &self,
    ) -> (
        &std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    ) {
        (&self.bindings, &self.fanout)
    }
    fn ports_mut(
        &mut self,
    ) -> (
        &mut std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &mut std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    ) {
        (&mut self.bindings, &mut self.fanout)
    }
}

impl PortSet for crate::process::ProjectSlotOverride {
    fn ports(
        &self,
    ) -> (
        &std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    ) {
        (&self.bindings, &self.fanout)
    }
    fn ports_mut(
        &mut self,
    ) -> (
        &mut std::collections::BTreeMap<String, Option<crate::process::ParamTarget>>,
        &mut std::collections::BTreeMap<String, Vec<crate::process::ProcessPortFanout>>,
    ) {
        (&mut self.bindings, &mut self.fanout)
    }
}

/// Whether `shift` changes any rack slot target in `sets`.
fn sets_need_shift<'a, P: PortSet + 'a>(
    sets: impl IntoIterator<Item = &'a P>,
    shift: RackSlotShift,
) -> bool {
    sets.into_iter().any(|set| {
        let (bindings, fanout) = set.ports();
        port_rack_slots_shift(bindings, fanout, shift)
    })
}

/// Apply `shift` to every port set of `sets`.
fn shift_sets<'a, P: PortSet + 'a>(
    sets: impl IntoIterator<Item = &'a mut P>,
    shift: RackSlotShift,
) {
    for set in sets {
        let (bindings, fanout) = set.ports_mut();
        shift_port_rack_slots(bindings, fanout, shift);
    }
}

/// Copy the strip controls a replace keeps (`preserve_rack_slot_configuration`
/// minus identity and effects: an inserted slot gets its own, empty chain).
fn copy_strip_controls(slot: &mut RackSlotSnapshot, from: &RackSlotSnapshot) {
    slot.choke_group = from.choke_group;
    slot.instrument_base_note_offset = from.instrument_base_note_offset;
    slot.gain = from.gain;
    slot.pan = from.pan;
    slot.mute = from.mute;
    slot.max_polyphony = from.max_polyphony;
    slot.param_plocks = from.param_plocks.clone();
}

impl ProjectScenes {
    /// Whether scene `scene` gives `track` a cell (rig spec §4 "plain
    /// tracks": a scene with no cell for the track does not play it).
    pub fn scene_plays_track(&self, scene: usize, track: usize) -> bool {
        self.composed_scene_cell(scene, track).is_some()
    }

    /// For each of `tracks`, every slot id its racks hold mapped to a bitmask
    /// of the banks (bit = bank position) with a scene that enables it: what
    /// a slot row's bank letters show (rig spec §4 recall); 0 means no scene
    /// does (unused). One pass over the scenes. Banks past the 128th do not
    /// show.
    pub fn rack_slot_bank_masks(
        &self,
        tracks: &[usize],
    ) -> Vec<std::collections::BTreeMap<u64, u128>> {
        let mut masks: Vec<std::collections::BTreeMap<u64, u128>> = tracks
            .iter()
            .map(|track| {
                self.track_pools
                    .get(*track)
                    .map(|pool| {
                        pool.sounds
                            .patches
                            .values()
                            .filter_map(|patch| patch.rack_track.as_ref())
                            .flat_map(|rack| rack.slots.iter().map(|slot| (slot.id, 0)))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        for (bank_index, (_, range)) in self.scene_bank_ranges().enumerate().take(128) {
            let bit = 1_u128 << bank_index;
            for scene in range {
                for (track, slots) in tracks.iter().zip(&mut masks) {
                    match self.scene_slot_enable(scene, *track) {
                        Some(set) => {
                            for id in set {
                                if let Some(mask) = slots.get_mut(id) {
                                    *mask |= bit;
                                }
                            }
                        }
                        // No entry: the pattern's stored flags decide.
                        None => {
                            for (id, mask) in slots.iter_mut() {
                                if self.slot_enabled_in_scene(scene, *track, *id) == Some(true) {
                                    *mask |= bit;
                                }
                            }
                        }
                    }
                }
            }
        }
        masks
    }

    /// Whether a scene outside `range` plays `track` at all.
    pub fn track_played_outside(&self, track: usize, range: Range<usize>) -> bool {
        (0..self.scenes.len())
            .filter(|scene| !range.contains(scene))
            .any(|scene| self.scene_plays_track(scene, track))
    }
}

impl SequencerState {
    /// Insert `slot` at `position` of `track`'s rack in every pattern and the
    /// live mirror, under one fresh stable id (rig spec §3.2: a slot id is one
    /// device in every pattern). With `controls_from` (a slot position before
    /// the insert), each pattern's copy takes that pattern's strip controls of
    /// that slot. The new slot is stored disabled and every scene that plays
    /// the track gets an explicit entry first, so it plays nowhere until the
    /// caller enables it. Rack macro targets at or after `position` shift
    /// down with their device. Returns the new slot's id.
    pub fn insert_rack_slot_for_all_pattern_snapshots(
        &self,
        track: usize,
        position: usize,
        mut slot: RackSlotSnapshot,
        controls_from: Option<usize>,
    ) -> Option<u64> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let live = racks.get_mut(track)?.as_mut()?;
        if position > live.slots.len() {
            return None;
        }
        // Scenes resolve by entry from here on: the stored flag of the new
        // slot (off) never decides what an existing scene plays.
        scenes.materialize_track_slot_enable(track);
        let next = scenes.max_rack_slot_id(track).max(live.max_slot_id()) + 1;
        slot.id = next;
        slot.enabled = false;
        let insert_into = |rack: &mut RackTrackSnapshot| {
            let mut copy = slot.clone();
            if let Some(from) = controls_from.and_then(|from| rack.slots.get(from)) {
                copy_strip_controls(&mut copy, from);
            }
            let at = position.min(rack.slots.len());
            rack.slots.insert(at, copy);
            remap_rack_macro_slot_targets(&mut rack.macros, RackSlotShift::Inserted(at));
        };
        insert_into(live);
        let pool = scenes.track_pools.get_mut(track)?;
        for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
            if let Some(rack) = patch.rack_track.as_mut() {
                insert_into(rack);
            }
        }
        Some(next)
    }

    /// Process targets naming a rack slot of `track` by position follow
    /// `shift` (a slot inserted or deleted there): every pattern's process
    /// chain and project-lane overrides, and the live copies. A binding to a
    /// deleted slot falls back to its port default; a fan-out row to one is
    /// dropped. Project-layer slots' shared bindings are left alone — they
    /// apply to every track, so one track's edit cannot move them.
    pub(crate) fn shift_rack_slot_process_targets(&self, track: usize, shift: RackSlotShift) {
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if let Some(pool) = scenes.track_pools.get_mut(track) {
                for patch in pool.sounds.patches.values_mut() {
                    if sets_need_shift(&patch.process_chain.slots, shift) {
                        shift_sets(&mut Arc::make_mut(patch).process_chain.slots, shift);
                    }
                }
                for stored in pool.patterns.values_mut() {
                    if sets_need_shift(stored.seq.project_process_lane_overrides.values(), shift) {
                        shift_sets(
                            Arc::make_mut(stored)
                                .seq
                                .project_process_lane_overrides
                                .values_mut(),
                            shift,
                        );
                    }
                }
            }
        }
        if let Some(chain) = self.pattern.process_chains.lock().unwrap().get_mut(track) {
            shift_sets(&mut chain.slots, shift);
        }
        if let Some(overrides) = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get_mut(track)
        {
            shift_sets(overrides.values_mut(), shift);
        }
    }

    /// The positions and ids of `track`'s live slots that no scene enables
    /// (rig spec §4 "purge": unused), in slot order.
    pub fn unused_rack_slots(&self, track: usize) -> Vec<(usize, u64)> {
        let ids: Vec<u64> = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| rack.slots.iter().map(|slot| slot.id).collect())
            .unwrap_or_default();
        let scenes = self.pattern.scenes.lock().unwrap();
        ids.into_iter()
            .enumerate()
            .filter(|(_, id)| *id != 0 && scenes.scenes_enabling_rack_slot(track, *id).is_empty())
            .collect()
    }
}

impl SequencerState {
    /// `ProjectScenes::set_rack_slot_enabled_where` under the scenes lock:
    /// switch slot `slot_id` of `track` per scene index. Unrecorded and
    /// unpublished.
    pub fn set_rack_slot_enabled_where(
        &self,
        track: usize,
        slot_id: u64,
        decide: impl FnMut(usize, SceneId) -> Option<bool>,
    ) -> bool {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .set_rack_slot_enabled_where(track, slot_id, decide)
    }
}

impl SequencerState {
    /// Copy effect `effect_slot`'s values (params, bypass, p-locks, key
    /// locks, tensors, source references) of rack slot `from` onto the same
    /// effect slot of rack slot `to`, pattern by pattern and in the live
    /// mirror, rebound to the effect instance `to` already runs there
    /// (eseq-ka5t.9: an inserted "replace for this bank" slot plays through
    /// the same effects as the slot it replaces). Returns false when either
    /// slot or the target instance is missing.
    pub fn copy_rack_slot_effect_values(
        &self,
        track: usize,
        from: usize,
        to: usize,
        effect_slot: usize,
        descriptor: &crate::effects::EffectDescriptor,
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let Some(live) = racks.get_mut(track).and_then(Option::as_mut) else {
            return false;
        };
        let Some(target) = live
            .slots
            .get(to)
            .and_then(|slot| slot.effect_slots.get(effect_slot))
        else {
            return false;
        };
        let (node, modulator) = (target.node_id, target.modulator_node_id);
        if node == 0 {
            return false;
        }
        let copy_into = |rack: &mut RackTrackSnapshot| {
            let Some(mut values) = rack
                .slots
                .get(from)
                .and_then(|slot| slot.effect_slots.get(effect_slot))
                .cloned()
            else {
                return;
            };
            values.sync_to_descriptor_with_modulator(descriptor, node, modulator);
            if let Some(slot) = rack
                .slots
                .get_mut(to)
                .and_then(|slot| slot.effect_slots.get_mut(effect_slot))
            {
                *slot = values;
            }
        };
        copy_into(live);
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for patch in pool.sounds.patches.values_mut().map(Arc::make_mut) {
                if let Some(rack) = patch.rack_track.as_mut() {
                    copy_into(rack);
                }
            }
        }
        true
    }
}
