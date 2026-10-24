//! Which banks use each track and kit (docs/rig-spec.md §9, eseq-ka5t.10):
//! what the bank-scoped views hide.
//!
//! A scene **uses** a track when it gives the track a pattern cell. Whether
//! that pattern has steps does not matter: a track a graph sequencer drives
//! plays with an empty pattern, and an empty cell still holds the lane's
//! devices, params and sound (`save_scene_snapshot_masked`).
//!
//! A kit's pads follow the kit, all or none: a scene uses a kit when it does
//! not park it (§6) and points at one of its clips (a legacy kit with no
//! clip bank: gives any of its pads a cell), whichever pads that clip has
//! patterns for.

use super::*;

/// A set of bank positions (bit = a bank's position in the bank list), with
/// no upper bound on the bank count.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BankMask(Vec<u64>);

impl BankMask {
    pub fn insert(&mut self, bank: usize) {
        let word = bank / 64;
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        self.0[word] |= 1 << (bank % 64);
    }

    pub fn contains(&self, bank: usize) -> bool {
        self.0.get(bank / 64).is_some_and(|word| word & (1 << (bank % 64)) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|word| *word == 0)
    }

    /// The bank positions in the set, ascending.
    pub fn banks(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word, bits)| {
            (0..64).filter(move |bit| bits & (1 << bit) != 0).map(move |bit| word * 64 + bit)
        })
    }
}

/// A kit for [`ProjectScenes::bank_usage`]: its group id and member tracks.
pub struct KitMembers<'a> {
    pub group_id: u64,
    pub members: &'a [usize],
}

/// Per track and per kit, the banks with a scene that uses it (empty: no
/// scene does).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankUsage {
    pub tracks: Vec<BankMask>,
    pub kits: Vec<BankMask>,
}

impl ProjectScenes {
    /// [`BankUsage`] for the first `track_count` tracks and `kits`, in one
    /// pass over the scenes.
    pub fn bank_usage(&self, track_count: usize, kits: &[KitMembers<'_>]) -> BankUsage {
        let mut kit_of: Vec<Option<usize>> = vec![None; track_count];
        for (kit, members) in kits.iter().enumerate() {
            for &track in members.members {
                if let Some(slot) = kit_of.get_mut(track) {
                    *slot = Some(kit);
                }
            }
        }
        let legacy: Vec<bool> = kits.iter().map(|kit| self.rack_is_legacy(kit.group_id)).collect();
        let has_cell = |scene: &Scene, track: usize| scene.cells.get(track).copied().flatten().is_some();
        let mut usage = BankUsage {
            tracks: vec![BankMask::default(); track_count],
            kits: vec![BankMask::default(); kits.len()],
        };
        for (bank, (_, range)) in self.scene_bank_ranges().enumerate() {
            let first = range.start;
            for (offset, scene) in self.clamped_scenes(range).iter().enumerate() {
                for (kit, members) in kits.iter().enumerate() {
                    if usage.kits[kit].contains(bank) || scene.parked_kits.contains(&members.group_id) {
                        continue;
                    }
                    let plays = if legacy[kit] {
                        members.members.iter().any(|track| has_cell(scene, *track))
                    } else {
                        self.scene_rack_clip(first + offset, members.group_id).is_some()
                    };
                    if plays {
                        usage.kits[kit].insert(bank);
                    }
                }
                for (track, mask) in usage.tracks.iter_mut().enumerate() {
                    if kit_of[track].is_none() && has_cell(scene, track) {
                        mask.insert(bank);
                    }
                }
            }
        }
        for (track, kit) in kit_of.iter().enumerate() {
            if let Some(kit) = kit {
                usage.tracks[track] = usage.kits[*kit].clone();
            }
        }
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` scenes of `tracks` tracks, every cell holding a pattern; the
    /// last `in_b` scenes moved to a second bank B.
    fn two_banks(count: usize, tracks: usize, in_b: usize) -> ProjectScenes {
        let snapshots: Vec<PatternSnapshot> = (0..count)
            .map(|_| PatternSnapshot::new_default(tracks, &[]))
            .collect();
        let mut scenes = ProjectScenes::from_pattern_snapshots(&snapshots, 0);
        let bank_b = scenes.create_scene_bank().expect("bank B");
        for _ in 0..in_b {
            scenes.move_scene_to_bank(count - in_b, bank_b).expect("move");
        }
        scenes
    }

    fn mask(banks: &[usize]) -> BankMask {
        let mut mask = BankMask::default();
        for bank in banks {
            mask.insert(*bank);
        }
        mask
    }

    #[test]
    fn a_bank_mask_holds_banks_past_the_128th() {
        let set = mask(&[0, 63, 64, 200]);
        assert!(set.contains(200) && set.contains(64) && !set.contains(199));
        assert_eq!(set.banks().collect::<Vec<_>>(), vec![0, 63, 64, 200]);
        assert!(BankMask::default().is_empty());
    }

    #[test]
    fn a_track_with_cells_only_in_bank_b_is_used_only_there() {
        let mut scenes = two_banks(3, 2, 1);
        assert_eq!(scenes.bank_usage(2, &[]).tracks, vec![mask(&[0, 1]), mask(&[0, 1])]);
        for scene in 0..2 {
            scenes.scenes[scene].cells[1] = None;
        }
        assert_eq!(scenes.bank_usage(2, &[]).tracks, vec![mask(&[0, 1]), mask(&[1])]);
        scenes.scenes[2].cells[1] = None;
        assert!(scenes.bank_usage(2, &[]).tracks[1].is_empty(), "no scene uses it");
    }

    #[test]
    fn a_legacy_kits_pads_follow_the_kit_and_hide_where_it_is_parked() {
        let mut scenes = two_banks(2, 3, 1);
        const KIT: u64 = 7;
        let members = [1, 2];
        let kits = [KitMembers { group_id: KIT, members: &members }];
        // Pad 2 has a cell in bank A only, pad 1 in both: both follow the kit.
        scenes.scenes[1].cells[2] = None;
        let usage = scenes.bank_usage(3, &kits);
        assert_eq!(usage.kits, vec![mask(&[0, 1])]);
        assert_eq!(usage.tracks[2], mask(&[0, 1]), "a pad follows its kit");
        scenes.set_kit_parked_in_scenes(KIT, true, 1..2);
        let usage = scenes.bank_usage(3, &kits);
        assert_eq!(usage.tracks, vec![mask(&[0, 1]), mask(&[0]), mask(&[0])], "bank B parks the kit");
        assert_eq!(usage.kits, vec![mask(&[0])]);
    }
}
