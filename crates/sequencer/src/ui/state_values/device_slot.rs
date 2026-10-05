//! [`DeviceSlot`]: one device by position, shared by the host kinds' device
//! syncs, their params and their setters (kind-bindings spec §14.2b, §14.2f).

use super::*;

/// The base of the `device.did` of an effect slot with no instance id bound
/// yet: below 2^53, so it survives a Lisp number, and far above any
/// allocated instance id. The other families' unbound ids sit above it, in
/// ranges of their own ([`DeviceSlot::did`], [`DeviceSlot::decode_unbound`]).
pub(crate) const UNBOUND_EFFECT_DID: u64 = 1 << 52;
const UNBOUND_MIDI_FX_DID: u64 = UNBOUND_EFFECT_DID + (1 << 40);
const UNBOUND_RACK_SLOT_DID: u64 = UNBOUND_EFFECT_DID + (2 << 40);
/// Plus `rack_slot << 20 | slot`.
const UNBOUND_RACK_EFFECT_DID: u64 = UNBOUND_EFFECT_DID + (3 << 40);
const UNBOUND_RANGE: u64 = 1 << 40;

/// One device, by position: the instrument or an effect of a track's chain,
/// a MIDI effect of the track, a drum rack's slot (its instrument) or an
/// effect of a rack slot, or an effect of a bus. Every method takes the
/// device's *owner* position: the track's, or the bus's for
/// [`Self::BusEffect`]. What the host kinds' params, their setters and the
/// device syncs share, so none carries a per-family branch of its own.
/// Values cross it in display (user) units: percent params read ×100
/// ([`Self::to_user`], [`Self::from_user_clamped`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DeviceSlot {
    Instrument,
    Effect(usize),
    MidiFx(usize),
    /// A drum rack's slot: its instrument, its voices and its effects.
    RackSlot(usize),
    RackEffect {
        rack_slot: usize,
        slot: usize,
    },
    /// An effect of a bus (the owner is the bus position).
    BusEffect(usize),
}

impl DeviceSlot {
    /// From a `DeviceChainEntry::slot` (-1 for the instrument).
    pub(crate) fn from_chain_slot(slot: i64) -> Self {
        usize::try_from(slot).map_or(Self::Instrument, Self::Effect)
    }

