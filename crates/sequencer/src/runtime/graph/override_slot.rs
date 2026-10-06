//! The addressable parts of a graph's overrides (kind-bindings spec §14.2k):
//! what a host kind setter edits and its history records
//! (`GraphOverridePatch`). Each slot is one field: a node intrinsic, a node
//! or edge param, a sequencer-level config field or one group matrix cell,
//! so undo and redo restore exactly the edited field and leave every other
//! edit of the graph alone (an unrecorded legacy `graph-*` write included).
//! A slot's value is `None` for "no override" (the manifest default).

use super::{
    ProjectGraphEdgeParamOverride, ProjectGraphNodeIntrinsicOverride,
    ProjectGraphNodeParamOverride, ProjectGraphOverrides, ProjectGraphQuantizeOverride,
    ProjectGraphRouteOverride, ProjectGraphSeedFrom, GROUP_COUPLING_DEFAULT, GROUP_GAIN_DEFAULT,
    NEURAL_GROUP_CELLS,
};
use crate::neural::NeuralMaxPolySelection;

/// One intrinsic of a node override, with its value (a node's process chain
/// is not one: it has its own history).
#[derive(Clone, Debug, PartialEq)]
pub enum GraphNodeField {
    Resolution(Option<Vec<u8>>),
    Delay(Option<u32>),
    Quantize(Option<ProjectGraphQuantizeOverride>),
    /// A track route or a generator gate (one field: setting either replaces
    /// the other).
    Route(Option<ProjectGraphRouteOverride>),
    SeedFrom(Option<ProjectGraphSeedFrom>),
    SeedOnReset(Option<f64>),
    Group(Option<u8>),
}

impl GraphNodeField {
    fn name(&self) -> &'static str {
        match self {
            Self::Resolution(_) => "resolution",
            Self::Delay(_) => "delay",
            Self::Quantize(_) => "quantize",
            Self::Route(_) => "route",
            Self::SeedFrom(_) => "seed-from",
            Self::SeedOnReset(_) => "seed-on-reset",
            Self::Group(_) => "group",
        }
    }

    /// The same field with `node`'s value.
    fn read(&self, node: Option<&ProjectGraphNodeIntrinsicOverride>) -> Self {
        match self {
            Self::Resolution(_) => Self::Resolution(node.and_then(|n| n.resolution.clone())),
            Self::Delay(_) => Self::Delay(node.and_then(|n| n.delay_steps)),
            Self::Quantize(_) => Self::Quantize(node.and_then(|n| n.quantize.clone())),
            Self::Route(_) => Self::Route(node.and_then(|n| n.route.clone())),
            Self::SeedFrom(_) => Self::SeedFrom(node.and_then(|n| n.seed_from.clone())),
            Self::SeedOnReset(_) => Self::SeedOnReset(node.and_then(|n| n.seed_on_reset)),
            Self::Group(_) => Self::Group(node.and_then(|n| n.neural_group)),
        }
    }

    fn write(&self, node: &mut ProjectGraphNodeIntrinsicOverride) {
        match self {
            Self::Resolution(value) => node.resolution.clone_from(value),
            Self::Delay(value) => node.delay_steps = *value,
            Self::Quantize(value) => node.quantize.clone_from(value),
            Self::Route(value) => node.route.clone_from(value),
            Self::SeedFrom(value) => node.seed_from.clone_from(value),
            Self::SeedOnReset(value) => node.seed_on_reset = *value,
            Self::Group(value) => node.neural_group = *value,
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Self::Resolution(Some(cycle)) => cycle.len(),
            Self::Quantize(Some(ProjectGraphQuantizeOverride::Timebase(cycle))) => cycle.len(),
            Self::SeedFrom(Some(ProjectGraphSeedFrom::Tracks(tracks))) => {
                tracks.len() * std::mem::size_of::<usize>()
            }
            _ => 0,
        }
    }
}

