//! Devices beyond the track chain (spec §14.2f, stage 7b-2): a track's
//! MIDI effects (`t.midi-devices`), a bus's effects (`b.devices`), a drum
//! rack's slots (the rack instrument device's `devices`) and each slot's
//! effects (the slot device's `devices`). They are `device` instances like
//! the chain's, with the same `param`s.
//!
//! **Identity.** A device is keyed (owner instance id, `did`): under its
//! track (MIDI effects, rack slots, rack slot effects) or its bus (bus
//! effects; `device` is keyed under either, `:key ((track bus) did)`). The
//! `did` is the identity the device registry binds (a MIDI effect's, a rack
//! slot's, a rack slot effect's or a bus effect's instance id, all from one
//! allocator and unique across families), so a reorder keeps the instance
//! and its params (only `slot` moves); a device with none bound yet uses a
//! placeholder of its family and position ([`DeviceSlot::unbound_did`]).
//! When the registry allocates an identity for an unbound device it
//! records the placeholder it came from, and the instance is re-keyed to
//! exactly that identity, not replaced ([`reconcile_devices`], shared with
//! the track chain); an identity with no record (or two claiming one
//! placeholder) replaces the instance instead. A descriptor change
//! replaces the params ([`HostKinds::sync_device_source`]); a deleted
//! device, track or bus, or a project load, drops it.
//!
//! **Feeds.** One pass per family, each behind its own key compared in
//! place (and updated only when it moved): MIDI effects on the FX, UI and
//! pattern epochs (every chain edit, undo, redo, scene switch), the
//! content library epoch, the registry's generation and the tracks; bus
//! effects on the FX and UI epochs, the registry's generation and the
//! buses; rack slots and their effects on the registry's generation, the
//! tracks and the chain devices (a rack's instrument device holds its
//! slots), and the rack revision (any rack edit, `RevisionedMutex`), where
//! a rack whose layout fingerprint (its slots, their instruments, names,
//! switches and voices, their effects) is unchanged is skipped: a knob drag
//! moves the rack revision and does no device work. None reads the history
//! revision. MIDI effect descriptors come from a cache ([`DeviceState`])
//! that reloads the library only when the content library epoch moves.
//! Rack slot devices are synced under the rack lock (nothing in a push
//! reads the rack). `voices` (a rack slot's max polyphony) is a model field
//! of that pass; `delete-target` is live (the device sync's observed list).
//!
//! **Strip controls** (eseq-0l17.42). A rack slot's `gain`, `pan`, `muted`,
//! `soloed` (each with its `-display` and `-locked`) and `choke` are live
//! fields: the device live loop collects the observed ones and reads them
//! all under one rack lock per tick ([`DeviceState::push_strips`]); a cold
//! read takes the lock for its one field ([`device_strip_field`]). The
//! shown value is `rack_slot_control_value` (shared with the legacy rack
//! panel fields) at the displayed step (the current track's selected step,
//! else its playing step).

use super::*;
use crate::host_commands::StripControl;
use sequencer::effects::EffectDescriptor;
use std::hash::{Hash, Hasher};

/// The device sync's state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct DeviceState {
    midi_key: Option<FamilyKey<(usize, usize, u64, u64, u64)>>,
    rack_key: Option<FamilyKey<u64>>,
    rack_revision: Option<u64>,
    bus_key: Option<FamilyKey<(usize, usize, u64)>>,
    /// Each family's device instances, for the live loop.
    midi_ids: Vec<InstanceId>,
    /// By track position: the rack's layout fingerprint and devices as of
    /// its last pass.
    racks: Vec<RackDevices>,
    bus_ids: Vec<InstanceId>,
    /// The MIDI effect library's descriptors and the content library epoch
    /// they were loaded at.
    midi_fx: Option<(u64, Rc<[EffectDescriptor]>)>,
    /// Ticks on which some family pass did device work, for tests.
    pub(crate) syncs: u64,
    /// Racks the rack pass re-synced (a skipped rack counts nothing), for
    /// tests.
    pub(crate) racks_synced: u64,
    /// MIDI effect library loads, for tests.
    pub(crate) midi_fx_loads: u64,
    /// The observed strip fields this tick, and the values read under the
    /// rack lock, pushed once it is released (both reused across ticks).
    strip_work: Vec<StripWork>,
    strip_pending: Vec<(InstanceId, FieldKey, Value)>,
    /// Rack locks the strip fields took, for tests (one per tick at most).
    pub(crate) strip_locks: u64,
}

