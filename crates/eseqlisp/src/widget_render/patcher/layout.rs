use std::collections::HashMap;

use super::display::node_size_for_ports;
use super::geometry::{
    patch_input_indices, patch_input_slot_counts, patch_output_counts, port_x_offset,
};
use super::metrics::{LAYER_SPACING, NODE_COLUMN_GAP, VIEW_PADDING_X, VIEW_PADDING_Y};
use super::model::{
    CableSegmentInfo, ConnectionKind, NodeKind, Patch, connection_touches_hidden_inline_node,
    hidden_inline_node_ids,
};

const CROSSING_MIN_ITERATIONS: usize = 6;
const HISTORY_LOCALITY_WEIGHT: f32 = 3.0;
const DUMMY_NODE_WIDTH: f32 = 1.0;
const MAX_REPAIR_MULTIPLIER: usize = 4;
const LAYOUT_LAYER_SPACING: f32 = LAYER_SPACING * 1.35;
const SEGMENT_NODE_MARGIN: f32 = 0.18;
const SEGMENT_RANGE_PADDING: f32 = 0.35;
const SEGMENT_MIN_GAP: f32 = 0.55;
const SEGMENT_LANE_SPACING: f32 = 0.62;
const PARAM_EXIT_ROW_PADDING: f32 = 0.22;
const FEEDBACK_SEGMENT_ROW_OFFSET: f32 = LAYOUT_LAYER_SPACING * 0.3;
const PARAM_STACK_MIN_COUNT: usize = 3;
const PARAM_STACK_VERTICAL_GAP: f32 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkNodeKind {
    Real(usize),
    Dummy { edge_index: usize },
}

#[derive(Debug, Clone)]
struct WorkNode {
    kind: WorkNodeKind,
    rank: usize,
    order: usize,
    stable_order: usize,
    width: f32,
    height: f32,
    is_history: bool,
    is_hidden: bool,
}

#[derive(Debug, Clone, Copy)]
struct LayoutEdge {
    from: usize,
    to: usize,
    from_output: usize,
    to_input: usize,
    kind: ConnectionKind,
}

#[derive(Debug, Clone, Copy)]
struct OrderingEdge {
    from: usize,
    to: usize,
    weight: f32,
    from_port_bias: f32,
    to_port_bias: f32,
}

pub(super) fn assign_layout(patch: &mut Patch) {
    if patch.nodes.is_empty() {
        return;
    }

    let Some(mut graph) = LayoutGraph::new(patch) else {
        return;
    };
    graph.assign_ranks(patch);
    graph.apply_history_rank_locality(patch);
    graph.build_work_graph();
    graph.minimize_crossings();
    graph.assign_coordinates(patch);
}

struct LayoutGraph {
    real_nodes: Vec<WorkNode>,
    real_edges: Vec<LayoutEdge>,
    work_nodes: Vec<WorkNode>,
    ordering_edges: Vec<OrderingEdge>,
    ranks: Vec<Vec<usize>>,
    input_slots: Vec<Vec<usize>>,
    input_slot_counts: Vec<usize>,
    output_counts: Vec<usize>,
}

impl LayoutGraph {
    fn new(patch: &Patch) -> Option<Self> {
        let input_indices = patch_input_indices(patch);
        let input_slot_counts_by_id = patch_input_slot_counts(patch, &input_indices);
        let output_counts_by_id = patch_output_counts(patch);
        let hidden_node_ids = hidden_inline_node_ids(patch);

        let mut id_to_idx = HashMap::new();
        for (idx, node) in patch.nodes.iter().enumerate() {
            id_to_idx.insert(node.id.as_str(), idx);
        }

        let mut input_slot_counts = Vec::with_capacity(patch.nodes.len());
        let mut input_slots = Vec::with_capacity(patch.nodes.len());
        let mut output_counts = Vec::with_capacity(patch.nodes.len());
        let mut real_nodes = Vec::with_capacity(patch.nodes.len());
        for (idx, node) in patch.nodes.iter().enumerate() {
            let hidden = hidden_node_ids.contains(&node.id);
            let input_count = input_slot_counts_by_id.get(&node.id).copied().unwrap_or(0);
            let output_count = output_counts_by_id.get(&node.id).copied().unwrap_or(0);
            let (width, height) = if hidden {
                (0.0, 0.0)
            } else {
                node_size_for_ports(node, input_count, output_count)
            };
            input_slot_counts.push(input_count);
            output_counts.push(output_count);
            real_nodes.push(WorkNode {
                kind: WorkNodeKind::Real(idx),
                rank: 0,
                order: idx,
                stable_order: idx,
                width,
                height,
                is_history: node.kind == NodeKind::History,
                is_hidden: hidden,
            });
            input_slots.push(
                input_indices
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_else(Vec::new),
            );
        }

        let mut real_edges = Vec::new();
        for connection in &patch.connections {
            if connection_touches_hidden_inline_node(connection, &hidden_node_ids) {
                continue;
            }
            let (Some(&from), Some(&to)) = (
                id_to_idx.get(connection.from_node.as_str()),
                id_to_idx.get(connection.to_node.as_str()),
            ) else {
                continue;
            };
            real_edges.push(LayoutEdge {
                from,
                to,
                from_output: connection.from_output,
                to_input: connection.to_input,
                kind: connection.kind,
            });
        }

        Some(Self {
            real_nodes,
            real_edges,
            work_nodes: Vec::new(),
            ordering_edges: Vec::new(),
            ranks: Vec::new(),
            input_slots,
            input_slot_counts,
            output_counts,
        })
    }

