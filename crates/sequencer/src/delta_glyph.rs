//! Cohort-relative parameter-difference glyphs (`docs/delta-glyph-spec.md`, rev 2).
//!
//! This module is deliberately UI-independent. It owns schema ordering, cohort
//! deviation scales, taper-space normalization, tail aggregation, and lattice
//! assignment. Renderers consume the resulting fixed-size [`DeltaGlyph`].
//!
//! The central invariant (spec §3.4): **layout and color are cohort-level**. Every
//! tile in one palette shares a lattice, a slot→parameter map, and an accent hue per
//! group; only the deviation vector varies. A tile whose slots mean something
//! different from its neighbour's communicates nothing.

use std::cmp::Ordering;
use std::collections::BTreeMap;

pub const MAX_SLOTS: usize = 25;
pub const DIRECT_SLOTS: usize = 20;
/// Absolute dead zone, in taper units: below this we do not care how much a
/// parameter dominates the cohort (spec §4.3).
pub const ABS_GATE: f32 = 0.05;
/// Floor on the per-parameter deviation scale (spec §4.2).
pub const SCALE_FLOOR: f32 = 0.05;
/// Quantile of the cohort's deviations that maps to full scale (spec §4.2).
pub const SCALE_QUANTILE: f32 = 0.9;
pub const DEV_GAMMA: f32 = 0.65;
/// Ink cap: at most this many lit parameters, i.e. accent pieces, per glyph
/// (spec §4.6). Each becomes its own shaded layer, so this also bounds shader cost.
pub const MAX_LIT: usize = 5;

/// Lattice steps, shared with the renderer.
pub const STEP_X: f32 = 0.3672;
pub const STEP_Y: f32 = 0.3636;
/// Odd rows shift by this much in x (spec §3, from the original's `alternate.x`).
pub const STAGGER_X: f32 = 0.18;

/// Fixed smooth-union factor and cell radius. Neither varies — see spec §6.1.
/// Magnitude rides occupancy (which cells a piece claims) and luminance instead.
pub const FUSE_K: f32 = 0.155;
pub const R_CELL: f32 = 0.18;
/// The substrate's radius band, entirely inside the fusion zone so occupied cells
/// always weld rather than reading as a scatter of beads (spec §5.1).
pub const R_SUB_MIN: f32 = 0.155;
pub const R_SUB_MAX: f32 = 0.185;
/// Fraction of assigned slots the substrate occupies. Occupancy — not radius — is
/// what gives the substrate a silhouette; filling every slot produces a featureless
/// filled rectangle whichever radii you choose, because the whole band welds.
/// Ranking within the patch (rather than thresholding an absolute value) keeps the
/// fill fraction constant across instruments whose defaults sit high or low.
pub const SUBSTRATE_FILL: f32 = 0.55;

/// Two equal discs at centre distance `d` merge iff their surface gap is within
/// `0.6452 * k` — each reads gap/2 at the midpoint and the smooth-min's maximum
/// sag is `0.5*k/1.55`. Rev 2 used `gap <= k`, overestimating the budget by 1.55x.
const fn welds(distance: f32, radius: f32) -> bool {
    distance - 2.0 * radius <= 0.6452 * FUSE_K
}

/// Squared vertical/diagonal neighbour distances; `const fn` has no `sqrt`, so the
/// comparison below is done on the squared form.
const D_VERTICAL_SQ: f32 = STAGGER_X * STAGGER_X + STEP_Y * STEP_Y;

const _: () = {
    // Spec §6.0. All THREE adjacencies must weld — rev 2 checked only the
    // horizontal one and shipped a configuration where vertical fusion was
    // impossible at any deviation, which is why nothing beyond a peanut appeared.
    assert!(welds(STEP_X, R_CELL), "horizontal neighbours must weld");
    // D_VERTICAL = 0.40572; the bound below is that distance rounded up.
    assert!(D_VERTICAL_SQ < 0.4058 * 0.4058, "vertical neighbour distance moved");
    assert!(welds(0.4058, R_CELL), "vertical neighbours must weld");
    assert!(welds(0.4090, R_CELL), "diagonal neighbours must weld");
    assert!(R_SUB_MIN < R_CELL && R_CELL <= R_SUB_MAX + 0.001, "substrate sits under the accents");
    assert!(welds(STEP_X, R_SUB_MIN), "the substrate must weld at its thinnest");
};