/// One observed device's strip fields to read this tick.
struct StripWork {
    id: InstanceId,
    /// The device's track.
    track: usize,
    device: DeviceSlot,
    /// The displayed step (read before the rack lock).
    step: Option<usize>,
    /// The observed strip bits.
    mask: u32,
}

/// One rack's devices as of its last pass.
#[derive(Default)]
struct RackDevices {
    /// [`rack_layout`]; `None` when the track holds no rack.
    layout: Option<u64>,
    /// The rack's slot and slot effect devices.
    ids: Vec<InstanceId>,
}

impl DeviceState {
    /// Sync every family at the next tick (a schema change, a hot reload).
    pub(super) fn invalidate(&mut self) {
        self.midi_key = None;
        self.rack_key = None;
        self.rack_revision = None;
        self.bus_key = None;
    }

    /// Every device instance this sync owns.
    pub(super) fn ids(&self) -> impl Iterator<Item = InstanceId> + '_ {
        let racks = self.racks.iter().flat_map(|rack| &rack.ids);
        (self.midi_ids.iter().chain(racks).chain(&self.bus_ids)).copied()
    }

    /// The MIDI effect library's descriptors, loaded once per content
    /// library epoch (a library change reloads them); never from disk
    /// otherwise. Shared by the device sync and the MIDI effect setters.
    pub(crate) fn midi_fx_descriptors(&mut self) -> Rc<[EffectDescriptor]> {
        let epoch = crate::content_library_epoch();
        if let Some((loaded, descriptors)) = &self.midi_fx {
            if *loaded == epoch {
                return descriptors.clone();
            }
        }
        self.midi_fx_loads += 1;
        let descriptors: Rc<[EffectDescriptor]> =
            sequencer::lisp_host::load_midi_fx_descriptors().into();
        self.midi_fx = Some((epoch, descriptors.clone()));
        descriptors
    }
}

/// What one family pass derives from: scalar counters and the owner (and,
/// for racks, chain device) instances.
#[derive(PartialEq)]
struct FamilyKey<S> {
    scalars: S,
    owners: Vec<Option<InstanceId>>,
    chain: Vec<InstanceId>,
}

impl<S: PartialEq + Copy> FamilyKey<S> {
    /// Whether `key` moved from what it holds to (`scalars`, `owners`,
    /// `chain`); when it did, it is updated in place (no allocation while
    /// the lists keep their length).
    fn moved(
        key: &mut Option<Self>,
        scalars: S,
        owners: &[Option<InstanceId>],
        chain: &[InstanceId],
    ) -> bool {
        if let Some(key) = key {
            if key.scalars == scalars && key.owners == owners && key.chain == chain {
                return false;
            }
            key.scalars = scalars;
            key.owners.clear();
            key.owners.extend_from_slice(owners);
            key.chain.clear();
            key.chain.extend_from_slice(chain);
            return true;
        }
        *key = Some(Self {
            scalars,
            owners: owners.to_vec(),
            chain: chain.to_vec(),
        });
        true
    }
}

/// The instance a device hangs off.
#[derive(Clone, Copy)]
pub(super) enum Parent {
    Track(InstanceId),
    Bus(InstanceId),
}

impl Parent {
    fn id(self) -> InstanceId {
        match self {
            Self::Track(id) | Self::Bus(id) => id,
        }
    }
}

