//! Kits on/off per bank (docs/rig-spec.md §6, eseq-ka5t.13): a drum rack
//! group's `parked` (off in the playing scene), `parked-banks` (the banks
//! whose every scene switches it off), `mixed-banks` (the banks where some
//! scenes do) and `unused` (off in every scene). The viewed bank is UI state
//! the host never sees, so Lisp reads a bank's state from these lists
//! (`eseq.bank-power/kit-bank-state`); bead eseq-ka5t.10 hides a parked kit's pads with them.
//!
//! Keyed on the playing scene and the shared [`TopologyKey`] (the scenes
//! revision, which every park, scene and bank edit moves, the group
//! topology and the group and bank instances).

use super::*;
use super::bank_usage::TopologyKey;
use sequencer::sequencer::{BankParkingRow, ParkedSet};

pub(super) struct KitParkingKey {
    topology: TopologyKey,
    current_scene: usize,
}

impl KitParkingKey {
    fn matches(&self, app: &app::App, kinds: &HostKinds) -> bool {
        self.current_scene == app.state.current_scene_index() && self.topology.matches(app, kinds)
    }
}

impl HostKinds {
    pub(super) fn sync_kit_parking(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        if self
            .kit_parking_key
            .as_ref()
            .is_some_and(|key| key.matches(app, self))
        {
            return;
        }
        let rows: Vec<BankParkingRow> = app.state.with_project_scenes(|scenes| {
            // Each bank's scene span, once for every group.
            let ranges = scenes.bank_ranges();
            app.groups
                .iter()
                .map(|group| match group.is_rack() {
                    true => scenes.bank_parking_row(ParkedSet::Kits, group.id, &ranges),
                    false => BankParkingRow::default(),
                })
                .collect()
        });
        for (group, row) in app.groups.iter().zip(&rows) {
            let Some(id) = self.groups.get(&group.id).copied() else {
                continue;
            };
            pusher.push(id, f::GROUP_PARKED, Value::Bool(row.parked));
            pusher.push(id, f::GROUP_PARKED_BANKS, bank_position_list(&self.bank_ids, &row.off_banks));
            pusher.push(id, f::GROUP_MIXED_BANKS, bank_position_list(&self.bank_ids, &row.mixed_banks));
            pusher.push(id, f::GROUP_UNUSED, Value::Bool(row.unused));
        }
        self.kit_parking_key = Some(KitParkingKey {
            topology: TopologyKey::read(app, self),
            current_scene: app.state.current_scene_index(),
        });
    }
}

/// The bank instances at `positions` (a [`BankParkingRow`]'s banks): the
/// one shape kits' `parked-banks` / `mixed-banks` and `project.instances`'
/// `:parked-banks` / `:mixed-banks` publish (docs/rig-spec.md §6, §10).
pub(crate) fn bank_position_list(bank_ids: &[Option<InstanceId>], positions: &[usize]) -> Value {
    instance_list(
        positions
            .iter()
            .filter_map(|index| bank_ids.get(*index).copied().flatten()),
    )
}
