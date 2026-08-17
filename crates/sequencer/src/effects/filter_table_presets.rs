//! Original Filter Table factory presets and their deterministic generator
//! (eseq-dtx.7).
//!
//! Presets are authored as dB response curves on an octave coordinate
//! relative to the table's reference harmonic (bin 24 sits at 0 octaves, so
//! the `cutoff` parameter transposes every curve by construction). Each
//! preset is a small set of parametric elements whose parameters follow a
//! *trajectory* ([`Traj`]) from frame 0 to frame 63.
//!
//! Trajectories are the character of a preset. A linear ramp fades politely;
//! the vocabulary here also covers wobble (sine, optionally damped, riding a
//! ramp), swoop (exponential ease), logistic (S-curve with an adjustable
//! knee), stepped jumps (seeded LCG values held for `count` segments with an
//! optional glide fraction) and piecewise segments. Presets therefore fall
//! into motion classes ([`MotionClass`]): `Glide` presets are single coherent
//! ramps, `Wobble` presets move non-monotonically (the "gulp" family: high-Q
//! peaks swooping octaves with overshoot), and `Jump` presets deliberately
//! discontinue (the "sprlonk" family: peak clusters scattering between frame
//! regions). Validation bounds are per class — see the tests.
//!
//! Policy, made explicit:
//! - curves are summed in dB, per frame;
//! - each frame is peak-normalized to 0 dB *in the dB domain*;
//! - everything below the recipe's `db_floor` clamps to the floor;
//! - conversion to linear magnitudes (`10^(dB/20)`) happens only when
//!   baking, and the baked bank is what ships in the `.fltab` asset.
//!
//! Generation is fully deterministic: there is no wall-clock or ambient RNG
//! input, and the one stochastic element ([`Element::Texture`]) derives all
//! values from an LCG seeded by a `seed` stored in the recipe. The complete
//! recipe is embedded in each asset's `recipe` metadata field, so an asset
//! can always be re-baked bit-for-bit from its own header (guarded by
//! `bundled_factory_assets_match_their_recipes`).
//!
//! All content here is original: curves use conventional DSP shapes
//! (slopes, resonant peaks, combs, formant-style peak sets) with
//! independently chosen parameters; no third-party preset data was used.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::filter_table::{MagnitudeTable, FRAMES, NBINS, REFERENCE_HARMONIC, TABLE_LEN};
use super::filter_table_asset::{write_asset, FilterTableAssetMeta};

/// Bump when the element math changes meaning; stored in every recipe so a
/// future generator can refuse (or migrate) recipes it would mis-render.
pub const GENERATOR_VERSION: u32 = 2;

/// How much a preset's frame motion is allowed to misbehave. Stored in the
/// recipe so the class travels with the asset, and used by the factory
/// regression tests to pick validation bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MotionClass {
    /// One coherent monotone-ish ramp: adjacent frames differ by roughly the
    /// same small amount all the way across.
    #[default]
    Glide,
    /// Non-monotonic but continuous: swoops, wobbles, overshoot. Steps may be
    /// several times larger than a glide preset's and must reverse direction.
    Wobble,
    /// Deliberately discontinuous: the response jumps between frame regions.
    /// Still bounded and validated, just not smooth.
    Jump,
}

/// How one element parameter moves across the 64 frames. `frame` is
/// normalized 0..1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "curve", rename_all = "kebab-case")]
pub enum Traj {
    /// Straight ramp `from` -> `to`.
    Linear { from: f32, to: f32 },
    /// Ramp plus a sine of `cycles` periods and `+/- depth`, phase in turns,
    /// amplitude decaying by `exp(-damp * frame)` (use `damp` for overshoot
    /// that settles).
    Wobble {
        from: f32,
        to: f32,
        cycles: f32,
        depth: f32,
        phase: f32,
        damp: f32,
    },
    /// Exponentially eased ramp. `bend > 0` loiters then rushes, `bend < 0`
    /// leaps then settles; `bend == 0` is linear.
    Swoop { from: f32, to: f32, bend: f32 },
    /// Normalized logistic S-curve: flat, a fast knee at `center`, flat.
    Logistic {
        from: f32,
        to: f32,
        steepness: f32,
        center: f32,
    },
    /// `count` held values drawn deterministically from `[from, to]` by an
    /// LCG on `seed`. `glide` (0..1) is the fraction of each segment spent
    /// smoothly moving to the new value: 0 is a hard jump.
    Steps {
        from: f32,
        to: f32,
        count: u32,
        seed: u32,
        glide: f32,
    },
    /// Piecewise-linear through `(frame, value)` breakpoints, clamped outside
    /// the first/last point. Points must be in ascending frame order.
    Segments { points: Vec<(f32, f32)> },
}

/// A constant-across-frames trajectory.
pub fn hold(value: f32) -> Traj {
    Traj::Linear { from: value, to: value }
}

pub fn sweep(from: f32, to: f32) -> Traj {
    Traj::Linear { from, to }
}

pub fn wobble(from: f32, to: f32, cycles: f32, depth: f32) -> Traj {
    Traj::Wobble { from, to, cycles, depth, phase: 0.0, damp: 0.0 }
}

/// Wobble with an explicit starting phase (turns) and decay rate.
pub fn wobble_at(from: f32, to: f32, cycles: f32, depth: f32, phase: f32, damp: f32) -> Traj {
    Traj::Wobble { from, to, cycles, depth, phase, damp }
}

pub fn swoop(from: f32, to: f32, bend: f32) -> Traj {
    Traj::Swoop { from, to, bend }
}

pub fn logistic(from: f32, to: f32, steepness: f32, center: f32) -> Traj {
    Traj::Logistic { from, to, steepness, center }
}

pub fn steps(from: f32, to: f32, count: u32, seed: u32, glide: f32) -> Traj {
    Traj::Steps { from, to, count, seed, glide }
}

pub fn segments(points: &[(f32, f32)]) -> Traj {
    Traj::Segments { points: points.to_vec() }
}

