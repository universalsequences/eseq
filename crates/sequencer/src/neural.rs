use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::scheduled_event::{EventSource, StepEvent};
use crate::sequencer::Timebase;

pub const NUM_NEURONS: usize = 16;
pub const NEURAL_DELAY_QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParamNodeId {
    pub logical_id: u64,
    pub node_param_idx: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectParamOverride {
    pub param_id: ParamNodeId,
    pub param_index: usize,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectEffectParamOverride {
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
        }
    }
}

#[derive(Clone, Debug)]
struct DelayedPropagation {
    remaining_steps: u32,
    event: StepEvent,
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

    fn push(&mut self, entry: DelayedPropagation) {
        if self.entries.len() == NEURAL_DELAY_QUEUE_CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }
}

pub struct NeuralRuntime {
    active: bool,
    network_id: u64,
    num_neurons: usize,
    weights: [[f32; NUM_NEURONS]; NUM_NEURONS],
    neurons: [NeuronConfig; NUM_NEURONS],
    energy: [f32; NUM_NEURONS],
    pending: [DelayQueue; NUM_NEURONS],
    dampening_level: [f32; NUM_NEURONS],
    source_events: [Option<StepEvent>; NUM_NEURONS],
    seed_on_reset: [f32; NUM_NEURONS],
    energy_decay: f32,
    max_poly: u32,
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
            pending: std::array::from_fn(|_| DelayQueue::default()),
            dampening_level: [0.0; NUM_NEURONS],
            source_events: std::array::from_fn(|_| None),
            seed_on_reset: [0.0; NUM_NEURONS],
            energy_decay: default_energy_decay(),
            max_poly: default_max_poly(),
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

