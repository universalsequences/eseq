use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::scheduled_event::{EventSource, StepEvent};
use crate::sequencer::Timebase;

pub const NUM_NEURONS: usize = 16;
pub const NEURAL_DELAY_QUEUE_CAPACITY: usize = 8;
const TRIGGER_VISUAL_HOLD_BEATS: f64 = 0.25;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NeuralMaxPolySelection {
    #[default]
    Deterministic,
    Propagation,
    Random,
    /// Keep the highest-velocity firings.
    Loudest,
    /// Keep the firings with the lowest emitted transpose.
    LowestTranspose,
    /// Keep the firings with the highest emitted transpose.
    HighestTranspose,
    /// Keep seed-originated firings before neural-only ones (graph engine; the native
    /// neural engine has no per-candidate seed tag and falls back to deterministic).
    SeedFirst,
}

impl NeuralMaxPolySelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Propagation => "propagation",
            Self::Random => "random",
            Self::Loudest => "loudest",
            Self::LowestTranspose => "lowest-transpose",
            Self::HighestTranspose => "highest-transpose",
            Self::SeedFirst => "seed-first",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParamNodeId {
    pub logical_id: u64,
    pub node_param_idx: u32,
}

impl ParamNodeId {
    pub fn from_slot_param(
        node_id: u32,
        modulator_node_id: u32,
        raw_param_idx: u32,
    ) -> Option<Self> {
        if raw_param_idx == u32::MAX {
            return None;
        }
        if raw_param_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            let logical_id = modulator_node_id as u64;
            if logical_id == 0 {
                return None;
            }
            Some(Self {
                logical_id,
                node_param_idx: raw_param_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE,
            })
        } else {
            let logical_id = node_id as u64;
            if logical_id == 0 {
                return None;
            }
            Some(Self {
                logical_id,
                node_param_idx: raw_param_idx,
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectParamOverride {
    pub target_track: usize,
    pub param_id: ParamNodeId,
    pub param_index: usize,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectEffectParamOverride {
    pub target_track: usize,
    pub slot_index: usize,
    pub param_id: ParamNodeId,
    pub param_index: usize,
    pub value: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectNeuronOutputOverrides {
    #[serde(default)]
    pub instrument: Vec<ProjectParamOverride>,
    #[serde(default)]
    pub effects: Vec<ProjectEffectParamOverride>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectNeuron {
    #[serde(default = "default_neuron_resolution")]
    pub resolution: u8,
    #[serde(default)]
    pub delay_steps: u32,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    #[serde(default)]
    pub transpose: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantize: Option<u8>,
    #[serde(default)]
    pub dampening_amount: f32,
    #[serde(default = "default_dampening_recovery")]
    pub dampening_recovery: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<usize>,
    #[serde(default)]
    pub output_overrides: ProjectNeuronOutputOverrides,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectNeuralNetwork {
    pub id: u64,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_num_neurons")]
    pub num_neurons: usize,
    #[serde(default)]
    pub weights: Vec<Vec<f32>>,
    #[serde(default)]
    pub neurons: Vec<ProjectNeuron>,
    #[serde(default = "default_reset_interval_bars")]
    pub reset_interval_bars: f32,
    #[serde(default = "default_energy_decay")]
    pub energy_decay: f32,
    #[serde(default = "default_max_poly")]
    pub max_poly: u32,
    #[serde(default = "default_max_poly_selection")]
    pub max_poly_selection: NeuralMaxPolySelection,
    #[serde(default)]
    pub seed_on_reset: Vec<f32>,
}

impl Default for ProjectNeuron {
    fn default() -> Self {
        Self {
            resolution: default_neuron_resolution(),
            delay_steps: 0,
            threshold: default_threshold(),
            transpose: 0.0,
            quantize: None,
            dampening_amount: 0.0,
            dampening_recovery: default_dampening_recovery(),
            route: None,
            output_overrides: ProjectNeuronOutputOverrides::default(),
        }
    }
}

impl Default for ProjectNeuralNetwork {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Neural".to_string(),
            enabled: true,
            num_neurons: NUM_NEURONS,
            weights: vec![vec![0.0; NUM_NEURONS]; NUM_NEURONS],
            neurons: vec![ProjectNeuron::default(); NUM_NEURONS],
            reset_interval_bars: default_reset_interval_bars(),
            energy_decay: default_energy_decay(),
            max_poly: default_max_poly(),
            max_poly_selection: default_max_poly_selection(),
            seed_on_reset: vec![0.0; NUM_NEURONS],
        }
    }
}

impl ProjectNeuron {
    pub fn resolution_timebase(&self) -> Timebase {
        Timebase::from_index(self.resolution as u32)
    }

    pub fn quantize_timebase(&self) -> Option<Timebase> {
        self.quantize.map(|idx| Timebase::from_index(idx as u32))
    }
}

pub fn remap_neural_route_after_track_delete(
    route: Option<usize>,
    deleted_track: usize,
) -> Option<usize> {
    match route {
        Some(track) if track == deleted_track => None,
        Some(track) if track > deleted_track => Some(track - 1),
        other => other,
    }
}

pub fn remap_neural_network_routes_after_track_delete(
    networks: &mut [ProjectNeuralNetwork],
    deleted_track: usize,
) {
    for network in networks {
        for neuron in &mut network.neurons {
            neuron.route = remap_neural_route_after_track_delete(neuron.route, deleted_track);
        }
    }
}

/// Removes instrument-parameter writes owned by a replaced track while
/// preserving effect writes and writes targeting every other track.
pub fn remove_instrument_overrides_for_track(
    networks: &mut [ProjectNeuralNetwork],
    target_track: usize,
) -> usize {
    let mut removed = 0;
    for network in networks {
        for neuron in &mut network.neurons {
            let before = neuron.output_overrides.instrument.len();
            neuron
                .output_overrides
                .instrument
                .retain(|override_param| override_param.target_track != target_track);
            removed += before - neuron.output_overrides.instrument.len();
        }
    }
    removed
}

// The grid-boundary clock machinery lives in `crate::runtime::grid_clock` now;
// re-exported here so existing `crate::neural::` paths keep working.
pub use crate::runtime::grid_clock::{next_grid_boundary, process_grid_boundaries, GridBoundaryClock};

#[derive(Clone, Copy, Debug)]
struct NeuronConfig {
    resolution: Timebase,
    delay_steps: u32,
    threshold: f32,
    transpose: f32,
    quantize: Option<Timebase>,
    dampening_amount: f32,
    dampening_recovery: f32,
    route: Option<usize>,
    seed_track_mask: u128,
}

impl Default for NeuronConfig {
    fn default() -> Self {
        Self {
            resolution: Timebase::Sixteenth,
            delay_steps: 0,
            threshold: 1.0,
            transpose: 0.0,
            quantize: None,
            dampening_amount: 0.0,
            dampening_recovery: 0.98,
            route: None,
            seed_track_mask: 0,
        }
    }
}

impl NeuronConfig {
    fn accepts_seed_track(self, track: usize) -> bool {
        seed_track_bit(track).is_some_and(|bit| self.seed_track_mask & bit != 0)
    }
}

fn seed_track_bit(track: usize) -> Option<u128> {
    (track < u128::BITS as usize).then_some(1_u128 << track)
}

fn neuron_seed_track_mask(neuron: &ProjectNeuron) -> u128 {
    let mut mask = 0_u128;
    if let Some(route) = neuron.route.and_then(seed_track_bit) {
        mask |= route;
    }
    for override_param in &neuron.output_overrides.instrument {
        if let Some(bit) = seed_track_bit(override_param.target_track) {
            mask |= bit;
        }
    }
    for override_param in &neuron.output_overrides.effects {
        if let Some(bit) = seed_track_bit(override_param.target_track) {
            mask |= bit;
        }
    }
    mask
}

#[derive(Clone, Debug)]
struct DelayedPropagation {
    remaining_steps: u32,
    ready_after_beats: f64,
    event: StepEvent,
    external_seed: bool,
}

#[derive(Clone, Debug)]
struct DelayQueue {
    entries: Vec<DelayedPropagation>,
}

impl Default for DelayQueue {
    fn default() -> Self {
        Self {
            entries: Vec::with_capacity(NEURAL_DELAY_QUEUE_CAPACITY),
        }
    }
}

impl DelayQueue {
    fn clear(&mut self) {
        self.entries.clear();
    }

    fn retain_external_seeds_at_or_after(&mut self, total_beats: f64) {
        self.entries
            .retain(|entry| entry.external_seed && entry.ready_after_beats + 1e-9 >= total_beats);
    }

    fn push(&mut self, entry: DelayedPropagation) {
        if self.entries.len() == NEURAL_DELAY_QUEUE_CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }
}

#[derive(Clone, Debug)]
struct NeuralFiringCandidate {
    neuron_idx: usize,
    fire_sample: u64,
    fire_beats: f64,
    event: StepEvent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NeuralOutput {
    pub sample_time: u64,
    pub event: StepEvent,
    pub emit_trigger: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NeuralVisualizationSnapshot {
    pub active: bool,
    pub network_id: u64,
    pub num_neurons: usize,
    pub energy: [f32; NUM_NEURONS],
    pub trigger_activity: [f32; NUM_NEURONS],
    pub dampening: [[f32; NUM_NEURONS]; NUM_NEURONS],
}

impl Default for NeuralVisualizationSnapshot {
    fn default() -> Self {
        Self {
            active: false,
            network_id: 0,
            num_neurons: 0,
            energy: [0.0; NUM_NEURONS],
            trigger_activity: [0.0; NUM_NEURONS],
            dampening: [[0.0; NUM_NEURONS]; NUM_NEURONS],
        }
    }
}

pub struct NeuralRuntime {
    active: bool,
    network_id: u64,
    num_neurons: usize,
    weights: [[f32; NUM_NEURONS]; NUM_NEURONS],
    neurons: [NeuronConfig; NUM_NEURONS],
    energy: [f32; NUM_NEURONS],
    trigger_activity: [f32; NUM_NEURONS],
    trigger_visual_until_beats: [f64; NUM_NEURONS],
    pending: [DelayQueue; NUM_NEURONS],
    dampening: [[f32; NUM_NEURONS]; NUM_NEURONS],
    incoming_triggers: [[f32; NUM_NEURONS]; NUM_NEURONS],
    source_events: [Option<StepEvent>; NUM_NEURONS],
    seed_on_reset: [f32; NUM_NEURONS],
    energy_decay: f32,
    max_poly: u32,
    max_poly_selection: NeuralMaxPolySelection,
    random_state: u64,
    reset_interval_beats: f64,
    next_reset_beat: f64,
    last_eval_indices: [u64; NUM_NEURONS],
    last_decay_index: u64,
}

impl Default for NeuralRuntime {
    fn default() -> Self {
        Self {
            active: false,
            network_id: 0,
            num_neurons: 0,
            weights: [[0.0; NUM_NEURONS]; NUM_NEURONS],
            neurons: [NeuronConfig::default(); NUM_NEURONS],
            energy: [0.0; NUM_NEURONS],
            trigger_activity: [0.0; NUM_NEURONS],
            trigger_visual_until_beats: [0.0; NUM_NEURONS],
            pending: std::array::from_fn(|_| DelayQueue::default()),
            dampening: [[0.0; NUM_NEURONS]; NUM_NEURONS],
            incoming_triggers: [[0.0; NUM_NEURONS]; NUM_NEURONS],
            source_events: std::array::from_fn(|_| None),
            seed_on_reset: [0.0; NUM_NEURONS],
            energy_decay: default_energy_decay(),
            max_poly: default_max_poly(),
            max_poly_selection: default_max_poly_selection(),
            random_state: default_neural_random_state(),
            reset_interval_beats: default_reset_interval_bars() as f64 * 4.0,
            next_reset_beat: 0.0,
            last_eval_indices: [0; NUM_NEURONS],
            last_decay_index: 0,
        }
    }
}

impl NeuralRuntime {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn visualization_snapshot(&self) -> NeuralVisualizationSnapshot {
        NeuralVisualizationSnapshot {
            active: self.active,
            network_id: self.network_id,
            num_neurons: self.num_neurons,
            energy: self.energy,
            trigger_activity: self.trigger_activity,
            dampening: self.dampening,
        }
    }

    pub fn load_from_networks(&mut self, networks: &[ProjectNeuralNetwork], total_beats: f64) {
        let Some(network) = networks
            .iter()
            .find(|network| network.enabled && network.num_neurons <= NUM_NEURONS)
        else {
            self.active = false;
            self.reset_state(total_beats);
            return;
        };

        let should_reset_state = !self.active
            || self.network_id != network.id
            || self.num_neurons != network.num_neurons;

        self.active = true;
        self.network_id = network.id;
        self.num_neurons = network.num_neurons;
        self.weights = [[0.0; NUM_NEURONS]; NUM_NEURONS];
        for row in 0..network.num_neurons {
            if let Some(src) = network.weights.get(row) {
                for col in 0..network.num_neurons {
                    self.weights[row][col] = src.get(col).copied().unwrap_or(0.0);
                }
            }
        }
        self.neurons = [NeuronConfig::default(); NUM_NEURONS];
        for idx in 0..network.num_neurons {
            let neuron = network.neurons.get(idx).cloned().unwrap_or_default();
            self.neurons[idx] = NeuronConfig {
                resolution: neuron.resolution_timebase(),
                delay_steps: neuron.delay_steps,
                threshold: neuron.threshold.max(0.0),
                transpose: neuron.transpose,
                quantize: neuron.quantize_timebase(),
                dampening_amount: neuron.dampening_amount.clamp(0.0, 1.0),
                dampening_recovery: neuron.dampening_recovery.clamp(0.0, 1.0),
                route: neuron.route,
                seed_track_mask: neuron_seed_track_mask(&neuron),
            };
        }
        self.seed_on_reset = [0.0; NUM_NEURONS];
        for idx in 0..network.num_neurons {
            self.seed_on_reset[idx] = network.seed_on_reset.get(idx).copied().unwrap_or(0.0);
        }
        self.energy_decay = network.energy_decay.clamp(0.0, 1.0);
        self.max_poly = network.max_poly.max(1);
        self.max_poly_selection = network.max_poly_selection;
        self.reset_interval_beats = (network.reset_interval_bars.max(0.25) as f64) * 4.0;
        if should_reset_state {
            self.random_state = neural_random_seed(network.id);
            self.reset_state(total_beats);
        } else {
            self.realign_timing_to_config(total_beats);
        }
    }

    fn realign_timing_to_config(&mut self, total_beats: f64) {
        for idx in 0..self.num_neurons {
            let step_beats = self.neurons[idx]
                .resolution
                .step_beats(NUM_NEURONS)
                .max(1e-9);
            self.last_eval_indices[idx] = (total_beats / step_beats).floor() as u64;
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        self.next_reset_beat =
            ((total_beats / self.reset_interval_beats).floor() + 1.0) * self.reset_interval_beats;
    }

    pub fn reset_state(&mut self, total_beats: f64) {
        self.energy = [0.0; NUM_NEURONS];
        self.trigger_activity = [0.0; NUM_NEURONS];
        self.trigger_visual_until_beats = [0.0; NUM_NEURONS];
        self.dampening = [[0.0; NUM_NEURONS]; NUM_NEURONS];
        self.incoming_triggers = [[0.0; NUM_NEURONS]; NUM_NEURONS];
        for queue in &mut self.pending {
            queue.retain_external_seeds_at_or_after(total_beats);
        }
        for event in &mut self.source_events {
            *event = None;
        }
        self.last_eval_indices = [0; NUM_NEURONS];
        for idx in 0..self.num_neurons {
            let step_beats = self.neurons[idx]
                .resolution
                .step_beats(NUM_NEURONS)
                .max(1e-9);
            self.last_eval_indices[idx] = (total_beats / step_beats).floor() as u64;
            if self.seed_on_reset[idx] > 0.0 {
                self.energy[idx] = self.seed_on_reset[idx];
            }
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        self.next_reset_beat =
            ((total_beats / self.reset_interval_beats).floor() + 1.0) * self.reset_interval_beats;
    }

    pub fn process_seed(&mut self, event: &StepEvent) {
        self.process_seed_at(event, 0.0);
    }

    pub fn process_seed_at(&mut self, event: &StepEvent, seed_beats: f64) {
        if !self.active {
            return;
        }
        for idx in 0..self.num_neurons {
            if self.neurons[idx].accepts_seed_track(event.track) {
                if event.resolved.velocity <= 0.0 {
                    continue;
                }
                self.pending[idx].push(DelayedPropagation {
                    remaining_steps: self.neurons[idx].delay_steps.max(1),
                    ready_after_beats: seed_beats,
                    event: event.clone(),
                    external_seed: true,
                });
            }
        }
    }

    pub fn process_boundaries(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        out: &mut Vec<(u64, StepEvent)>,
    ) {
        let mut outputs = Vec::new();
        self.process_boundaries_with_outputs(
            start_beats,
            end_beats,
            block_start_sample,
            samples_per_quarter,
            &mut outputs,
        );
        out.extend(
            outputs
                .into_iter()
                .filter(|output| output.emit_trigger)
                .map(|output| (output.sample_time, output.event)),
        );
    }

    pub fn process_boundaries_with_outputs(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        out: &mut Vec<NeuralOutput>,
    ) {
        if !self.active || end_beats <= start_beats {
            return;
        }

        while let Some(boundary_beats) = self.next_eval_boundary(start_beats, end_beats) {
            if self.next_reset_beat > 0.0 && self.next_reset_beat <= boundary_beats {
                self.apply_energy_decay(self.next_reset_beat);
                self.reset_state(self.next_reset_beat);
                continue;
            }

            let sample_offset = ((boundary_beats - start_beats) * samples_per_quarter)
                .round()
                .max(0.0) as u64;
            let sample_time = block_start_sample.saturating_add(sample_offset);
            let mut due_at_boundary = [false; NUM_NEURONS];
            for (neuron_idx, due) in due_at_boundary
                .iter_mut()
                .enumerate()
                .take(self.num_neurons)
            {
                let step_beats = self.neurons[neuron_idx]
                    .resolution
                    .step_beats(NUM_NEURONS)
                    .max(1e-9);
                let next_index = self.last_eval_indices[neuron_idx].saturating_add(1);
                let neuron_boundary = next_index as f64 * step_beats;
                *due = (neuron_boundary - boundary_beats).abs() <= 1e-9;
            }
            let mut deferred_energy = [0.0; NUM_NEURONS];
            let mut deferred_source_events: [Option<StepEvent>; NUM_NEURONS] =
                std::array::from_fn(|_| None);
            for neuron_idx in 0..self.num_neurons {
                if due_at_boundary[neuron_idx] {
                    let next_index = self.last_eval_indices[neuron_idx].saturating_add(1);
                    self.last_eval_indices[neuron_idx] = next_index;
                    self.apply_due_propagations(
                        neuron_idx,
                        boundary_beats,
                        &due_at_boundary,
                        &mut deferred_energy,
                        &mut deferred_source_events,
                    );
                }
            }
            let mut candidates = Vec::new();
            for neuron_idx in 0..self.num_neurons {
                if due_at_boundary[neuron_idx] {
                    if let Some(candidate) = self.firing_candidate(
                        neuron_idx,
                        boundary_beats,
                        sample_time,
                        samples_per_quarter,
                    ) {
                        candidates.push(candidate);
                    }
                }
            }
            candidates.sort_by_key(|candidate| (candidate.fire_sample, candidate.neuron_idx));
            let accepted = self.select_firing_candidates(&candidates);
            let mut rejected = [false; NUM_NEURONS];
            for candidate in &candidates {
                if !accepted[candidate.neuron_idx] {
                    rejected[candidate.neuron_idx] = true;
                }
            }
            for candidate in candidates {
                if accepted[candidate.neuron_idx] {
                    self.commit_firing(candidate, out);
                }
            }
            for neuron_idx in 0..self.num_neurons {
                if !due_at_boundary[neuron_idx] {
                    continue;
                }
                if accepted[neuron_idx] {
                    continue;
                }
                if rejected[neuron_idx] {
                    self.drop_firing(neuron_idx);
                } else {
                    self.recover_non_firing_neuron(neuron_idx);
                }
                self.clear_incoming_triggers(neuron_idx);
            }
            self.apply_energy_decay(boundary_beats);
            for idx in 0..self.num_neurons {
                if deferred_energy[idx] != 0.0 {
                    self.energy[idx] += deferred_energy[idx];
                }
                if let Some(event) = deferred_source_events[idx].take() {
                    self.source_events[idx] = Some(event);
                }
            }
        }

        while self.next_reset_beat > 0.0 && self.next_reset_beat <= end_beats {
            self.apply_energy_decay(self.next_reset_beat);
            self.reset_state(self.next_reset_beat);
        }
        self.apply_energy_decay(end_beats);
        self.refresh_trigger_activity(end_beats);
    }

    fn next_eval_boundary(&mut self, start_beats: f64, end_beats: f64) -> Option<f64> {
        let mut next_boundary: Option<f64> = None;
        for neuron_idx in 0..self.num_neurons {
            let step_beats = self.neurons[neuron_idx].resolution.step_beats(NUM_NEURONS);
            if let Some(boundary_beats) = next_grid_boundary(
                &mut self.last_eval_indices[neuron_idx],
                step_beats,
                start_beats,
                end_beats,
            ) {
                next_boundary = Some(match next_boundary {
                    Some(current) => current.min(boundary_beats),
                    None => boundary_beats,
                });
            }
        }
        next_boundary
    }

    fn apply_energy_decay(&mut self, end_beats: f64) {
        let decay_index = self.finest_decay_index(end_beats);
        while self.last_decay_index < decay_index {
            for idx in 0..self.num_neurons {
                self.energy[idx] *= self.energy_decay;
            }
            self.last_decay_index += 1;
        }
    }

    fn finest_decay_index(&self, total_beats: f64) -> u64 {
        let finest = (0..self.num_neurons)
            .map(|idx| self.neurons[idx].resolution.step_beats(NUM_NEURONS))
            .filter(|beats| *beats > 0.0)
            .fold(0.25_f64, f64::min);
        (total_beats / finest.max(1e-9)).floor() as u64
    }

    fn apply_due_propagations(
        &mut self,
        neuron_idx: usize,
        boundary_beats: f64,
        due_at_boundary: &[bool; NUM_NEURONS],
        deferred_energy: &mut [f32; NUM_NEURONS],
        deferred_source_events: &mut [Option<StepEvent>; NUM_NEURONS],
    ) {
        let mut propagated = Vec::new();
        for pending in &mut self.pending[neuron_idx].entries {
            if boundary_beats <= pending.ready_after_beats + 1e-9 {
                continue;
            }
            pending.remaining_steps = pending.remaining_steps.saturating_sub(1);
        }
        let mut keep = Vec::with_capacity(self.pending[neuron_idx].entries.capacity());
        for pending in self.pending[neuron_idx].entries.drain(..) {
            if pending.remaining_steps == 0 {
                propagated.push(pending.event);
            } else {
                keep.push(pending);
            }
        }
        self.pending[neuron_idx].entries = keep;

        for event in propagated {
            for target in 0..self.num_neurons {
                let amount = (self.weights[neuron_idx][target]
                    - self.dampening[neuron_idx][target])
                    .max(0.0);
                if amount <= 0.0 {
                    continue;
                }
                self.incoming_triggers[neuron_idx][target] = 1.0;
                if target == neuron_idx || (due_at_boundary[target] && target > neuron_idx) {
                    self.energy[target] += amount;
                    self.source_events[target] = Some(event.clone());
                } else {
                    deferred_energy[target] += amount;
                    deferred_source_events[target] = Some(event.clone());
                }
            }
        }
    }

    fn firing_candidate(
        &self,
        neuron_idx: usize,
        boundary_beats: f64,
        sample_time: u64,
        samples_per_quarter: f64,
    ) -> Option<NeuralFiringCandidate> {
        if self.energy[neuron_idx] < self.neurons[neuron_idx].threshold {
            return None;
        }
        let mut event = self.source_events[neuron_idx].clone()?;
        event.resolved = ResolvedStep {
            transpose: event.resolved.transpose + self.neurons[neuron_idx].transpose,
            ..event.resolved
        };
        event.source = EventSource::Network {
            seed: match event.source {
                EventSource::Step { track, step, .. } => Some((track, step)),
                EventSource::Network { seed, .. } => seed,
            },
            neuron: neuron_idx,
            instrument_fingerprint: 0,
        };
        let (fire_sample, fire_beats) = self.quantized_fire_timing(
            neuron_idx,
            boundary_beats,
            sample_time,
            samples_per_quarter,
        );
        Some(NeuralFiringCandidate {
            neuron_idx,
            fire_sample,
            fire_beats,
            event,
        })
    }

    fn commit_firing(&mut self, mut candidate: NeuralFiringCandidate, out: &mut Vec<NeuralOutput>) {
        let neuron_idx = candidate.neuron_idx;
        let emit_trigger = if let Some(route) = self.neurons[neuron_idx].route {
            candidate.event.track = route;
            true
        } else {
            false
        };
        out.push(NeuralOutput {
            sample_time: candidate.fire_sample,
            event: candidate.event.clone(),
            emit_trigger,
        });
        self.pending[neuron_idx].push(DelayedPropagation {
            remaining_steps: self.neurons[neuron_idx].delay_steps.max(1),
            ready_after_beats: candidate.fire_beats,
            event: candidate.event,
            external_seed: false,
        });
        self.trigger_activity[neuron_idx] = 1.0;
        self.trigger_visual_until_beats[neuron_idx] = self.trigger_visual_until_beats[neuron_idx]
            .max(candidate.fire_beats + TRIGGER_VISUAL_HOLD_BEATS);
        self.energy[neuron_idx] = 0.0;
        for source in 0..self.num_neurons {
            let trigger = self.incoming_triggers[source][neuron_idx];
            if trigger > 0.0 {
                self.dampening[source][neuron_idx] = (self.dampening[source][neuron_idx]
                    + trigger * self.neurons[neuron_idx].dampening_amount)
                    .min(1.0);
            }
        }
        self.clear_incoming_triggers(neuron_idx);
    }

    fn drop_firing(&mut self, neuron_idx: usize) {
        self.energy[neuron_idx] = 0.0;
    }

    fn recover_non_firing_neuron(&mut self, neuron_idx: usize) {
        for source in 0..self.num_neurons {
            self.dampening[source][neuron_idx] *= self.neurons[neuron_idx].dampening_recovery;
        }
    }

    fn clear_incoming_triggers(&mut self, neuron_idx: usize) {
        for source in 0..self.num_neurons {
            self.incoming_triggers[source][neuron_idx] = 0.0;
        }
    }

    fn refresh_trigger_activity(&mut self, total_beats: f64) {
        for idx in 0..self.num_neurons {
            self.trigger_activity[idx] = if total_beats <= self.trigger_visual_until_beats[idx] {
                1.0
            } else {
                0.0
            };
        }
    }

    fn quantized_fire_timing(
        &self,
        neuron_idx: usize,
        boundary_beats: f64,
        sample_time: u64,
        samples_per_quarter: f64,
    ) -> (u64, f64) {
        let Some(timebase) = self.neurons[neuron_idx].quantize else {
            return (sample_time, boundary_beats);
        };
        let grid_beats = timebase.step_beats(NUM_NEURONS).max(1e-9);
        let grid_position = boundary_beats / grid_beats;
        let nearest_grid = grid_position.round();
        let quantized_grid = if (grid_position - nearest_grid).abs() <= 1e-9 {
            nearest_grid
        } else {
            grid_position.ceil()
        };
        let quantized_beats = (quantized_grid * grid_beats).max(boundary_beats);
        let offset_samples = ((quantized_beats - boundary_beats) * samples_per_quarter)
            .round()
            .max(0.0) as u64;
        let quantized_sample = sample_time.saturating_add(offset_samples);
        (quantized_sample, quantized_beats)
    }

    fn select_firing_candidates(
        &mut self,
        candidates: &[NeuralFiringCandidate],
    ) -> [bool; NUM_NEURONS] {
        let accepted_count = candidates.len().min(self.max_poly as usize);
        let mut accepted = [false; NUM_NEURONS];
        if accepted_count == candidates.len() {
            for candidate in candidates {
                accepted[candidate.neuron_idx] = true;
            }
            return accepted;
        }

        match self.max_poly_selection {
            NeuralMaxPolySelection::Deterministic => {
                for candidate in candidates.iter().take(accepted_count) {
                    accepted[candidate.neuron_idx] = true;
                }
            }
            NeuralMaxPolySelection::Propagation => {
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                indices.sort_by(|left, right| {
                    let left_candidate = &candidates[*left];
                    let right_candidate = &candidates[*right];
                    self.propagation_selection_score(right_candidate.neuron_idx)
                        .total_cmp(&self.propagation_selection_score(left_candidate.neuron_idx))
                        .then(left_candidate.fire_sample.cmp(&right_candidate.fire_sample))
                        .then(left_candidate.neuron_idx.cmp(&right_candidate.neuron_idx))
                });
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidates[candidate_idx].neuron_idx] = true;
                }
            }
            NeuralMaxPolySelection::Random => {
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                for pos in 0..accepted_count {
                    let selected = pos + self.random_index(indices.len() - pos);
                    indices.swap(pos, selected);
                }
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidates[candidate_idx].neuron_idx] = true;
                }
            }
            NeuralMaxPolySelection::Loudest => {
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                indices.sort_by(|left, right| {
                    candidates[*right]
                        .event
                        .resolved
                        .velocity
                        .total_cmp(&candidates[*left].event.resolved.velocity)
                        .then(
                            candidates[*left]
                                .fire_sample
                                .cmp(&candidates[*right].fire_sample),
                        )
                        .then(
                            candidates[*left]
                                .neuron_idx
                                .cmp(&candidates[*right].neuron_idx),
                        )
                });
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidates[candidate_idx].neuron_idx] = true;
                }
            }
            NeuralMaxPolySelection::LowestTranspose | NeuralMaxPolySelection::HighestTranspose => {
                let highest = matches!(
                    self.max_poly_selection,
                    NeuralMaxPolySelection::HighestTranspose
                );
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                indices.sort_by(|left, right| {
                    let left_t = candidates[*left].event.resolved.transpose;
                    let right_t = candidates[*right].event.resolved.transpose;
                    let ordered = if highest {
                        right_t.total_cmp(&left_t)
                    } else {
                        left_t.total_cmp(&right_t)
                    };
                    ordered
                        .then(
                            candidates[*left]
                                .fire_sample
                                .cmp(&candidates[*right].fire_sample),
                        )
                        .then(
                            candidates[*left]
                                .neuron_idx
                                .cmp(&candidates[*right].neuron_idx),
                        )
                });
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidates[candidate_idx].neuron_idx] = true;
                }
            }
            // The native neural engine has no per-candidate seed tag; fall back to the
            // deterministic earliest-sample order. `seed-first` is meaningful on the graph
            // engine (def-sequencer/def-node), which carries the seed origin.
            NeuralMaxPolySelection::SeedFirst => {
                for candidate in candidates.iter().take(accepted_count) {
                    accepted[candidate.neuron_idx] = true;
                }
            }
        }
        accepted
    }

    fn propagation_selection_score(&self, source: usize) -> f32 {
        let mut score = 0.0;
        for target in 0..self.num_neurons {
            let amount = (self.weights[source][target] - self.dampening[source][target]).max(0.0);
            if amount <= 0.0 {
                continue;
            }
            let threshold = self.neurons[target].threshold.max(1e-6);
            let projected = self.energy[target] + amount;
            if projected >= threshold {
                score += 1_000.0 + (projected - threshold);
            } else {
                score += amount / threshold;
            }
        }
        score
    }

    fn random_index(&mut self, upper: usize) -> usize {
        debug_assert!(upper > 0);
        (self.next_random_u64() % upper as u64) as usize
    }

    fn next_random_u64(&mut self) -> u64 {
        self.random_state = self.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.random_state)
    }
}

fn default_enabled() -> bool {
    true
}

fn default_num_neurons() -> usize {
    NUM_NEURONS
}

fn default_neuron_resolution() -> u8 {
    Timebase::Sixteenth as u8
}

fn default_threshold() -> f32 {
    1.0
}

fn default_dampening_recovery() -> f32 {
    0.98
}

fn default_reset_interval_bars() -> f32 {
    4.0
}

fn default_energy_decay() -> f32 {
    0.994
}

fn default_max_poly() -> u32 {
    2
}

fn default_max_poly_selection() -> NeuralMaxPolySelection {
    NeuralMaxPolySelection::Deterministic
}

fn default_neural_random_state() -> u64 {
    0xA076_1D64_78BD_642F
}

fn neural_random_seed(network_id: u64) -> u64 {
    let seed = splitmix64(network_id ^ default_neural_random_state());
    if seed == 0 {
        default_neural_random_state()
    } else {
        seed
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accumulator::ResolvedStep;
    use crate::scheduled_event::{
        EventSource, ScheduledChordData, ScheduledInstrumentParams,
        ScheduledInstrumentTensorParams, ScheduledSamplerParams, StepEvent,
    };
    use crate::audio::MAX_VOICES;

    fn test_event(track: usize) -> StepEvent {
        StepEvent {
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            track,
            samples_per_step: 12_000.0,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity: 1.0,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; MAX_VOICES],
                durations: [0.0; MAX_VOICES],
                delays: [0.0; MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Step {
                track,
                step: 0,
                instrument_fingerprint: 0,
            },
        }
    }

    fn output_neuron(event: &StepEvent) -> usize {
        match event.source {
            EventSource::Network { neuron, .. } => neuron,
            EventSource::Step { .. } => panic!("expected network event"),
        }
    }

    #[test]
    fn instrument_source_replacement_removes_only_target_track_instrument_overrides() {
        let param_id = ParamNodeId {
            logical_id: 10,
            node_param_idx: 2,
        };
        let mut network = ProjectNeuralNetwork::default();
        network.neurons[0].output_overrides.instrument = vec![
            ProjectParamOverride {
                target_track: 0,
                param_id,
                param_index: 0,
                value: 0.25,
            },
            ProjectParamOverride {
                target_track: 1,
                param_id,
                param_index: 0,
                value: 0.75,
            },
        ];
        network.neurons[0]
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 0,
                slot_index: 0,
                param_id,
                param_index: 0,
                value: 0.5,
            });

        assert_eq!(
            remove_instrument_overrides_for_track(&mut [network.clone()], 9),
            0
        );
        assert_eq!(
            remove_instrument_overrides_for_track(std::slice::from_mut(&mut network), 0),
            1
        );
        assert_eq!(
            network.neurons[0].output_overrides.instrument,
            vec![ProjectParamOverride {
                target_track: 1,
                param_id,
                param_index: 0,
                value: 0.75,
            }]
        );
        assert_eq!(network.neurons[0].output_overrides.effects.len(), 1);
    }