    fn assign_ranks(&mut self, patch: &Patch) {
        for node in &mut self.real_nodes {
            node.rank = 0;
        }

        self.relax_forward_constraints(patch.nodes.len());
        self.place_local_sources_near_consumers(patch);
        self.relax_forward_constraints(patch.nodes.len());
        self.force_outputs_to_final_rank(patch);
    }

    fn apply_history_rank_locality(&mut self, patch: &Patch) {
        for idx in 0..self.real_nodes.len() {
            if patch.nodes[idx].kind != NodeKind::History {
                continue;
            }

            let mut writer_ranks = Vec::new();
            let mut consumer_ranks = Vec::new();
            for edge in &self.real_edges {
                if edge.kind == ConnectionKind::Feedback && edge.to == idx {
                    writer_ranks.push(self.real_nodes[edge.from].rank);
                } else if edge.kind == ConnectionKind::Forward && edge.from == idx {
                    consumer_ranks.push(self.real_nodes[edge.to].rank);
                }
            }

            let Some(preferred) = preferred_history_rank(&writer_ranks, &consumer_ranks) else {
                continue;
            };
            self.real_nodes[idx].rank = preferred;
        }

        let repair_limit = self
            .real_nodes
            .len()
            .saturating_mul(MAX_REPAIR_MULTIPLIER)
            .max(1);
        self.relax_forward_constraints(repair_limit);
        self.place_local_sources_near_consumers(patch);
        self.relax_forward_constraints(repair_limit);
        self.force_outputs_to_final_rank(patch);
    }

