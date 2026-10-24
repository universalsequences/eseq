//! Links between imported things (docs/rig-spec.md §8, "Links"): every
//! source track, bus and instance id maps to the one the import created;
//! a link to anything not imported is cut and reported, never pointed at
//! whatever the target holds under the same index or id.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::effects::{EffectDescriptor, EffectSlotSnapshot, HostControl};
use crate::graph::{ProjectGraphOverrides, ProjectGraphRouteOverride as Route, ProjectGraphSeedFrom as Seed};
use crate::sequencer::{BusId, TrackOutput, TrackPatternData, MAX_STEPS};

use super::source::ImportSource;

/// Summary lines, unique, in the order first seen.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    lines: Vec<String>,
    seen: HashSet<String>,
}

impl Lines {
    pub fn push(&mut self, line: String) {
        if self.seen.insert(line.clone()) {
            self.lines.push(line);
        }
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.push(line);
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.lines.iter()
    }
}

/// Source -> new identities.
#[derive(Default)]
pub(super) struct IdMap {
    pub tracks: BTreeMap<usize, usize>,
    pub buses: HashMap<u64, u64>,
    pub instances: HashMap<u64, u64>,
    /// Source track count, and this project's after the import (sidechain
    /// selections are positional over them).
    pub source_tracks: usize,
    pub target_tracks: usize,
}

impl IdMap {
    /// The new bus for a source bus; the master mix stays itself.
    pub fn bus(&self, bus: u64) -> Option<u64> {
        if bus == BusId::MIX.0 {
            return Some(bus);
        }
        self.buses.get(&bus).copied()
    }

    /// Source track -> new track, as the positional map
    /// `remap_graph_member_routes` takes.
    pub fn track_vec(&self) -> Vec<Option<usize>> {
        let mut map = vec![None; self.source_tracks];
        for (old, new) in &self.tracks {
            if let Some(slot) = map.get_mut(*old) {
                *slot = Some(*new);
            }
        }
        map
    }
}

pub(super) fn bus_name(source: &ImportSource, bus: u64) -> String {
    source
        .file
        .buses
        .iter()
        .find(|channel| channel.id == bus)
        .map(|channel| channel.name.clone())
        .unwrap_or_else(|| format!("bus {bus}"))
}

/// The summary line for a mod cable that was cut.
pub(super) fn cut_cable_text(source: &ImportSource, from: usize, to: &str) -> String {
    format!("mod cable {} -> {to}", source.track_name(from))
}

/// A lane's bus links (sends, per-step sends, process bus sends) through
/// `ids`; with `output`, its output too. What has no new bus is cut.
pub(super) fn remap_lane_buses(
    lane: &mut TrackPatternData,
    source: &ImportSource,
    ids: &IdMap,
    who: &str,
    output: bool,
    cut: &mut Lines,
) {
    if output {
        if let TrackOutput::Bus(bus) = lane.track_params.output {
            lane.track_params.output = match ids.bus(bus.0) {
                Some(new) if new == BusId::MIX.0 => TrackOutput::Mix,
                Some(new) => TrackOutput::Bus(BusId(new)),
                None => {
                    cut.push(format!(
                        "{who}: output to '{}' (now the master mix)",
                        bus_name(source, bus.0)
                    ));
                    TrackOutput::Mix
                }
            };
        }
    }
    let keep = |destination: &mut BusId, cut: &mut Lines| match ids.bus(destination.0) {
        Some(new) => {
            *destination = BusId(new);
            true
        }
        None => {
            cut.push(format!("{who}: send to '{}'", bus_name(source, destination.0)));
            false
        }
    };
    lane.track_params.sends.retain_mut(|send| keep(&mut send.destination, cut));
    for step in &mut lane.track_send_plock_snapshot {
        step.retain_mut(|send| keep(&mut send.destination, cut));
    }
    for slot in &mut lane.process_chain.slots {
        let label = slot.instance_name.clone().unwrap_or_else(|| slot.class_name.clone());
        let cut_bus = |bus: u64, cut: &mut Lines| {
            cut.push(format!("{who}: process '{label}' bus send to '{}'", bus_name(source, bus)));
        };
        for binding in slot.bindings.values_mut() {
            if let Some(crate::process::ParamTarget::BusSend { bus }) = binding {
                match ids.bus(*bus) {
                    Some(new) => *bus = new,
                    None => {
                        cut_bus(*bus, cut);
                        *binding = None;
                    }
                }
            }
        }
        for fanout in slot.fanout.values_mut() {
            fanout.retain_mut(|entry| match &mut entry.target {
                crate::process::ParamTarget::BusSend { bus } => match ids.bus(*bus) {
                    Some(new) => {
                        *bus = new;
                        true
                    }
                    None => {
                        cut_bus(*bus, cut);
                        false
                    }
                },
                _ => true,
            });
        }
    }
}

/// The track a positional sidechain selection names (`owner` excluded,
/// 0 = none), as `track_delete_remap::sidechain_source_track` decodes it.
fn sidechain_source(owner: usize, selection: usize, total: usize) -> Option<usize> {
    if selection == 0 {
        return None;
    }
    (0..total).filter(|track| *track != owner).nth(selection - 1)
}

