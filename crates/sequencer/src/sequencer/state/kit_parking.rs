//! Kits on/off per bank (docs/rig-spec.md §3.2, §6; eseq-ka5t.13).
//!
//! `Scene::parked_kits` holds the group ids of the drum racks (kits) that are
//! switched off in that scene. A kit plays in a scene when the scene points
//! at one of its clips (`Scene::rack_clips`, the musical choice) **and** the
//! kit is not parked there (the resource choice). Parking never touches a
//! clip pointer, so switching a kit back on restores every scene exactly,
//! intentional silences (no pointer) included.
//!
//! Resolution is a gate, not a rewrite of what the scene composes: the live
//! grid still installs the kit's clip (so its lanes stay visible, editable and
//! saved back into the right clip), and the scheduler snapshot of a scene that
//! parks the kit marks every member track `scene_silenced` (no pattern
//! triggers) and lists the kit in `SequencerSnapshot::parked_racks`, which
//! stops its rack-owned graphs and generators from running at all
//! (`SequencerSnapshot::park_kits`). Every snapshot builder applies the parked
//! set of the scene it plays: the live capture the playing scene's, a
//! boundary launch its target's, a song row its row scene's.

use std::collections::BTreeSet;
use std::ops::Range;

use super::*;

/// The group ids of the kits a scene switches off (rig spec §3.2).
pub type ParkedKits = BTreeSet<u64>;

/// The project-file shape of every scene's `parked_kits`, outer index scene.
pub type SceneParkedKitsWire = Vec<Vec<u64>>;

/// A kit's on/off over the scenes of one bank (rig spec §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KitBankState {
    /// Parked in no scene of the bank (an empty bank reads on).
    On,
    /// Parked in every scene of the bank.
    Off,
    /// Parked in some scenes of the bank and not in others.
    Mixed,
}