    fn relax_forward_constraints(&mut self, limit: usize) {
        for _ in 0..limit {
            let mut changed = false;
            for edge in &self.real_edges {
                if edge.kind == ConnectionKind::Feedback || self.is_history_read_cycle_edge(*edge) {
                    continue;
                }
                let next = self.real_nodes[edge.from].rank.saturating_add(1);
                if self.real_nodes[edge.to].rank < next {
                    self.real_nodes[edge.to].rank = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn force_outputs_to_final_rank(&mut self, patch: &Patch) {
        let max_non_output_rank = self
            .real_nodes
            .iter()
            .enumerate()
            .filter(|(idx, _)| patch.nodes[*idx].kind != NodeKind::Out)
            .map(|(_, node)| node.rank)
            .max()
            .unwrap_or(0);
        let output_rank = max_non_output_rank.saturating_add(1);
        for (idx, node) in self.real_nodes.iter_mut().enumerate() {
            if patch.nodes[idx].kind == NodeKind::Out {
                node.rank = output_rank;
            }
        }
    }

    fn place_local_sources_near_consumers(&mut self, patch: &Patch) {
        for idx in 0..self.real_nodes.len() {
            if patch.nodes[idx].kind != NodeKind::Constant {
                continue;
            }

            let consumer_edges = self
                .real_edges
                .iter()
                .filter(|edge| edge.kind == ConnectionKind::Forward && edge.from == idx)
                .collect::<Vec<_>>();
            let consumer_rank = consumer_edges
                .iter()
                .map(|edge| self.real_nodes[edge.to].rank)
                .min();
            if let Some(rank) = consumer_rank.and_then(|rank| rank.checked_sub(1)) {
                self.real_nodes[idx].rank = rank;
            }
        }
    }

    fn build_work_graph(&mut self) {
        self.work_nodes = self.real_nodes.clone();
        self.ordering_edges.clear();

        let real_edges = self.real_edges.clone();
        for (edge_index, edge) in real_edges.into_iter().enumerate() {
            if edge.kind == ConnectionKind::Feedback {
                self.add_feedback_locality_edges(edge);
                continue;
            }
            if self.is_history_read_cycle_edge(edge) {
                self.add_history_read_locality_edges(edge);
                continue;
            }

            let from_rank = self.real_nodes[edge.from].rank;
            let to_rank = self.real_nodes[edge.to].rank;
            if to_rank <= from_rank + 1 {
                self.ordering_edges.push(OrderingEdge {
                    from: edge.from,
                    to: edge.to,
                    weight: 1.0,
                    from_port_bias: output_port_order_bias(
                        edge.from_output,
                        self.output_counts[edge.from],
                    ),
                    to_port_bias: input_port_order_bias(
                        self.visible_input_slot(edge.to, edge.to_input),
                        self.input_slot_counts[edge.to],
                    ),
                });
                continue;
            }

            let mut previous = edge.from;
            for rank in (from_rank + 1)..to_rank {
                let dummy_idx = self.work_nodes.len();
                self.work_nodes.push(WorkNode {
                    kind: WorkNodeKind::Dummy { edge_index },
                    rank,
                    order: 0,
                    stable_order: self.real_nodes.len() + edge_index,
                    width: DUMMY_NODE_WIDTH,
                    height: 0.1,
                    is_history: false,
                    is_hidden: false,
                });
                self.ordering_edges.push(OrderingEdge {
                    from: previous,
                    to: dummy_idx,
                    weight: 1.0,
                    from_port_bias: 0.0,
                    to_port_bias: 0.0,
                });
                previous = dummy_idx;
            }
            self.ordering_edges.push(OrderingEdge {
                from: previous,
                to: edge.to,
                weight: 1.0,
                from_port_bias: 0.0,
                to_port_bias: input_port_order_bias(
                    self.visible_input_slot(edge.to, edge.to_input),
                    self.input_slot_counts[edge.to],
                ),
            });
        }

        self.rebuild_ranks();
    }

    fn add_feedback_locality_edges(&mut self, edge: LayoutEdge) {
        let from = edge.from;
        let to = edge.to;
        self.ordering_edges.push(OrderingEdge {
            from,
            to,
            weight: HISTORY_LOCALITY_WEIGHT,
            from_port_bias: 0.0,
            to_port_bias: 0.0,
        });
        self.ordering_edges.push(OrderingEdge {
            from: to,
            to: from,
            weight: HISTORY_LOCALITY_WEIGHT,
            from_port_bias: 0.0,
            to_port_bias: 0.0,
        });
    }

    fn add_history_read_locality_edges(&mut self, edge: LayoutEdge) {
        self.ordering_edges.push(OrderingEdge {
            from: edge.from,
            to: edge.to,
            weight: HISTORY_LOCALITY_WEIGHT,
            from_port_bias: output_port_order_bias(edge.from_output, self.output_counts[edge.from]),
            to_port_bias: input_port_order_bias(
                self.visible_input_slot(edge.to, edge.to_input),
                self.input_slot_counts[edge.to],
            ),
        });
        self.ordering_edges.push(OrderingEdge {
            from: edge.to,
            to: edge.from,
            weight: HISTORY_LOCALITY_WEIGHT,
            from_port_bias: 0.0,
            to_port_bias: 0.0,
        });
    }

    fn is_history_read_cycle_edge(&self, edge: LayoutEdge) -> bool {
        edge.kind == ConnectionKind::Forward
            && self.real_nodes[edge.from].is_history
            && self.real_edges.iter().any(|candidate| {
                candidate.kind == ConnectionKind::Feedback && candidate.to == edge.from
            })
    }

    fn rebuild_ranks(&mut self) {
        let max_rank = self
            .work_nodes
            .iter()
            .map(|node| node.rank)
            .max()
            .unwrap_or(0);
        self.ranks = vec![Vec::new(); max_rank + 1];
        for idx in 0..self.work_nodes.len() {
            self.ranks[self.work_nodes[idx].rank].push(idx);
        }
        for rank in &mut self.ranks {
            rank.sort_by_key(|idx| self.work_nodes[*idx].stable_order);
            for (order, idx) in rank.iter().copied().enumerate() {
                self.work_nodes[idx].order = order;
            }
        }
    }

    fn minimize_crossings(&mut self) {
        if self.ranks.len() < 2 {
            return;
        }

        for _ in 0..CROSSING_MIN_ITERATIONS {
            for rank in 1..self.ranks.len() {
                self.order_rank_by_barycenter(rank, NeighborDirection::Incoming);
            }
            for rank in (0..self.ranks.len().saturating_sub(1)).rev() {
                self.order_rank_by_barycenter(rank, NeighborDirection::Outgoing);
            }
        }
    }

    fn order_rank_by_barycenter(&mut self, rank: usize, direction: NeighborDirection) {
        let mut scored = self.ranks[rank]
            .iter()
            .copied()
            .map(|node_idx| {
                let barycenter = self.weighted_barycenter(node_idx, direction);
                (
                    node_idx,
                    barycenter.unwrap_or(self.work_nodes[node_idx].order as f32),
                    self.work_nodes[node_idx].order,
                    self.work_nodes[node_idx].stable_order,
                )
            })
            .collect::<Vec<_>>();

        scored.sort_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(&b.3))
        });

        self.ranks[rank] = scored.iter().map(|(idx, _, _, _)| *idx).collect();
        for (order, idx) in self.ranks[rank].iter().copied().enumerate() {
            self.work_nodes[idx].order = order;
        }
    }

    fn weighted_barycenter(&self, node_idx: usize, direction: NeighborDirection) -> Option<f32> {
        let node_rank = self.work_nodes[node_idx].rank;
        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        for edge in &self.ordering_edges {
            let neighbor = match direction {
                NeighborDirection::Incoming if edge.to == node_idx => edge.from,
                NeighborDirection::Outgoing if edge.from == node_idx => edge.to,
                _ => continue,
            };
            if self.work_nodes[neighbor].rank == node_rank {
                weighted_sum +=
                    neighbor_order_position(edge, neighbor, direction, &self.work_nodes)
                        * edge.weight;
                total_weight += edge.weight;
            } else if self.work_nodes[neighbor].rank.abs_diff(node_rank) == 1 {
                weighted_sum +=
                    neighbor_order_position(edge, neighbor, direction, &self.work_nodes)
                        * edge.weight;
                total_weight += edge.weight;
            }
        }

        (total_weight > 0.0).then_some(weighted_sum / total_weight)
    }

    fn assign_coordinates(&mut self, patch: &mut Patch) {
        let rank_metrics = self
            .ranks
            .iter()
            .map(|rank| rank_metrics(rank, &self.work_nodes, patch))
            .collect::<Vec<_>>();
        let widest_rank = rank_metrics
            .iter()
            .map(|metrics| metrics.width)
            .fold(0.0, f32::max);

        let mut y = VIEW_PADDING_Y;
        for (rank_idx, rank) in self.ranks.iter().enumerate() {
            let metrics = rank_metrics[rank_idx];
            let rank_offset = ((widest_rank - metrics.width) * 0.5).max(0.0);
            let mut x = VIEW_PADDING_X + rank_offset;

            let stacked_params = stacked_param_nodes(rank, &self.work_nodes, patch);
            if stacked_params.len() >= PARAM_STACK_MIN_COUNT {
                let param_stack_width = stacked_params
                    .iter()
                    .map(|idx| self.real_nodes[*idx].width)
                    .fold(0.0, f32::max);
                let mut param_y = y;
                for real_idx in stacked_params {
                    patch.nodes[real_idx].position = (x, param_y);
                    param_y += self.real_nodes[real_idx].height + PARAM_STACK_VERTICAL_GAP;
                }
                x += param_stack_width + NODE_COLUMN_GAP;
            }

            for work_idx in rank.iter().copied() {
                match self.work_nodes[work_idx].kind {
                    WorkNodeKind::Real(_) if self.work_nodes[work_idx].is_hidden => {}
                    WorkNodeKind::Real(real_idx)
                        if !is_stacked_param(real_idx, rank, &self.work_nodes, patch) =>
                    {
                        patch.nodes[real_idx].position = (x, y);
                        x += self.work_nodes[work_idx].width + NODE_COLUMN_GAP;
                    }
                    WorkNodeKind::Real(_) => {}
                    WorkNodeKind::Dummy { .. } => {
                        x += self.work_nodes[work_idx].width + NODE_COLUMN_GAP;
                    }
                }
            }
            y += metrics.height.max(LAYOUT_LAYER_SPACING);
        }

        self.refine_horizontal_positions(patch);
        self.place_local_sources_next_to_consumers(patch);
        self.resolve_rank_overlaps_by_x(patch);
        self.translate_to_positive_padding(patch);
        self.assign_generated_cable_segments(patch);
    }

    fn refine_horizontal_positions(&self, patch: &mut Patch) {
        for rank in &self.ranks {
            let stacked_params = stacked_param_nodes(rank, &self.work_nodes, patch);
            let real_rank = rank
                .iter()
                .copied()
                .filter_map(|work_idx| match self.work_nodes[work_idx].kind {
                    WorkNodeKind::Real(real_idx) if !stacked_params.contains(&real_idx) => {
                        Some(real_idx)
                    }
                    WorkNodeKind::Real(_) => None,
                    WorkNodeKind::Dummy { .. } => None,
                })
                .collect::<Vec<_>>();
            if real_rank.is_empty() {
                continue;
            }

            let ideals = real_rank
                .iter()
                .copied()
                .map(|idx| (idx, self.ideal_x_for_node(idx, patch)))
                .collect::<Vec<_>>();
            let mut next_x = if real_rank.len() == 1 {
                ideals
                    .first()
                    .map(|(_, ideal)| *ideal)
                    .unwrap_or(VIEW_PADDING_X)
            } else {
                real_rank
                    .iter()
                    .map(|idx| patch.nodes[*idx].position.0)
                    .fold(f32::INFINITY, f32::min)
            };
            for (idx, ideal) in ideals {
                let x = ideal.max(next_x);
                patch.nodes[idx].position.0 = x;
                next_x = x + self.real_nodes[idx].width + NODE_COLUMN_GAP;
            }
        }
    }

    fn ideal_x_for_node(&self, idx: usize, patch: &Patch) -> f32 {
        if self.real_nodes[idx].is_history {
            return self.ideal_x_for_history(idx, patch);
        }
        if let Some(center_x) = self.primary_chain_center_x(idx, patch) {
            return center_x - self.real_nodes[idx].width * 0.5;
        }

        let mut targets = Vec::new();
        for edge in &self.real_edges {
            if edge.kind == ConnectionKind::Feedback {
                continue;
            }
            if edge.from == idx {
                let target_x = patch.nodes[edge.to].position.0
                    + input_port_x_offset(
                        self.visible_input_slot(edge.to, edge.to_input),
                        self.input_slot_counts[edge.to],
                        self.real_nodes[edge.to].width,
                    );
                targets.push(
                    target_x
                        - output_port_x_offset(
                            edge.from_output,
                            self.output_counts[idx],
                            self.real_nodes[idx].width,
                        ),
                );
            } else if edge.to == idx {
                let source_x = patch.nodes[edge.from].position.0
                    + output_port_x_offset(
                        edge.from_output,
                        self.output_counts[edge.from],
                        self.real_nodes[edge.from].width,
                    );
                targets.push(
                    source_x
                        - input_port_x_offset(
                            self.visible_input_slot(idx, edge.to_input),
                            self.input_slot_counts[idx],
                            self.real_nodes[idx].width,
                        ),
                );
            }
        }

        if targets.is_empty() {
            patch.nodes[idx].position.0
        } else {
            targets.iter().sum::<f32>() / targets.len() as f32
        }
    }

    fn primary_chain_center_x(&self, idx: usize, patch: &Patch) -> Option<f32> {
        if matches!(patch.nodes[idx].kind, NodeKind::Param | NodeKind::Constant) {
            return None;
        }

        let preferred_incoming = self
            .real_edges
            .iter()
            .filter(|edge| {
                edge.kind == ConnectionKind::Forward
                    && edge.to == idx
                    && !self.is_history_read_cycle_edge(**edge)
                    && edge.to_input == 0
                    && is_chain_node_kind(patch.nodes[edge.from].kind)
                    && self.real_nodes[edge.from].rank < self.real_nodes[idx].rank
            })
            .min_by_key(|edge| {
                (
                    self.real_nodes[idx].rank - self.real_nodes[edge.from].rank,
                    self.real_nodes[edge.from].stable_order,
                )
            });
        if let Some(edge) = preferred_incoming {
            return Some(node_center_x(edge.from, patch, &self.real_nodes));
        }

        let incoming = self
            .real_edges
            .iter()
            .filter(|edge| {
                edge.kind == ConnectionKind::Forward
                    && edge.to == idx
                    && !self.is_history_read_cycle_edge(**edge)
                    && is_chain_node_kind(patch.nodes[edge.from].kind)
                    && self.real_nodes[edge.from].rank < self.real_nodes[idx].rank
            })
            .collect::<Vec<_>>();
        if incoming.len() == 1 {
            return Some(node_center_x(incoming[0].from, patch, &self.real_nodes));
        }

        let preferred_outgoing = self
            .real_edges
            .iter()
            .filter(|edge| {
                edge.kind == ConnectionKind::Forward
                    && edge.from == idx
                    && !self.is_history_read_cycle_edge(**edge)
                    && edge.to_input == 0
                    && is_chain_node_kind(patch.nodes[edge.to].kind)
                    && self.real_nodes[edge.to].rank > self.real_nodes[idx].rank
            })
            .min_by_key(|edge| {
                (
                    self.real_nodes[edge.to].rank - self.real_nodes[idx].rank,
                    self.real_nodes[edge.to].stable_order,
                )
            });
        preferred_outgoing.map(|edge| node_center_x(edge.to, patch, &self.real_nodes))
    }

    fn ideal_x_for_history(&self, idx: usize, patch: &Patch) -> f32 {
        let mut anchors = Vec::new();
        for edge in &self.real_edges {
            if edge.kind == ConnectionKind::Feedback && edge.to == idx {
                anchors.push(patch.nodes[edge.from].position.0);
            } else if edge.kind == ConnectionKind::Forward && edge.from == idx {
                anchors.push(patch.nodes[edge.to].position.0);
            }
        }
        if anchors.is_empty() {
            return patch.nodes[idx].position.0;
        }
        anchors.sort_by(f32::total_cmp);
        let median = anchors[anchors.len() / 2];
        (median - self.real_nodes[idx].width - NODE_COLUMN_GAP).max(VIEW_PADDING_X)
    }

    fn place_local_sources_next_to_consumers(&self, patch: &mut Patch) {
        for idx in 0..patch.nodes.len() {
            if !is_local_source_candidate(idx, patch, &self.real_edges) {
                continue;
            }

            let outgoing = self
                .real_edges
                .iter()
                .filter(|edge| {
                    edge.kind == ConnectionKind::Forward
                        && edge.from == idx
                        && !self.is_history_read_cycle_edge(**edge)
                })
                .collect::<Vec<_>>();
            let [edge] = outgoing.as_slice() else {
                continue;
            };

            let target_x = patch.nodes[edge.to].position.0
                + input_port_x_offset(
                    self.visible_input_slot(edge.to, edge.to_input),
                    self.input_slot_counts[edge.to],
                    self.real_nodes[edge.to].width,
                );
            patch.nodes[idx].position.0 = target_x
                - output_port_x_offset(
                    edge.from_output,
                    self.output_counts[idx],
                    self.real_nodes[idx].width,
                );
        }
    }

    fn resolve_rank_overlaps_by_x(&self, patch: &mut Patch) {
        for rank in &self.ranks {
            let stacked_params = stacked_param_nodes(rank, &self.work_nodes, patch);
            let mut real_rank = rank
                .iter()
                .filter_map(|work_idx| match self.work_nodes[*work_idx].kind {
                    WorkNodeKind::Real(real_idx) if !stacked_params.contains(&real_idx) => {
                        Some(real_idx)
                    }
                    WorkNodeKind::Real(_) => None,
                    WorkNodeKind::Dummy { .. } => None,
                })
                .collect::<Vec<_>>();
            real_rank.sort_by(|a, b| {
                patch.nodes[*a]
                    .position
                    .0
                    .total_cmp(&patch.nodes[*b].position.0)
                    .then_with(|| {
                        self.real_nodes[*a]
                            .stable_order
                            .cmp(&self.real_nodes[*b].stable_order)
                    })
            });

            let mut next_x = VIEW_PADDING_X;
            for idx in real_rank {
                let x = patch.nodes[idx].position.0.max(next_x);
                patch.nodes[idx].position.0 = x;
                next_x = x + self.real_nodes[idx].width + NODE_COLUMN_GAP;
            }
        }
    }

    fn translate_to_positive_padding(&self, patch: &mut Patch) {
        let min_x = patch
            .nodes
            .iter()
            .map(|node| node.position.0)
            .fold(f32::INFINITY, f32::min);
        let min_y = patch
            .nodes
            .iter()
            .map(|node| node.position.1)
            .fold(f32::INFINITY, f32::min);
        let dx = if min_x < VIEW_PADDING_X {
            VIEW_PADDING_X - min_x
        } else {
            0.0
        };
        let dy = if min_y < VIEW_PADDING_Y {
            VIEW_PADDING_Y - min_y
        } else {
            0.0
        };
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        for node in &mut patch.nodes {
            node.position.0 += dx;
            node.position.1 += dy;
        }
    }

    fn assign_generated_cable_segments(&self, patch: &mut Patch) {
        let mut id_to_idx = HashMap::new();
        for (idx, node) in patch.nodes.iter().enumerate() {
            id_to_idx.insert(node.id.as_str(), idx);
        }

        let mut forward_routes = Vec::new();
        let mut generated_segments = Vec::new();
        for (connection_idx, connection) in patch.connections.iter().enumerate() {
            if connection.segment.is_some() {
                continue;
            }

            let (Some(&from), Some(&to)) = (
                id_to_idx.get(connection.from_node.as_str()),
                id_to_idx.get(connection.to_node.as_str()),
            ) else {
                continue;
            };

            let from_position = patch.nodes[from].position;
            let to_position = patch.nodes[to].position;
            let start = (
                from_position.0
                    + output_port_x_offset(
                        connection.from_output,
                        self.output_counts[from],
                        self.real_nodes[from].width,
                    ),
                from_position.1 + self.real_nodes[from].height,
            );
            let end = (
                to_position.0
                    + input_port_x_offset(
                        self.visible_input_slot(to, connection.to_input),
                        self.input_slot_counts[to],
                        self.real_nodes[to].width,
                    ),
                to_position.1,
            );

            let segment_row = if connection.kind == ConnectionKind::Feedback {
                Some(start.1.max(end.1) + FEEDBACK_SEGMENT_ROW_OFFSET)
            } else {
                let (route_min, route_max) = segment_route_range(start, end);
                let preferred = if patch.nodes[from].kind == NodeKind::Param {
                    start.1 + PARAM_EXIT_ROW_PADDING
                } else {
                    self.best_forward_segment_row(start, end, from, to, patch)
                };
                forward_routes.push(ForwardRoute {
                    connection_idx,
                    source: from,
                    start,
                    end,
                    min_row: route_min,
                    max_row: route_max,
                    preferred_row: preferred,
                });
                None
            };
            if let Some(segment_row) = segment_row {
                generated_segments.push((connection_idx, segment_row));
            }
        }
        generated_segments.extend(self.assign_forward_segment_lanes(forward_routes));

        for (connection_idx, segment_row) in generated_segments {
            patch.connections[connection_idx].segment = Some(CableSegmentInfo {
                is_segmented: true,
                segment_row,
            });
        }
    }

    fn assign_forward_segment_lanes(&self, mut routes: Vec<ForwardRoute>) -> Vec<(usize, f32)> {
        routes.sort_by_key(|route| {
            (
                self.real_nodes[route.source].stable_order,
                route.connection_idx,
            )
        });

        let mut groups = Vec::<SourceRouteGroup>::new();
        for route in routes {
            if let Some(group) = groups.last_mut() {
                if group.source == route.source {
                    group.routes.push(route);
                    continue;
                }
            }
            groups.push(SourceRouteGroup {
                source: route.source,
                routes: vec![route],
            });
        }

        let mut lanes = Vec::<AssignedLane>::new();
        let mut assignments = Vec::new();
        for group in groups {
            let preferred = source_group_preferred_row(&group.routes);
            let min_row = group
                .routes
                .iter()
                .map(|route| route.min_row)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_row = group
                .routes
                .iter()
                .map(|route| route.max_row)
                .fold(f32::INFINITY, f32::min);
            let (span_min_x, span_max_x) = source_group_horizontal_span(&group.routes);
            let can_share_one_valid_lane = min_row <= max_row;
            let mut row = if can_share_one_valid_lane {
                preferred.clamp(min_row, max_row)
            } else {
                preferred
            };
            row = nearest_available_lane(row, min_row, max_row, span_min_x, span_max_x, &lanes);

            lanes.push(AssignedLane { row });
            for route in group.routes {
                assignments.push((route.connection_idx, row));
            }
        }
        assignments
    }

    fn best_forward_segment_row(
        &self,
        start: (f32, f32),
        end: (f32, f32),
        from: usize,
        to: usize,
        patch: &Patch,
    ) -> f32 {
        let min_x = start.0.min(end.0);
        let max_x = start.0.max(end.0);
        let min_row = start.1.min(end.1);
        let max_row = start.1.max(end.1);
        let midpoint = (start.1 + end.1) * 0.5;

        if max_row - min_row < SEGMENT_MIN_GAP {
            return midpoint;
        }

        let (route_min, route_max) = segment_route_range(start, end);
        if route_min >= route_max {
            return midpoint;
        }

        let mut blocking_ranges = Vec::new();
        for (idx, node) in patch.nodes.iter().enumerate() {
            if self.real_nodes[idx].is_hidden {
                continue;
            }
            if idx == from || idx == to {
                continue;
            }
            let node_left = node.position.0 - SEGMENT_NODE_MARGIN;
            let node_right = node.position.0 + self.real_nodes[idx].width + SEGMENT_NODE_MARGIN;
            let node_top = node.position.1 - SEGMENT_NODE_MARGIN;
            let node_bottom = node.position.1 + self.real_nodes[idx].height + SEGMENT_NODE_MARGIN;

            let x_overlaps = node_right >= min_x && node_left <= max_x;
            let row_overlaps = node_bottom >= route_min && node_top <= route_max;
            if x_overlaps && row_overlaps {
                let clipped_top = node_top.max(route_min);
                let clipped_bottom = node_bottom.min(route_max);
                if clipped_bottom > clipped_top {
                    blocking_ranges.push((clipped_top, clipped_bottom));
                }
            }
        }

        if blocking_ranges.is_empty() {
            return midpoint.clamp(route_min, route_max);
        }

        blocking_ranges.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        let mut merged = Vec::<(f32, f32)>::new();
        for (top, bottom) in blocking_ranges {
            if let Some(last) = merged.last_mut() {
                if top <= last.1 + SEGMENT_MIN_GAP {
                    last.1 = last.1.max(bottom);
                    continue;
                }
            }
            merged.push((top, bottom));
        }

        let mut gaps = Vec::new();
        let mut cursor = route_min;
        for (top, bottom) in &merged {
            if *top - cursor >= SEGMENT_MIN_GAP {
                gaps.push((cursor, *top));
            }
            cursor = cursor.max(*bottom);
        }
        if route_max - cursor >= SEGMENT_MIN_GAP {
            gaps.push((cursor, route_max));
        }

        gaps.into_iter()
            .map(|(top, bottom)| (top + bottom) * 0.5)
            .min_by(|a, b| {
                (a - midpoint)
                    .abs()
                    .total_cmp(&(b - midpoint).abs())
                    .then_with(|| a.total_cmp(b))
            })
            .unwrap_or_else(|| midpoint.clamp(route_min, route_max))
    }

    fn visible_input_slot(&self, node_idx: usize, semantic_input: usize) -> usize {
        self.input_slots[node_idx]
            .iter()
            .position(|input| *input == semantic_input)
            .unwrap_or_else(|| {
                semantic_input.min(self.input_slot_counts[node_idx].saturating_sub(1))
            })
    }
}

#[derive(Debug, Clone, Copy)]
enum NeighborDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone)]
struct ForwardRoute {
    connection_idx: usize,
    source: usize,
    start: (f32, f32),
    end: (f32, f32),
    min_row: f32,
    max_row: f32,
    preferred_row: f32,
}