/// One device's model fields.
pub(super) struct DeviceModel<'a> {
    pub(super) device: DeviceSlot,
    pub(super) did: u64,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) enabled: bool,
    /// The device's descriptor (`None`: no params).
    pub(super) desc: Option<&'a EffectDescriptor>,
    /// A sampler instrument (its lane depths are DSP units).
    pub(super) sampler: bool,
    /// The device whose `devices` holds this one (a rack slot's rack, a
    /// rack slot effect's slot).
    pub(super) container: Option<InstanceId>,
    /// A rack slot's voices; 0 otherwise.
    pub(super) voices: usize,
}

/// Bring the devices under `parent` that `family` owns in line with
/// `wanted` (each device's `did` and position), in order: re-key a
/// placeholder device to the identity bound from it, drop the gone ones and
/// register the missing ones. `placeholder` answers the placeholder `did`
/// an identity was bound from ([`DeviceSlot::placeholder_did`]); a wanted
/// identity whose placeholder instance is there (and no longer wanted) takes
/// that instance over, unless another wanted identity claims the same
/// placeholder (then both are new instances). Devices of other families
/// under the same parent are left alone (they have passes of their own),
/// told apart by their [`DeviceSource`]. Returns the instance of each
/// wanted device (`None` where registering failed).
pub(super) fn reconcile_devices(
    pusher: &mut Pusher<'_>,
    parent: InstanceId,
    wanted: &[(u64, DeviceSlot)],
    family: impl Fn(DeviceSlot) -> bool,
    placeholder: impl Fn(u64) -> Option<u64>,
) -> Vec<Option<InstanceId>> {
    let is_wanted = |did: u64| wanted.iter().any(|(want, _)| *want == did);
    let ours = |pusher: &Pusher<'_>, id: InstanceId| {
        let shared = pusher.shared.borrow();
        (shared.devices.get(&id)).is_none_or(|source| family(source.device))
    };
    let claims: Vec<(u64, u64)> = wanted
        .iter()
        .filter(|(did, _)| *did < UNBOUND_EFFECT_DID)
        .filter(|(did, _)| pusher.rt.keyed_instance(DEVICE, &[parent, *did]).is_none())
        .filter_map(|(did, _)| Some((*did, placeholder(*did)?)))
        .filter(|(_, from)| !is_wanted(*from))
        .collect();
    let moves: Vec<(InstanceId, Vec<u64>)> = claims
        .iter()
        .filter(|(_, from)| claims.iter().filter(|(_, other)| other == from).count() == 1)
        .filter_map(|(did, from)| {
            let id = pusher.rt.keyed_instance(DEVICE, &[parent, *from])?;
            ours(pusher, id).then(|| (id, vec![parent, *did]))
        })
        .collect();
    if !moves.is_empty() && pusher.rt.rekey_instances(&moves).is_ok() {
        pusher.changed = true;
    }
    let gone: Vec<InstanceId> = (pusher.rt.keyed_children_of_kind(parent, DEVICE))
        .filter(|(_, key)| !is_wanted(key.get(1).copied().unwrap_or(0)))
        .map(|(id, _)| id)
        .collect();
    for id in gone {
        if ours(pusher, id) {
            pusher.rt.drop_instance(id);
            pusher.changed = true;
        }
    }
    wanted
        .iter()
        .map(|(did, _)| {
            let key = [parent, *did];
            if let Some(id) = pusher.rt.keyed_instance(DEVICE, &key) {
                return Some(id);
            }
            pusher.changed = true;
            pusher.rt.register_keyed_instance(DEVICE, &key).ok()
        })
        .collect()
}

