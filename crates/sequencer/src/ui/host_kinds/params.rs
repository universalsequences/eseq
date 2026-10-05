//! Device parameters (`param`, keyed (device instance id, param index)),
//! the device live fields and the step p-lock render (spec §14, stage 7b).
//!
//! A device's params are registered lazily, on the first read of
//! `d.params` (the reader hook, or the tick once `params` is observed),
//! like steps; from then on `d.params` is a model field. What their fields
//! read is copied out of the `App` at the model sync into a
//! [`DeviceSource`] per device, kept while its descriptor and position are
//! unchanged, so the reader hook can answer cold reads: the descriptor
//! (names, ranges, kinds) and where the values live ([`DeviceSlot`]). A
//! descriptor change (another effect or instrument in the device) drops
//! the params and registers fresh ones, so old handles go stale. Values,
//! p-lock state and the print latch are live fields read from the shared
//! sequencer state while observed; the descriptor fields are pushed once,
//! at registration. Every value is in display units (percent params ×100).
//!
//! Every device family reads its values where they live
//! ([`DeviceSlot::with_values`]): a track chain device or MIDI effect from
//! its live slot (the p-lock in force at the displayed step, with the
//! off-step hold, and an engaged project macro: [`device_param_display`]),
//! a drum rack slot or rack slot effect from the rack's snapshot (the
//! displayed step's p-lock, else a rack macro mapped onto it:
//! `rack_slot_instrument_param_display`, `rack_effect_param_display`,
//! shared with the rack panel), a bus effect from the shared bus copy (its
//! base, like the legacy `bus-N-fx-*` fields: bus effects show no p-lock).

use super::*;
use sequencer::effects::{ParamDescriptor, ParamKind};

/// What one device's params read without the `App` (see the module docs).
pub(crate) struct DeviceSource {
    /// The owner's position as of the model sync: the track's, or the
    /// bus's for a bus effect.
    pub(super) owner: usize,
    pub(super) device: DeviceSlot,
    pub(super) params: Rc<[ParamDescriptor]>,
    /// A sampler instrument's voices, for a cold `device.playhead` read (the
    /// tick re-resolves them from the `App` per read).
    pub(super) sampler: Option<SamplerPlayhead>,
}

/// Whether two descriptor param lists describe the same params.
fn same_params(a: &[ParamDescriptor], b: &[ParamDescriptor]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.same_shape(b))
}

/// `param.type`.
fn param_type(pdesc: &ParamDescriptor) -> &'static str {
    match pdesc.kind {
        ParamKind::Continuous { .. } => "continuous",
        ParamKind::Enum { .. } => "enum",
        ParamKind::Boolean => "boolean",
    }
}

/// The descriptor (model) fields of a param, in display units.
fn param_model_fields(pdesc: &ParamDescriptor) -> [(FieldKey, Value); 7] {
    let user = |stored| number(DeviceSlot::to_user(pdesc, stored));
    let options = param_enum_labels(pdesc).into_iter().map(Value::String);
    [
        (f::PARAM_NAME, Value::String(pdesc.name.clone())),
        (f::PARAM_MIN, user(pdesc.min)),
        (f::PARAM_MAX, user(pdesc.max)),
        (f::PARAM_DEFAULT, user(pdesc.default)),
        (f::PARAM_OPTIONS, list_value(options)),
        (f::PARAM_TYPE, Value::String(param_type(pdesc).to_string())),
        (
            f::PARAM_UNIT,
            Value::String(param_unit(pdesc).unwrap_or_default()),
        ),
    ]
}

