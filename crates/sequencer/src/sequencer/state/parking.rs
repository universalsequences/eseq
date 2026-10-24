//! Who plays is scene state (docs/rig-spec.md §3, §6, §10): the one parking
//! implementation behind kits on/off per bank (`Scene::parked_kits`,
//! `super::kit_parking`) and project-owned sequencers on/off per bank
//! (`Scene::parked_instances`, `super::instance_parking`).
//!
//! Both are a set of ids per scene. Every read and write here takes the set
//! it works on ([`ParkedSet`]), so a bank's on/off/mixed state, the bank
//! toggle, the "unused" check, the project-file wire and the per-bank host
//! rows are written once. The scheduler side is one entry point too:
//! [`SequencerSnapshot::apply_scene_parking`] applies everything the scene a
//! snapshot plays switches off, and [`SequencerSnapshot::apply_track_parking`]
//! re-applies it to one recaptured track.

use std::collections::BTreeSet;
use std::ops::Range;

use super::*;

/// Which per-scene parked set an operation works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkedSet {
    /// `Scene::parked_kits`: drum rack group ids.
    Kits,
    /// `Scene::parked_instances`: project-owned kind instance ids.
    Instances,
}

impl ParkedSet {
    pub fn of(self, scene: &Scene) -> &BTreeSet<u64> {
        match self {
            Self::Kits => &scene.parked_kits,
            Self::Instances => &scene.parked_instances,
        }
    }

    pub fn of_mut(self, scene: &mut Scene) -> &mut BTreeSet<u64> {
        match self {
            Self::Kits => &mut scene.parked_kits,
            Self::Instances => &mut scene.parked_instances,
        }
    }
}

/// A kit's or instance's on/off over the scenes of one bank (rig spec §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankState {
    /// Parked in no scene of the bank (an empty bank reads on).
    On,
    /// Parked in every scene of the bank.
    Off,
    /// Parked in some scenes of the bank and not in others.
    Mixed,
}

/// One kit's or instance's on/off in every bank, as the host rows publish
/// it: `off_banks` / `mixed_banks` are bank positions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankParkingRow {
    /// Off in the playing scene.
    pub parked: bool,
    /// Off in every scene (listed for purge).
    pub unused: bool,
    pub off_banks: Vec<usize>,
    pub mixed_banks: Vec<usize>,
}

/// Everything the scene a snapshot plays switches off (docs/rig-spec.md §6,
/// §10): every snapshot builder takes one, so none can forget either half.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneParking {
    /// Drum rack group ids (`Scene::parked_kits`).
    pub kits: Vec<u64>,
    /// Project-owned instance ids (`Scene::parked_instances`), sorted,
    /// rack-owned ones already filtered out.
    pub instances: Vec<u64>,
}

impl ProjectScenes {
    /// `scenes` clamped to the scenes that exist (one rule for every
    /// bank-range read and write).
    pub fn clamped_range(&self, scenes: Range<usize>) -> Range<usize> {
        let end = scenes.end.min(self.scenes.len());
        scenes.start.min(end)..end
    }

    pub fn clamped_scenes(&self, scenes: Range<usize>) -> &[Scene] {
        &self.scenes[self.clamped_range(scenes)]
    }

    /// Whether scene `scene` switches `id` off in `set`.
    pub fn parked_in_scene(&self, set: ParkedSet, scene: usize, id: u64) -> bool {
        self.scenes
            .get(scene)
            .is_some_and(|scene| set.of(scene).contains(&id))
    }

    /// `id` over the scenes of `scenes` in `set`.
    pub fn parked_state_in_scenes(&self, set: ParkedSet, id: u64, scenes: Range<usize>) -> BankState {
        let (mut parked, mut playing) = (0usize, 0usize);
        for scene in self.clamped_scenes(scenes) {
            if set.of(scene).contains(&id) {
                parked += 1;
            } else {
                playing += 1;
            }
        }
        match (parked, playing) {
            (0, _) => BankState::On,
            (_, 0) => BankState::Off,
            _ => BankState::Mixed,
        }
    }

    /// Whether parking (`parked`) `id` in `scenes` changes any of them.
    pub fn parked_would_change(&self, set: ParkedSet, id: u64, parked: bool, scenes: Range<usize>) -> bool {
        self.clamped_scenes(scenes)
            .iter()
            .any(|scene| set.of(scene).contains(&id) != parked)
    }

