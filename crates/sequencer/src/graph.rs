//! Graph-mode sequencer runtime — the engine half of graph-mode `def-sequencer`.
//!
//! A *graph* sequencer is a node field (the *shape*) wired by *edges*, ticked by a
//! native gather/scatter loop. Each grid boundary the engine (1) gathers weighted,
//! delayed contributions along edges into each node's transient input accumulator,
//! (2) runs the node's `:update` rule to decide whether to fire, and (3) scatters a
//! fired node's signal back along its out-edges after a per-node delay.
//!
//! This module owns **only** the timing/ordering/gather-scatter machinery and is
//! deliberately lisp-agnostic: the per-node fire decision is a plain Rust closure
//! (`update_fn`), so the engine is driven directly in unit tests and wired to the
//! scheduler-side lisp VM in `lisp_effect.rs`/`scheduler.rs`. This mirrors how
//! [`crate::generator::GeneratorRuntime`] keeps the tick closure external.
//!
//! It is a generalization of the hardcoded neural sequencer (`neural.rs`): the
//! per-neuron `GridBoundaryClock`, the `delay_steps` propagation queue, the
//! `quantize` fire-time snap, and the finest-grid energy decay are all reused here,
//! but over a `Vec`-sized node field with a sparse edge set rather than a fixed
//! 64-neuron dense weight matrix.
//!
//! Status: v1a (grid skeleton) + v1b (payload relay). Nodes accumulate gather into
//! `energy`, fire on a pluggable predicate, propagate after `delay_steps`, and re-emit
//! the incoming payload (Ext 1) with the node's `transpose` added so a seed note
//! ripples and re-pitches on every hop. Edge-state dampening (Ext 2) and the rest of
//! engine reset/seed config (Ext 3) land in v1c — the data carries the fields it needs
//! (`dampening`, `seed_track_mask`, `reset_interval_beats`, `seed_on_reset`).

use crate::generator::{default_resolved, GENERATOR_RESOLUTION_REF_STEPS};
use crate::lisp_effect::EmittedAccumulatorEvent;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::neural::{next_grid_boundary, NeuralMaxPolySelection};
use crate::sequencer::Timebase;

/// Reference subdivision used to convert a `Timebase` to beats — only affects
/// `Timebase::Polyrhythm`. Matches the generator/neural convention so all three
/// clocks agree.
pub const GRAPH_RESOLUTION_REF_STEPS: usize = GENERATOR_RESOLUTION_REF_STEPS;

/// Coarsest fallback decay step (quarter note), used when no node declares a
/// resolution. Matches `neural::finest_decay_index`.
const DEFAULT_DECAY_STEP_BEATS: f64 = 0.25;

/// How a node folds the several edge currents arriving in one boundary into the
/// single `node-input` scalar its `:update` reads. `gather` lives on the edge;
/// `reduce` lives on the node (spec §1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reduce {
    Sum,
    Max,
    Min,
    Product,
    Count,
}

impl Default for Reduce {
    fn default() -> Self {
        Reduce::Sum
    }
}

impl Reduce {
    /// Fold `incoming` into `acc`. `acc` is the running value; `first` is true when
    /// `incoming` is the first current of the boundary (so `Max`/`Min` seed from it
    /// rather than from a sentinel, and `Count` starts at 1).
    fn fold(self, acc: f64, incoming: f64, first: bool) -> f64 {
        if first {
            return match self {
                Reduce::Count => 1.0,
                _ => incoming,
            };
        }
        match self {
            Reduce::Sum => acc + incoming,
            Reduce::Max => acc.max(incoming),
            Reduce::Min => acc.min(incoming),
            Reduce::Product => acc * incoming,
            Reduce::Count => acc + 1.0,
        }
    }
}

/// One directed edge in the graph. `weight`/`dampening` are edge fields read by the
/// native gather kernel; `dampening` is mutated at runtime by Ext 2 (v1c) and is 0 in
/// v1a. `delay_steps` is per-edge transport delay (0 in v1a; node `delay_steps`
/// carries the propagation latency for now).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub weight: f64,
    pub dampening: f64,
    pub delay_steps: u32,
}

impl GraphEdge {
    pub fn new(from: usize, to: usize, weight: f64) -> Self {
        Self {
            from,
            to,
            weight,
            dampening: 0.0,
            delay_steps: 0,
        }
    }

    /// The scalar this edge injects into its target this boundary: `max(0, weight -
    /// dampening)` (spec §2.3 neural case — spike propagation, magnitude independent
    /// of source activation).
    fn gather(self) -> f64 {
        (self.weight - self.dampening).max(0.0)
    }
}

/// Per-instance node configuration. These are the **intrinsic** fields the engine
/// reads to schedule/route (spec §2.2); they are prototype defaults but per-instance
/// editable. `seed_track_mask` is the resolved `seed-from` track set (Ext 1/§4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGraphRouteOverride {
    None,
    Track(usize),
}

