//! Native neural networks (spec §14.2q, stage 7g-3): the current scene's
//! `ProjectNeuralNetwork`s (the neural engine `neural-create` and the other
//! `neural-*` natives author) as `network`s, their neurons as `neuron`s;
//! `project.networks`.
//!
//! Identity: networks are positional (`(index)`, in the scene's order), the
//! instance kept by network id across reorders (`registry::reconcile`; a
//! repeated id keeps its first network) and replaced on a project load. A
//! scene switch shows the new scene's networks: a network id both scenes
//! hold keeps its instance, any other goes stale. Neurons are keyed (network
//! instance id, neuron index), as the network numbers them.
//!
//! Feeds: the model fields behind one key ([`NeuralKey`]: the scenes
//! revision, which every network edit moves, the current scene and the track
//! instances, which a route names), compared without allocating; when it
//! moves the sync reads the current scene's networks once and pushes only a
//! network that differs from its last push (a step edit pushes none). Live
//! (observed only, compared in place with the last push, so an idle tick
//! allocates nothing): `network.active` and a neuron's `energy`, `trigger`
//! and `dampening`, read from the engine's visualization snapshot (one read
//! per tick) and shaped for display (energy clamped to 0-4 and dampening to
//! 0-1, both rounded to 0.01, dampening by target neuron, triggers clamped to
//! 0-1; zeros unless the network is the one the engine runs), and a neuron's
//! `selected` (the step-editing selection, `SharedSelectedNeuralNeurons`, of
//! the current pattern).

use super::*;
use sequencer::lisp_host::SelectedNeuralNeuron;
use sequencer::neural::{
    NeuralVisualizationSnapshot, ProjectNeuralNetwork, ProjectNeuron, NUM_NEURONS,
};
use std::collections::BTreeSet;

/// The inputs of the network model sync, compared without allocating.
struct NeuralKey {
    scenes: u64,
    scene: usize,
    tracks: Vec<Option<InstanceId>>,
}

/// A neuron's live fields as last pushed (`dampening` holds `width` cells).
#[derive(Clone, Copy, PartialEq)]
struct NeuronLive {
    selected: bool,
    energy: f64,
    trigger: f64,
    width: usize,
    dampening: [f64; NUM_NEURONS],
}

/// What the network half of the sync keeps across ticks.
#[derive(Default)]
pub(crate) struct NeuralState {
    key: Option<NeuralKey>,
    /// Network id → instance (dropped on a project load).
    instances: HashMap<u64, InstanceId>,
    ids: Vec<Option<InstanceId>>,
    /// Each network instance's network as last pushed.
    pushed: HashMap<InstanceId, ProjectNeuralNetwork>,
    /// Every neuron instance, and the observed ones.
    neurons: Vec<InstanceId>,
    neuron_observed: ObservedList,
    /// Live values as last pushed, per observing network and neuron (a
    /// neuron's with the observed mask they were pushed under).
    active: HashMap<InstanceId, bool>,
    live: HashMap<InstanceId, (u32, NeuronLive)>,
    /// Network model syncs, and networks pushed (tests: a step edit pushes
    /// none).
    pub(crate) syncs: u64,
    pub(crate) pushes: u64,
}

impl NeuralState {
    /// One network instance (the stale check's representative).
    pub(super) fn representative(&self) -> Option<&InstanceId> {
        self.ids.iter().flatten().next()
    }

    /// Push every network at the next tick (a schema change or a hot
    /// reload dropped instances); the registered instances are kept.
    pub(super) fn invalidate(&mut self) {
        self.key = None;
        self.pushed.clear();
        self.active.clear();
        self.live.clear();
        self.neuron_observed.reset();
    }

    /// The network instances, for a project load (ids restart with it).
    pub(super) fn drain(&mut self) -> impl Iterator<Item = (u64, InstanceId)> + '_ {
        self.key = None;
        self.instances.drain()
    }
}

/// The neurons a network publishes (its count, at most the engine's).
fn neuron_count(network: &ProjectNeuralNetwork) -> usize {
    network.num_neurons.min(NUM_NEURONS)
}

