//! Which banks use each track and kit (docs/rig-spec.md §9, eseq-ka5t.10):
//! what the bank-scoped views hide.
//!
//! A scene **uses** a track when it gives the track a pattern cell. Whether
//! that pattern has steps does not matter: a track a graph sequencer drives
//! plays with an empty pattern, and an empty cell still holds the lane's
//! devices, params and sound (`save_scene_snapshot_masked`). A kit's pad is
//! used in a scene that points at one of the kit's clips and does not park
//! the kit (§6), whichever pads that clip has patterns for; a legacy kit (no
//! clip bank) keeps its pads' cells in the scenes, so a pad is used where
//! the scene gives it a cell and does not park the kit. A kit is used where
//! any of its pads is (a clip-bearing kit: where it plays).

use super::*;

/// A kit for [`ProjectScenes::bank_usage`]: its group id and member tracks.
pub struct KitMembers<'a> {
    pub group_id: u64,
    pub members: &'a [usize],
}

/// Per track and per kit, a bitmask of the banks (bit = bank position) with
/// a scene that uses it; 0: no scene does. Banks past the 128th do not show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankUsage {
    pub tracks: Vec<u128>,
    pub kits: Vec<u128>,
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
        let clip_bearing: Vec<bool> = kits
            .iter()
            .map(|kit| self.rack_bank(kit.group_id).is_some_and(|bank| !bank.clips.is_empty()))
            .collect();
        let mut usage = BankUsage {
            tracks: vec![0; track_count],
            kits: vec![0; kits.len()],
        };
        let mut parked = vec![false; kits.len()];
        let mut plays = vec![false; kits.len()];
        for (bank_index, (_, range)) in self.scene_bank_ranges().enumerate().take(128) {
            let bit = 1_u128 << bank_index;
            let end = range.end.min(self.scenes.len());
            for scene in &self.scenes[range.start.min(end)..end] {
                for (kit, members) in kits.iter().enumerate() {
                    parked[kit] = scene.parked_kits.contains(&members.group_id);
                    plays[kit] = clip_bearing[kit]
                        && !parked[kit]
                        && scene.rack_clips.iter().any(|(group, _)| *group == members.group_id);
                    if plays[kit] {
                        usage.kits[kit] |= bit;
                    }
                }
                for (track, mask) in usage.tracks.iter_mut().enumerate() {
                    let has_cell = || scene.cells.get(track).copied().flatten().is_some();
                    let used = match kit_of[track] {
                        Some(kit) if clip_bearing[kit] => plays[kit],
                        Some(kit) => !parked[kit] && has_cell(),
                        None => has_cell(),
                    };
                    if used {
                        *mask |= bit;
                        if let Some(kit) = kit_of[track] {
                            usage.kits[kit] |= bit;
                        }
                    }
                }
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

    #[test]
    fn a_track_with_cells_only_in_bank_b_is_used_only_there() {
        let mut scenes = two_banks(3, 2, 1);
        assert_eq!(scenes.bank_usage(2, &[]).tracks, vec![0b11, 0b11]);
        for scene in 0..2 {
            scenes.scenes[scene].cells[1] = None;
        }
        assert_eq!(scenes.bank_usage(2, &[]).tracks, vec![0b11, 0b10]);
        scenes.scenes[2].cells[1] = None;
        assert_eq!(scenes.bank_usage(2, &[]).tracks[1], 0, "no scene uses it");
    }

    #[test]
    fn a_legacy_kit_pad_is_unused_where_its_kit_is_parked() {
        let mut scenes = two_banks(2, 2, 1);
        const KIT: u64 = 7;
        let members = [1];
        let kits = [KitMembers { group_id: KIT, members: &members }];
        assert_eq!(scenes.bank_usage(2, &kits), BankUsage { tracks: vec![0b11, 0b11], kits: vec![0b11] });
        scenes.set_kit_parked_in_scenes(KIT, true, 1..2);
        let usage = scenes.bank_usage(2, &kits);
        assert_eq!(usage.tracks, vec![0b11, 0b01], "bank B parks the kit");
        assert_eq!(usage.kits, vec![0b01]);
    }
}
