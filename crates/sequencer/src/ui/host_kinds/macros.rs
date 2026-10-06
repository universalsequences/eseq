//! Macros (spec §14.2g, stage 7b-3): the project's macros (`macro`,
//! `project.macros`, legacy `SEQ.macros`), a drum rack's macros
//! (`rack-macro`, the rack instrument device's `macros`, legacy
//! `SEQ.instrument-panel :macros`), and what each drives (`macro-mapping`).
//!
//! **Identity.** Project macros are positional (`(index)`), the instance
//! kept by macro id across reorders (`registry::reconcile`) and replaced on
//! a project load (the track registry's generation). A rack macro is keyed
//! (rack instrument device instance id, macro index 0-7), so it goes with
//! its rack. A mapping is keyed (its macro's instance id, position), as the
//! legacy commands address it. A mapping's `target` is the param instance it
//! drives: the target device's params are registered for it.
//!
//! **Feeds.** The project macros' structure (everything but the values) is
//! compared every tick with the last synced (a handful of macros), and the
//! sync also runs when what targets resolve through moved ([`MacroInputs`]:
//! the param registrations, the device registry, the FX epoch, the track,
//! bus, scene and device instances); the values are compared every tick
//! (a macro drag moves no counter). The racks' macros sync when the rack
//! revision (any rack edit) or the inputs moved, skipping a rack whose
//! macro names and mappings did not change (a macro knob drag does no
//! sync). None reads the history revision. A rack macro's value, base and
//! lock flags are live (an [`ObservedList`]): one rack lock per tick fills
//! every observed field, `has-locks` recomputed only when its track's
//! [`PlockKey`] moved.
//!
//! **Mapping handles.** A mapping has no stable id in the model, so its
//! instance is positional: deleting a mapping retargets the handles of the
//! mappings after it (each then names the mapping now at its position).

use super::*;
use sequencer::macro_engine::{Macro, MacroKind, StealQuantize};
use sequencer::sequencer::{RackMacroMapping, RackMacroTarget};

/// What macro targets resolve through: compared in place every tick (no
/// allocation unless it moved).
#[derive(Default)]
struct MacroInputs {
    /// The param registrations' generation, the device registry's, the FX
    /// epoch.
    scalars: (u64, u64, usize),
    tracks: Vec<Option<InstanceId>>,
    buses: Vec<Option<InstanceId>>,
    scenes: Vec<Option<InstanceId>>,
    devices: Vec<InstanceId>,
}

impl HostKinds {
    /// Make the macro syncs' [`MacroInputs`] what they resolve through now;
    /// returns whether that moved.
    fn refresh_macro_inputs(&mut self, pusher: &Pusher<'_>, app: &app::App) -> bool {
        let scalars = (
            pusher.shared.borrow().params_generation,
            app.device_registry.generation(),
            pusher.sources.fx_epoch.load(Ordering::Relaxed),
        );
        let devices = || all_device_ids(&self.device_ids, &self.devices);
        if let Some(inputs) = &self.macros.inputs {
            if inputs.scalars == scalars
                && inputs.tracks == self.track_ids
                && inputs.buses == self.bus_ids
                && inputs.scenes == self.scene_ids
                && inputs.devices.iter().copied().eq(devices())
            {
                return false;
            }
        }
        let inputs = self.macros.inputs.get_or_insert_with(MacroInputs::default);
        inputs.scalars = scalars;
        inputs.tracks.clone_from(&self.track_ids);
        inputs.buses.clone_from(&self.bus_ids);
        inputs.scenes.clone_from(&self.scene_ids);
        inputs.devices.clear();
        inputs
            .devices
            .extend(all_device_ids(&self.device_ids, &self.devices));
        true
    }
}

/// A rack's macro names and mappings as of its last sync.
type RackLayout = Vec<(String, Vec<RackMacroMapping>)>;

/// Whether `cached` is `rack`'s macro names and mappings (no allocation).
fn same_layout(
    cached: Option<&RackLayout>,
    rack: Option<&sequencer::sequencer::RackTrackSnapshot>,
) -> bool {
    match (cached, rack) {
        (None, None) => true,
        (Some(cached), Some(rack)) => {
            cached.len() == rack.macros.len()
                && (cached.iter().zip(&rack.macros))
                    .all(|((name, mappings), now)| *name == now.name && *mappings == now.mappings)
        }
        _ => false,
    }
}