/// Whether the engine runs network `nid` (its snapshot is that network's).
fn runs(snapshot: &NeuralVisualizationSnapshot, nid: u64) -> bool {
    snapshot.active && snapshot.network_id == nid && neural_snapshot_size(snapshot) > 0
}

/// Neuron `index` of network `nid` (of `count` neurons): its playback from
/// `snapshot` while the engine runs the network (shaped for display;
/// zeros otherwise) and whether `selection` holds it.
fn neuron_live(
    snapshot: &NeuralVisualizationSnapshot,
    selection: Option<&BTreeSet<SelectedNeuralNeuron>>,
    neuron: SelectedNeuralNeuron,
    count: usize,
) -> NeuronLive {
    let mut live = NeuronLive {
        selected: selection.is_some_and(|selection| selection.contains(&neuron)),
        energy: 0.0,
        trigger: 0.0,
        width: count.min(NUM_NEURONS),
        dampening: [0.0; NUM_NEURONS],
    };
    if !runs(snapshot, neuron.network_id) {
        return live;
    }
    let (size, index) = (neural_snapshot_size(snapshot), neuron.neuron_idx);
    live.width = size;
    if index < size {
        live.energy = neural_energy_display_value(snapshot.energy[index]);
        live.trigger = neural_trigger_display_value(snapshot.trigger_activity[index]);
        for (cell, value) in live
            .dampening
            .iter_mut()
            .zip(&snapshot.dampening[index][..size])
        {
            *cell = neural_dampening_display_value(*value);
        }
    }
    live
}

/// One live field of `live`. `key` is one of `NEURON_LIVE.keys` (the tick
/// visits only those; the reader hook dispatches only a neuron's live
/// fields here).
fn neuron_live_field(live: &NeuronLive, key: FieldKey) -> Value {
    match key {
        f::NEURON_SELECTED => Value::Bool(live.selected),
        f::NEURON_ENERGY => number(live.energy),
        f::NEURON_TRIGGER => number(live.trigger),
        f::NEURON_DAMPENING => numbers(&live.dampening[..live.width]),
        key => unreachable!("{key:?} is not a neuron live field"),
    }
}

/// Whether `key` (one of `NEURON_LIVE.keys`) differs between `a` and `b`.
fn neuron_live_moved(a: &NeuronLive, b: &NeuronLive, key: FieldKey) -> bool {
    match key {
        f::NEURON_SELECTED => a.selected != b.selected,
        f::NEURON_ENERGY => a.energy != b.energy,
        f::NEURON_TRIGGER => a.trigger != b.trigger,
        f::NEURON_DAMPENING => a.dampening[..a.width] != b.dampening[..b.width],
        key => unreachable!("{key:?} is not a neuron live field"),
    }
}

/// A network's or a neuron's live field (the reader hook's cold read; the
/// tick compares in place instead). The network's id and neuron count come
/// from its cells, which the model sync pushed when it registered it.
pub(super) fn neural_live_value<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    let int = |id: InstanceId, key: FieldKey| match store.field(id, key.1)? {
        Value::Number(n) if n >= 0.0 => Some(n as u64),
        _ => None,
    };
    let snapshot = sources.state.neural_visualization();
    if key == f::NETWORK_ACTIVE {
        return Some(Value::Bool(runs(&snapshot, int(id, f::NETWORK_NID)?)));
    }
    let &[network, index] = store.key_of(id)? else {
        return None;
    };
    let neuron = SelectedNeuralNeuron {
        pattern_idx: sources.state.current_scene_index(),
        network_id: int(network, f::NETWORK_NID)?,
        neuron_idx: index as usize,
    };
    let count = int(network, f::NETWORK_NEURON_COUNT)? as usize;
    let selection = sources.selected_neural_neurons.lock().unwrap();
    let live = neuron_live(&snapshot, Some(&selection), neuron, count);
    Some(neuron_live_field(&live, key))
}

/// The weight matrix as rows of numbers (`num_neurons` square).
fn weight_rows(network: &ProjectNeuralNetwork) -> Value {
    let rows = network.shaped_weights();
    list_value(
        rows.iter()
            .take(NUM_NEURONS)
            .map(|row| numbers(&row[..row.len().min(NUM_NEURONS)])),
    )
}

