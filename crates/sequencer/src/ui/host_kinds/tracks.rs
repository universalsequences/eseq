//! Tracks: the track registry and model fields, their devices and sends,
//! and the per-tick track live fields.

use super::*;

impl HostKinds {
    /// Track registry and model fields (index, name, color, preset,
    /// devices, sends, group, `project.tracks`), with the groups. Returns
    /// false when the registry was not in step with the track list this
    /// frame (nothing changed then) or the group ids were not distinct.
    pub(super) fn sync_track_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let registry = &app.track_registry;
        if registry.len() != app.tracks.len() {
            return false;
        }
        let count = app.tracks.len().min(app.state.active_track_count());
        let model: Vec<u64> = registry.ids()[..count].iter().map(|id| id.0).collect();
        let tracks = reconcile(pusher, TRACK, &mut self.tracks, &model);
        self.steps.retain(|id, _| pusher.rt.instance_is_live(*id));
        let presets = track_loaded_presets(app, count);
        let racks: Vec<bool> = {
            let racks = app.state.pattern.rack_tracks.lock().unwrap();
            (0..count)
                .map(|track| racks.get(track).is_some_and(Option::is_some))
                .collect()
        };
        self.send_ids.clear();
        let previous_devices = std::mem::take(&mut self.device_ids);
        {
            let rt = &*pusher.rt;
            let mut shared = pusher.shared.borrow_mut();
            shared.devices.retain(|id, _| rt.instance_is_live(*id));
            shared.param_devices.retain(|id| rt.instance_is_live(*id));
            shared.effect_slots.clear();
            let slots = app.graph.effect_descriptors.iter().map(Vec::len);
            shared.effect_slots.extend(slots);
            if tracks != self.track_ids {
                // Renders are cached by track position.
                shared.plock_renders.clear();
            }
        }
        let mut params_replaced = false;
        for (track, id) in tracks.iter().enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::TRACK_INDEX, number(track as f64));
            let tid = registry.ids()[track].0;
            pusher.push(id, f::TRACK_TID, number(tid as f64));
            pusher.push(id, f::TRACK_NAME, Value::String(app.tracks[track].clone()));
            pusher.push(id, f::TRACK_COLOR, rgb(track_display_color(app, track)));
            pusher.push(id, f::TRACK_PRESET, Value::String(presets[track].clone()));
            let devices = sync_devices(pusher, app, track, id, &mut params_replaced);
            self.device_ids.extend_from_slice(&devices);
            pusher.push(id, f::TRACK_DEVICES, instance_list(devices));
            let instrument = app
                .graph
                .track_instrument_types
                .get(track)
                .map_or("empty", |kind| instrument_type_label(*kind));
            pusher.push(
                id,
                f::TRACK_INSTRUMENT_TYPE,
                Value::String(instrument.to_string()),
            );
            pusher.push(id, f::TRACK_RACK, Value::Bool(racks[track]));
            let sends = sync_sends(pusher, app, id, &self.buses);
            self.send_ids.extend_from_slice(&sends);
            pusher.push(id, f::TRACK_SENDS, instance_list(sends));
        }
        if self.device_ids != previous_devices {
            self.device_observed.reset();
            params_replaced = true;
        }
        if params_replaced {
            self.param_observed.reset();
        }
        let holders = self.sync_group_model(pusher, app, &tracks);
        if let Some(holders) = &holders {
            for (track, id) in tracks.iter().enumerate() {
                let Some(id) = *id else { continue };
                let group = holders.get(track).copied().flatten();
                pusher.push(id, f::TRACK_GROUP, instance_or_nil(group));
            }
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(
                project,
                f::PROJECT_TRACKS,
                instance_list(tracks.iter().flatten().copied()),
            );
        }
        self.track_ids = tracks;
        holders.is_some()
    }

    /// The observed live fields of every track, then its steps.
    pub(super) fn sync_track_live(&mut self, pusher: &mut Pusher<'_>, selection_changed: bool) {
        let peak_bit = TRACK_LIVE.bit(f::TRACK_PEAK);
        let steps_bit = TRACK_LIVE.bit(f::TRACK_STEPS);
        let mut peaks_observed = false;
        for (track, id) in self.track_ids.iter().enumerate() {
            let Some(id) = *id else { continue };
            if !pusher.sources.track_exists(track) {
                continue;
            }
            let observed = pusher.push_live(id, &TRACK_LIVE);
            peaks_observed |= observed & peak_bit != 0;
            let diff = self.steps.entry(id).or_default();
            sync_steps(
                pusher,
                diff,
                &self.selection,
                &mut self.step_changes,
                track,
                id,
                observed & steps_bit != 0,
                selection_changed,
            );
        }
        self.peaks_observed = peaks_observed;
    }

    /// The observed live fields of every send; nothing while no send
    /// field is observed.
    pub(super) fn sync_send_live(&mut self, pusher: &mut Pusher<'_>) {
        pusher.push_live_all(
            self.send_ids.iter().copied(),
            &SEND_LIVE,
            &mut self.send_observers,
        );
    }
}