#[derive(Debug)]
struct SourceRouteGroup {
    source: usize,
    routes: Vec<ForwardRoute>,
}

#[derive(Debug)]
struct AssignedLane {
    row: f32,
}

fn preferred_history_rank(writer_ranks: &[usize], consumer_ranks: &[usize]) -> Option<usize> {
    if writer_ranks.is_empty() && consumer_ranks.is_empty() {
        return None;
    }

    let mut ranks = Vec::with_capacity(writer_ranks.len() + consumer_ranks.len());
    ranks.extend_from_slice(writer_ranks);
    ranks.extend_from_slice(consumer_ranks);
    ranks.sort_unstable();
    let mut preferred = ranks[ranks.len() / 2];

    if !writer_ranks.is_empty() && !consumer_ranks.is_empty() {
        let min_consumer = consumer_ranks.iter().copied().min().unwrap();
        let max_writer = writer_ranks.iter().copied().max().unwrap();
        if min_consumer <= max_writer {
            preferred = preferred.clamp(min_consumer, max_writer);
        }
    }

    Some(preferred)
}

fn segment_route_range(start: (f32, f32), end: (f32, f32)) -> (f32, f32) {
    let min_row = start.1.min(end.1);
    let max_row = start.1.max(end.1);
    let midpoint = (start.1 + end.1) * 0.5;
    let route_min = (min_row + SEGMENT_RANGE_PADDING).min(midpoint);
    let route_max = (max_row - SEGMENT_RANGE_PADDING).max(midpoint);
    (route_min, route_max)
}

