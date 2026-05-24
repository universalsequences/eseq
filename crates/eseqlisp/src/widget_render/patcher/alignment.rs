use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::display::node_size_for_ports;
use super::geometry::{
    patch_input_indices, patch_input_slot_counts, patch_output_counts, port_x_offset,
};
use super::model::{Patch, PatchNode};
use super::state::{AlignmentGuide, AlignmentGuideKind, AlignmentSnapState};

const ALIGNMENT_ENTER_THRESHOLD_PX: f32 = 6.0;
const ALIGNMENT_ESCAPE_MULTIPLIER: f32 = 1.3;
const ALIGNMENT_SEARCH_RANGE_CELLS: f32 = 24.0;

#[derive(Clone, Copy, Debug)]
struct NodeFrame {
    position: (f32, f32),
    size: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct XAlignment {
    guide_x: f32,
    aligned_x: f32,
    min_y: f32,
    max_y: f32,
    snap_distance: f32,
    proximity: f32,
}

#[derive(Clone, Copy, Debug)]
struct YAlignment {
    guide_y: f32,
    aligned_y: f32,
    other_frame: NodeFrame,
    snap_distance: f32,
    proximity: f32,
}

pub(super) fn align_dragged_primary_position(
    patch: &Patch,
    primary_node_id: &str,
    selected_nodes: &HashSet<String>,
    raw_position: (f32, f32),
    snap_state: &mut AlignmentSnapState,
    zoom: f32,
    cell_w: f32,
    cell_h: f32,
) -> (f32, f32) {
    let Some(primary_node) = patch.nodes.iter().find(|node| node.id == primary_node_id) else {
        clear_alignment(snap_state);
        return raw_position;
    };
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let primary_frame = NodeFrame {
        position: raw_position,
        size: node_size_for_ports(
            primary_node,
            input_slot_counts.get(primary_node_id).copied().unwrap_or(0),
            output_counts.get(primary_node_id).copied().unwrap_or(0),
        ),
    };

    let x_threshold = model_threshold(
        if snap_state.snapped_x {
            ALIGNMENT_ENTER_THRESHOLD_PX * ALIGNMENT_ESCAPE_MULTIPLIER
        } else {
            ALIGNMENT_ENTER_THRESHOLD_PX
        },
        zoom,
        cell_w,
    );
    let y_threshold = model_threshold(
        if snap_state.snapped_y {
            ALIGNMENT_ENTER_THRESHOLD_PX * ALIGNMENT_ESCAPE_MULTIPLIER
        } else {
            ALIGNMENT_ENTER_THRESHOLD_PX
        },
        zoom,
        cell_h,
    );

    let mut aligned_position = raw_position;
    let mut guides = Vec::new();

    if let Some(alignment) = find_x_alignment(
        patch,
        primary_node,
        primary_frame,
        selected_nodes,
        &input_indices,
        &input_slot_counts,
        &output_counts,
        x_threshold,
    ) {
        aligned_position.0 = alignment.aligned_x;
        snap_state.snapped_x = true;
        guides.push(AlignmentGuide {
            kind: AlignmentGuideKind::Vertical,
            position: alignment.guide_x,
            extent_start: alignment.min_y,
            extent_end: alignment.max_y,
        });
    } else {
        snap_state.snapped_x = false;
    }

    if let Some(alignment) = find_y_alignment(
        patch,
        primary_frame,
        selected_nodes,
        &input_slot_counts,
        &output_counts,
        y_threshold,
    ) {
        aligned_position.1 = alignment.aligned_y;
        snap_state.snapped_y = true;
        let dragged_left = aligned_position.0;
        let dragged_right = aligned_position.0 + primary_frame.size.0;
        let other_left = alignment.other_frame.position.0;
        let other_right = alignment.other_frame.position.0 + alignment.other_frame.size.0;
        guides.push(AlignmentGuide {
            kind: AlignmentGuideKind::Horizontal,
            position: alignment.guide_y,
            extent_start: dragged_left.min(other_left),
            extent_end: dragged_right.max(other_right),
        });
    } else {
        snap_state.snapped_y = false;
    }

    snap_state.guides = guides;
    aligned_position
}

pub(super) fn clear_alignment(snap_state: &mut AlignmentSnapState) {
    snap_state.snapped_x = false;
    snap_state.snapped_y = false;
    snap_state.guides.clear();
}

fn model_threshold(screen_px: f32, zoom: f32, cell_px: f32) -> f32 {
    screen_px / (zoom.max(0.001) * cell_px.max(1.0))
}

fn find_x_alignment(
    patch: &Patch,
    primary_node: &PatchNode,
    primary_frame: NodeFrame,
    selected_nodes: &HashSet<String>,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    threshold: f32,
) -> Option<XAlignment> {
    let dragged_left = primary_frame.position.0;
    let dragged_right = primary_frame.position.0 + primary_frame.size.0;
    let dragged_top = primary_frame.position.1;
    let dragged_bottom = primary_frame.position.1 + primary_frame.size.1;
    let mut best = None;

    for other in &patch.nodes {
        if selected_nodes.contains(&other.id) {
            continue;
        }
        let other_frame = node_frame(other, input_slot_counts, output_counts);
        if !within_search_range(primary_frame, other_frame) {
            continue;
        }

        let other_left = other_frame.position.0;
        let other_right = other_frame.position.0 + other_frame.size.0;
        let other_top = other_frame.position.1;
        let other_bottom = other_frame.position.1 + other_frame.size.1;
        let min_y = dragged_top.min(other_top);
        let max_y = dragged_bottom.max(other_bottom);
        let proximity = axis_gap_y(primary_frame, other_frame);

        for (distance, guide_x, aligned_x) in [
            ((dragged_left - other_left).abs(), other_left, other_left),
            ((dragged_left - other_right).abs(), other_right, other_right),
            (
                (dragged_right - other_left).abs(),
                other_left,
                other_left - primary_frame.size.0,
            ),
            (
                (dragged_right - other_right).abs(),
                other_right,
                other_right - primary_frame.size.0,
            ),
        ] {
            if distance < threshold {
                keep_best_x_alignment(
                    &mut best,
                    XAlignment {
                        guide_x,
                        aligned_x,
                        min_y,
                        max_y,
                        snap_distance: distance,
                        proximity,
                    },
                );
            }
        }

        if let Some(alignment) = find_output_to_input_alignment(
            primary_node,
            primary_frame,
            other,
            other_frame,
            input_indices,
            input_slot_counts,
            output_counts,
            threshold,
        ) {
            keep_best_x_alignment(&mut best, alignment);
        }
        if let Some(alignment) = find_input_to_output_alignment(
            primary_node,
            primary_frame,
            other,
            other_frame,
            input_indices,
            input_slot_counts,
            output_counts,
            threshold,
        ) {
            keep_best_x_alignment(&mut best, alignment);
        }
    }

    best
}

fn find_y_alignment(
    patch: &Patch,
    primary_frame: NodeFrame,
    selected_nodes: &HashSet<String>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    threshold: f32,
) -> Option<YAlignment> {
    let dragged_top = primary_frame.position.1;
    let dragged_bottom = primary_frame.position.1 + primary_frame.size.1;
    let mut best = None;

    for other in &patch.nodes {
        if selected_nodes.contains(&other.id) {
            continue;
        }
        let other_frame = node_frame(other, input_slot_counts, output_counts);
        if !within_search_range(primary_frame, other_frame) {
            continue;
        }

        let other_top = other_frame.position.1;
        let other_bottom = other_frame.position.1 + other_frame.size.1;
        let proximity = axis_gap_x(primary_frame, other_frame);
        for (distance, guide_y, aligned_y) in [
            ((dragged_top - other_top).abs(), other_top, other_top),
            (
                (dragged_top - other_bottom).abs(),
                other_bottom,
                other_bottom,
            ),
            (
                (dragged_bottom - other_top).abs(),
                other_top,
                other_top - primary_frame.size.1,
            ),
            (
                (dragged_bottom - other_bottom).abs(),
                other_bottom,
                other_bottom - primary_frame.size.1,
            ),
        ] {
            if distance < threshold {
                keep_best_y_alignment(
                    &mut best,
                    YAlignment {
                        guide_y,
                        aligned_y,
                        other_frame,
                        snap_distance: distance,
                        proximity,
                    },
                );
            }
        }
    }

    best
}

fn find_output_to_input_alignment(
    primary_node: &PatchNode,
    primary_frame: NodeFrame,
    other_node: &PatchNode,
    other_frame: NodeFrame,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    threshold: f32,
) -> Option<XAlignment> {
    let primary_output_count = output_counts.get(&primary_node.id).copied().unwrap_or(0);
    let other_inputs = input_indices.get(&other_node.id)?;
    let other_input_count = input_slot_counts.get(&other_node.id).copied().unwrap_or(0);
    if primary_output_count == 0 || other_input_count == 0 {
        return None;
    }

    let y1 = primary_frame.position.1 + primary_frame.size.1;
    let y2 = other_frame.position.1;
    let proximity = axis_gap_y(primary_frame, other_frame);
    let mut best = None;
    for output_index in 0..primary_output_count {
        let output_offset = port_x_offset(output_index, primary_output_count, primary_frame.size.0);
        let output_x = primary_frame.position.0 + output_offset;
        for input_index in other_inputs {
            let input_x = other_frame.position.0
                + port_x_offset(*input_index, other_input_count, other_frame.size.0);
            let distance = (output_x - input_x).abs();
            if distance < threshold {
                keep_best_x_alignment(
                    &mut best,
                    XAlignment {
                        guide_x: input_x,
                        aligned_x: input_x - output_offset,
                        min_y: y1.min(y2),
                        max_y: y1.max(y2),
                        snap_distance: distance,
                        proximity,
                    },
                );
            }
        }
    }
    best
}

fn find_input_to_output_alignment(
    primary_node: &PatchNode,
    primary_frame: NodeFrame,
    other_node: &PatchNode,
    other_frame: NodeFrame,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    threshold: f32,
) -> Option<XAlignment> {
    let primary_inputs = input_indices.get(&primary_node.id)?;
    let primary_input_count = input_slot_counts
        .get(&primary_node.id)
        .copied()
        .unwrap_or(0);
    let other_output_count = output_counts.get(&other_node.id).copied().unwrap_or(0);
    if primary_input_count == 0 || other_output_count == 0 {
        return None;
    }

    let y1 = primary_frame.position.1;
    let y2 = other_frame.position.1 + other_frame.size.1;
    let proximity = axis_gap_y(primary_frame, other_frame);
    let mut best = None;
    for input_index in primary_inputs {
        let input_offset = port_x_offset(*input_index, primary_input_count, primary_frame.size.0);
        let input_x = primary_frame.position.0 + input_offset;
        for output_index in 0..other_output_count {
            let output_x = other_frame.position.0
                + port_x_offset(output_index, other_output_count, other_frame.size.0);
            let distance = (input_x - output_x).abs();
            if distance < threshold {
                keep_best_x_alignment(
                    &mut best,
                    XAlignment {
                        guide_x: output_x,
                        aligned_x: output_x - input_offset,
                        min_y: y1.min(y2),
                        max_y: y1.max(y2),
                        snap_distance: distance,
                        proximity,
                    },
                );
            }
        }
    }
    best
}

fn keep_best_x_alignment(best: &mut Option<XAlignment>, candidate: XAlignment) {
    if best.is_none_or(|current| alignment_is_better(candidate, current)) {
        *best = Some(candidate);
    }
}

fn keep_best_y_alignment(best: &mut Option<YAlignment>, candidate: YAlignment) {
    if best.is_none_or(|current| alignment_is_better(candidate, current)) {
        *best = Some(candidate);
    }
}

fn alignment_is_better(
    candidate: impl AlignmentCandidate,
    current: impl AlignmentCandidate,
) -> bool {
    match candidate.proximity().total_cmp(&current.proximity()) {
        Ordering::Equal => candidate
            .snap_distance()
            .total_cmp(&current.snap_distance())
            .is_lt(),
        ordering => ordering.is_lt(),
    }
}

trait AlignmentCandidate {
    fn proximity(&self) -> f32;
    fn snap_distance(&self) -> f32;
}

impl AlignmentCandidate for XAlignment {
    fn proximity(&self) -> f32 {
        self.proximity
    }