impl ProjectGraphRouteOverride {
    fn to_route(&self) -> Option<usize> {
        match self {
            Self::None => None,
            Self::Track(track) => Some(*track),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGraphQuantizeOverride {
    Off,
    Timebase(u8),
}

impl ProjectGraphQuantizeOverride {
    fn to_quantize(&self) -> Option<Timebase> {
        match self {
            Self::Off => None,
            Self::Timebase(index) => Some(Timebase::from_index(*index as u32)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGraphSeedFrom {
    Route,
    Tracks(Vec<usize>),
}

impl From<&SeedFrom> for ProjectGraphSeedFrom {
    fn from(value: &SeedFrom) -> Self {
        match value {
            SeedFrom::Route => Self::Route,
            SeedFrom::Tracks(tracks) => Self::Tracks(tracks.clone()),
        }
    }
}

impl From<&ProjectGraphSeedFrom> for SeedFrom {
    fn from(value: &ProjectGraphSeedFrom) -> Self {
        match value {
            ProjectGraphSeedFrom::Route => Self::Route,
            ProjectGraphSeedFrom::Tracks(tracks) => Self::Tracks(tracks.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectGraphNodeIntrinsicOverride {
    pub group: String,
    pub instance: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantize: Option<ProjectGraphQuantizeOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<ProjectGraphRouteOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_from: Option<ProjectGraphSeedFrom>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectGraphNodeParamOverride {
    pub group: String,
    pub instance: usize,
    pub param: String,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectGraphEdgeParamOverride {
    pub group: String,
    pub from: usize,
    pub to: usize,
    pub param: String,
    pub value: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectGraphOverrides {
    pub sequencer_id: u64,
    pub sequencer_name: String,
    #[serde(default)]
    pub node_intrinsics: Vec<ProjectGraphNodeIntrinsicOverride>,
    #[serde(default)]
    pub node_params: Vec<ProjectGraphNodeParamOverride>,
    #[serde(default)]
    pub edge_params: Vec<ProjectGraphEdgeParamOverride>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub resolution: Timebase,
    pub delay_steps: u32,
    pub quantize: Option<Timebase>,
    pub route: Option<usize>,
    pub seed_track_mask: u128,
    pub reduce: Reduce,
    /// Initial value of the `energy` state cell on reset/seed (per-node, spec §3.3).
    pub seed_on_reset: f64,
    /// Semitone offset added to the carried payload note each time this node fires
    /// (Ext 1). Mirrors `neural`'s per-neuron `transpose`; the engine applies it on the
    /// default emit path so the cascade works with a bare threshold `:update`.
    pub transpose: f32,
    /// Threshold cached from params for native max-poly propagation scoring.
    pub threshold: f64,
}

impl Default for GraphNode {
    fn default() -> Self {
        Self {
            resolution: Timebase::Sixteenth,
            delay_steps: 0,
            quantize: None,
            route: None,
            seed_track_mask: 0,
            reduce: Reduce::Sum,
            seed_on_reset: 0.0,
            transpose: 0.0,
            threshold: 1.0,
        }
    }
}

/// The payload that rides along an edge with a spike (Ext 1, spec §3.1): the seed
/// event (note / velocity) that originated from a step trigger and ripples through the
/// net, re-pitched on every hop. A firing node re-emits its incoming payload with its
/// own `transpose` added — the running sum of transposes around a feedback cycle is
/// the Aphex-Twin melodic-cascade behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphPayload {
    /// Accumulated transpose (semitone offset) carried by the signal.
    pub note: f32,
    pub velocity: f32,
}

impl Default for GraphPayload {
    fn default() -> Self {
        Self {
            note: 0.0,
            velocity: 1.0,
        }
    }
}

/// Context passed to the per-node `:update` predicate at one evaluation boundary.
/// Carries only musical/symbolic coordinates and the node's resolved input — never
/// samples (the engine owns all sample math).
#[derive(Clone, Debug)]
pub struct NodeEval {
    pub node_index: usize,
    /// The reduced gather result this boundary (`node-input`). 0 when nothing arrived.
    pub input: f64,
    /// The node's integrated `energy` state *after* folding `input` in this boundary.
    pub energy: f64,
    /// 0-based count of this node's evaluation boundaries since reset (`node-tick`).
    pub tick_index: u64,
    /// Musical position of this boundary in quarter-note beats.
    pub beat: f64,
    /// The payload that arrived this boundary (`node-input-event`), if any.
    pub input_event: Option<GraphPayload>,
    /// Behavioral params for this node instance: prototype defaults plus sparse
    /// per-pattern overrides.
    pub params: HashMap<String, f64>,
}

/// The decision returned by a node's `:update`. v1a: just whether it fired and the
/// energy to retain if it does *not* fire (so the engine can keep accumulation/decay
/// bookkeeping out of the closure while still letting the rule own the threshold).
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeFire {
    pub fired: bool,
    /// If set, commit-time edge-state mutation for incoming edges that actually
    /// triggered this node this eval.
    pub dampen_incoming: Option<f64>,
    /// If set, non-fire recovery factor for all incoming edges.
    pub recover_incoming: Option<f64>,
}

/// One firing resolved to an absolute sample time, tagged with the node index for the
/// deterministic `(sample_time, node_index)` ordering shared with the neural and
/// generator runtimes.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphEmission {
    pub sample_time: u64,
    pub node_index: usize,
    pub event: EmittedAccumulatorEvent,
}

/// A scatter scheduled to deposit along a fired node's out-edges after a delay,
/// counted down in integer boundary steps (mirrors `neural::DelayedPropagation`).
#[derive(Clone, Copy, Debug)]
struct GraphPropagation {
    remaining_steps: u32,
    ready_after_beats: f64,
    payload: GraphPayload,
}

/// A fired node awaiting `max_poly` selection within one boundary.
#[derive(Clone, Debug)]
struct GraphFiringCandidate {
    node_index: usize,
    fire_sample: u64,
    fire_beats: f64,
    dampen_incoming: Option<f64>,
}

/// The runtime for one graph-mode `def-sequencer`. Structure-of-arrays over the
/// shape; sparse edge set; per-node clocks. Reconciled by id like the generator
/// runtime so hot-reloads preserve live state on compatible edits.
#[derive(Clone, Debug)]
pub struct GraphRuntime {
    pub id: u64,
    pub name: String,
    active: bool,
    num_nodes: usize,

    // ── per-node config (SoA) ──
    nodes: Vec<GraphNode>,

    // ── edges ──
    edges: Vec<GraphEdge>,
    edge_default_dampening: Vec<f64>,
    out_edges: Vec<Vec<usize>>, // node -> edge indices originating there
    in_edges: Vec<Vec<usize>>,  // node -> edge indices terminating there (Ext 2)

    // ── sequencer-level engine config ──
    energy_decay: f64,
    reset_interval_beats: f64,
    max_poly_selection: NeuralMaxPolySelection,
    random_state: u64,

    // ── per-node runtime state ──
    node_params: Vec<HashMap<String, f64>>,
    energy: Vec<f64>,
    input_accum: Vec<f64>,
    input_seen: Vec<bool>,
    /// The payload that last arrived at each node (Ext 1), consumed by its next fire.
    source_event: Vec<Option<GraphPayload>>,
    tick_count: Vec<u64>,
    pending: Vec<Vec<GraphPropagation>>,
    /// Edge indices that contributed positive current to each target since that target
    /// last evaluated. Cleared when the target's update/recovery path commits.
    incoming_triggers: Vec<Vec<usize>>,

    // ── clock bookkeeping ──
    last_eval_indices: Vec<u64>,
    last_decay_index: u64,
    next_reset_beat: f64,
}

impl GraphRuntime {
    /// Build a runtime from a fully materialized node field + edge set. `edges` are
    /// taken as-is (the topology generator has already expanded e.g. `all-to-all`).
    pub fn new(
        id: u64,
        name: String,
        nodes: Vec<GraphNode>,
        edges: Vec<GraphEdge>,
        energy_decay: f64,
        reset_interval_beats: f64,
    ) -> Self {
        let node_params = nodes
            .iter()
            .map(|node| {
                let mut params = HashMap::new();
                params.insert("threshold".to_string(), node.threshold);
                params.insert("transpose".to_string(), node.transpose as f64);
                params
            })
            .collect();
        Self::new_with_config(
            id,
            name,
            nodes,
            edges,
            energy_decay,
            reset_interval_beats,
            NeuralMaxPolySelection::Deterministic,
            node_params,
        )
    }

    pub fn new_with_config(
        id: u64,
        name: String,
        nodes: Vec<GraphNode>,
        edges: Vec<GraphEdge>,
        energy_decay: f64,
        reset_interval_beats: f64,
        max_poly_selection: NeuralMaxPolySelection,
        node_params: Vec<HashMap<String, f64>>,
    ) -> Self {
        let num_nodes = nodes.len();
        let mut out_edges = vec![Vec::new(); num_nodes];
        let mut in_edges = vec![Vec::new(); num_nodes];
        for (edge_idx, edge) in edges.iter().enumerate() {
            if edge.from < num_nodes {
                out_edges[edge.from].push(edge_idx);
            }
            if edge.to < num_nodes {
                in_edges[edge.to].push(edge_idx);
            }
        }
        let edge_default_dampening = edges.iter().map(|edge| edge.dampening).collect();
        let mut runtime = Self {
            id,
            name,
            active: true,
            num_nodes,
            nodes,
            edge_default_dampening,
            edges,
            out_edges,
            in_edges,
            energy_decay: energy_decay.clamp(0.0, 1.0),
            reset_interval_beats: reset_interval_beats.max(0.0),
            max_poly_selection,
            random_state: id,
            node_params: normalized_node_params(num_nodes, node_params),
            energy: vec![0.0; num_nodes],
            input_accum: vec![0.0; num_nodes],
            input_seen: vec![false; num_nodes],
            source_event: vec![None; num_nodes],
            tick_count: vec![0; num_nodes],
            pending: vec![Vec::new(); num_nodes],
            incoming_triggers: vec![Vec::new(); num_nodes],
            last_eval_indices: vec![0; num_nodes],
            last_decay_index: 0,
            next_reset_beat: 0.0,
        };
        runtime.reset(0.0);
        runtime
    }

    pub fn is_empty(&self) -> bool {
        self.num_nodes == 0
    }

    pub fn num_nodes(&self) -> usize {
        self.num_nodes
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Read a node's energy (telemetry / tests).
    pub fn energy(&self, node_index: usize) -> f64 {
        self.energy.get(node_index).copied().unwrap_or(0.0)
    }

    pub fn edge_dampening(&self, edge_index: usize) -> Option<f64> {
        self.edges.get(edge_index).map(|edge| edge.dampening)
    }

    /// Reset all runtime state: clocks realigned to `total_beats`, energy zeroed then
    /// seeded from `seed_on_reset`, pending queues cleared, decay/reset indices
    /// recomputed (mirrors `neural::reset_state`).
    pub fn reset(&mut self, total_beats: f64) {
        for idx in 0..self.num_nodes {
            self.energy[idx] = self.nodes[idx].seed_on_reset;
            self.input_accum[idx] = 0.0;
            self.input_seen[idx] = false;
            self.source_event[idx] = None;
            self.tick_count[idx] = 0;
            self.pending[idx].clear();
            self.incoming_triggers[idx].clear();
            let step_beats = self.node_step_beats(idx);
            self.last_eval_indices[idx] = (total_beats / step_beats).floor() as u64;
        }
        for (edge, default_dampening) in self.edges.iter_mut().zip(&self.edge_default_dampening) {
            edge.dampening = *default_dampening;
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        self.next_reset_beat = if self.reset_interval_beats > 0.0 {
            ((total_beats / self.reset_interval_beats).floor() + 1.0) * self.reset_interval_beats
        } else {
            0.0
        };
    }

    /// Realign every node's clock to the current transport position without firing
    /// the boundaries in between (used on resync / pattern jump).
    pub fn realign(&mut self, total_beats: f64) {
        for idx in 0..self.num_nodes {
            let step_beats = self.node_step_beats(idx);
            self.last_eval_indices[idx] = (total_beats / step_beats).floor() as u64;
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        if self.reset_interval_beats > 0.0 {
            self.next_reset_beat = ((total_beats / self.reset_interval_beats).floor() + 1.0)
                * self.reset_interval_beats;
        }
    }

    /// Inject a fire into every node whose resolved `seed-from` includes `track`,
    /// respecting that node's delay (spec §4). Mechanically identical to a firing's
    /// scatter: it pushes a delayed propagation onto the seeded node carrying the step
    /// event's `payload`, so the node scatters that payload along its out-edges after
    /// `delay_steps` (Ext 1 — the seed note then ripples through the net).
    pub fn seed(&mut self, track: usize, seed_beats: f64, payload: GraphPayload) {
        if !self.active {
            return;
        }
        let Some(bit) = seed_track_bit(track) else {
            return;
        };
        for idx in 0..self.num_nodes {
            if self.nodes[idx].seed_track_mask & bit != 0 {
                self.push_propagation(idx, seed_beats, payload);
            }
        }
    }

    /// Drive the whole graph across `(start_beats, end_beats]`, appending firings to
    /// `out` resolved to absolute sample times and sorted by `(sample_time,
    /// node_index)`. `update_fn` is the per-node fire predicate; it sees the node's
    /// integrated energy and returns whether the node fires.
    pub fn process_block<F>(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        max_poly: u32,
        mut update_fn: F,
        out: &mut Vec<GraphEmission>,
    ) where
        F: FnMut(&NodeEval) -> NodeFire,
    {
        if !self.active || self.num_nodes == 0 || end_beats <= start_beats {
            return;
        }
        let appended_from = out.len();
        while let Some(boundary_beats) = self.next_eval_boundary(start_beats, end_beats) {
            // Periodic reset lands on a boundary: decay up to it, then reset.
            if self.next_reset_beat > 0.0 && self.next_reset_beat <= boundary_beats + 1e-9 {
                self.apply_energy_decay(self.next_reset_beat);
                self.reset(self.next_reset_beat);
                continue;
            }

            let sample_offset = ((boundary_beats - start_beats) * samples_per_quarter)
                .round()
                .max(0.0) as u64;
            let sample_time = block_start_sample.saturating_add(sample_offset);

            // Which nodes hit their own grid boundary here?
            let mut due = vec![false; self.num_nodes];
            for idx in 0..self.num_nodes {
                let step_beats = self.node_step_beats(idx);
                let next_index = self.last_eval_indices[idx].saturating_add(1);
                let node_boundary = next_index as f64 * step_beats;
                due[idx] = (node_boundary - boundary_beats).abs() <= 1e-9;
            }

            // ── Phase 1: deposit. Scatter every due node's ready propagations into
            // targets' input accumulators. Running this fully before any fire decision
            // makes the boundary order-independent (the double-buffer of spec §5).
            for idx in 0..self.num_nodes {
                if due[idx] {
                    self.last_eval_indices[idx] = self.last_eval_indices[idx].saturating_add(1);
                    self.deposit_ready_propagations(idx, boundary_beats);
                }
            }

            // ── Phase 2: fire. Integrate gather into energy, run the rule, collect
            // candidates (read-only — energy reset is deferred to commit so max_poly
            // can reject without consuming the node).
            let mut candidates: Vec<GraphFiringCandidate> = Vec::new();
            let mut decisions = vec![NodeFire::default(); self.num_nodes];
            for idx in 0..self.num_nodes {
                if !due[idx] {
                    continue;
                }
                let input = if self.input_seen[idx] {
                    self.input_accum[idx]
                } else {
                    0.0
                };
                self.input_accum[idx] = 0.0;
                self.input_seen[idx] = false;
                self.energy[idx] += input;
                let eval = NodeEval {
                    node_index: idx,
                    input,
                    energy: self.energy[idx],
                    tick_index: self.tick_count[idx],
                    beat: boundary_beats,
                    input_event: self.source_event[idx],
                    params: self.node_params[idx].clone(),
                };
                self.tick_count[idx] = self.tick_count[idx].saturating_add(1);
                let decision = update_fn(&eval);
                decisions[idx] = decision;
                if decision.fired {
                    let (fire_sample, fire_beats) = self.quantized_fire_timing(
                        idx,
                        boundary_beats,
                        sample_time,
                        samples_per_quarter,
                    );
                    candidates.push(GraphFiringCandidate {
                        node_index: idx,
                        fire_sample,
                        fire_beats,
                        dampen_incoming: decision.dampen_incoming,
                    });
                }
            }

            // ── max_poly selection (deterministic: earliest sample, then index). ──
            candidates.sort_by_key(|c| (c.fire_sample, c.node_index));
            let accepted = self.max_poly_accept(&candidates, max_poly);
            let mut rejected = vec![false; self.num_nodes];
            let mut accepted_node = vec![false; self.num_nodes];
            for (cand_idx, candidate) in candidates.iter().enumerate() {
                if accepted[cand_idx] {
                    accepted_node[candidate.node_index] = true;
                    self.commit_firing(candidate, out);
                } else {
                    rejected[candidate.node_index] = true;
                }
            }
            for idx in 0..self.num_nodes {
                if !due[idx] {
                    continue;
                }
                if accepted_node[idx] {
                    continue;
                }
                if rejected[idx] {
                    self.drop_firing(idx);
                } else {
                    if let Some(factor) = decisions[idx].recover_incoming {
                        self.recover_incoming(idx, factor);
                    }
                    self.clear_incoming_triggers(idx);
                }
            }

            self.apply_energy_decay(boundary_beats);
        }

        // Resets and decay between the last boundary and the block end.
        while self.next_reset_beat > 0.0 && self.next_reset_beat <= end_beats + 1e-9 {
            self.apply_energy_decay(self.next_reset_beat);
            self.reset(self.next_reset_beat);
        }
        self.apply_energy_decay(end_beats);

        out[appended_from..].sort_by_key(|e| (e.sample_time, e.node_index));
    }

    // ── internals ──

    fn node_step_beats(&self, node_index: usize) -> f64 {
        self.nodes[node_index]
            .resolution
            .step_beats(GRAPH_RESOLUTION_REF_STEPS)
            .max(1e-9)
    }

    /// Next boundary across the union of all node grids in `(start, end]`, advancing
    /// per-node eval indices past already-passed boundaries (mirrors
    /// `neural::next_eval_boundary`).
    fn next_eval_boundary(&mut self, start_beats: f64, end_beats: f64) -> Option<f64> {
        let mut next: Option<f64> = None;
        for idx in 0..self.num_nodes {
            let step_beats = self.node_step_beats(idx);
            if let Some(boundary) = next_grid_boundary(
                &mut self.last_eval_indices[idx],
                step_beats,
                start_beats,
                end_beats,
            ) {
                next = Some(match next {
                    Some(cur) => cur.min(boundary),
                    None => boundary,
                });
            }
        }
        next
    }

    fn finest_decay_index(&self, total_beats: f64) -> u64 {
        let finest = (0..self.num_nodes)
            .map(|idx| self.node_step_beats(idx))
            .filter(|beats| *beats > 0.0)
            .fold(DEFAULT_DECAY_STEP_BEATS, f64::min);
        (total_beats / finest.max(1e-9)).floor() as u64
    }

    fn apply_energy_decay(&mut self, end_beats: f64) {
        let decay_index = self.finest_decay_index(end_beats);
        while self.last_decay_index < decay_index {
            for idx in 0..self.num_nodes {
                self.energy[idx] *= self.energy_decay;
            }
            self.last_decay_index += 1;
        }
    }

    /// Decrement this node's pending scatters, then for each that became ready deposit
    /// `gather()` along its out-edges into the targets' input accumulators, carrying the
    /// scatter's payload into each target's `source_event` (Ext 1).
    fn deposit_ready_propagations(&mut self, node_index: usize, boundary_beats: f64) {
        let mut ready: Vec<GraphPayload> = Vec::new();
        let mut kept = Vec::with_capacity(self.pending[node_index].len());
        for mut prop in std::mem::take(&mut self.pending[node_index]) {
            if boundary_beats > prop.ready_after_beats + 1e-9 {
                prop.remaining_steps = prop.remaining_steps.saturating_sub(1);
            }
            if prop.remaining_steps == 0 {
                ready.push(prop.payload);
            } else {
                kept.push(prop);
            }
        }
        self.pending[node_index] = kept;
        for payload in ready {
            for &edge_idx in &self.out_edges[node_index] {
                let edge = self.edges[edge_idx];
                let amount = edge.gather();
                if amount <= 0.0 {
                    continue;
                }
                let target = edge.to;
                let reduce = self.nodes[target].reduce;
                let first = !self.input_seen[target];
                self.input_accum[target] = reduce.fold(self.input_accum[target], amount, first);
                self.input_seen[target] = true;
                self.source_event[target] = Some(payload);
                if !self.incoming_triggers[target].contains(&edge_idx) {
                    self.incoming_triggers[target].push(edge_idx);
                }
            }
        }
    }

    /// Snap a fire to the node's `quantize` grid if set, else pass through (mirrors
    /// `neural::quantized_fire_timing`).
    fn quantized_fire_timing(
        &self,
        node_index: usize,
        boundary_beats: f64,
        sample_time: u64,
        samples_per_quarter: f64,
    ) -> (u64, f64) {
        let Some(timebase) = self.nodes[node_index].quantize else {
            return (sample_time, boundary_beats);
        };
        let grid_beats = timebase.step_beats(GRAPH_RESOLUTION_REF_STEPS).max(1e-9);
        let grid_position = boundary_beats / grid_beats;
        let nearest = grid_position.round();
        let quantized_grid = if (grid_position - nearest).abs() <= 1e-9 {
            nearest
        } else {
            grid_position.ceil()
        };
        let quantized_beats = (quantized_grid * grid_beats).max(boundary_beats);
        let offset_samples = ((quantized_beats - boundary_beats) * samples_per_quarter)
            .round()
            .max(0.0) as u64;
        (sample_time.saturating_add(offset_samples), quantized_beats)
    }

    /// Emit the firing, reset the node's energy, and schedule its delayed scatter.
    ///
    /// Ext 1: the firing re-emits the payload that arrived (`source_event`) with this
    /// node's `transpose` added — so the seed note re-pitches on every hop — and the
    /// same re-pitched payload rides the outgoing scatter, accumulating around feedback
    /// loops. Mirrors `neural::firing_candidate` (clone source event, add transpose).
    fn commit_firing(&mut self, candidate: &GraphFiringCandidate, out: &mut Vec<GraphEmission>) {
        let node_index = candidate.node_index;
        let incoming = self.source_event[node_index].unwrap_or_default();
        let payload = GraphPayload {
            note: incoming.note + self.nodes[node_index].transpose,
            velocity: incoming.velocity,
        };
        let mut event = EmittedAccumulatorEvent {
            offset_beats: 0.0,
            track: self.nodes[node_index].route,
            resolved: default_resolved(),
            chord: Vec::new(),
            chord_durations: Vec::new(),
            chord_step_transpose: 0.0,
            effect_params: Vec::new(),
            instrument_params: Vec::new(),
        };
        event.resolved.transpose = payload.note;
        event.resolved.velocity = payload.velocity;
        out.push(GraphEmission {
            sample_time: candidate.fire_sample,
            node_index,
            event,
        });
        self.energy[node_index] = 0.0;
        if let Some(amount) = candidate.dampen_incoming {
            self.dampen_incoming(node_index, amount);
        }
        self.clear_incoming_triggers(node_index);
        self.push_propagation(node_index, candidate.fire_beats, payload);
    }

    fn drop_firing(&mut self, node_index: usize) {
        self.energy[node_index] = 0.0;
        self.clear_incoming_triggers(node_index);
    }

    fn dampen_incoming(&mut self, node_index: usize, amount: f64) {
        let amount = amount.clamp(0.0, 1.0);
        for edge_idx in self.incoming_triggers[node_index].iter().copied() {
            if let Some(edge) = self.edges.get_mut(edge_idx) {
                edge.dampening = (edge.dampening + amount).min(1.0);
            }
        }
    }

    fn recover_incoming(&mut self, node_index: usize, factor: f64) {
        let factor = factor.clamp(0.0, 1.0);
        for edge_idx in self.in_edges[node_index].iter().copied() {
            if let Some(edge) = self.edges.get_mut(edge_idx) {
                edge.dampening *= factor;
            }
        }
    }

    fn clear_incoming_triggers(&mut self, node_index: usize) {
        self.incoming_triggers[node_index].clear();
    }

    fn max_poly_accept(&mut self, candidates: &[GraphFiringCandidate], max_poly: u32) -> Vec<bool> {
        let mut accepted = vec![true; candidates.len()];
        if max_poly == 0 || candidates.len() <= max_poly as usize {
            return accepted;
        }
        accepted.fill(false);
        let accepted_count = max_poly as usize;
        match self.max_poly_selection {
            NeuralMaxPolySelection::Deterministic => {
                for slot in accepted.iter_mut().take(accepted_count) {
                    *slot = true;
                }
            }
            NeuralMaxPolySelection::Propagation => {
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                indices.sort_by(|left, right| {
                    let left_candidate = &candidates[*left];
                    let right_candidate = &candidates[*right];
                    self.propagation_selection_score(right_candidate.node_index)
                        .total_cmp(&self.propagation_selection_score(left_candidate.node_index))
                        .then(left_candidate.fire_sample.cmp(&right_candidate.fire_sample))
                        .then(left_candidate.node_index.cmp(&right_candidate.node_index))
                });
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidate_idx] = true;
                }
            }
            NeuralMaxPolySelection::Random => {
                let mut indices = (0..candidates.len()).collect::<Vec<_>>();
                for pos in 0..accepted_count {
                    let selected = pos + self.random_index(indices.len() - pos);
                    indices.swap(pos, selected);
                }
                for candidate_idx in indices.into_iter().take(accepted_count) {
                    accepted[candidate_idx] = true;
                }
            }
        }
        accepted
    }

    fn propagation_selection_score(&self, source: usize) -> f64 {
        let mut score = 0.0;
        for &edge_idx in &self.out_edges[source] {
            let edge = self.edges[edge_idx];
            let amount = edge.gather();
            if amount <= 0.0 || edge.to >= self.num_nodes {
                continue;
            }
            let threshold = self.nodes[edge.to].threshold.max(1e-6);
            let projected = self.energy[edge.to] + amount;
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

    fn push_propagation(
        &mut self,
        node_index: usize,
        ready_after_beats: f64,
        payload: GraphPayload,
    ) {
        let remaining = self.nodes[node_index].delay_steps.max(1);
        self.pending[node_index].push(GraphPropagation {
            remaining_steps: remaining,
            ready_after_beats,
            payload,
        });
    }
}

fn normalized_node_params(
    num_nodes: usize,
    mut node_params: Vec<HashMap<String, f64>>,
) -> Vec<HashMap<String, f64>> {
    node_params.resize_with(num_nodes, HashMap::new);
    node_params
}

fn splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// Bit for `track` in a `u128` seed mask, or `None` if out of range (mirrors
/// `neural::seed_track_bit`).
fn seed_track_bit(track: usize) -> Option<u128> {
    if track < 128 {
        Some(1u128 << track)
    } else {
        None
    }
}

/// Build a `seed-from` mask from a list of input track indices.
pub fn seed_track_mask(tracks: &[usize]) -> u128 {
    tracks
        .iter()
        .filter_map(|&t| seed_track_bit(t))
        .fold(0, |m, b| m | b)
}

// ───────────────────────────── Authored manifest ─────────────────────────────
//
// The *manifest* is the parsed, still-symbolic form of a graph-mode `def-sequencer`
// (shape + node prototype + edge sets + sequencer-level config). It is produced from
// the published lisp body (see `lisp_effect::parse_graph_manifest`) and `materialize`d
// into the SoA node field + edge set that drives [`GraphRuntime`]. Keeping it a plain
// data type (no lisp dependency) lets it be built and tested directly.

/// The node field. Materializes the node count + addressing (spec §2.4).
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeSpec {
    /// `R*C` nodes addressed `(r,c)`, flat index `r*C+c`.
    Grid { rows: usize, cols: usize },
    /// `N` nodes, index `0..N`.
    Line(usize),
    /// Like line, with wrap semantics for neighbor topologies.
    Ring(usize),
}

impl ShapeSpec {
    pub fn num_nodes(&self) -> usize {
        match *self {
            ShapeSpec::Grid { rows, cols } => rows * cols,
            ShapeSpec::Line(n) | ShapeSpec::Ring(n) => n,
        }
    }
}

/// Which input track(s) seed a node (spec §4). Defaults to following `:route`.
#[derive(Clone, Debug, PartialEq)]
pub enum SeedFrom {
    /// Follow `:route` — "the track I write to is the track I listen to".
    Route,
    /// Explicit input track set (decouples input from output).
    Tracks(Vec<usize>),
}

impl Default for SeedFrom {
    fn default() -> Self {
        SeedFrom::Route
    }
}

/// A behavioral param descriptor (`:params` entry, spec §2.2/§2.3). The engine only
/// stores the default; the lisp `:update`/`:gather` reads it. v1a tracks the numeric
/// default and range; richer kinds (enum/string) are deferred.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// True for `:int` params (clamped/rounded), false for `:float`.
    pub is_int: bool,
}

/// Elapsed-time decay applied to a leaky state cell (`:leak (per-step :energy-decay)`).
#[derive(Clone, Debug, PartialEq)]
pub enum LeakSpec {
    /// Decay by the sequencer-level `:energy-decay` once per finest grid step.
    PerStepEnergyDecay,
    /// Decay by an explicit factor once per finest grid step.
    PerStep(f64),
}

/// A runtime state cell (`:state` entry). v1a's only meaningful cell is `energy`.
#[derive(Clone, Debug, PartialEq)]
pub struct StateSpec {
    pub name: String,
    pub leak: Option<LeakSpec>,
}

/// A node prototype (`def-node`). Intrinsics are engine-read defaults; `params`/`state`
/// are author-defined; `update_source` is the serialized `:update` body the scheduler
/// compiles once and invokes per node-event.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeProto {
    pub name: String,
    pub resolution: Timebase,
    pub delay_steps: u32,
    pub quantize: Option<Timebase>,
    pub route: Option<usize>,
    pub seed_from: SeedFrom,
    pub reduce: Reduce,
    pub params: Vec<ParamSpec>,
    pub state: Vec<StateSpec>,
    pub update_source: Option<String>,
}

impl Default for NodeProto {
    fn default() -> Self {
        Self {
            name: String::new(),
            resolution: Timebase::Sixteenth,
            delay_steps: 0,
            quantize: None,
            route: None,
            seed_from: SeedFrom::Route,
            reduce: Reduce::Sum,
            params: Vec::new(),
            state: Vec::new(),
            update_source: None,
        }
    }
}

impl NodeProto {
    pub fn param_default(&self, name: &str) -> Option<f64> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.default)
    }
}

/// The adjacency a topology generator expands at build time (spec §2.5). v1 ships only
/// `all-to-all`.
#[derive(Clone, Debug, PartialEq)]
pub enum Topology {
    AllToAll,
}

/// An edge set (`edges`): source/target prototype names, topology, the `:gather`
/// kernel source, and edge param defaults (`weight`, `dampening`).
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeSetSpec {
    pub from: String,
    pub to: String,
    pub topology: Topology,
    pub gather_source: Option<String>,
    pub params: Vec<ParamSpec>,
}

impl EdgeSetSpec {
    fn param_default(&self, name: &str) -> f64 {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.default)
            .unwrap_or(0.0)
    }
}

/// A fully parsed graph-mode `def-sequencer`. v1: exactly one prototype applying to
/// every cell of the shape.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphManifest {
    pub id: u64,
    pub name: String,
    pub shape: ShapeSpec,
    pub energy_decay: f64,
    pub reset_every_beats: f64,
    pub seed_on_reset: f64,
    pub max_poly: u32,
    pub max_poly_selection: NeuralMaxPolySelection,
    pub node: NodeProto,
    pub edge_sets: Vec<EdgeSetSpec>,
}

impl GraphManifest {
    /// True if `other` materializes to the same node field + edge set as `self`, i.e.
    /// a hot-reload can keep the live runtime's energy/pending state. Ignores the
    /// `:update`/`:gather` *code* and behavioral `:params` (re-read each eval), and the
    /// node `:state` declarations — only materialization-relevant structure matters.
    pub fn structurally_compatible(&self, other: &GraphManifest) -> bool {
        let intrinsics = |n: &NodeProto| {
            (
                n.resolution,
                n.delay_steps,
                n.quantize,
                n.route,
                n.seed_from.clone(),
                n.reduce,
            )
        };
        let edge_shape = |sets: &[EdgeSetSpec]| {
            sets.iter()
                .map(|s| {
                    (
                        s.from.clone(),
                        s.to.clone(),
                        s.topology.clone(),
                        s.param_default("weight"),
                        s.param_default("dampening"),
                    )
                })
                .collect::<Vec<_>>()
        };
        self.shape == other.shape
            && self.energy_decay == other.energy_decay
            && self.reset_every_beats == other.reset_every_beats
            && self.seed_on_reset == other.seed_on_reset
            && self.max_poly_selection == other.max_poly_selection
            && intrinsics(&self.node) == intrinsics(&other.node)
            && edge_shape(&self.edge_sets) == edge_shape(&other.edge_sets)
    }

    /// Expand the manifest into the node field + edge set for [`GraphRuntime::new`],
    /// applying prototype defaults to every instance. Per-instance plocks (weights,
    /// per-node delay edits) are layered on later by the serialization store; this is
    /// the zero-override baseline.
    pub fn materialize(&self) -> GraphRuntime {
        self.materialize_with_overrides(None)
    }

    pub fn materialize_with_overrides(
        &self,
        overrides: Option<&ProjectGraphOverrides>,
    ) -> GraphRuntime {
        let num_nodes = self.shape.num_nodes();
        let proto_params = self
            .node
            .params
            .iter()
            .map(|param| (param.name.clone(), param.default))
            .collect::<HashMap<_, _>>();
        let mut nodes = Vec::with_capacity(num_nodes);
        let mut node_params = vec![proto_params; num_nodes];
        for idx in 0..num_nodes {
            let mut route = self.node.route;
            let mut seed_from = self.node.seed_from.clone();
            let mut resolution = self.node.resolution;
            let mut delay_steps = self.node.delay_steps;
            let mut quantize = self.node.quantize;
            if let Some(overrides) = overrides {
                for intrinsic in overrides.node_intrinsics.iter().filter(|intrinsic| {
                    intrinsic.group == self.node.name && intrinsic.instance == idx
                }) {
                    if let Some(value) = intrinsic.resolution {
                        resolution = Timebase::from_index(value as u32);
                    }
                    if let Some(value) = intrinsic.delay_steps {
                        delay_steps = value;
                    }
                    if let Some(value) = &intrinsic.quantize {
                        quantize = value.to_quantize();
                    }
                    if let Some(value) = &intrinsic.route {
                        route = value.to_route();
                    }
                    if let Some(value) = &intrinsic.seed_from {
                        seed_from = SeedFrom::from(value);
                    }
                }
                for param in overrides
                    .node_params
                    .iter()
                    .filter(|param| param.group == self.node.name && param.instance == idx)
                {
                    node_params[idx].insert(param.param.clone(), param.value);
                }
            }
            let seed_track_mask = match &seed_from {
                SeedFrom::Route => route.map(|t| seed_track_mask(&[t])).unwrap_or(0),
                SeedFrom::Tracks(tracks) => seed_track_mask(tracks),
            };
            nodes.push(GraphNode {
                resolution,
                delay_steps,
                quantize,
                route,
                seed_track_mask,
                reduce: self.node.reduce,
                seed_on_reset: self.seed_on_reset,
                transpose: node_params[idx].get("transpose").copied().unwrap_or(0.0) as f32,
                threshold: node_params[idx].get("threshold").copied().unwrap_or(1.0),
            });
        }

        let mut edges = Vec::new();
        for set in &self.edge_sets {
            let edge_group = edge_set_group_id(set);
            let weight = set.param_default("weight");
            let dampening = set.param_default("dampening");
            match set.topology {
                Topology::AllToAll => {
                    for from in 0..num_nodes {
                        for to in 0..num_nodes {
                            let mut edge = GraphEdge {
                                from,
                                to,
                                weight,
                                dampening,
                                delay_steps: 0,
                            };
                            if let Some(overrides) = overrides {
                                for param in overrides.edge_params.iter().filter(|param| {
                                    param.group == edge_group
                                        && param.from == from
                                        && param.to == to
                                }) {
                                    match param.param.as_str() {
                                        "weight" => edge.weight = param.value,
                                        "dampening" => edge.dampening = param.value.clamp(0.0, 1.0),
                                        _ => {}
                                    }
                                }
                            }
                            edges.push(edge);
                        }
                    }
                }
            }
        }

        GraphRuntime::new_with_config(
            self.id,
            self.name.clone(),
            nodes,
            edges,
            self.energy_decay,
            self.reset_every_beats,
            self.max_poly_selection,
            node_params,
        )
    }
}

pub fn edge_set_group_id(set: &EdgeSetSpec) -> String {
    format!("{}->{}", set.from, set.to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node that fires when its integrated energy reaches `threshold`.
    fn threshold_update(thresholds: Vec<f64>) -> impl FnMut(&NodeEval) -> NodeFire {
        move |eval: &NodeEval| NodeFire {
            fired: eval.energy >= thresholds[eval.node_index],
            ..NodeFire::default()
        }
    }

    fn node(resolution: Timebase) -> GraphNode {
        GraphNode {
            resolution,
            ..GraphNode::default()
        }
    }

    fn run(
        runtime: &mut GraphRuntime,
        end_beats: f64,
        max_poly: u32,
        thresholds: Vec<f64>,
    ) -> Vec<GraphEmission> {
        let mut out = Vec::new();
        let mut update = threshold_update(thresholds);
        runtime.process_block(0.0, end_beats, 0, 48_000.0, max_poly, &mut update, &mut out);
        out
    }

    fn always_fire_with_dampen(amount: f64) -> impl FnMut(&NodeEval) -> NodeFire {
        move |eval| NodeFire {
            fired: eval.input > 0.0,
            dampen_incoming: Some(amount),
            ..NodeFire::default()
        }
    }

    #[test]
    fn empty_graph_emits_nothing() {
        let mut runtime = GraphRuntime::new(1, "g".into(), Vec::new(), Vec::new(), 0.9, 0.0);
        assert!(runtime.is_empty());
        let out = run(&mut runtime, 4.0, 0, Vec::new());
        assert!(out.is_empty());
    }

    #[test]
    fn seeded_node_fires_after_propagation_delay() {
        // Two nodes, 1/4 grid. A propagation onto node0 (delay floored to 1 step)
        // becomes ready at the first boundary (beat 1), scatters weight 1 along
        // edge 0->1, and node1 fires at beat 1 (sample 48000). node0 has no energy of
        // its own, so it never fires.
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 0.5, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 4.0, 0, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 48_000);
    }

    #[test]
    fn self_loop_produces_stable_spiking() {
        // One node with a self-edge weight 1, threshold 1, no decay: each fire scatters
        // to itself after the 1-step delay floor, so it spikes on every quarter boundary.
        let nodes = vec![node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 0, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 8.0, 0, vec![1.0]);
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(
            samples,
            vec![48_000, 96_000, 144_000, 192_000, 240_000, 288_000, 336_000, 384_000]
        ); // beats 1..8
    }

    #[test]
    fn coincident_firings_order_by_node_index() {
        // Two independent self-looping nodes seeded together fire on the same boundary;
        // emissions order by node index.
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 0, 1.0), GraphEdge::new(1, 1, 1.0)];
        let mut runtime = GraphRuntime::new(7, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());
        runtime.push_propagation(1, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].sample_time, out[1].sample_time);
        assert_eq!(out[0].node_index, 0);
        assert_eq!(out[1].node_index, 1);
    }

    #[test]
    fn max_poly_caps_coincident_firings() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 0, 1.0), GraphEdge::new(1, 1, 1.0)];
        let mut runtime = GraphRuntime::new(7, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());
        runtime.push_propagation(1, 0.0, GraphPayload::default());

        // max_poly 1: only the lowest-index coincident firing survives this boundary.
        let out = run(&mut runtime, 1.0, 1, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 0);
    }

    #[test]
    fn seed_mask_routes_seed_to_subscribed_node() {
        let mut n0 = node(Timebase::Quarter);
        n0.seed_track_mask = seed_track_mask(&[3]);
        let nodes = vec![n0, node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 0.5, 0.0);

        runtime.seed(3, 0.0, GraphPayload::default()); // node0 subscribes to track 3
        runtime.seed(5, 0.0, GraphPayload::default()); // nobody subscribes to track 5

        let out = run(&mut runtime, 4.0, 0, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 48_000);
    }

    #[test]
    fn reset_every_reseeds_energy() {
        // node seeded with energy 2 (>= threshold 1) fires at the first eval after each
        // re-seed, then has no input until the next reset. reset_interval = 4 beats.
        let mut n0 = node(Timebase::Quarter);
        n0.seed_on_reset = 2.0;
        let nodes = vec![n0];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, Vec::new(), 1.0, 4.0);
        let out = run(&mut runtime, 8.0, 0, vec![1.0]);
        // Construction-seed fires at beat 1; reset at beat 4 re-seeds, fires at beat 5.
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(samples, vec![48_000, 240_000]);
        assert!(out.iter().all(|e| e.node_index == 0));
    }

    #[test]
    fn manifest_materializes_all_to_all_and_spikes() {
        // A 1-node line, self all-to-all weight 1: materialized graph self-loops and
        // spikes every quarter once seeded — proving manifest -> runtime -> emit.
        let manifest = GraphManifest {
            id: 42,
            name: "neural".into(),
            shape: ShapeSpec::Line(1),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: -1.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        };
        let mut runtime = manifest.materialize();
        assert_eq!(runtime.num_nodes(), 1);
        runtime.push_propagation(0, 0.0, GraphPayload::default());
        let out = run(&mut runtime, 4.0, 0, vec![1.0]);
        let samples: Vec<u64> = out.iter().map(|e| e.sample_time).collect();
        assert_eq!(samples, vec![48_000, 96_000, 144_000, 192_000]);
    }

    #[test]
    fn seed_payload_repitches_on_each_hop() {
        // Chain 0->1->2 (weight 1), each node transposes +5. A payload (note 10) seeded
        // into node0 scatters down the chain; each firing re-emits the incoming note
        // plus its own transpose, so notes accumulate: node1 -> 15, node2 -> 20.
        let mut n = node(Timebase::Quarter);
        n.transpose = 5.0;
        let nodes = vec![n.clone(), n.clone(), n];
        let edges = vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(1, 2, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(
            0,
            0.0,
            GraphPayload {
                note: 10.0,
                velocity: 1.0,
            },
        );

        let out = run(&mut runtime, 4.0, 0, vec![1.0, 1.0, 1.0]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 48_000);
        assert_eq!(out[0].event.resolved.transpose, 15.0);
        assert_eq!(out[1].node_index, 2);
        assert_eq!(out[1].sample_time, 96_000);
        assert_eq!(out[1].event.resolved.transpose, 20.0);
    }

    #[test]
    fn seed_via_mask_carries_payload_downstream() {
        // node0 subscribes to track 2; edge 0->1 (weight 1); node1 transposes +7.
        // A seed on track 2 (note 3, vel 0.5) scatters from node0 to node1, which fires
        // emitting note 3+7=10 carrying the seed velocity.
        let mut n0 = node(Timebase::Quarter);
        n0.seed_track_mask = seed_track_mask(&[2]);
        let mut n1 = node(Timebase::Quarter);
        n1.transpose = 7.0;
        let nodes = vec![n0, n1];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.seed(
            2,
            0.0,
            GraphPayload {
                note: 3.0,
                velocity: 0.5,
            },
        );

        let out = run(&mut runtime, 4.0, 0, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.transpose, 10.0);
        assert_eq!(out[0].event.resolved.velocity, 0.5);
    }

    #[test]
    fn dampen_incoming_mutates_only_triggered_edges() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let edges = vec![GraphEdge::new(0, 2, 1.0), GraphEdge::new(1, 2, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        let mut update = always_fire_with_dampen(0.5);
        runtime.process_block(0.0, 1.0, 0, 48_000.0, 0, &mut update, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 2);
        assert_eq!(runtime.edges[0].dampening, 0.5);
        assert_eq!(
            runtime.edges[1].dampening, 0.0,
            "untriggered incoming edge must not be dampened"
        );
    }

    #[test]
    fn dampening_subtracts_from_weight_on_next_gather() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        let mut update = always_fire_with_dampen(0.5);
        runtime.process_block(0.0, 1.0, 0, 48_000.0, 0, &mut update, &mut out);
        assert_eq!(runtime.edges[0].dampening, 0.5);

        runtime.push_propagation(0, 1.0, GraphPayload::default());
        let mut observed_inputs = Vec::new();
        runtime.process_block(
            1.0,
            2.0,
            48_000,
            48_000.0,
            0,
            |eval| {
                if eval.node_index == 1 {
                    observed_inputs.push(eval.input);
                }
                NodeFire::default()
            },
            &mut out,
        );
        assert_eq!(observed_inputs, vec![0.5]);
    }

    #[test]
    fn recover_incoming_applies_to_non_firing_due_node() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut edge = GraphEdge::new(0, 1, 1.0);
        edge.dampening = 0.8;
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, vec![edge], 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |_eval| NodeFire {
                fired: false,
                recover_incoming: Some(0.5),
                ..NodeFire::default()
            },
            &mut out,
        );
        assert!(out.is_empty());
        assert!((runtime.edges[0].dampening - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn reset_restores_runtime_edge_dampening_to_default() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut edge = GraphEdge::new(0, 1, 1.0);
        edge.dampening = 0.2;
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, vec![edge], 1.0, 0.0);
        runtime.edges[0].dampening = 0.9;

        runtime.reset(0.0);

        assert_eq!(runtime.edges[0].dampening, 0.2);
    }

    #[test]
    fn rejected_max_poly_candidate_does_not_dampen() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let edges = vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(0, 2, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        let mut update = always_fire_with_dampen(0.5);
        runtime.process_block(0.0, 1.0, 0, 48_000.0, 1, &mut update, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(runtime.edges[0].dampening, 0.5);
        assert_eq!(runtime.edges[1].dampening, 0.0);
    }

    #[test]
    fn max_poly_propagation_selection_uses_effective_downstream_score() {
        let mut n0 = node(Timebase::Quarter);
        n0.seed_on_reset = 1.0;
        let mut n1 = node(Timebase::Quarter);
        n1.seed_on_reset = 1.0;
        let n2 = node(Timebase::Quarter);
        let mut runtime = GraphRuntime::new_with_config(
            1,
            "g".into(),
            vec![n0, n1, n2],
            vec![GraphEdge::new(0, 2, 0.1), GraphEdge::new(1, 2, 1.0)],
            1.0,
            0.0,
            NeuralMaxPolySelection::Propagation,
            Vec::new(),
        );
        let out = run(&mut runtime, 1.0, 1, vec![1.0, 1.0, 99.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
    }

    #[test]
    fn max_poly_random_selection_can_accept_non_first_candidate() {
        let mut nodes = Vec::new();
        for _ in 0..4 {
            let mut n = node(Timebase::Quarter);
            n.seed_on_reset = 1.0;
            nodes.push(n);
        }
        let mut runtime = GraphRuntime::new_with_config(
            0,
            "g".into(),
            nodes,
            Vec::new(),
            1.0,
            0.0,
            NeuralMaxPolySelection::Random,
            Vec::new(),
        );
        let out = run(&mut runtime, 1.0, 1, vec![1.0; 4]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 3);
    }

    #[test]
    fn seed_from_can_differ_from_route() {
        let mut n0 = node(Timebase::Quarter);
        n0.seed_track_mask = seed_track_mask(&[0]);
        n0.route = Some(2);
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![n0],
            vec![GraphEdge::new(0, 0, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());
        runtime.seed(2, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 1.0, 0, vec![1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.track, Some(2));
    }

    #[test]
    fn manifest_grid_shape_node_count() {
        assert_eq!(ShapeSpec::Grid { rows: 8, cols: 8 }.num_nodes(), 64);
        assert_eq!(ShapeSpec::Line(5).num_nodes(), 5);
        assert_eq!(ShapeSpec::Ring(3).num_nodes(), 3);
    }

    #[test]
    fn quantize_snaps_fire_forward() {
        // node fires on an 8th grid but quantizes to the quarter grid: the off-quarter
        // fire at beat 0.5 snaps forward to beat 1.0.
        let mut n0 = node(Timebase::Eighth);
        n0.quantize = Some(Timebase::Quarter);
        let nodes = vec![n0];
        let edges = vec![GraphEdge::new(0, 0, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 0.0, 0.0);
        // Make it ready to fire on the first 8th boundary (beat 0.5).
        runtime.energy[0] = 5.0;

        let mut out = Vec::new();
        let mut update = threshold_update(vec![1.0]);
        runtime.process_block(0.0, 0.5, 0, 48_000.0, 0, &mut update, &mut out);
        assert_eq!(out.len(), 1);
        // beat 0.5 boundary snapped to beat 1.0 -> sample 48000.
        assert_eq!(out[0].sample_time, 48_000);
    }
}
