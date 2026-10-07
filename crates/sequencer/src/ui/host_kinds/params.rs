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
use sequencer::effects::{
    EffectDescriptor, InstrumentModulationTarget, ParamDescriptor, ParamKind, TensorParamDescriptor,
};

/// What one device's params read without the `App` (see the module docs).
pub(crate) struct DeviceSource {
    /// The owner's position as of the model sync: the track's, or the
    /// bus's for a bus effect.
    pub(super) owner: usize,
    pub(super) device: DeviceSlot,
    pub(super) desc: Rc<DeviceDescriptor>,
    /// A sampler instrument's voices, which `device.playhead` samples
    /// (observed and cold; the tick refreshes them when they move:
    /// [`SamplerPlayhead::current`]).
    pub(super) sampler: Option<SamplerPlayhead>,
}

impl DeviceSource {
    pub(super) fn params(&self) -> &[ParamDescriptor] {
        &self.desc.desc.params
    }
}

/// A device's descriptor as its kinds read it (the params, the modulation
/// lanes onto them, the tensors; process bindings resolve against it),
/// copied out of the `App` when it changes.
pub(crate) struct DeviceDescriptor {
    pub(super) desc: EffectDescriptor,
    /// A sampler's lane depths are stored in DSP units (scaled for display:
    /// `mod_target_depth_range`); every other instrument's are display units.
    pub(super) sampler_depths: bool,
}

impl DeviceDescriptor {
    pub(super) fn of(desc: Option<&EffectDescriptor>, sampler_depths: bool) -> Self {
        let desc = desc.cloned().unwrap_or_else(|| EffectDescriptor {
            declared_latency_samples: None,
            name: String::new(),
            params: Vec::new(),
            tensor_params: Vec::new(),
            input_channels: 0,
            output_channels: 0,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
        });
        Self {
            desc,
            sampler_depths,
        }
    }

    pub(super) fn targets(&self) -> &[InstrumentModulationTarget] {
        &self.desc.instrument_modulation_targets
    }

    pub(super) fn tensors(&self) -> &[TensorParamDescriptor] {
        &self.desc.tensor_params
    }

    /// Whether `desc` describes the same device: its name, params (with
    /// their UI metadata), lanes, tensors and fixed modulators.
    fn same(&self, desc: Option<&EffectDescriptor>, sampler_depths: bool) -> bool {
        let mine = &self.desc;
        let Some(desc) = desc else {
            return mine.params.is_empty()
                && mine.instrument_modulation_targets.is_empty()
                && mine.tensor_params.is_empty()
                && mine.instrument_modulators.is_empty();
        };
        // The name too: another effect is another device (its table and
        // IR fields follow the name).
        self.sampler_depths == sampler_depths
            && mine.name == desc.name
            && same_params(&mine.params, &desc.params)
            && mine.tensor_params == desc.tensor_params
            && (mine.instrument_modulators.len()) == desc.instrument_modulators.len()
            && (mine.instrument_modulators.iter())
                .zip(&desc.instrument_modulators)
                .all(|(a, b)| a.slot == b.slot && a.label == b.label)
            && (mine.instrument_modulation_targets.len())
                == desc.instrument_modulation_targets.len()
            && (mine.instrument_modulation_targets.iter())
                .zip(&desc.instrument_modulation_targets)
                .all(|(a, b)| same_target(a, b))
    }
}

/// Whether two descriptor param lists describe the same params (their UI
/// metadata included: `param.group`, … are pushed at registration).
fn same_params(a: &[ParamDescriptor], b: &[ParamDescriptor]) -> bool {
    a.len() == b.len()
        && (a.iter().zip(b)).all(|(a, b)| a.same_shape(b) && a.ui_metadata == b.ui_metadata)
}

fn same_target(a: &InstrumentModulationTarget, b: &InstrumentModulationTarget) -> bool {
    a.base_param_idx == b.base_param_idx
        && a.source_param_idx == b.source_param_idx
        && a.modulator_slot == b.modulator_slot
        && a.depth_param_idx == b.depth_param_idx
        && a.depth_min.to_bits() == b.depth_min.to_bits()
        && a.depth_max.to_bits() == b.depth_max.to_bits()
        && a.depth_unit == b.depth_unit
}

/// `param.type`.
fn param_type(pdesc: &ParamDescriptor) -> &'static str {
    match pdesc.kind {
        ParamKind::Continuous { .. } => "continuous",
        ParamKind::Enum { .. } => "enum",
        ParamKind::Boolean => "boolean",
    }
}