/// Smoothstep on 0..1, used for the glide portion of stepped trajectories.
fn smoothstep(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Value of a stepped trajectory at continuous step position `u` (whole
/// numbers are segment boundaries). Shared by [`Traj::Steps`] and
/// [`Element::ScatterPeaks`].
fn stepped_value(seed: u32, from: f32, to: f32, count: u32, glide: f32, u: f32) -> f32 {
    let count = count.max(1);
    let clamped = u.max(0.0).min(count as f32 - 1.0e-4);
    let index = clamped.floor() as i32;
    let fraction = clamped - index as f32;
    let pick = |i: i32| from + (to - from) * lattice_value(seed, i);
    let current = pick(index);
    let glide = glide.clamp(0.0, 1.0);
    if glide <= 0.0 || fraction >= glide || index == 0 {
        current
    } else {
        let previous = pick(index - 1);
        previous + (current - previous) * smoothstep(fraction / glide)
    }
}

impl Traj {
    fn at(&self, frame: f32) -> f32 {
        let t = frame.clamp(0.0, 1.0);
        match self {
            Traj::Linear { from, to } => from + (to - from) * t,
            Traj::Wobble { from, to, cycles, depth, phase, damp } => {
                let base = from + (to - from) * t;
                let decay = (-damp * t).exp();
                base + depth
                    * decay
                    * (std::f32::consts::TAU * (cycles * t + phase)).sin()
            }
            Traj::Swoop { from, to, bend } => {
                let shaped = if bend.abs() < 1.0e-4 {
                    t
                } else {
                    ((bend * t).exp() - 1.0) / (bend.exp() - 1.0)
                };
                from + (to - from) * shaped
            }
            Traj::Logistic { from, to, steepness, center } => {
                let k = steepness.max(1.0e-3);
                let logistic = |x: f32| 1.0 / (1.0 + (-k * (x - center)).exp());
                let low = logistic(0.0);
                let high = logistic(1.0);
                let span = (high - low).abs().max(1.0e-6);
                from + (to - from) * ((logistic(t) - low) / span)
            }
            Traj::Steps { from, to, count, seed, glide } => {
                stepped_value(*seed, *from, *to, *count, *glide, t * (*count).max(1) as f32)
            }
            Traj::Segments { points } => {
                if points.is_empty() {
                    return 0.0;
                }
                if t <= points[0].0 {
                    return points[0].1;
                }
                for pair in points.windows(2) {
                    let (left_at, left) = pair[0];
                    let (right_at, right) = pair[1];
                    if t <= right_at {
                        let span = (right_at - left_at).max(1.0e-6);
                        return left + (right - left) * ((t - left_at) / span);
                    }
                }
                points[points.len() - 1].1
            }
        }
    }
}

/// One additive dB-domain curve element. `center`/`edge`/`pivot`/`base`
/// coordinates are octaves relative to the reference harmonic (0.0 = the
/// cutoff frequency itself).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum Element {
    /// 0 dB below `edge`, rolling off at `slope_db_per_octave` above it.
    Lowpass { edge: Traj, slope_db_per_octave: Traj },
    /// 0 dB above `edge`, rolling off below it.
    Highpass { edge: Traj, slope_db_per_octave: Traj },
    /// Gaussian peak (or dip, with negative `gain_db`) in dB.
    Peak { center: Traj, width_octaves: Traj, gain_db: Traj },
    /// Cosine comb over harmonic number `n = bin / (24 * spacing)`; dips
    /// reach `-depth_db`. `stretch` warps the tooth positions upward like a
    /// stiff string (`n' = n * (1 + stretch * n)`), making the comb
    /// inharmonic.
    Comb { spacing: Traj, depth_db: Traj, stretch: Traj },
    /// `count` Gaussian notches evenly spread over `span_octaves` starting
    /// at `base` — a phaser-style all-pass-bank magnitude signature.
    /// `stagger` offsets notch `i`'s trajectory time by `stagger * i` (wrapped
    /// into 0..1), turning a bank sweep into a barber-pole.
    NotchBank {
        count: u32,
        base: Traj,
        span_octaves: Traj,
        depth_db: Traj,
        width_octaves: Traj,
        #[serde(default)]
        stagger: f32,
    },
    /// The peak counterpart of [`Element::NotchBank`]: `count` Gaussian peaks
    /// spread over `span_octaves` from `base`, with the same `stagger` trick.
    /// Staggered banks are how a resonant cluster scatters instead of gliding.
    PeakBank {
        count: u32,
        base: Traj,
        span_octaves: Traj,
        gain_db: Traj,
        width_octaves: Traj,
        #[serde(default)]
        stagger: f32,
    },
    /// `count` narrow peaks whose centers *jump* between `jumps` seeded
    /// positions inside `[low, high]`. Peak `i` runs its own LCG stream and
    /// its own step clock offset by `stagger * i`, so the cluster scatters
    /// rather than moves as a block. `glide` is the per-step glide fraction
    /// (0 = hard jump). This is the "sprlonk" primitive.
    ScatterPeaks {
        count: u32,
        seed: u32,
        low: Traj,
        high: Traj,
        width_octaves: Traj,
        gain_db: Traj,
        jumps: u32,
        #[serde(default)]
        glide: f32,
        #[serde(default)]
        stagger: f32,
    },
    /// Linear-in-octaves spectral tilt through `pivot`.
    Tilt { db_per_octave: Traj, pivot: Traj },
    /// Emphasis mask over integer harmonics of the reference: bins near odd
    /// harmonics get `odd_db`, near even harmonics `even_db`, and the space
    /// between falls toward `rest_db`. `width` is the Gaussian half-width
    /// around each harmonic in harmonic-number units.
    HarmonicMask {
        odd_db: Traj,
        even_db: Traj,
        rest_db: Traj,
        width: Traj,
    },
    /// Seeded smooth spectral noise: value noise on an octave grid of
    /// `grain_octaves` spacing, mapped to `+/- depth_db`. Frames morph by
    /// sliding the sample position `drift_octaves` across the table, so
    /// motion is a continuous scroll rather than a re-roll.
    Texture {
        seed: u32,
        grain_octaves: Traj,
        depth_db: Traj,
        drift_octaves: Traj,
    },
}

/// A complete deterministic generation recipe, embedded verbatim in the
/// asset's `recipe` metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub generator_version: u32,
    /// dB clamp floor after per-frame peak normalization.
    pub db_floor: f32,
    /// How wild this table's frame motion is allowed to be. Purely
    /// descriptive for baking; the factory tests validate against it.
    #[serde(default)]
    pub motion: MotionClass,
    pub elements: Vec<Element>,
}

/// A factory preset: stem (file name / `fltab:` reference), display name,
/// one-line sonic intent, suggested controls, and the recipe.
#[derive(Clone, Debug)]
pub struct PresetDefinition {
    pub stem: &'static str,
    pub display_name: &'static str,
    pub intent: &'static str,
    pub default_controls: &'static [(&'static str, f32)],
    pub recipe: Recipe,
}

/// Octave position of a table bin relative to the reference harmonic. Bin 0
/// (DC) is treated as half a bin to keep the coordinate finite.
fn octaves(bin: usize) -> f32 {
    ((bin as f32).max(0.5) / REFERENCE_HARMONIC as f32).log2()
}

