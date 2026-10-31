//! Cable ties ("zip ties"). A marquee drawn over cables alone selects the
//! ones it touches and offers to tie them up; a tie gathers its
//! cables through a short bar, side by side. Cables that share two ties run
//! between them as a parallel bundle. Ties are layout only — they live in the
//! sidecar next to node positions and never reach the generated source.
//!
//! A tie stores only its centre. Its angle is derived on every layout from
//! where its cables come from and go to (the neighbouring tie or port on each
//! side), so moving a node re-aims the ties along its cables. Each leg of a
//! tied cable is a cubic that leaves a port vertically with the handle an
//! untied cable gets there, and crosses a tie along the tie's direction.
//!
//! Patcher cells are not square, so all of the angular geometry is done in an
//! isotropic space (rows scaled by the cell aspect) and mapped back to cells.
//!
//! A segmented cable stays segmented through its ties: each leg is its own
//! right-angled route, entering and leaving each tie vertically. The first leg
//! runs at the cable's `segment_row`, the last at its `after_row`, and a leg
//! between two ties at the row the earlier tie keeps for it (`leg_rows`);
//! unset rows fall where the bundle puts them.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use crate::layout::Rect;
use crate::widget_render::cable::{
    CableCurve, cable_curve, cable_y_handle, cubic_bezier_point, distance_to_segmented_cable_px,
    segmented_cable_edit_points, segmented_horizontal_segment_hit, should_render_segmented_cable,
};

use super::geometry::{
    connection_endpoints_at, patch_input_indices, patch_input_slot_counts, patch_output_counts,
    segmented_cable_row_px,
};
use super::metrics::{
    CABLE_HANDLE_DISTANCE_CELLS, CABLE_HANDLE_MAX_SPAN_FRACTION, CABLE_HIT_RADIUS_CELLS,
    SEGMENTED_CABLE_CORNER_RADIUS_CELLS,
};
use super::model::{
    CableSegmentInfo, CableTie, InputPresentation, Patch, connection_touches_hidden_inline_node,
    hidden_inline_node_ids,
};
use super::state::{
    PatcherInteractionState, SegmentPart, connection_edit_key, source_connection_id,
};

/// Distance between neighbouring cables through a tie, in unzoomed cell widths.
pub(super) const TIE_LANE_SPACING_CELLS: f32 = 0.42;
/// How far the bar overhangs its outermost cables, in unzoomed cell widths.
const TIE_BAR_OVERHANG_CELLS: f32 = 0.5;
/// How far past the bar's end its grab knob sits, in unzoomed cell widths.
const TIE_KNOB_OFFSET_CELLS: f32 = 0.55;
/// Grab tolerance around the bar, in unzoomed cell widths.
const TIE_HIT_SLACK_CELLS: f32 = 0.55;
/// A leg's handle at a tie, as a fraction of how far the leg travels along
/// the tie's direction, and its bounds in unzoomed rows. The same numbers
/// `cable_curve` sizes a port's handle with from the vertical drop, so a cord
/// bends through a tie the way it bends out of a port.
const TIE_HANDLE_FRACTION: f32 = 0.33;
const TIE_HANDLE_MIN_ROWS: f32 = 0.8;
const TIE_HANDLE_MAX_ROWS: f32 = 3.6;
/// Straight pieces each leg is cut into for hit testing and box selection.
const LEG_SAMPLES: usize = 32;
/// Leaving a port, a cable heads straight down.
const PORT_TANGENT: (f32, f32) = (0.0, 1.0);

thread_local! {
    /// Cell height over cell width. The patcher's hit tests run without the
    /// viewport in hand, and the cell metrics are fixed for a session, so the
    /// render pass and the pointer handlers record them here.
    static CELL_ASPECT: Cell<f32> = const { Cell::new(2.0) };
}

pub(super) fn note_cell_size(cell_w: f32, cell_h: f32) {
    if cell_w > 0.0 && cell_h > 0.0 {
        CELL_ASPECT.with(|aspect| aspect.set(cell_h / cell_w));
    }
}