/// The descriptor (model) fields of a param, in display units, with its
/// panel placement ([`PanelSection`]) and UI metadata (as the legacy
/// `insert_param_ui_metadata`: an unresolved options reference only where
/// no option labels resolved).
fn param_model_fields(pdesc: &ParamDescriptor) -> [(FieldKey, Value); 16] {
    let user = |stored| number(DeviceSlot::to_user(pdesc, stored));
    let options = param_enum_labels(pdesc).into_iter().map(Value::String);
    let section = PanelSection::of(pdesc);
    let metadata = pdesc.ui_metadata.as_ref();
    let meta = |read: fn(&sequencer::effects::ParamUiMetadata) -> &Option<String>| {
        Value::String(metadata.and_then(|m| read(m).clone()).unwrap_or_default())
    };
    let asset_options = metadata
        .and_then(|metadata| metadata.asset_options.as_ref())
        .filter(|_| !matches!(pdesc.kind, ParamKind::Enum { .. }))
        .map_or(Value::Nil, param_asset_options_value);
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
        (f::PARAM_PERCENT, Value::Bool(pdesc.is_percent())),
        (f::PARAM_LABEL, Value::String(section.label(pdesc))),
        (f::PARAM_SECTION, text(section.name())),
        (f::PARAM_MOD_SLOT, number(section.mod_slot(pdesc) as f64)),
        (f::PARAM_GROUP, meta(|m| &m.group)),
        (f::PARAM_ENV, meta(|m| &m.env)),
        (f::PARAM_ROLE, meta(|m| &m.role)),
        (f::PARAM_DISPLAY_NAME, meta(|m| &m.display_name)),
        (f::PARAM_ASSET_OPTIONS, asset_options),
    ]
}

/// Register the param instances of `device` (missing ones get their fixed
/// `index`/`device` and descriptor fields, and their modulation lanes:
/// [`register_mod_targets`]), drop those past its param count, and record
/// the device as having registered params. `None` when the device has no
/// [`DeviceSource`] (not synced yet, or gone).
pub(super) fn register_params<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    device: InstanceId,
) -> Option<Vec<InstanceId>> {
    let source = shared.borrow().devices.get(&device)?.clone();
    let count = source.params().len();
    let mut fresh = false;
    let params = indexed_children(store, device, PARAM, count, |store, id, index| {
        fresh = true;
        store.push(id, f::PARAM_INDEX, number(index as f64));
        store.push(id, f::PARAM_DEVICE, Value::Instance(device));
        for (key, value) in param_model_fields(&source.params()[index]) {
            store.push(id, key, value);
        }
    });
    // A fresh registration moves no `params_generation`: no macro target
    // resolves anew (the macro sync registers the params it targets).
    shared.borrow_mut().param_devices.insert(device);
    if fresh {
        register_mod_targets(store, &source.desc, &params);
    }
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
    Value(Value),
}