fn source_group_preferred_row(routes: &[ForwardRoute]) -> f32 {
    let mut rows = routes
        .iter()
        .map(|route| route.preferred_row)
        .collect::<Vec<_>>();
    rows.sort_by(f32::total_cmp);
    rows[rows.len() / 2]
}

fn source_group_horizontal_span(routes: &[ForwardRoute]) -> (f32, f32) {
    let min_x = routes
        .iter()
        .map(|route| route.start.0.min(route.end.0))
        .fold(f32::INFINITY, f32::min);
    let max_x = routes
        .iter()
        .map(|route| route.start.0.max(route.end.0))
        .fold(f32::NEG_INFINITY, f32::max);
    (min_x, max_x)
}

fn nearest_available_lane(
    preferred: f32,
    min_row: f32,
    max_row: f32,
    _min_x: f32,
    _max_x: f32,
    lanes: &[AssignedLane],
) -> f32 {
    let mut candidates = Vec::with_capacity(15);
    candidates.push(preferred);
    for step in 1..=7 {
        let offset = step as f32 * SEGMENT_LANE_SPACING;
        candidates.push(preferred - offset);
        candidates.push(preferred + offset);
    }

    candidates
        .into_iter()
        .map(|row| {
            if min_row <= max_row {
                row.clamp(min_row, max_row)
            } else {
                row
            }
        })
        .min_by(|a, b| {
            let a_conflicts = lane_conflict_count(*a, lanes);
            let b_conflicts = lane_conflict_count(*b, lanes);
            a_conflicts
                .cmp(&b_conflicts)
                .then_with(|| (a - preferred).abs().total_cmp(&(b - preferred).abs()))
                .then_with(|| a.total_cmp(b))
        })
        .unwrap_or(preferred)
}