/// Trajectory time for member `index` of a staggered bank: the member's own
/// clock, wrapped into 0..1 so a staggered bank reads as a barber-pole.
fn staggered_frame(frame: f32, stagger: f32, index: u32) -> f32 {
    if stagger == 0.0 {
        return frame;
    }
    (frame + stagger * index as f32).rem_euclid(1.0)
}

fn gaussian_db(distance: f32, width: f32) -> f32 {
    let normalized = distance / width.max(1.0e-3);
    (-0.5 * normalized * normalized).exp()
}

/// Deterministic 0..1 hash of a lattice index for [`Element::Texture`].
fn lattice_value(seed: u32, index: i32) -> f32 {
    let mut state = seed
        .wrapping_mul(747796405)
        .wrapping_add(index as u32 ^ 0x9E37_79B9);
    for _ in 0..3 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    }
    (state >> 8) as f32 / 16_777_216.0
}

/// Cosine-interpolated value noise over the octave axis.
fn value_noise(seed: u32, position: f32) -> f32 {
    let cell = position.floor();
    let fraction = position - cell;
    let index = cell as i32;
    let smooth = 0.5 - 0.5 * (std::f32::consts::PI * fraction).cos();
    let left = lattice_value(seed, index);
    let right = lattice_value(seed, index + 1);
    left + (right - left) * smooth
}

impl Element {
    /// dB contribution of this element at `frame` (0..1) and table bin.
    fn db(&self, frame: f32, bin: usize) -> f32 {
        let o = octaves(bin);
        match self {
            Element::Lowpass { edge, slope_db_per_octave } => {
                let excess = o - edge.at(frame);
                if excess <= 0.0 { 0.0 } else { -slope_db_per_octave.at(frame) * excess }
            }
            Element::Highpass { edge, slope_db_per_octave } => {
                let deficit = edge.at(frame) - o;
                if deficit <= 0.0 { 0.0 } else { -slope_db_per_octave.at(frame) * deficit }
            }
            Element::Peak { center, width_octaves, gain_db } => {
                gain_db.at(frame)
                    * gaussian_db(o - center.at(frame), width_octaves.at(frame))
            }
            Element::Comb { spacing, depth_db, stretch } => {
                let harmonic =
                    bin as f32 / (REFERENCE_HARMONIC as f32 * spacing.at(frame).max(1.0e-3));
                let warped = harmonic * (1.0 + stretch.at(frame) * harmonic);
                let tooth = 0.5 - 0.5 * (std::f32::consts::TAU * warped).cos();
                -depth_db.at(frame) * tooth
            }
            Element::NotchBank {
                count,
                base,
                span_octaves,
                depth_db,
                width_octaves,
                stagger,
            } => {
                let count = (*count).max(1);
                let mut db = 0.0f32;
                for notch in 0..count {
                    let along = if count == 1 {
                        0.0
                    } else {
                        notch as f32 / (count - 1) as f32
                    };
                    let local = staggered_frame(frame, *stagger, notch);
                    let center = base.at(local) + span_octaves.at(local) * along;
                    db -= depth_db.at(local)
                        * gaussian_db(o - center, width_octaves.at(local));
                }
                db
            }
            Element::PeakBank {
                count,
                base,
                span_octaves,
                gain_db,
                width_octaves,
                stagger,
            } => {
                let count = (*count).max(1);
                let mut db = 0.0f32;
                for peak in 0..count {
                    let along = if count == 1 {
                        0.0
                    } else {
                        peak as f32 / (count - 1) as f32
                    };
                    let local = staggered_frame(frame, *stagger, peak);
                    let center = base.at(local) + span_octaves.at(local) * along;
                    db += gain_db.at(local)
                        * gaussian_db(o - center, width_octaves.at(local));
                }
                db
            }
            Element::ScatterPeaks {
                count,
                seed,
                low,
                high,
                width_octaves,
                gain_db,
                jumps,
                glide,
                stagger,
            } => {
                let count = (*count).max(1);
                let jumps = (*jumps).max(1);
                let mut db = 0.0f32;
                for peak in 0..count {
                    // Per-peak LCG stream and per-peak step clock: the
                    // cluster scatters instead of moving as a block.
                    let stream = seed
                        .wrapping_add((peak as u32).wrapping_mul(0x9E37_79B1));
                    let u = frame * jumps as f32 + *stagger * peak as f32;
                    let center = stepped_value(
                        stream,
                        low.at(frame),
                        high.at(frame),
                        jumps,
                        *glide,
                        u,
                    );
                    db += gain_db.at(frame)
                        * gaussian_db(o - center, width_octaves.at(frame));
                }
                db
            }
            Element::Tilt { db_per_octave, pivot } => {
                db_per_octave.at(frame) * (o - pivot.at(frame))
            }
            Element::HarmonicMask { odd_db, even_db, rest_db, width } => {
                let harmonic = bin as f32 / REFERENCE_HARMONIC as f32;
                let nearest = harmonic.round().max(1.0);
                let proximity = gaussian_db(harmonic - nearest, width.at(frame));
                let harmonic_db = if (nearest as i64) % 2 == 1 {
                    odd_db.at(frame)
                } else {
                    even_db.at(frame)
                };
                let rest = rest_db.at(frame);
                rest + (harmonic_db - rest) * proximity
            }
            Element::Texture { seed, grain_octaves, depth_db, drift_octaves } => {
                let grain = grain_octaves.at(frame).max(0.05);
                // Offset by +8 octaves so lattice positions stay positive
                // across the whole table range.
                let position = (o + 8.0 + drift_octaves.at(frame) * frame) / grain;
                let noise = value_noise(*seed, position);
                depth_db.at(frame) * (noise * 2.0 - 1.0)
            }
        }
    }
}

/// Bake a recipe into the runtime magnitude bank. Deterministic: same recipe
/// in, bit-identical table out.
pub fn bake(recipe: &Recipe) -> Result<MagnitudeTable, String> {
    if recipe.generator_version != GENERATOR_VERSION {
        return Err(format!(
            "recipe requires generator version {}, this build is version {GENERATOR_VERSION}",
            recipe.generator_version
        ));
    }
    if !(recipe.db_floor.is_finite() && recipe.db_floor < 0.0) {
        return Err("recipe db_floor must be a finite negative dB value".to_string());
    }
    let mut data = Vec::with_capacity(TABLE_LEN);
    for frame in 0..FRAMES {
        let position = frame as f32 / (FRAMES - 1) as f32;
        let mut db_row: Vec<f32> = (0..NBINS)
            .map(|bin| {
                recipe
                    .elements
                    .iter()
                    .map(|element| element.db(position, bin))
                    .sum()
            })
            .collect();
        let peak = db_row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for db in db_row.drain(..) {
            data.push(10.0f32.powf((db - peak).max(recipe.db_floor) / 20.0));
        }
    }
    MagnitudeTable::new(data)
}