/// The macro syncs' state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct MacroState {
    /// Macro id → instance.
    ids: HashMap<u64, InstanceId>,
    instances: Vec<Option<InstanceId>>,
    /// The project macros as of the last structure sync.
    seen: Option<Vec<Macro>>,
    inputs: Option<MacroInputs>,
    /// Values last pushed, by macro position.
    values: Vec<f32>,
    /// The rack revision of the last rack sync, and per track position the
    /// rack's macro names and mappings as of it.
    rack_revision: Option<u64>,
    racks: Vec<Option<RackLayout>>,
    /// Every rack macro instance, for the live loop.
    rack_ids: Vec<InstanceId>,
    pub(super) rack_observed: ObservedList,
    /// The observed live fields read under the rack lock, pushed after it.
    rack_pending: Vec<(InstanceId, FieldKey, Value)>,
    /// Project macro structure syncs, rack macro syncs (racks synced) and
    /// rack locks taken for the live fields, for tests.
    pub(crate) syncs: u64,
    pub(crate) rack_syncs: u64,
    pub(crate) rack_live_locks: u64,
}

impl MacroState {
    /// Sync everything at the next tick (a schema change, a hot reload).
    pub(super) fn invalidate(&mut self) {
        self.seen = None;
        self.inputs = None;
        self.rack_revision = None;
        self.values.clear();
    }

    /// Instances whose loss (a hot reload) means a full sync.
    pub(super) fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        (self.instances.iter().flatten()).chain(self.rack_ids.first())
    }

    /// Drop the project macros (a project load: macro ids restart).
    pub(super) fn drain(&mut self) -> impl Iterator<Item = (u64, InstanceId)> + '_ {
        self.seen = None;
        self.ids.drain()
    }
}

/// Whether two macros differ in nothing but their values.
fn same_structure(a: &Macro, b: &Macro) -> bool {
    a.id == b.id
        && a.key == b.key
        && a.name == b.name
        && a.kind == b.kind
        && a.mappings == b.mappings
}

/// One mapping's model fields, before its target resolves.
struct MappingModel {
    /// (owner instance, device did, param index) of the target param.
    target: Option<(InstanceId, u64, usize)>,
    label: String,
    min: f32,
    max: f32,
    curve: &'static str,
    suspended: bool,
}

/// Register a macro's mapping instances (keyed (macro instance id,
/// position)), push their fields, resolving each target to its param
/// instance (registering the target device's params), and return them.
fn sync_mappings(
    pusher: &mut Pusher<'_>,
    owner: InstanceId,
    rack: bool,
    models: Vec<MappingModel>,
) -> Vec<InstanceId> {
    let ids = indexed_children(
        &mut *pusher.rt,
        owner,
        MACRO_MAPPING,
        models.len(),
        |store, id, at| {
            let (project, rack_macro) = match rack {
                true => (None, Some(owner)),
                false => (Some(owner), None),
            };
            store.push(id, f::MAPPING_MACRO, instance_or_nil(project));
            store.push(id, f::MAPPING_RACK_MACRO, instance_or_nil(rack_macro));
            store.push(id, f::MAPPING_INDEX, number(at as f64));
        },
    );
    for (id, model) in ids.iter().zip(models) {
        let target = model.target.and_then(|(parent, did, index)| {
            let device = pusher.rt.keyed_instance(DEVICE, &[parent, did])?;
            if !pusher.shared.borrow().param_devices.contains(&device) {
                let params = register_params(&mut *pusher.rt, pusher.shared, device)?;
                pusher.push(device, f::DEVICE_PARAMS, instance_list(params));
            }
            pusher.rt.keyed_instance(PARAM, &[device, index as u64])
        });
        pusher.push(*id, f::MAPPING_TARGET, instance_or_nil(target));
        pusher.push(*id, f::MAPPING_LABEL, Value::String(model.label));
        pusher.push(*id, f::MAPPING_MIN, number(model.min));
        pusher.push(*id, f::MAPPING_MAX, number(model.max));
        pusher.push(*id, f::MAPPING_CURVE, text(model.curve));
        pusher.push(*id, f::MAPPING_SUSPENDED, Value::Bool(model.suspended));
    }
    ids
}