/// One sequencer-level config field of [`ProjectGraphOverrides`], with its
/// value (the group matrices are [`GraphOverrideSlot::GroupCell`]s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GraphConfigField {
    ResetEveryBeats(Option<f64>),
    MaxPoly(Option<u32>),
    MaxPolySelection(Option<NeuralMaxPolySelection>),
    NodeCount(Option<u32>),
    GroupTraceDecay(Option<f64>),
    GroupCouplingScale(Option<f64>),
    GroupExciteFloor(Option<f64>),
}

impl GraphConfigField {
    fn name(self) -> &'static str {
        match self {
            Self::ResetEveryBeats(_) => "reset-every-beats",
            Self::MaxPoly(_) => "max-poly",
            Self::MaxPolySelection(_) => "max-poly-selection",
            Self::NodeCount(_) => "node-count",
            Self::GroupTraceDecay(_) => "group-trace-decay",
            Self::GroupCouplingScale(_) => "group-coupling-scale",
            Self::GroupExciteFloor(_) => "group-excite-floor",
        }
    }

    /// The same field with `graph`'s value.
    fn read(self, graph: &ProjectGraphOverrides) -> Self {
        match self {
            Self::ResetEveryBeats(_) => Self::ResetEveryBeats(graph.reset_every_beats),
            Self::MaxPoly(_) => Self::MaxPoly(graph.max_poly),
            Self::MaxPolySelection(_) => Self::MaxPolySelection(graph.max_poly_selection),
            Self::NodeCount(_) => Self::NodeCount(graph.node_count),
            Self::GroupTraceDecay(_) => Self::GroupTraceDecay(graph.group_trace_decay),
            Self::GroupCouplingScale(_) => Self::GroupCouplingScale(graph.group_coupling_scale),
            Self::GroupExciteFloor(_) => Self::GroupExciteFloor(graph.group_excite_floor),
        }
    }

    fn write(self, graph: &mut ProjectGraphOverrides) {
        match self {
            Self::ResetEveryBeats(value) => graph.reset_every_beats = value,
            Self::MaxPoly(value) => graph.max_poly = value,
            Self::MaxPolySelection(value) => graph.max_poly_selection = value,
            Self::NodeCount(value) => graph.node_count = value,
            Self::GroupTraceDecay(value) => graph.group_trace_decay = value,
            Self::GroupCouplingScale(value) => graph.group_coupling_scale = value,
            Self::GroupExciteFloor(value) => graph.group_excite_floor = value,
        }
    }
}

/// A k×k group matrix: `G` propagation gain or `H` activity coupling, flat
/// row-major (cell `row * NEURAL_GROUP_MAX + col`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupMatrix {
    Gain,
    Coupling,
}

impl GroupMatrix {
    pub fn default_cell(self) -> f64 {
        match self {
            Self::Gain => GROUP_GAIN_DEFAULT,
            Self::Coupling => GROUP_COUPLING_DEFAULT,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Gain => "gain",
            Self::Coupling => "coupling",
        }
    }

    pub fn cells(self, graph: &ProjectGraphOverrides) -> Option<&Vec<f64>> {
        match self {
            Self::Gain => graph.group_gain.as_ref(),
            Self::Coupling => graph.group_coupling.as_ref(),
        }
    }

    fn cells_mut(self, graph: &mut ProjectGraphOverrides) -> &mut Option<Vec<f64>> {
        match self {
            Self::Gain => &mut graph.group_gain,
            Self::Coupling => &mut graph.group_coupling,
        }
    }
}