/// A piece primitive: a lattice offset from the piece's anchor, drawn either as a
/// disc or as a capsule welding this cell to its horizontal neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prim {
    pub dcol: i8,
    pub drow: i8,
    pub capsule: bool,
}

const fn disc(dcol: i8, drow: i8) -> Prim {
    Prim { dcol, drow, capsule: false }
}

const fn cap(dcol: i8, drow: i8) -> Prim {
    Prim { dcol, drow, capsule: true }
}

/// The shape vocabulary (spec §6.1), five tiers of three variants. Tier is chosen by
/// deviation magnitude, variant by slot index — so each parameter has a characteristic
/// growth shape and no hash is involved. Capsules appear in 6 of the 15 entries (40%,
/// against the original's ~26%); they are the source of the elongated lobes.
pub const PIECES: [[&[Prim]; 3]; 5] = [
    // 1 cell
    [&[disc(0, 0)], &[disc(0, 0)], &[disc(0, 0)]],
    // 2 cells: capsule / vertical pair / diagonal pair
    [&[cap(0, 0)], &[disc(0, 0), disc(0, 1)], &[disc(0, 0), disc(1, 1)]],
    // 3 cells: capsule + disc / L / vertical run
    [
        &[cap(0, 0), disc(0, 1)],
        &[disc(0, 0), disc(0, 1), disc(1, 1)],
        &[disc(0, 0), disc(0, 1), disc(0, 2)],
    ],
    // 4 cells: stacked capsules / 2x2 / capsule + 2 discs
    [
        &[cap(0, 0), cap(0, 1)],
        &[disc(0, 0), disc(0, 1), disc(1, 0), disc(1, 1)],
        &[cap(0, 0), disc(0, 1), disc(1, 1)],
    ],
    // 5 cells
    [
        &[cap(0, 0), cap(0, 1), disc(0, 2)],
        &[disc(0, 0), disc(0, 1), disc(0, 2), disc(1, 0), disc(1, 1)],
        &[cap(0, 0), disc(0, 1), disc(1, 1), disc(1, 2)],
    ],
];

/// Encoded piece id: `tier * 3 + variant`, 0..14.
pub fn piece_id(tier: usize, variant: usize) -> u8 {
    (tier.min(4) * 3 + variant.min(2)) as u8
}