fn cell_aspect() -> f32 {
    CELL_ASPECT.with(Cell::get)
}

/// A cable's port endpoints, in widget-local screen cells.
pub(super) type CableEnds = ((f32, f32), (f32, f32));

/// One drawn tie, in screen cells: the bar from `start` to `end`, and the
/// knob just past `end`.
pub(super) struct TieBar {
    pub(super) id: u64,
    /// How many live cables run through it.
    pub(super) cables: usize,
    pub(super) start: (f32, f32),
    pub(super) end: (f32, f32),
    pub(super) knob: (f32, f32),
}

/// Screen geometry of one scope's ties.
#[derive(Default)]
pub(super) struct TieLayout {
    /// The legs each tied cable is drawn as, outlet first.
    pub(super) routes: HashMap<String, Vec<CableCurve>>,
    /// For a tied segmented cable, the screen row of each leg's horizontal
    /// run, one per leg of its route; `None` for a leg too narrow to turn two
    /// corners, which is drawn as a curve.
    pub(super) segment_rows: HashMap<String, Vec<Option<f32>>>,
    /// The model row each of those legs runs at when nothing is authored:
    /// halfway, staggered so the bundle's runs nest instead of overlapping.
    pub(super) auto_segment_rows: HashMap<String, Vec<f32>>,
    /// The ties each tied cable runs through, outlet first: leg `i + 1`
    /// leaves tie `i`.
    pub(super) cable_ties: HashMap<String, Vec<u64>>,
    pub(super) bars: Vec<TieBar>,
}

/// A segmented cable's horizontal run under the pointer: which cable, which of
/// its runs, and the ends of the leg it belongs to, in screen cells.
pub(super) struct SegmentRun {
    pub(super) cable_id: String,
    pub(super) part: SegmentPart,
    pub(super) start: (f32, f32),
    pub(super) end: (f32, f32),
}

impl TieLayout {
    /// The legs a cable is drawn as: its route when tied, else one curve.
    pub(super) fn legs(&self, cable_id: &str, ends: CableEnds, zoom: f32) -> Vec<CableCurve> {
        self.routes
            .get(cable_id)
            .cloned()
            .unwrap_or_else(|| vec![cable_curve(ends.0, ends.1, zoom)])
    }

    /// The screen row leg `index` of a tied cable runs at, when that leg is
    /// drawn segmented.
    pub(super) fn leg_row(&self, cable_id: &str, index: usize) -> Option<f32> {
        *self.segment_rows.get(cable_id)?.get(index)?
    }

    /// Distance from `point` to a tied cable, or `None` when it is not tied.
    pub(super) fn distance_to(&self, cable_id: &str, point: (f32, f32), zoom: f32) -> Option<f32> {
        let legs = self.routes.get(cable_id)?;
        Some(
            legs.iter()
                .enumerate()
                .map(|(index, leg)| match self.leg_row(cable_id, index) {
                    Some(row) => distance_to_segmented_cable_px(
                        leg.p0,
                        leg.p3,
                        row,
                        SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom,
                        point,
                        zoom,
                    ),
                    None => distance_to_samples(&sample_leg(*leg, None).collect::<Vec<_>>(), point),
                })
                .fold(f32::MAX, f32::min),
        )
    }

    /// Endpoint edit handles of a tied cable: one along its first leg, one
    /// along its last, the usual handle distance in from each port.
    pub(super) fn edit_points(
        &self,
        cable_id: &str,
        zoom: f32,
    ) -> Option<((f32, f32), (f32, f32))> {
        let legs = self.routes.get(cable_id)?;
        let last = legs.len() - 1;
        let along = |index: usize, from_end: bool| {
            let leg = legs[index];
            let span = length(sub(leg.p3, leg.p0)).max(f32::EPSILON);
            let distance =
                (CABLE_HANDLE_DISTANCE_CELLS * zoom).min(span * CABLE_HANDLE_MAX_SPAN_FRACTION);
            if self.leg_row(cable_id, index).is_some() {
                let (from, to) = segmented_cable_edit_points(leg.p0, leg.p3, distance, zoom);
                return if from_end { to } else { from };
            }
            let t = (distance / span).min(0.5);
            cubic_bezier_point(leg, if from_end { 1.0 - t } else { t })
        };
        Some((along(0, false), along(last, true)))
    }