/// One cell of a group matrix override, a missing or non-finite cell at
/// `default`.
pub fn group_matrix_cell(cells: Option<&Vec<f64>>, index: usize, default: f64) -> f64 {
    cells
        .and_then(|cells| cells.get(index))
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

/// Every cell of a group matrix override, row-major (see
/// [`group_matrix_cell`]); shared with the legacy `graph-config` and the
/// host kinds' `graph.group-gain` / `group-coupling`.
pub fn graph_group_cells(cells: Option<&Vec<f64>>, default: f64) -> Vec<f64> {
    (0..NEURAL_GROUP_CELLS)
        .map(|index| group_matrix_cell(cells, index, default))
        .collect()
}

/// One addressable part of a graph's overrides with its value
/// ([`ProjectGraphOverrides::slot`]).
#[derive(Clone, Debug, PartialEq)]
pub enum GraphOverrideSlot {
    NodeField {
        group: String,
        instance: usize,
        field: GraphNodeField,
    },
    NodeParam {
        group: String,
        instance: usize,
        param: String,
        value: Option<f64>,
    },
    EdgeParam {
        group: String,
        from: usize,
        to: usize,
        param: String,
        value: Option<f64>,
    },
    ConfigField(GraphConfigField),
    /// `None`: no override of the matrix at all.
    GroupCell {
        matrix: GroupMatrix,
        index: usize,
        value: Option<f64>,
    },
}

impl GraphOverrideSlot {
    /// What the slot addresses, as a merge key part: a drag's `set!`s on one
    /// field join one undo entry, a drag over two fields records two.
    pub fn address(&self) -> String {
        match self {
            Self::NodeField {
                group,
                instance,
                field,
            } => format!("node:{group}:{instance}:{}", field.name()),
            Self::NodeParam {
                group,
                instance,
                param,
                ..
            } => format!("param:{group}:{instance}:{param}"),
            Self::EdgeParam {
                group,
                from,
                to,
                param,
                ..
            } => format!("edge:{group}:{from}:{to}:{param}"),
            Self::ConfigField(field) => format!("config:{}", field.name()),
            Self::GroupCell { matrix, index, .. } => format!("cell:{}:{index}", matrix.name()),
        }
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::NodeField { group, field, .. } => group.len() + field.heap_bytes(),
                Self::NodeParam { group, param, .. } | Self::EdgeParam { group, param, .. } => {
                    group.len() + param.len()
                }
                Self::ConfigField(_) | Self::GroupCell { .. } => 0,
            }
    }
}

impl ProjectGraphNodeIntrinsicOverride {
    /// No override of node `instance` of prototype `group`.
    pub fn empty(group: &str, instance: usize) -> Self {
        Self {
            group: group.to_string(),
            instance,
            resolution: None,
            delay_steps: None,
            quantize: None,
            route: None,
            seed_from: None,
            seed_on_reset: None,
            duration: None,
            swing: None,
            neural_group: None,
            process_chain: None,
        }
    }

    /// Overrides nothing (an entry that may as well not exist).
    fn is_empty(&self) -> bool {
        self.resolution.is_none()
            && self.delay_steps.is_none()
            && self.quantize.is_none()
            && self.route.is_none()
            && self.seed_from.is_none()
            && self.seed_on_reset.is_none()
            && self.duration.is_none()
            && self.swing.is_none()
            && self.neural_group.is_none()
            && self.process_chain.is_none()
    }
}

impl ProjectGraphOverrides {
    fn node_intrinsic_at(&self, group: &str, instance: usize) -> Option<usize> {
        (self.node_intrinsics.iter())
            .position(|node| node.group == group && node.instance == instance)
    }

    fn node_param_at(&self, group: &str, instance: usize, param: &str) -> Option<usize> {
        self.node_params.iter().position(|entry| {
            entry.group == group && entry.instance == instance && entry.param == param
        })
    }

    fn edge_param_at(&self, group: &str, from: usize, to: usize, param: &str) -> Option<usize> {
        self.edge_params.iter().position(|entry| {
            entry.group == group && entry.from == from && entry.to == to && entry.param == param
        })
    }