/// Register the param instances of `device` (missing ones get their fixed
/// `index`/`device` and descriptor fields), drop those past its param
/// count, and record the device as having registered params. `None` when
/// the device has no [`DeviceSource`] (not synced yet, or gone).
pub(super) fn register_params<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    device: InstanceId,
) -> Option<Vec<InstanceId>> {
    let source = shared.borrow().devices.get(&device)?.clone();
    let count = source.params.len();
    let params = indexed_children(store, device, PARAM, count, |store, id, index| {
        store.push(id, f::PARAM_INDEX, number(index as f64));
        store.push(id, f::PARAM_DEVICE, Value::Instance(device));
        for (key, value) in param_model_fields(&source.params[index]) {
            store.push(id, key, value);
        }
    });
    shared.borrow_mut().param_devices.insert(device);
    Some(params)
}

/// A cold read of `d.params` (the reader hook): registers the params the
/// first time; afterwards the model field answers (`None`).
pub(super) fn cold_device_params<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    device: InstanceId,
) -> Option<Value> {
    {
        let shared = shared.borrow();
        if shared.param_devices.contains(&device) || shared.skip.contains(&f::DEVICE_PARAMS) {
            return None;
        }
    }
    let params = register_params(store, shared, device)?;
    shared.borrow_mut().count(f::DEVICE_PARAMS);
    Some(instance_list(params))
}

/// One param field's value, before it becomes a [`Value`] (`text` borrows
/// its label, so an unchanged label costs no allocation).
pub(super) enum ParamField<'a> {
    Number(f32),
    Bool(bool),
    Text(&'a str),
}

impl ParamField<'_> {
    pub(super) fn value(&self) -> Value {
        match self {
            Self::Number(n) => number(*n),
            Self::Bool(b) => Value::Bool(*b),
            Self::Text(text) => Value::String((*text).to_string()),
        }
    }
}

/// The text a param shows: the option an enum value selects (clamped), on
/// or off for a boolean, empty for a continuous param.
fn param_text(pdesc: &ParamDescriptor, stored: f32) -> &str {
    match &pdesc.kind {
        ParamKind::Boolean if stored >= 0.5 => "on",
        ParamKind::Boolean => "off",
        _ => pdesc.option_label(stored).unwrap_or(""),
    }
}

/// The observed-mask bits of the param live fields.
struct ParamBits {
    value: u32,
    base: u32,
    locked: u32,
    has_locks: u32,
    text: u32,
    printing: u32,
}

static PARAM_BITS: LazyLock<ParamBits> = LazyLock::new(|| ParamBits {
    value: PARAM_LIVE.bit(f::PARAM_VALUE),
    base: PARAM_LIVE.bit(f::PARAM_BASE),
    locked: PARAM_LIVE.bit(f::PARAM_LOCKED),
    has_locks: PARAM_LIVE.bit(f::PARAM_HAS_LOCKS),
    text: PARAM_LIVE.bit(f::PARAM_TEXT),
    printing: PARAM_LIVE.bit(f::PARAM_PRINTING),
});

/// What one param shows, as far as a mask asks: the displayed value
/// (stored units) and whether a p-lock supplies it, the base, whether some
/// step locks it.
#[derive(Default)]
struct ParamReading {
    shown: Option<(f32, bool)>,
    base: Option<f32>,
    has_locks: Option<bool>,
}

