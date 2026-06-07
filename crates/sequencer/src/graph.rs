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

/// How long a fired graph node stays visible in the runtime telemetry. Matches the
/// native neural visualization hold so both layers read similarly in the UI.
const TRIGGER_VISUAL_HOLD_BEATS: f64 = 0.25;

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

/// How a node picks *which* incoming payload (note/velocity) survives when several
/// propagations deposit into it in one boundary. This is the payload analogue of
/// [`Reduce`]: `reduce` folds the scalar *energy*, `EventSelect` folds the *event*.
/// `Newest` is the historical "last-writer-wins" behavior; the others let a loud seed
/// keep its velocity instead of being clobbered by a decayed neural payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EventSelect {
    /// Last propagation to deposit this boundary wins (historical behavior).
    #[default]
    Newest,
    /// Keep the payload with the highest velocity.
    Loudest,
    /// Prefer an external seed's payload over any non-seed; else newest.
    SeedPriority,
    /// Keep the payload carried by the edge with the largest `gather()`.
    Strongest,
}

impl EventSelect {
    /// Whether an incoming payload should replace the one currently held this boundary.
    /// Ties resolve to the incoming (newest) payload so behavior matches `Newest` when
    /// the deciding field is equal.
    fn prefer(
        self,
        cur_seed: bool,
        cur_strength: f64,
        cur_vel: f32,
        new_seed: bool,
        new_strength: f64,
        new_vel: f32,
    ) -> bool {
        match self {
            EventSelect::Newest => true,
            EventSelect::Loudest => new_vel >= cur_vel,
            EventSelect::Strongest => new_strength >= cur_strength,
            EventSelect::SeedPriority => match (cur_seed, new_seed) {
                (false, true) => true,
                (true, false) => false,
                _ => true,
            },
        }
    }
}

/// How a firing source distributes its outgoing payload across positive outgoing
/// edges. `BroadcastWeighted` is the existing neural-style behavior: every positive
/// edge receives the payload. `WeightedChoice` is Markov-style: exactly one positive
/// outgoing edge is selected with probability proportional to its gathered weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EdgeDistribution {
    #[default]
    BroadcastWeighted,
    WeightedChoice,
}

/// One directed edge in the graph. `weight`/`dampening` are edge fields read by the
/// native gather kernel; `dampening` is mutated at runtime by Ext 2. `delay_steps`
/// is per-edge transport delay; 0 means inherit the source node's `delay_steps`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub weight: f64,
    pub dampening: f64,
    pub delay_steps: u32,
    pub distribution: EdgeDistribution,
}

