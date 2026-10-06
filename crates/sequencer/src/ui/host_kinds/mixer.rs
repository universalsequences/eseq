//! The mixer: buses, groups, the bus mixer state and the meters.

use super::*;

/// The meter readings host kinds publish, from the tick's meter cache.
#[derive(Clone, Copy)]
pub(crate) struct KindsMeters<'a> {
    /// Track meter levels by track position.
    pub(crate) tracks: &'a [f64],
    /// Bus meter levels by bus position.
    pub(crate) buses: &'a [f64],
    /// Master left and right.
    pub(crate) master: (f64, f64),
    /// Audio callback load, percent.
    pub(crate) cpu_load: f64,
    /// The mod port levels (`t.mod-in-1`, …).
    pub(crate) mod_ports: &'a ModPortLevels,
    /// Whether the audio-overload warning shows.
    pub(crate) overloaded: bool,
    /// Drum rack pad lights by track position (`pad.triggered`).
    pub(crate) pad_triggers: &'a [bool],
    /// The modulation display sample (`param.mod-offset`, …,
    /// `device.mod-phases`): the tick polls it while the fx panel shows or
    /// a kind field observes it (`HostKinds::wants_mod_display`).
    pub(crate) mod_display: &'a ModDisplayValues,
}

/// No modulation sample, for [`KindsMeters::default`].
static NO_MOD_DISPLAY: ModDisplayValues = ModDisplayValues {
    effects: Vec::new(),
    instrument: None,
    rack_slot: None,
};

/// No mod port levels, for [`KindsMeters::default`].
static NO_MOD_PORTS: ModPortLevels = ModPortLevels {
    track_inputs: Vec::new(),
    track_outputs: Vec::new(),
    bus_inputs: Vec::new(),
};

impl Default for KindsMeters<'_> {
    fn default() -> Self {
        Self {
            tracks: &[],
            buses: &[],
            master: (0.0, 0.0),
            cpu_load: 0.0,
            mod_ports: &NO_MOD_PORTS,
            overloaded: false,
            pad_triggers: &[],
            mod_display: &NO_MOD_DISPLAY,
        }
    }
}