/// Read param `index` of `device` where its family keeps it
/// ([`DeviceSlot::with_values`], see the module docs), computing only what
/// `mask` asks for. `None` when the device is gone.
fn read_param(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    pdesc: &ParamDescriptor,
    index: usize,
    mask: u32,
) -> Option<ParamReading> {
    let bits = &*PARAM_BITS;
    let shown = mask & (bits.value | bits.locked | bits.text) != 0;
    let base = mask & bits.base != 0;
    let has_locks = mask & bits.has_locks != 0;
    let (state, owner) = (&sources.state, device.owner);
    let is_bus = matches!(device.device, DeviceSlot::BusEffect(_));
    if !is_bus && !sources.track_exists(owner) {
        return None;
    }
    // Only a bus effect reads the shared bus copy.
    let buses = is_bus.then(|| sources.bus_state.lock().unwrap());
    let buses = buses.as_deref().map_or(&[][..], Vec::as_slice);
    let step = || sources.plock_display_step(owner);
    let num_steps = || match is_bus {
        true => MAX_STEPS,
        false => sources.num_steps(owner),
    };
    device.device.with_values(state, buses, owner, |values| {
        let display = match values {
            DeviceValues::Live(slot) => shown.then(|| {
                let effective = device.device.macro_key(state, owner, index).map(|key| {
                    let overrides = &shared.borrow().macro_overrides;
                    (overrides.get(&key).copied()).unwrap_or_else(|| slot.defaults.get(index))
                });
                device_param_display(state, owner, slot, pdesc, index, step(), effective)
            }),
            DeviceValues::Rack { rack, values } => shown.then(|| match device.device {
                DeviceSlot::RackEffect { rack_slot, slot } => rack_effect_param_display(
                    rack,
                    rack_slot,
                    slot,
                    values,
                    pdesc.default,
                    index,
                    step(),
                ),
                DeviceSlot::RackSlot(slot) => rack_slot_instrument_param_display(
                    rack,
                    slot,
                    &rack.slots[slot],
                    pdesc.default,
                    index,
                    step(),
                ),
                _ => (
                    values.defaults.get(index).copied().unwrap_or(pdesc.default),
                    false,
                ),
            }),
            // A bus effect shows its own value (no p-lock), as its legacy
            // field does.
            DeviceValues::Snapshot(_) => Some((values.base(pdesc, index), false)),
        };
        ParamReading {
            shown: display,
            base: base.then(|| values.base(pdesc, index)),
            has_locks: has_locks.then(|| values.has_lock(index, num_steps())),
        }
    })
}

/// The track a device's param follows for its print latch and its
/// `has-locks` key: its own track, or the current track for a bus effect
/// (whose knob latches under the current track).
fn param_track(sources: &KindsHandles, device: &DeviceSource) -> usize {
    match device.device {
        DeviceSlot::BusEffect(_) => sources.current_track.load(Ordering::Relaxed),
        _ => device.owner,
    }
}

/// The live fields of param `index` of `device` in `mask` (bit `i` is
/// `PARAM_LIVE.keys[i]`), each passed to `emit`. The displayed value and
/// its p-lock state are computed once for `value`, `locked` and `text`.
pub(super) fn param_live_fields<'a>(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &'a DeviceSource,
    index: usize,
    mask: u32,
    mut emit: impl FnMut(FieldKey, ParamField<'a>),
) {
    let Some(pdesc) = device.params.get(index) else {
        return;
    };
    let Some(reading) = read_param(sources, shared, device, pdesc, index, mask) else {
        return;
    };
    let bits = &*PARAM_BITS;
    let user = |stored| DeviceSlot::to_user(pdesc, stored);
    if let Some((stored, locked)) = reading.shown {
        if mask & bits.value != 0 {
            emit(f::PARAM_VALUE, ParamField::Number(user(stored)));
        }
        if mask & bits.locked != 0 {
            emit(f::PARAM_LOCKED, ParamField::Bool(locked));
        }
        if mask & bits.text != 0 {
            emit(f::PARAM_TEXT, ParamField::Text(param_text(pdesc, stored)));
        }
    }
    if let Some(stored) = reading.base.filter(|_| mask & bits.base != 0) {
        emit(f::PARAM_BASE, ParamField::Number(user(stored)));
    }
    if let Some(any) = reading.has_locks {
        emit(f::PARAM_HAS_LOCKS, ParamField::Bool(any));
    }
    if mask & bits.printing != 0 {
        let state = &sources.state;
        let track = param_track(sources, device);
        let target = device.device.print_target(device.owner, index);
        let printing = state.transport.playing.load(Ordering::Relaxed)
            && sources.recording.load(Ordering::Relaxed)
            && (sources.step_print.lock().unwrap()).holds(track, target);
        emit(f::PARAM_PRINTING, ParamField::Bool(printing));
    }
}