impl ParamField<'_> {
    pub(super) fn value(self) -> Value {
        match self {
            Self::Number(n) => number(n),
            Self::Bool(b) => Value::Bool(b),
            Self::Text(text) => Value::String(text.to_string()),
            Self::Value(value) => value,
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
pub(super) struct ParamBits {
    pub(super) value: u32,
    pub(super) base: u32,
    pub(super) locked: u32,
    pub(super) overridden: u32,
    pub(super) has_locks: u32,
    pub(super) text: u32,
    pub(super) printing: u32,
    pub(super) visible: u32,
    pub(super) mod_offset: u32,
    pub(super) mod_value: u32,
    pub(super) mod_scale: u32,
    pub(super) mod_ratio: u32,
    pub(super) mod_phase: u32,
    pub(super) process_mapped: u32,
    pub(super) process_value: u32,
    pub(super) process_clamped: u32,
    pub(super) key_locks: u32,
    pub(super) step_locks: u32,
}

impl ParamBits {
    /// The fields that need the displayed value.
    fn shown(&self) -> u32 {
        self.value | self.locked | self.overridden | self.text | self.mod_value | self.process_value
    }

    /// The modulation display fields: what the tick's modulation sample
    /// feeds (observing one keeps it polled; `mod-phase` only on a
    /// modulation source's setting).
    pub(super) fn mod_display(&self) -> u32 {
        self.mod_values() | self.mod_phase
    }

    /// The modulation display fields that read the param's own modulation.
    pub(super) fn mod_values(&self) -> u32 {
        self.mod_offset | self.mod_value | self.mod_scale | self.mod_ratio
    }

    pub(super) fn process(&self) -> u32 {
        self.process_mapped | self.process_value | self.process_clamped
    }

    /// The fields that move only with their track's [`PlockKey`]: the tick
    /// recomputes them only when it moved.
    pub(super) fn plock_keyed(&self) -> u32 {
        self.has_locks | self.process_mapped | self.key_locks | self.step_locks
    }
}

pub(super) static PARAM_BITS: LazyLock<ParamBits> = LazyLock::new(|| ParamBits {
    value: PARAM_LIVE.bit(f::PARAM_VALUE),
    base: PARAM_LIVE.bit(f::PARAM_BASE),
    locked: PARAM_LIVE.bit(f::PARAM_LOCKED),
    overridden: PARAM_LIVE.bit(f::PARAM_OVERRIDDEN),
    has_locks: PARAM_LIVE.bit(f::PARAM_HAS_LOCKS),
    text: PARAM_LIVE.bit(f::PARAM_TEXT),
    printing: PARAM_LIVE.bit(f::PARAM_PRINTING),
    visible: PARAM_LIVE.bit(f::PARAM_VISIBLE),
    mod_offset: PARAM_LIVE.bit(f::PARAM_MOD_OFFSET),
    mod_value: PARAM_LIVE.bit(f::PARAM_MOD_VALUE),
    mod_scale: PARAM_LIVE.bit(f::PARAM_MOD_SCALE),
    mod_ratio: PARAM_LIVE.bit(f::PARAM_MOD_RATIO),
    mod_phase: PARAM_LIVE.bit(f::PARAM_MOD_PHASE),
    process_mapped: PARAM_LIVE.bit(f::PARAM_PROCESS_MAPPED),
    process_value: PARAM_LIVE.bit(f::PARAM_PROCESS_VALUE),
    process_clamped: PARAM_LIVE.bit(f::PARAM_PROCESS_CLAMPED),
    key_locks: PARAM_LIVE.bit(f::PARAM_KEY_LOCKS),
    step_locks: PARAM_LIVE.bit(f::PARAM_STEP_LOCKS),
});

/// What one param shows, as far as a mask asks: the displayed value
/// (stored units) and whether a p-lock supplies it, whether a selected
/// neuron's override replaces it, the base, whether some step locks it.
#[derive(Default)]
struct ParamReading {
    shown: Option<(f32, bool)>,
    overridden: bool,
    base: Option<f32>,
    has_locks: Option<bool>,
    /// (step, stored value) of every step of the pattern that locks it.
    step_locks: Option<Vec<(usize, f32)>>,
}

/// A selected neural neuron's output override of a track chain param (the
/// step editing overlay the legacy value fields show); `None` while no
/// neuron is selected.
fn neural_override(
    sources: &KindsHandles,
    device: DeviceSlot,
    track: usize,
    index: usize,
) -> Option<f32> {
    let selection = sources.selected_neural_neurons.lock().unwrap();
    if selection.is_empty() {
        return None;
    }
    let state = &sources.state;
    match device {
        DeviceSlot::Instrument => sequencer::lisp_host::selected_neural_instrument_plock_value(
            state, &selection, track, index,
        ),
        DeviceSlot::Effect(slot) => sequencer::lisp_host::selected_neural_effect_plock_value(
            state, &selection, track, slot, index,
        ),
        _ => None,
    }
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
    let shown = mask & bits.shown() != 0;
    let base = mask & bits.base != 0;
    let has_locks = mask & bits.has_locks != 0;
    let step_locks = mask & bits.step_locks != 0;
    let (state, owner) = (&sources.state, device.owner);
    let is_bus = matches!(device.device, DeviceSlot::BusEffect(_));
    if !is_bus && !sources.track_exists(owner) {
        return None;
    }
    // A selected neuron's override shows over a chain param's value
    // (`overridden`; `locked` keeps saying whether a p-lock supplies it).
    let neural = shown
        .then(|| neural_override(sources, device.device, owner, index))
        .flatten();
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
        let display = match neural {
            Some(stored) => display.map(|(_, locked)| (stored, locked)),
            None => display,
        };
        ParamReading {
            shown: display,
            overridden: neural.is_some(),
            base: base.then(|| values.base(pdesc, index)),
            has_locks: has_locks.then(|| values.has_lock(index, num_steps())),
            step_locks: step_locks.then(|| {
                (0..num_steps())
                    .filter_map(|step| Some((step, values.lock(step, index)?)))
                    .collect()
            }),
        }
    })
}

