//! Geometry mapping per docs/sound-glyph-spec.md §2 (phase P2): resolve a
//! [`Skeleton`] plus a sound's normalized param values into concrete plant
//! geometry — branch polylines, thicknesses, node marks — in the unit square
//! (x,y in 0..1, y = 0 at the top, trunk rooted at the bottom center).
//!
//! HARD invariant: this is a pure function of (skeleton, values). All organic
//! jitter is seeded from an FNV-1a hash of the branch/node path — never
//! random, never HashMap iteration order. A branch's geometry depends only on
//! topology (branch index/count/weights) and its *own* cluster's param
//! values, so editing one param moves only that param's branch and marks.

use std::collections::BTreeMap;

use super::extract::{Branch, ExtractedSkeleton};
use super::sexpr::{parse, Sexpr};

/// Declared bounds + default of one `(param …)` form; undeclared fields
/// fall back to min 0, max 1, default = min.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// Parse `@min`/`@max`/`@default` for every `(param …)` declaration in an
/// instrument source. Used by hosts (and the capture harness) to normalize
/// raw values before calling [`resolve_geometry`].
pub fn param_specs(source: &str) -> BTreeMap<String, ParamSpec> {
    let mut out = BTreeMap::new();
    for form in parse(source) {
        let Sexpr::List(items) = &form else { continue };
        if items.first().and_then(Sexpr::atom) != Some("param") {
            continue;
        }
        let Some(name) = items.get(1).and_then(Sexpr::atom) else {
            continue;
        };
        let (mut lo, mut hi, mut default) = (0.0f32, 1.0f32, None);
        let mut i = 2;
        while i + 1 < items.len() {
            let value = items[i + 1].atom().and_then(|a| a.parse::<f32>().ok());
            match items[i].atom() {
                Some("@min") => lo = value.unwrap_or(lo),
                Some("@max") => hi = value.unwrap_or(hi),
                Some("@default") => default = value.or(default),
                _ => {}
            }
            i += 1;
        }
        // Sanitize: non-finite (`@min inf`) or degenerate (min >= max) bounds
        // would poison host normalization (divide-by-zero → NaN), so they
        // reset to the 0..1 default range; the default clamps into bounds.
        let (lo, hi) = if !lo.is_finite() || !hi.is_finite() || lo >= hi {
            (0.0, 1.0)
        } else {
            (lo, hi)
        };
        let default = default.filter(|d| d.is_finite()).unwrap_or(lo);
        out.entry(name.to_string()).or_insert(ParamSpec {
            min: lo,
            max: hi,
            default: default.clamp(lo, hi),
        });
    }
    out
}

/// Param name → declared (min, max); see [`param_specs`].
pub fn param_ranges(source: &str) -> BTreeMap<String, (f32, f32)> {
    param_specs(source)
        .into_iter()
        .map(|(name, spec)| (name, (spec.min, spec.max)))
        .collect()
}

/// A single stroked polyline (trunk segment or branch spine).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphStroke {
    /// Cluster name of the owning branch; [`TRUNK`] for the trunk.
    pub branch: String,
    /// Unit-space points, root first.
    pub points: Vec<[f32; 2]>,
    /// Stroke width at the root, in unit space; renderers taper toward the tip.
    pub width: f32,
}

/// A node mark for one param, sized by its normalized value.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphMark {
    pub branch: String,
    pub param: String,
    pub pos: [f32; 2],
    pub radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphGeometry {
    pub strokes: Vec<GlyphStroke>,
    pub marks: Vec<GlyphMark>,
}

/// Branch name used for trunk strokes (never collides with a cluster name
/// because cluster names come from snake_case param prefixes).
pub const TRUNK: &str = "~trunk";

// ── tuning constants (iterated by eye via capture scripts) ──