/// Push one device's model fields and keep its [`DeviceSource`] current;
/// returns whether its params were replaced.
fn push_device(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    id: InstanceId,
    parent: Parent,
    owner: usize,
    model: DeviceModel<'_>,
) -> bool {
    let (track, bus) = match parent {
        Parent::Track(track) => (Value::Instance(track), Value::Nil),
        Parent::Bus(bus) => (Value::Nil, Value::Instance(bus)),
    };
    pusher.push(id, f::DEVICE_TRACK, track);
    pusher.push(id, f::DEVICE_BUS, bus);
    pusher.push(id, f::DEVICE_SLOT, number(model.device.chain_slot() as f64));
    pusher.push(id, f::DEVICE_DID, number(model.did as f64));
    pusher.push(id, f::DEVICE_ROLE, text(model.device.role()));
    pusher.push(id, f::DEVICE_TYPE, Value::String(model.kind));
    pusher.push(id, f::DEVICE_NAME, Value::String(model.name));
    pusher.push(id, f::DEVICE_ENABLED, Value::Bool(model.enabled));
    pusher.push(id, f::DEVICE_CONTAINER, instance_or_nil(model.container));
    pusher.push(id, f::DEVICE_VOICES, number(model.voices as f64));
    let desc = (model.desc, model.sampler);
    HostKinds::sync_device_source(pusher, app, id, owner, model.device, desc)
}

/// The placeholder lookup [`reconcile_devices`] takes for the devices
/// under `parent` (at position `owner`).
fn placeholder_of(
    app: &app::App,
    parent: Parent,
    owner: usize,
) -> impl Fn(u64) -> Option<u64> + '_ {
    let bus = matches!(parent, Parent::Bus(_));
    move |did| DeviceSlot::placeholder_did(app, bus, owner, did)
}

/// Register, push and keep the devices of one family under `parent` (at
/// position `owner`); returns their instances in order and whether params
/// were replaced. Shared by every family pass and the track chain.
pub(super) fn sync_family(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    parent: Parent,
    owner: usize,
    models: Vec<DeviceModel<'_>>,
    family: impl Fn(DeviceSlot) -> bool,
) -> (Vec<InstanceId>, bool) {
    let wanted: Vec<(u64, DeviceSlot)> = models.iter().map(|m| (m.did, m.device)).collect();
    let placeholder = placeholder_of(app, parent, owner);
    let ids = reconcile_devices(pusher, parent.id(), &wanted, family, placeholder);
    let mut replaced = false;
    let ids = ids
        .into_iter()
        .zip(models)
        .filter_map(|(id, model)| {
            let id = id?;
            replaced |= push_device(pusher, app, id, parent, owner, model);
            Some(id)
        })
        .collect();
    (ids, replaced)
}

/// A snapshot chain effect (a bus's, a rack slot's) as a device model.
fn effect_model<'a>(
    device: DeviceSlot,
    did: u64,
    entry: DeviceChainEntry,
    descriptors: &'a [EffectDescriptor],
    container: Option<InstanceId>,
) -> DeviceModel<'a> {
    DeviceModel {
        device,
        did,
        kind: entry.name.clone(),
        name: entry.name,
        enabled: entry.enabled,
        desc: descriptors.get(entry.slot as usize),
        sampler: false,
        container,
        voices: 0,
    }
}