    /// `device.role`.
    pub(crate) fn role(self) -> &'static str {
        match self {
            Self::Instrument => "instrument",
            Self::Effect(_) => "effect",
            Self::MidiFx(_) => "midi-fx",
            Self::RackSlot(_) => "rack-slot",
            Self::RackEffect { .. } => "rack-effect",
            Self::BusEffect(_) => "bus-effect",
        }
    }

    /// The position `device.slot` shows: -1 for the instrument, else the
    /// device's place in its own chain (effect, MIDI effect, rack slot,
    /// rack slot effect or bus effect slot).
    pub(crate) fn chain_slot(self) -> i64 {
        match self {
            Self::Instrument => -1,
            Self::Effect(slot)
            | Self::MidiFx(slot)
            | Self::RackSlot(slot)
            | Self::RackEffect { slot, .. }
            | Self::BusEffect(slot) => slot as i64,
        }
    }

    /// The device's stable id under its owner (`device.did`, the key the
    /// host kinds register it under and the setters address it by): 0 for
    /// the instrument; else the identity the device registry binds to it
    /// (an effect's, a MIDI effect's, a rack slot's or a rack slot
    /// effect's instance id, all from one allocator), which a reorder
    /// keeps, or [`Self::unbound_did`] while none is bound yet.
    pub(crate) fn did(self, app: &app::App, owner: usize) -> u64 {
        let registry = &app.device_registry;
        let track = || app.track_registry.id_at(owner);
        let bound = match self {
            Self::Instrument => return 0,
            Self::Effect(slot) => {
                (track().and_then(|id| registry.audio_effect_id(id, slot))).map(|id| id.0)
            }
            Self::MidiFx(slot) => {
                (track().and_then(|id| registry.midi_effect_id(id, slot))).map(|id| id.0)
            }
            Self::RackSlot(slot) => {
                (track().and_then(|id| registry.rack_slot_id(id, slot))).map(|id| id.0)
            }
            Self::RackEffect { rack_slot, slot } => track()
                .and_then(|id| registry.rack_slot_id(id, rack_slot))
                .and_then(|id| registry.rack_audio_effect_id(id, slot))
                .map(|id| id.0),
            Self::BusEffect(slot) => app
                .buses
                .get(owner)
                .and_then(|bus| registry.bus_audio_effect_id(bus.id, slot))
                .map(|id| id.0),
        };
        bound.unwrap_or_else(|| self.unbound_did())
    }

    /// The placeholder `did` of a device with no identity bound yet: an
    /// unbound id of its family + its position (0 for the instrument).
    pub(crate) fn unbound_did(self) -> u64 {
        match self {
            Self::Instrument => 0,
            Self::Effect(slot) | Self::BusEffect(slot) => UNBOUND_EFFECT_DID + slot as u64,
            Self::MidiFx(slot) => UNBOUND_MIDI_FX_DID + slot as u64,
            Self::RackSlot(slot) => UNBOUND_RACK_SLOT_DID + slot as u64,
            Self::RackEffect { rack_slot, slot } => {
                UNBOUND_RACK_EFFECT_DID + ((rack_slot as u64) << 20) + slot as u64
            }
        }
    }

    /// The device a placeholder `did` ([`Self::unbound_did`]) names under a
    /// track (`bus` false) or a bus; `None` for a bound id or an unbound id
    /// of no family the owner holds.
    pub(crate) fn decode_unbound(did: u64, bus: bool) -> Option<Self> {
        let offset = did.checked_sub(UNBOUND_EFFECT_DID)?;
        let slot = (offset % UNBOUND_RANGE) as usize;
        Some(match (offset / UNBOUND_RANGE, bus) {
            (0, true) => Self::BusEffect(slot),
            (0, false) => Self::Effect(slot),
            (1, false) => Self::MidiFx(slot),
            (2, false) => Self::RackSlot(slot),
            (3, false) => Self::RackEffect {
                rack_slot: slot >> 20,
                slot: slot & ((1 << 20) - 1),
            },
            _ => return None,
        })
    }

    /// The placeholder `did` the identity `did` was bound from, under the
    /// track (`bus` false) or bus at `owner`: the device registry records,
    /// when it allocates an identity for an unbound device, the family and
    /// position the device sat at ([`app::DevicePlaceholder`]). `None` for an
    /// identity allocated for no unbound device (a new one, a persisted
    /// one) or for another owner. What re-keys a placeholder device
    /// instance to its new identity (the host kinds' `reconcile_devices`).
    pub(crate) fn placeholder_did(
        app: &app::App,
        bus: bool,
        owner: usize,
        did: u64,
    ) -> Option<u64> {
        use app::DevicePlaceholder as P;
        let track = || app.track_registry.id_at(owner);
        let device = match (app.device_registry.placeholder(did)?, bus) {
            (P::AudioEffect { track: id, slot }, false) if track() == Some(id) => {
                Self::Effect(slot)
            }
            (P::MidiEffect { track: id, slot }, false) if track() == Some(id) => Self::MidiFx(slot),
            (P::RackSlot { track: id, slot }, false) if track() == Some(id) => Self::RackSlot(slot),
            (
                P::RackEffect {
                    track: id,
                    rack_slot,
                    slot,
                },
                false,
            ) if track() == Some(id) => Self::RackEffect { rack_slot, slot },
            (P::BusEffect { bus: id, slot }, true)
                if app.buses.get(owner).is_some_and(|bus| bus.id == id) =>
            {
                Self::BusEffect(slot)
            }
            _ => return None,
        };
        Some(device.unbound_did())
    }

    /// The track position and device a (`TrackId`, [`Self::did`]) pair
    /// names now; `None` once the track or the device is gone. A
    /// placeholder `did` names the device at its family and position while
    /// one is there, even once an identity is bound to it (two edits of one
    /// unbound device in one batch, the first binding it, both land; a drag
    /// keeps landing until the next sync re-keys the device).
    pub(crate) fn resolve(
        app: &app::App,
        track_id: sequencer::sequencer::TrackId,
        did: u64,
    ) -> Option<(usize, Self)> {
        let track = live_track_index(app, track_id)?;
        if did >= UNBOUND_EFFECT_DID {
            let device = Self::decode_unbound(did, false)?;
            return device.present(app, track).then_some((track, device));
        }
        let registry = &app.device_registry;
        let device = match did {
            0 => Self::Instrument,
            did => {
                let effect = sequencer::sequencer::EffectInstanceId(did);
                let rack_slot = |id| {
                    let (owner, slot) = registry.rack_slot_location(id)?;
                    (owner == track_id).then_some(slot)
                };
                if let Some((owner, slot)) = registry.audio_effect_location(effect) {
                    (owner == track_id).then_some(Self::Effect(slot))?
                } else if let Some((owner, slot)) =
                    registry.midi_effect_location(sequencer::sequencer::MidiFxInstanceId(did))
                {
                    (owner == track_id).then_some(Self::MidiFx(slot))?
                } else if let Some(slot) = rack_slot(sequencer::sequencer::RackSlotId(did)) {
                    Self::RackSlot(slot)
                } else {
                    let (owner, slot) = registry.rack_audio_effect_location(effect)?;
                    Self::RackEffect {
                        rack_slot: rack_slot(owner)?,
                        slot,
                    }
                }
            }
        };
        (device.present(app, track) && device.did(app, track) == did).then_some((track, device))
    }

    /// The bus position and effect a (`BusId`, [`Self::did`]) pair names
    /// now; `None` once the bus or the effect is gone. A placeholder `did`
    /// as in [`Self::resolve`].
    pub(crate) fn resolve_bus(
        app: &app::App,
        bus_id: sequencer::sequencer::BusId,
        did: u64,
    ) -> Option<(usize, Self)> {
        let bus = app.buses.iter().position(|bus| bus.id == bus_id)?;
        if did >= UNBOUND_EFFECT_DID {
            let device = Self::decode_unbound(did, true)?;
            return device.present(app, bus).then_some((bus, device));
        }
        let effect = sequencer::sequencer::EffectInstanceId(did);
        let (owner, slot) = app.device_registry.bus_audio_effect_location(effect)?;
        let device = (owner == bus_id).then_some(Self::BusEffect(slot))?;
        (device.present(app, bus) && device.did(app, bus) == did).then_some((bus, device))
    }

    /// Whether the device is there: an instrument always (its descriptor
    /// may be empty), a MIDI effect while its chain names one at its slot,
    /// a rack slot while the rack has it (an empty slot has no descriptor),
    /// anything else while its slot holds a device (an unused chain slot
    /// has an empty descriptor). Reads no file.
    fn present(self, app: &app::App, owner: usize) -> bool {
        match self {
            Self::MidiFx(slot) => {
                (app.state.pattern.track_params.get(owner)).is_some_and(|params| {
                    params
                        .midi_fx_chain()
                        .get(slot)
                        .is_some_and(|name| !name.is_empty())
                })
            }
            Self::RackSlot(slot) => with_rack_slot(&app.state, owner, slot, |_, _| ()).is_some(),
            Self::RackEffect { rack_slot, slot } => {
                with_rack_slot(&app.state, owner, rack_slot, |_, rack_slot| {
                    (rack_slot.effect_descriptors.get(slot))
                        .is_some_and(|desc| !desc.name.is_empty())
                })
                .unwrap_or(false)
            }
            _ => self
                .descriptor(app, owner, &[])
                .is_some_and(|desc| self == Self::Instrument || !desc.name.is_empty()),
        }
    }

    /// The device's descriptor. Borrowed from the `App` where it lives
    /// there; a MIDI effect's from `midi_fx` (the MIDI effect library's
    /// descriptors, which the host kinds cache: never loaded here) by its
    /// chain name, and a rack slot effect's copied out of the rack, so
    /// callers that walk many devices take them from a table of their own
    /// (the host kinds' device sync).
    pub(crate) fn descriptor<'a>(
        self,
        app: &'a app::App,
        owner: usize,
        midi_fx: &'a [sequencer::effects::EffectDescriptor],
    ) -> Option<std::borrow::Cow<'a, sequencer::effects::EffectDescriptor>> {
        use std::borrow::Cow;
        match self {
            Self::Instrument => app
                .graph
                .instrument_descriptors
                .get(owner)
                .map(Cow::Borrowed),
            Self::Effect(slot) => app
                .graph
                .effect_descriptors
                .get(owner)?
                .get(slot)
                .map(Cow::Borrowed),
            Self::MidiFx(slot) => {
                let chain = app.state.pattern.track_params.get(owner)?.midi_fx_chain();
                let name = chain.get(slot)?;
                (midi_fx.iter())
                    .find(|desc| desc.name.eq_ignore_ascii_case(name))
                    .map(Cow::Borrowed)
            }
            Self::RackSlot(slot) => with_rack_slot(&app.state, owner, slot, |_, rack_slot| {
                app.rack_slot_descriptor(rack_slot)
            })?
            .map(Cow::Borrowed),
            Self::RackEffect { rack_slot, slot } => {
                with_rack_slot(&app.state, owner, rack_slot, |_, rack_slot| {
                    rack_slot.effect_descriptors.get(slot).cloned()
                })?
                .map(Cow::Owned)
            }
            Self::BusEffect(slot) => app
                .buses
                .get(owner)?
                .effect_descriptors
                .get(slot)
                .map(Cow::Borrowed),
        }
    }

    /// The live slot state of a track chain device or MIDI effect; `None`
    /// for the families whose values live in a snapshot (rack slots, bus
    /// effects: [`Self::with_values`]).
    pub(crate) fn slot_state(
        self,
        state: &SequencerState,
        track: usize,
    ) -> Option<&sequencer::effects::EffectSlotState> {
        match self {
            Self::Instrument => state.pattern.instrument_slots.get(track),
            Self::Effect(slot) => state.pattern.effect_chains.get(track)?.get(slot),
            Self::MidiFx(slot) => state.pattern.midi_fx_slots.get(track)?.get(slot),
            Self::RackSlot(_) | Self::RackEffect { .. } | Self::BusEffect(_) => None,
        }
    }

    /// Run `read` over where the device's param values live: the live
    /// slot state, the rack's snapshot (under the rack lock, with the rack
    /// for its display rules) or the bus's (`buses`: the `App`'s, or the
    /// shared copy the natives read).
    pub(crate) fn with_values<R>(
        self,
        state: &SequencerState,
        buses: &[app::BusChannelState],
        owner: usize,
        read: impl FnOnce(DeviceValues<'_>) -> R,
    ) -> Option<R> {
        match self {
            Self::Instrument | Self::Effect(_) | Self::MidiFx(_) => {
                Some(read(DeviceValues::Live(self.slot_state(state, owner)?)))
            }
            Self::RackSlot(slot)
            | Self::RackEffect {
                rack_slot: slot, ..
            } => with_rack_slot(state, owner, slot, |rack, rack_slot| {
                let values = match self {
                    Self::RackEffect { slot, .. } => rack_slot.effect_slots.get(slot)?,
                    _ => &rack_slot.instrument_slot,
                };
                Some(read(DeviceValues::Rack { rack, values }))
            })?,
            Self::BusEffect(slot) => {
                let values = buses.get(owner)?.effect_slots.get(slot)?;
                Some(read(DeviceValues::Snapshot(values)))
            }
        }
    }

    /// A stored value in display units.
    pub(crate) fn to_user(pdesc: &sequencer::effects::ParamDescriptor, stored: f32) -> f32 {
        pdesc.stored_to_user(stored)
    }

    /// A display-unit value as stored: clamped to the range, and rounded
    /// for an enum or boolean param.
    pub(crate) fn from_user_clamped(pdesc: &sequencer::effects::ParamDescriptor, user: f32) -> f32 {
        let stored = pdesc.clamp(pdesc.user_input_to_stored(user));
        if pdesc.is_enum() || pdesc.is_boolean() {
            stored.round().clamp(pdesc.min, pdesc.max)
        } else {
            stored
        }
    }

    /// The project macro engine's key for a track chain param (an engaged
    /// macro shows in `param.value`); the other families show no macro
    /// override, like their legacy fields (a rack's own macros are part of
    /// the rack slot value derivation).
    pub(crate) fn macro_key(
        self,
        state: &SequencerState,
        track: usize,
        param_idx: usize,
    ) -> Option<sequencer::macro_engine::MacroParamKey> {
        match self {
            Self::Instrument => app::instrument_param_macro_key(state, track, param_idx),
            Self::Effect(slot) => app::effect_param_macro_key(state, track, slot, param_idx),
            _ => None,
        }
    }

    /// The print latch target of param `param_idx` (a bus effect's latches
    /// under the current track, as its knob does).
    pub(crate) fn print_target(self, owner: usize, param_idx: usize) -> PrintTarget {
        match self {
            Self::Instrument => PrintTarget::Instrument { param_idx },
            Self::Effect(slot_idx) => PrintTarget::Effect {
                slot_idx,
                param_idx,
            },
            Self::MidiFx(slot_idx) => PrintTarget::MidiFx {
                slot_idx,
                param_idx,
            },
            Self::RackSlot(slot_idx) => PrintTarget::RackSlotInstrument {
                slot_idx,
                param_idx,
            },
            Self::RackEffect { rack_slot, slot } => PrintTarget::RackSlotEffect {
                rack_slot_idx: rack_slot,
                effect_slot_idx: slot,
                param_idx,
            },
            Self::BusEffect(slot_idx) => PrintTarget::BusEffect {
                bus_idx: owner,
                slot_idx,
                param_idx,
            },
        }
    }

    /// The invalidation a base (`plock` false) or p-lock edit of a param
    /// queues; `None` for the families whose commands refresh their legacy
    /// fields directly (rack slots: the rack panel refresh; bus effects).
    pub(crate) fn invalidation(
        self,
        owner: usize,
        param: usize,
        plock: bool,
    ) -> Option<UiInvalidation> {
        let track = owner;
        Some(match (self, plock) {
            (Self::Instrument, false) => UiInvalidation::Instrument {
                track,
                change: InstrumentInvalidation::Param { param },
            },
            (Self::Instrument, true) => UiInvalidation::Instrument {
                track,
                change: InstrumentInvalidation::Plock { param },
            },
            (Self::Effect(slot), false) => UiInvalidation::TrackFx {
                track,
                change: TrackFxInvalidation::Param { slot, param },
            },
            (Self::Effect(slot), true) => UiInvalidation::TrackFx {
                track,
                change: TrackFxInvalidation::Plock { slot, param },
            },
            // One arm serves a MIDI effect's value and its locks.
            (Self::MidiFx(slot), _) => UiInvalidation::MidiFx {
                track,
                change: MidiFxInvalidation::Param { slot, param },
            },
            (Self::BusEffect(slot), _) => UiInvalidation::BusFx {
                bus: owner,
                change: BusFxInvalidation::Param { slot, param },
            },
            (Self::RackSlot(_) | Self::RackEffect { .. }, _) => return None,
        })
    }

    /// The history command that sets a param's base (stored units); `None`
    /// for a bus effect, whose edits go through the bus effect value
    /// mutation (`apply_recorded_bus_effect_value_mutation`).
    pub(crate) fn set_command(
        self,
        track: usize,
        param_idx: usize,
        value: f32,
    ) -> Option<app::AppCommand> {
        Some(match self {
            Self::Instrument => app::AppCommand::SetInstrumentParam {
                track,
                param_idx,
                value,
            },
            Self::Effect(slot_idx) => app::AppCommand::SetEffectParam {
                track,
                slot_idx,
                param_idx,
                value,
            },
            Self::MidiFx(slot_idx) => app::AppCommand::SetMidiFxParam {
                track,
                slot_idx,
                param_idx,
                value,
            },
            Self::RackSlot(slot_idx) => app::AppCommand::SetRackSlotInstrumentParam {
                track,
                slot_idx,
                param_idx,
                value,
            },
            Self::RackEffect { rack_slot, slot } => app::AppCommand::SetRackSlotEffectParam {
                track,
                rack_slot_idx: rack_slot,
                effect_slot_idx: slot,
                param_idx,
                value,
            },
            Self::BusEffect(_) => return None,
        })
    }

    /// The `clear-param-plocks` target naming this device: the target, its
    /// (effect) slot and rack slot (the host commands'
    /// `clear_plocks_command`); `None` where no clear command exists (a
    /// rack slot's instrument, a bus effect).
    pub(crate) fn plock_target(self) -> Option<(&'static str, Option<usize>, Option<usize>)> {
        match self {
            Self::Instrument => Some(("instrument", None, None)),
            Self::Effect(slot) => Some(("effect", Some(slot), None)),
            Self::MidiFx(slot) => Some(("midi-fx", Some(slot), None)),
            Self::RackEffect { rack_slot, slot } => {
                Some(("rack-effect", Some(slot), Some(rack_slot)))
            }
            Self::RackSlot(_) | Self::BusEffect(_) => None,
        }
    }

    /// The delete target naming the device (`device.delete-target`): a
    /// track chain effect or MIDI effect only on the current track (the fx
    /// panel's targets name the current track's chains), a rack slot or
    /// rack slot effect on any track, a bus effect; `None` for an
    /// instrument and for an effect of another track.
    pub(crate) fn delete_target(
        self,
        owner: usize,
        current_track: usize,
    ) -> Option<ActiveDeleteTarget> {
        let chain = |chain, slot| ActiveDeleteTarget::FxEffect {
            chain,
            bus: None,
            slot,
        };
        match self {
            Self::Instrument => None,
            Self::Effect(slot) => {
                (owner == current_track).then(|| chain(FxDeleteChain::Audio, slot))
            }
            Self::MidiFx(slot) => {
                (owner == current_track).then(|| chain(FxDeleteChain::Midi, slot))
            }
            Self::RackSlot(slot) => Some(ActiveDeleteTarget::RackSlot { track: owner, slot }),
            Self::RackEffect { rack_slot, slot } => Some(ActiveDeleteTarget::RackEffect {
                track: owner,
                rack_slot,
                effect_slot: slot,
            }),
            Self::BusEffect(slot) => Some(ActiveDeleteTarget::FxEffect {
                chain: FxDeleteChain::Bus,
                bus: Some(owner),
                slot,
            }),
        }
    }

    /// The history command that p-locks a param on `steps` (stored units);
    /// `None` for a bus effect (bus effects take no p-locks here).
    pub(crate) fn lock_command(
        self,
        track: usize,
        steps: Vec<usize>,
        param_idx: usize,
        value: f32,
    ) -> Option<app::AppCommand> {
        Some(match self {
            Self::Instrument => app::AppCommand::SetInstrumentPlockMulti {
                track,
                steps,
                param_idx,
                value,
            },
            Self::Effect(slot_idx) => app::AppCommand::SetEffectPlockMulti {
                track,
                steps,
                slot_idx,
                param_idx,
                value,
            },
            Self::MidiFx(slot_idx) => app::AppCommand::SetMidiFxPlockMulti {
                track,
                steps,
                slot_idx,
                param_idx,
                value,
            },
            Self::RackSlot(slot_idx) => app::AppCommand::SetRackSlotInstrumentPlockMulti {
                track,
                slot_idx,
                steps,
                param_idx,
                value,
            },
            Self::RackEffect { rack_slot, slot } => app::AppCommand::SetRackSlotEffectPlockMulti {
                track,
                steps,
                rack_slot_idx: rack_slot,
                effect_slot_idx: slot,
                param_idx,
                value,
            },
            Self::BusEffect(_) => return None,
        })
    }
}

