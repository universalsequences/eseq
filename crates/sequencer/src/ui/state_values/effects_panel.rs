use super::*;

/// A Filter Table's or a Convolution Reverb's table fields, from its effect
/// node's registries; each `None` where the effect has none (`mode` and
/// `data_key` also until set or prepared). Shared by the legacy effect panel
/// dicts and the host kinds' `device.table-*` / `ir-name` (which cache
/// `table-options` themselves).
#[derive(Default)]
pub(crate) struct EffectTableFields {
    /// A Filter Table's `table-name`, `table-mode`, `table-engine` and
    /// `table-data-key`.
    pub(crate) name: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) engine: Option<String>,
    pub(crate) data_key: Option<String>,
    /// A Convolution Reverb's `ir-name`.
    pub(crate) ir_name: Option<String>,
}

impl EffectTableFields {
    /// Whether an effect named `desc_name` has table fields.
    pub(crate) fn applies(desc_name: &str) -> bool {
        use sequencer::effects::{conv_reverb, filter_table};
        desc_name == filter_table::NAME || desc_name == conv_reverb::NAME
    }

    /// The table fields of effect `desc_name` at graph node `node_id`.
    pub(crate) fn of(desc_name: &str, node_id: i32) -> Self {
        use sequencer::effects::{conv_reverb, filter_table};
        if desc_name == conv_reverb::NAME {
            let ir_name = conv_reverb::ir_name_for(node_id).unwrap_or_else(|| "No IR".to_string());
            return Self {
                ir_name: Some(ir_name),
                ..Self::default()
            };
        }
        if desc_name != filter_table::NAME {
            return Self::default();
        }
        let reference = filter_table::table_ref_for(node_id);
        Self {
            name: Some(
                filter_table::table_name_for(node_id).unwrap_or_else(|| "No table".to_string()),
            ),
            mode: (reference.as_deref())
                .and_then(|reference| filter_table::decode_table_ref(reference).1)
                .map(|mode| mode.label().to_string()),
            engine: Some(filter_table::engine_for(node_id).display_name().to_string()),
            data_key: (filter_table::prepared_table_for(node_id).is_some())
                .then(|| filter_table::visualization_key(node_id)),
            ir_name: None,
        }
    }

    /// Whether these are a Filter Table's (it lists `table-options`).
    pub(crate) fn is_table(&self) -> bool {
        self.name.is_some()
    }

}

pub(super) fn enabled_param_index(desc: &sequencer::effects::EffectDescriptor) -> Option<usize> {
    desc.params.iter().position(|param| param.name == "enabled")
}

/// The channel-strip label for an engine name. Engine names are library ids
/// ("factory:Drums/808 Clap"); sidecar instruments (a folder with dsp.lisp +
/// ui.lisp) carry a trailing slash ("factory:Drums/808 Kick/"), and a bare
/// file id may end in ".lisp". The strip wants just the leaf.
pub(crate) fn device_leaf_name(name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    let leaf = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let leaf = leaf.rsplit(':').next().unwrap_or(leaf);
    let leaf = leaf.strip_suffix(".lisp").unwrap_or(leaf);
    if leaf.is_empty() {
        trimmed.to_string()
    } else {
        leaf.to_string()
    }
}

/// One device of a track's chain ([`track_device_chain`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DeviceChainEntry {
    pub(crate) name: String,
    /// "instrument" or "effect".
    pub(crate) kind: &'static str,
    pub(crate) enabled: bool,
    /// Effect slot index; -1 for the instrument.
    pub(crate) slot: i64,
}

/// A track's devices in signal order: its instrument (when it has one),
/// then each occupied effect slot: the `device` host kind's (kind-bindings
/// spec §13 stage 4).
pub(crate) fn track_device_chain(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
) -> Vec<DeviceChainEntry> {
    let mut entries = Vec::new();
    let instrument_type = app.graph.track_instrument_types.get(track).copied();
    let instrument_name = match instrument_type {
        Some(sequencer::sequencer::InstrumentType::Custom) => app
            .graph
            .track_engine_ids
            .get(track)
            .and_then(|engine_id| *engine_id)
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| engine.name.clone())
            .or_else(|| app.tracks.get(track).cloned()),
        Some(sequencer::sequencer::InstrumentType::Sampler) => Some("sampler".to_string()),
        // A rack has no engine name of its own; the track's name is the
        // only label that says which rack this is.
        Some(sequencer::sequencer::InstrumentType::Rack) => app
            .tracks
            .get(track)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .or_else(|| Some("rack".to_string())),
        Some(sequencer::sequencer::InstrumentType::Modulator) => Some("modulator".to_string()),
        Some(sequencer::sequencer::InstrumentType::Empty) | None => None,
    };
    if let Some(name) = instrument_name {
        entries.push(DeviceChainEntry {
            name: device_leaf_name(&name),
            kind: "instrument",
            enabled: true,
            slot: -1,
        });
    }
    let descs = app.graph.effect_descriptors.get(track);
    let chain = state.pattern.effect_chains.get(track);
    if let Some(descs) = descs {
        // Chains are fixed-size slot arrays; an unused slot has an
        // empty descriptor. Only occupied slots are devices.
        for (slot_idx, desc) in descs.iter().enumerate().filter(|(_, d)| !d.name.is_empty()) {
            let enabled = effect_enabled(
                desc,
                chain.and_then(|c| c.get(slot_idx)).map(DeviceValues::Live),
            );
            entries.push(DeviceChainEntry {
                name: desc.name.clone(),
                kind: "effect",
                enabled,
                slot: slot_idx as i64,
            });
        }
    }
    entries
}

/// The occupied effect slots of a snapshot chain (a bus's, a rack slot's),
/// in order.
fn snapshot_effect_chain(
    descriptors: &[sequencer::effects::EffectDescriptor],
    slots: &[sequencer::effects::EffectSlotSnapshot],
) -> Vec<DeviceChainEntry> {
    descriptors
        .iter()
        .enumerate()
        .filter(|(_, desc)| !desc.name.is_empty())
        .map(|(slot_idx, desc)| DeviceChainEntry {
            name: desc.name.clone(),
            kind: "effect",
            enabled: effect_enabled(desc, slots.get(slot_idx).map(DeviceValues::Snapshot)),
            slot: slot_idx as i64,
        })
        .collect()
}

/// A bus's effects in chain order (its occupied slots): the host kinds'
/// `bus.devices`.
pub(crate) fn bus_device_chain(bus: &app::BusChannelState) -> Vec<DeviceChainEntry> {
    snapshot_effect_chain(&bus.effect_descriptors, &bus.effect_slots)
}

/// A drum rack slot's effects in chain order (its occupied slots), for the
/// host kinds' rack slot devices.
pub(crate) fn rack_slot_effect_chain(
    slot: &sequencer::sequencer::RackSlotSnapshot,
) -> Vec<DeviceChainEntry> {
    snapshot_effect_chain(&slot.effect_descriptors, &slot.effect_slots)
}

/// A track's MIDI effects in chain order, each with its descriptor
/// (`descriptors`: `load_midi_fx_descriptors`); a slot whose effect has no
/// descriptor is left out: the host kinds' `track.midi-devices`.
pub(crate) fn midi_fx_device_chain<'a>(
    chain: &[String],
    descriptors: &'a [sequencer::effects::EffectDescriptor],
) -> Vec<(usize, &'a sequencer::effects::EffectDescriptor)> {
    chain
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, name)| {
            let desc = descriptors
                .iter()
                .find(|desc| desc.name.eq_ignore_ascii_case(name))?;
            Some((slot_idx, desc))
        })
        .collect()
}