/// A rack's layout fingerprint: everything the rack pass pushes from the
/// rack (its slots, their instruments, names, switches, voices and
/// descriptors, their effects and those effects' switches), so a value
/// edit (a knob drag) leaves it alone. Allocates nothing but the slot
/// names.
fn rack_layout(app: &app::App, rack: &sequencer::sequencer::RackTrackSnapshot) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rack.slots.len().hash(&mut hasher);
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        instrument_type_label(slot.instrument_type).hash(&mut hasher);
        rack_slot_raw_name(app, slot_idx, slot).hash(&mut hasher);
        (slot.enabled, slot.max_polyphony).hash(&mut hasher);
        let descriptor = app.rack_slot_descriptor(slot);
        descriptor
            .map(|desc| (desc.params.as_ptr() as usize, desc.params.len()))
            .hash(&mut hasher);
        for (effect_slot, desc) in slot.effect_descriptors.iter().enumerate() {
            let values = slot
                .effect_slots
                .get(effect_slot)
                .map(DeviceValues::Snapshot);
            (&desc.name, desc.params.len()).hash(&mut hasher);
            effect_enabled(desc, values).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// What one family pass did: whether params were replaced, whether its
/// instance list changed, whether it did any device work.
#[derive(Default)]
struct PassOutcome {
    replaced: bool,
    ids_changed: bool,
    worked: bool,
}

impl PassOutcome {
    fn merge(&mut self, other: Self) {
        self.replaced |= other.replaced;
        self.ids_changed |= other.ids_changed;
        self.worked |= other.worked;
    }
}

impl HostKinds {
    /// The device sync (see the module docs): every track's MIDI effects
    /// (`t.midi-devices`), every drum rack's slots and their effects, every
    /// bus's effects (`b.devices`), each family when its key moved.
    pub(super) fn sync_device_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        // A hot reload drops every instance: one representative tells.
        if (self.devices.ids().next()).is_some_and(|id| !pusher.rt.instance_is_live(id)) {
            self.devices.invalidate();
        }
        let sources = pusher.sources;
        let fx_epoch = sources.fx_epoch.load(Ordering::Relaxed);
        let ui_epoch = sources.ui_epoch.load(Ordering::Relaxed);
        let registry = app.device_registry.generation();
        let devices = &mut self.devices;
        let mut outcome = PassOutcome::default();
        let midi = (
            fx_epoch,
            ui_epoch,
            app.state.transport.pattern_epoch.load(Ordering::Relaxed),
            crate::content_library_epoch(),
            registry,
        );
        if FamilyKey::moved(&mut devices.midi_key, midi, &self.track_ids, &[]) {
            outcome.merge(devices.sync_midi(pusher, app, &self.track_ids));
        }
        let all_racks = FamilyKey::moved(
            &mut devices.rack_key,
            registry,
            &self.track_ids,
            &self.device_ids,
        );
        let rack_revision = app.state.pattern.rack_tracks.revision();
        if all_racks || devices.rack_revision != Some(rack_revision) {
            devices.rack_revision = Some(rack_revision);
            outcome.merge(devices.sync_racks(pusher, app, &self.track_ids, all_racks));
        }
        let bus = (fx_epoch, ui_epoch, registry);
        if FamilyKey::moved(&mut devices.bus_key, bus, &self.bus_ids, &[]) {
            outcome.merge(devices.sync_buses(pusher, app, &self.bus_ids));
        }
        if outcome.worked {
            devices.syncs += 1;
        }
        if outcome.ids_changed {
            self.device_observed.reset();
            outcome.replaced = true;
        }
        if outcome.replaced {
            self.param_observed.reset();
        }
    }
}

impl DeviceState {
    /// Every track's MIDI effects; the library's descriptors only when
    /// some track has one ([`Self::midi_fx_descriptors`]).
    fn sync_midi(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        track_ids: &[Option<InstanceId>],
    ) -> PassOutcome {
        let mut outcome = PassOutcome {
            worked: true,
            ..PassOutcome::default()
        };
        let mut descriptors: Option<Rc<[EffectDescriptor]>> = None;
        let mut ids = Vec::new();
        for (track, id) in track_ids.iter().enumerate() {
            let Some(track_id) = *id else { continue };
            let Some(params) = app.state.pattern.track_params.get(track) else {
                continue;
            };
            let chain = params.midi_fx_chain();
            let descriptors = match chain.is_empty() {
                true => None,
                false => Some(descriptors.get_or_insert_with(|| self.midi_fx_descriptors())),
            };
            let library = descriptors.map_or(&[][..], |descriptors| &descriptors[..]);
            let slots = app.state.pattern.midi_fx_slots.get(track);
            let models = midi_fx_device_chain(&chain, library)
                .into_iter()
                .map(|(slot, desc)| {
                    let device = DeviceSlot::MidiFx(slot);
                    let values = slots.and_then(|s| s.get(slot)).map(DeviceValues::Live);
                    DeviceModel {
                        device,
                        did: device.did(app, track),
                        kind: desc.name.clone(),
                        name: desc.name.clone(),
                        enabled: effect_enabled(desc, values),
                        desc: Some(desc),
                        sampler: false,
                        container: None,
                        voices: 0,
                    }
                })
                .collect();
            let family = |device| matches!(device, DeviceSlot::MidiFx(_));
            let parent = Parent::Track(track_id);
            let (devices, replaced) = sync_family(pusher, app, parent, track, models, family);
            outcome.replaced |= replaced;
            ids.extend_from_slice(&devices);
            pusher.push(track_id, f::TRACK_MIDI_DEVICES, instance_list(devices));
        }
        outcome.ids_changed = ids != self.midi_ids;
        self.midi_ids = ids;
        outcome
    }