    #[test]
    fn graph_runtime_matches_native_neural_for_seeded_transpose_hop() {
        let mut network = ProjectNeuralNetwork {
            enabled: true,
            num_neurons: 2,
            weights: vec![vec![0.0, 1.0], vec![0.0, 0.0]],
            neurons: vec![ProjectNeuron::default(), ProjectNeuron::default()],
            max_poly: 2,
            ..ProjectNeuralNetwork::default()
        };
        network.neurons[0].route = Some(0);
        network.neurons[1].route = Some(1);
        network.neurons[1].transpose = 7.0;

        let mut native = NeuralRuntime::default();
        native.load_from_networks(&[network], 0.0);
        let mut seed = test_event(0);
        seed.resolved.transpose = 4.0;
        seed.resolved.velocity = 0.6;
        native.process_seed_at(&seed, 0.0);
        let mut native_out = Vec::new();
        native.process_boundaries(0.0, 2.0, 0, 48_000.0, &mut native_out);

        let mut graph_nodes = vec![crate::graph::GraphNode::default(); 2];
        graph_nodes[0].resolution = Timebase::Sixteenth;
        graph_nodes[0].route = Some(0);
        graph_nodes[0].seed_track_mask = crate::graph::seed_track_mask(&[0]);
        graph_nodes[1].resolution = Timebase::Sixteenth;
        graph_nodes[1].route = Some(1);
        graph_nodes[1].transpose = 7.0;
        graph_nodes[1].threshold = 1.0;
        let mut graph = crate::graph::GraphRuntime::new(
            1,
            "neural".to_string(),
            graph_nodes,
            vec![crate::graph::GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        graph.seed(
            0,
            0.0,
            crate::graph::GraphPayload {
                note: seed.resolved.transpose,
                velocity: seed.resolved.velocity,
                duration_beats: seed.resolved.duration,
            },
        );
        let mut graph_out = Vec::new();
        graph.process_block(
            0.0,
            2.0,
            0,
            48_000.0,
            2,
            |eval| crate::graph::NodeFire {
                fired: eval.energy >= 1.0,
                ..crate::graph::NodeFire::default()
            },
            &mut graph_out,
        );

        let native_summary = native_out
            .iter()
            .map(|(sample, event)| {
                (
                    *sample,
                    event.track,
                    event.resolved.transpose,
                    event.resolved.velocity,
                )
            })
            .collect::<Vec<_>>();
        let graph_summary = graph_out
            .iter()
            .map(|emission| {
                (
                    emission.sample_time,
                    emission.event.track.unwrap_or(usize::MAX),
                    emission.event.resolved.transpose,
                    emission.event.resolved.velocity,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(graph_summary, native_summary);
    }

    #[test]
    fn runtime_runs_first_enabled_valid_network() {
        let mut disabled = ProjectNeuralNetwork::default();
        disabled.id = 1;
        disabled.enabled = false;
        let mut enabled = ProjectNeuralNetwork::default();
        enabled.id = 2;
        enabled.num_neurons = 1;
        enabled.weights = vec![vec![1.0]];
        enabled.neurons[0].route = Some(0);
        enabled.neurons[0].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[disabled, enabled], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        assert_eq!(runtime.network_id, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.track, 0);
    }

    #[test]
    fn route_off_neuron_can_seed_from_target_track_plock() {
        let mut network = ProjectNeuralNetwork::default();
        network.id = 9;
        network.num_neurons = 1;
        network.weights = vec![vec![1.0]];
        network.neurons[0].route = None;
        network.neurons[0].threshold = 0.5;
        network.neurons[0]
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 0,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: 0,
                },
                param_index: 0,
                value: 0.75,
            });

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries_with_outputs(0.0, 0.25, 0, 48_000.0, &mut out);

        assert_eq!(out.len(), 1);
        assert!(!out[0].emit_trigger);
        assert_eq!(output_neuron(&out[0].event), 0);
        assert_eq!(out[0].event.track, 0);
    }

    #[test]
    fn reloading_same_network_config_preserves_runtime_state() {
        let mut network = ProjectNeuralNetwork::default();
        network.id = 7;
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].delay_steps = 2;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network.clone()], 0.0);
        runtime.process_seed(&test_event(0));
        runtime.energy[1] = 0.75;
        runtime.source_events[1] = Some(test_event(0));
        assert_eq!(runtime.pending[0].entries.len(), 1);