/// A scene bank's auto label by position: `A`…`Z`, then `AA`, `AB`, ….
pub fn scene_bank_auto_label(mut index: usize) -> String {
    let mut reversed = Vec::new();
    loop {
        reversed.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    reversed.reverse();
    String::from_utf8(reversed).expect("scene bank labels contain only ASCII letters")
}

impl ProjectScenes {
    /// Whether scene `scene` switches kit `group_id` off.
    pub fn kit_parked_in_scene(&self, scene: usize, group_id: u64) -> bool {
        self.scenes
            .get(scene)
            .is_some_and(|scene| scene.parked_kits.contains(&group_id))
    }

    /// The kits scene `scene` switches off, in id order (empty past the end).
    pub fn scene_parked_kits(&self, scene: usize) -> Vec<u64> {
        self.scenes
            .get(scene)
            .map(|scene| scene.parked_kits.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The kits the playing scene switches off.
    pub fn current_parked_kits(&self) -> Vec<u64> {
        self.scene_parked_kits(self.current_scene)
    }

    /// Kit `group_id` over the scenes of `scenes` (rig spec §6 "Mixed bank").
    pub fn kit_state_in_scenes(&self, group_id: u64, scenes: Range<usize>) -> KitBankState {
        let (mut parked, mut playing) = (0usize, 0usize);
        for scene in self.clamped_scenes(scenes) {
            if scene.parked_kits.contains(&group_id) {
                parked += 1;
            } else {
                playing += 1;
            }
        }
        match (parked, playing) {
            (0, _) => KitBankState::On,
            (_, 0) => KitBankState::Off,
            _ => KitBankState::Mixed,
        }
    }

    /// Kit `group_id` over the scenes of bank `bank`; `None` when the bank
    /// does not exist.
    pub fn kit_state_in_bank(&self, group_id: u64, bank: SceneBankId) -> Option<KitBankState> {
        let range = self.scene_bank_range(bank)?;
        Some(self.kit_state_in_scenes(group_id, range))
    }

    /// Whether parking (`parked`) kit `group_id` in `scenes` changes any of
    /// them.
    pub fn kit_parked_would_change(&self, group_id: u64, parked: bool, scenes: Range<usize>) -> bool {
        self.clamped_scenes(scenes)
            .iter()
            .any(|scene| scene.parked_kits.contains(&group_id) != parked)
    }

    /// `scenes` clamped to the scenes that exist (one rule for every
    /// bank-range read and write here).
    fn clamped_range(&self, scenes: Range<usize>) -> Range<usize> {
        let end = scenes.end.min(self.scenes.len());
        scenes.start.min(end)..end
    }

    fn clamped_scenes(&self, scenes: Range<usize>) -> &[Scene] {
        &self.scenes[self.clamped_range(scenes)]
    }

    /// Switch kit `group_id` off (`parked`) or on in every scene of
    /// `scenes`. Clip pointers are left alone. Returns whether anything
    /// changed.
    pub fn set_kit_parked_in_scenes(&mut self, group_id: u64, parked: bool, scenes: Range<usize>) -> bool {
        let range = self.clamped_range(scenes);
        let mut changed = false;
        for scene in &mut self.scenes[range] {
            changed |= if parked {
                scene.parked_kits.insert(group_id)
            } else {
                scene.parked_kits.remove(&group_id)
            };
        }
        changed
    }

    /// The kits of `racks` (group ids) that every scene parks: unused
    /// project-wide (rig spec §6), listed for a purge, never deleted here.
    /// A project with no scenes parks nothing.
    #[cfg(test)]
    pub fn kits_parked_in_every_scene(&self, racks: &[u64]) -> Vec<u64> {
        racks
            .iter()
            .copied()
            .filter(|group_id| self.kit_parked_in_every_scene(*group_id))
            .collect()
    }

    /// Whether every scene switches kit `group_id` off (a project with no
    /// scenes parks nothing).
    pub fn kit_parked_in_every_scene(&self, group_id: u64) -> bool {
        !self.scenes.is_empty() && self.scenes.iter().all(|scene| scene.parked_kits.contains(&group_id))
    }

    /// Drop every parked entry whose kit is not in `live` (a deleted kit, or
    /// a group that stopped being a drum rack), so a later group reusing the
    /// id does not inherit its scenes.
    pub fn retain_parked_kits(&mut self, live: &[u64]) {
        for scene in &mut self.scenes {
            scene.parked_kits.retain(|group_id| live.contains(group_id));
        }
    }

    /// Every scene's `parked_kits` in the project-file shape. All empty
    /// yields an empty list, so a project that parks nothing writes nothing.
    pub fn parked_kits_to_wire(&self) -> SceneParkedKitsWire {
        if self.scenes.iter().all(|scene| scene.parked_kits.is_empty()) {
            return Vec::new();
        }
        self.scenes
            .iter()
            .map(|scene| scene.parked_kits.iter().copied().collect())
            .collect()
    }

    /// Install `parked_kits_to_wire`'s shape, scene by scene. Scenes the
    /// wire does not cover (older files: all of them) park nothing.
    pub fn parked_kits_from_wire(&mut self, wire: &[Vec<u64>]) {
        for (index, scene) in self.scenes.iter_mut().enumerate() {
            scene.parked_kits = wire
                .get(index)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default();
        }
    }
}

impl SequencerState {
    /// The kits the playing scene switches off.
    pub fn current_parked_kits(&self) -> Vec<u64> {
        self.pattern.scenes.lock().unwrap().current_parked_kits()
    }

    /// `ProjectScenes::set_kit_parked_in_scenes` under the scenes lock.
    /// Unrecorded and unpublished: the caller records the step and
    /// publishes the scheduler snapshot once.
    pub fn set_kit_parked_in_scenes(&self, group_id: u64, parked: bool, scenes: Range<usize>) -> bool {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .set_kit_parked_in_scenes(group_id, parked, scenes)
    }

    /// Install per-scene `parked_kits` read from a project file.
    pub fn install_scene_parked_kits(&self, wire: &[Vec<u64>]) {
        self.pattern.scenes.lock().unwrap().parked_kits_from_wire(wire);
    }
}

impl SequencerSnapshot {
    /// Apply the parked kits of the scene this snapshot plays (rig spec §6):
    /// every member track of a parked kit gets no pattern triggers, and the
    /// kit's rack-owned graphs and generators do not run
    /// (`parked_racks`). Membership is the snapshot's own
    /// `rack_memberships`, so a legacy kit (no clip bank) parks too.
    pub fn park_kits(&mut self, parked: Vec<u64>) {
        for membership in &self.rack_memberships {
            if !parked.contains(&membership.group_id) {
                continue;
            }
            for &track in &membership.members {
                let Some(snapshot) = self.tracks.get_mut(track) else {
                    continue;
                };
                if !snapshot.scene_silenced {
                    std::sync::Arc::make_mut(snapshot).scene_silenced = true;
                }
            }
        }
        self.parked_racks = parked;
    }

    /// Re-apply this snapshot's `parked_racks` to track `track` alone (a
    /// single-track recapture), without reading the scenes.
    pub fn park_track(&mut self, track: usize) {
        if self.parked_racks.is_empty() {
            return;
        }
        let parked = self.rack_memberships.iter().any(|membership| {
            self.parked_racks.contains(&membership.group_id) && membership.members.contains(&track)
        });
        if let Some(snapshot) = self.tracks.get_mut(track).filter(|_| parked) {
            if !snapshot.scene_silenced {
                std::sync::Arc::make_mut(snapshot).scene_silenced = true;
            }
        }
    }

    /// Whether rack `group_id` is parked in the scene this snapshot plays.
    pub fn rack_parked(&self, group_id: u64) -> bool {
        self.parked_racks.contains(&group_id)
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

    const KIT: u64 = 7;

    #[test]
    fn a_kit_bank_state_reads_on_off_and_mixed() {
        let mut scenes = scenes(3);
        assert_eq!(scenes.kit_state_in_scenes(KIT, 0..3), KitBankState::On);
        assert!(scenes.set_kit_parked_in_scenes(KIT, true, 0..1));
        assert_eq!(scenes.kit_state_in_scenes(KIT, 0..3), KitBankState::Mixed);
        assert!(scenes.set_kit_parked_in_scenes(KIT, true, 0..3));
        assert_eq!(scenes.kit_state_in_scenes(KIT, 0..3), KitBankState::Off);
        assert!(!scenes.set_kit_parked_in_scenes(KIT, true, 0..3), "already off");
        assert!(!scenes.kit_parked_would_change(KIT, true, 0..3));
        assert!(scenes.kit_parked_would_change(KIT, false, 1..2));
        assert_eq!(scenes.kit_state_in_scenes(KIT, 3..3), KitBankState::On, "an empty bank");
    }

    #[test]
    fn parked_kits_round_trip_the_wire_and_old_files_park_nothing() {
        let mut scenes = scenes(3);
        assert!(scenes.parked_kits_to_wire().is_empty(), "nothing parked writes nothing");
        scenes.set_kit_parked_in_scenes(KIT, true, 1..3);
        let wire = scenes.parked_kits_to_wire();
        assert_eq!(wire, vec![vec![], vec![KIT], vec![KIT]]);
        let mut loaded = self::scenes(3);
        loaded.parked_kits_from_wire(&wire);
        assert_eq!(loaded.scene_parked_kits(2), vec![KIT]);
        loaded.parked_kits_from_wire(&[]);
        assert!((0..3).all(|scene| loaded.scene_parked_kits(scene).is_empty()));
    }

    #[test]
    fn unused_kits_are_the_ones_every_scene_parks_and_deletion_forgets_them() {
        let mut scenes = scenes(2);
        scenes.set_kit_parked_in_scenes(KIT, true, 0..2);
        scenes.set_kit_parked_in_scenes(9, true, 0..1);
        assert_eq!(scenes.kits_parked_in_every_scene(&[KIT, 9, 11]), vec![KIT]);
        scenes.retain_parked_kits(&[9]);
        assert!(!scenes.kit_parked_in_scene(0, KIT));
        assert!(scenes.kit_parked_in_scene(0, 9));
    }
}
