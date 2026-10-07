//! Device panel extras (spec §14.2g, stage 7b-3): what the instrument and
//! effect panels show beyond a param's value.
//!
//! - **Placement** (`param.label`, `section`, `mod-slot`; model fields of
//!   the descriptor, pushed at registration, [`PanelSection`] shared with
//!   the panel builders) and `param.visible` (live: a modulation source's
//!   settings show only for the source type its slot picks,
//!   `selected_source_param_indices` over the displayed values).
//! - **Modulation lanes** (`param.mod-targets`, `mod-target`): registered
//!   with the params, from the descriptor's `instrument_modulation_targets`;
//!   a lane names its source and depth params as instances.
//! - **Modulation display** (`param.mod-offset`, `mod-value`, `mod-scale`,
//!   `device.mod-phases`): the tick's modulation sample (`ModDisplayValues`,
//!   copied into [`KindsShared`] while one of these fields is observed),
//!   which the tick polls while the fx panel shows or one of these fields
//!   of a device the sample covers is observed
//!   (`HostKinds::wants_mod_display`).
//!   The sample covers what the panel shows: every effect, the current
//!   track's instrument and its drum rack's selected slot; anything else
//!   reads no modulation (offset 0, value `param.value`, scale 1).
//! - **Process mapping** (`param.process-mapped`, `process-value`,
//!   `process-clamped`): a track instrument's params an enabled process slot
//!   writes (`process_bound_instrument_params`, cached per track under its
//!   [`PlockKey`]: a process chain edit moves it) and the scheduler's last
//!   write (`SequencerState::process_effective_param`). The track's sends
//!   a process writes (`send.process-mapped`) are cached the same way.
//! - **Key locks** (`param.key-locks`, `device.key-locked-notes`): a track
//!   instrument's ([`instrument_key_locks`], shared with the panel), cached
//!   per track under its [`PlockKey`] (a key-lock edit moves the fx and UI
//!   epochs).
//! - **Base note** (`device.base-note`): a track instrument's offset (an
//!   atomic; the setter is `set-device`'s), read with the strip controls
//!   (`devices::other_strip_field`; a rack slot's is its strip control).
//! - **Tensors** (`tensor`, keyed (device instance id, index)): registered
//!   with their device (`sync_device_source`); their cells are live, from
//!   the slot's tensor data at the displayed step (track instruments,
//!   effects and MIDI effects; a rack's or a bus's read their defaults),
//!   read per observed field into a reused buffer and compared in place.

use super::*;
use sequencer::effects::{InstrumentModulationTarget, ParamDescriptor};
use sequencer::instruments::voice_modulator::SLOT_COUNT;

/// Register the modulation lanes of `params` (a device's, in descriptor
/// order) as `mod-target`s keyed (param instance id, lane), and push each
/// param's `mod-targets`. Lanes whose depth param is missing are left out,
/// as the panels leave them out.
pub(super) fn register_mod_targets<S: KindStore>(
    store: &mut S,
    desc: &DeviceDescriptor,
    params: &[InstanceId],
) {
    let pdescs = &desc.desc.params;
    let param_at = |idx: usize| params.get(idx).copied();
    for (index, &param) in params.iter().enumerate() {
        let lanes: Vec<&InstrumentModulationTarget> = (desc.targets().iter())
            .filter(|lane| lane.base_param_idx == index && lane.depth_param_idx < pdescs.len())
            .collect();
        let targets = indexed_children(store, param, MOD_TARGET, lanes.len(), |store, id, at| {
            let lane = lanes[at];
            let depth = &pdescs[lane.depth_param_idx];
            // A % depth stored as a ratio reads (value, range) x 100 like
            // every % param (`depth-min` doc: the depth's display units).
            let (min, max) = if depth.is_percent() {
                (
                    depth.stored_to_user(lane.depth_min),
                    depth.stored_to_user(lane.depth_max),
                )
            } else {
                mod_target_depth_range(depth, lane, desc.sampler_depths)
            };
            let source = lane.source_param_idx.and_then(param_at);
            let unit = lane.depth_unit.clone().unwrap_or_default();
            store.push(id, f::MOD_TARGET_PARAM, Value::Instance(param));
            store.push(id, f::MOD_TARGET_INDEX, number(at as f64));
            store.push(id, f::MOD_TARGET_SOURCE, instance_or_nil(source));
            store.push(id, f::MOD_TARGET_SLOT, number(lane.modulator_slot as f64));
            store.push(
                id,
                f::MOD_TARGET_DEPTH,
                instance_or_nil(param_at(lane.depth_param_idx)),
            );
            store.push(id, f::MOD_TARGET_DEPTH_MIN, number(min));
            store.push(id, f::MOD_TARGET_DEPTH_MAX, number(max));
            store.push(id, f::MOD_TARGET_UNIT, Value::String(unit));
        });
        store.push(param, f::PARAM_MOD_TARGETS, instance_list(targets));
    }
}