impl HostKinds {
    /// The project and rack macro syncs (see the module docs).
    pub(super) fn sync_macro_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        if (self.macros.representatives()).any(|id| !pusher.rt.instance_is_live(*id)) {
            // A hot reload dropped macro instances.
            self.macros.invalidate();
        }
        let inputs_moved = self.refresh_macro_inputs(pusher, app);
        let macros = app.macro_engine.macros();
        let structure_moved = !(self.macros.seen.as_ref()).is_some_and(|seen| {
            seen.len() == macros.len() && seen.iter().zip(macros).all(|(a, b)| same_structure(a, b))
        });
        let rack_revision = app.state.pattern.rack_tracks.revision();
        let racks_moved = inputs_moved || self.macros.rack_revision != Some(rack_revision);
        if racks_moved {
            self.macros.rack_revision = Some(rack_revision);
            self.sync_rack_macros(pusher, app, inputs_moved);
        }
        if structure_moved || inputs_moved {
            self.sync_project_macros(pusher, app);
            self.macros.seen = Some(macros.to_vec());
        }
        self.sync_macro_values(pusher, macros);
    }

    /// The project macros' instances and structure fields (the values are
    /// [`Self::sync_macro_values`]'s).
    fn sync_project_macros(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        self.macros.syncs += 1;
        let macros = app.macro_engine.macros();
        let model: Vec<u64> = macros.iter().map(|m| u64::from(m.id)).collect();
        let ids = reconcile(pusher, MACRO, &mut self.macros.ids, &model);
        self.macros.values.clear();
        for (index, (id, definition)) in ids.iter().zip(macros).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::MACRO_INDEX, number(index as f64));
            pusher.push(id, f::MACRO_MID, number(definition.id));
            let key = definition.key.clone().unwrap_or_default();
            pusher.push(id, f::MACRO_KEY, Value::String(key));
            pusher.push(id, f::MACRO_NAME, Value::String(definition.name.clone()));
            let (kind, scene) = match &definition.kind {
                MacroKind::Mapped => ("mapped", None),
                MacroKind::Scene(config) => ("scene", Some(config)),
            };
            pusher.push(id, f::MACRO_TYPE, text(kind));
            let target_scene = scene.and_then(|config| self.scene_ids.get(config.target_scene));
            pusher.push(
                id,
                f::MACRO_TARGET_SCENE,
                instance_or_nil(target_scene.copied().flatten()),
            );
            let morph = scene.is_some_and(|config| config.morph_params);
            pusher.push(id, f::MACRO_MORPH_PARAMS, Value::Bool(morph));
            let steal = scene.is_some_and(|config| config.steal_patterns);
            pusher.push(id, f::MACRO_STEAL_PATTERNS, Value::Bool(steal));
            let quantize = scene.map_or("", |config| match config.quantize {
                StealQuantize::Off => "off",
                StealQuantize::Sixteenth => "sixteenth",
                StealQuantize::Bar => "bar",
            });
            pusher.push(id, f::MACRO_QUANTIZE, text(quantize));
            let models = definition
                .mappings
                .iter()
                .map(|mapping| {
                    let (path, _, min, max, ..) = macro_mapping_display_metadata(app, mapping);
                    let target =
                        macro_mapping_location(app, mapping).and_then(|(owner, device, index)| {
                            let parent = match device {
                                DeviceSlot::BusEffect(_) => self.bus_ids.get(owner),
                                _ => self.track_ids.get(owner),
                            };
                            Some((parent.copied().flatten()?, device.did(app, owner), index))
                        });
                    let label = format!("{path} · {}", process_param_target_label(&mapping.target));
                    MappingModel {
                        target,
                        label,
                        min,
                        max,
                        curve: mapping.curve.label(),
                        suspended: mapping.suspended,
                    }
                })
                .collect();
            let mappings = sync_mappings(pusher, id, false, models);
            pusher.push(id, f::MACRO_MAPPINGS, instance_list(mappings));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(project, f::PROJECT_MACROS, listed_instances(&ids));
        }
        self.macros.instances = ids;
    }

    /// The project macros' values, compared every tick with the last push.
    fn sync_macro_values(&mut self, pusher: &mut Pusher<'_>, macros: &[Macro]) {
        let values = &mut self.macros.values;
        if values.len() != macros.len() {
            values.clear();
            values.resize(macros.len(), f32::NAN);
        }
        for (at, definition) in macros.iter().enumerate() {
            if values[at].to_bits() == definition.value.to_bits() {
                continue;
            }
            let Some(Some(id)) = self.macros.instances.get(at) else {
                continue;
            };
            pusher.push(*id, f::MACRO_VALUE, number(definition.value));
            values[at] = definition.value;
        }
    }

    /// Every drum rack's macros (on its instrument device), under the rack
    /// lock; with `all` false, a rack whose macro names and mappings did
    /// not change is skipped.
    fn sync_rack_macros(&mut self, pusher: &mut Pusher<'_>, app: &app::App, all: bool) {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let state = &mut self.macros;
        state.racks.resize_with(self.track_ids.len(), || None);
        state.racks.truncate(self.track_ids.len());
        let mut changed = false;
        for (track, track_id) in self.track_ids.iter().enumerate() {
            let Some(track_id) = *track_id else { continue };
            let Some(instrument) = pusher.rt.keyed_instance(DEVICE, &[track_id, 0]) else {
                continue;
            };
            let rack = racks.get(track).and_then(Option::as_ref);
            if !all && same_layout(state.racks[track].as_ref(), rack) {
                continue;
            }
            state.rack_syncs += 1;
            changed = true;
            state.racks[track] = rack.map(|rack| {
                (rack.macros.iter())
                    .map(|rack_macro| (rack_macro.name.clone(), rack_macro.mappings.clone()))
                    .collect()
            });
            let macros = rack.map_or(&[][..], |rack| &rack.macros[..]);
            let ids = indexed_children(
                &mut *pusher.rt,
                instrument,
                RACK_MACRO,
                macros.len(),
                |store, id, at| {
                    store.push(id, f::RACK_MACRO_DEVICE, Value::Instance(instrument));
                    store.push(id, f::RACK_MACRO_INDEX, number(at as f64));
                },
            );
            for (id, rack_macro) in ids.iter().zip(macros) {
                pusher.push(
                    *id,
                    f::RACK_MACRO_KEY,
                    Value::String(rack_macro.id.stable_key()),
                );
                pusher.push(
                    *id,
                    f::RACK_MACRO_NAME,
                    Value::String(rack_macro.name.clone()),
                );
                let rack = rack.expect("a rack holds these macros");
                let models = (rack_macro.mappings.iter())
                    .map(|mapping| {
                        let (path, param, min, max, ..) =
                            rack_macro_mapping_display_metadata(app, rack, mapping);
                        let device = match &mapping.target {
                            RackMacroTarget::SlotInstrumentParam {
                                slot, param_index, ..
                            } => Some((DeviceSlot::RackSlot(*slot), *param_index)),
                            RackMacroTarget::SlotEffectParam {
                                slot,
                                effect_slot,
                                param_index,
                                ..
                            } => Some((
                                DeviceSlot::RackEffect {
                                    rack_slot: *slot,
                                    slot: *effect_slot,
                                },
                                *param_index,
                            )),
                            RackMacroTarget::SlotParam { .. } => None,
                        };
                        MappingModel {
                            target: device
                                .map(|(device, index)| (track_id, device.did(app, track), index)),
                            label: format!("{path} · {param}"),
                            min,
                            max,
                            curve: mapping.curve.label(),
                            suspended: false,
                        }
                    })
                    .collect();
                let mappings = sync_mappings(pusher, *id, true, models);
                pusher.push(*id, f::RACK_MACRO_MAPPINGS, instance_list(mappings));
            }
            pusher.push(instrument, f::DEVICE_MACROS, instance_list(ids));
        }
        drop(racks);
        if changed || all {
            let ids = children_of_owners(pusher.rt, self.device_ids.iter().copied(), RACK_MACRO);
            self.macros.rack_ids = ids;
            self.macros.rack_observed.reset();
        }
    }

    /// The observed live fields of every rack macro, read under one rack
    /// lock and pushed after it; `has-locks` only when its track's
    /// [`PlockKey`] moved.
    pub(super) fn sync_rack_macro_live(&mut self, pusher: &mut Pusher<'_>) {
        let state = &mut self.macros;
        let ids = &state.rack_ids;
        (state.rack_observed).refresh(pusher.rt, &RACK_MACRO_LIVE.names, || ids.clone());
        if state.rack_observed.entries.is_empty() {
            return;
        }
        let has_locks = RACK_MACRO_LIVE.bit(f::RACK_MACRO_HAS_LOCKS);
        let (sources, shared) = (pusher.sources, pusher.shared);
        let pending = &mut state.rack_pending;
        {
            let racks = sources.state.pattern.rack_tracks.lock().unwrap();
            state.rack_live_locks += 1;
            for entry in &mut state.rack_observed.entries {
                let (id, mut mask, seen) = *entry;
                let Some(&[device, index]) = pusher.rt.instance_key(id) else {
                    continue;
                };
                let Some(track) = shared.borrow().devices.get(&device).map(|d| d.owner) else {
                    continue;
                };
                if !sources.track_exists(track) {
                    continue;
                }
                if mask & has_locks != 0 {
                    let key = sources.plock_key(track);
                    if seen == Some(key) {
                        mask &= !has_locks;
                    }
                    entry.2 = Some(key);
                }
                let rack = racks.get(track).and_then(Option::as_ref);
                let Some(rack_macro) = rack.and_then(|rack| rack.macros.get(index as usize)) else {
                    continue;
                };
                for (bit, key) in RACK_MACRO_LIVE.keys.iter().enumerate() {
                    if mask & (1 << bit) == 0 {
                        continue;
                    }
                    if let Some(value) = rack_macro_field(sources, shared, track, rack_macro, *key)
                    {
                        pending.push((id, *key, value));
                    }
                }
            }
        }
        for (id, key, value) in pending.drain(..) {
            pusher.push_computed(id, key, value);
        }
    }
}