/// Trunk root / top in unit space.
const TRUNK_BASE_Y: f32 = 0.97;
const TRUNK_TOP_Y: f32 = 0.16;
/// Branches attach along this fraction of the trunk (bottom → top).
const ATTACH_LO_Y: f32 = 0.88;
const ATTACH_HI_Y: f32 = 0.28;
/// Base branch length range (low weight → high weight), unit space.
const LEN_MIN: f32 = 0.15;
const LEN_MAX: f32 = 0.42;
/// Magnitude (mean cluster value) scales length within this range.
const LEN_MAG_LO: f32 = 0.50;
const LEN_MAG_HI: f32 = 1.35;
/// Branch departure angle from vertical, degrees: lower branches spread wide,
/// upper branches hug the trunk.
const ANGLE_LOW_DEG: f32 = 78.0;
const ANGLE_HIGH_DEG: f32 = 30.0;
/// Max deviation added to the departure angle from the folded param values.
const ANGLE_DEV_DEG: f32 = 30.0;
/// Max total curl (accumulated turn over the branch), degrees.
const CURL_MAX_DEG: f32 = 85.0;
/// Hash jitter on attach height / angle, to break mechanical regularity.
const JITTER_ANGLE_DEG: f32 = 5.0;
const JITTER_ATTACH: f32 = 0.012;
/// Stroke widths (unit space).
const TRUNK_WIDTH: f32 = 0.016;
const BRANCH_WIDTH_MIN: f32 = 0.0045;
const BRANCH_WIDTH_MAX: f32 = 0.011;
/// Node mark radii (unit space).
const MARK_R_MIN: f32 = 0.0035;
const MARK_R_MAX: f32 = 0.020;
/// Child offshoots: length relative to parent, angle offset.
const CHILD_LEN_FRAC_MIN: f32 = 0.22;
const CHILD_LEN_FRAC_MAX: f32 = 0.46;
const CHILD_ANGLE_DEG: f32 = 38.0;
/// Segments per branch spine polyline.
const SPINE_SEGMENTS: usize = 10;
const CHILD_SEGMENTS: usize = 6;

// ── deterministic hashing ──

/// FNV-1a, fixed offsets — stable across platforms and Rust versions.
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn path_hash(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in part.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0x1f;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Map a hash to [-1, 1].
fn hash_signed(h: u64) -> f32 {
    ((h >> 11) as f64 / (1u64 << 53) as f64) as f32 * 2.0 - 1.0
}

/// Map a hash to [0, 1].
fn hash_unit(h: u64) -> f32 {
    ((h >> 11) as f64 / (1u64 << 53) as f64) as f32
}

// ── param folding ──

/// Params of one branch, name-ordered (BTreeMap order — deterministic).
fn branch_params<'a>(extracted: &'a ExtractedSkeleton, cluster: &str) -> Vec<&'a str> {
    extracted
        .param_branch
        .iter()
        .filter(|(_, b)| b.as_str() == cluster)
        .map(|(p, _)| p.as_str())
        .collect()
}

fn value_of(values: &BTreeMap<String, f32>, param: &str) -> f32 {
    // `clamp` passes NaN through, so non-finite host values (a normalization
    // gone wrong upstream) fall back to the missing-param default instead of
    // poisoning every downstream fold/point.
    let v = values.get(param).copied().unwrap_or(0.5);
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// Mean of the branch's normalized values (0.5 when the branch has none).
fn magnitude(params: &[&str], values: &BTreeMap<String, f32>) -> f32 {
    if params.is_empty() {
        return 0.5;
    }
    params.iter().map(|p| value_of(values, p)).sum::<f32>() / params.len() as f32
}

/// Fold the branch's values into [-1, 1] with fixed per-(branch, param, salt)
/// hash weights. RMS weight normalization (not mean): a single edited param
/// out of n moves the fold by ~|w|/sqrt(sum w^2) ≈ 1/sqrt(n), so a one-knob
/// tweak on a 12-param branch still reads at glyph size — mean-normalizing
/// washed real-world edits (a few params out of ~100) into invisibility.
fn fold(cluster: &str, salt: &str, params: &[&str], values: &BTreeMap<String, f32>) -> f32 {
    if params.is_empty() {
        return hash_signed(path_hash(&[cluster, salt, "empty"]));
    }
    let mut acc = 0.0f32;
    let mut norm_sq = 0.0f32;
    for p in params {
        let w = hash_signed(path_hash(&[cluster, salt, p]));
        acc += w * (value_of(values, p) - 0.5) * 2.0;
        norm_sq += w * w;
    }
    if norm_sq <= f32::EPSILON {
        0.0
    } else {
        (acc / norm_sq.sqrt()).clamp(-1.0, 1.0)
    }
}

// ── geometry construction ──

fn deg(d: f32) -> f32 {
    d * std::f32::consts::PI / 180.0
}

/// Grow a curved spine from `origin`: constant per-segment turn (curl),
/// per-segment hash wobble. Returns the polyline, root first.
fn grow_spine(
    origin: [f32; 2],
    angle_from_up: f32,
    length: f32,
    curl: f32,
    segments: usize,
    seed: u64,
) -> Vec<[f32; 2]> {
    let mut points = Vec::with_capacity(segments + 1);
    points.push(origin);
    let step = length / segments as f32;
    let turn = curl / segments as f32;
    let mut theta = angle_from_up;
    let mut pos = origin;
    for i in 0..segments {
        let wobble = hash_signed(
            seed.wrapping_add(i as u64)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15),
        ) * deg(2.2);
        theta += turn + wobble;
        // angle measured from "up" (-y); positive = toward +x.
        pos = [pos[0] + step * theta.sin(), pos[1] - step * theta.cos()];
        points.push(pos);
    }
    points
}