    /// Every drum rack's slots (the rack instrument device's `devices`)
    /// and each slot's effects (the slot device's `devices`), under the
    /// rack lock; an instrument device of a track that is no rack holds
    /// none. With `all` false, a rack whose [`rack_layout`] is unchanged is
    /// skipped.
    fn sync_racks(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        track_ids: &[Option<InstanceId>],
        all: bool,
    ) -> PassOutcome {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let mut outcome = PassOutcome::default();
        self.racks
            .resize_with(track_ids.len(), RackDevices::default);
        let family = |device| {
            matches!(
                device,
                DeviceSlot::RackSlot(_) | DeviceSlot::RackEffect { .. }
            )
        };
        for (track, id) in track_ids.iter().enumerate() {
            let Some(track_id) = *id else {
                // No track instance: no devices (its children went with it).
                self.racks[track] = RackDevices::default();
                continue;
            };
            let rack = racks.get(track).and_then(Option::as_ref);
            let layout = rack.map(|rack| rack_layout(app, rack));
            let cached = &mut self.racks[track];
            if !all && cached.layout == layout {
                continue;
            }
            cached.layout = layout;
            outcome.worked = true;
            self.racks_synced += 1;
            let instrument = pusher.rt.keyed_instance(DEVICE, &[track_id, 0]);
            let parent = Parent::Track(track_id);
            // One pass over the rack: each slot, then its effects; an
            // effect's model remembers its slot's index in `models` (its
            // container is the slot's instance, known after reconciling).
            let mut models = Vec::new();
            let mut holders: Vec<Option<usize>> = Vec::new();
            for (slot_idx, slot) in rack
                .into_iter()
                .flat_map(|rack| rack.slots.iter().enumerate())
            {
                let device = DeviceSlot::RackSlot(slot_idx);
                let holder = models.len();
                models.push(DeviceModel {
                    device,
                    did: device.did(app, track),
                    kind: instrument_type_label(slot.instrument_type).to_string(),
                    name: rack_slot_raw_name(app, slot_idx, slot),
                    enabled: slot.enabled,
                    desc: app.rack_slot_descriptor(slot),
                    sampler: slot.instrument_type == sequencer::sequencer::InstrumentType::Sampler,
                    container: instrument,
                    voices: slot.max_polyphony,
                });
                holders.push(None);
                for entry in rack_slot_effect_chain(slot) {
                    let effect = DeviceSlot::RackEffect {
                        rack_slot: slot_idx,
                        slot: entry.slot as usize,
                    };
                    let did = effect.did(app, track);
                    let descriptors = &slot.effect_descriptors;
                    models.push(effect_model(effect, did, entry, descriptors, None));
                    holders.push(Some(holder));
                }
            }
            let wanted: Vec<(u64, DeviceSlot)> = models.iter().map(|m| (m.did, m.device)).collect();
            let placeholder = placeholder_of(app, parent, track);
            let instances = reconcile_devices(pusher, track_id, &wanted, family, placeholder);
            // A slot's effects, by the slot's index in `models`.
            let mut children: HashMap<usize, Vec<InstanceId>> = HashMap::new();
            let mut slot_ids = Vec::new();
            let mut ids = Vec::new();
            for (at, (mut model, holder)) in models.into_iter().zip(holders).enumerate() {
                let Some(id) = instances[at] else { continue };
                match holder {
                    Some(slot_at) => {
                        model.container = instances[slot_at];
                        children.entry(slot_at).or_default().push(id);
                    }
                    None => slot_ids.push((at, id)),
                }
                outcome.replaced |= push_device(pusher, app, id, parent, track, model);
                ids.push(id);
            }
            for (at, slot_id) in &slot_ids {
                let effects = children.remove(at).unwrap_or_default();
                pusher.push(*slot_id, f::DEVICE_DEVICES, instance_list(effects));
            }
            let slot_ids = slot_ids.into_iter().map(|(_, id)| id);
            if let Some(instrument) = instrument {
                pusher.push(instrument, f::DEVICE_DEVICES, instance_list(slot_ids));
            }
            outcome.ids_changed |= ids != self.racks[track].ids;
            self.racks[track].ids = ids;
        }
        outcome
    }