/// Register `device`'s tensors (missing ones get their descriptor fields),
/// drop those past its tensor count, and push `device.tensors`.
pub(super) fn sync_device_tensors(
    pusher: &mut Pusher<'_>,
    device: InstanceId,
    source: &DeviceSource,
) {
    let tensors = source.desc.tensors();
    let ids = indexed_children(
        &mut *pusher.rt,
        device,
        TENSOR,
        tensors.len(),
        |store, id, at| {
            let tensor = &tensors[at];
            store.push(id, f::TENSOR_DEVICE, Value::Instance(device));
            store.push(id, f::TENSOR_INDEX, number(at as f64));
            store.push(id, f::TENSOR_NAME, Value::String(tensor.name.clone()));
            store.push(id, f::TENSOR_ROWS, number(tensor.rows() as f64));
            store.push(id, f::TENSOR_COLS, number(tensor.cols() as f64));
            store.push(id, f::TENSOR_MIN, number(tensor.min));
            store.push(id, f::TENSOR_MAX, number(tensor.max));
        },
    );
    pusher.push(device, f::DEVICE_TENSORS, instance_list(ids));
}

/// Whether `cell` already holds exactly `values` as a list of numbers.
pub(super) fn same_numbers<T: Copy + Into<f64>>(cell: &Value, values: &[T]) -> bool {
    match cell {
        Value::List(items) => {
            items.len() == values.len()
                && (items.iter().zip(values)).all(|(item, value)| {
                    matches!(&*item.borrow(), Value::Number(n) if *n == (*value).into())
                })
        }
        _ => false,
    }
}

pub(super) fn numbers<T: Copy + Into<f64>>(values: &[T]) -> Value {
    list_value(values.iter().map(|value| Value::Number((*value).into())))
}

impl Pusher<'_> {
    /// Push a list-of-numbers live field computed outside [`live_value`],
    /// counting it like one; allocates only when the cell differs.
    pub(super) fn push_numbers<T: Copy + Into<f64>>(
        &mut self,
        id: InstanceId,
        key: FieldKey,
        values: &[T],
    ) {
        if self.shared.borrow().skip.contains(&key) {
            return;
        }
        self.shared.borrow_mut().count(key);
        let same =
            (self.rt.instance_field(id, key.1)).is_ok_and(|cell| same_numbers(&cell, values));
        if !same {
            self.push(id, key, numbers(values));
        }
    }

    /// Push a list-of-numbers model field; allocates only when the cell
    /// differs (and counts nothing).
    pub(super) fn put_numbers(&mut self, id: InstanceId, key: FieldKey, values: &[f64]) {
        let same =
            (self.rt.instance_field(id, key.1)).is_ok_and(|cell| same_numbers(&cell, values));
        if !same {
            self.push(id, key, numbers(values));
        }
    }
}

/// Which cells of a tensor a read fills.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TensorPart {
    /// `values`: the p-lock at the displayed step, else the device's own.
    Shown,
    /// `base`: the device's own.
    Base,
    /// None (`locked` only).
    Locked,
}