/// Asset metadata for a factory preset, recipe embedded.
pub fn preset_meta(preset: &PresetDefinition) -> Result<FilterTableAssetMeta, String> {
    let mut meta = FilterTableAssetMeta::new(preset.display_name);
    meta.magnitude_floor = 10.0f32.powf(preset.recipe.db_floor / 20.0);
    meta.default_controls = preset
        .default_controls
        .iter()
        .map(|(name, value)| (name.to_string(), *value))
        .collect::<BTreeMap<_, _>>();
    meta.recipe = Some(
        serde_json::to_value(&preset.recipe)
            .map_err(|error| format!("failed to encode recipe: {error}"))?,
    );
    Ok(meta)
}

/// Bake a preset and write `<dir>/<stem>.fltab`.
pub fn write_preset(dir: &Path, preset: &PresetDefinition) -> Result<(), String> {
    let table = bake(&preset.recipe)?;
    let meta = preset_meta(preset)?;
    write_asset(&dir.join(format!("{}.fltab", preset.stem)), &meta, &table)
}

const DEFAULT_CONTROLS: &[(&str, f32)] = &[
    ("frame", 0.0),
    ("cutoff", 517.0), // ~ the identity cutoff at 44.1k: table bins == spectrum bins
    ("resonance", 0.0),
    ("mix", 100.0),
];

