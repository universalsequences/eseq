//! Kits on/off per bank (docs/rig-spec.md §6, eseq-ka5t.13): a drum rack
//! group's `parked` (off in the playing scene), `parked-banks` (the banks
//! whose every scene switches it off), `mixed-banks` (the banks where some
//! scenes do) and `unused` (off in every scene). The viewed bank is UI state
//! the host never sees, so Lisp reads a bank's state from these lists
//! (`kit-bank-state`); bead eseq-ka5t.10 hides a parked kit's pads with them.
//!
//! Keyed on the scenes revision (every park edit, scene and bank edit moves
//! it), the playing scene, the racks and the group and bank instances.

use super::*;
use sequencer::sequencer::KitBankState;

pub(super) struct KitParkingKey {
    scenes_revision: u64,
    current_scene: usize,
    racks: Vec<(u64, bool)>,
    group_ids: Vec<Option<InstanceId>>,
    bank_ids: Vec<Option<InstanceId>>,
}

impl KitParkingKey {
    /// Compared in place: the two counters first, then the instance lists
    /// and the racks, without allocating.
    fn matches(&self, app: &app::App, group_ids: &[Option<InstanceId>], bank_ids: &[Option<InstanceId>]) -> bool {
        self.scenes_revision == app.state.project_scenes_revision()
            && self.current_scene == app.state.current_scene_index()
            && self.group_ids == group_ids
            && self.bank_ids == bank_ids
            && (app.groups.iter().map(|g| (g.id, g.is_rack()))).eq(self.racks.iter().copied())
    }
}

struct KitRow {
    parked: bool,
    unused: bool,
    off_banks: Vec<usize>,
    mixed_banks: Vec<usize>,
}

impl HostKinds {
    pub(super) fn sync_kit_parking(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        if self
            .kit_parking_key
            .as_ref()
            .is_some_and(|key| key.matches(app, &self.group_ids, &self.bank_ids))
        {
            return;
        }
        let rows: Vec<KitRow> = app.state.with_project_scenes(|scenes| {
            // Each bank's scene span, once for every group.
            let ranges: Vec<std::ops::Range<usize>> = scenes
                .scene_banks()
                .iter()
                .filter_map(|bank| scenes.scene_bank_range(bank.id))
                .collect();
            app.groups
                .iter()
                .map(|group| {
                    let mut row = KitRow {
                        parked: false,
                        unused: false,
                        off_banks: Vec::new(),
                        mixed_banks: Vec::new(),
                    };
                    if !group.is_rack() {
                        return row;
                    }
                    for (index, range) in ranges.iter().enumerate() {
                        match scenes.kit_state_in_scenes(group.id, range.clone()) {
                            KitBankState::Off => row.off_banks.push(index),
                            KitBankState::Mixed => row.mixed_banks.push(index),
                            KitBankState::On => {}
                        }
                    }
                    row.parked = scenes.kit_parked_in_scene(scenes.current_scene, group.id);
                    row.unused = scenes.kit_parked_in_every_scene(group.id);
                    row
                })
                .collect()
        });
        for (group, row) in app.groups.iter().zip(&rows) {
            let Some(id) = self.groups.get(&group.id).copied() else {
                continue;
            };
            let banks = |positions: &[usize]| {
                instance_list(
                    positions
                        .iter()
                        .filter_map(|index| self.bank_ids.get(*index).copied().flatten()),
                )
            };
            pusher.push(id, f::GROUP_PARKED, Value::Bool(row.parked));
            pusher.push(id, f::GROUP_PARKED_BANKS, banks(&row.off_banks));
            pusher.push(id, f::GROUP_MIXED_BANKS, banks(&row.mixed_banks));
            pusher.push(id, f::GROUP_UNUSED, Value::Bool(row.unused));
        }
        self.kit_parking_key = Some(KitParkingKey {
            scenes_revision: app.state.project_scenes_revision(),
            current_scene: app.state.current_scene_index(),
            racks: app.groups.iter().map(|g| (g.id, g.is_rack())).collect(),
            group_ids: self.group_ids.clone(),
            bank_ids: self.bank_ids.clone(),
        });
    }
}