/// One live field of a param (the reader hook's cold read).
pub(super) fn param_live_value(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &DeviceSource,
    index: usize,
    key: FieldKey,
) -> Option<Value> {
    let mut value = None;
    param_live_fields(
        sources,
        shared,
        device,
        index,
        PARAM_LIVE.bit(key),
        |_, field| {
            value = Some(field.value());
        },
    );
    value
}

/// The fields a device's observed mask covers: its live `playhead` and
/// `delete-target`, and `params`, a model field the tick registers once
/// something observes it.
const DEVICE_OBSERVED: [&str; 3] = ["playhead", "params", "delete-target"];
const DEVICE_PLAYHEAD_BIT: u32 = 1;
const DEVICE_PARAMS_BIT: u32 = 2;
const DEVICE_DELETE_TARGET_BIT: u32 = 4;

/// `device.delete-target`: the delete target names the device
/// ([`DeviceSlot::delete_target`]).
pub(super) fn device_delete_target(sources: &KindsHandles, device: &DeviceSource) -> bool {
    let current = sources.current_track.load(Ordering::Relaxed);
    let Some(target) = device.device.delete_target(device.owner, current) else {
        return false;
    };
    sources.active_delete_target.lock().unwrap().as_ref() == Some(&target)
}

/// One step's p-lock render (`plocked`, `lock-kind`, `variant-color`).
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct StepPlockRender {
    pub(super) plocked: bool,
    pub(super) kind: u8,
    pub(super) color: [f32; 3],
}

impl StepPlockRender {
    pub(super) fn field(&self, key: FieldKey) -> Option<Value> {
        Some(match key {
            f::STEP_PLOCKED => Value::Bool(self.plocked),
            f::STEP_LOCK_KIND => number(self.kind),
            f::STEP_VARIANT_COLOR => rgb3(self.color),
            _ => return None,
        })
    }
}

/// What a track's p-locks may have moved with: its p-lock revision in the
/// invalidation queue (`UiInvalidation::plock_scope`), and the epochs a
/// scene switch, an undo or a structural edit bumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlockKey {
    generation: u64,
    pattern_epoch: u64,
    fx_epoch: usize,
    ui_epoch: usize,
}

impl KindsHandles {
    pub(super) fn plock_key(&self, track: usize) -> PlockKey {
        PlockKey {
            generation: self.ui_invalidations.plock_generation(track),
            pattern_epoch: self.state.transport.pattern_epoch.load(Ordering::Relaxed),
            fx_epoch: self.fx_epoch.load(Ordering::Relaxed),
            ui_epoch: self.ui_epoch.load(Ordering::Relaxed),
        }
    }
}

/// The p-lock render of every step of `track` (the legacy
/// `plock_variant_step_render_values` and `track_step_plock_mask`): one
/// whole-track scan, cached per track until its [`PlockKey`] moves, which
/// the reader hook's cold reads and the step diff share.
pub(super) fn track_plock_render(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
) -> Rc<[StepPlockRender]> {
    if !sources.track_exists(track) {
        return Rc::from(Vec::new());
    }
    let key = sources.plock_key(track);
    if let Some(Some((cached, render))) = shared.borrow().plock_renders.get(track) {
        if *cached == key {
            return render.clone();
        }
    }
    let state = &sources.state;
    let render = plock_variant_step_render_values(state, track);
    let slots = shared
        .borrow()
        .effect_slots
        .get(track)
        .copied()
        .unwrap_or(0);
    let mask = track_step_plock_mask_for_slots(state, track, slots, Some(&render));
    let render: Rc<[StepPlockRender]> = render
        .iter()
        .enumerate()
        .map(|(step, render)| StepPlockRender {
            plocked: mask[step / 64] & (1u64 << (step % 64)) != 0,
            kind: render.kind,
            color: render.color,
        })
        .collect();
    let mut shared = shared.borrow_mut();
    shared.plock_scans += 1;
    if shared.plock_renders.len() <= track {
        shared.plock_renders.resize(track + 1, None);
    }
    shared.plock_renders[track] = Some((key, render.clone()));
    render
}