/// Where one device's param values live ([`DeviceSlot::with_values`]):
/// a live slot (track chain devices, MIDI effects), a rack's snapshot (rack
/// slots and their effects, with the rack) or a bus's (bus effects).
#[derive(Clone, Copy)]
pub(crate) enum DeviceValues<'a> {
    Live(&'a sequencer::effects::EffectSlotState),
    Rack {
        rack: &'a sequencer::sequencer::RackTrackSnapshot,
        values: &'a sequencer::effects::EffectSlotSnapshot,
    },
    /// A snapshot with no rack around it (a bus effect's).
    Snapshot(&'a sequencer::effects::EffectSlotSnapshot),
}

impl<'a> DeviceValues<'a> {
    /// The values of a snapshot family (a rack's, a bus's).
    fn snapshot(self) -> Option<&'a sequencer::effects::EffectSlotSnapshot> {
        match self {
            Self::Live(_) => None,
            Self::Rack { values, .. } | Self::Snapshot(values) => Some(values),
        }
    }

    /// Param `param_idx`'s own value (stored units). A snapshot's
    /// `defaults` hold exactly its `num_params` values
    /// (`EffectSlotSnapshot::capture`), so past them is the descriptor
    /// default.
    pub(crate) fn base(self, pdesc: &sequencer::effects::ParamDescriptor, param_idx: usize) -> f32 {
        match (self, self.snapshot()) {
            (Self::Live(slot), _) => slot_param_stored_value(slot, pdesc, param_idx, None),
            (_, Some(slot)) => slot
                .defaults
                .get(param_idx)
                .copied()
                .unwrap_or(pdesc.default),
            (_, None) => pdesc.default,
        }
    }

    /// The p-lock on `step`, if any.
    pub(crate) fn lock(self, step: usize, param_idx: usize) -> Option<f32> {
        match (self, self.snapshot()) {
            (Self::Live(slot), _) => slot.plocks.get(step, param_idx),
            (_, Some(slot)) => slot.plocks.get(step)?.get(param_idx).copied().flatten(),
            (_, None) => None,
        }
    }

    /// Whether one of the first `num_steps` steps locks the param.
    pub(crate) fn has_lock(self, param_idx: usize, num_steps: usize) -> bool {
        match self {
            Self::Live(slot) => slot.plocks.param_has_any_plock(param_idx, num_steps),
            _ => (0..num_steps).any(|step| self.lock(step, param_idx).is_some()),
        }
    }
}

/// Run `read` over rack slot `slot` of the drum rack on track `track` (and
/// the rack), under the rack lock; `None` when the track holds no rack or
/// the rack no such slot. Nothing in `read` may take the rack lock again.
pub(crate) fn with_rack_slot<R>(
    state: &SequencerState,
    track: usize,
    slot: usize,
    read: impl FnOnce(
        &sequencer::sequencer::RackTrackSnapshot,
        &sequencer::sequencer::RackSlotSnapshot,
    ) -> R,
) -> Option<R> {
    let racks = state.pattern.rack_tracks.lock().unwrap();
    let rack = racks.get(track)?.as_ref()?;
    Some(read(rack, rack.slots.get(slot)?))
}

/// Whether an effect is on: its `enabled` param's own value
/// ([`DeviceValues::base`]), on when it has none or its values are gone.
pub(crate) fn effect_enabled(
    desc: &sequencer::effects::EffectDescriptor,
    values: Option<DeviceValues<'_>>,
) -> bool {
    match (enabled_param_index(desc), values) {
        (Some(idx), Some(values)) => values.base(&desc.params[idx], idx) >= 0.5,
        _ => true,
    }
}