/// Read tensor `index` of `device`: whether a p-lock at the displayed step
/// supplies its shown cells, and the cells `part` asks for written into
/// `out` (a reused buffer; nothing else allocates). A rack's or a bus's
/// device reads its descriptor defaults (no p-lock). `None` when the
/// device or the tensor is gone.
pub(super) fn read_tensor(
    sources: &KindsHandles,
    device: &DeviceSource,
    index: usize,
    part: TensorPart,
    out: &mut Vec<f32>,
) -> Option<bool> {
    let tensor = device.desc.tensors().get(index)?;
    let defaults = |out: &mut Vec<f32>| {
        out.clear();
        out.extend_from_slice(&tensor.default);
    };
    let owner = device.owner;
    let Some(slot) = device.device.slot_state(&sources.state, owner) else {
        if part != TensorPart::Locked {
            defaults(out);
        }
        return Some(false);
    };
    if !sources.track_exists(owner) {
        return None;
    }
    let data = &slot.tensor_params;
    let step = sources.plock_display_step(owner);
    let locked = step.is_some_and(|step| data.has_tensor_plock(step, index));
    let read = match part {
        TensorPart::Locked => return Some(locked),
        TensorPart::Shown if locked => data.read_cells_into(index, step, out),
        _ => data.read_cells_into(index, None, out),
    };
    if !read {
        defaults(out);
    }
    Some(locked)
}

/// One live field of a tensor (the reader hook's cold read).
pub(super) fn tensor_live_value(
    sources: &KindsHandles,
    device: &DeviceSource,
    index: usize,
    key: FieldKey,
) -> Option<Value> {
    let part = match key {
        f::TENSOR_VALUES => TensorPart::Shown,
        f::TENSOR_BASE => TensorPart::Base,
        f::TENSOR_LOCKED => TensorPart::Locked,
        _ => return None,
    };
    let mut cells = Vec::new();
    let locked = read_tensor(sources, device, index, part, &mut cells)?;
    Some(match part {
        TensorPart::Locked => Value::Bool(locked),
        _ => numbers(&cells),
    })
}

/// The graph node of an effect device (whose modulation sample is keyed by
/// node); `None` for the other families or an empty slot.
pub(super) fn effect_node(sources: &KindsHandles, device: &DeviceSource) -> Option<u32> {
    let owner = device.owner;
    let node = match device.device {
        DeviceSlot::Effect(slot) => {
            let chain = sources.state.pattern.effect_chains.get(owner)?;
            chain.get(slot)?.node_id.load(Ordering::Relaxed)
        }
        DeviceSlot::BusEffect(slot) => {
            let buses = sources.bus_state.lock().unwrap();
            buses.get(owner)?.effect_slots.get(slot)?.node_id
        }
        DeviceSlot::RackEffect { rack_slot, slot } => {
            with_rack_slot(&sources.state, owner, rack_slot, |_, rack_slot| {
                rack_slot
                    .effect_slots
                    .get(slot)
                    .map(|effect| effect.node_id)
            })??
        }
        _ => return None,
    };
    (node > 0).then_some(node)
}

/// Whether the tick's modulation sample covers `device` (every effect, the
/// current track's instrument and its drum rack's slot): only then does an
/// observed modulation field keep the tick polling.
pub(super) fn mod_sampled(sources: &KindsHandles, device: &DeviceSource) -> bool {
    match device.device {
        DeviceSlot::Effect(_) | DeviceSlot::BusEffect(_) | DeviceSlot::RackEffect { .. } => true,
        DeviceSlot::Instrument | DeviceSlot::RackSlot(_) => {
            device.owner == sources.current_track.load(Ordering::Relaxed)
        }
        DeviceSlot::MidiFx(_) => false,
    }
}

/// `read` over `device`'s entry of the modulation sample: its param values
/// and source phases, and whether they are stored units (an effect's) to
/// be shown in display units; `None` when nothing samples the device.
fn mod_sample<R>(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    read: impl FnOnce(&[ParamModValue], &[f64; SLOT_COUNT], bool) -> R,
) -> Option<R> {
    let shared = shared.borrow();
    let display = &shared.mod_display;
    let owner = device.owner;
    match device.device {
        DeviceSlot::Instrument => {
            let sample = (display.instrument.as_ref()).filter(|sample| sample.track == owner)?;
            Some(read(&sample.values, &sample.slot_phases, false))
        }
        DeviceSlot::RackSlot(slot) => {
            let sample = (display.rack_slot.as_ref())
                .filter(|sample| sample.track == owner && sample.slot_idx == slot)?;
            Some(read(&sample.values, &sample.slot_phases, false))
        }
        DeviceSlot::MidiFx(_) => None,
        _ => {
            let node = effect_node(sources, device)? as i32;
            let sample = display
                .effects
                .iter()
                .find(|sample| sample.node_id == node)?;
            Some(read(&sample.values, &sample.slot_phases, true))
        }
    }
}