fn point_along(points: &[[f32; 2]], t: f32) -> ([f32; 2], f32) {
    // Returns (position, local angle-from-up) at fraction t of the polyline
    // by segment count (segments are equal-length by construction).
    let n = points.len() - 1;
    let ft = (t.clamp(0.0, 1.0)) * n as f32;
    let idx = (ft.floor() as usize).min(n - 1);
    let frac = ft - idx as f32;
    let a = points[idx];
    let b = points[idx + 1];
    let pos = [a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac];
    let angle = (b[0] - a[0]).atan2(-(b[1] - a[1]));
    (pos, angle)
}

/// Resolve a skeleton + normalized param values into renderable geometry.
///
/// `values` maps param name → value normalized to 0..1 via the param's
/// declared @min/@max; missing params read as 0.5.
pub fn resolve_geometry(
    extracted: &ExtractedSkeleton,
    values: &BTreeMap<String, f32>,
) -> GlyphGeometry {
    let branches = &extracted.skeleton.branches;
    let mut out = GlyphGeometry::default();

    // Trunk: gently bowed vertical spine, fixed shape (topology-only).
    let trunk_seed = path_hash(&[TRUNK]);
    let trunk_bow = hash_signed(trunk_seed) * 0.018;
    let trunk_points: Vec<[f32; 2]> = (0..=SPINE_SEGMENTS)
        .map(|i| {
            let t = i as f32 / SPINE_SEGMENTS as f32;
            let y = TRUNK_BASE_Y + (TRUNK_TOP_Y - TRUNK_BASE_Y) * t;
            let x = 0.5 + trunk_bow * (t * std::f32::consts::PI).sin();
            [x, y]
        })
        .collect();
    out.strokes.push(GlyphStroke {
        branch: TRUNK.to_string(),
        points: trunk_points.clone(),
        width: TRUNK_WIDTH,
    });

    if branches.is_empty() {
        return out;
    }

    let max_weight = branches.iter().map(|b| b.weight).max().unwrap_or(1).max(1) as f32;

    for (idx, branch) in branches.iter().enumerate() {
        let cluster = branch.cluster.as_str();
        let params = branch_params(extracted, cluster);
        let mag = magnitude(&params, values);
        let seed = path_hash(&[cluster]);

        // Attachment: spread branches bottom→top along the trunk in skeleton
        // order, alternating sides, with a small fixed jitter.
        let denom = (branches.len().max(2) - 1) as f32;
        let t_raw = idx as f32 / denom;
        let t = (t_raw + hash_signed(seed) * JITTER_ATTACH).clamp(0.0, 1.0);
        let attach_y = ATTACH_LO_Y + (ATTACH_HI_Y - ATTACH_LO_Y) * t;
        let trunk_t = (TRUNK_BASE_Y - attach_y) / (TRUNK_BASE_Y - TRUNK_TOP_Y);
        let (attach, _) = point_along(&trunk_points, trunk_t);
        let side = if idx % 2 == 0 { 1.0 } else { -1.0 };

        // Length: weight sets the base, magnitude scales it.
        let wnorm = (branch.weight as f32 / max_weight).sqrt();
        let base_len = LEN_MIN + (LEN_MAX - LEN_MIN) * wnorm;
        // Length responds to the cluster mean plus a folded per-param term,
        // so a single-knob edit stretches or shrinks the branch even when
        // the mean barely moves. Headroom cap: a branch can never out-reach
        // its attach point's distance to the top margin (topology-only,
        // preserves locality).
        let len_fold = fold(cluster, "len", &params, values);
        let length =
            (base_len * (LEN_MAG_LO + (LEN_MAG_HI - LEN_MAG_LO) * mag) * (1.0 + 0.22 * len_fold))
                .min(attach_y - 0.035)
                // Lateral cap: the trunk sits at x≈0.5, so no branch may reach
                // farther than the side margin even fully horizontal.
                .min(0.44);

        // Departure angle: wide near the ground, upright near the top, then
        // the folded param values push it around its resting pose.
        let base_angle = ANGLE_LOW_DEG + (ANGLE_HIGH_DEG - ANGLE_LOW_DEG) * t;
        let dev = fold(cluster, "angle", &params, values);
        let jitter = hash_signed(path_hash(&[cluster, "angle-jitter"])) * JITTER_ANGLE_DEG;
        // Departure capped slightly past horizontal (droop reads organic);
        // curl only ever lifts the tip, so this also bounds the branch's
        // lowest point above the bottom margin.
        let angle = side * deg((base_angle + dev * ANGLE_DEV_DEG + jitter).min(96.0));

        // Curl: second fold, biased upward (negative curl lifts the tip).
        let curl_fold = fold(cluster, "curl", &params, values);
        let curl = -side * deg(CURL_MAX_DEG) * (0.08 + 0.92 * (curl_fold * 0.5 + 0.5));

        let spine = grow_spine(
            attach,
            angle,
            length,
            curl,
            SPINE_SEGMENTS,
            path_hash(&[cluster, "wobble"]),
        );
        let width =
            (BRANCH_WIDTH_MIN + (BRANCH_WIDTH_MAX - BRANCH_WIDTH_MIN) * wnorm) * (0.7 + 0.6 * mag);
        out.strokes.push(GlyphStroke {
            branch: cluster.to_string(),
            points: spine.clone(),
            width,
        });

        // Child offshoots: def chains, purely topological + hash jitter.
        emit_children(cluster, &branch.children, &spine, length, width, &mut out);

        // Node marks: one per param, spaced along the spine, sized by value.
        let count = params.len();
        for (pi, param) in params.iter().enumerate() {
            let mt = (pi as f32 + 1.0) / (count as f32 + 1.0);
            let (pos, _) = point_along(&spine, mt);
            let v = value_of(values, param);
            out.marks.push(GlyphMark {
                branch: cluster.to_string(),
                param: (*param).to_string(),
                pos,
                radius: MARK_R_MIN + (MARK_R_MAX - MARK_R_MIN) * v,
            });
        }
    }

    out
}