/// The observed instances of one kind, kept per observer epoch.
#[derive(Default)]
pub(super) struct ObservedList {
    /// The epoch the entries were built at; `None` until built and after a
    /// [`Self::reset`].
    epoch: Option<u64>,
    /// (instance, observed mask, the [`PlockKey`] its `has-locks` was last
    /// computed under).
    pub(super) entries: Vec<(InstanceId, u32, Option<PlockKey>)>,
}

impl ObservedList {
    /// Rebuild at the next tick (instances were added or dropped).
    pub(super) fn reset(&mut self) {
        self.epoch = None;
    }

    /// Bring the list up to date: rebuilt from `ids` when the observer
    /// epoch moved (or after a reset); else the cached entries re-queried
    /// (losing an observer moves no epoch), dropping the unobserved. A tick
    /// between epoch moves costs work in proportion to the observed.
    /// Returns how many instances it queried.
    pub(super) fn refresh(
        &mut self,
        rt: &Runtime,
        names: &[&str],
        ids: impl FnOnce() -> Vec<InstanceId>,
    ) -> usize {
        let epoch = rt.instance_observer_epoch();
        if self.epoch == Some(epoch) {
            let queried = self.entries.len();
            self.entries.retain_mut(|(id, mask, _)| {
                *mask = rt.host_fields_observed(*id, names);
                *mask != 0
            });
            return queried;
        }
        self.epoch = Some(epoch);
        self.entries.clear();
        let ids = ids();
        for id in &ids {
            let mask = rt.host_fields_observed(*id, names);
            if mask != 0 {
                self.entries.push((*id, mask, None));
            }
        }
        ids.len()
    }

    /// Push each entry's observed `fields`.
    pub(super) fn push_masked(&self, pusher: &mut Pusher<'_>, fields: &LiveFields) {
        for &(id, mask, _) in &self.entries {
            pusher.push_live_masked(id, fields, mask);
        }
    }
}

impl HostKinds {
    /// Keep `device_id`'s [`DeviceSource`] current at a model sync (the
    /// track model's for the track chain, the device sync's for the other
    /// families): the existing one while its descriptor (`params`) and
    /// position are unchanged; else a new one. On a descriptor change
    /// (another effect or instrument in the device) its params are dropped
    /// and, when they were registered, registered fresh with `d.params`
    /// re-pushed. Returns whether param instances were replaced.
    pub(super) fn sync_device_source(
        pusher: &mut Pusher<'_>,
        app: &app::App,
        device_id: InstanceId,
        owner: usize,
        device: DeviceSlot,
        params: &[ParamDescriptor],
    ) -> bool {
        let sampler = match device {
            DeviceSlot::Instrument => SamplerPlayhead::of(app, owner),
            _ => None,
        };
        let existing = pusher.shared.borrow().devices.get(&device_id).cloned();
        let same_params = existing
            .as_ref()
            .is_some_and(|source| same_params(&source.params, params));
        if let Some(source) = &existing {
            let same_sampler = match (&source.sampler, &sampler) {
                (Some(a), Some(b)) => a.same_voices(b),
                (a, b) => a.is_none() && b.is_none(),
            };
            if same_params && source.owner == owner && source.device == device && same_sampler {
                return false;
            }
        }
        let params = match &existing {
            Some(source) if same_params => source.params.clone(),
            _ => Rc::from(params.to_vec()),
        };
        let source = DeviceSource {
            owner,
            device,
            params,
            sampler,
        };
        let registered = {
            let mut shared = pusher.shared.borrow_mut();
            shared.devices.insert(device_id, Rc::new(source));
            if existing.is_none() || same_params {
                return false;
            }
            shared.param_devices.remove(&device_id)
        };
        let doomed: Vec<InstanceId> = pusher
            .rt
            .keyed_children_of_kind(device_id, PARAM)
            .map(|(id, _)| id)
            .collect();
        for id in doomed {
            pusher.rt.drop_instance(id);
            pusher.changed = true;
        }
        if registered {
            if let Some(params) = register_params(&mut *pusher.rt, pusher.shared, device_id) {
                pusher.push(device_id, f::DEVICE_PARAMS, instance_list(params));
                pusher.changed = true;
            }
        }
        true
    }