/// Param `index`'s modulation now, as (offset, value, scale) in display
/// units; `None` while unmodulated or not sampled.
fn param_mod_display(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    pdesc: &ParamDescriptor,
    index: usize,
) -> Option<(f64, f64, f64)> {
    mod_sample(sources, shared, device, |values, _, stored| {
        let v = values.iter().find(|v| v.param_idx == index)?;
        let user = |value: f64| match stored {
            true => f64::from(pdesc.stored_to_user(value as f32)),
            false => value,
        };
        Some((user(v.offset), user(v.value), v.scale))
    })
    .flatten()
}

/// `device.mod-phases`: each modulation source's cycle position, -1 when
/// it has none or nothing samples the device.
pub(super) fn device_mod_phases(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
) -> [f64; SLOT_COUNT] {
    mod_sample(sources, shared, device, |_, phases, _| *phases).unwrap_or(NO_SLOT_PHASES)
}

/// The instrument params of `track` an enabled process slot writes, cached
/// per track under its [`PlockKey`] (and the descriptor it resolved
/// against).
fn process_bound(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
) -> Rc<HashSet<usize>> {
    let track = device.owner;
    let key = (sources.plock_key(track), Rc::as_ptr(&device.desc) as usize);
    plock_cached(
        shared,
        |shared| &mut shared.process_bound,
        track,
        key,
        || process_bound_instrument_params(&sources.state, &device.desc.desc, track),
    )
}

/// The buses (by id) an enabled process slot of `track` writes a send of
/// (`send.process-mapped`), cached per track under its [`PlockKey`].
pub(super) fn process_bound_sends(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
) -> Rc<HashSet<u64>> {
    plock_cached(
        shared,
        |shared| &mut shared.process_bound_sends,
        track,
        (sources.plock_key(track), 0),
        || process_bound_bus_sends(&sources.state, track),
    )
}

/// A track instrument's key locks, cached per track under its [`PlockKey`]
/// (and its descriptor); `None` for any other device.
pub(super) fn device_key_locks(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
) -> Option<Rc<InstrumentKeyLocks>> {
    if device.device != DeviceSlot::Instrument || !sources.track_exists(device.owner) {
        return None;
    }
    let track = device.owner;
    let slot = sources.state.pattern.instrument_slots.get(track)?;
    let key = (sources.plock_key(track), Rc::as_ptr(&device.desc) as usize);
    Some(plock_cached(
        shared,
        |shared| &mut shared.key_locks,
        track,
        key,
        || instrument_key_locks(slot, device.params()),
    ))
}

/// `device.key-locked-notes`, through `read` (no copy).
pub(super) fn key_locked_notes<R>(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    read: impl FnOnce(&[u8]) -> R,
) -> R {
    let locks = device_key_locks(sources, shared, device);
    read(locks.as_ref().map_or(&[][..], |locks| &locks.notes[..]))
}

/// `param.key-locks` of param `index`: `(note value)` rows.
fn param_key_locks(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    index: usize,
) -> Value {
    let locks = device_key_locks(sources, shared, device);
    let rows = locks.as_ref().and_then(|locks| locks.by_param.get(index));
    let rows = rows.map_or(&[][..], Vec::as_slice);
    list_value(
        rows.iter()
            .map(|(note, value)| numbers(&[f64::from(*note), f64::from(*value)])),
    )
}