/// One live field of rack macro `rack_macro` of `track` (read under the
/// rack lock): its shown value (a take's override, else the displayed
/// step's p-lock, else its own value under an engaged project macro:
/// `app::rack_macro_shown_value`, shared with the panel), its own value,
/// whether the displayed step locks it, whether some step of the pattern
/// does.
fn rack_macro_field(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
    rack_macro: &sequencer::sequencer::RackMacro,
    key: FieldKey,
) -> Option<Value> {
    let lock_at = |step: usize| rack_macro.plocks.get(step).copied().flatten();
    Some(match key {
        f::RACK_MACRO_VALUE => {
            let state = &sources.state;
            let take =
                state.take_rack_macro_override.values_for_track(track)[rack_macro.id.index()];
            let step = sources.plock_display_step(track);
            let overrides = &shared.borrow().macro_overrides;
            number(app::rack_macro_shown_value(
                take, overrides, track, rack_macro, step,
            ))
        }
        f::RACK_MACRO_BASE => number(rack_macro.value),
        f::RACK_MACRO_LOCKED => {
            let step = sources.plock_display_step(track);
            Value::Bool(step.and_then(lock_at).is_some())
        }
        f::RACK_MACRO_HAS_LOCKS => {
            Value::Bool((0..sources.num_steps(track)).any(|step| lock_at(step).is_some()))
        }
        _ => return None,
    })
}

/// One live field of a rack macro (the reader hook's cold read).
pub(super) fn rack_macro_live_value<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    let &[device, index] = store.key_of(id)? else {
        return None;
    };
    let track = shared.borrow().devices.get(&device)?.owner;
    if !sources.track_exists(track) {
        return None;
    }
    let racks = sources.state.pattern.rack_tracks.lock().unwrap();
    let rack_macro = racks.get(track)?.as_ref()?.macros.get(index as usize)?;
    rack_macro_field(sources, shared, track, rack_macro, key)
}