/// The positional selection of `source` for an effect on `owner`.
fn sidechain_selection(owner: usize, source: usize, total: usize) -> usize {
    if source >= total || source == owner {
        return 0;
    }
    (0..total)
        .filter(|track| *track != owner)
        .position(|track| track == source)
        .map_or(0, |index| index + 1)
}

/// Re-point every sidechain selector of `slots` (a track's or a rack
/// slot's chain, `descriptors` alongside) from the source's track space to
/// this project's: decoded with the source owner and track count, mapped
/// through `ids`, encoded for the new owner. A source track that did not
/// come along selects nothing and is reported.
pub(super) fn remap_slot_sidechains(
    slots: &mut [EffectSlotSnapshot],
    descriptors: &[EffectDescriptor],
    owner: (usize, usize),
    ids: &IdMap,
    source: &ImportSource,
    who: &str,
    cut: &mut Lines,
) {
    let (old_owner, new_owner) = owner;
    let remap = |value: f32, cut: &mut Lines| -> f32 {
        let selection = value.round().max(0.0) as usize;
        let Some(old) = sidechain_source(old_owner, selection, ids.source_tracks) else {
            return 0.0;
        };
        match ids.tracks.get(&old) {
            Some(new) => sidechain_selection(new_owner, *new, ids.target_tracks) as f32,
            None => {
                cut.push(format!("{who}: sidechain from '{}'", source.track_name(old)));
                0.0
            }
        }
    };
    for (slot, descriptor) in slots.iter_mut().zip(descriptors) {
        let count = (slot.num_params as usize).min(descriptor.params.len());
        for param in 0..count {
            if !matches!(descriptor.params[param].host_control, Some(HostControl::FxSidechain { .. })) {
                continue;
            }
            if let Some(value) = slot.defaults.get(param).copied() {
                slot.defaults[param] = remap(value, cut);
            }
            for step in 0..MAX_STEPS {
                let locked = slot.plocks.get(step).and_then(|row| row.get(param)).and_then(|value| *value);
                if let Some(value) = locked {
                    slot.plocks.set(step, param, Some(remap(value, cut)));
                }
            }
        }
    }
}

/// Every sidechain selector of a lane: its track effects (`descriptors`,
/// the new track's chain) and each rack slot's own chain.
pub(super) fn remap_lane_sidechains(
    lane: &mut TrackPatternData,
    descriptors: &[EffectDescriptor],
    owner: (usize, usize),
    ids: &IdMap,
    source: &ImportSource,
    who: &str,
    cut: &mut Lines,
) {
    remap_slot_sidechains(&mut lane.effect_slots, descriptors, owner, ids, source, who, cut);
    if let Some(rack) = lane.rack_track.as_mut() {
        for slot in &mut rack.slots {
            let descriptors = slot.effect_descriptors.clone();
            remap_slot_sidechains(&mut slot.effect_slots, &descriptors, owner, ids, source, who, cut);
        }
    }
}

/// A bus effect's sidechain selection (bus selectors list every track,
/// `track + 1`), mapped from the source's tracks to the imported ones.
pub(super) fn remap_bus_sidechain(
    selection: usize,
    ids: &IdMap,
    source: &ImportSource,
    who: &str,
    cut: &mut Lines,
) -> usize {
    let Some(old) = selection.checked_sub(1) else {
        return 0;
    };
    match ids.tracks.get(&old) {
        Some(new) => new + 1,
        None => {
            cut.push(format!("{who}: sidechain from '{}'", source.track_name(old)));
            0
        }
    }
}

/// A project-owned sequencer's override on the imported tracks: track
/// routes and seed tracks follow `ids` (one not imported goes off), and
/// generator routes follow the imported instances. Every drop is reported.
pub(super) fn remap_sequencer_routes(
    graph: &mut ProjectGraphOverrides,
    source: &ImportSource,
    ids: &IdMap,
    label: &str,
    cut: &mut Lines,
) {
    for node in &graph.node_intrinsics {
        match node.route {
            Some(Route::Track(track)) if !ids.tracks.contains_key(&track) => {
                cut.push(format!("sequencer '{label}': route to '{}'", source.track_name(track)));
            }
            Some(Route::Generator(id) | Route::GeneratorRestart(id)) if !ids.instances.contains_key(&id) => {
                cut.push(format!("sequencer '{label}': gate route to sequencer {id}"));
            }
            _ => {}
        }
        if let Some(Seed::Tracks(seed)) = &node.seed_from {
            for track in seed.iter().filter(|track| !ids.tracks.contains_key(track)) {
                cut.push(format!("sequencer '{label}': seed from '{}'", source.track_name(*track)));
            }
        }
    }
    super::super::rack_sequencers::remap_graph_member_routes(graph, &ids.track_vec());
    super::super::rack_sequencers::remap_graph_generator_routes(graph, &ids.instances);
}
