//! Bank-scoped display (docs/rig-spec.md §9, eseq-ka5t.10): `track.used-banks`
//! and `group.used-banks` (drum racks only), the banks with a scene that uses
//! the track (a pattern cell; a kit's pad follows its kit) or the kit (a
//! scene points at one of its clips and does not park it). See
//! `sequencer::sequencer::ProjectScenes::bank_usage` for the rule. A plain
//! group's list stays empty: Lisp shows it where any member shows
//! (`eseq.scene-banks/group-used-in-bank?`), which includes members no bank
//! uses, a rule a bank list cannot carry.
//!
//! The viewed bank is UI state the host never sees, so Lisp tests membership
//! (`eseq.scene-banks/used-in-viewed-bank?`). Recomputed only when the
//! [`TopologyKey`] or the track instances moved: one pass over the scenes,
//! never per frame. Only rows whose banks changed are pushed.

use super::*;
use sequencer::sequencer::{BankMask, BankUsage, KitMembers};

/// A group as the bank feeds key on it.
#[derive(PartialEq)]
struct GroupShape {
    id: u64,
    rack: bool,
    members: Vec<usize>,
    rack_members: Vec<u64>,
}

impl GroupShape {
    fn matches(&self, group: &sequencer::project::ProjectTrackGroup) -> bool {
        self.id == group.id
            && self.rack == group.is_rack()
            && self.members == group.members
            && self.rack_members == group.rack_members
    }
}

/// What the per-bank group feeds (kit parking, bank usage) recompute on:
/// the scenes revision (every cell, clip pointer, park, scene and bank edit
/// moves it), the group and bank instances and the group topology.
pub(super) struct TopologyKey {
    scenes_revision: u64,
    group_ids: Vec<Option<InstanceId>>,
    bank_ids: Vec<Option<InstanceId>>,
    groups: Vec<GroupShape>,
}

impl TopologyKey {
    /// Compared in place, without allocating.
    pub(super) fn matches(&self, app: &app::App, kinds: &HostKinds) -> bool {
        self.scenes_revision == app.state.project_scenes_revision()
            && self.same_instances(kinds)
            && self.groups.len() == app.groups.len()
            && self.groups.iter().zip(&app.groups).all(|(shape, group)| shape.matches(group))
    }

    pub(super) fn read(app: &app::App, kinds: &HostKinds) -> Self {
        Self {
            scenes_revision: app.state.project_scenes_revision(),
            group_ids: kinds.group_ids.clone(),
            bank_ids: kinds.bank_ids.clone(),
            groups: app
                .groups
                .iter()
                .map(|g| GroupShape {
                    id: g.id,
                    rack: g.is_rack(),
                    members: g.members.clone(),
                    rack_members: g.rack_members.clone(),
                })
                .collect(),
        }
    }

    /// Whether the group and bank instances are the ones the feed last
    /// pushed with.
    fn same_instances(&self, kinds: &HostKinds) -> bool {
        self.group_ids == kinds.group_ids && self.bank_ids == kinds.bank_ids
    }
}

pub(super) struct BankUsageKey {
    topology: TopologyKey,
    track_ids: Vec<Option<InstanceId>>,
    /// What the last pass pushed, by position (kits in `app.groups` rack
    /// order).
    usage: BankUsage,
}

/// The bank instances a bank set names (positions past the bank list are
/// dropped).
pub(super) fn bank_list(mask: &BankMask, bank_ids: &[Option<InstanceId>]) -> Value {
    instance_list(mask.banks().filter_map(|bank| bank_ids.get(bank).copied().flatten()))
}

impl HostKinds {
    pub(super) fn sync_bank_usage(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let previous = self.bank_usage_key.take();
        if let Some(key) = &previous {
            if key.track_ids == self.track_ids && key.topology.matches(app, self) {
                self.bank_usage_key = previous;
                return;
            }
        }
        #[cfg(test)]
        {
            self.bank_usage_passes += 1;
        }
        let kits: Vec<KitMembers<'_>> = app
            .groups
            .iter()
            .filter(|g| g.is_rack())
            .map(|g| KitMembers { group_id: g.id, members: &g.members })
            .collect();
        let usage = app
            .state
            .with_project_scenes(|scenes| scenes.bank_usage(self.track_ids.len(), &kits));
        // The same instances (and the same kits): push only the rows whose
        // banks changed.
        let racks: Vec<u64> = kits.iter().map(|kit| kit.group_id).collect();
        let last = previous
            .filter(|key| {
                key.track_ids == self.track_ids
                    && key.topology.same_instances(self)
                    && key.topology.groups.iter().filter(|g| g.rack).map(|g| g.id).eq(racks.iter().copied())
            })
            .map(|key| key.usage);
        let changed = |last: Option<&BankMask>, mask: &BankMask| last != Some(mask);
        for (track, id) in self.track_ids.iter().enumerate() {
            let mask = &usage.tracks[track];
            let before = last.as_ref().and_then(|last| last.tracks.get(track));
            if let (Some(id), true) = (*id, changed(before, mask)) {
                pusher.push(id, f::TRACK_USED_BANKS, bank_list(mask, &self.bank_ids));
            }
        }
        for (kit, group_id) in racks.iter().enumerate() {
            let mask = &usage.kits[kit];
            let before = last.as_ref().and_then(|last| last.kits.get(kit));
            if let (Some(id), true) = (self.groups.get(group_id).copied(), changed(before, mask)) {
                pusher.push(id, f::GROUP_USED_BANKS, bank_list(mask, &self.bank_ids));
            }
        }
        self.bank_usage_key = Some(BankUsageKey {
            topology: TopologyKey::read(app, self),
            track_ids: self.track_ids.clone(),
            usage,
        });
    }

    /// How many times the bank usage has been recomputed (a sync with
    /// nothing changed must not recompute).
    #[cfg(test)]
    pub(crate) fn bank_usage_passes(&self) -> u64 {
        self.bank_usage_passes
    }
}