    pub fn load_from_networks(&mut self, networks: &[ProjectNeuralNetwork], total_beats: f64) {
        let Some(network) = networks
            .iter()
            .find(|network| network.enabled && network.num_neurons <= NUM_NEURONS)
        else {
            self.active = false;
            self.reset_state(total_beats);
            return;
        };

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
            };
        }
        self.seed_on_reset = [0.0; NUM_NEURONS];
        for idx in 0..network.num_neurons {
            self.seed_on_reset[idx] = network.seed_on_reset.get(idx).copied().unwrap_or(0.0);
        }
        self.energy_decay = network.energy_decay.clamp(0.0, 1.0);
        self.max_poly = network.max_poly.max(1);
        self.reset_interval_beats = (network.reset_interval_bars.max(0.25) as f64) * 4.0;
        self.reset_state(total_beats);
    }

    pub fn reset_state(&mut self, total_beats: f64) {
        self.energy = [0.0; NUM_NEURONS];
        self.dampening_level = [0.0; NUM_NEURONS];
        for queue in &mut self.pending {
            queue.clear();
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
        if !self.active {
            return;
        }
        for idx in 0..self.num_neurons {
            if self.neurons[idx].route == Some(event.track) {
                self.energy[idx] += event.resolved.velocity.max(0.0);
                self.source_events[idx] = Some(event.clone());
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
        if !self.active || end_beats <= start_beats {
            return;
        }
        while self.next_reset_beat > 0.0 && self.next_reset_beat <= end_beats {
            self.reset_state(self.next_reset_beat);
        }
        self.apply_energy_decay(end_beats);

        for neuron_idx in 0..self.num_neurons {
            let step_beats = self.neurons[neuron_idx]
                .resolution
                .step_beats(NUM_NEURONS)
                .max(1e-9);
            let end_index = (end_beats / step_beats).floor() as u64;
            while self.last_eval_indices[neuron_idx] < end_index {
                self.last_eval_indices[neuron_idx] += 1;
                let boundary_beats = self.last_eval_indices[neuron_idx] as f64 * step_beats;
                if boundary_beats <= start_beats || boundary_beats > end_beats {
                    continue;
                }
                let sample_offset = ((boundary_beats - start_beats) * samples_per_quarter)
                    .round()
                    .max(0.0) as u64;
                let sample_time = block_start_sample.saturating_add(sample_offset);
                self.evaluate_neuron(neuron_idx, sample_time, samples_per_quarter, out);
            }
        }
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

    fn evaluate_neuron(
        &mut self,
        neuron_idx: usize,
        sample_time: u64,
        samples_per_quarter: f64,
        out: &mut Vec<(u64, StepEvent)>,
    ) {
        let mut propagated = Vec::new();
        for pending in &mut self.pending[neuron_idx].entries {
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
            let scale = 1.0 - self.dampening_level[neuron_idx];
            for target in 0..self.num_neurons {
                let amount = self.weights[neuron_idx][target] * scale;
                if amount == 0.0 {
                    continue;
                }
                self.energy[target] += amount;
                self.source_events[target] = Some(event.clone());
            }
        }

        if self.energy[neuron_idx] >= self.neurons[neuron_idx].threshold {
            let source = self.source_events[neuron_idx].clone();
            if let Some(mut event) = source {
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
                if let Some(route) = self.neurons[neuron_idx].route {
                    event.track = route;
                    let audio_sample =
                        self.quantized_sample(neuron_idx, sample_time, samples_per_quarter);
                    out.push((audio_sample, event.clone()));
                }
                self.pending[neuron_idx].push(DelayedPropagation {
                    remaining_steps: self.neurons[neuron_idx].delay_steps.max(1),
                    event,
                });
            }
            self.energy[neuron_idx] = 0.0;
            self.dampening_level[neuron_idx] = (self.dampening_level[neuron_idx]
                + self.neurons[neuron_idx].dampening_amount)
                .min(1.0);
        }
        self.dampening_level[neuron_idx] *= self.neurons[neuron_idx].dampening_recovery;
    }

    fn quantized_sample(
        &self,
        neuron_idx: usize,
        sample_time: u64,
        samples_per_quarter: f64,
    ) -> u64 {
        let Some(timebase) = self.neurons[neuron_idx].quantize else {
            return sample_time;
        };
        let grid = (timebase.step_beats(NUM_NEURONS) * samples_per_quarter)
            .round()
            .max(1.0) as u64;
        let rem = sample_time % grid;
        if rem == 0 {
            sample_time
        } else {
            sample_time.saturating_add(grid - rem)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accumulator::ResolvedStep;
    use crate::scheduled_event::{
        EventSource, ScheduledChordData, ScheduledInstrumentParams, ScheduledSamplerParams,
        StepEvent,
    };
    use crate::voice::MAX_VOICES;

    fn test_event(track: usize) -> StepEvent {
        StepEvent {
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
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Step {
                track,
                step: 0,
                instrument_fingerprint: 0,
            },
        }
    }

    #[test]
    fn runtime_runs_first_enabled_valid_network() {
        let mut disabled = ProjectNeuralNetwork::default();
        disabled.id = 1;
        disabled.enabled = false;
        let mut enabled = ProjectNeuralNetwork::default();
        enabled.id = 2;
        enabled.num_neurons = 1;
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
    fn delay_queue_is_bounded_and_drops_oldest() {
        let mut network = ProjectNeuralNetwork::default();
        network.num_neurons = 1;
        network.neurons[0].route = Some(0);
        network.neurons[0].threshold = 0.5;
        network.neurons[0].delay_steps = 16;

        let mut runtime = NeuralRuntime::default();
        runtime.load_from_networks(&[network], 0.0);
        for idx in 0..NEURAL_DELAY_QUEUE_CAPACITY + 1 {
            runtime.process_seed(&test_event(0));
            let mut out = Vec::new();
            runtime.process_boundaries(
                idx as f64 * 0.25,
                (idx + 1) as f64 * 0.25,
                idx as u64 * 12_000,
                48_000.0,
                &mut out,
            );
        }

        assert_eq!(
            runtime.pending[0].entries.len(),
            NEURAL_DELAY_QUEUE_CAPACITY
        );
    }
}