/// Register, update and drop the send instances of one track: one per bus
/// but the main mix, keyed (track id, bus id); returns them in bus order.
fn sync_sends(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    track_id: InstanceId,
    buses: &HashMap<u64, InstanceId>,
) -> Vec<InstanceId> {
    let (wanted, bus_ids): (Vec<u64>, Vec<InstanceId>) = app
        .buses
        .iter()
        .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
        .filter_map(|bus| Some((bus.id.0, *buses.get(&bus.id.0)?)))
        .unzip();
    let sends = reconcile_children(pusher, track_id, SEND, &wanted);
    sends
        .into_iter()
        .zip(bus_ids)
        .filter_map(|(id, bus_id)| {
            let id = id?;
            pusher.push(id, f::SEND_TRACK, Value::Instance(track_id));
            pusher.push(id, f::SEND_BUS, Value::Instance(bus_id));
            Some(id)
        })
        .collect()
}

/// Register, update and drop the device instances of one track; returns
/// them in chain order. Keyed (track id, [`DeviceSlot::did`]): a reorder
/// keeps an effect's instance (and its params), only `slot` moves.
/// `params_replaced` is set when a device's params were replaced.
fn sync_devices(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    track: usize,
    track_id: InstanceId,
    params_replaced: &mut bool,
) -> Vec<InstanceId> {
    let chain = track_device_chain(app, &app.state, track);
    let slots: Vec<DeviceSlot> = chain
        .iter()
        .map(|entry| DeviceSlot::from_chain_slot(entry.slot))
        .collect();
    let wanted: Vec<u64> = slots.iter().map(|slot| slot.did(app, track)).collect();
    let devices = reconcile_children(pusher, track_id, DEVICE, &wanted);
    let instrument_type = || {
        let kind = app.graph.track_instrument_types.get(track);
        kind.map_or("empty", |kind| instrument_type_label(*kind))
    };
    chain
        .into_iter()
        .zip(slots)
        .zip(wanted.iter().zip(devices))
        .filter_map(|((entry, device), (did, id))| {
            let id = id?;
            let device_type = match device {
                DeviceSlot::Instrument => instrument_type().to_string(),
                DeviceSlot::Effect(_) => entry.name.clone(),
            };
            pusher.push(id, f::DEVICE_TRACK, Value::Instance(track_id));
            pusher.push(id, f::DEVICE_SLOT, number(device.chain_slot() as f64));
            pusher.push(id, f::DEVICE_DID, number(*did as f64));
            pusher.push(id, f::DEVICE_TYPE, Value::String(device_type));
            pusher.push(id, f::DEVICE_NAME, Value::String(entry.name));
            pusher.push(id, f::DEVICE_ENABLED, Value::Bool(entry.enabled));
            *params_replaced |= HostKinds::sync_device_source(pusher, app, id, track, device);
            Some(id)
        })
        .collect()
}