impl HostKinds {
    /// The bus registry (by `BusId`), `index`, `bid`, `name`, `output`,
    /// `output-options`, `project.buses` and `project.output-options`.
    /// Returns false when the bus ids were not distinct this frame (nothing
    /// changed then).
    pub(super) fn sync_bus_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let model: Vec<u64> = app.buses.iter().map(|bus| bus.id.0).collect();
        if !distinct(&model) {
            return false;
        }
        let buses = reconcile(pusher, BUS, &mut self.buses, &model);
        let instance = |bus: sequencer::sequencer::BusId| self.buses.get(&bus.0).copied();
        for (index, id) in buses.iter().enumerate() {
            let Some(id) = *id else { continue };
            let bus = &app.buses[index];
            pusher.push(id, f::BUS_INDEX, number(index as f64));
            pusher.push(id, f::BUS_BID, number(bus.id.0 as f64));
            pusher.push(id, f::BUS_NAME, Value::String(bus.name.clone()));
            // The main mix feeds nothing.
            let output = (bus.id != sequencer::sequencer::BusId::MIX)
                .then(|| instance(bus_output_destination(bus)))
                .flatten();
            pusher.push(id, f::BUS_OUTPUT, instance_or_nil(output));
            let options = app.bus_output_options(bus.id).into_iter();
            let options = instance_list(options.filter_map(instance));
            pusher.push(id, f::BUS_OUTPUT_OPTIONS, options);
        }
        pusher.shared.borrow_mut().bus_ids = model;
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(project, f::PROJECT_BUSES, listed_instances(&buses));
        }
        self.sync_output_options(pusher, &buses);
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

    /// The observed live fields of every bus (`peak`, the mod inputs);
    /// nothing while none is observed.
    pub(super) fn sync_bus_live(&mut self, pusher: &mut Pusher<'_>) {
        let ids = self.bus_ids.iter().flatten().copied();
        let observed = pusher.push_live_all(ids, &BUS_LIVE, &mut self.bus_observers);
        self.bus_peaks_observed = observed & BUS_LIVE.bit(f::BUS_PEAK) != 0;
        self.bus_mod_levels_observed = observed & BUS_LIVE.bits(&f::BUS_MOD_IN) != 0;
    }

    /// The mod routes (`route`, keyed by their endpoints' stable ids:
    /// [`RouteKey`]), their fields and `project.routes`. Runs after the
    /// tracks (`tracks`, their instances by position) and buses. A route
    /// naming a track the registry lacks, or repeating another's endpoints,
    /// gets no instance.
    pub(super) fn sync_route_model(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        tracks: &[Option<InstanceId>],
    ) {
        let track_ids = app.track_registry.ids();
        let mut model = Vec::new();
        let mut keys = Vec::new();
        let mut connections = Vec::new();
        for connection in app.state.current_mod_connections() {
            let Some(key) = RouteKey::of(&connection, track_ids) else {
                continue;
            };
            let id = *self.route_keys.entry(key).or_insert_with(|| {
                self.next_route_id += 1;
                self.next_route_id
            });
            if !model.contains(&id) {
                model.push(id);
                keys.push(key);
                connections.push(connection);
            }
        }
        // A route gone from the model loses its instance below; its key goes
        // too (ids are never reused).
        self.route_keys.retain(|key, _| keys.contains(key));
        let routes = reconcile(pusher, ROUTE, &mut self.routes, &model);
        let mut sources = HashMap::new();
        for (index, (connection, id)) in connections.iter().zip(&routes).enumerate() {
            let Some(id) = *id else { continue };
            sources.insert(id, *connection);
            let track = |track: usize| tracks.get(track).copied().flatten();
            let (dest, dest_bus) = match connection.destination {
                sequencer::sequencer::ModDestination::Track(dest) => (track(dest), None),
                sequencer::sequencer::ModDestination::Bus(bus) => {
                    (None, self.buses.get(&bus.0).copied())
                }
            };
            pusher.push(id, f::ROUTE_INDEX, number(index as f64));
            let source = track(connection.source_track);
            pusher.push(id, f::ROUTE_SOURCE, instance_or_nil(source));
            pusher.push(id, f::ROUTE_DEST, instance_or_nil(dest));
            pusher.push(id, f::ROUTE_DEST_BUS, instance_or_nil(dest_bus));
            // 1-4, as the `mod-in-N` fields name the inputs.
            pusher.push(
                id,
                f::ROUTE_INPUT,
                number(connection.dest_input as f64 + 1.0),
            );
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            let list = listed_instances(&routes);
            pusher.push(project, f::PROJECT_ROUTES, list);
        }
        pusher.shared.borrow_mut().routes = sources;
        if routes != self.route_ids {
            self.route_observed.reset();
        }
        self.route_ids = routes;
    }

    /// The observed route fields (`selected`), from a list kept per
    /// observer epoch ([`ObservedList`]).
    pub(super) fn sync_route_live(&mut self, pusher: &mut Pusher<'_>) {
        let ids = &self.route_ids;
        let names = &ROUTE_LIVE.names;
        let ids = || ids.iter().flatten().copied().collect();
        self.route_observed.refresh(pusher.rt, names, ids);
        self.route_observed.push_masked(pusher, &ROUTE_LIVE);
    }

    /// The observed group fields (`armed`, `delete-target`), from a list
    /// kept per observer epoch ([`ObservedList`]); `delete-target` only
    /// when the delete target's version (or the observers) moved.
    pub(super) fn sync_group_live(&mut self, pusher: &mut Pusher<'_>) {
        let ids = &self.group_ids;
        let names = &GROUP_LIVE.names;
        let ids = || ids.iter().flatten().copied().collect();
        self.group_observed.refresh(pusher.rt, names, ids);
        let seen = (
            (pusher.sources.active_delete_target_version).load(Ordering::Relaxed),
            pusher.rt.instance_observer_epoch(),
        );
        let skip = if self.group_delete_target_seen == Some(seen) {
            GROUP_LIVE.bit(f::GROUP_DELETE_TARGET)
        } else {
            0
        };
        self.group_delete_target_seen = Some(seen);
        for &(id, mask, _) in &self.group_observed.entries {
            pusher.push_live_masked(id, &GROUP_LIVE, mask & !skip);
        }
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
            // Nesting, both ways (as `SEQ.groups` :rack-members / :parent).
            let racks = group.rack_members.iter();
            let racks = racks.filter_map(|gid| self.groups.get(gid).copied());
            pusher.push(id, f::GROUP_RACKS, instance_list(racks));
            let parent = sequencer::project::rack_parent(&app.groups, group.id)
                .and_then(|parent| self.groups.get(&app.groups[parent].id).copied());
            pusher.push(id, f::GROUP_PARENT, instance_or_nil(parent));
        }
        pusher.shared.borrow_mut().group_gids = model;
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(project, f::PROJECT_GROUPS, listed_instances(&groups));
        }
        if groups != self.group_ids {
            self.group_observed.reset();
        }
        self.group_ids = groups;
        Some(holders)
    }
}

/// A mod route's identity: its endpoints by stable id (the source's
/// `TrackId`, the destination's `TrackId` or `BusId`) and the input.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RouteKey {
    source: u64,
    dest: RouteDest,
    input: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RouteDest {
    Track(u64),
    Bus(u64),
}

impl RouteKey {
    /// The key of `connection` (track positions) under the registry's
    /// order `track_ids`; `None` when a track is out of range.
    fn of(
        connection: &sequencer::sequencer::ModConnection,
        track_ids: &[sequencer::sequencer::TrackId],
    ) -> Option<Self> {
        let dest = match connection.destination {
            sequencer::sequencer::ModDestination::Track(track) => {
                RouteDest::Track(track_ids.get(track)?.0)
            }
            sequencer::sequencer::ModDestination::Bus(bus) => RouteDest::Bus(bus.0),
        };
        Some(Self {
            source: track_ids.get(connection.source_track)?.0,
            dest,
            input: connection.dest_input,
        })
    }
}