pub fn piece_prims(id: u8) -> &'static [Prim] {
    let id = (id as usize).min(14);
    PIECES[id / 3][id % 3]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Continuous,
    Discrete,
    Boolean,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamTaper {
    Linear,
    Log,
    Exponential(f32),
    Stepped(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParamGroup {
    Osc,
    Filter,
    Env,
    Mod,
    Fx,
    Mix,
    Other(String),
}

impl ParamGroup {
    fn rank(&self) -> u8 {
        match self {
            Self::Osc => 0,
            Self::Filter => 1,
            Self::Env => 2,
            Self::Mod => 3,
            Self::Fx => 4,
            Self::Mix => 5,
            Self::Other(_) => 6,
        }
    }

    /// Palette index for this group, dual-maintained with `DG_HUES` in the shader.
    /// Named groups take fixed slots; free-form groups take the remainder by sorted
    /// position, so adding a parameter to an existing group never re-colors a glyph.
    pub fn palette_index(&self, groups: &[ParamGroup]) -> u8 {
        match self {
            Self::Osc => 0,
            Self::Filter => 1,
            Self::Env => 2,
            Self::Mod => 3,
            Self::Fx => 4,
            Self::Mix => 5,
            Self::Other(_) => {
                let position = groups
                    .iter()
                    .filter(|group| matches!(group, Self::Other(_)))
                    .position(|group| group == self)
                    .unwrap_or(0);
                // Slot 6 is the neutral "unclassified" tone; only distinct extra
                // groups beyond the first reach back into the named hues.
                if position == 0 { 6 } else { ((position - 1) % 6) as u8 }
            }
        }
    }
}

/// Mutually distinguishable at ~20px on a dark tile, plus a deliberately un-hued
/// slot 6 for "unclassified". Dual-maintained with `DG_HUES` in the Metal shader.
pub const GROUP_PALETTE: [[f32; 4]; 7] = [
    [0.98, 0.72, 0.22, 1.0], // 0 osc — amber
    [0.95, 0.30, 0.62, 1.0], // 1 filter — magenta
    [0.55, 0.95, 0.30, 1.0], // 2 env — lime
    [0.25, 0.80, 0.95, 1.0], // 3 mod — cyan
    [0.62, 0.45, 0.98, 1.0], // 4 fx — violet
    [0.96, 0.46, 0.28, 1.0], // 5 mix — coral
    [0.56, 0.57, 0.60, 1.0], // 6 unclassified — neutral
];

#[derive(Clone, Debug)]
pub struct ParamSchema {
    pub id: String,
    pub kind: ParamKind,
    pub range: (f32, f32),
    pub taper: ParamTaper,
    pub group: ParamGroup,
    pub order: usize,
    pub link: Option<String>,
    pub visible: bool,
    pub audio: bool,
    pub default: f32,
    pub weight: f32,
}

/// One lit parameter: a welded polyomino anchored at its slot, its own shaded layer.
#[derive(Clone, Debug, PartialEq)]
pub struct DeltaGlyphPiece {
    pub slot: usize,
    /// `tier * 3 + variant`, see [`PIECES`].
    pub piece: u8,
    /// Index into the shared hue palette, dual-maintained with the shader.
    pub hue: u8,
    /// Luminance magnitude, 0..7 — the continuous channel between piece tiers.
    pub magnitude: u8,
    /// Grow the piece leftward instead of rightward, so anchors in the right half
    /// of the lattice keep their cells on-tile.
    pub mirror: bool,
    /// Deviation below the reference; drives a cool hue shift (spec §6.2).
    pub negative: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeltaGlyph {
    pub cols: usize,
    pub rows: usize,
    /// Per slot, 0 = unassigned, else 1..15 mapping onto the substrate radius band.
    pub substrate: Vec<u8>,
    pub pieces: Vec<DeltaGlyphPiece>,
    /// True for the anchor tile: substrate only, no accents.
    pub anchor: bool,
}

/// One lattice slot's plan: which schema parameters it draws, fixed for the cohort.
#[derive(Clone, Debug)]
struct SlotPlan {
    members: Vec<usize>,
    group: ParamGroup,
    aggregate: bool,
    link: Option<String>,
}

/// Cohort-level model: deviation scales, lattice layout, and accent colors, all
/// resolved once. `build` then fills in one subject's deviations against a layout
/// that is identical for every tile.
pub struct DeltaGlyphCohort<'a> {
    schema: &'a [ParamSchema],
    /// Per schema index; only meaningful for indices present in `layout`.
    scales: Vec<f32>,
    reference: Vec<f32>,
    cols: usize,
    rows: usize,
    layout: Vec<Option<SlotPlan>>,
    /// Per slot, the palette index of its group. Resolved once so a hue means the
    /// same group on every tile in the cohort.
    hues: Vec<u8>,
}

impl<'a> DeltaGlyphCohort<'a> {
    /// `cohort` is every compatible patch's value vector, in stable palette order.
    /// `reference` is the anchor (spec §7) — normally `cohort[0]`.
    pub fn new(schema: &'a [ParamSchema], cohort: &[Vec<f32>], reference: &[f32]) -> Self {
        let reference = reference.to_vec();
        let scales = schema
            .iter()
            .enumerate()
            .map(|(index, param)| deviation_scale(param, index, cohort, &reference))
            .collect::<Vec<f32>>();

        // Visibility is a cohort-level decision. Deciding it per subject (rev 1)
        // gives tiles different lattices, which makes them incomparable.
        let mut visible = schema
            .iter()
            .enumerate()
            .filter(|(index, param)| {
                param.visible && param.audio && !is_dead(param, *index, cohort)
            })
            .map(|(index, param)| SlotPlan {
                members: vec![index],
                group: param.group.clone(),
                aggregate: false,
                link: param.link.clone(),
            })
            .collect::<Vec<_>>();
        visible.sort_by(|left, right| {
            let (a, b) = (&schema[left.members[0]], &schema[right.members[0]]);
            a.group
                .rank()
                .cmp(&b.group.rank())
                .then_with(|| a.group.cmp(&b.group))
                .then_with(|| a.order.cmp(&b.order))
                .then_with(|| a.id.cmp(&b.id))
        });
        let plans = aggregate_tail(visible);
        let (cols, rows, layout) = assign_lattice(plans);

        // Hue is per group and resolved cohort-wide, so it is a legend rather than
        // decoration (spec §5.3).
        let distinct = layout
            .iter()
            .flatten()
            .map(|plan| plan.group.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let hues = layout
            .iter()
            .map(|plan| plan.as_ref().map_or(0, |plan| plan.group.palette_index(&distinct)))
            .collect();

        Self { schema, scales, reference, cols, rows, layout, hues }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Which lattice slots carry a parameter. Cohort-level and therefore identical
    /// for every tile — unlike substrate *occupancy*, which varies per patch.
    pub fn assigned(&self) -> Vec<bool> {
        self.layout.iter().map(|plan| plan.is_some()).collect()
    }

    pub fn build(&self, subject: &[f32], anchor: bool) -> DeltaGlyph {
        let mut deviations = vec![0.0f32; self.layout.len()];
        let mut signs = vec![0i8; self.layout.len()];
        let mut substrate = vec![0u8; self.layout.len()];
        let mut absolutes: Vec<(usize, f32)> = Vec::new();
        for (slot, plan) in self.layout.iter().enumerate() {
            let Some(plan) = plan else { continue };
            let (deviation, sign) =
                plan_deviation(self.schema, &self.scales, plan, subject, &self.reference);
            deviations[slot] = deviation;
            signs[slot] = sign;
            // The substrate reads ABSOLUTE values, so it is a property of the patch
            // rather than of the comparison, and it is drawn even on the anchor tile.
            absolutes.push((slot, plan_absolute(self.schema, plan, subject)));
        }

        // Occupancy is the substrate's shape channel: take the highest-valued
        // SUBSTRATE_FILL of this patch's slots, so the mass is an irregular
        // polyomino with real negative space rather than a filled rectangle.
        let occupied = (absolutes.len() as f32 * SUBSTRATE_FILL).round() as usize;
        absolutes.sort_by(|(slot_a, a), (slot_b, b)| {
            b.partial_cmp(a).unwrap_or(Ordering::Equal).then(slot_a.cmp(slot_b))
        });
        for (slot, absolute) in absolutes.into_iter().take(occupied.max(1)) {
            // Radius still varies within the fusion band, but only as surface
            // texture — the silhouette comes from which slots are occupied.
            substrate[slot] = 1 + (absolute.clamp(0.0, 1.0) * 14.0).round() as u8;
        }

        apply_ink_cap(&mut deviations);

        let pieces = if anchor {
            Vec::new()
        } else {
            self.layout
                .iter()
                .enumerate()
                .filter_map(|(slot, plan)| {
                    plan.as_ref()?;
                    let deviation = deviations[slot];
                    if deviation <= 0.0 {
                        return None;
                    }
                    let tier = ((deviation * 5.0) as usize).min(4);
                    Some(DeltaGlyphPiece {
                        slot,
                        piece: piece_id(tier, slot % 3),
                        hue: self.hues[slot],
                        magnitude: (deviation.clamp(0.0, 1.0) * 7.0).round() as u8,
                        mirror: slot / self.rows.max(1) * 2 >= self.cols,
                        negative: signs[slot] < 0,
                    })
                })
                .collect()
        };

        DeltaGlyph { cols: self.cols, rows: self.rows, substrate, pieces, anchor }
    }
}

/// Convenience entry point for callers building only one glyph.
pub fn build_delta_glyph(
    schema: &[ParamSchema],
    subject: &[f32],
    reference: &[f32],
    cohort: &[Vec<f32>],
    anchor: bool,
) -> DeltaGlyph {
    DeltaGlyphCohort::new(schema, cohort, reference).build(subject, anchor)
}

/// Spec §4.2: the scale is a high quantile of the cohort's *deviations from the
/// reference*, not a spread of its values. A MAD of values (rev 1) collapses to zero
/// whenever most of the cohort is identical, which pins every parameter to the floor
/// and makes the one differing patch saturate.
fn deviation_scale(
    param: &ParamSchema,
    index: usize,
    cohort: &[Vec<f32>],
    reference: &[f32],
) -> f32 {
    let anchor = taper_value(param, value_at(reference, index, param.default));
    let mut deviations = cohort
        .iter()
        .map(|subject| (taper_value(param, value_at(subject, index, param.default)) - anchor).abs())
        .collect::<Vec<f32>>();
    quantile(&mut deviations, SCALE_QUANTILE).max(SCALE_FLOOR)
}

/// A parameter is dead when no patch in the cohort moves it off the instrument
/// default. Evaluated over the cohort, never over one subject.
fn is_dead(param: &ParamSchema, index: usize, cohort: &[Vec<f32>]) -> bool {
    let default = taper_value(param, param.default);
    cohort.iter().all(|subject| {
        (taper_value(param, value_at(subject, index, param.default)) - default).abs()
            <= f32::EPSILON
    })
}

/// One slot's deviation. Aggregate slots RMS their members; the sign is taken from
/// the largest-magnitude member so it stays meaningful under aggregation.
fn plan_deviation(
    schema: &[ParamSchema],
    scales: &[f32],
    plan: &SlotPlan,
    subject: &[f32],
    reference: &[f32],
) -> (f32, i8) {
    let mut sum_squares = 0.0f32;
    let mut dominant = (0.0f32, 0i8);
    for &index in &plan.members {
        let (deviation, sign) =
            param_deviation(&schema[index], index, scales[index], subject, reference);
        sum_squares += deviation * deviation;
        if deviation > dominant.0 {
            dominant = (deviation, sign);
        }
    }
    let count = plan.members.len().max(1) as f32;
    ((sum_squares / count).sqrt(), dominant.1)
}

/// Mean absolute taper value over a slot's members — the substrate's input.
fn plan_absolute(schema: &[ParamSchema], plan: &SlotPlan, subject: &[f32]) -> f32 {
    let sum: f32 = plan
        .members
        .iter()
        .map(|&index| taper_value(&schema[index], value_at(subject, index, schema[index].default)))
        .sum();
    sum / plan.members.len().max(1) as f32
}

fn param_deviation(
    param: &ParamSchema,
    index: usize,
    scale: f32,
    subject: &[f32],
    reference: &[f32],
) -> (f32, i8) {
    let subject_raw = value_at(subject, index, param.default);
    let reference_raw = value_at(reference, index, param.default);
    let (relative, sign) = match param.kind {
        ParamKind::Continuous => {
            let difference =
                taper_value(param, subject_raw) - taper_value(param, reference_raw);
            // Spec §4.3: the absolute gate rejects inaudible differences even when
            // they dominate the cohort. The relative scale then decides loudness.
            if difference.abs() < ABS_GATE {
                return (0.0, 0);
            }
            (
                difference.abs() / scale.max(f32::EPSILON),
                if difference > 0.0 { 1 } else { -1 },
            )
        }
        // No magnitude exists for a waveform swap: any change is full scale.
        ParamKind::Discrete | ParamKind::Boolean => {
            ((subject_raw != reference_raw) as u8 as f32, 0)
        }
    };
    let weighted = relative * param.weight.max(0.0);
    (weighted.clamp(0.0, 1.0).powf(DEV_GAMMA), sign)
}

/// Spec §4.6: keep the `MAX_LIT` strongest deviations, zero the rest. Ties break by
/// slot order so the cut is deterministic.
fn apply_ink_cap(deviations: &mut [f32]) {
    let mut lit = deviations
        .iter()
        .enumerate()
        .filter(|(_, deviation)| **deviation > 0.0)
        .map(|(slot, deviation)| (slot, *deviation))
        .collect::<Vec<_>>();
    if lit.len() <= MAX_LIT {
        return;
    }
    lit.sort_by(|(slot_a, a), (slot_b, b)| {
        b.partial_cmp(a).unwrap_or(Ordering::Equal).then(slot_a.cmp(slot_b))
    });
    for (slot, _) in lit.drain(MAX_LIT..) {
        deviations[slot] = 0.0;
    }
}

fn value_at(values: &[f32], index: usize, default: f32) -> f32 {
    values.get(index).copied().filter(|value| value.is_finite()).unwrap_or(default)
}

fn taper_value(param: &ParamSchema, value: f32) -> f32 {
    let (min, max) = param.range;
    let range = max - min;
    if !range.is_finite() || range <= 0.0 {
        return 0.0;
    }
    let linear = ((value - min) / range).clamp(0.0, 1.0);
    match param.taper {
        ParamTaper::Linear => linear,
        ParamTaper::Log if min > 0.0 && max > min => {
            ((value.clamp(min, max).ln() - min.ln()) / (max.ln() - min.ln())).clamp(0.0, 1.0)
        }
        ParamTaper::Exponential(k) if k.is_finite() && k > 0.0 => linear.powf(1.0 / k),
        ParamTaper::Stepped(count) if count > 1 => {
            (linear * (count - 1) as f32).round() / (count - 1) as f32
        }
        _ => linear,
    }
}

/// Linear-interpolated quantile. Small cohorts make index-selection too coarse.
fn quantile(values: &mut [f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let position = quantile.clamp(0.0, 1.0) * (values.len() - 1) as f32;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f32;
    values[lower] + (values[upper] - values[lower]) * fraction
}

fn aggregate_tail(mut plans: Vec<SlotPlan>) -> Vec<SlotPlan> {
    if plans.len() <= MAX_SLOTS {
        return plans;
    }
    let tail = plans.split_off(DIRECT_SLOTS);
    let mut grouped: BTreeMap<ParamGroup, Vec<SlotPlan>> = BTreeMap::new();
    for plan in tail {
        grouped.entry(plan.group.clone()).or_default().push(plan);
    }
    let available = MAX_SLOTS - plans.len();
    let mut groups = grouped.into_iter().collect::<Vec<_>>();
    if groups.len() > available {
        let overflow = groups.split_off(available - 1);
        let members = overflow.into_iter().flat_map(|(_, members)| members).collect();
        groups.push((ParamGroup::Other("tail".to_string()), members));
    }

    // One aggregate per group is the semantic minimum. Use otherwise-empty tail
    // capacity to split large groups into contiguous RMS chunks: collapsing sixty
    // ungrouped controls into one cell throws away everything the glyph is for.
    let mut allocations = vec![1usize; groups.len()];
    while allocations.iter().sum::<usize>() < available {
        let Some(index) = groups
            .iter()
            .enumerate()
            .filter(|(index, (_, members))| allocations[*index] < members.len())
            .max_by_key(|(index, (_, members))| members.len().div_ceil(allocations[*index]))
            .map(|(index, _)| index)
        else {
            break;
        };
        allocations[index] += 1;
    }
    for ((group, members), chunks) in groups.into_iter().zip(allocations) {
        let chunk_size = members.len().div_ceil(chunks);
        for chunk in members.chunks(chunk_size) {
            plans.push(SlotPlan {
                members: chunk.iter().flat_map(|plan| plan.members.clone()).collect(),
                group: group.clone(),
                aggregate: true,
                link: None,
            });
        }
    }
    plans.truncate(MAX_SLOTS);
    plans
}

fn dimensions(count: usize) -> (usize, usize) {
    if count == 0 {
        return (1, 1);
    }
    let cols = ((count as f32 * 1.15).sqrt().ceil() as usize).clamp(1, 5);
    let rows = count.div_ceil(cols).clamp(1, 5);
    (cols, rows)
}

fn slot_position(slot: usize, rows: usize) -> (usize, usize) {
    let col = slot / rows;
    let sequence_row = slot % rows;
    let row = if col % 2 == 0 { sequence_row } else { rows - sequence_row - 1 };
    (col, row)
}

fn assign_lattice(plans: Vec<SlotPlan>) -> (usize, usize, Vec<Option<SlotPlan>>) {
    let (cols, rows) = dimensions(plans.len());
    let capacity = cols * rows;
    let mut assigned = Vec::with_capacity(capacity);
    let mut previous_group: Option<ParamGroup> = None;
    for plan in plans {
        if previous_group.as_ref().is_some_and(|group| group != &plan.group) {
            let remainder = rows - assigned.len() % rows;
            if remainder <= 1 && assigned.len() + remainder < capacity {
                assigned.extend((0..remainder).map(|_| None));
            }
        }
        if assigned.len() == capacity {
            break;
        }
        previous_group = Some(plan.group.clone());
        assigned.push(Some(plan));
    }
    assigned.resize_with(capacity, || None);
    // Storage order is lattice slot order. Geometry performs the odd-column row
    // reversal, yielding column-major boustrophedon traversal.
    (cols, rows, assigned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(id: &str, kind: ParamKind, group: ParamGroup) -> ParamSchema {
        ParamSchema {
            id: id.to_string(),
            kind,
            range: (0.0, 1.0),
            taper: ParamTaper::Linear,
            group,
            order: 0,
            link: None,
            visible: true,
            audio: true,
            default: 0.0,
            weight: 1.0,
        }
    }

    /// Piece tier is monotone in deviation, so tier is a usable proxy for it.
    fn tier(glyph: &DeltaGlyph, slot: usize) -> Option<usize> {
        glyph.pieces.iter().find(|piece| piece.slot == slot).map(|piece| piece.piece as usize / 3)
    }

    fn magnitude(glyph: &DeltaGlyph, slot: usize) -> Option<u8> {
        glyph.pieces.iter().find(|piece| piece.slot == slot).map(|piece| piece.magnitude)
    }

    #[test]
    fn absolute_gate_rejects_tiny_cohort_differences() {
        let schema = vec![param("cutoff", ParamKind::Continuous, ParamGroup::Filter)];
        let cohort = vec![vec![0.5], vec![0.501]];
        let glyph = build_delta_glyph(&schema, &[0.501], &[0.5], &cohort, false);
        assert!(glyph.pieces.is_empty());
        assert!(glyph.substrate.iter().any(|level| *level > 0), "the substrate still draws");
    }

    /// The rev 1 regression: a cohort of near-identical patches plus one variant
    /// pinned every scale to the floor, so the variant saturated and everyone else
    /// went dark. The corrected scale must grade them.
    #[test]
    fn mostly_identical_cohort_grades_rather_than_saturating() {
        let schema = vec![param("cutoff", ParamKind::Continuous, ParamGroup::Filter)];
        let cohort = vec![vec![0.50], vec![0.50], vec![0.50], vec![0.65], vec![0.95]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let mild = magnitude(&model.build(&cohort[3], false), 0).unwrap();
        let wild = magnitude(&model.build(&cohort[4], false), 0).unwrap();
        assert!(mild < wild, "magnitude must be ordered, not all-or-nothing");
        assert!(mild < 6, "a mid difference must not saturate: {mild}");
    }

    /// Spec §3.4. Rev 1 chose visible parameters from the subject, so tiles in one
    /// palette disagreed about the lattice itself.
    #[test]
    fn layout_is_identical_for_every_subject_in_a_cohort() {
        let schema = vec![
            param("a", ParamKind::Continuous, ParamGroup::Osc),
            param("b", ParamKind::Continuous, ParamGroup::Filter),
            param("c", ParamKind::Continuous, ParamGroup::Env),
        ];
        let cohort = vec![vec![0.0, 0.0, 0.0], vec![0.9, 0.0, 0.0], vec![0.0, 0.9, 0.9]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let glyphs = cohort.iter().map(|subject| model.build(subject, false)).collect::<Vec<_>>();
        for glyph in &glyphs {
            assert_eq!(glyph.cols, glyphs[0].cols);
            assert_eq!(glyph.rows, glyphs[0].rows);
            assert_eq!(glyph.substrate.len(), glyphs[0].substrate.len());
        }
        // The slot->parameter map is cohort-level; substrate occupancy is not, and
        // is expected to differ per patch (spec §5.1).
        assert_eq!(model.assigned().iter().filter(|a| **a).count(), 3);
    }

    /// Spec §5.3: a hue means the same group on every tile, so it is a legend.
    #[test]
    fn hue_is_stable_per_group_across_the_cohort() {
        let schema = vec![
            param("a", ParamKind::Continuous, ParamGroup::Osc),
            param("b", ParamKind::Continuous, ParamGroup::Filter),
        ];
        let cohort = vec![vec![0.0, 0.0], vec![0.9, 0.0], vec![0.0, 0.9]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let osc = model.build(&cohort[1], false).pieces[0].hue;
        let filter = model.build(&cohort[2], false).pieces[0].hue;
        assert_eq!(osc, 0, "osc keeps its named palette slot");
        assert_eq!(filter, 1, "filter keeps its named palette slot");
    }

    /// Spec §6.1: deviation selects a piece TIER, so a bigger difference claims
    /// more cells. Rev 2 varied radius instead, which is what forbade blobs.
    #[test]
    fn deviation_grows_the_piece_rather_than_the_radius() {
        let schema = vec![param("cutoff", ParamKind::Continuous, ParamGroup::Filter)];
        let cohort = vec![vec![0.0], vec![0.25], vec![0.6], vec![1.0]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let tiers = cohort[1..]
            .iter()
            .map(|subject| tier(&model.build(subject, false), 0).unwrap())
            .collect::<Vec<_>>();
        assert!(tiers.windows(2).all(|pair| pair[0] <= pair[1]), "tiers monotone: {tiers:?}");
        assert!(tiers[2] > tiers[0], "the largest difference must claim more cells");
    }

    /// Every piece must be contiguous, or it cannot weld into one blob — which was
    /// rev 2's core defect (no adjacency requirement at all).
    #[test]
    fn every_piece_in_the_vocabulary_is_contiguous() {
        for id in 0..15u8 {
            let prims = piece_prims(id);
            let cells = prims
                .iter()
                .flat_map(|prim| {
                    let mut cells = vec![(prim.dcol, prim.drow)];
                    if prim.capsule {
                        cells.push((prim.dcol + 1, prim.drow));
                    }
                    cells
                })
                .collect::<std::collections::BTreeSet<_>>();
            let mut reached = std::collections::BTreeSet::from([*cells.iter().next().unwrap()]);
            loop {
                let grown = cells
                    .iter()
                    .filter(|(col, row)| {
                        reached.iter().any(|(rc, rr): &(i8, i8)| {
                            (rc - col).abs() + (rr - row).abs() == 1
                                || ((rc - col).abs() == 1 && (rr - row).abs() == 1)
                        })
                    })
                    .copied()
                    .collect::<Vec<_>>();
                let before = reached.len();
                reached.extend(grown);
                if reached.len() == before {
                    break;
                }
            }
            assert_eq!(reached.len(), cells.len(), "piece {id} is disconnected: {cells:?}");
            assert!(prims.len() <= 5, "piece {id} exceeds the primitive budget");
        }
    }

    #[test]
    fn capsules_are_a_normal_outcome_not_a_marker() {
        let with_capsule = (0..15u8)
            .filter(|id| piece_prims(*id).iter().any(|prim| prim.capsule))
            .count();
        assert!(with_capsule >= 6, "capsules appeared in only {with_capsule}/15 pieces");
    }

    #[test]
    fn anchor_tile_is_substrate_only() {
        let schema = vec![
            param("a", ParamKind::Continuous, ParamGroup::Osc),
            param("b", ParamKind::Continuous, ParamGroup::Filter),
        ];
        let cohort = vec![vec![0.1, 0.2], vec![0.9, 0.8]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let glyph = model.build(&cohort[0], true);
        assert!(glyph.anchor);
        assert!(glyph.pieces.is_empty());
        assert!(glyph.substrate.iter().any(|level| *level > 0));
    }

    /// Spec §5.1: identical patches share a substrate, different ones do not — the
    /// character that rev 2's uniform address grid threw away.
    #[test]
    fn substrate_tracks_the_absolute_parameter_vector() {
        let schema = vec![
            param("a", ParamKind::Continuous, ParamGroup::Osc),
            param("b", ParamKind::Continuous, ParamGroup::Filter),
        ];
        let cohort = vec![vec![0.1, 0.9], vec![0.1, 0.9], vec![0.9, 0.1]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        let twin = model.build(&cohort[1], false).substrate;
        assert_eq!(model.build(&cohort[0], false).substrate, twin, "same values, same form");
        assert_ne!(model.build(&cohort[2], false).substrate, twin, "different values, different form");
    }

    #[test]
    fn discrete_change_is_deliberately_loud() {
        let schema = vec![param("wave", ParamKind::Discrete, ParamGroup::Osc)];
        let glyph = build_delta_glyph(&schema, &[1.0], &[0.0], &[vec![0.0], vec![1.0]], false);
        assert_eq!(tier(&glyph, 0), Some(4), "a structural change claims the largest piece");
        assert!(!glyph.pieces[0].negative, "discrete changes have no sign");
    }

    #[test]
    fn dead_parameters_are_dropped_over_the_whole_cohort() {
        let schema = vec![
            param("dead", ParamKind::Continuous, ParamGroup::Osc),
            param("live", ParamKind::Continuous, ParamGroup::Filter),
        ];
        let cohort = vec![vec![0.0, 0.2], vec![0.0, 0.9]];
        let model = DeltaGlyphCohort::new(&schema, &cohort, &cohort[0]);
        assert_eq!(model.assigned().iter().filter(|a| **a).count(), 1);
    }

    #[test]
    fn ink_cap_bounds_accent_pieces() {
        let schema = (0..20)
            .map(|i| param(&format!("p{i}"), ParamKind::Continuous, ParamGroup::Fx))
            .collect::<Vec<_>>();
        let reference = vec![0.0; 20];
        let subject = (0..20).map(|i| 0.2 + i as f32 * 0.04).collect::<Vec<f32>>();
        let glyph = build_delta_glyph(
            &schema, &subject, &reference,
            &[reference.clone(), subject.clone()], false,
        );
        assert_eq!(glyph.pieces.len(), MAX_LIT);
    }

    #[test]
    fn oversized_schemas_stay_within_five_by_five() {
        let schema = (0..50)
            .map(|i| param(&format!("p{i}"), ParamKind::Continuous, ParamGroup::Fx))
            .collect::<Vec<_>>();
        let subject = vec![1.0; 50];
        let reference = vec![0.0; 50];
        let glyph = build_delta_glyph(
            &schema, &subject, &reference,
            &[reference.clone(), subject.clone()], false,
        );
        assert!(glyph.cols <= 5 && glyph.rows <= 5);
        assert!(glyph.substrate.len() <= MAX_SLOTS);
    }
}