    /// Every bus's effects (`b.devices`).
    fn sync_buses(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        bus_ids: &[Option<InstanceId>],
    ) -> PassOutcome {
        let mut outcome = PassOutcome {
            worked: true,
            ..PassOutcome::default()
        };
        let mut ids = Vec::new();
        for (bus, id) in bus_ids.iter().enumerate() {
            let (Some(bus_id), Some(channel)) = (*id, app.buses.get(bus)) else {
                continue;
            };
            let models = bus_device_chain(channel)
                .into_iter()
                .map(|entry| {
                    let device = DeviceSlot::BusEffect(entry.slot as usize);
                    let did = device.did(app, bus);
                    effect_model(device, did, entry, &channel.effect_descriptors, None)
                })
                .collect();
            let parent = Parent::Bus(bus_id);
            let (devices, replaced) = sync_family(pusher, app, parent, bus, models, |_| true);
            outcome.replaced |= replaced;
            ids.extend_from_slice(&devices);
            pusher.push(bus_id, f::BUS_DEVICES, instance_list(devices));
        }
        outcome.ids_changed = ids != self.bus_ids;
        self.bus_ids = ids;
        outcome
    }
}

/// What a strip field reads of its control: the slot's own value, the value
/// shown at the displayed step, or whether that step locks it.
#[derive(Clone, Copy)]
enum StripPart {
    Base,
    Display,
    Locked,
}

/// The strip fields: each with the strip control it reads and what it reads
/// of it.
const STRIP_FIELDS: [(FieldKey, StripControl, StripPart); 13] = [
    (f::DEVICE_GAIN, StripControl::Gain, StripPart::Base),
    (
        f::DEVICE_GAIN_DISPLAY,
        StripControl::Gain,
        StripPart::Display,
    ),
    (f::DEVICE_GAIN_LOCKED, StripControl::Gain, StripPart::Locked),
    (f::DEVICE_PAN, StripControl::Pan, StripPart::Base),
    (f::DEVICE_PAN_DISPLAY, StripControl::Pan, StripPart::Display),
    (f::DEVICE_PAN_LOCKED, StripControl::Pan, StripPart::Locked),
    (f::DEVICE_MUTED, StripControl::Mute, StripPart::Base),
    (
        f::DEVICE_MUTED_DISPLAY,
        StripControl::Mute,
        StripPart::Display,
    ),
    (
        f::DEVICE_MUTED_LOCKED,
        StripControl::Mute,
        StripPart::Locked,
    ),
    (f::DEVICE_SOLOED, StripControl::Solo, StripPart::Base),
    (
        f::DEVICE_SOLOED_DISPLAY,
        StripControl::Solo,
        StripPart::Display,
    ),
    (
        f::DEVICE_SOLOED_LOCKED,
        StripControl::Solo,
        StripPart::Locked,
    ),
    (f::DEVICE_CHOKE, StripControl::Choke, StripPart::Base),
];

/// The strip fields' keys.
pub(super) fn strip_keys() -> impl Iterator<Item = FieldKey> {
    STRIP_FIELDS.iter().map(|(key, ..)| *key)
}

/// Whether `key` is a strip field.
pub(super) fn is_strip_field(key: FieldKey) -> bool {
    STRIP_FIELDS.iter().any(|(strip, ..)| *strip == key)
}