/// One materialized graph edge as exposed through runtime visualization. This is a
/// read-only snapshot of [`GraphRuntime`]'s flat edge list; it is not serialized and is
/// never a second source of truth for live dampening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphVisualizationEdge {
    pub from: usize,
    pub to: usize,
    pub weight: f64,
    pub dampening: f64,
    pub delay_steps: u32,
    pub distribution: EdgeDistribution,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphVisualizationEvent {
    pub node_index: usize,
    pub track: Option<usize>,
    pub sample_time: u64,
    pub beat: f64,
    pub transpose: f32,
    pub velocity: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphVisualizationSnapshot {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub num_nodes: usize,
    pub energy: Vec<f64>,
    pub trigger_activity: Vec<f32>,
    pub node_events: Vec<Option<GraphVisualizationEvent>>,
    pub edges: Vec<GraphVisualizationEdge>,
}

impl GraphEdge {
    pub fn new(from: usize, to: usize, weight: f64) -> Self {
        Self {
            from,
            to,
            weight,
            dampening: 0.0,
            delay_steps: 0,
            distribution: EdgeDistribution::BroadcastWeighted,
        }
    }

    /// The scalar this edge injects into its target this boundary: `max(0, weight -
    /// dampening)` (spec §2.3 neural case — spike propagation, magnitude independent
    /// of source activation).
    fn gather(self) -> f64 {
        (self.weight - self.dampening).max(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GraphDurationSpec {
    Steps {
        value: f64,
    },
    Beats {
        value: f64,
    },
    Timebase {
        index: u8,
    },
    Delay,
    Seed,
    Add {
        items: Vec<GraphDurationSpec>,
    },
    Mul {
        factor: f64,
        item: Box<GraphDurationSpec>,
    },
    Min {
        items: Vec<GraphDurationSpec>,
    },
    Max {
        items: Vec<GraphDurationSpec>,
    },
}

impl Default for GraphDurationSpec {
    fn default() -> Self {
        Self::Steps { value: 1.0 }
    }
}

impl GraphDurationSpec {
    fn resolve_beats(
        &self,
        node_resolution: Timebase,
        node_delay_steps: u32,
        incoming: GraphPayload,
    ) -> f32 {
        fn finite_nonnegative(value: f64) -> f32 {
            if value.is_finite() {
                value.max(0.0) as f32
            } else {
                0.0
            }
        }

        let node_step_beats = node_resolution
            .step_beats(GRAPH_RESOLUTION_REF_STEPS)
            .max(0.0);
        let value = match self {
            Self::Steps { value } => value.max(0.0) * node_step_beats,
            Self::Beats { value } => value.max(0.0),
            Self::Timebase { index } => Timebase::from_index(*index as u32)
                .step_beats(GRAPH_RESOLUTION_REF_STEPS)
                .max(0.0),
            Self::Delay => node_delay_steps as f64 * node_step_beats,
            Self::Seed => incoming.duration_beats as f64,
            Self::Add { items } => items
                .iter()
                .map(|item| item.resolve_beats(node_resolution, node_delay_steps, incoming) as f64)
                .sum(),
            Self::Mul { factor, item } => {
                factor
                    * item
                        .resolve_beats(node_resolution, node_delay_steps, incoming)
                        .max(0.0) as f64
            }
            Self::Min { items } => items
                .iter()
                .map(|item| item.resolve_beats(node_resolution, node_delay_steps, incoming) as f64)
                .reduce(f64::min)
                .unwrap_or(0.0),
            Self::Max { items } => items
                .iter()
                .map(|item| item.resolve_beats(node_resolution, node_delay_steps, incoming) as f64)
                .reduce(f64::max)
                .unwrap_or(0.0),
        };
        finite_nonnegative(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphSwingSpec {
    /// Existing sequencer swing convention: 50 is straight, 75 is maximum delayed swing.
    pub amount: f32,
    /// Swing resolution index: 0=1/16, 1=1/8, 2=1/4, 3=1/2.
    pub resolution: u8,
}

impl Default for GraphSwingSpec {
    fn default() -> Self {
        Self {
            amount: 50.0,
            resolution: 0,
        }
    }
}

impl GraphSwingSpec {
    pub fn new(amount: f32, resolution: u8) -> Self {
        Self {
            amount: amount.clamp(50.0, 75.0),
            resolution: resolution.min(3),
        }
    }

    fn resolution_beats(self) -> f64 {
        match self.resolution {
            1 => 0.5,
            2 => 1.0,
            3 => 2.0,
            _ => 0.25,
        }
    }

    fn apply_to_timing(self, sample_time: u64, beats: f64, samples_per_quarter: f64) -> (u64, f64) {
        if self.amount <= 50.0 || !samples_per_quarter.is_finite() || samples_per_quarter <= 0.0 {
            return (sample_time, beats);
        }
        let resolution_beats = self.resolution_beats().max(1e-9);
        let bucket = ((beats + 1e-9) / resolution_beats).floor() as u64;
        if bucket % 2 == 0 {
            return (sample_time, beats);
        }
        let delay_beats = ((self.amount as f64 / 100.0) - 0.5) * 2.0 * resolution_beats;
        let delay_samples = (delay_beats * samples_per_quarter).round().max(0.0) as u64;
        (
            sample_time.saturating_add(delay_samples),
            beats + delay_beats.max(0.0),
        )
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<GraphDurationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<GraphSwingSpec>,
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
    /// Sequencer-level overrides (None = inherit the manifest default).
    #[serde(default)]
    pub reset_every_beats: Option<f64>,
    #[serde(default)]
    pub max_poly: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub resolution: Timebase,
    pub delay_steps: u32,
    pub quantize: Option<Timebase>,
    pub route: Option<usize>,
    pub seed_track_mask: u128,
    pub reduce: Reduce,
    /// Which incoming payload survives when several arrive in one boundary (Layer A).
    pub event_select: EventSelect,
    /// Initial value of the `energy` state cell on reset/seed (per-node, spec §3.3).
    pub seed_on_reset: f64,
    /// Semitone offset added to the carried payload note each time this node fires
    /// (Ext 1). Mirrors `neural`'s per-neuron `transpose`; the engine applies it on the
    /// default emit path so the cascade works with a bare threshold `:update`.
    pub transpose: f32,
    /// Threshold cached from params for native max-poly propagation scoring.
    pub threshold: f64,
    /// Musical gate duration policy for emitted graph triggers.
    pub duration: GraphDurationSpec,
    /// Timing swing policy for graph triggers.
    pub swing: GraphSwingSpec,
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
            event_select: EventSelect::Newest,
            seed_on_reset: 0.0,
            transpose: 0.0,
            threshold: 1.0,
            duration: GraphDurationSpec::default(),
            swing: GraphSwingSpec::default(),
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
    /// Resolved musical gate duration in quarter-note beats.
    pub duration_beats: f32,
}

impl Default for GraphPayload {
    fn default() -> Self {
        Self {
            note: 0.0,
            velocity: 1.0,
            duration_beats: 0.25,
        }
    }
}

/// A firing's shaped event, built by `(emit …)` in a node's `:update` (Ext B). Each
/// field is optional: an unset field *relays the incoming payload verbatim* (no
/// implicit transpose), so the author owns exactly what they name and nothing else.
/// When a firing carries no `EmitSpec` at all (the `:update` returned a bare truthy
/// value), `commit_firing` falls back to the legacy relay + `transpose` so the native
/// neuron drop-in keeps working unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitSpec {
    /// Absolute emitted/propagated note (semitone offset). `None` relays `in-note`.
    pub note: Option<f32>,
    /// Absolute emitted/propagated velocity. `None` relays `in-vel`.
    pub velocity: Option<f32>,
    /// Emitted/propagated duration policy. `None` falls back to the node duration policy.
    pub duration: Option<GraphDurationSpec>,
    /// Emitted timing swing policy. `None` falls back to the node swing policy.
    pub swing: Option<GraphSwingSpec>,
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
    /// Resolved node resolution for this instance.
    pub resolution: Timebase,
    /// Resolved node propagation delay for this instance.
    pub delay_steps: u32,
    /// The payload that arrived this boundary (`node-input-event`), if any.
    pub input_event: Option<GraphPayload>,
    /// Behavioral params for this node instance: prototype defaults plus sparse
    /// per-pattern overrides.
    pub params: HashMap<String, f64>,
}

/// The decision returned by a node's `:update`. v1a: just whether it fired and the
/// energy to retain if it does *not* fire (so the engine can keep accumulation/decay
/// bookkeeping out of the closure while still letting the rule own the threshold).
#[derive(Clone, Debug, Default)]
pub struct NodeFire {
    pub fired: bool,
    /// The shaped event from `(emit …)`, if the rule emitted one (Ext B). `None` means
    /// the legacy default (relay incoming payload + this node's `transpose`).
    pub emit: Option<EmitSpec>,
    /// If true on an accepted firing, clear the whole graph's runtime state after all
    /// accepted firings at this boundary have emitted.
    pub reset_graph_state: bool,
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
    external_seed: bool,
    edge_index: usize,
}

/// A fired node awaiting `max_poly` selection within one boundary.
#[derive(Clone, Debug)]
struct GraphFiringCandidate {
    node_index: usize,
    fire_sample: u64,
    fire_beats: f64,
    emit: Option<EmitSpec>,
    reset_graph_state: bool,
    dampen_incoming: Option<f64>,
    /// The velocity this firing would emit, for velocity-aware `max_poly` selection.
    velocity: f32,
    /// The note this firing would emit, for transpose-aware `max_poly` selection.
    note: f32,
    /// Whether the surviving incoming payload originated from an external seed, for
    /// `seed-first` `max_poly` selection.
    from_seed: bool,
}

/// Fully materialized, override-resolved graph configuration.
///
/// This is deliberately separate from [`GraphRuntime`]'s mutable state so live UI
/// edits can update routes, delays, params, and weights without replacing energy,
/// pending propagations, RNG, or dampening state.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphRuntimeConfig {
    pub id: u64,
    pub name: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub edge_default_dampening: Vec<f64>,
    pub out_edges: Vec<Vec<usize>>,
    pub in_edges: Vec<Vec<usize>>,
    pub energy_decay: f64,
    pub reset_interval_beats: f64,
    pub max_poly: u32,
    pub max_poly_selection: NeuralMaxPolySelection,
    pub default_duration: GraphDurationSpec,
    pub default_swing: GraphSwingSpec,
    pub node_params: Vec<HashMap<String, f64>>,
}

impl GraphRuntimeConfig {
    pub fn new(
        id: u64,
        name: String,
        nodes: Vec<GraphNode>,
        edges: Vec<GraphEdge>,
        energy_decay: f64,
        reset_interval_beats: f64,
        max_poly: u32,
        max_poly_selection: NeuralMaxPolySelection,
        default_duration: GraphDurationSpec,
        default_swing: GraphSwingSpec,
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
        Self {
            id,
            name,
            nodes,
            edges,
            edge_default_dampening,
            out_edges,
            in_edges,
            energy_decay: energy_decay.clamp(0.0, 1.0),
            reset_interval_beats: reset_interval_beats.max(0.0),
            max_poly,
            max_poly_selection,
            default_duration,
            default_swing,
            node_params: normalized_node_params(num_nodes, node_params),
        }
    }

    fn num_nodes(&self) -> usize {
        self.nodes.len()
    }
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
    max_poly: u32,
    max_poly_selection: NeuralMaxPolySelection,
    default_duration: GraphDurationSpec,
    default_swing: GraphSwingSpec,
    random_state: u64,

    // ── per-node runtime state ──
    node_params: Vec<HashMap<String, f64>>,
    energy: Vec<f64>,
    trigger_activity: Vec<f32>,
    trigger_visual_until_beats: Vec<f64>,
    node_events: Vec<Option<GraphVisualizationEvent>>,
    input_accum: Vec<f64>,
    input_seen: Vec<bool>,
    /// The payload that last arrived at each node (Ext 1), consumed by its next fire.
    source_event: Vec<Option<GraphPayload>>,
    /// Per-node, valid for the current boundary only (gated by `input_seen`): whether the
    /// held `source_event` came from an external seed, and the `gather()` strength of the
    /// edge that delivered it. Used by `EventSelect::SeedPriority` / `Strongest`.
    source_event_seed: Vec<bool>,
    source_event_strength: Vec<f64>,
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
            0,
            NeuralMaxPolySelection::Deterministic,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
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
        max_poly: u32,
        max_poly_selection: NeuralMaxPolySelection,
        default_duration: GraphDurationSpec,
        default_swing: GraphSwingSpec,
        node_params: Vec<HashMap<String, f64>>,
    ) -> Self {
        Self::new_from_config(GraphRuntimeConfig::new(
            id,
            name,
            nodes,
            edges,
            energy_decay,
            reset_interval_beats,
            max_poly,
            max_poly_selection,
            default_duration,
            default_swing,
            node_params,
        ))
    }

    pub fn new_from_config(config: GraphRuntimeConfig) -> Self {
        let num_nodes = config.num_nodes();
        let mut runtime = Self {
            id: config.id,
            name: config.name,
            active: true,
            num_nodes,
            nodes: config.nodes,
            edge_default_dampening: config.edge_default_dampening,
            edges: config.edges,
            out_edges: config.out_edges,
            in_edges: config.in_edges,
            energy_decay: config.energy_decay,
            reset_interval_beats: config.reset_interval_beats,
            max_poly: config.max_poly,
            max_poly_selection: config.max_poly_selection,
            default_duration: config.default_duration,
            default_swing: config.default_swing,
            random_state: config.id,
            node_params: config.node_params,
            energy: vec![0.0; num_nodes],
            trigger_activity: vec![0.0; num_nodes],
            trigger_visual_until_beats: vec![0.0; num_nodes],
            node_events: vec![None; num_nodes],
            input_accum: vec![0.0; num_nodes],
            input_seen: vec![false; num_nodes],
            source_event: vec![None; num_nodes],
            source_event_seed: vec![false; num_nodes],
            source_event_strength: vec![0.0; num_nodes],
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

    /// Voice-steal cap applied per boundary. Resolved (override-or-manifest) into the
    /// config at reconcile time, so the scheduler reads it from the runtime rather than
    /// the static manifest.
    pub fn max_poly(&self) -> u32 {
        self.max_poly
    }

    pub fn visualization_snapshot(&self) -> GraphVisualizationSnapshot {
        GraphVisualizationSnapshot {
            id: self.id,
            name: self.name.clone(),
            active: self.active,
            num_nodes: self.num_nodes,
            energy: self.energy.clone(),
            trigger_activity: self.trigger_activity.clone(),
            node_events: self.node_events.clone(),
            edges: self
                .edges
                .iter()
                .map(|edge| GraphVisualizationEdge {
                    from: edge.from,
                    to: edge.to,
                    weight: edge.weight,
                    dampening: edge.dampening,
                    delay_steps: edge.delay_steps,
                    distribution: edge.distribution,
                })
                .collect(),
        }
    }

    pub fn config_compatible(&self, config: &GraphRuntimeConfig) -> bool {
        self.id == config.id
            && self.num_nodes == config.num_nodes()
            && self.edges.len() == config.edges.len()
            && self
                .edges
                .iter()
                .zip(&config.edges)
                .all(|(current, next)| current.from == next.from && current.to == next.to)
    }

    pub fn apply_config_preserving_state(
        &mut self,
        config: GraphRuntimeConfig,
        total_beats: f64,
    ) -> bool {
        if !self.config_compatible(&config) {
            return false;
        }

        let num_nodes = config.num_nodes();
        let previous_step_beats: Vec<f64> = (0..self.num_nodes)
            .map(|idx| self.node_step_beats(idx))
            .collect();
        let previous_reset_interval_beats = self.reset_interval_beats;
        let edge_default_dampening = config.edge_default_dampening;
        self.id = config.id;
        self.name = config.name;
        self.num_nodes = num_nodes;
        self.nodes = config.nodes;
        self.out_edges = config.out_edges;
        self.in_edges = config.in_edges;
        self.energy_decay = config.energy_decay;
        self.reset_interval_beats = config.reset_interval_beats;
        self.max_poly = config.max_poly;
        self.max_poly_selection = config.max_poly_selection;
        self.default_duration = config.default_duration;
        self.default_swing = config.default_swing;
        self.node_params = config.node_params;

        for (idx, next_edge) in config.edges.into_iter().enumerate() {
            let old_default = self.edge_default_dampening[idx];
            let old_current = self.edges[idx].dampening;
            let new_default = edge_default_dampening[idx];
            let dampening_delta = old_current - old_default;
            let mut updated_edge = next_edge;
            updated_edge.dampening = (new_default + dampening_delta).clamp(0.0, 1.0);
            self.edges[idx] = updated_edge;
            self.edge_default_dampening[idx] = new_default;
        }

        let mut eval_grid_changed = false;
        for (idx, previous_step_beats) in previous_step_beats.into_iter().enumerate() {
            let next_step_beats = self.node_step_beats(idx);
            if (previous_step_beats - next_step_beats).abs() > 1e-9 {
                self.last_eval_indices[idx] = grid_index_at(total_beats, next_step_beats);
                eval_grid_changed = true;
            }
        }
        if eval_grid_changed {
            self.last_decay_index = self.finest_decay_index(total_beats);
        }
        if (previous_reset_interval_beats - self.reset_interval_beats).abs() > 1e-9 {
            self.next_reset_beat = next_reset_beat_after(total_beats, self.reset_interval_beats);
        }
        true
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

    /// Read a node's resolved seed mask (telemetry / tests).
    pub fn seed_track_mask_for_node(&self, node_index: usize) -> Option<u128> {
        self.nodes.get(node_index).map(|node| node.seed_track_mask)
    }

    /// Number of delayed propagations currently queued for a node (telemetry / tests).
    pub fn pending_count_for_node(&self, node_index: usize) -> Option<usize> {
        self.pending.get(node_index).map(Vec::len)
    }

    /// Reset runtime state for transport/periodic resets: clocks realigned to
    /// `total_beats`, energy zeroed then seeded from `seed_on_reset`, edge dampening
    /// restored, decay/reset indices recomputed. Future external seed propagations are
    /// preserved so a clock trigger at the reset boundary can still drive the graph.
    pub fn reset(&mut self, total_beats: f64) {
        self.reset_internal(total_beats, true);
    }

    /// Reset caused by an accepted graph firing. This clears pending propagations too:
    /// a node-authored reset means "clear the graph's state", not "resync transport".
    fn reset_clearing_pending(&mut self, total_beats: f64) {
        self.reset_internal(total_beats, false);
    }

    fn reset_internal(&mut self, total_beats: f64, preserve_external_seeds: bool) {
        for idx in 0..self.num_nodes {
            let preserved_external_seeds = if preserve_external_seeds {
                self.pending[idx]
                    .iter()
                    .copied()
                    .filter(|prop| {
                        prop.external_seed && prop.ready_after_beats + 1e-9 >= total_beats
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            self.energy[idx] = self.nodes[idx].seed_on_reset;
            self.trigger_activity[idx] = 0.0;
            self.trigger_visual_until_beats[idx] = 0.0;
            self.node_events[idx] = None;
            self.input_accum[idx] = 0.0;
            self.input_seen[idx] = false;
            self.source_event[idx] = None;
            self.source_event_seed[idx] = false;
            self.source_event_strength[idx] = 0.0;
            self.tick_count[idx] = 0;
            self.pending[idx] = preserved_external_seeds;
            self.incoming_triggers[idx].clear();
            let step_beats = self.node_step_beats(idx);
            self.last_eval_indices[idx] = grid_index_at(total_beats, step_beats);
        }
        for (edge, default_dampening) in self.edges.iter_mut().zip(&self.edge_default_dampening) {
            edge.dampening = *default_dampening;
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        self.next_reset_beat = next_reset_beat_after(total_beats, self.reset_interval_beats);
    }

    /// Realign every node's clock to the current transport position without firing
    /// the boundaries in between (used on resync / pattern jump).
    pub fn realign(&mut self, total_beats: f64) {
        for idx in 0..self.num_nodes {
            let step_beats = self.node_step_beats(idx);
            self.last_eval_indices[idx] = grid_index_at(total_beats, step_beats);
        }
        self.last_decay_index = self.finest_decay_index(total_beats);
        self.next_reset_beat = next_reset_beat_after(total_beats, self.reset_interval_beats);
    }

    /// Inject a fire into every node whose resolved `seed-from` includes `track`,
    /// respecting that node's delay (spec §4). Mechanically identical to a firing's
    /// scatter: it pushes a delayed propagation onto the seeded node carrying the step
    /// event's `payload`, so the node scatters that payload along its out-edges after
    /// `delay_steps` (Ext 1 — the seed note then ripples through the net).
    pub fn seed(&mut self, track: usize, seed_beats: f64, payload: GraphPayload) -> usize {
        if !self.active {
            return 0;
        }
        let Some(bit) = seed_track_bit(track) else {
            return 0;
        };
        let mut seeded = 0;
        for idx in 0..self.num_nodes {
            if self.nodes[idx].seed_track_mask & bit != 0 {
                self.push_seed_propagation(idx, seed_beats, payload);
                seeded += 1;
            }
        }
        seeded
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
                    resolution: self.nodes[idx].resolution,
                    delay_steps: self.nodes[idx].delay_steps,
                    input_event: self.source_event[idx],
                    params: self.node_params[idx].clone(),
                };
                self.tick_count[idx] = self.tick_count[idx].saturating_add(1);
                let decision = update_fn(&eval);
                decisions[idx] = decision.clone();
                if decision.fired {
                    let (fire_sample, fire_beats) = self.quantized_fire_timing(
                        idx,
                        boundary_beats,
                        sample_time,
                        samples_per_quarter,
                    );
                    let swing = decision
                        .emit
                        .as_ref()
                        .and_then(|emit| emit.swing)
                        .unwrap_or(self.nodes[idx].swing);
                    let (fire_sample, fire_beats) =
                        swing.apply_to_timing(fire_sample, fire_beats, samples_per_quarter);
                    // Mirror commit_firing's payload resolution so max_poly selection
                    // ranks on the velocity/note this firing will actually emit.
                    let incoming = self.source_event[idx].unwrap_or_default();
                    let (note, velocity) = match &decision.emit {
                        Some(spec) => (
                            spec.note.unwrap_or(incoming.note),
                            spec.velocity.unwrap_or(incoming.velocity),
                        ),
                        None => (incoming.note + self.nodes[idx].transpose, incoming.velocity),
                    };
                    candidates.push(GraphFiringCandidate {
                        node_index: idx,
                        fire_sample,
                        fire_beats,
                        emit: decision.emit,
                        reset_graph_state: decision.reset_graph_state,
                        dampen_incoming: decision.dampen_incoming,
                        velocity,
                        note,
                        from_seed: self.source_event_seed[idx],
                    });
                }
            }

            // ── max_poly selection (deterministic: earliest sample, then index). ──
            candidates.sort_by_key(|c| (c.fire_sample, c.node_index));
            let accepted = self.max_poly_accept(&candidates, max_poly);
            let reset_graph_state = candidates
                .iter()
                .enumerate()
                .any(|(cand_idx, candidate)| accepted[cand_idx] && candidate.reset_graph_state);
            let pending_lengths_before_commit = if reset_graph_state {
                self.pending.iter().map(Vec::len).collect::<Vec<_>>()
            } else {
                Vec::new()
            };
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
            if reset_graph_state {
                let mut preserved_reset_firings = Vec::new();
                for (cand_idx, candidate) in candidates.iter().enumerate() {
                    if !accepted[cand_idx] || !candidate.reset_graph_state {
                        continue;
                    }
                    let node_index = candidate.node_index;
                    let pending_start = pending_lengths_before_commit[node_index];
                    let pending = self.pending[node_index][pending_start..].to_vec();
                    preserved_reset_firings.push((
                        node_index,
                        pending,
                        self.trigger_activity[node_index],
                        self.trigger_visual_until_beats[node_index],
                        self.node_events[node_index].clone(),
                    ));
                }
                self.reset_clearing_pending(boundary_beats);
                for (
                    node_index,
                    pending,
                    trigger_activity,
                    trigger_visual_until_beats,
                    node_event,
                ) in preserved_reset_firings
                {
                    self.pending[node_index].extend(pending);
                    self.trigger_activity[node_index] = trigger_activity;
                    self.trigger_visual_until_beats[node_index] = trigger_visual_until_beats;
                    self.node_events[node_index] = node_event;
                }
            }
            if !reset_graph_state {
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
            }

            self.apply_energy_decay(boundary_beats);
        }

        // Resets and decay between the last boundary and the block end.
        while self.next_reset_beat > 0.0 && self.next_reset_beat <= end_beats + 1e-9 {
            self.apply_energy_decay(self.next_reset_beat);
            self.reset(self.next_reset_beat);
        }
        self.apply_energy_decay(end_beats);
        self.refresh_trigger_activity(end_beats);

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
    /// `gather()` along its already-selected edge into the target's input accumulator,
    /// carrying the scatter's payload into the target's `source_event` (Ext 1).
    fn deposit_ready_propagations(&mut self, node_index: usize, boundary_beats: f64) {
        let mut ready: Vec<(usize, GraphPayload, bool)> = Vec::new();
        let mut kept = Vec::with_capacity(self.pending[node_index].len());
        for mut prop in std::mem::take(&mut self.pending[node_index]) {
            if boundary_beats > prop.ready_after_beats + 1e-9 {
                prop.remaining_steps = prop.remaining_steps.saturating_sub(1);
            }
            if prop.remaining_steps == 0 {
                ready.push((prop.edge_index, prop.payload, prop.external_seed));
            } else {
                kept.push(prop);
            }
        }
        self.pending[node_index] = kept;
        for (edge_idx, payload, is_seed) in ready {
            let Some(edge) = self.edges.get(edge_idx).copied() else {
                continue;
            };
            if edge.from != node_index {
                continue;
            }
            let amount = edge.gather();
            if amount <= 0.0 {
                continue;
            }
            let target = edge.to;
            let reduce = self.nodes[target].reduce;
            let first = !self.input_seen[target];
            self.input_accum[target] = reduce.fold(self.input_accum[target], amount, first);
            self.input_seen[target] = true;
            // Energy always accumulates; the *payload* is chosen by the node's
            // EventSelect so a loud seed isn't clobbered by a quiet neural hit.
            let take = first
                || self.nodes[target].event_select.prefer(
                    self.source_event_seed[target],
                    self.source_event_strength[target],
                    self.source_event[target].map(|p| p.velocity).unwrap_or(0.0),
                    is_seed,
                    amount,
                    payload.velocity,
                );
            if take {
                self.source_event[target] = Some(payload);
                self.source_event_seed[target] = is_seed;
                self.source_event_strength[target] = amount;
            }
            if !self.incoming_triggers[target].contains(&edge_idx) {
                self.incoming_triggers[target].push(edge_idx);
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
    /// The emitted event *and* the payload that rides the outgoing scatter are one and
    /// the same `GraphPayload`, so whatever the firing emits is what downstream nodes
    /// receive as their `in-event` — that identity is what makes per-hop fields
    /// accumulate around feedback loops (Ext 1's melodic cascade).
    ///
    /// Ext B: if the rule supplied an `(emit …)` spec, each field it names overrides
    /// the payload and each field it omits relays the incoming value verbatim (no
    /// implicit transpose — the author writes `(+ (in-note) (param :transpose))` if they
    /// want it). With no spec (a bare truthy `:update`), fall back to the legacy relay +
    /// `transpose`, preserving the native-neuron drop-in.
    fn commit_firing(&mut self, candidate: &GraphFiringCandidate, out: &mut Vec<GraphEmission>) {
        let node_index = candidate.node_index;
        let incoming = self.source_event[node_index].unwrap_or_default();
        let payload = match &candidate.emit {
            Some(spec) => GraphPayload {
                note: spec.note.unwrap_or(incoming.note),
                velocity: spec.velocity.unwrap_or(incoming.velocity),
                duration_beats: spec
                    .duration
                    .as_ref()
                    .unwrap_or(&self.nodes[node_index].duration)
                    .resolve_beats(
                        self.nodes[node_index].resolution,
                        self.nodes[node_index].delay_steps,
                        incoming,
                    ),
            },
            None => GraphPayload {
                note: incoming.note + self.nodes[node_index].transpose,
                velocity: incoming.velocity,
                duration_beats: self.nodes[node_index].duration.resolve_beats(
                    self.nodes[node_index].resolution,
                    self.nodes[node_index].delay_steps,
                    incoming,
                ),
            },
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
        event.resolved.duration = payload.duration_beats;
        self.node_events[node_index] = Some(GraphVisualizationEvent {
            node_index,
            track: event.track,
            sample_time: candidate.fire_sample,
            beat: candidate.fire_beats,
            transpose: payload.note,
            velocity: payload.velocity,
        });
        out.push(GraphEmission {
            sample_time: candidate.fire_sample,
            node_index,
            event,
        });
        self.trigger_activity[node_index] = 1.0;
        self.trigger_visual_until_beats[node_index] = self.trigger_visual_until_beats[node_index]
            .max(candidate.fire_beats + TRIGGER_VISUAL_HOLD_BEATS);
        self.energy[node_index] = 0.0;
        if let Some(amount) = candidate.dampen_incoming {
            self.dampen_incoming(node_index, amount);
        }
        self.clear_incoming_triggers(node_index);
        self.push_outgoing_propagations(node_index, candidate.fire_beats, payload, false);
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

    fn refresh_trigger_activity(&mut self, total_beats: f64) {
        for idx in 0..self.num_nodes {
            self.trigger_activity[idx] = if total_beats <= self.trigger_visual_until_beats[idx] {
                1.0
            } else {
                self.node_events[idx] = None;
                0.0
            };
        }
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
            NeuralMaxPolySelection::Loudest => {
                self.accept_top_n(candidates, accepted_count, &mut accepted, |l, r| {
                    r.velocity
                        .total_cmp(&l.velocity)
                        .then(l.fire_sample.cmp(&r.fire_sample))
                        .then(l.node_index.cmp(&r.node_index))
                });
            }
            NeuralMaxPolySelection::LowestTranspose => {
                self.accept_top_n(candidates, accepted_count, &mut accepted, |l, r| {
                    l.note
                        .total_cmp(&r.note)
                        .then(l.fire_sample.cmp(&r.fire_sample))
                        .then(l.node_index.cmp(&r.node_index))
                });
            }
            NeuralMaxPolySelection::HighestTranspose => {
                self.accept_top_n(candidates, accepted_count, &mut accepted, |l, r| {
                    r.note
                        .total_cmp(&l.note)
                        .then(l.fire_sample.cmp(&r.fire_sample))
                        .then(l.node_index.cmp(&r.node_index))
                });
            }
            NeuralMaxPolySelection::SeedFirst => {
                self.accept_top_n(candidates, accepted_count, &mut accepted, |l, r| {
                    r.from_seed
                        .cmp(&l.from_seed)
                        .then(l.fire_sample.cmp(&r.fire_sample))
                        .then(l.node_index.cmp(&r.node_index))
                });
            }
        }
        accepted
    }

    /// Sort candidate indices by `cmp` (best first) and mark the first `accepted_count`.
    fn accept_top_n(
        &self,
        candidates: &[GraphFiringCandidate],
        accepted_count: usize,
        accepted: &mut [bool],
        cmp: impl Fn(&GraphFiringCandidate, &GraphFiringCandidate) -> std::cmp::Ordering,
    ) {
        let mut indices = (0..candidates.len()).collect::<Vec<_>>();
        indices.sort_by(|left, right| cmp(&candidates[*left], &candidates[*right]));
        for candidate_idx in indices.into_iter().take(accepted_count) {
            accepted[candidate_idx] = true;
        }
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

    fn next_random_unit(&mut self) -> f64 {
        (self.next_random_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn next_random_u64(&mut self) -> u64 {
        self.random_state = self.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.random_state)
    }

    fn push_seed_propagation(
        &mut self,
        node_index: usize,
        ready_after_beats: f64,
        payload: GraphPayload,
    ) {
        self.push_outgoing_propagations(node_index, ready_after_beats, payload, true);
    }

    fn push_propagation(
        &mut self,
        node_index: usize,
        ready_after_beats: f64,
        payload: GraphPayload,
    ) {
        self.push_outgoing_propagations(node_index, ready_after_beats, payload, false);
    }

    fn push_outgoing_propagations(
        &mut self,
        node_index: usize,
        ready_after_beats: f64,
        payload: GraphPayload,
        external_seed: bool,
    ) {
        let outgoing = self.out_edges.get(node_index).cloned().unwrap_or_default();
        for distribution in [
            EdgeDistribution::BroadcastWeighted,
            EdgeDistribution::WeightedChoice,
        ] {
            let edge_indices = outgoing
                .iter()
                .copied()
                .filter(|&edge_idx| {
                    self.edges
                        .get(edge_idx)
                        .map(|edge| edge.distribution == distribution && edge.gather() > 0.0)
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            match distribution {
                EdgeDistribution::BroadcastWeighted => {
                    for edge_idx in edge_indices {
                        self.push_edge_propagation(
                            node_index,
                            edge_idx,
                            ready_after_beats,
                            payload,
                            external_seed,
                        );
                    }
                }
                EdgeDistribution::WeightedChoice => {
                    if let Some(edge_idx) = self.choose_weighted_edge(&edge_indices) {
                        self.push_edge_propagation(
                            node_index,
                            edge_idx,
                            ready_after_beats,
                            payload,
                            external_seed,
                        );
                    }
                }
            }
        }
    }

    fn choose_weighted_edge(&mut self, edge_indices: &[usize]) -> Option<usize> {
        let mut total = 0.0;
        for &edge_idx in edge_indices {
            total += self
                .edges
                .get(edge_idx)
                .map(|edge| edge.gather())
                .unwrap_or(0.0);
        }
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        let mut cursor = self.next_random_unit() * total;
        for &edge_idx in edge_indices {
            let amount = self
                .edges
                .get(edge_idx)
                .map(|edge| edge.gather())
                .unwrap_or(0.0);
            if amount <= 0.0 {
                continue;
            }
            if cursor < amount {
                return Some(edge_idx);
            }
            cursor -= amount;
        }
        edge_indices.last().copied()
    }

    fn push_edge_propagation(
        &mut self,
        node_index: usize,
        edge_index: usize,
        ready_after_beats: f64,
        payload: GraphPayload,
        external_seed: bool,
    ) {
        let Some(edge) = self.edges.get(edge_index).copied() else {
            return;
        };
        if edge.from != node_index {
            return;
        }
        let delay_steps = if edge.delay_steps > 0 {
            edge.delay_steps
        } else {
            self.nodes[node_index].delay_steps
        };
        let remaining = delay_steps.max(1);
        self.pending[node_index].push(GraphPropagation {
            remaining_steps: remaining,
            ready_after_beats,
            payload,
            external_seed,
            edge_index,
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

fn grid_index_at(total_beats: f64, step_beats: f64) -> u64 {
    (total_beats / step_beats.max(1e-9)).floor().max(0.0) as u64
}

fn next_reset_beat_after(total_beats: f64, reset_interval_beats: f64) -> f64 {
    if reset_interval_beats > 0.0 {
        ((total_beats / reset_interval_beats).floor() + 1.0) * reset_interval_beats
    } else {
        0.0
    }
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
    pub duration: Option<GraphDurationSpec>,
    pub swing: Option<GraphSwingSpec>,
    pub reduce: Reduce,
    pub event_select: EventSelect,
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
            duration: None,
            swing: None,
            reduce: Reduce::Sum,
            event_select: EventSelect::Newest,
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
/// kernel source, distribution policy, and edge param defaults (`weight`,
/// `dampening`, `delay`).
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeSetSpec {
    pub from: String,
    pub to: String,
    pub topology: Topology,
    pub distribution: EdgeDistribution,
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
    pub duration: GraphDurationSpec,
    pub swing: GraphSwingSpec,
    pub node: NodeProto,
    pub edge_sets: Vec<EdgeSetSpec>,
}

impl GraphManifest {
    /// True if `other` materializes to the same runtime identity and edge topology.
    /// Live-editable config fields such as route, delay, threshold, weight, and
    /// default dampening are intentionally compatible because they can be applied to
    /// an existing runtime without replacing state.
    pub fn structurally_compatible(&self, other: &GraphManifest) -> bool {
        let current = self.runtime_config_with_overrides(None);
        let next = other.runtime_config_with_overrides(None);
        current.id == next.id
            && current.num_nodes() == next.num_nodes()
            && current.edges.len() == next.edges.len()
            && current
                .edges
                .iter()
                .zip(&next.edges)
                .all(|(current, next)| current.from == next.from && current.to == next.to)
    }

    /// Expand the manifest into the node field + edge set for [`GraphRuntime::new`],
    /// applying prototype defaults to every instance. Per-instance plocks (weights,
    /// per-node delay edits) are layered on later by the serialization store; this is
    /// the zero-override baseline.
    pub fn materialize(&self) -> GraphRuntime {
        self.materialize_with_overrides(None)
    }

    pub fn runtime_config_with_overrides(
        &self,
        overrides: Option<&ProjectGraphOverrides>,
    ) -> GraphRuntimeConfig {
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
            let mut duration = self
                .node
                .duration
                .clone()
                .unwrap_or_else(|| self.duration.clone());
            let mut swing = self.node.swing.unwrap_or(self.swing);
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
                    if let Some(value) = &intrinsic.duration {
                        duration = value.clone();
                    }
                    if let Some(value) = intrinsic.swing {
                        swing = value;
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
                event_select: self.node.event_select,
                seed_on_reset: self.seed_on_reset,
                transpose: node_params[idx].get("transpose").copied().unwrap_or(0.0) as f32,
                threshold: node_params[idx].get("threshold").copied().unwrap_or(1.0),
                duration,
                swing,
            });
        }

        let mut edges = Vec::new();
        for set in &self.edge_sets {
            let edge_group = edge_set_group_id(set);
            let weight = set.param_default("weight");
            let dampening = set.param_default("dampening");
            let delay_steps = set.param_default("delay").max(0.0).round() as u32;
            match set.topology {
                Topology::AllToAll => {
                    for from in 0..num_nodes {
                        for to in 0..num_nodes {
                            let mut edge = GraphEdge {
                                from,
                                to,
                                weight,
                                dampening,
                                delay_steps,
                                distribution: set.distribution,
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
                                        "delay" | "delay-steps" => {
                                            edge.delay_steps = param.value.max(0.0).round() as u32
                                        }
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

        let reset_every_beats = overrides
            .and_then(|o| o.reset_every_beats)
            .unwrap_or(self.reset_every_beats);
        let max_poly = overrides.and_then(|o| o.max_poly).unwrap_or(self.max_poly);

        GraphRuntimeConfig::new(
            self.id,
            self.name.clone(),
            nodes,
            edges,
            self.energy_decay,
            reset_every_beats,
            max_poly,
            self.max_poly_selection,
            self.duration.clone(),
            self.swing,
            node_params,
        )
    }

    pub fn materialize_with_overrides(
        &self,
        overrides: Option<&ProjectGraphOverrides>,
    ) -> GraphRuntime {
        GraphRuntime::new_from_config(self.runtime_config_with_overrides(overrides))
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

    fn run_on_input(runtime: &mut GraphRuntime, end_beats: f64) -> Vec<GraphEmission> {
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            end_beats,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.input > 0.0,
                ..NodeFire::default()
            },
            &mut out,
        );
        out
    }

    fn runtime_config(id: u64, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> GraphRuntimeConfig {
        GraphRuntimeConfig::new(
            id,
            "g".into(),
            nodes,
            edges,
            1.0,
            0.0,
            0,
            NeuralMaxPolySelection::Deterministic,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            Vec::new(),
        )
    }

    #[test]
    fn graph_duration_default_is_one_node_step() {
        let mut source = node(Timebase::Sixteenth);
        source.seed_track_mask = seed_track_mask(&[0]);
        let target = node(Timebase::Sixteenth);
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());

        let out = run_on_input(&mut runtime, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.duration, 0.25);
    }

    #[test]
    fn graph_duration_delay_uses_node_delay_steps() {
        let mut source = node(Timebase::Sixteenth);
        source.seed_track_mask = seed_track_mask(&[0]);
        let mut target = node(Timebase::Sixteenth);
        target.delay_steps = 2;
        target.duration = GraphDurationSpec::Delay;
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());

        let out = run_on_input(&mut runtime, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.duration, 0.5);
    }

    #[test]
    fn graph_duration_seed_inherits_incoming_payload_duration() {
        let mut source = node(Timebase::Sixteenth);
        source.seed_track_mask = seed_track_mask(&[0]);
        let mut target = node(Timebase::Sixteenth);
        target.duration = GraphDurationSpec::Seed;
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.75,
            },
        );

        let out = run_on_input(&mut runtime, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.duration, 0.75);
    }

    #[test]
    fn emit_duration_overrides_node_default_and_propagates_downstream() {
        let mut seed = node(Timebase::Quarter);
        seed.seed_track_mask = seed_track_mask(&[0]);
        let n0 = node(Timebase::Quarter);
        let mut n1 = node(Timebase::Quarter);
        n1.duration = GraphDurationSpec::Seed;
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![seed, n0, n1],
            vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(1, 2, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            3.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.input > 0.0,
                emit: (eval.node_index == 1).then(|| EmitSpec {
                    duration: Some(GraphDurationSpec::Beats { value: 0.5 }),
                    swing: None,
                    ..EmitSpec::default()
                }),
                ..NodeFire::default()
            },
            &mut out,
        );

        let node0 = out
            .iter()
            .find(|emission| emission.node_index == 1)
            .expect("node 0 emission");
        let node1 = out
            .iter()
            .find(|emission| emission.node_index == 2)
            .expect("node 1 emission");
        assert_eq!(node0.event.resolved.duration, 0.5);
        assert_eq!(node1.event.resolved.duration, 0.5);
    }

    #[test]
    fn graph_swing_delays_odd_resolution_bucket() {
        let mut source = node(Timebase::Sixteenth);
        source.seed_track_mask = seed_track_mask(&[0]);
        let mut target = node(Timebase::Sixteenth);
        target.swing = GraphSwingSpec::new(75.0, 0);
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());

        let out = run_on_input(&mut runtime, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 18_000);
        assert_eq!(out[0].event.offset_beats, 0.0);
    }

    #[test]
    fn emit_swing_overrides_node_straight_timing() {
        let mut source = node(Timebase::Sixteenth);
        source.seed_track_mask = seed_track_mask(&[0]);
        let target = node(Timebase::Sixteenth);
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.seed(0, 0.0, GraphPayload::default());

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.input > 0.0,
                emit: Some(EmitSpec {
                    swing: Some(GraphSwingSpec::new(75.0, 0)),
                    ..EmitSpec::default()
                }),
                ..NodeFire::default()
            },
            &mut out,
        );

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 18_000);
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
    fn weighted_choice_distribution_selects_one_positive_outgoing_edge() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let mut edges = vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(0, 2, 1.0)];
        for edge in &mut edges {
            edge.distribution = EdgeDistribution::WeightedChoice;
        }
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);

        assert_eq!(out.len(), 1, "weighted choice must not broadcast");
        assert!(
            out[0].node_index == 1 || out[0].node_index == 2,
            "chosen target should be one of the weighted outgoing edges: {out:?}"
        );
    }

    #[test]
    fn broadcast_distribution_deposits_to_every_positive_outgoing_edge() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let edges = vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(0, 2, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);
        let nodes = out
            .iter()
            .map(|emission| emission.node_index)
            .collect::<Vec<_>>();

        assert_eq!(nodes, vec![1, 2]);
    }

    #[test]
    fn weighted_choice_uses_effective_edge_weights_deterministically() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let mut edges = vec![GraphEdge::new(0, 1, 0.2), GraphEdge::new(0, 2, 0.8)];
        for edge in &mut edges {
            edge.distribution = EdgeDistribution::WeightedChoice;
        }

        let mut chooses_first =
            GraphRuntime::new(3, "g".into(), nodes.clone(), edges.clone(), 1.0, 0.0);
        chooses_first.random_state = 3;
        chooses_first.push_propagation(0, 0.0, GraphPayload::default());
        let first_out = run(&mut chooses_first, 1.0, 0, vec![1.0, 0.1, 0.1]);
        assert_eq!(first_out.len(), 1);
        assert_eq!(first_out[0].node_index, 1);

        let mut chooses_second = GraphRuntime::new(0, "g".into(), nodes, edges, 1.0, 0.0);
        chooses_second.random_state = 0;
        chooses_second.push_propagation(0, 0.0, GraphPayload::default());
        let second_out = run(&mut chooses_second, 1.0, 0, vec![1.0, 0.1, 0.1]);
        assert_eq!(second_out.len(), 1);
        assert_eq!(second_out[0].node_index, 2);
    }

    #[test]
    fn weighted_choice_schedules_nothing_when_row_has_no_positive_weight() {
        let nodes = vec![
            node(Timebase::Quarter),
            node(Timebase::Quarter),
            node(Timebase::Quarter),
        ];
        let mut edges = vec![GraphEdge::new(0, 1, 0.0), GraphEdge::new(0, 2, -1.0)];
        for edge in &mut edges {
            edge.distribution = EdgeDistribution::WeightedChoice;
        }
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        assert_eq!(runtime.pending_count_for_node(0), Some(0));
        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);
        assert!(out.is_empty());
    }

    #[test]
    fn zero_edge_delay_inherits_source_node_delay() {
        let mut source = node(Timebase::Quarter);
        source.delay_steps = 2;
        let nodes = vec![source, node(Timebase::Quarter)];
        let edge = GraphEdge::new(0, 1, 1.0);
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, vec![edge], 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 2.0, 0, vec![1.0, 1.0]);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].sample_time, 96_000);
    }

    #[test]
    fn edge_delay_overrides_source_node_delay_for_scheduled_transition() {
        let mut source = node(Timebase::Quarter);
        source.delay_steps = 1;
        let nodes = vec![source, node(Timebase::Quarter)];
        let mut edge = GraphEdge::new(0, 1, 1.0);
        edge.delay_steps = 3;
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, vec![edge], 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let out = run(&mut runtime, 3.0, 0, vec![1.0, 1.0]);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].sample_time, 144_000);
    }

    #[test]
    fn materialized_edge_delay_defaults_and_overrides_reach_runtime_edges() {
        let manifest = GraphManifest {
            id: 1,
            name: "markov".into(),
            shape: ShapeSpec::Line(2),
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            duration: GraphDurationSpec::default(),
            swing: GraphSwingSpec::default(),
            node: NodeProto {
                name: "state".into(),
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "state".into(),
                to: "state".into(),
                topology: Topology::AllToAll,
                distribution: EdgeDistribution::WeightedChoice,
                gather_source: None,
                params: vec![
                    ParamSpec {
                        name: "weight".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        is_int: false,
                    },
                    ParamSpec {
                        name: "delay".into(),
                        min: 0.0,
                        max: 16.0,
                        default: 2.0,
                        is_int: true,
                    },
                ],
            }],
        };
        let group = edge_set_group_id(&manifest.edge_sets[0]);
        let overrides = ProjectGraphOverrides {
            sequencer_id: manifest.id,
            sequencer_name: manifest.name.clone(),
            edge_params: vec![ProjectGraphEdgeParamOverride {
                group,
                from: 0,
                to: 1,
                param: "delay".into(),
                value: 5.0,
            }],
            ..ProjectGraphOverrides::default()
        };

        let runtime = manifest.materialize_with_overrides(Some(&overrides));
        let snapshot = runtime.visualization_snapshot();
        let default_edge = snapshot
            .edges
            .iter()
            .find(|edge| edge.from == 0 && edge.to == 0)
            .expect("default edge");
        let overridden_edge = snapshot
            .edges
            .iter()
            .find(|edge| edge.from == 0 && edge.to == 1)
            .expect("overridden edge");
        assert_eq!(default_edge.delay_steps, 2);
        assert_eq!(overridden_edge.delay_steps, 5);
        assert_eq!(
            overridden_edge.distribution,
            EdgeDistribution::WeightedChoice
        );
    }

    #[test]
    fn process_block_evaluates_due_boundaries_across_audio_sized_chunks() {
        let nodes = vec![node(Timebase::Sixteenth)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, Vec::new(), 1.0, 0.0);
        let mut eval_beats = Vec::new();
        let mut out = Vec::new();
        let mut start_beats = 0.0_f64;

        while start_beats < 1.0 {
            let end_beats = (start_beats + 0.021_f64).min(1.0_f64);
            runtime.process_block(
                start_beats,
                end_beats,
                0,
                48_000.0,
                0,
                |eval| {
                    eval_beats.push(eval.beat);
                    NodeFire::default()
                },
                &mut out,
            );
            start_beats = end_beats;
        }

        assert_eq!(eval_beats, vec![0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn compatible_config_refresh_preserves_eval_cursor_across_audio_sized_chunks() {
        let nodes = vec![node(Timebase::Sixteenth)];
        let mut runtime =
            GraphRuntime::new_from_config(runtime_config(1, nodes.clone(), Vec::new()));
        let mut eval_beats = Vec::new();
        let mut out = Vec::new();
        let mut start_beats = 0.0_f64;

        while start_beats < 1.0 {
            let end_beats = (start_beats + 0.021_f64).min(1.0_f64);
            assert!(runtime.apply_config_preserving_state(
                runtime_config(1, nodes.clone(), Vec::new()),
                end_beats,
            ));
            runtime.process_block(
                start_beats,
                end_beats,
                0,
                48_000.0,
                0,
                |eval| {
                    eval_beats.push(eval.beat);
                    NodeFire::default()
                },
                &mut out,
            );
            start_beats = end_beats;
        }

        assert_eq!(eval_beats, vec![0.25, 0.5, 0.75, 1.0]);
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
    fn reset_every_preserves_external_seed_at_reset_boundary() {
        let mut n0 = node(Timebase::Sixteenth);
        n0.seed_track_mask = seed_track_mask(&[0]);
        let mut n1 = node(Timebase::Sixteenth);
        n1.route = Some(1);
        let nodes = vec![n0, n1];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 1.0);

        runtime.seed(0, 1.0, GraphPayload::default());

        let out = run(&mut runtime, 1.25, 0, vec![1.0, 1.0]);
        let observed = out
            .iter()
            .map(|emission| {
                (
                    emission.sample_time,
                    emission.node_index,
                    emission.event.track,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![(60_000, 1, Some(1))]);
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
            duration: GraphDurationSpec::default(),
            swing: GraphSwingSpec::default(),
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                distribution: EdgeDistribution::BroadcastWeighted,
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
                duration_beats: 0.25,
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
                duration_beats: 0.25,
            },
        );

        let out = run(&mut runtime, 4.0, 0, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.transpose, 10.0);
        assert_eq!(out[0].event.resolved.velocity, 0.5);
    }

    #[test]
    fn accepted_reset_state_firing_clears_graph_state_after_boundary_emission() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            nodes,
            vec![GraphEdge::new(0, 1, 1.0), GraphEdge::new(1, 0, 1.0)],
            1.0,
            0.0,
        );
        runtime.energy[0] = 3.0;
        runtime.edges[0].dampening = 0.75;
        runtime.nodes[0].seed_track_mask = seed_track_mask(&[9]);
        assert_eq!(runtime.seed(9, 2.0, GraphPayload::default()), 1);
        runtime.push_propagation(
            0,
            0.0,
            GraphPayload {
                note: 5.0,
                velocity: 0.5,
                duration_beats: 0.25,
            },
        );

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.node_index == 1 && eval.input > 0.0,
                reset_graph_state: eval.node_index == 1,
                ..NodeFire::default()
            },
            &mut out,
        );

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.transpose, 5.0);
        assert_eq!(out[0].event.resolved.velocity, 0.5);
        assert_eq!(runtime.energy, vec![0.0, 0.0]);
        assert_eq!(
            runtime.pending_count_for_node(0),
            Some(0),
            "unrelated pending state should be cleared"
        );
        assert_eq!(
            runtime.pending_count_for_node(1),
            Some(1),
            "the reset firing's outgoing propagation should survive the reset"
        );
        assert_eq!(runtime.edge_dampening(0), Some(0.0));
        assert_eq!(runtime.trigger_activity, vec![0.0, 1.0]);

        let mut propagated_after_reset = Vec::new();
        runtime.process_block(
            1.0,
            2.0,
            48_000,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.node_index == 0 && eval.input > 0.0,
                ..NodeFire::default()
            },
            &mut propagated_after_reset,
        );
        assert_eq!(propagated_after_reset.len(), 1);
        assert_eq!(propagated_after_reset[0].node_index, 0);
        assert_eq!(propagated_after_reset[0].event.resolved.transpose, 5.0);
        assert_eq!(propagated_after_reset[0].event.resolved.velocity, 0.5);
    }

    #[test]
    fn accepted_firing_sets_trigger_activity_and_event_payload() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(7, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(
            0,
            0.0,
            GraphPayload {
                note: 5.0,
                velocity: 0.7,
                duration_beats: 0.25,
            },
        );

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.node_index == 1 && eval.input > 0.0,
                emit: Some(EmitSpec {
                    note: Some(12.0),
                    velocity: Some(0.42),
                    duration: None,
                    swing: None,
                }),
                ..NodeFire::default()
            },
            &mut out,
        );

        let snapshot = runtime.visualization_snapshot();
        assert_eq!(snapshot.trigger_activity, vec![0.0, 1.0]);
        let event = snapshot.node_events[1].expect("node 1 event telemetry");
        assert_eq!(event.node_index, 1);
        assert_eq!(event.track, None);
        assert_eq!(event.sample_time, 48_000);
        assert_eq!(event.beat, 1.0);
        assert_eq!(event.transpose, 12.0);
        assert_eq!(event.velocity, 0.42);
    }

    #[test]
    fn trigger_activity_and_event_payload_expire_and_reset() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let edges = vec![GraphEdge::new(0, 1, 1.0)];
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes, edges, 1.0, 0.0);
        runtime.push_propagation(0, 0.0, GraphPayload::default());
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            always_fire_with_dampen(0.0),
            &mut out,
        );
        assert_eq!(runtime.visualization_snapshot().trigger_activity[1], 1.0);

        runtime.process_block(
            1.0,
            1.26,
            48_000,
            48_000.0,
            0,
            |_eval| NodeFire::default(),
            &mut out,
        );
        let expired = runtime.visualization_snapshot();
        assert_eq!(expired.trigger_activity[1], 0.0);
        assert!(expired.node_events[1].is_none());

        runtime.push_propagation(0, 1.26, GraphPayload::default());
        runtime.process_block(
            1.26,
            2.0,
            60_480,
            48_000.0,
            0,
            always_fire_with_dampen(0.0),
            &mut out,
        );
        assert_eq!(runtime.visualization_snapshot().trigger_activity[1], 1.0);
        runtime.reset(2.0);
        let reset = runtime.visualization_snapshot();
        assert_eq!(reset.trigger_activity[1], 0.0);
        assert!(reset.node_events[1].is_none());
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
        assert_eq!(runtime.visualization_snapshot().edges[0].dampening, 0.5);
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
        assert!((runtime.visualization_snapshot().edges[0].dampening - 0.4).abs() < f64::EPSILON);
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
        let snapshot = runtime.visualization_snapshot();
        assert_eq!(snapshot.trigger_activity[1], 1.0);
        assert_eq!(snapshot.trigger_activity[2], 0.0);
        assert!(snapshot.node_events[1].is_some());
        assert!(snapshot.node_events[2].is_none());
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
            0,
            NeuralMaxPolySelection::Propagation,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            Vec::new(),
        );
        let out = run(&mut runtime, 1.0, 1, vec![1.0, 1.0, 99.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
    }

    // Two sources scatter into one target in the same boundary, carrying different
    // velocities. Source 0 (loud, vel 1.0) deposits before source 1 (quiet, vel 0.2)
    // because deposit runs in node-index order, so `Newest` keeps the quiet payload and
    // `Loudest` keeps the loud one. This is the seed-vs-neural velocity clobber.
    fn two_source_velocity_runtime(event_select: EventSelect) -> GraphRuntime {
        let n0 = node(Timebase::Quarter);
        let n1 = node(Timebase::Quarter);
        let mut target = node(Timebase::Quarter);
        target.event_select = event_select;
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![n0, n1, target],
            vec![GraphEdge::new(0, 2, 1.0), GraphEdge::new(1, 2, 1.0)],
            1.0,
            0.0,
        );
        runtime.push_propagation(
            0,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        runtime.push_propagation(
            1,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 0.2,
                duration_beats: 0.25,
            },
        );
        runtime
    }

    #[test]
    fn event_select_newest_keeps_last_deposited_velocity() {
        let mut runtime = two_source_velocity_runtime(EventSelect::Newest);
        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 2);
        assert_eq!(out[0].event.resolved.velocity, 0.2);
    }

    #[test]
    fn event_select_loudest_keeps_highest_velocity() {
        let mut runtime = two_source_velocity_runtime(EventSelect::Loudest);
        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 2);
        assert_eq!(out[0].event.resolved.velocity, 1.0);
    }

    #[test]
    fn event_select_seed_priority_prefers_seed_payload() {
        let n0 = node(Timebase::Quarter);
        let n1 = node(Timebase::Quarter);
        let mut target = node(Timebase::Quarter);
        target.event_select = EventSelect::SeedPriority;
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            vec![n0, n1, target],
            vec![GraphEdge::new(0, 2, 1.0), GraphEdge::new(1, 2, 1.0)],
            1.0,
            0.0,
        );
        // Source 0 carries the loud seed; source 1 is a louder *non-seed* that deposits
        // afterward. Seed-priority keeps the seed's payload regardless of velocity order.
        runtime.push_seed_propagation(
            0,
            0.0,
            GraphPayload {
                note: 7.0,
                velocity: 0.5,
                duration_beats: 0.25,
            },
        );
        runtime.push_propagation(
            1,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 1.0,
                duration_beats: 0.25,
            },
        );
        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.resolved.velocity, 0.5);
        assert_eq!(out[0].event.resolved.transpose, 7.0);
    }

    #[test]
    fn max_poly_loudest_selection_keeps_highest_velocity_fire() {
        // Two nodes fire in the same boundary off seeded energy; node 1's incoming payload
        // is louder, so `loudest` keeps it over the earlier-indexed node 0.
        let mut n0 = node(Timebase::Quarter);
        n0.seed_track_mask = seed_track_mask(&[0]);
        let mut n1 = node(Timebase::Quarter);
        n1.seed_track_mask = seed_track_mask(&[1]);
        let mut runtime = GraphRuntime::new_with_config(
            1,
            "g".into(),
            vec![n0, n1],
            vec![GraphEdge::new(0, 0, 1.0), GraphEdge::new(1, 1, 1.0)],
            1.0,
            0.0,
            1,
            NeuralMaxPolySelection::Loudest,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            Vec::new(),
        );
        runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 0.3,
                duration_beats: 0.25,
            },
        );
        runtime.seed(
            1,
            0.0,
            GraphPayload {
                note: 0.0,
                velocity: 0.9,
                duration_beats: 0.25,
            },
        );
        let out = run(&mut runtime, 1.0, 1, vec![1.0, 1.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.velocity, 0.9);
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
            0,
            NeuralMaxPolySelection::Random,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
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
    fn live_config_update_preserves_runtime_state() {
        let mut n0 = node(Timebase::Quarter);
        n0.seed_track_mask = seed_track_mask(&[0]);
        let mut n1 = node(Timebase::Quarter);
        n1.route = Some(1);
        let mut runtime = GraphRuntime::new_with_config(
            11,
            "g".into(),
            vec![n0, n1],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            8.0,
            0,
            NeuralMaxPolySelection::Deterministic,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            Vec::new(),
        );
        runtime.energy[1] = 0.75;
        runtime.input_accum[1] = 0.25;
        runtime.input_seen[1] = true;
        runtime.source_event[1] = Some(GraphPayload {
            note: 7.0,
            velocity: 0.8,
            duration_beats: 0.25,
        });
        runtime.tick_count[1] = 9;
        runtime.pending[0].push(GraphPropagation {
            remaining_steps: 3,
            ready_after_beats: 2.0,
            payload: GraphPayload::default(),
            external_seed: false,
            edge_index: 0,
        });
        runtime.incoming_triggers[1].push(0);
        runtime.random_state = 123;

        let mut next0 = node(Timebase::Eighth);
        next0.delay_steps = 4;
        next0.seed_track_mask = seed_track_mask(&[3]);
        let mut next1 = node(Timebase::Quarter);
        next1.route = Some(2);
        next1.threshold = 0.4;
        next1.transpose = 5.0;
        let mut params = vec![HashMap::new(), HashMap::new()];
        params[1].insert("threshold".into(), 0.4);
        params[1].insert("transpose".into(), 5.0);
        let config = GraphRuntimeConfig::new(
            11,
            "g".into(),
            vec![next0, next1],
            vec![GraphEdge::new(0, 1, 0.2)],
            0.5,
            4.0,
            0,
            NeuralMaxPolySelection::Propagation,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            params,
        );

        assert!(runtime.apply_config_preserving_state(config, 2.0));

        assert_eq!(runtime.energy[1], 0.75);
        assert_eq!(runtime.input_accum[1], 0.25);
        assert!(runtime.input_seen[1]);
        assert_eq!(
            runtime.source_event[1],
            Some(GraphPayload {
                note: 7.0,
                velocity: 0.8,
                duration_beats: 0.25,
            })
        );
        assert_eq!(runtime.tick_count[1], 9);
        assert_eq!(runtime.pending[0][0].remaining_steps, 3);
        assert_eq!(runtime.incoming_triggers[1], vec![0]);
        assert_eq!(runtime.random_state, 123);
        assert_eq!(runtime.nodes[0].delay_steps, 4);
        assert_eq!(runtime.nodes[0].seed_track_mask, seed_track_mask(&[3]));
        assert_eq!(runtime.nodes[1].route, Some(2));
        assert_eq!(runtime.nodes[1].threshold, 0.4);
        assert_eq!(runtime.nodes[1].transpose, 5.0);
        assert_eq!(runtime.edges[0].weight, 0.2);
        assert_eq!(runtime.energy_decay, 0.5);
        assert_eq!(runtime.reset_interval_beats, 4.0);
        assert_eq!(
            runtime.max_poly_selection,
            NeuralMaxPolySelection::Propagation
        );
    }

    #[test]
    fn live_weight_zero_affects_pending_propagation() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            nodes.clone(),
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.push_propagation(0, 0.0, GraphPayload::default());

        let config = runtime_config(1, nodes, vec![GraphEdge::new(0, 1, 0.0)]);
        assert!(runtime.apply_config_preserving_state(config, 0.0));

        let out = run(&mut runtime, 1.0, 0, vec![1.0, 1.0]);
        assert!(
            out.is_empty(),
            "pending scatter should read the live-edited zero weight"
        );
    }

    #[test]
    fn live_delay_edit_preserves_existing_pending_and_affects_future_seeds() {
        let mut n0 = node(Timebase::Quarter);
        n0.delay_steps = 3;
        n0.seed_track_mask = seed_track_mask(&[0]);
        let edges = vec![GraphEdge::new(0, 0, 1.0)];
        let mut runtime =
            GraphRuntime::new(1, "g".into(), vec![n0.clone()], edges.clone(), 1.0, 0.0);
        runtime.seed(0, 0.0, GraphPayload::default());
        assert_eq!(runtime.pending[0][0].remaining_steps, 3);

        n0.delay_steps = 6;
        let config = runtime_config(1, vec![n0], edges);
        assert!(runtime.apply_config_preserving_state(config, 0.0));
        assert_eq!(runtime.pending[0][0].remaining_steps, 3);

        runtime.seed(0, 0.0, GraphPayload::default());
        assert_eq!(runtime.pending[0][1].remaining_steps, 6);
    }

    #[test]
    fn live_node_param_update_changes_threshold_and_preserves_payload() {
        let mut n0 = node(Timebase::Quarter);
        n0.threshold = 1.0;
        let mut runtime = GraphRuntime::new(1, "g".into(), vec![n0.clone()], Vec::new(), 1.0, 0.0);
        runtime.energy[0] = 0.5;
        runtime.source_event[0] = Some(GraphPayload {
            note: 12.0,
            velocity: 0.6,
            duration_beats: 0.25,
        });

        n0.threshold = 0.25;
        n0.transpose = 7.0;
        let mut params = HashMap::new();
        params.insert("threshold".into(), 0.25);
        params.insert("transpose".into(), 7.0);
        let config = GraphRuntimeConfig::new(
            1,
            "g".into(),
            vec![n0],
            Vec::new(),
            1.0,
            0.0,
            0,
            NeuralMaxPolySelection::Deterministic,
            GraphDurationSpec::default(),
            GraphSwingSpec::default(),
            vec![params],
        );
        assert!(runtime.apply_config_preserving_state(config, 0.0));

        assert_eq!(runtime.energy[0], 0.5);
        assert_eq!(
            runtime.source_event[0],
            Some(GraphPayload {
                note: 12.0,
                velocity: 0.6,
                duration_beats: 0.25,
            })
        );
        assert_eq!(runtime.nodes[0].threshold, 0.25);
        assert_eq!(runtime.node_params[0].get("threshold"), Some(&0.25));
    }

    #[test]
    fn live_default_dampening_edit_preserves_delta_from_default() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut edge = GraphEdge::new(0, 1, 1.0);
        edge.dampening = 0.2;
        let mut runtime = GraphRuntime::new(1, "g".into(), nodes.clone(), vec![edge], 1.0, 0.0);
        runtime.edges[0].dampening = 0.5;

        let mut next_edge = GraphEdge::new(0, 1, 0.8);
        next_edge.dampening = 0.1;
        let config = runtime_config(1, nodes, vec![next_edge]);
        assert!(runtime.apply_config_preserving_state(config, 0.0));

        assert_eq!(runtime.edge_default_dampening[0], 0.1);
        assert!((runtime.edges[0].dampening - 0.4).abs() < f64::EPSILON);
        assert_eq!(runtime.edges[0].weight, 0.8);
    }

    #[test]
    fn incompatible_config_is_rejected_without_clearing_state() {
        let nodes = vec![node(Timebase::Quarter), node(Timebase::Quarter)];
        let mut runtime = GraphRuntime::new(
            1,
            "g".into(),
            nodes.clone(),
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        runtime.energy[1] = 0.5;
        let config = runtime_config(1, nodes, vec![GraphEdge::new(1, 0, 1.0)]);

        assert!(!runtime.apply_config_preserving_state(config, 0.0));
        assert_eq!(runtime.energy[1], 0.5);
        assert_eq!((runtime.edges[0].from, runtime.edges[0].to), (0, 1));
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