/// The source params a device's modulation sources show now
/// (`selected_source_param_indices` over the displayed values), for
/// `param.visible`. Computed once per device per tick ([`VisibleCache`]).
fn shown_source_params(sources: &KindsHandles, device: &DeviceSource) -> Vec<usize> {
    let owner = device.owner;
    let is_bus = matches!(device.device, DeviceSlot::BusEffect(_));
    let buses = is_bus.then(|| sources.bus_state.lock().unwrap());
    let buses = buses.as_deref().map_or(&[][..], Vec::as_slice);
    let step = sources.plock_display_step(owner);
    let params = device.params();
    let shown = device
        .device
        .with_values(&sources.state, buses, owner, |values| {
            sequencer::instruments::voice_modulator::selected_source_param_indices(
                params,
                |idx, pdesc| {
                    step.and_then(|step| values.lock(step, idx))
                        .unwrap_or_else(|| values.base(pdesc, idx))
                },
            )
        });
    shown.unwrap_or_default()
}

/// The shown source params per device, computed at most once per device
/// per tick (keyed by the device's [`DeviceSource`]).
#[derive(Default)]
pub(super) struct VisibleCache(HashMap<usize, Vec<usize>>);

impl VisibleCache {
    fn visible(
        &mut self,
        sources: &KindsHandles,
        device: &DeviceSource,
        pdesc: &ParamDescriptor,
        index: usize,
    ) -> bool {
        match PanelSection::of(pdesc) {
            PanelSection::Hidden => false,
            PanelSection::Source => {
                let at = std::ptr::from_ref(device) as usize;
                let shown =
                    (self.0.entry(at)).or_insert_with(|| shown_source_params(sources, device));
                shown.contains(&index)
            }
            PanelSection::Main | PanelSection::Mod => true,
        }
    }
}

/// The panel-extra live fields of param `index` in `mask` (see the module
/// docs), each passed to `emit`. `shown` is the displayed value and whether
/// a p-lock supplies it (stored units), when the caller computed it.
#[allow(clippy::too_many_arguments)]
pub(super) fn param_panel_fields(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    pdesc: &ParamDescriptor,
    index: usize,
    mask: u32,
    shown: Option<f32>,
    visible: &mut VisibleCache,
    emit: &mut dyn FnMut(FieldKey, Value),
) {
    let bits = &*PARAM_BITS;
    let user = |stored: f32| f64::from(DeviceSlot::to_user(pdesc, stored));
    let shown = shown.map(user).unwrap_or_default();
    if mask & bits.visible != 0 {
        let on = visible.visible(sources, device, pdesc, index);
        emit(f::PARAM_VISIBLE, Value::Bool(on));
    }
    if mask & bits.mod_display() != 0 {
        let (offset, value, scale) =
            param_mod_display(sources, shared, device, pdesc, index).unwrap_or((0.0, shown, 1.0));
        if mask & bits.mod_offset != 0 {
            emit(f::PARAM_MOD_OFFSET, number(offset));
        }
        if mask & bits.mod_value != 0 {
            emit(f::PARAM_MOD_VALUE, number(value));
        }
        if mask & bits.mod_scale != 0 {
            emit(f::PARAM_MOD_SCALE, number(scale));
        }
        if mask & bits.mod_ratio != 0 {
            let ratio = if pdesc.is_percent() {
                value / 100.0
            } else {
                value
            };
            emit(f::PARAM_MOD_RATIO, number(ratio));
        }
    }
    if mask & bits.process() != 0 {
        let instrument =
            device.device == DeviceSlot::Instrument && sources.track_exists(device.owner);
        let mapped = instrument && process_bound(sources, shared, device).contains(&index);
        let written = (mapped)
            .then(|| sources.state.process_effective_param(device.owner, index))
            .flatten();
        if mask & bits.process_mapped != 0 {
            emit(f::PARAM_PROCESS_MAPPED, Value::Bool(mapped));
        }
        if mask & bits.process_value != 0 {
            let value = written.map_or(shown, |written| user(written.value));
            emit(f::PARAM_PROCESS_VALUE, number(value));
        }
        if mask & bits.process_clamped != 0 {
            let clamped = written.is_some_and(|written| written.clamped);
            emit(f::PARAM_PROCESS_CLAMPED, Value::Bool(clamped));
        }
    }
    if mask & bits.key_locks != 0 {
        emit(
            f::PARAM_KEY_LOCKS,
            param_key_locks(sources, shared, device, index),
        );
    }
}