    fn snap_distance(&self) -> f32 {
        self.snap_distance
    }
}

impl AlignmentCandidate for YAlignment {
    fn proximity(&self) -> f32 {
        self.proximity
    }

    fn snap_distance(&self) -> f32 {
        self.snap_distance
    }
}

fn node_frame(
    node: &PatchNode,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
) -> NodeFrame {
    NodeFrame {
        position: node.position,
        size: node_size_for_ports(
            node,
            input_slot_counts.get(&node.id).copied().unwrap_or(0),
            output_counts.get(&node.id).copied().unwrap_or(0),
        ),
    }
}

fn axis_gap_x(a: NodeFrame, b: NodeFrame) -> f32 {
    let a_left = a.position.0;
    let a_right = a.position.0 + a.size.0;
    let b_left = b.position.0;
    let b_right = b.position.0 + b.size.0;
    if a_right < b_left {
        b_left - a_right
    } else if b_right < a_left {
        a_left - b_right
    } else {
        0.0
    }
}

fn axis_gap_y(a: NodeFrame, b: NodeFrame) -> f32 {
    let a_top = a.position.1;
    let a_bottom = a.position.1 + a.size.1;
    let b_top = b.position.1;
    let b_bottom = b.position.1 + b.size.1;
    if a_bottom < b_top {
        b_top - a_bottom
    } else if b_bottom < a_top {
        a_top - b_bottom
    } else {
        0.0
    }
}

fn within_search_range(dragged: NodeFrame, other: NodeFrame) -> bool {
    axis_gap_x(dragged, other).max(axis_gap_y(dragged, other)) <= ALIGNMENT_SEARCH_RANGE_CELLS
}