fn lane_conflict_count(row: f32, lanes: &[AssignedLane]) -> usize {
    lanes
        .iter()
        .filter(|lane| (row - lane.row).abs() < SEGMENT_LANE_SPACING)
        .count()
}

fn input_port_x_offset(input_index: usize, input_count: usize, width: f32) -> f32 {
    port_x_offset(input_index, input_count, width)
}

fn output_port_x_offset(output_index: usize, output_count: usize, width: f32) -> f32 {
    port_x_offset(output_index, output_count, width)
}

fn input_port_order_bias(input_index: usize, input_count: usize) -> f32 {
    port_order_bias(input_index, input_count)
}

fn output_port_order_bias(output_index: usize, output_count: usize) -> f32 {
    port_order_bias(output_index, output_count)
}

fn port_order_bias(index: usize, count: usize) -> f32 {
    let count = count.max(1);
    if count == 1 {
        0.0
    } else {
        (index.min(count - 1) as f32 / (count - 1) as f32 - 0.5) * 0.5
    }
}

fn neighbor_order_position(
    edge: &OrderingEdge,
    neighbor: usize,
    direction: NeighborDirection,
    nodes: &[WorkNode],
) -> f32 {
    let bias = match direction {
        NeighborDirection::Incoming if neighbor == edge.from => edge.from_port_bias,
        NeighborDirection::Outgoing if neighbor == edge.to => edge.to_port_bias,
        _ => 0.0,
    };
    nodes[neighbor].order as f32 + bias
}