/// The original factory library. Each preset is one coherent morph family;
/// see each `intent` line for what the 64-frame motion does, and each
/// recipe's [`MotionClass`] for how wild that motion is allowed to get.
pub fn factory_presets() -> Vec<PresetDefinition> {
    vec![
        // ---- pass-filter motion -------------------------------------------------
        PresetDefinition {
            stem: "swoop-low",
            display_name: "Swoop Low",
            intent: "resonant lowpass climbing four octaves while the edge wobbles three times — the filter swallows as it opens",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    // Edge and resonant lip share one wobble so the peak
                    // rides exactly on the knee the whole way up.
                    Element::Lowpass {
                        edge: wobble_at(0.2, 4.2, 3.0, 0.5, 0.0, 0.0),
                        slope_db_per_octave: sweep(20.0, 26.0),
                    },
                    Element::Peak {
                        center: wobble_at(0.25, 4.25, 3.0, 0.5, 0.0, 0.0),
                        width_octaves: sweep(0.30, 0.17),
                        gain_db: sweep(34.0, 44.0),
                    },
                    Element::Tilt { db_per_octave: hold(-1.0), pivot: hold(0.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "cavity-high",
            display_name: "Cavity High",
            intent: "highpass edge dropping fast then settling, with a resonance that overshoots below it and rings out — hollow, gulping bottom end",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -58.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Highpass {
                        edge: swoop(4.2, -0.8, -2.5),
                        slope_db_per_octave: sweep(26.0, 14.0),
                    },
                    // Damped wobble: the resonance overshoots past the edge
                    // twice and settles.
                    Element::Peak {
                        center: wobble_at(4.0, -0.5, 2.0, 0.6, 0.25, 1.4),
                        width_octaves: hold(0.22),
                        gain_db: sweep(38.0, 30.0),
                    },
                    Element::Peak {
                        center: swoop(3.0, 1.2, -2.0),
                        width_octaves: hold(0.55),
                        gain_db: hold(-22.0),
                    },
                ],
            },
        },
        PresetDefinition {
            stem: "band-flight",
            display_name: "Band Flight",
            intent: "a resonant bandpass climbing three octaves in two big loping arcs, tightening from a broad band to a singing peak",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Peak {
                        center: wobble_at(-0.2, 3.2, 2.5, 0.75, 0.0, 0.5),
                        width_octaves: swoop(1.1, 0.22, -2.0),
                        gain_db: sweep(44.0, 52.0),
                    },
                    Element::Tilt { db_per_octave: hold(-2.0), pivot: hold(0.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "notch-drift",
            display_name: "Notch Drift",
            intent: "a deep notch and the resonance above it lurching up through the mids in two and a half wobbles, widening as they go",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Peak {
                        center: wobble(-0.5, 3.5, 2.5, 0.5),
                        width_octaves: sweep(0.25, 0.7),
                        gain_db: sweep(-44.0, -32.0),
                    },
                    // A modest resonance rides three quarters of an octave
                    // above the notch on the same wobble, so the pair audibly
                    // swings instead of the notch quietly passing through.
                    Element::Peak {
                        center: wobble(0.25, 4.25, 2.5, 0.5),
                        width_octaves: hold(0.30),
                        gain_db: sweep(26.0, 34.0),
                    },
                    Element::Lowpass { edge: hold(5.0), slope_db_per_octave: hold(18.0) },
                ],
            },
        },
        // ---- vowels and formants ------------------------------------------------
        PresetDefinition {
            stem: "vowel-drift",
            display_name: "Vowel Drift",
            intent: "three-formant vocal morph where each formant moves on its own curve: F1 opens, closes and reopens, F2 wobbles twice, F3 leaps then settles",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Peak {
                        center: segments(&[
                            (0.0, -0.75),
                            (0.35, 0.50),
                            (0.62, -0.10),
                            (1.0, 0.45),
                        ]),
                        width_octaves: hold(0.26),
                        gain_db: hold(38.0),
                    },
                    Element::Peak {
                        center: wobble(0.55, 2.25, 2.0, 0.55),
                        width_octaves: hold(0.22),
                        gain_db: sweep(26.0, 36.0),
                    },
                    Element::Peak {
                        center: swoop(2.35, 3.15, -3.0),
                        width_octaves: hold(0.3),
                        gain_db: sweep(16.0, 28.0),
                    },
                    Element::Lowpass { edge: hold(4.2), slope_db_per_octave: hold(24.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "talkbox-cycle",
            display_name: "Talkbox Cycle",
            intent: "a closed five-vowel loop (oo-ee-oh-ah-eh and back) held on segments, so the second formant zig-zags and frame 63 meets frame 0 — mouthy, cyclable",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -50.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    // F1/F2 breakpoints are octaves relative to the reference
                    // harmonic; at the identity cutoff 0 oct is ~517 Hz, which
                    // puts these in the usual 300 Hz-2.6 kHz formant range.
                    Element::Peak {
                        center: segments(&[
                            (0.0, -0.78),
                            (0.2, -0.78),
                            (0.4, -0.20),
                            (0.6, 0.44),
                            (0.8, 0.09),
                            (1.0, -0.78),
                        ]),
                        width_octaves: hold(0.20),
                        gain_db: hold(42.0),
                    },
                    Element::Peak {
                        center: segments(&[
                            (0.0, 0.72),
                            (0.2, 2.33),
                            (0.4, 0.95),
                            (0.6, 1.22),
                            (0.8, 1.88),
                            (1.0, 0.72),
                        ]),
                        width_octaves: hold(0.18),
                        gain_db: hold(36.0),
                    },
                    Element::Peak {
                        center: hold(2.62),
                        width_octaves: hold(0.30),
                        gain_db: hold(20.0),
                    },
                    Element::Lowpass { edge: hold(4.2), slope_db_per_octave: hold(24.0) },
                ],
            },
        },
        // ---- the gulp family ----------------------------------------------------
        PresetDefinition {
            stem: "gulp-throat",
            display_name: "Gulp Throat",
            intent: "the archetype gulp: a very high-Q peak pair swooping three octaves with a big early wobble that settles — a throat swallowing the sound",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -50.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Peak {
                        center: wobble_at(-0.3, 2.9, 2.5, 0.85, 0.0, 0.35),
                        width_octaves: hold(0.11),
                        gain_db: hold(54.0),
                    },
                    // Second formant a fixed 1.2 octaves up, same wobble, so
                    // the pair reads as one voice rather than two filters.
                    Element::Peak {
                        center: wobble_at(0.9, 4.1, 2.5, 0.85, 0.0, 0.35),
                        width_octaves: hold(0.16),
                        gain_db: hold(40.0),
                    },
                    Element::Lowpass { edge: hold(4.6), slope_db_per_octave: hold(14.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "gulp-choir",
            display_name: "Gulp Choir",
            intent: "three resonant voices swooping an octave and a half each, staggered a third of a cycle apart — a seamless barber-pole gulp that loops",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -50.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    // A pure single-cycle wobble (from == to) is seamless
                    // under the stagger wrap, so the three voices form a
                    // continuous rotation instead of snapping at the wrap.
                    Element::PeakBank {
                        count: 3,
                        base: wobble_at(0.9, 0.9, 1.0, 1.5, 0.0, 0.0),
                        span_octaves: hold(1.5),
                        gain_db: hold(46.0),
                        width_octaves: hold(0.13),
                        stagger: 1.0 / 3.0,
                    },
                    Element::Lowpass { edge: hold(4.6), slope_db_per_octave: hold(12.0) },
                    Element::Tilt { db_per_octave: hold(-1.5), pivot: hold(0.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "wub-gate",
            display_name: "Wub Gate",
            intent: "four full wubs across the frame axis: a resonant lowpass edge rocking two octaves with a counter-phase notch chasing it",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Lowpass {
                        edge: wobble_at(1.2, 1.2, 4.0, 1.1, 0.0, 0.0),
                        slope_db_per_octave: hold(26.0),
                    },
                    Element::Peak {
                        center: wobble_at(1.2, 1.2, 4.0, 1.1, 0.0, 0.0),
                        width_octaves: hold(0.20),
                        gain_db: sweep(36.0, 48.0),
                    },
                    Element::Peak {
                        center: wobble_at(1.2, 1.2, 4.0, 1.1, 0.5, 0.0),
                        width_octaves: hold(0.40),
                        gain_db: hold(-26.0),
                    },
                ],
            },
        },
        PresetDefinition {
            stem: "rubber-neck",
            display_name: "Rubber Neck",
            intent: "a narrow resonance flung four octaves that overshoots hard, bounces three times and settles — boing on a filter",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -52.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Peak {
                        center: wobble_at(-0.6, 3.4, 3.0, 1.2, 0.0, 2.6),
                        width_octaves: hold(0.13),
                        gain_db: hold(50.0),
                    },
                    Element::Peak {
                        center: wobble_at(0.6, 4.6, 3.0, 1.2, 0.0, 2.6),
                        width_octaves: hold(0.20),
                        gain_db: hold(30.0),
                    },
                    Element::Tilt { db_per_octave: hold(-1.0), pivot: hold(0.0) },
                ],
            },
        },
        // ---- the sprlonk family (jump class) ------------------------------------
        PresetDefinition {
            stem: "sprlonk",
            display_name: "Sprlonk",
            intent: "five needle resonances scattering to new pitches eleven times across the frame axis, each on its own clock — stepped, watery, plucked",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -46.0,
                motion: MotionClass::Jump,
                elements: vec![
                    Element::ScatterPeaks {
                        count: 5,
                        seed: 0x0053_9C1A,
                        low: hold(-0.8),
                        high: hold(3.4),
                        width_octaves: hold(0.09),
                        gain_db: hold(42.0),
                        jumps: 11,
                        glide: 0.06,
                        stagger: 0.23,
                    },
                    Element::Lowpass { edge: hold(4.6), slope_db_per_octave: hold(12.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "droplet",
            display_name: "Droplet",
            intent: "three hair-thin peaks jumping fourteen times through a window that itself rises two octaves — water drops falling upward",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -46.0,
                motion: MotionClass::Jump,
                elements: vec![
                    Element::ScatterPeaks {
                        count: 3,
                        seed: 0x00D9_0B17,
                        low: sweep(-0.5, 1.6),
                        high: sweep(1.4, 4.2),
                        width_octaves: hold(0.06),
                        gain_db: hold(46.0),
                        jumps: 14,
                        glide: 0.0,
                        stagger: 0.37,
                    },
                    Element::Lowpass { edge: hold(5.0), slope_db_per_octave: hold(10.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "stutter-band",
            display_name: "Stutter Band",
            intent: "one fat resonant band teleporting between seven positions while a comb re-spaces itself on the same clock — hard spectral stutter",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Jump,
                elements: vec![
                    Element::Peak {
                        center: steps(-0.4, 3.2, 7, 0x0051_DE01, 0.0),
                        width_octaves: hold(0.28),
                        gain_db: hold(44.0),
                    },
                    Element::Comb {
                        spacing: steps(0.5, 2.5, 7, 0x0051_DE02, 0.0),
                        depth_db: sweep(10.0, 28.0),
                        stretch: hold(0.0),
                    },
                    Element::Lowpass { edge: hold(4.4), slope_db_per_octave: hold(14.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "arp-harmonic",
            display_name: "Harmonic Arp",
            intent: "a needle resonance stepping the harmonic series 1-2-3-4-5-6-7-8 over a held tonic band — a spectral arpeggio you play with the frame knob",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Jump,
                elements: vec![
                    // Staircase breakpoints: log2 of harmonics 1..8, each held
                    // for an eighth of the frame axis.
                    Element::Peak {
                        center: segments(&[
                            (0.000, 0.0),
                            (0.124, 0.0),
                            (0.125, 1.0),
                            (0.249, 1.0),
                            (0.250, 1.585),
                            (0.374, 1.585),
                            (0.375, 2.0),
                            (0.499, 2.0),
                            (0.500, 2.322),
                            (0.624, 2.322),
                            (0.625, 2.585),
                            (0.749, 2.585),
                            (0.750, 2.807),
                            (0.874, 2.807),
                            (0.875, 3.0),
                            (1.000, 3.0),
                        ]),
                        width_octaves: hold(0.10),
                        gain_db: hold(48.0),
                    },
                    Element::Peak {
                        center: hold(0.0),
                        width_octaves: hold(0.16),
                        gain_db: hold(30.0),
                    },
                    Element::Lowpass { edge: hold(4.8), slope_db_per_octave: hold(12.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "comb-lurch",
            display_name: "Comb Lurch",
            intent: "a deep comb whose tooth spacing and inharmonic stretch both re-throw eight times — metallic tuning lurches, no glide",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Jump,
                elements: vec![
                    Element::Comb {
                        spacing: steps(0.5, 3.0, 8, 0x00C0_1B01, 0.02),
                        depth_db: hold(34.0),
                        stretch: steps(0.0, 0.004, 8, 0x00C0_1B02, 0.02),
                    },
                    Element::Tilt { db_per_octave: sweep(-1.0, 1.0), pivot: hold(1.0) },
                    Element::Lowpass { edge: hold(4.6), slope_db_per_octave: hold(12.0) },
                ],
            },
        },
        // ---- kept glide-class colour --------------------------------------------
        PresetDefinition {
            stem: "comb-bloom",
            display_name: "Comb Bloom",
            intent: "harmonic comb fading in: flat at first, blooming into deep evenly spaced teeth",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Glide,
                elements: vec![
                    Element::Comb {
                        spacing: hold(1.0),
                        depth_db: sweep(0.0, 40.0),
                        stretch: hold(0.0),
                    },
                    Element::Lowpass { edge: hold(4.5), slope_db_per_octave: hold(12.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "glass-comb",
            display_name: "Glass Comb",
            intent: "inharmonic comb whose teeth stretch apart like a struck bar going glassy and detuned",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Glide,
                elements: vec![
                    // Stretch stays small: the warp scales with harmonic^2,
                    // so even 0.002 drifts the top teeth several positions
                    // across the table while frame-to-frame motion stays
                    // continuous.
                    Element::Comb {
                        spacing: hold(1.0),
                        depth_db: sweep(26.0, 34.0),
                        stretch: sweep(0.0, 0.0015),
                    },
                    Element::Tilt { db_per_octave: sweep(0.0, 1.5), pivot: hold(1.0) },
                    Element::Lowpass { edge: hold(5.0), slope_db_per_octave: hold(12.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "phase-flower",
            display_name: "Phase Flower",
            intent: "six-notch phaser bank sweeping upward, the classic swooshing barber-pole magnitude print",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                motion: MotionClass::Glide,
                elements: vec![Element::NotchBank {
                    count: 6,
                    base: sweep(-1.0, 1.5),
                    span_octaves: hold(4.0),
                    depth_db: hold(30.0),
                    width_octaves: hold(0.22),
                    stagger: 0.0,
                }],
            },
        },
        PresetDefinition {
            stem: "tilt-horizon",
            display_name: "Tilt Horizon",
            intent: "broadband color fader: -5 dB/oct dark tape tone through flat to +5 dB/oct exciter brightness",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                motion: MotionClass::Glide,
                elements: vec![Element::Tilt {
                    db_per_octave: sweep(-5.0, 5.0),
                    pivot: hold(1.0),
                }],
            },
        },
        PresetDefinition {
            stem: "odd-even",
            display_name: "Odd / Even",
            intent: "harmonic mask morph from hollow odd-only (square-like) to smooth even-emphasis timbre",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Glide,
                elements: vec![
                    Element::HarmonicMask {
                        odd_db: sweep(0.0, -30.0),
                        even_db: sweep(-30.0, 0.0),
                        rest_db: hold(-38.0),
                        width: hold(0.16),
                    },
                    Element::Lowpass { edge: hold(4.0), slope_db_per_octave: hold(10.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "dust-veil",
            display_name: "Dust Veil",
            intent: "controlled spectral texture: slowly scrolling smooth noise, deepening from a haze to carved bands",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
                motion: MotionClass::Wobble,
                elements: vec![
                    Element::Texture {
                        seed: 0x00C0_FFEE,
                        grain_octaves: sweep(0.9, 0.55),
                        depth_db: sweep(10.0, 26.0),
                        drift_octaves: hold(1.5),
                    },
                    Element::Lowpass { edge: hold(4.5), slope_db_per_octave: hold(9.0) },
                ],
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::filter_table::{dsp_source, N};
    use crate::effects::filter_table_asset::{
        bundled_asset_dir, read_asset, resolve_asset_path,
    };

    #[test]
    fn factory_presets_bake_deterministically() {
        for preset in factory_presets() {
            let first = bake(&preset.recipe).expect(preset.stem);
            let second = bake(&preset.recipe).expect(preset.stem);
            assert_eq!(
                first.data.as_slice(),
                second.data.as_slice(),
                "{} must bake bit-identically",
                preset.stem
            );
            // The recipe JSON round-trips, so the embedded metadata recipe
            // re-bakes the same table.
            let json = serde_json::to_value(&preset.recipe).expect("encode recipe");
            let decoded: Recipe = serde_json::from_value(json).expect("decode recipe");
            let rebaked = bake(&decoded).expect("re-bake from decoded recipe");
            assert_eq!(first.data.as_slice(), rebaked.data.as_slice(), "{}", preset.stem);
        }
    }

    /// Frame-motion metrics shared by the class-bounded regressions below.
    struct MotionReport {
        /// Worst adjacent-frame RMS difference in linear magnitude.
        max_step: f64,
        /// Worst / mean adjacent-frame difference in the dB authoring domain.
        max_db_step: f64,
        mean_db_step: f64,
        /// Adjacent-frame linear steps, for jump counting.
        steps: Vec<f64>,
        /// Largest distance any frame reaches from frame 0 (a closed-loop
        /// preset returns to its start, so endpoint distance is not enough).
        max_excursion: f64,
        /// Per-frame spectral centroid in octaves relative to the reference
        /// harmonic: the scalar "where is the energy" trace.
        centroid: Vec<f64>,
    }

    impl MotionReport {
        fn centroid_range(&self) -> f64 {
            let low = self.centroid.iter().copied().fold(f64::INFINITY, f64::min);
            let high = self.centroid.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            high - low
        }

        /// Direction changes of the centroid trace, ignoring steps smaller
        /// than `deadband` octaves so numerical wiggle does not count.
        fn centroid_reversals(&self, deadband: f64) -> usize {
            let mut reversals = 0usize;
            let mut previous = 0.0f64;
            for pair in self.centroid.windows(2) {
                let delta = pair[1] - pair[0];
                if delta.abs() < deadband {
                    continue;
                }
                if previous != 0.0 && delta.signum() != previous.signum() {
                    reversals += 1;
                }
                previous = delta;
            }
            reversals
        }

        /// Adjacent-frame steps at least `factor` times the median step: the
        /// discontinuities a Jump preset is made of.
        fn jump_count(&self, factor: f64) -> usize {
            let mut sorted = self.steps.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = sorted[sorted.len() / 2].max(1.0e-9);
            self.steps.iter().filter(|step| **step >= factor * median).count()
        }

        /// The original (pre-class) factory bound: one coherent one-way ramp,
        /// small bounded steps, no step carrying an outsized share of the
        /// motion, and an energy centre that does not keep changing its mind.
        /// `Glide` presets must satisfy this; `Wobble`/`Jump` presets must
        /// not — that is what the classes buy.
        fn passes_glide_bounds(&self) -> bool {
            self.max_step < 0.25
                && self.max_db_step <= 4.0 * self.mean_db_step
                && self.centroid_reversals(0.01) <= 2
        }

        /// Evidence that the response re-throws rather than travels.
        fn has_discontinuities(&self) -> bool {
            self.max_db_step >= 4.0 * self.mean_db_step
                || self.centroid_reversals(0.02) >= 4
        }
    }

    fn motion_report(table: &MagnitudeTable) -> MotionReport {
        let frame = |index: usize| &table.data[index * NBINS..(index + 1) * NBINS];
        let rms_delta = |a: &[f32], b: &[f32]| {
            (a.iter()
                .zip(b)
                .map(|(x, y)| {
                    let d = (x - y) as f64;
                    d * d
                })
                .sum::<f64>()
                / NBINS as f64)
                .sqrt()
        };
        // Same delta metric in the authoring (dB) domain, where "equal motion
        // per frame" is what a glide recipe actually promises.
        let db_delta = |a: &[f32], b: &[f32]| {
            (a.iter()
                .zip(b)
                .map(|(x, y)| {
                    let d = 20.0
                        * ((x.max(1.0e-3) as f64).log10() - (y.max(1.0e-3) as f64).log10());
                    d * d
                })
                .sum::<f64>()
                / NBINS as f64)
                .sqrt()
        };
        let steps = (1..FRAMES)
            .map(|index| rms_delta(frame(index - 1), frame(index)))
            .collect::<Vec<_>>();
        let db_steps = (1..FRAMES)
            .map(|index| db_delta(frame(index - 1), frame(index)))
            .collect::<Vec<_>>();
        let centroid = (0..FRAMES)
            .map(|index| {
                let row = frame(index);
                // Energy-weighted mean of the octave coordinate.
                let mut weight = 0.0f64;
                let mut sum = 0.0f64;
                for (bin, magnitude) in row.iter().enumerate() {
                    let energy = (*magnitude as f64) * (*magnitude as f64);
                    weight += energy;
                    sum += energy * octaves(bin) as f64;
                }
                if weight > 0.0 { sum / weight } else { 0.0 }
            })
            .collect::<Vec<_>>();
        MotionReport {
            max_step: steps.iter().copied().fold(0.0f64, f64::max),
            max_db_step: db_steps.iter().copied().fold(0.0f64, f64::max),
            mean_db_step: db_steps.iter().sum::<f64>() / db_steps.len() as f64,
            steps,
            max_excursion: (1..FRAMES)
                .map(|index| rms_delta(frame(0), frame(index)))
                .fold(0.0f64, f64::max),
            centroid,
        }
    }

    #[test]
    fn factory_presets_are_valid_and_move_within_their_motion_class() {
        for preset in factory_presets() {
            let table = bake(&preset.recipe).expect(preset.stem);
            let stem = preset.stem;
            // Class-independent sanity: finite, normalized, floored.
            let floor = 10.0f32.powf(preset.recipe.db_floor / 20.0);
            assert!(
                table
                    .data
                    .iter()
                    .all(|value| value.is_finite() && *value >= floor * 0.999 && *value <= 1.0),
                "{stem}: magnitudes must be finite and inside [floor, 1]"
            );
            for index in 0..FRAMES {
                let row = &table.data[index * NBINS..(index + 1) * NBINS];
                let peak = row.iter().copied().fold(0.0f32, f32::max);
                assert!(
                    (peak - 1.0).abs() < 1.0e-5,
                    "{stem}: frame {index} must be peak-normalized, got {peak}"
                );
            }

            let report = motion_report(&table);
            // Every preset must actually move somewhere.
            assert!(
                report.max_excursion > 0.02,
                "{stem}: the table must move across its frames, excursion={}",
                report.max_excursion
            );

            match preset.recipe.motion {
                MotionClass::Glide => {
                    // One coherent ramp: bounded steps, no single step
                    // carrying an outsized share of the total motion, and no
                    // meandering.
                    assert!(
                        report.passes_glide_bounds(),
                        "{stem}: glide presets must morph smoothly one way: worst step rms={}, dB step {} vs mean {}, reversals={}",
                        report.max_step,
                        report.max_db_step,
                        report.mean_db_step,
                        report.centroid_reversals(0.01)
                    );
                }
                MotionClass::Wobble => {
                    // Continuous but non-monotonic: bigger steps allowed, and
                    // the energy centre must genuinely turn around.
                    assert!(
                        report.max_step < 0.6,
                        "{stem}: wobble presets must stay continuous, worst step rms={}",
                        report.max_step
                    );
                    assert!(
                        report.max_db_step <= 12.0 * report.mean_db_step,
                        "{stem}: wobble motion must stay spread out: worst dB step {} vs mean {}",
                        report.max_db_step,
                        report.mean_db_step
                    );
                    assert!(
                        report.max_excursion > 0.05,
                        "{stem}: wobble presets must travel, excursion={}",
                        report.max_excursion
                    );
                    assert!(
                        report.centroid_range() > 0.5,
                        "{stem}: wobble presets must sweep their energy centre, range={} oct",
                        report.centroid_range()
                    );
                    assert!(
                        report.centroid_reversals(0.01) >= 3,
                        "{stem}: wobble presets must reverse direction, reversals={}",
                        report.centroid_reversals(0.01)
                    );
                    assert!(
                        !report.passes_glide_bounds(),
                        "{stem}: a wobble preset that satisfies the glide bounds is just a glide preset"
                    );
                }
                MotionClass::Jump => {
                    // Discontinuous on purpose, but still bounded: a jump is
                    // a re-throw of the response, never garbage.
                    assert!(
                        report.max_step < 1.0,
                        "{stem}: jump presets must stay bounded, worst step rms={}",
                        report.max_step
                    );
                    assert!(
                        report.has_discontinuities(),
                        "{stem}: jump presets must actually jump: dB step {} vs mean {}, reversals={}, big steps={}",
                        report.max_db_step,
                        report.mean_db_step,
                        report.centroid_reversals(0.02),
                        report.jump_count(3.0)
                    );
                    assert!(
                        report.max_excursion > 0.05,
                        "{stem}: jump presets must travel, excursion={}",
                        report.max_excursion
                    );
                    assert!(
                        report.centroid_range() > 0.5,
                        "{stem}: jump presets must scatter their energy centre, range={} oct",
                        report.centroid_range()
                    );
                    assert!(
                        !report.passes_glide_bounds(),
                        "{stem}: a jump preset that satisfies the glide bounds never jumps"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn dump_motion_metrics() {
        for preset in factory_presets() {
            let r = motion_report(&bake(&preset.recipe).expect(preset.stem));
            println!(
                "{:<14} {:?}\tmax_step={:.4} db={:.2}/mean{:.2}={:.2} exc={:.3} range={:.2} rev={} rev005={} jumps={}",
                preset.stem, preset.recipe.motion, r.max_step, r.max_db_step, r.mean_db_step,
                r.max_db_step / r.mean_db_step, r.max_excursion, r.centroid_range(),
                r.centroid_reversals(0.01), r.centroid_reversals(0.005), r.jump_count(3.0)
            );
        }
    }

    #[test]
    fn gulp_and_sprlonk_presets_would_fail_the_glide_bounds() {
        // The class system has to do something: these presets are *supposed*
        // to violate the glide envelope, and the glide presets are supposed to
        // satisfy it.
        let presets = factory_presets();
        let report_for = |stem: &str| {
            let preset = presets
                .iter()
                .find(|preset| preset.stem == stem)
                .unwrap_or_else(|| panic!("{stem} exists"));
            (preset.recipe.motion, motion_report(&bake(&preset.recipe).expect(stem)))
        };

        let (class, gulp) = report_for("gulp-throat");
        assert_eq!(class, MotionClass::Wobble);
        assert!(
            gulp.centroid_reversals(0.01) >= 4,
            "gulp-throat must swoop back and forth, reversals={}",
            gulp.centroid_reversals(0.01)
        );
        assert!(
            gulp.centroid_range() > 2.0,
            "gulp-throat must travel octaves, range={} oct",
            gulp.centroid_range()
        );
        assert!(
            !gulp.passes_glide_bounds(),
            "gulp-throat must break the glide envelope, or the classes buy nothing"
        );

        let (class, sprlonk) = report_for("sprlonk");
        assert_eq!(class, MotionClass::Jump);
        assert!(
            !sprlonk.passes_glide_bounds(),
            "sprlonk must break the glide envelope"
        );
        assert!(sprlonk.has_discontinuities());

        let (class, glide) = report_for("comb-bloom");
        assert_eq!(class, MotionClass::Glide);
        assert!(glide.passes_glide_bounds());
    }

    #[test]
    fn bundled_factory_assets_match_their_recipes() {
        let dir = bundled_asset_dir();
        for preset in factory_presets() {
            let path = dir.join(format!("{}.fltab", preset.stem));
            assert!(
                path.exists(),
                "bundled asset missing: {} (run `cargo run -p sequencer --bin generate_filter_tables`)",
                path.display()
            );
            let asset = read_asset(&path).expect(preset.stem);
            assert_eq!(asset.meta.name, preset.display_name);
            let recipe: Recipe = serde_json::from_value(
                asset.meta.recipe.clone().expect("factory assets embed their recipe"),
            )
            .expect("recipe decodes");
            assert_eq!(
                recipe, preset.recipe,
                "{}: bundled recipe differs from the in-code definition — regenerate the factory assets",
                preset.stem
            );
            let rebaked = bake(&recipe).expect(preset.stem);
            assert_eq!(
                asset.table.data.as_slice(),
                rebaked.data.as_slice(),
                "{}: baked file has drifted from its recipe — regenerate the factory assets",
                preset.stem
            );
            // And the stem resolves through the normal asset lookup.
            assert_eq!(resolve_asset_path(preset.stem), Some(path));
        }
    }

    #[test]
    fn bake_rejects_bad_recipes() {
        let mut recipe = Recipe {
            generator_version: GENERATOR_VERSION + 1,
            db_floor: -60.0,
            motion: MotionClass::Glide,
            elements: Vec::new(),
        };
        assert!(bake(&recipe).unwrap_err().contains("generator version"));
        recipe.generator_version = GENERATOR_VERSION;
        recipe.db_floor = 3.0;
        assert!(bake(&recipe).unwrap_err().contains("db_floor"));
    }

    #[test]
    fn bundled_dsp_cutoff_transposes_a_factory_preset() {
        let _render = crate::effects::filter_table::tests::render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        // Frame 0 of swoop-low is a lowpass closing a fifth of an octave
        // above the cutoff. At the identity cutoff a 6 kHz tone sits ~3.3
        // octaves into the rolloff; raising cutoff 8x moves the same tone
        // back up against the resonant edge.
        let preset = factory_presets()
            .into_iter()
            .find(|preset| preset.stem == "swoop-low")
            .expect("swoop-low exists");
        let table = bake(&preset.recipe).expect("bake swoop-low");
        let identity_cutoff = REFERENCE_HARMONIC as f32 * 44_100.0 / N as f32;
        let render = |cutoff: f32, tone_hz: f32| {
            crate::lisp_host::render_effect_source_for_test(
                dsp_source(),
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 16_384,
                    param_overrides: vec![
                        ("frame".to_string(), 0.0),
                        ("cutoff".to_string(), cutoff),
                        ("resonance".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: vec![(0, tone_hz, 0.4)],
                    tensor_overrides: vec![(
                        "table_magnitudes".to_string(),
                        table.data.as_ref().clone(),
                    )],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render swoop-low probe")
        };

        let passband = render(identity_cutoff, 300.0);
        let stopband = render(identity_cutoff, 6_000.0);
        assert!(
            passband.left_rms > 5.0 * stopband.left_rms.max(1.0e-6),
            "frame 0 must be a real lowpass: pass={}, stop={}",
            passband.left_rms,
            stopband.left_rms,
        );
        let reopened = render(identity_cutoff * 8.0, 6_000.0);
        assert!(
            reopened.left_rms > 4.0 * stopband.left_rms.max(1.0e-6),
            "raising cutoff must transpose the curve and reopen the 6 kHz tone: closed={}, open={}",
            stopband.left_rms,
            reopened.left_rms,
        );
    }
}