    /// Switch `id` off (`parked`) or on in every scene of `scenes`. Returns
    /// whether anything changed.
    pub fn set_parked_in_scenes(&mut self, set: ParkedSet, id: u64, parked: bool, scenes: Range<usize>) -> bool {
        let range = self.clamped_range(scenes);
        let mut changed = false;
        for scene in &mut self.scenes[range] {
            let ids = set.of_mut(scene);
            changed |= if parked { ids.insert(id) } else { ids.remove(&id) };
        }
        changed
    }

    /// Park `id` in every scene outside `active` and nowhere inside it.
    pub fn park_outside(&mut self, set: ParkedSet, id: u64, active: Range<usize>) {
        for (index, scene) in self.scenes.iter_mut().enumerate() {
            let ids = set.of_mut(scene);
            if active.contains(&index) {
                ids.remove(&id);
            } else {
                ids.insert(id);
            }
        }
    }

    /// Whether every scene switches `id` off (a project with no scenes
    /// parks nothing).
    pub fn parked_in_every_scene(&self, set: ParkedSet, id: u64) -> bool {
        !self.scenes.is_empty() && self.scenes.iter().all(|scene| set.of(scene).contains(&id))
    }

    /// Forget `ids` in every scene. Returns whether anything changed.
    pub fn forget_parked(&mut self, set: ParkedSet, ids: &[u64]) -> bool {
        let mut changed = false;
        for scene in &mut self.scenes {
            let parked = set.of_mut(scene);
            for id in ids {
                changed |= parked.remove(id);
            }
        }
        changed
    }

    /// Drop every entry of `set` not in `live`.
    pub fn retain_parked(&mut self, set: ParkedSet, live: &[u64]) {
        for scene in &mut self.scenes {
            set.of_mut(scene).retain(|id| live.contains(id));
        }
    }

    /// Every scene's `set` in the project-file shape, outer index scene.
    /// All empty yields an empty list, so a project that parks nothing
    /// writes nothing.
    pub fn parked_to_wire(&self, set: ParkedSet) -> Vec<Vec<u64>> {
        if self.scenes.iter().all(|scene| set.of(scene).is_empty()) {
            return Vec::new();
        }
        self.scenes
            .iter()
            .map(|scene| set.of(scene).iter().copied().collect())
            .collect()
    }

    /// Install `parked_to_wire`'s shape, scene by scene. Scenes the wire
    /// does not cover (older files: all of them) park nothing.
    pub fn parked_from_wire(&mut self, set: ParkedSet, wire: &[Vec<u64>]) {
        for (index, scene) in self.scenes.iter_mut().enumerate() {
            *set.of_mut(scene) = wire
                .get(index)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default();
        }
    }

    /// Every bank's scene span, by bank position: computed once for a batch
    /// of [`Self::bank_parking_row`]s.
    pub fn bank_ranges(&self) -> Vec<Range<usize>> {
        self.scene_banks()
            .iter()
            .filter_map(|bank| self.scene_bank_range(bank.id))
            .collect()
    }

    /// `id`'s on/off in every bank of `ranges` (from [`Self::bank_ranges`]).
    pub fn bank_parking_row(&self, set: ParkedSet, id: u64, ranges: &[Range<usize>]) -> BankParkingRow {
        let mut row = BankParkingRow::default();
        for (index, range) in ranges.iter().enumerate() {
            match self.parked_state_in_scenes(set, id, range.clone()) {
                BankState::Off => row.off_banks.push(index),
                BankState::Mixed => row.mixed_banks.push(index),
                BankState::On => {}
            }
        }
        row.parked = self.parked_in_scene(set, self.current_scene, id);
        row.unused = self.parked_in_every_scene(set, id);
        row
    }

    /// What scene `scene` switches off: its kits and its project-owned
    /// instances (empty past the end).
    pub fn scene_parking(&self, scene: usize) -> SceneParking {
        let Some(scene) = self.scenes.get(scene) else {
            return SceneParking::default();
        };
        SceneParking {
            kits: scene.parked_kits.iter().copied().collect(),
            instances: scene
                .parked_instances
                .iter()
                .copied()
                .filter(|id| !self.instance_racks.contains_key(id))
                .collect(),
        }
    }