#[derive(Debug, Clone, Copy)]
struct RankMetrics {
    width: f32,
    height: f32,
}

fn rank_metrics(rank: &[usize], nodes: &[WorkNode], patch: &Patch) -> RankMetrics {
    let stacked_params = stacked_param_nodes(rank, nodes, patch);
    let stacks_params = stacked_params.len() >= PARAM_STACK_MIN_COUNT;
    let mut width = 0.0;
    let mut height = 0.0;
    let mut needs_gap = false;

    if stacks_params {
        width += stacked_params
            .iter()
            .map(|real_idx| nodes[*real_idx].width)
            .fold(0.0, f32::max);
        height = stacked_params
            .iter()
            .map(|real_idx| nodes[*real_idx].height)
            .sum::<f32>()
            + PARAM_STACK_VERTICAL_GAP * stacked_params.len().saturating_sub(1) as f32;
        needs_gap = true;
    }

    for work_idx in rank {
        if nodes[*work_idx].is_hidden {
            continue;
        }
        if let WorkNodeKind::Real(real_idx) = nodes[*work_idx].kind {
            if stacks_params && stacked_params.contains(&real_idx) {
                continue;
            }
        }
        if needs_gap {
            width += NODE_COLUMN_GAP;
        }
        width += nodes[*work_idx].width;
        height = height.max(nodes[*work_idx].height);
        needs_gap = true;
    }

    RankMetrics { width, height }
}