    /// The observed device fields (`playhead`; `params` registered once
    /// observed), then the observed param fields, both from lists kept per
    /// observer epoch ([`ObservedList`]): a tick costs work in proportion
    /// to what is observed, not to what is registered.
    pub(super) fn sync_device_live(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let (chain, devices) = (&self.device_ids, &self.devices);
        (self.device_observed).refresh(pusher.rt, &DEVICE_OBSERVED, || {
            chain.iter().copied().chain(devices.ids()).collect()
        });
        for index in 0..self.device_observed.entries.len() {
            let (id, mask, _) = self.device_observed.entries[index];
            let Some(source) = pusher.shared.borrow().devices.get(&id).cloned() else {
                continue;
            };
            if mask & DEVICE_PLAYHEAD_BIT != 0 {
                // Re-resolved from the `App` per read: a sample load or a
                // voice rebuild moves no model counter.
                let seconds = match source.device {
                    DeviceSlot::Instrument => read_sampler_playhead_seconds(app, source.owner),
                    _ => 0.0,
                };
                pusher.push_computed(id, f::DEVICE_PLAYHEAD, number(seconds));
            }
            if mask & DEVICE_DELETE_TARGET_BIT != 0 {
                let held = device_delete_target(pusher.sources, &source);
                pusher.push_computed(id, f::DEVICE_DELETE_TARGET, Value::Bool(held));
            }
            let registered = pusher.shared.borrow().param_devices.contains(&id);
            if mask & DEVICE_PARAMS_BIT != 0 && !registered {
                if let Some(params) = register_params(&mut *pusher.rt, pusher.shared, id) {
                    pusher.push_computed(id, f::DEVICE_PARAMS, instance_list(params));
                    pusher.changed = true;
                    self.param_observed.reset();
                }
            }
        }
        // Rebuilt from the registered devices' params only when the observer
        // epoch moved: a tick between costs work in proportion to the
        // observed params.
        let (rt, shared) = (&*pusher.rt, pusher.shared);
        let (chain, devices) = (&self.device_ids, &self.devices);
        let queried = self.param_observed.refresh(rt, &PARAM_LIVE.names, || {
            let shared = shared.borrow();
            let registered = (chain.iter().copied().chain(devices.ids()))
                .filter(|id| shared.param_devices.contains(id));
            let params = registered
                .flat_map(|device| rt.keyed_children_of_kind(device, PARAM).map(|(id, _)| id));
            params.collect()
        });
        pusher.shared.borrow_mut().param_queries += queried as u64;
        let has_locks = PARAM_BITS.has_locks;
        let (sources, shared) = (pusher.sources, pusher.shared);
        for entry in &mut self.param_observed.entries {
            let (id, mut mask, seen) = *entry;
            let Some(&[device_id, param]) = pusher.rt.instance_key(id) else {
                continue;
            };
            let Some(device) = shared.borrow().devices.get(&device_id).cloned() else {
                continue;
            };
            // `has-locks` scans the slot's p-locks: only after they may
            // have moved.
            if mask & has_locks != 0 {
                let key = sources.plock_key(param_track(sources, &device));
                if seen == Some(key) {
                    mask &= !has_locks;
                }
                entry.2 = Some(key);
            }
            param_live_fields(
                sources,
                shared,
                &device,
                param as usize,
                mask,
                |key, field| match field {
                    ParamField::Text(text) => pusher.push_text(id, key, text),
                    field => pusher.push_computed(id, key, field.value()),
                },
            );
        }
    }
}