    /// The horizontal run of a segmented tied cable under `point`; where
    /// several are in reach, the nearest.
    pub(super) fn hit_segment_run(&self, point: (f32, f32), zoom: f32) -> Option<SegmentRun> {
        let mut ids = self.segment_rows.keys().collect::<Vec<_>>();
        ids.sort();
        ids.into_iter()
            .flat_map(|cable_id| {
                let legs = &self.routes[cable_id];
                let last = legs.len() - 1;
                (0..=last).filter_map(move |index| {
                    let row = self.leg_row(cable_id, index)?;
                    let leg = legs[index];
                    segmented_horizontal_segment_hit(
                        leg.p0,
                        leg.p3,
                        row,
                        SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom,
                        CABLE_HIT_RADIUS_CELLS * zoom,
                        point,
                    )
                    .then(|| {
                        let run = SegmentRun {
                            cable_id: cable_id.clone(),
                            part: match index {
                                0 => SegmentPart::Before,
                                index if index == last => SegmentPart::After,
                                index => SegmentPart::Between(self.cable_ties[cable_id][index - 1]),
                            },
                            start: leg.p0,
                            end: leg.p3,
                        };
                        ((point.1 - row).abs(), run)
                    })
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, run)| run)
    }

    pub(super) fn hit_bar(&self, point: (f32, f32), zoom: f32) -> Option<u64> {
        let aspect = cell_aspect();
        let slack = TIE_HIT_SLACK_CELLS * zoom;
        let point = to_iso(point, aspect);
        self.bars
            .iter()
            .rev()
            .find(|bar| {
                let start = to_iso(bar.start, aspect);
                let knob = to_iso(bar.knob, aspect);
                distance_to_segment(point, start, knob) <= slack
            })
            .map(|bar| bar.id)
    }
}

fn to_iso(point: (f32, f32), aspect: f32) -> (f32, f32) {
    (point.0, point.1 * aspect)
}

fn to_cells(point: (f32, f32), aspect: f32) -> (f32, f32) {
    (point.0, point.1 / aspect)
}

fn sub(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 - b.0, a.1 - b.1)
}

fn add_scaled(a: (f32, f32), v: (f32, f32), scale: f32) -> (f32, f32) {
    (a.0 + v.0 * scale, a.1 + v.1 * scale)
}

fn dot(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.0 + a.1 * b.1
}

fn length(v: (f32, f32)) -> f32 {
    dot(v, v).sqrt()
}

fn normalized(v: (f32, f32)) -> Option<(f32, f32)> {
    let len = length(v);
    (len > f32::EPSILON).then(|| (v.0 / len, v.1 / len))
}

/// Endpoints of every cable a tie can hold: every one drawn as a cable.
pub(super) fn tieable_cable_ends(
    patch: &Patch,
    node_rects: &HashMap<String, Rect>,
    zoom: f32,
) -> HashMap<String, CableEnds> {
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .connections
        .iter()
        .filter(|connection| connection.presentation == InputPresentation::Cable)
        .filter(|connection| !connection_touches_hidden_inline_node(connection, &hidden_node_ids))
        .filter_map(|connection| {
            let ends = connection_endpoints_at(
                connection,
                node_rects,
                &input_indices,
                &input_slot_counts,
                &output_counts,
                zoom,
            )?;
            Some((source_connection_id(connection), ends))
        })
        .collect()
}

/// The segments of the segmented cables among `patch`'s connections.
fn segmented_cables(patch: &Patch) -> HashMap<String, CableSegmentInfo> {
    patch
        .connections
        .iter()
        .filter_map(|connection| {
            let segment = connection.segment.filter(|segment| segment.is_segmented)?;
            Some((source_connection_id(connection), segment))
        })
        .collect()
}

/// Lay out `ties` over the given cable ends; `segments` holds the segmented
/// ones' runs. `origin` is the screen position of model (0, 0).
pub(super) fn tie_layout(
    ties: &[CableTie],
    ends: &HashMap<String, CableEnds>,
    segments: &HashMap<String, CableSegmentInfo>,
    origin: (f32, f32),
    zoom: f32,
) -> TieLayout {
    let aspect = cell_aspect();
    let iso_ends = ends
        .iter()
        .map(|(id, (start, end))| (id.as_str(), (to_iso(*start, aspect), to_iso(*end, aspect))))
        .collect::<HashMap<_, _>>();

    // Each tie's live members and centre.
    let mut members = HashMap::<u64, Vec<&str>>::new();
    let mut centers = HashMap::<u64, (f32, f32)>::new();
    for tie in ties {
        let mut seen = HashSet::new();
        let live = tie
            .cables
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .filter_map(|id| iso_ends.get_key_value(id.as_str()).map(|(id, _)| *id))
            .collect::<Vec<_>>();
        if live.is_empty() {
            continue;
        }
        let center = (
            origin.0 + tie.position.0 * zoom,
            origin.1 + tie.position.1 * zoom,
        );
        centers.insert(tie.id, to_iso(center, aspect));
        members.insert(tie.id, live);
    }

    // Each cable's ties, in order from its outlet to its inlet.
    let mut cable_ties = HashMap::<&str, Vec<u64>>::new();
    for tie in ties {
        for id in members.get(&tie.id).into_iter().flatten() {
            cable_ties.entry(id).or_default().push(tie.id);
        }
    }
    // Every cable that two ties share runs through them in the same order,
    // or one of the bundle's cables would double back between them and look
    // to have slipped out of a tie. The order follows the way those cables
    // flow, outlet to inlet, taken together: `a` comes first when the step
    // from `a` to `b` goes with them. On a draw the older tie comes first.
    let members_of = |tie_id: u64| members.get(&tie_id).map_or(&[][..], Vec::as_slice);
    let first_of_pair = |a: u64, b: u64| {
        let step = sub(centers[&b], centers[&a]);
        let along = members_of(a)
            .iter()
            .filter(|id| members_of(b).contains(id))
            .filter_map(|id| {
                let (start, end) = iso_ends[id];
                normalized(sub(end, start)).map(|flow| dot(step, flow))
            })
            .sum::<f32>();
        if along.abs() > f32::EPSILON { along > 0.0 } else { a < b }
    };
    for tie_ids in cable_ties.values_mut() {
        // Insertion, not `sort_by`: over three or more ties the pairwise
        // choice need not be transitive, and this settles on an order anyway.
        for index in 1..tie_ids.len() {
            let mut at = index;
            while at > 0 && first_of_pair(tie_ids[at], tie_ids[at - 1]) {
                tie_ids.swap(at, at - 1);
                at -= 1;
            }
        }
    }
    // What a cable meets on either side of one of its ties: the neighbouring
    // tie's centre, or its own port.
    let neighbours = |id: &str, tie_id: u64| {
        let (start, end) = iso_ends[id];
        let sequence = &cable_ties[id];
        let index = sequence.iter().position(|other| *other == tie_id).unwrap_or(0);
        let previous = index
            .checked_sub(1)
            .map_or(start, |previous| centers[&sequence[previous]]);
        let next = sequence
            .get(index + 1)
            .map_or(end, |next| centers[next]);
        (previous, next)
    };

    // The tie faces along its cables' average heading through it. A
    // segmented cable crosses its ties straight down.
    let directions = ties
        .iter()
        .filter_map(|tie| {
            let tie_members = members.get(&tie.id)?;
            let center = centers[&tie.id];
            let heading = tie_members.iter().fold((0.0, 0.0), |sum, id| {
                if segments.contains_key(*id) {
                    return (sum.0 + PORT_TANGENT.0, sum.1 + PORT_TANGENT.1);
                }
                // A cord pulled through a point runs along the bisector of
                // the ways it comes in and goes out, like a rope over a peg; a
                // tie dragged off the straight line between its neighbours
                // drapes the bundle over it instead of aiming past it. A
                // hairpin, where in and out cancel, falls back to the
                // straight line.
                let (previous, next) = neighbours(id, tie.id);
                let through = normalized(sub(center, previous))
                    .zip(normalized(sub(next, center)))
                    .and_then(|(into, out)| normalized((into.0 + out.0, into.1 + out.1)))
                    .or_else(|| normalized(sub(next, previous)));
                through.map_or(sum, |v| (sum.0 + v.0, sum.1 + v.1))
            });
            Some((tie.id, normalized(heading).unwrap_or(PORT_TANGENT)))
        })
        .collect::<HashMap<_, _>>();

    // Lanes follow where each cable's ports are, so a bundle does not cross
    // itself. Two cables that share two ties must sit in the same order at
    // both, or they cross in the run between them; so every tie linked to
    // another through shared cables orders its lanes along one axis, common
    // to the linked group, rather than its own (which turns as it moves, and
    // would reorder one tie's lanes but not its neighbour's).
    let mut group = directions.keys().map(|id| (*id, *id)).collect::<HashMap<_, _>>();
    fn root(group: &HashMap<u64, u64>, mut id: u64) -> u64 {
        while group[&id] != id {
            id = group[&id];
        }
        id
    }
    for tie_ids in cable_ties.values() {
        for pair in tie_ids.windows(2) {
            let (a, b) = (root(&group, pair[0]), root(&group, pair[1]));
            if a != b {
                group.insert(a, b);
            }
        }
    }
    let mut group_headings = HashMap::<u64, (f32, f32)>::new();
    for (id, direction) in &directions {
        let sum = group_headings.entry(root(&group, *id)).or_default();
        *sum = (sum.0 + direction.0, sum.1 + direction.1);
    }

    let spacing = TIE_LANE_SPACING_CELLS * zoom;
    let mut lanes = HashMap::<(&str, u64), ((f32, f32), (f32, f32))>::new();
    let mut layout = TieLayout::default();
    for tie in ties {
        let Some(tie_members) = members.get(&tie.id) else {
            continue;
        };
        let center = centers[&tie.id];
        let direction = directions[&tie.id];
        let across = (-direction.1, direction.0);
        let shared = normalized(group_headings[&root(&group, tie.id)]).unwrap_or(direction);
        let order_axis = (-shared.1, shared.0);
        let mut ordered = tie_members.clone();
        ordered.sort_by(|a, b| {
            let key = |id: &str| {
                let (start, end) = iso_ends[id];
                dot((start.0 + end.0, start.1 + end.1), order_axis)
            };
            key(a).total_cmp(&key(b)).then_with(|| a.cmp(b))
        });
        let first = -((ordered.len() - 1) as f32) * 0.5 * spacing;
        for (index, id) in ordered.iter().enumerate() {
            let lane = add_scaled(center, across, first + index as f32 * spacing);
            lanes.insert((id, tie.id), (lane, direction));
        }
        let half = -first + TIE_BAR_OVERHANG_CELLS * zoom;
        let end = add_scaled(center, across, half);
        layout.bars.push(TieBar {
            id: tie.id,
            cables: ordered.len(),
            start: to_cells(add_scaled(center, across, -half), aspect),
            end: to_cells(end, aspect),
            knob: to_cells(add_scaled(end, across, TIE_KNOB_OFFSET_CELLS * zoom), aspect),
        });
    }

    let ties_by_id = ties.iter().map(|tie| (tie.id, tie)).collect::<HashMap<_, _>>();
    // Handle lengths, in the isotropic space. At a port it is exactly what an
    // untied cable gets there: sized from the leg's vertical drop, so a
    // nearly level leg leaves its port with a short bend and runs taut. At a
    // tie it is sized from how far the leg goes on in the tie's direction: a
    // leg that heads off sideways from the tie bends close to it rather than
    // swinging wide.
    let tie_handle_min = TIE_HANDLE_MIN_ROWS * aspect * zoom;
    let tie_handle_max = TIE_HANDLE_MAX_ROWS * aspect * zoom;
    let handle = |a: (f32, f32), b: (f32, f32), tangent: (f32, f32), at_port: bool| {
        if at_port {
            let drop_rows = (b.1 - a.1).abs() / aspect / zoom;
            cable_y_handle(drop_rows) * aspect * zoom
        } else {
            (dot(sub(b, a), tangent) * TIE_HANDLE_FRACTION).clamp(tie_handle_min, tie_handle_max)
        }
    };
    for (id, tie_ids) in &cable_ties {
        let (start, end) = iso_ends[id];
        let mut points = vec![(start, PORT_TANGENT)];
        points.extend(tie_ids.iter().map(|tie_id| lanes[&(*id, *tie_id)]));
        points.push((end, PORT_TANGENT));
        let last = points.len() - 1;
        let legs = points
            .windows(2)
            .enumerate()
            .map(|(index, leg)| {
                let ((a, a_tangent), (b, b_tangent)) = (leg[0], leg[1]);
                let a_handle = handle(a, b, a_tangent, index == 0);
                let b_handle = handle(a, b, b_tangent, index + 1 == last);
                CableCurve {
                    p0: to_cells(a, aspect),
                    p1: to_cells(add_scaled(a, a_tangent, a_handle), aspect),
                    p2: to_cells(add_scaled(b, b_tangent, -b_handle), aspect),
                    p3: to_cells(b, aspect),
                }
            })
            .collect::<Vec<CableCurve>>();
        if let Some(segment) = segments.get(*id) {
            let last = legs.len() - 1;
            let auto = legs
                .iter()
                .enumerate()
                .map(|(index, leg)| {
                    // The tie this leg meets, and its lane through it.
                    let (tie_id, lane) = if index == 0 {
                        (tie_ids[0], leg.p3)
                    } else {
                        (tie_ids[index - 1], leg.p0)
                    };
                    let center = to_cells(centers[&tie_id], aspect);
                    // Turning towards the side a lane sits on, the outer lanes
                    // run nearer the tie: the runs nest without crossing. One
                    // lane's width across is one lane's height down.
                    let turn = (leg.p3.0 - leg.p0.0).signum();
                    let stagger = (center.0 - lane.0) * turn / aspect;
                    ((leg.p0.1 + leg.p3.1) * 0.5 + stagger - origin.1) / zoom
                })
                .collect::<Vec<_>>();
            let rows = legs
                .iter()
                .enumerate()
                .map(|(index, leg)| {
                    let authored = match index {
                        0 => Some(segment.segment_row),
                        index if index == last => segment.after_row,
                        index => ties_by_id
                            .get(&tie_ids[index - 1])
                            .and_then(|tie| tie.leg_rows.get(*id))
                            .copied(),
                    };
                    // A lane nearly above its inlet has no room for two
                    // rounded corners; that leg drops in as a curve.
                    let turns = should_render_segmented_cable(leg.p0, leg.p3)
                        && (leg.p3.0 - leg.p0.0).abs()
                            >= 2.0 * SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom;
                    turns.then(|| {
                        segmented_cable_row_px(
                            leg.p0,
                            leg.p3,
                            origin.1,
                            authored.unwrap_or(auto[index]),
                            zoom,
                        )
                    })
                })
                .collect();
            layout.segment_rows.insert(id.to_string(), rows);
            layout.auto_segment_rows.insert(id.to_string(), auto);
        }
        layout.cable_ties.insert(id.to_string(), tie_ids.clone());
        layout.routes.insert(id.to_string(), legs);
    }
    layout
}

/// The tie layout of `patch` as drawn in a canvas with these node rects.
pub(super) fn patch_tie_layout(
    patch: &Patch,
    node_rects: &HashMap<String, Rect>,
    origin: (f32, f32),
    zoom: f32,
) -> (HashMap<String, CableEnds>, TieLayout) {
    let ends = tieable_cable_ends(patch, node_rects, zoom);
    let layout = tie_layout(&patch.cable_ties, &ends, &segmented_cables(patch), origin, zoom);
    (ends, layout)
}

/// Points along one leg: along its curve, or along its right-angled route
/// when it runs segmented at `row`.
fn sample_leg(leg: CableCurve, row: Option<f32>) -> impl Iterator<Item = (f32, f32)> {
    (0..=LEG_SAMPLES).map(move |step| {
        let t = step as f32 / LEG_SAMPLES as f32;
        let Some(row) = row else {
            return cubic_bezier_point(leg, t);
        };
        let corners = [leg.p0, (leg.p0.0, row), (leg.p3.0, row), leg.p3];
        let spans = corners.windows(2).map(|pair| length(sub(pair[1], pair[0])));
        let total = spans.clone().sum::<f32>().max(f32::EPSILON);
        let mut remaining = t * total;
        for (pair, span) in corners.windows(2).zip(spans) {
            if remaining <= span {
                return add_scaled(pair[0], sub(pair[1], pair[0]), remaining / span.max(f32::EPSILON));
            }
            remaining -= span;
        }
        leg.p3
    })
}

/// Points along a cable, tied or not.
fn sample_cable(layout: &TieLayout, cable_id: &str, ends: CableEnds, zoom: f32) -> Vec<(f32, f32)> {
    layout
        .legs(cable_id, ends, zoom)
        .into_iter()
        .enumerate()
        .flat_map(|(index, leg)| sample_leg(leg, layout.leg_row(cable_id, index)))
        .collect()
}

fn distance_to_samples(samples: &[(f32, f32)], point: (f32, f32)) -> f32 {
    samples
        .windows(2)
        .map(|piece| distance_to_segment(point, piece[0], piece[1]))
        .fold(f32::MAX, f32::min)
}

fn distance_to_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = sub(b, a);
    let len_sq = dot(ab, ab);
    let t = if len_sq > f32::EPSILON {
        (dot(sub(point, a), ab) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    length(sub(point, add_scaled(a, ab, t)))
}

/// Tieable cables passing through `rect`, each with its control point: the
/// spot inside the box nearest the box's centre.
pub(super) fn wires_in_rect(
    ends: &HashMap<String, CableEnds>,
    layout: &TieLayout,
    rect: Rect,
    zoom: f32,
) -> Vec<(String, (f32, f32))> {
    let aspect = cell_aspect();
    let center = to_iso((rect.col + rect.width * 0.5, rect.row + rect.height * 0.5), aspect);
    let mut wires = ends
        .iter()
        .filter_map(|(id, cable_ends)| {
            sample_cable(layout, id, *cable_ends, zoom)
                .into_iter()
                .filter(|point| {
                    point.0 >= rect.col
                        && point.0 <= rect.col + rect.width
                        && point.1 >= rect.row
                        && point.1 <= rect.row + rect.height
                })
                .min_by(|a, b| {
                    length(sub(to_iso(*a, aspect), center))
                        .total_cmp(&length(sub(to_iso(*b, aspect), center)))
                })
                .map(|point| (id.clone(), point))
        })
        .collect::<Vec<_>>();
    wires.sort_by(|a, b| a.0.cmp(&b.0));
    wires
}

/// Replace `view_key`'s ties in the edit state.
fn set_scope_ties(state: &mut PatcherInteractionState, view_key: &str, ties: Vec<CableTie>) {
    state
        .edit_state
        .cable_ties
        .insert(view_key.to_string(), ties);
}

/// Gather `cables` into a new tie centred at model `position`. Returns false
/// when there is nothing to tie.
pub(super) fn add_tie(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    position: (f32, f32),
    cables: Vec<String>,
) -> bool {
    if cables.is_empty() {
        return false;
    }
    // Tying a cable up again undoes its cut.
    for cable in &cables {
        state
            .edit_state
            .cut_tie_cables
            .remove(&connection_edit_key(view_key, cable));
    }
    let mut ties = patch.cable_ties.clone();
    let id = ties.iter().map(|tie| tie.id).max().map_or(1, |id| id + 1);
    ties.push(CableTie {
        id,
        position,
        cables,
        leg_rows: Default::default(),
    });
    set_scope_ties(state, view_key, ties);
    true
}

pub(super) fn move_tie(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    tie_id: u64,
    position: (f32, f32),
) {
    let mut ties = patch.cable_ties.clone();
    if let Some(tie) = ties.iter_mut().find(|tie| tie.id == tie_id) {
        tie.position = position;
        set_scope_ties(state, view_key, ties);
    }
}

/// Set the row of `cable_id`'s run from tie `tie_id` to the next tie.
pub(super) fn set_tie_leg_row(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    tie_id: u64,
    cable_id: &str,
    row: f32,
) {
    let mut ties = patch.cable_ties.clone();
    if let Some(tie) = ties.iter_mut().find(|tie| tie.id == tie_id) {
        tie.leg_rows.insert(cable_id.to_string(), row);
        set_scope_ties(state, view_key, ties);
    }
}

/// Move several ties at once: `positions` maps tie id to model position.
pub(super) fn move_ties(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    positions: &HashMap<u64, (f32, f32)>,
) {
    if positions.is_empty() {
        return;
    }
    let mut ties = patch.cable_ties.clone();
    for tie in &mut ties {
        if let Some(position) = positions.get(&tie.id) {
            tie.position = *position;
        }
    }
    set_scope_ties(state, view_key, ties);
}

/// Ties whose centre lies inside `rect` (screen cells).
pub(super) fn ties_in_rect(layout: &TieLayout, rect: Rect) -> impl Iterator<Item = u64> + '_ {
    layout.bars.iter().filter_map(move |bar| {
        let center = ((bar.start.0 + bar.end.0) * 0.5, (bar.start.1 + bar.end.1) * 0.5);
        (center.0 >= rect.col
            && center.0 <= rect.col + rect.width
            && center.1 >= rect.row
            && center.1 <= rect.row + rect.height)
            .then_some(bar.id)
    })
}

pub(super) fn remove_tie(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
    tie_id: u64,
) -> bool {
    let mut ties = patch.cable_ties.clone();
    let before = ties.len();
    ties.retain(|tie| tie.id != tie_id);
    if ties.len() == before {
        return false;
    }
    set_scope_ties(state, view_key, ties);
    true
}

/// Ties restricted to the cables that still exist, dropping any left empty.
/// Run before persisting, so a deleted cable does not linger in the sidecar.
pub(super) fn live_ties(patch: &Patch) -> Vec<CableTie> {
    let live = patch
        .connections
        .iter()
        .map(source_connection_id)
        .collect::<HashSet<_>>();
    live_ties_among(&patch.cable_ties, &live)
}

/// `ties` restricted to the cables in `live`, dropping any left empty.
pub(super) fn live_ties_among(ties: &[CableTie], live: &HashSet<String>) -> Vec<CableTie> {
    ties.iter()
        .filter_map(|tie| {
            let cables = tie
                .cables
                .iter()
                .filter(|id| live.contains(id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let leg_rows = tie
                .leg_rows
                .iter()
                .filter(|(id, _)| live.contains(id.as_str()))
                .map(|(id, row)| (id.clone(), *row))
                .collect();
            (!cables.is_empty()).then(|| CableTie {
                cables,
                leg_rows,
                ..tie.clone()
            })
        })
        .collect()
}