    /// The value `slot` addresses (its own value ignored).
    pub fn slot(&self, slot: &GraphOverrideSlot) -> GraphOverrideSlot {
        match slot {
            GraphOverrideSlot::NodeField {
                group,
                instance,
                field,
            } => {
                let at = self.node_intrinsic_at(group, *instance);
                GraphOverrideSlot::NodeField {
                    group: group.clone(),
                    instance: *instance,
                    field: field.read(at.map(|at| &self.node_intrinsics[at])),
                }
            }
            GraphOverrideSlot::NodeParam {
                group,
                instance,
                param,
                ..
            } => GraphOverrideSlot::NodeParam {
                group: group.clone(),
                instance: *instance,
                param: param.clone(),
                value: (self.node_param_at(group, *instance, param))
                    .map(|at| self.node_params[at].value),
            },
            GraphOverrideSlot::EdgeParam {
                group,
                from,
                to,
                param,
                ..
            } => GraphOverrideSlot::EdgeParam {
                group: group.clone(),
                from: *from,
                to: *to,
                param: param.clone(),
                value: (self.edge_param_at(group, *from, *to, param))
                    .map(|at| self.edge_params[at].value),
            },
            GraphOverrideSlot::ConfigField(field) => {
                GraphOverrideSlot::ConfigField(field.read(self))
            }
            GraphOverrideSlot::GroupCell { matrix, index, .. } => {
                let cells = matrix.cells(self);
                GraphOverrideSlot::GroupCell {
                    matrix: *matrix,
                    index: *index,
                    value: cells.map(|_| group_matrix_cell(cells, *index, matrix.default_cell())),
                }
            }
        }
    }

    /// Write `slot`'s value (`None` removes the override; a node entry or a
    /// group matrix left overriding nothing goes with it).
    pub fn set_slot(&mut self, slot: &GraphOverrideSlot) {
        match slot {
            GraphOverrideSlot::NodeField {
                group,
                instance,
                field,
            } => match self.node_intrinsic_at(group, *instance) {
                Some(at) => {
                    field.write(&mut self.node_intrinsics[at]);
                    if self.node_intrinsics[at].is_empty() {
                        self.node_intrinsics.remove(at);
                    }
                }
                None => {
                    let mut node = ProjectGraphNodeIntrinsicOverride::empty(group, *instance);
                    field.write(&mut node);
                    if !node.is_empty() {
                        self.node_intrinsics.push(node);
                    }
                }
            },
            GraphOverrideSlot::NodeParam {
                group,
                instance,
                param,
                value,
            } => match (self.node_param_at(group, *instance, param), value) {
                (Some(at), Some(value)) => self.node_params[at].value = *value,
                (Some(at), None) => {
                    self.node_params.remove(at);
                }
                (None, Some(value)) => self.node_params.push(ProjectGraphNodeParamOverride {
                    group: group.clone(),
                    instance: *instance,
                    param: param.clone(),
                    value: *value,
                }),
                (None, None) => {}
            },
            GraphOverrideSlot::EdgeParam {
                group,
                from,
                to,
                param,
                value,
            } => match (self.edge_param_at(group, *from, *to, param), value) {
                (Some(at), Some(value)) => self.edge_params[at].value = *value,
                (Some(at), None) => {
                    self.edge_params.remove(at);
                }
                (None, Some(value)) => self.edge_params.push(ProjectGraphEdgeParamOverride {
                    group: group.clone(),
                    from: *from,
                    to: *to,
                    param: param.clone(),
                    value: *value,
                }),
                (None, None) => {}
            },
            GraphOverrideSlot::ConfigField(field) => field.write(self),
            GraphOverrideSlot::GroupCell {
                matrix,
                index,
                value,
            } => {
                if *index >= NEURAL_GROUP_CELLS {
                    return;
                }
                let default = matrix.default_cell();
                let cells = matrix.cells_mut(self);
                if value.is_none() && cells.is_none() {
                    return;
                }
                let mut next = graph_group_cells(cells.as_ref(), default);
                next[*index] = value.unwrap_or(default);
                // A matrix back at its defaults everywhere is no override
                // (they resolve alike).
                *cells =
                    (value.is_some() || next.iter().any(|cell| *cell != default)).then_some(next);
            }
        }
    }
}