/// The panel extras' state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct PanelState {
    /// The observed tensors and variants.
    pub(super) tensor_observed: ObservedList,
    pub(super) variant_observed: ObservedList,
    /// Each observing track's [`PlockKey`] when its `variants` was last
    /// computed.
    pub(super) track_variants: HashMap<InstanceId, PlockKey>,
    /// Whether a modulation display field (`param.mod-offset`, …,
    /// `device.mod-phases`) of a device the sample covers ([`mod_sampled`])
    /// was observed at the last sync.
    pub(super) mod_display_observed: bool,
    /// Whether a modulator track's instrument's `modulator-phase` or
    /// `-level` was observed at the last sync
    /// (`HostKinds::wants_modulator_meters`).
    pub(super) modulator_meters_observed: bool,
    /// Per device observing `table-options`, the cache key of the list
    /// last pushed (`None`: the empty list of a device that is no Filter
    /// Table).
    pub(super) table_options: HashMap<InstanceId, Option<TableOptionsKey>>,
    /// The devices observing `sound-binding`, and per one the model sync
    /// count its value was computed at (`HostKinds::sync_sound_bindings`).
    pub(super) sound_binding_observed: ObservedList,
    pub(super) sound_bindings: HashMap<InstanceId, u64>,
    /// The tensor cells read for the observed fields (reused per tick).
    tensor_cells: Vec<f32>,
}

impl HostKinds {
    /// Whether a kind field reads the modulation sample: the tick then
    /// keeps polling it with the fx panel hidden.
    pub(crate) fn wants_mod_display(&self) -> bool {
        self.panel.mod_display_observed
    }

    /// The observed cells of every tensor ([`ObservedList`] over the
    /// devices' tensors).
    pub(super) fn sync_tensor_live(&mut self, pusher: &mut Pusher<'_>) {
        let rt = &*pusher.rt;
        let devices = all_device_ids(&self.device_ids, &self.devices);
        let panel = &mut self.panel;
        (panel.tensor_observed).refresh(rt, &TENSOR_LIVE.names, || {
            children_of_owners(rt, devices, TENSOR)
        });
        let parts = [
            (
                TENSOR_LIVE.bit(f::TENSOR_VALUES),
                f::TENSOR_VALUES,
                TensorPart::Shown,
            ),
            (
                TENSOR_LIVE.bit(f::TENSOR_BASE),
                f::TENSOR_BASE,
                TensorPart::Base,
            ),
        ];
        let locked_bit = TENSOR_LIVE.bit(f::TENSOR_LOCKED);
        let (sources, shared) = (pusher.sources, pusher.shared);
        let cells = &mut panel.tensor_cells;
        for &(id, mask, _) in &panel.tensor_observed.entries {
            let Some(&[device, index]) = pusher.rt.instance_key(id) else {
                continue;
            };
            let Some(source) = shared.borrow().devices.get(&device).cloned() else {
                continue;
            };
            let index = index as usize;
            for (bit, key, part) in parts {
                if mask & bit != 0 && read_tensor(sources, &source, index, part, cells).is_some() {
                    pusher.push_numbers(id, key, cells);
                }
            }
            if mask & locked_bit != 0 {
                let part = TensorPart::Locked;
                if let Some(locked) = read_tensor(sources, &source, index, part, cells) {
                    pusher.push_computed(id, f::TENSOR_LOCKED, Value::Bool(locked));
                }
            }
        }
    }

    /// A track's `variants` (its step variants) while observed, recomputed
    /// only when its [`PlockKey`] moved.
    pub(super) fn sync_track_variants(
        &mut self,
        pusher: &mut Pusher<'_>,
        track: usize,
        id: InstanceId,
    ) {
        let key = pusher.sources.plock_key(track);
        if self.panel.track_variants.get(&id) == Some(&key) {
            return;
        }
        self.panel.track_variants.insert(id, key);
        let (sources, shared) = (pusher.sources, pusher.shared);
        let owner = (id, id);
        let list = owner_variants(
            &mut *pusher.rt,
            sources,
            shared,
            owner,
            track,
            VariantScope::Steps,
        );
        pusher.push_computed(id, f::TRACK_VARIANTS, list);
        self.panel.variant_observed.reset();
    }
}
