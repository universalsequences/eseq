//! Bank-scoped display (docs/rig-spec.md §9, eseq-ka5t.10): `track.used-banks`
//! and `group.used-banks`, the banks with a scene that uses the track (a
//! pattern cell; a kit's pad: a scene that points at one of the kit's clips
//! and does not park it) or the group (a kit: where it plays; a plain group:
//! where any member or child kit is used). See
//! `sequencer::sequencer::ProjectScenes::bank_usage` for the rule.
//!
//! The viewed bank is UI state the host never sees, so Lisp tests membership
//! (`eseq.scene-banks/used-in-viewed-bank?`). Recomputed only when the
//! scenes revision (every cell, clip pointer, park, scene and bank edit
//! moves it), the track, group or bank instances or the group topology
//! moved: one pass over the scenes, never per frame.

use super::*;
use sequencer::sequencer::KitMembers;

pub(super) struct BankUsageKey {
    scenes_revision: u64,
    track_ids: Vec<Option<InstanceId>>,
    group_ids: Vec<Option<InstanceId>>,
    bank_ids: Vec<Option<InstanceId>>,
    groups: Vec<(u64, bool, Vec<usize>, Vec<u64>)>,
}

impl BankUsageKey {
    /// Compared in place, without allocating.
    fn matches(&self, app: &app::App, kinds: &HostKinds) -> bool {
        self.scenes_revision == app.state.project_scenes_revision()
            && self.track_ids == kinds.track_ids
            && self.group_ids == kinds.group_ids
            && self.bank_ids == kinds.bank_ids
            && self.groups.len() == app.groups.len()
            && self.groups.iter().zip(&app.groups).all(|((id, rack, members, racks), group)| {
                *id == group.id
                    && *rack == group.is_rack()
                    && *members == group.members
                    && *racks == group.rack_members
            })
    }

    fn read(app: &app::App, kinds: &HostKinds) -> Self {
        Self {
            scenes_revision: app.state.project_scenes_revision(),
            track_ids: kinds.track_ids.clone(),
            group_ids: kinds.group_ids.clone(),
            bank_ids: kinds.bank_ids.clone(),
            groups: app
                .groups
                .iter()
                .map(|g| (g.id, g.is_rack(), g.members.clone(), g.rack_members.clone()))
                .collect(),
        }
    }
}

/// The bank instances a bank bitmask names.
fn banks_of(mask: u128, bank_ids: &[Option<InstanceId>]) -> Value {
    instance_list(
        bank_ids
            .iter()
            .enumerate()
            .take(128)
            .filter(|(index, _)| mask & (1_u128 << index) != 0)
            .filter_map(|(_, id)| *id),
    )
}

impl HostKinds {
    pub(super) fn sync_bank_usage(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        if self
            .bank_usage_key
            .as_ref()
            .is_some_and(|key| key.matches(app, self))
        {
            return;
        }
        self.bank_usage_passes += 1;
        let kits: Vec<KitMembers<'_>> = app
            .groups
            .iter()
            .filter(|g| g.is_rack())
            .map(|g| KitMembers { group_id: g.id, members: &g.members })
            .collect();
        let usage = app
            .state
            .with_project_scenes(|scenes| scenes.bank_usage(self.track_ids.len(), &kits));
        for (track, id) in self.track_ids.iter().enumerate() {
            if let Some(id) = *id {
                pusher.push(id, f::TRACK_USED_BANKS, banks_of(usage.tracks[track], &self.bank_ids));
            }
        }
        let kit_mask = |group_id: u64| {
            kits.iter()
                .position(|kit| kit.group_id == group_id)
                .map(|kit| usage.kits[kit])
        };
        for group in &app.groups {
            let Some(id) = self.groups.get(&group.id).copied() else {
                continue;
            };
            let mask = kit_mask(group.id).unwrap_or_else(|| {
                let members = group
                    .members
                    .iter()
                    .filter_map(|track| usage.tracks.get(*track))
                    .fold(0, |mask, used| mask | used);
                group
                    .rack_members
                    .iter()
                    .filter_map(|rack| kit_mask(*rack))
                    .fold(members, |mask, used| mask | used)
            });
            pusher.push(id, f::GROUP_USED_BANKS, banks_of(mask, &self.bank_ids));
        }
        self.bank_usage_key = Some(BankUsageKey::read(app, self));
    }

    /// How many times the bank usage has been recomputed (tests: a sync
    /// with nothing changed must not recompute).
    #[cfg(test)]
    pub(crate) fn bank_usage_passes(&self) -> u64 {
        self.bank_usage_passes
    }
}