        network.neurons[0].delay_steps = 3;
        network.neurons[1].quantize = Some(Timebase::Quarter as u8);
        runtime.load_from_networks(&[network], 0.25);

        assert_eq!(runtime.pending[0].entries.len(), 1);
        assert_eq!(runtime.energy[1], 0.75);
        assert!(runtime.source_events[1].is_some());
        assert_eq!(runtime.neurons[0].delay_steps, 3);
        assert_eq!(runtime.neurons[1].quantize, Some(Timebase::Quarter));
    }

    #[test]
    fn reloading_different_selected_network_resets_runtime_state() {
        let mut first = ProjectNeuralNetwork::default();
        first.id = 1;
        first.num_neurons = 1;
        first.weights = vec![vec![1.0]];
        first.neurons[0].route = Some(0);
        first.neurons[0].threshold = 0.5;

        let mut second = first.clone();
        second.id = 2;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[first], 0.0);
        runtime.process_seed(&test_event(0));
        runtime.energy[0] = 0.75;
        runtime.source_events[0] = Some(test_event(0));
        assert_eq!(runtime.pending[0].entries.len(), 1);

        runtime.load_from_networks(&[second], 0.25);

        assert_eq!(runtime.network_id, 2);
        assert_eq!(runtime.pending[0].entries.len(), 0);
        assert_eq!(runtime.energy[0], 0.0);
        assert!(runtime.source_events[0].is_none());
    }

    #[test]
    fn threshold_compare_happens_before_energy_decay() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.weights = vec![vec![1.0]];
        network.energy_decay = 0.5;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 1.0;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.track, 0);
        assert_eq!(runtime.energy[0], 0.0);
    }

    #[test]
    fn subthreshold_energy_decays_after_boundary_compare() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.weights = vec![vec![1.0]];
        network.energy_decay = 0.5;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 2.0;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        assert!(out.is_empty());
        assert_eq!(runtime.energy[0], 0.5);
    }

    #[test]
    fn delay_queue_is_bounded_and_drops_oldest() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].delay_steps = 16;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        for _ in 0..NEURAL_DELAY_QUEUE_CAPACITY + 1 {
            runtime.process_seed(&test_event(0));
        }

        assert_eq!(
            runtime.pending[0].entries.len(),
            NEURAL_DELAY_QUEUE_CAPACITY
        );
    }

    #[test]
    fn evaluates_by_sample_time_then_neuron_index() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.neurons[0].route = Some(0);
        network.neurons[0].resolution = Timebase::Eighth as u8;
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(0);
        network.neurons[1].resolution = Timebase::Sixteenth as u8;
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.energy[0] = 1.0;
        runtime.energy[1] = 1.0;
        runtime.source_events[0] = Some(test_event(0));
        runtime.source_events[1] = Some(test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.5, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 1), (24_000, 0)]);
    }

    #[test]
    fn seed_propagates_without_reemitting_seed_neuron() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 1, 1)]);
    }

    #[test]
    fn seed_timestamp_cannot_affect_earlier_or_same_boundary() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed_at(&test_event(0), 0.25);

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);
        assert!(
            out.is_empty(),
            "a seed at a boundary must not feed that same boundary"
        );

        runtime.process_boundaries(0.25, 0.5, 12_000, 48_000.0, &mut out);
        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(24_000, 1, 1)]);
    }

    #[test]
    fn seed_uses_source_neuron_delay_for_first_propagation() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].delay_steps = 3;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(36_000, 1, 1)]);
    }

    #[test]
    fn backward_edge_energy_is_not_decayed_before_next_compare() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 3;
        network.weights = vec![
            vec![0.0, 1.0, 0.0],
            vec![1.0, 0.0, 1.0],
            vec![0.0, 0.0, 0.0],
        ];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 1.0;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 1.0;
        network.neurons[2].route = Some(2);
        network.neurons[2].threshold = 1.0;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.75, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![(12_000, 1, 1), (24_000, 2, 2), (36_000, 0, 0)]
        );
    }

    #[test]
    fn delayed_network_propagation_becomes_visible_on_later_boundary() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 3;
        network.weights = vec![
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.0, 0.0],
        ];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.neurons[1].delay_steps = 2;
        network.neurons[2].route = Some(2);
        network.neurons[2].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 1, 1), (36_000, 2, 2)]);
    }

    #[test]
    fn dampening_matrix_suppresses_the_specific_triggered_edge() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.neurons[1].dampening_amount = 1.0;
        network.neurons[1].dampening_recovery = 1.0;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed_at(&test_event(0), 0.0);

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);
        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 1, 1)]);
        assert_eq!(runtime.visualization_snapshot().dampening[0][1], 1.0);

        out.clear();
        runtime.process_seed_at(&test_event(0), 1.0);
        runtime.process_boundaries(1.0, 1.25, 48_000, 48_000.0, &mut out);

        assert!(
            out.is_empty(),
            "fully damped edge 0->1 should not retrigger neuron 1"
        );
    }

    #[test]
    fn reset_clears_dampening_visualization_state() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.neurons[1].dampening_amount = 1.0;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed_at(&test_event(0), 0.0);

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);
        assert!(runtime.visualization_snapshot().dampening[0][1] > 0.0);

        runtime.reset_state(1.0);
        assert_eq!(runtime.visualization_snapshot().dampening[0][1], 0.0);
        assert_eq!(runtime.visualization_snapshot().trigger_activity[0], 0.0);
        assert_eq!(runtime.visualization_snapshot().energy[0], 0.0);
    }

    #[test]
    fn accepted_firing_sets_trigger_visualization_activity() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.energy[0] = 1.0;
        runtime.source_events[0] = Some(test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        let snapshot = runtime.visualization_snapshot();
        assert_eq!(snapshot.trigger_activity[0], 1.0);
        assert_eq!(snapshot.energy[0], 0.0);
    }

    #[test]
    fn quantize_snaps_to_project_relative_grid() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.weights = vec![vec![1.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].quantize = Some(Timebase::Eighth as u8);

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.20, 0.30, 9_600, 48_000.0, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 24_000);
    }

    #[test]
    fn quantize_uses_transport_beats_not_absolute_sample_phase() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.weights = vec![vec![1.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].quantize = Some(Timebase::Eighth as u8);

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.energy[0] = 1.0;
        runtime.source_events[0] = Some(test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.20, 0.30, 109_600, 48_000.0, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 124_000);
    }

    #[test]
    fn quantized_fire_time_anchors_outgoing_propagation_delay() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 3;
        network.weights = vec![
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.0, 0.0],
        ];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.neurons[1].quantize = Some(Timebase::Quarter as u8);
        network.neurons[2].route = Some(2);
        network.neurons[2].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.5, 0, 48_000.0, &mut out);
        out.sort_by_key(|(sample, event)| (*sample, output_neuron(event)));

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(48_000, 1, 1), (60_000, 2, 2)]);
    }

    #[test]
    fn max_poly_caps_same_boundary_firings_and_drops_rejected_candidates() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 3;
        network.max_poly = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.neurons[2].route = Some(2);
        network.neurons[2].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        for idx in 0..3 {
            runtime.energy[idx] = 1.0;
            runtime.source_events[idx] = Some(test_event(idx));
        }

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 0, 0)]);
        assert_eq!(runtime.pending[0].entries.len(), 1);
        assert!(runtime.pending[1].entries.is_empty());
        assert!(runtime.pending[2].entries.is_empty());
        assert_eq!(runtime.energy[1], 0.0);
        assert_eq!(runtime.energy[2], 0.0);

        out.clear();
        runtime.process_boundaries(0.25, 0.5, 12_000, 48_000.0, &mut out);
        assert!(
            out.is_empty(),
            "rejected max_poly candidates should be dropped instead of firing later"
        );
    }

    #[test]
    fn max_poly_accepts_earliest_quantized_sample_then_neuron_index() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.max_poly = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].quantize = Some(Timebase::Quarter as u8);
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.energy[0] = 1.0;
        runtime.energy[1] = 1.0;
        runtime.source_events[0] = Some(test_event(0));
        runtime.source_events[1] = Some(test_event(1));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 0.25, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(12_000, 1, 1)]);
        assert!(runtime.pending[0].entries.is_empty());
        assert_eq!(runtime.pending[1].entries.len(), 1);
    }

    #[test]
    fn max_poly_random_selection_can_accept_non_first_candidate() {
        let mut runtime = NeuralRuntime {
            max_poly: 1,
            max_poly_selection: NeuralMaxPolySelection::Random,
            random_state: 0,
            ..NeuralRuntime::default()
        };
        let candidates = (0..4)
            .map(|neuron_idx| NeuralFiringCandidate {
                neuron_idx,
                fire_sample: 0,
                fire_beats: 0.0,
                event: test_event(neuron_idx),
            })
            .collect::<Vec<_>>();

        let accepted = runtime.select_firing_candidates(&candidates);

        assert!(accepted[3], "fixed random state should choose neuron 3");
        assert!(
            !accepted[0],
            "random mode should not always choose the first candidate"
        );
    }

    #[test]
    fn max_poly_propagation_selection_prioritizes_effective_downstream_trigger() {
        let mut runtime = NeuralRuntime {
            num_neurons: 4,
            max_poly: 1,
            max_poly_selection: NeuralMaxPolySelection::Propagation,
            ..NeuralRuntime::default()
        };
        for idx in 0..4 {
            runtime.neurons[idx].threshold = 0.5;
        }
        runtime.weights[0][3] = 1.0;
        runtime.dampening[0][3] = 1.0;
        runtime.weights[1][3] = 0.2;
        runtime.weights[2][3] = 0.6;
        runtime.weights[3][3] = 0.1;
        let candidates = (0..4)
            .map(|neuron_idx| NeuralFiringCandidate {
                neuron_idx,
                fire_sample: 0,
                fire_beats: 0.0,
                event: test_event(neuron_idx),
            })
            .collect::<Vec<_>>();

        let accepted = runtime.select_firing_candidates(&candidates);

        assert!(
            accepted[2],
            "propagation mode should choose the candidate most likely to trigger downstream"
        );
        assert!(
            !accepted[0],
            "fully damped outgoing edges should not win propagation priority"
        );
    }

    #[test]
    fn reset_interval_does_not_clear_pre_reset_boundaries_in_same_block() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.reset_interval_bars = 0.25;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.energy[0] = 1.0;
        runtime.source_events[0] = Some(test_event(0));

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.0, 0, 48_000.0, &mut out);

        let samples = out.iter().map(|(sample, _)| *sample).collect::<Vec<_>>();
        assert_eq!(samples, vec![12_000]);

        out.clear();
        runtime.process_boundaries(1.0, 1.25, 48_000, 48_000.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn reset_interval_preserves_external_seed_at_reset_boundary() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;
        network.reset_interval_bars = 0.25;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed_at(&test_event(0), 1.0);

        let mut out = Vec::new();
        runtime.process_boundaries(0.0, 1.25, 0, 48_000.0, &mut out);

        let observed = out
            .iter()
            .map(|(sample, event)| (*sample, event.track, output_neuron(event)))
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(60_000, 1, 1)]);
    }

    #[test]
    fn manual_reset_clears_pending_delays() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 2;
        network.weights = vec![vec![0.0, 1.0], vec![0.0, 0.0]];
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].delay_steps = 1;
        network.neurons[1].route = Some(1);
        network.neurons[1].threshold = 0.5;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        runtime.process_seed(&test_event(0));

        assert_eq!(runtime.pending[0].entries.len(), 1);

        runtime.reset_state(0.25);
        assert!(runtime.pending[0].entries.is_empty());

        let mut out = Vec::new();
        out.clear();
        runtime.process_boundaries(0.25, 0.5, 12_000, 48_000.0, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn grid_boundary_clock_emits_each_boundary_once_per_bar() {
        let res = Timebase::Sixteenth.step_beats(NUM_NEURONS);
        let mut clock = GridBoundaryClock::new(res);
        let mut hits: Vec<(u64, f64, u64)> = Vec::new();
        process_grid_boundaries(&mut clock, 0.0, 4.0, 0, 48_000.0, |beats, index, sample| {
            hits.push((index, beats, sample));
        });

        let expected = (4.0 / res).floor() as usize;
        assert_eq!(hits.len(), expected);
        // Indices are contiguous 1..=expected, ascending.
        for (offset, (index, beats, _)) in hits.iter().enumerate() {
            assert_eq!(*index, offset as u64 + 1);
            assert!((*beats - (offset as f64 + 1.0) * res).abs() <= 1e-9);
        }
        // First boundary is sample-accurate: res beats * samples_per_quarter.
        assert_eq!(hits[0].2, (res * 48_000.0).round() as u64);
    }

    #[test]
    fn grid_boundary_clock_does_not_refire_boundary_on_block_edge() {
        let res = Timebase::Sixteenth.step_beats(NUM_NEURONS);
        let mut clock = GridBoundaryClock::new(res);

        // A boundary landing exactly on the first block's end fires once.
        let mut first = Vec::new();
        process_grid_boundaries(&mut clock, 0.0, res, 0, 48_000.0, |_, index, _| {
            first.push(index);
        });
        assert_eq!(first, vec![1]);

        // The next block starting at that same beat must not re-emit it.
        let mut second = Vec::new();
        process_grid_boundaries(
            &mut clock,
            res,
            res * 2.0,
            (res * 48_000.0).round() as u64,
            48_000.0,
            |_, index, _| second.push(index),
        );
        assert_eq!(second, vec![2]);
    }
}