    /// What the playing scene switches off.
    pub fn current_scene_parking(&self) -> SceneParking {
        self.scene_parking(self.current_scene)
    }
}

impl SequencerState {
    /// What the playing scene switches off (kits and instances).
    pub fn current_scene_parking(&self) -> SceneParking {
        self.pattern.scenes.lock().unwrap().current_scene_parking()
    }

    /// `ProjectScenes::set_parked_in_scenes` under the scenes lock.
    /// Unrecorded and unpublished: the caller records the step and
    /// publishes the scheduler snapshot once.
    pub fn set_parked_in_scenes(&self, set: ParkedSet, id: u64, parked: bool, scenes: Range<usize>) -> bool {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .set_parked_in_scenes(set, id, parked, scenes)
    }

    /// Install a per-scene parked set read from a project file.
    pub fn install_scene_parked(&self, set: ParkedSet, wire: &[Vec<u64>]) {
        self.pattern.scenes.lock().unwrap().parked_from_wire(set, wire);
    }
}

impl SequencerSnapshot {
    /// Apply everything the scene this snapshot plays switches off (rig spec
    /// §6, §10), the one entry point every snapshot builder calls:
    ///
    /// - kits: every member track of a parked kit gets no pattern triggers,
    ///   and the kit's rack-owned graphs and generators do not run
    ///   (`parked_racks`). Membership is the snapshot's own
    ///   `rack_memberships`, so a legacy kit (no clip bank) parks too.
    /// - project-owned instances: their graphs and generators do not run
    ///   (`parked_instances`, read by the lookahead gates), and every process
    ///   slot one of them owns is disabled on every track.
    pub fn apply_scene_parking(&mut self, parking: SceneParking) {
        self.parked_racks = parking.kits;
        self.parked_instances = parking.instances;
        for track in 0..self.tracks.len() {
            self.apply_track_parking(track);
        }
    }

    /// Re-apply this snapshot's parking to track `track` alone (a
    /// single-track recapture starts unparked), without reading the scenes.
    pub fn apply_track_parking(&mut self, track: usize) {
        let silenced = !self.parked_racks.is_empty()
            && self.rack_memberships.iter().any(|membership| {
                self.parked_racks.contains(&membership.group_id) && membership.members.contains(&track)
            });
        let Some(snapshot) = self.tracks.get_mut(track) else {
            return;
        };
        if silenced && !snapshot.scene_silenced {
            std::sync::Arc::make_mut(snapshot).scene_silenced = true;
        }
        if !self.parked_instances.is_empty() {
            super::instance_parking::park_owned_process_slots(snapshot, &self.parked_instances);
        }
    }

    /// Whether rack `group_id` is parked in the scene this snapshot plays.
    pub fn rack_parked(&self, group_id: u64) -> bool {
        self.parked_racks.contains(&group_id)
    }

    /// Whether project-owned instance `id` is parked in the scene this
    /// snapshot plays (`parked_instances` is sorted).
    pub fn instance_parked(&self, id: u64) -> bool {
        self.parked_instances.binary_search(&id).is_ok()
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

    #[test]
    fn the_two_sets_are_independent_and_share_one_implementation() {
        let mut scenes = scenes(3);
        assert!(scenes.set_parked_in_scenes(ParkedSet::Kits, 7, true, 0..1));
        assert!(!scenes.parked_in_scene(ParkedSet::Instances, 0, 7), "kit 7 is not instance 7");
        assert_eq!(scenes.parked_state_in_scenes(ParkedSet::Kits, 7, 0..3), BankState::Mixed);
        assert_eq!(scenes.parked_state_in_scenes(ParkedSet::Kits, 7, 3..3), BankState::On, "an empty bank");
        scenes.park_outside(ParkedSet::Instances, 7, 0..1);
        assert_eq!(scenes.parked_to_wire(ParkedSet::Instances), vec![vec![], vec![7], vec![7]]);
        let row = scenes.bank_parking_row(ParkedSet::Instances, 7, &[0..1, 1..3]);
        assert_eq!(row, BankParkingRow { parked: false, unused: false, off_banks: vec![1], mixed_banks: vec![] });
        assert!(scenes.forget_parked(ParkedSet::Instances, &[7]));
        assert!(scenes.parked_in_scene(ParkedSet::Kits, 0, 7), "forgetting one set leaves the other");
    }
}
