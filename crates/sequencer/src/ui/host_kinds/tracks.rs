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
            let listed = shared.devices.len();
            shared.devices.retain(|id, _| rt.instance_is_live(*id));
            if shared.devices.len() != listed {
                shared.devices_generation += 1;
            }
            shared.param_devices.retain(|id| rt.instance_is_live(*id));
            shared.effect_slots.clear();
            let slots = app.graph.effect_descriptors.iter().map(Vec::len);
            shared.effect_slots.extend(slots);
            if tracks != self.track_ids {
                // Renders are cached by track position.
                shared.plock_renders.clear();
            }
        }
        let accumulators = build_accumulator_names(app);
        let accumulators_changed = self.sync_accumulator_options(pusher, &accumulators);
        {
            let rt = &*pusher.rt;
            self.settings.retain(|id, _| rt.instance_is_live(*id));
            self.tunings.retain(|id, _| rt.instance_is_live(*id));
            self.bar_transposes.retain(|id, _| rt.instance_is_live(*id));
            self.step_params_in_use
                .retain(|id, _| rt.instance_is_live(*id));
            self.panel
                .track_variants
                .retain(|id, _| rt.instance_is_live(*id));
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
            pusher.push(
                id,
                f::TRACK_INSTRUMENT_ID,
                Value::String(track_instrument_id(app, track)),
            );
            pusher.push(id, f::TRACK_RACK, Value::Bool(racks[track]));
            let sends = sync_sends(pusher, app, id, &self.buses);
            self.send_ids.extend_from_slice(&sends);
            pusher.push(id, f::TRACK_SENDS, instance_list(sends));
            self.sync_track_settings(pusher, app, track, id, &accumulators, accumulators_changed);
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
            pusher.push(project, f::PROJECT_TRACKS, listed_instances(&tracks));
        }
        self.sync_route_model(pusher, app, &tracks);
        self.track_ids = tracks;
        holders.is_some()
    }

    /// The observed live fields of every track, then its steps.
    pub(super) fn sync_track_live(&mut self, pusher: &mut Pusher<'_>, selection_changed: bool) {
        let peak_bit = TRACK_LIVE.bit(f::TRACK_PEAK);
        let steps_bit = TRACK_LIVE.bit(f::TRACK_STEPS);
        let bars_bit = TRACK_LIVE.bit(f::TRACK_BAR_TRANSPOSES);
        let in_use_bit = TRACK_LIVE.bit(f::TRACK_STEP_PARAMS_IN_USE);
        let variants_bit = TRACK_LIVE.bit(f::TRACK_VARIANTS);
        let notes_bit = TRACK_LIVE.bit(f::TRACK_ACTIVE_NOTES);
        let mod_bits = TRACK_LIVE.bits(&f::TRACK_MOD_IN) | TRACK_LIVE.bit(f::TRACK_MOD_OUT_LEVEL);
        let mut peaks_observed = false;
        let mut mod_levels_observed = false;
        for index in 0..self.track_ids.len() {
            let (track, Some(id)) = (index, self.track_ids[index]) else {
                continue;
            };
            if !pusher.sources.track_exists(track) {
                continue;
            }
            let observed = pusher.push_live_except(
                id,
                &TRACK_LIVE,
                bars_bit | in_use_bit | variants_bit | notes_bit,
            );
            peaks_observed |= observed & peak_bit != 0;
            mod_levels_observed |= observed & mod_bits != 0;
            if observed & bars_bit != 0 {
                self.sync_bar_transposes(pusher, track, id);
            } else {
                self.bar_transposes.remove(&id);
            }
            if observed & in_use_bit != 0 {
                self.sync_step_params_in_use(pusher, track, id);
            } else {
                self.step_params_in_use.remove(&id);
            }
            if observed & variants_bit != 0 {
                self.sync_track_variants(pusher, track, id);
            } else {
                self.panel.track_variants.remove(&id);
            }
            if observed & notes_bit != 0 {
                self.sync_active_notes(pusher, track, id);
            } else {
                self.graphs.active_notes.remove(&id);
            }
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
        self.track_mod_levels_observed = mod_levels_observed;
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
    let sends = pusher.reconcile_children(track_id, SEND, &wanted);
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

/// Register, update and drop the chain device instances (the instrument
/// and the effects) of one track ([`sync_family`]); returns them in chain
/// order. Keyed (track id, [`DeviceSlot::did`]): a reorder keeps an
/// effect's instance (and its params), only `slot` moves; the track's other
/// devices (MIDI effects, rack slots) are the device sync's.
/// `params_replaced` is set when a device's params were replaced.
fn sync_devices(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    track: usize,
    track_id: InstanceId,
    params_replaced: &mut bool,
) -> Vec<InstanceId> {
    let instrument_type = || {
        let kind = app.graph.track_instrument_types.get(track);
        kind.map_or("empty", |kind| instrument_type_label(*kind))
    };
    let models = track_device_chain(app, &app.state, track)
        .into_iter()
        .map(|entry| {
            let device = DeviceSlot::from_chain_slot(entry.slot);
            // A chain device's descriptor is the `App`'s own (borrowed).
            let desc = match device.descriptor(app, track, &[]) {
                Some(std::borrow::Cow::Borrowed(desc)) => Some(desc),
                _ => None,
            };
            DeviceModel {
                device,
                did: device.did(app, track),
                kind: match device {
                    DeviceSlot::Instrument => instrument_type().to_string(),
                    _ => entry.name.clone(),
                },
                name: entry.name,
                enabled: entry.enabled,
                desc,
                sampler: device == DeviceSlot::Instrument && instrument_type() == "sampler",
                container: None,
                voices: 0,
            }
        })
        .collect();
    let family = |device| matches!(device, DeviceSlot::Instrument | DeviceSlot::Effect(_));
    let parent = Parent::Track(track_id);
    let (devices, replaced) = sync_family(pusher, app, parent, track, models, family);
    *params_replaced |= replaced;
    devices
}