/// One network's fields, its neurons' (registered or dropped with its
/// neuron count) and `neurons`.
fn push_network(
    pusher: &mut Pusher<'_>,
    tracks: &[Option<InstanceId>],
    id: InstanceId,
    network: &ProjectNeuralNetwork,
) {
    pusher.push(id, f::NETWORK_NID, number(network.id as f64));
    pusher.push(id, f::NETWORK_NAME, text(&network.name));
    pusher.push(id, f::NETWORK_ENABLED, Value::Bool(network.enabled));
    let count = neuron_count(network);
    pusher.push(id, f::NETWORK_NEURON_COUNT, number(count as f64));
    pusher.push(
        id,
        f::NETWORK_RESET_BARS,
        number(network.reset_interval_bars),
    );
    pusher.push(id, f::NETWORK_ENERGY_DECAY, number(network.energy_decay));
    pusher.push(id, f::NETWORK_MAX_POLY, number(network.max_poly));
    let selection = network.max_poly_selection.as_str();
    pusher.push(id, f::NETWORK_MAX_POLY_SELECTION, text(selection));
    pusher.push(id, f::NETWORK_WEIGHTS, weight_rows(network));
    let dropped = drop_children_past(&mut *pusher.rt, id, NEURON, count);
    let mut added = false;
    let neurons = indexed_children(
        &mut *pusher.rt,
        id,
        NEURON,
        count,
        |store, neuron, index| {
            store.push(neuron, f::NEURON_NETWORK, Value::Instance(id));
            store.push(neuron, f::NEURON_INDEX, number(index as f64));
            added = true;
        },
    );
    pusher.changed |= dropped || added;
    let default = ProjectNeuron::default();
    for (index, neuron_id) in neurons.iter().enumerate() {
        let neuron = network.neurons.get(index).unwrap_or(&default);
        push_neuron(pusher, tracks, *neuron_id, neuron);
    }
    pusher.push(id, f::NETWORK_NEURONS, instance_list(neurons));
}

/// One neuron's model fields (the route a track instance, the clocks
/// labels).
fn push_neuron(
    pusher: &mut Pusher<'_>,
    tracks: &[Option<InstanceId>],
    id: InstanceId,
    neuron: &ProjectNeuron,
) {
    let route = neuron
        .route
        .and_then(|track| tracks.get(track).copied().flatten());
    pusher.push(id, f::NEURON_ROUTE, instance_or_nil(route));
    let resolution = neuron.resolution_timebase().label();
    pusher.push(id, f::NEURON_RESOLUTION, text(resolution));
    pusher.push(id, f::NEURON_DELAY, number(neuron.delay_steps));
    pusher.push(id, f::NEURON_THRESHOLD, number(neuron.threshold));
    pusher.push(id, f::NEURON_TRANSPOSE, number(neuron.transpose));
    let quantize = quantize_label(neuron.quantize_timebase());
    pusher.push(id, f::NEURON_QUANTIZE, text(quantize));
    let amount = neuron.dampening_amount;
    pusher.push(id, f::NEURON_DAMPENING_AMOUNT, number(amount));
    let recovery = neuron.dampening_recovery;
    pusher.push(id, f::NEURON_DAMPENING_RECOVERY, number(recovery));
}