/// The track a device's param follows for its print latch and its
/// `has-locks` key: its own track, or the current track for a bus effect
/// (whose knob latches under the current track).
pub(super) fn param_track(sources: &KindsHandles, device: &DeviceSource) -> usize {
    match device.device {
        DeviceSlot::BusEffect(_) => sources.current_track.load(Ordering::Relaxed),
        _ => device.owner,
    }
}

/// The live fields of param `index` of `device` in `mask` (bit `i` is
/// `PARAM_LIVE.keys[i]`), each passed to `emit`. The displayed value and
/// its p-lock state are computed once for `value`, `locked`, `text` and the
/// panel extras ([`param_panel_fields`]; `visible` caches per device).
pub(super) fn param_live_fields<'a>(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    device: &'a DeviceSource,
    index: usize,
    mask: u32,
    visible: &mut VisibleCache,
    mut emit: impl FnMut(FieldKey, ParamField<'a>),
) {
    let Some(pdesc) = device.params().get(index) else {
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
        if mask & bits.overridden != 0 {
            emit(f::PARAM_OVERRIDDEN, ParamField::Bool(reading.overridden));
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
    if let Some(locks) = &reading.step_locks {
        // `(step value)` rows, the value in display units (the tracker's
        // lock cells).
        let rows = (locks.iter())
            .map(|(step, stored)| list_value([number(*step as f64), number(user(*stored))]));
        emit(f::PARAM_STEP_LOCKS, ParamField::Value(list_value(rows)));
    }
    if mask & bits.printing != 0 {
        let track = param_track(sources, device);
        let target = device.device.print_target(device.owner, index);
        let printing = sources.print_latch(track, target).is_some();
        emit(f::PARAM_PRINTING, ParamField::Bool(printing));
    }
    let shown = reading.shown.map(|(stored, _)| stored);
    let extras = bits.visible | bits.mod_display() | bits.process() | bits.key_locks;
    if mask & extras != 0 {
        let emit = &mut |key, value| emit(key, ParamField::Value(value));
        param_panel_fields(
            sources, shared, device, pdesc, index, mask, shown, visible, emit,
        );
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
    let mask = PARAM_LIVE.bit(key);
    let visible = &mut VisibleCache::default();
    param_live_fields(sources, shared, device, index, mask, visible, |_, field| {
        value = Some(field.value());
    });
    value
}

/// The fields a device's observed mask covers: its live fields (bit `i` is
/// `DEVICE_LIVE.keys[i]`), then `params`, a model field the tick registers
/// once something observes it ([`DEVICE_PARAMS_BIT`]).
static DEVICE_OBSERVED: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut names = DEVICE_LIVE.names.clone();
    names.push(f::DEVICE_PARAMS.1);
    names
});
static DEVICE_PARAMS_BIT: LazyLock<u32> = LazyLock::new(|| {
    assert!(
        DEVICE_LIVE.keys.len() < 32,
        "device live fields + params exceed the u32 observed mask: widen host_fields_observed"
    );
    1 << DEVICE_LIVE.keys.len()
});

/// `device.delete-target`: the delete target names the device
/// ([`DeviceSlot::delete_target`]).
pub(super) fn device_delete_target(sources: &KindsHandles, device: &DeviceSource) -> bool {
    let current = sources.current_track.load(Ordering::Relaxed);
    let Some(target) = device.device.delete_target(device.owner, current) else {
        return false;
    };
    sources.active_delete_target.lock().unwrap().as_ref() == Some(&target)
}

/// One step's p-lock render (`plocked`, `lock-kind`, `variant-color`, and
/// the `vid` of the step variant it plays, `variant`).
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct StepPlockRender {
    pub(super) plocked: bool,
    pub(super) kind: u8,
    pub(super) color: [f32; 3],
    pub(super) variant: Option<u64>,
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
            variant: render.vid,
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
    /// families): the existing one while its descriptor and position are
    /// unchanged; else a new one. On a descriptor change (another effect or
    /// instrument in the device) its params and tensors are dropped and,
    /// when the params were registered, registered fresh with `d.params`
    /// re-pushed; the tensors are registered with the device
    /// ([`sync_device_tensors`]). Returns whether param instances were
    /// replaced.
    pub(super) fn sync_device_source(
        pusher: &mut Pusher<'_>,
        app: &app::App,
        device_id: InstanceId,
        owner: usize,
        device: DeviceSlot,
        (desc, sampler_depths): (Option<&EffectDescriptor>, bool),
    ) -> bool {
        let sampler = match device {
            DeviceSlot::Instrument => SamplerPlayhead::of(app, owner),
            _ => None,
        };
        let existing = pusher.shared.borrow().devices.get(&device_id).cloned();
        if existing.is_none() {
            // A device just registered reads the defaults of the fields
            // computed only while observed (the no-media ones, no sound
            // binding) until they are.
            push_all_media_defaults(pusher, device_id);
            pusher.push(
                device_id,
                f::DEVICE_SOUND_BINDING,
                Value::String(String::new()),
            );
        }
        let same_desc = existing
            .as_ref()
            .is_some_and(|source| source.desc.same(desc, sampler_depths));
        if let Some(source) = &existing {
            let same_sampler = match (&source.sampler, &sampler) {
                (Some(a), Some(b)) => a.same_voices(b),
                (a, b) => a.is_none() && b.is_none(),
            };
            if same_desc && source.owner == owner && source.device == device && same_sampler {
                return false;
            }
        }
        let desc = match &existing {
            Some(source) if same_desc => source.desc.clone(),
            _ => Rc::new(DeviceDescriptor::of(desc, sampler_depths)),
        };
        let source = Rc::new(DeviceSource {
            owner,
            device,
            desc,
            sampler,
        });
        let registered = {
            let mut shared = pusher.shared.borrow_mut();
            shared.devices.insert(device_id, source.clone());
            shared.devices_generation += 1;
            if same_desc && existing.is_some() {
                return false;
            }
            shared.param_devices.remove(&device_id)
        };
        let doomed: Vec<InstanceId> = (pusher.rt.keyed_children_of_kind(device_id, PARAM))
            .chain(pusher.rt.keyed_children_of_kind(device_id, TENSOR))
            .chain(pusher.rt.keyed_children_of_kind(device_id, MODULATOR))
            .map(|(id, _)| id)
            .collect();
        for id in doomed {
            pusher.rt.drop_instance(id);
            pusher.changed = true;
        }
        sync_device_tensors(pusher, device_id, &source);
        sync_device_modulators(pusher, device_id, &source);
        if existing.is_none() {
            return false;
        }
        pusher.shared.borrow_mut().params_generation += 1;
        if registered {
            if let Some(params) = register_params(&mut *pusher.rt, pusher.shared, device_id) {
                pusher.push(device_id, f::DEVICE_PARAMS, instance_list(params));
                pusher.changed = true;
            }
        }
        true
    }

    /// Keep each track instrument's sampler voices (what `device.playhead`
    /// samples, observed or cold) current: a sample load or a voice
    /// rebuild moves no model counter, so the tick compares them with the
    /// `App`'s every tick, allocating nothing unless they moved
    /// ([`SamplerPlayhead::current`]).
    pub(super) fn refresh_sampler_playheads(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        for (track, id) in self.track_ids.iter().enumerate() {
            let Some(track_id) = *id else { continue };
            let Some(device) = pusher.rt.keyed_instance(DEVICE, &[track_id, 0]) else {
                continue;
            };
            let Some(source) = pusher.shared.borrow().devices.get(&device).cloned() else {
                continue;
            };
            if SamplerPlayhead::current(source.sampler.as_ref(), app, track) {
                continue;
            }
            let fresh = DeviceSource {
                owner: source.owner,
                device: source.device,
                desc: source.desc.clone(),
                sampler: SamplerPlayhead::of(app, track),
            };
            pusher
                .shared
                .borrow_mut()
                .devices
                .insert(device, Rc::new(fresh));
            pusher.shared.borrow_mut().sampler_refreshes += 1;
        }
    }

    /// The observed device fields (`params` registered once observed),
    /// then the observed param fields, both from lists kept per observer
    /// epoch ([`ObservedList`]): a tick costs work in proportion to what is
    /// observed, not to what is registered. Fields that move only with
    /// their track's [`PlockKey`] (a param's `has-locks`, `process-mapped`
    /// and `key-locks`; a device's `key-locked-notes` and `variants`) are
    /// recomputed only when it moved.
    pub(super) fn sync_device_live(&mut self, pusher: &mut Pusher<'_>) {
        let devices = all_device_ids(&self.device_ids, &self.devices);
        (self.device_observed).refresh(pusher.rt, &DEVICE_OBSERVED, || devices.collect());
        let bits = DeviceBits::get();
        let mut mod_display = false;
        let mut modulator_meters = false;
        let mut sampler_playhead = false;
        for index in 0..self.device_observed.entries.len() {
            let (id, mut mask, seen) = self.device_observed.entries[index];
            let Some(source) = pusher.shared.borrow().devices.get(&id).cloned() else {
                continue;
            };
            mod_display |= mask & bits.mod_phases != 0 && mod_sampled(pusher.sources, &source);
            // The tick watches the current track's sampler voices alone.
            sampler_playhead |= mask & bits.playhead != 0
                && source.sampler.is_some()
                && source.owner == pusher.sources.current_track.load(Ordering::Relaxed);
            // Only a modulator track's instrument reads the envelopes.
            modulator_meters |= mask & (bits.modulator_phase | bits.modulator_level) != 0
                && source.device == DeviceSlot::Instrument
                && (pusher.rt.instance_field(id, f::DEVICE_TYPE.1))
                    .is_ok_and(|kind| matches!(kind, Value::String(kind) if kind == "modulator"));
            if mask & bits.plock_keyed != 0 {
                let key = pusher.sources.plock_key(source.owner);
                if seen == Some(key) {
                    mask &= !bits.plock_keyed;
                }
                self.device_observed.entries[index].2 = Some(key);
            }
            let strips = mask & bits.strips;
            if strips != 0 {
                let (sources, shared) = (pusher.sources, pusher.shared);
                (self.devices).queue_strips(sources, shared, id, &source, strips);
            }
            self.push_device_fields(pusher, id, &source, mask & !strips);
            let registered = pusher.shared.borrow().param_devices.contains(&id);
            if mask & *DEVICE_PARAMS_BIT != 0 && !registered {
                if let Some(params) = register_params(&mut *pusher.rt, pusher.shared, id) {
                    pusher.push_computed(id, f::DEVICE_PARAMS, instance_list(params));
                    pusher.changed = true;
                    self.param_observed.reset();
                }
            }
        }
        self.devices.push_strips(pusher);
        // `table-options` is pushed again once a device observes it anew.
        let observed = &self.device_observed.entries;
        (self.panel.table_options).retain(|id, _| {
            (observed.iter()).any(|(entry, mask, _)| entry == id && mask & bits.table_options != 0)
        });
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
        let plock_keyed = PARAM_BITS.plock_keyed();
        let (sources, shared) = (pusher.sources, pusher.shared);
        let visible = &mut VisibleCache::default();
        for entry in &mut self.param_observed.entries {
            let (id, mut mask, seen) = *entry;
            let Some(&[device_id, param]) = pusher.rt.instance_key(id) else {
                continue;
            };
            let Some(device) = shared.borrow().devices.get(&device_id).cloned() else {
                continue;
            };
            // `mod-phase` reads the sample for a source's setting alone.
            let source_setting = || {
                let pdesc = device.params().get(param as usize);
                pdesc.and_then(param_source_slot).is_some()
            };
            let mod_phase = mask & PARAM_BITS.mod_phase != 0 && source_setting();
            let mod_read = mask & PARAM_BITS.mod_values() != 0 || mod_phase;
            mod_display |= mod_read && mod_sampled(sources, &device);
            // `has-locks` scans the slot's p-locks, `key-locks` and
            // `process-mapped` read caches under the same key: only after
            // they may have moved.
            if mask & plock_keyed != 0 {
                let key = sources.plock_key(param_track(sources, &device));
                if seen == Some(key) {
                    mask &= !plock_keyed;
                }
                entry.2 = Some(key);
            }
            param_live_fields(
                sources,
                shared,
                &device,
                param as usize,
                mask,
                visible,
                |key, field| match field {
                    ParamField::Text(text) => pusher.push_text(id, key, text),
                    field => pusher.push_computed(id, key, field.value()),
                },
            );
        }
        self.panel.mod_display_observed = mod_display;
        self.panel.modulator_meters_observed = modulator_meters;
        self.panel.sampler_playhead_observed = sampler_playhead;
    }

    /// One observed device's live fields in `mask`.
    fn push_device_fields(
        &mut self,
        pusher: &mut Pusher<'_>,
        id: InstanceId,
        source: &DeviceSource,
        mask: u32,
    ) {
        let bits = DeviceBits::get();
        let (sources, shared) = (pusher.sources, pusher.shared);
        if mask & bits.playhead != 0 {
            // The voices `refresh_sampler_playheads` keeps current.
            let seconds = source
                .sampler
                .as_ref()
                .map_or(0.0, SamplerPlayhead::seconds);
            pusher.push_computed(id, f::DEVICE_PLAYHEAD, number(seconds));
        }
        if mask & bits.delete_target != 0 {
            let held = device_delete_target(sources, source);
            pusher.push_computed(id, f::DEVICE_DELETE_TARGET, Value::Bool(held));
        }
        if mask & bits.mod_phases != 0 {
            let phases = device_mod_phases(sources, shared, source);
            pusher.push_numbers(id, f::DEVICE_MOD_PHASES, &phases);
        }
        if mask & bits.key_locked_notes != 0 {
            key_locked_notes(sources, shared, source, |notes| {
                pusher.push_numbers(id, f::DEVICE_KEY_LOCKED_NOTES, notes);
            });
        }
        if mask & bits.variants != 0 {
            let variants = device_variants(&mut *pusher.rt, sources, shared, id, source);
            pusher.push_computed(id, f::DEVICE_VARIANTS, variants);
            self.panel.variant_observed.reset();
        }
        if mask & bits.modulator_phase != 0 {
            let phase = device_modulator_meter(shared, source, true);
            pusher.push_computed(id, f::DEVICE_MODULATOR_PHASE, number(phase));
        }
        if mask & bits.modulator_level != 0 {
            let level = device_modulator_meter(shared, source, false);
            pusher.push_computed(id, f::DEVICE_MODULATOR_LEVEL, number(level));
        }
        if mask & bits.tables != 0 {
            self.push_table_fields(pusher, id, source, mask & bits.tables);
        }
    }
}

/// The observed-mask bits of the device live fields.
struct DeviceBits {
    playhead: u32,
    delete_target: u32,
    mod_phases: u32,
    key_locked_notes: u32,
    variants: u32,
    plock_keyed: u32,
    /// The strip fields' ([`strip_keys`]).
    strips: u32,
    modulator_phase: u32,
    modulator_level: u32,
    /// The effect table fields' ([`table_keys`]), `table-options`'s.
    tables: u32,
    table_options: u32,
}

impl DeviceBits {
    fn get() -> &'static Self {
        static BITS: LazyLock<DeviceBits> = LazyLock::new(|| {
            let key_locked_notes = DEVICE_LIVE.bit(f::DEVICE_KEY_LOCKED_NOTES);
            let variants = DEVICE_LIVE.bit(f::DEVICE_VARIANTS);
            DeviceBits {
                playhead: DEVICE_LIVE.bit(f::DEVICE_PLAYHEAD),
                delete_target: DEVICE_LIVE.bit(f::DEVICE_DELETE_TARGET),
                mod_phases: DEVICE_LIVE.bit(f::DEVICE_MOD_PHASES),
                key_locked_notes,
                variants,
                plock_keyed: key_locked_notes | variants,
                strips: DEVICE_LIVE.bits(&strip_keys().collect::<Vec<_>>()),
                modulator_phase: DEVICE_LIVE.bit(f::DEVICE_MODULATOR_PHASE),
                modulator_level: DEVICE_LIVE.bit(f::DEVICE_MODULATOR_LEVEL),
                tables: DEVICE_LIVE.bits(&table_keys().collect::<Vec<_>>()),
                table_options: DEVICE_LIVE.bit(f::DEVICE_TABLE_OPTIONS),
            }
        });
        &BITS
    }
}

/// `device.variants`: a track instrument's key-lock variants (registering
/// their instances); empty for any other device.
pub(super) fn device_variants<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    source: &DeviceSource,
) -> Value {
    let track = source.owner;
    let owner = (source.device == DeviceSlot::Instrument)
        .then(|| store.keyed(TRACK, &[track as u64]))
        .flatten();
    match owner {
        Some(track_id) => owner_variants(
            store,
            sources,
            shared,
            (id, track_id),
            track,
            VariantScope::Keys,
        ),
        None => instance_list([]),
    }
}