fn emit_children(
    cluster: &str,
    children: &[Branch],
    spine: &[[f32; 2]],
    parent_len: f32,
    parent_width: f32,
    out: &mut GlyphGeometry,
) {
    if children.is_empty() {
        return;
    }
    let max_w = children.iter().map(|c| c.weight).max().unwrap_or(1).max(1) as f32;
    let count = children.len();
    for (ci, child) in children.iter().enumerate() {
        let seed = path_hash(&[cluster, "child", child.cluster.as_str()]);
        let t = (ci as f32 + 1.0) / (count as f32 + 1.0);
        let (pos, local_angle) = point_along(spine, t);
        let side = if ci % 2 == 0 { 1.0 } else { -1.0 };
        let frac = CHILD_LEN_FRAC_MIN
            + (CHILD_LEN_FRAC_MAX - CHILD_LEN_FRAC_MIN) * (child.weight as f32 / max_w).sqrt();
        let angle = local_angle + side * deg(CHILD_ANGLE_DEG + hash_unit(seed) * 14.0);
        let curl = -side * deg(24.0);
        // Same headroom rule as the parent: an offshoot can never out-reach
        // its attach point's distance to the top margin.
        let len = (parent_len * frac).min((pos[1] - 0.03).max(0.01));
        let pts = grow_spine(pos, angle, len, curl, CHILD_SEGMENTS, seed);
        out.strokes.push(GlyphStroke {
            branch: cluster.to_string(),
            points: pts,
            width: parent_width * 0.55,
        });
    }
}