impl HostKinds {
    /// The networks, their neurons and `project.networks`, when
    /// [`NeuralKey`] moved. Runs after the model sync (tracks).
    pub(super) fn sync_network_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let read = (
            app.state.project_scenes_revision(),
            app.state.current_scene_index(),
        );
        let (tracks, state) = (&self.track_ids, &mut self.networks);
        let key = state.key.as_ref();
        if key.is_some_and(|key| (key.scenes, key.scene) == read && key.tracks == *tracks) {
            return;
        }
        let tracks_moved = !key.is_some_and(|key| key.tracks == *tracks);
        let mut networks = app.state.current_neural_networks();
        let mut seen = HashSet::with_capacity(networks.len());
        networks.retain(|network| seen.insert(network.id));
        let model: Vec<u64> = networks.iter().map(|network| network.id).collect();
        let ids = reconcile(pusher, NETWORK, &mut state.instances, &model);
        state.pushed.retain(|id, _| ids.contains(&Some(*id)));
        state.active.retain(|id, _| ids.contains(&Some(*id)));
        let ids_moved = ids != state.ids;
        let mut pushed = false;
        for (index, (network, id)) in networks.into_iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::NETWORK_INDEX, number(index as f64));
            if !tracks_moved && state.pushed.get(&id) == Some(&network) {
                continue;
            }
            push_network(pusher, tracks, id, &network);
            state.pushed.insert(id, network);
            state.pushes += 1;
            pushed = true;
        }
        if ids_moved || pushed {
            let counts = ids.iter().flatten().map(|id| {
                let count = state.pushed.get(id).map_or(0, neuron_count);
                (*id, count)
            });
            let neurons: Vec<InstanceId> = counts
                .flat_map(|(id, count)| (0..count).map(move |index| (id, index)))
                .filter_map(|(id, index)| pusher.rt.keyed_instance(NEURON, &[id, index as u64]))
                .collect();
            if neurons != state.neurons {
                state.live.retain(|id, _| neurons.contains(id));
                state.neurons = neurons;
                state.neuron_observed.reset();
            }
        }
        if ids_moved {
            if let Some(project) = pusher.singleton(PROJECT) {
                pusher.push(project, f::PROJECT_NETWORKS, listed_instances(&ids));
            }
        }
        state.ids = ids;
        state.syncs += 1;
        state.key = Some(NeuralKey {
            scenes: read.0,
            scene: read.1,
            tracks: tracks.clone(),
        });
    }

    /// The observed live fields of every network (`active`) and neuron
    /// (playback, `selected`), compared in place with the last push; the
    /// engine's snapshot is read once per tick, the selection locked once,
    /// and only while something observes them.
    pub(super) fn sync_network_live(&mut self, pusher: &mut Pusher<'_>) {
        let state = &mut self.networks;
        let sources = pusher.sources;
        let mut snapshot = None;
        for id in state.ids.iter().flatten().copied() {
            if pusher.rt.host_fields_observed(id, &NETWORK_LIVE.names) == 0 {
                state.active.remove(&id);
                continue;
            }
            let Some(nid) = state.pushed.get(&id).map(|network| network.id) else {
                continue;
            };
            let snapshot = snapshot.get_or_insert_with(|| sources.state.neural_visualization());
            let active = runs(snapshot, nid);
            let changed = state.active.insert(id, active) != Some(active);
            pusher.push_computed_if(id, f::NETWORK_ACTIVE, changed, || Value::Bool(active));
        }
        let neurons = &state.neurons;
        (state.neuron_observed).refresh(pusher.rt, &NEURON_LIVE.names, || neurons.clone());
        let entries = &state.neuron_observed.entries;
        state
            .live
            .retain(|id, _| entries.iter().any(|(observed, _, _)| observed == id));
        if entries.is_empty() {
            return;
        }
        let snapshot = snapshot.get_or_insert_with(|| sources.state.neural_visualization());
        let selected_bit = NEURON_LIVE.bit(f::NEURON_SELECTED);
        let wants_selection = entries.iter().any(|(_, mask, _)| mask & selected_bit != 0);
        let selection = wants_selection.then(|| sources.selected_neural_neurons.lock().unwrap());
        let pattern_idx = sources.state.current_scene_index();
        for &(id, mask, _) in entries {
            let Some(&[network, index]) = pusher.rt.instance_key(id) else {
                continue;
            };
            let Some(owner) = state.pushed.get(&network) else {
                continue;
            };
            let neuron = SelectedNeuralNeuron {
                pattern_idx,
                network_id: owner.id,
                neuron_idx: index as usize,
            };
            let live = neuron_live(snapshot, selection.as_deref(), neuron, neuron_count(owner));
            let last = state.live.insert(id, (mask, live));
            for (bit, key) in NEURON_LIVE.keys.iter().enumerate() {
                let bit = 1 << bit;
                if mask & bit == 0 {
                    continue;
                }
                let changed = last.is_none_or(|(pushed, last)| {
                    pushed & bit == 0 || neuron_live_moved(&last, &live, *key)
                });
                pusher.push_computed_if(id, *key, changed, || neuron_live_field(&live, *key));
            }
        }
    }
}
