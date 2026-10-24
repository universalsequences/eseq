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
//! (`SequencerSnapshot::apply_scene_parking`). Every snapshot builder applies
//! the parked set of the scene it plays: the live capture the playing
//! scene's, a boundary launch its target's, a song row its row scene's.
//!
//! The implementation is shared with project-owned instances
//! (`super::parking`, `ParkedSet::Kits`); these are the kit-named reads the
//! rest of the crate uses.

use std::collections::BTreeSet;
use std::ops::Range;

use super::*;

/// The group ids of the kits a scene switches off (rig spec §3.2).
pub type ParkedKits = BTreeSet<u64>;

/// The project-file shape of every scene's `parked_kits`, outer index scene.
pub type SceneParkedKitsWire = Vec<Vec<u64>>;

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
        self.parked_in_scene(ParkedSet::Kits, scene, group_id)
    }

    /// Switch kit `group_id` off (`parked`) or on in every scene of
    /// `scenes`. Clip pointers are left alone. Returns whether anything
    /// changed.
    pub fn set_kit_parked_in_scenes(&mut self, group_id: u64, parked: bool, scenes: Range<usize>) -> bool {
        self.set_parked_in_scenes(ParkedSet::Kits, group_id, parked, scenes)
    }

    /// Whether every scene switches kit `group_id` off: unused project-wide
    /// (rig spec §6), listed for a purge, never deleted here.
    pub fn kit_parked_in_every_scene(&self, group_id: u64) -> bool {
        self.parked_in_every_scene(ParkedSet::Kits, group_id)
    }

    /// Drop every parked entry whose kit is not in `live` (a deleted kit, or
    /// a group that stopped being a drum rack), so a later group reusing the
    /// id does not inherit its scenes.
    pub fn retain_parked_kits(&mut self, live: &[u64]) {
        self.retain_parked(ParkedSet::Kits, live);
    }

    /// Every scene's `parked_kits` in the project-file shape.
    pub fn parked_kits_to_wire(&self) -> SceneParkedKitsWire {
        self.parked_to_wire(ParkedSet::Kits)
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
        let state = |scenes: &ProjectScenes, range| scenes.parked_state_in_scenes(ParkedSet::Kits, KIT, range);
        assert_eq!(state(&scenes, 0..3), BankState::On);
        assert!(scenes.set_kit_parked_in_scenes(KIT, true, 0..1));
        assert_eq!(state(&scenes, 0..3), BankState::Mixed);
        assert!(scenes.set_kit_parked_in_scenes(KIT, true, 0..3));
        assert_eq!(state(&scenes, 0..3), BankState::Off);
        assert!(!scenes.set_kit_parked_in_scenes(KIT, true, 0..3), "already off");
        assert!(!scenes.parked_would_change(ParkedSet::Kits, KIT, true, 0..3));
        assert!(scenes.parked_would_change(ParkedSet::Kits, KIT, false, 1..2));
        assert_eq!(state(&scenes, 3..3), BankState::On, "an empty bank");
    }

    #[test]
    fn parked_kits_round_trip_the_wire_and_old_files_park_nothing() {
        let mut scenes = scenes(3);
        assert!(scenes.parked_kits_to_wire().is_empty(), "nothing parked writes nothing");
        scenes.set_kit_parked_in_scenes(KIT, true, 1..3);
        let wire = scenes.parked_kits_to_wire();
        assert_eq!(wire, vec![vec![], vec![KIT], vec![KIT]]);
        let mut loaded = self::scenes(3);
        loaded.parked_from_wire(ParkedSet::Kits, &wire);
        assert!(loaded.kit_parked_in_scene(2, KIT));
        loaded.parked_from_wire(ParkedSet::Kits, &[]);
        assert!((0..3).all(|scene| !loaded.kit_parked_in_scene(scene, KIT)));
    }

    #[test]
    fn unused_kits_are_the_ones_every_scene_parks_and_deletion_forgets_them() {
        let mut scenes = scenes(2);
        scenes.set_kit_parked_in_scenes(KIT, true, 0..2);
        scenes.set_kit_parked_in_scenes(9, true, 0..1);
        let unused: Vec<u64> = [KIT, 9, 11].into_iter().filter(|kit| scenes.kit_parked_in_every_scene(*kit)).collect();
        assert_eq!(unused, vec![KIT]);
        scenes.retain_parked_kits(&[9]);
        assert!(!scenes.kit_parked_in_scene(0, KIT));
        assert!(scenes.kit_parked_in_scene(0, 9));
    }
}
