//! The mixer: buses, groups, the bus mixer state and the meters.

use super::*;

/// The meter readings host kinds publish, from the tick's meter cache.
#[derive(Clone, Copy, Default)]
pub(crate) struct KindsMeters<'a> {
    /// Track meter levels by track position.
    pub(crate) tracks: &'a [f64],
    /// Bus meter levels by bus position.
    pub(crate) buses: &'a [f64],
    /// Master left and right.
    pub(crate) master: (f64, f64),
    /// Audio callback load, percent.
    pub(crate) cpu_load: f64,
}

impl HostKinds {
    /// The bus registry (by `BusId`), `index`, `bid`, `name` and
    /// `project.buses`. Returns false when the bus ids were not distinct
    /// this frame (nothing changed then).
    pub(super) fn sync_bus_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let model: Vec<u64> = app.buses.iter().map(|bus| bus.id.0).collect();
        if !distinct(&model) {
            return false;
        }
        let buses = reconcile(pusher, BUS, &mut self.buses, &model);
        for (index, id) in buses.iter().enumerate() {
            let Some(id) = *id else { continue };
            let bus = &app.buses[index];
            pusher.push(id, f::BUS_INDEX, number(index as f64));
            pusher.push(id, f::BUS_BID, number(bus.id.0 as f64));
            pusher.push(id, f::BUS_NAME, Value::String(bus.name.clone()));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(
                project,
                f::PROJECT_BUSES,
                instance_list(buses.iter().flatten().copied()),
            );
        }
        self.bus_ids = buses;
        true
    }

    /// Bus volume, mute and solo, compared every tick against what was
    /// last pushed: the `App` owns them and a fader drag moves no model
    /// counter; there are only a few buses.
    pub(super) fn sync_bus_mixer(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        self.bus_mixer.resize(self.bus_ids.len(), None);
        let buses = app.buses.iter().zip(&self.bus_ids).zip(&mut self.bus_mixer);
        for ((bus, id), last) in buses {
            let Some(id) = *id else { continue };
            let now = (bus.volume, bus.mute, bus.solo);
            if *last == Some(now) {
                continue;
            }
            *last = Some(now);
            pusher.push(id, f::BUS_VOLUME, number(bus.volume));
            pusher.push(id, f::BUS_MUTED, Value::Bool(bus.mute));
            pusher.push(id, f::BUS_SOLOED, Value::Bool(bus.solo));
        }
    }

    /// The observed live fields of every bus (`peak`); nothing while none
    /// is observed.
    pub(super) fn sync_bus_live(&mut self, pusher: &mut Pusher<'_>) {
        let ids = self.bus_ids.iter().flatten().copied();
        let observed = pusher.push_live_all(ids, &BUS_LIVE, &mut self.bus_observers);
        self.bus_peaks_observed = observed & BUS_LIVE.bit(f::BUS_PEAK) != 0;
    }

    /// The group registry (by group id), the group fields and
    /// `project.groups`. Returns, per position of `tracks` (the track
    /// instances), the group holding the track; `None` when the group ids
    /// were not distinct this frame (nothing changed then). Runs after the
    /// track registry (members are tracks) and the buses (a group's bus).
    pub(super) fn sync_group_model(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        tracks: &[Option<InstanceId>],
    ) -> Option<Vec<Option<InstanceId>>> {
        let model: Vec<u64> = app.groups.iter().map(|group| group.id).collect();
        if !distinct(&model) {
            return None;
        }
        let mut holders = vec![None; tracks.len()];
        let groups = reconcile(pusher, GROUP, &mut self.groups, &model);
        for (index, (group, id)) in app.groups.iter().zip(&groups).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::GROUP_INDEX, number(index as f64));
            pusher.push(id, f::GROUP_GID, number(group.id as f64));
            pusher.push(id, f::GROUP_NAME, Value::String(group.name.clone()));
            pusher.push(id, f::GROUP_COLOR, rgb3(themed_track_rgb(group.color)));
            pusher.push(id, f::GROUP_COLLAPSED, Value::Bool(group.collapsed));
            pusher.push(id, f::GROUP_RACK, Value::Bool(group.is_rack()));
            let members = group.members.iter().filter_map(|track| {
                let member = tracks.get(*track).copied().flatten()?;
                if let Some(holder) = holders.get_mut(*track) {
                    holder.get_or_insert(id);
                }
                Some(member)
            });
            let members: Vec<InstanceId> = members.collect();
            pusher.push(id, f::GROUP_TRACKS, instance_list(members));
            let bus = self.buses.get(&group.bus_id).copied();
            pusher.push(id, f::GROUP_BUS, instance_or_nil(bus));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(
                project,
                f::PROJECT_GROUPS,
                instance_list(groups.iter().flatten().copied()),
            );
        }
        self.group_ids = groups;
        Some(holders)
    }
}