/// Strip field `key` of rack slot `slot_idx` of `rack` (read under the rack
/// lock) at the displayed step `step`; `None` for another key. The base
/// value is the slot's stored one, unclamped (as `set-device` compares it),
/// so it round-trips as a no-op.
fn rack_strip_field(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    key: FieldKey,
    step: Option<usize>,
) -> Option<Value> {
    let &(_, control, part) = STRIP_FIELDS.iter().find(|(strip, ..)| *strip == key)?;
    let Some(param) = control.param() else {
        return Some(number(control.read(slot).number()));
    };
    Some(match part {
        StripPart::Base => rack_slot_control_reactive_value(param, control.read(slot).number()),
        StripPart::Display => rack_slot_control_reactive_value(
            param,
            rack_slot_control_value(rack, slot_idx, slot, param, step),
        ),
        StripPart::Locked => {
            Value::Bool(step.is_some_and(|step| slot.param_plocks.get(step, param).is_some()))
        }
    })
}

/// Strip field `key` of a device that is no rack slot: 0 or false.
fn no_strip_field(key: FieldKey) -> Option<Value> {
    let &(_, control, part) = STRIP_FIELDS.iter().find(|(strip, ..)| *strip == key)?;
    Some(match (control.param(), part) {
        (_, StripPart::Locked) => Value::Bool(false),
        (Some(param), _) => rack_slot_control_reactive_value(param, 0.0),
        (None, _) => number(0),
    })
}

/// Strip field `key` of `device` (the reader hook's cold read); `None`
/// while its slot is gone.
pub(super) fn device_strip_field(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    key: FieldKey,
) -> Option<Value> {
    let DeviceSlot::RackSlot(slot_idx) = device.device else {
        return no_strip_field(key);
    };
    let step = display_step(sources, shared, device.owner);
    with_rack_slot(&sources.state, device.owner, slot_idx, |rack, slot| {
        rack_strip_field(rack, slot_idx, slot, key, step)
    })
    .flatten()
}

impl DeviceState {
    /// Queue observed device `id`'s strip fields in `mask` (strip bits
    /// only) for [`Self::push_strips`].
    pub(super) fn queue_strips(
        &mut self,
        sources: &KindsHandles,
        shared: &RefCell<KindsShared>,
        id: InstanceId,
        device: &DeviceSource,
        mask: u32,
    ) {
        // The displayed step before the rack lock (it reads the selection).
        let step = match device.device {
            DeviceSlot::RackSlot(_) => display_step(sources, shared, device.owner),
            _ => None,
        };
        self.strip_work.push(StripWork {
            id,
            track: device.owner,
            device: device.device,
            step,
            mask,
        });
    }

    /// Read every queued strip field under one rack lock, then push them.
    pub(super) fn push_strips(&mut self, pusher: &mut Pusher<'_>) {
        if self.strip_work.is_empty() {
            return;
        }
        let pending = &mut self.strip_pending;
        let any_slot =
            (self.strip_work.iter()).any(|work| matches!(work.device, DeviceSlot::RackSlot(_)));
        {
            let racks = any_slot.then(|| pusher.sources.state.pattern.rack_tracks.lock().unwrap());
            if any_slot {
                self.strip_locks += 1;
            }
            for work in &self.strip_work {
                let StripWork {
                    id,
                    track,
                    device,
                    step,
                    mask,
                } = *work;
                for (bit, key) in DEVICE_LIVE.keys.iter().enumerate() {
                    if mask & (1 << bit) == 0 {
                        continue;
                    }
                    let value = match (device, &racks) {
                        (DeviceSlot::RackSlot(slot_idx), Some(racks)) => {
                            let rack = racks.get(track).and_then(Option::as_ref);
                            rack.and_then(|rack| {
                                let slot = rack.slots.get(slot_idx)?;
                                rack_strip_field(rack, slot_idx, slot, *key, step)
                            })
                        }
                        _ => no_strip_field(*key),
                    };
                    if let Some(value) = value {
                        pending.push((id, *key, value));
                    }
                }
            }
        }
        self.strip_work.clear();
        for (id, key, value) in pending.drain(..) {
            pusher.push_computed(id, key, value);
        }
    }
}