fn stacked_param_nodes(rank: &[usize], nodes: &[WorkNode], patch: &Patch) -> Vec<usize> {
    let mut params = rank
        .iter()
        .filter_map(|work_idx| match nodes[*work_idx].kind {
            WorkNodeKind::Real(real_idx) if patch.nodes[real_idx].kind == NodeKind::Param => {
                Some(real_idx)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    params.sort_by_key(|idx| nodes[*idx].stable_order);
    if params.len() >= PARAM_STACK_MIN_COUNT {
        params
    } else {
        Vec::new()
    }
}

fn is_stacked_param(real_idx: usize, rank: &[usize], nodes: &[WorkNode], patch: &Patch) -> bool {
    patch.nodes[real_idx].kind == NodeKind::Param
        && stacked_param_nodes(rank, nodes, patch).contains(&real_idx)
}

fn is_chain_node_kind(kind: NodeKind) -> bool {
    !matches!(kind, NodeKind::Param | NodeKind::Constant)
}

fn is_local_source_candidate(idx: usize, patch: &Patch, edges: &[LayoutEdge]) -> bool {
    match patch.nodes[idx].kind {
        NodeKind::Constant => true,
        NodeKind::In => {
            let incoming = edges.iter().any(|edge| edge.to == idx);
            let outgoing_count = edges
                .iter()
                .filter(|edge| edge.kind == ConnectionKind::Forward && edge.from == idx)
                .count();
            !incoming && outgoing_count == 1
        }
        _ => false,
    }
}

fn node_center_x(idx: usize, patch: &Patch, nodes: &[WorkNode]) -> f32 {
    patch.nodes[idx].position.0 + nodes[idx].width * 0.5
}
