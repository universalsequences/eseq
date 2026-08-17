//! Original Filter Table factory presets and their deterministic generator
//! (eseq-dtx.7).
//!
//! Presets are authored as dB response curves on an octave coordinate
//! relative to the table's reference harmonic (bin 24 sits at 0 octaves, so
//! the `cutoff` parameter transposes every curve by construction). Each
//! preset is a small set of parametric elements whose parameters sweep
//! linearly from frame 0 to frame 63, which keeps frame motion a single
//! coherent trajectory rather than jumps between unrelated responses.
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
pub const GENERATOR_VERSION: u32 = 1;

/// A parameter that sweeps linearly across the 64 frames.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub from: f32,
    pub to: f32,
}

/// A constant-across-frames sweep.
pub fn hold(value: f32) -> Sweep {
    Sweep { from: value, to: value }
}

pub fn sweep(from: f32, to: f32) -> Sweep {
    Sweep { from, to }
}

impl Sweep {
    fn at(self, frame: f32) -> f32 {
        self.from + (self.to - self.from) * frame
    }
}

/// One additive dB-domain curve element. `center`/`edge`/`pivot`/`base`
/// coordinates are octaves relative to the reference harmonic (0.0 = the
/// cutoff frequency itself).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum Element {
    /// 0 dB below `edge`, rolling off at `slope_db_per_octave` above it.
    Lowpass { edge: Sweep, slope_db_per_octave: Sweep },
    /// 0 dB above `edge`, rolling off below it.
    Highpass { edge: Sweep, slope_db_per_octave: Sweep },
    /// Gaussian peak (or dip, with negative `gain_db`) in dB.
    Peak { center: Sweep, width_octaves: Sweep, gain_db: Sweep },
    /// Cosine comb over harmonic number `n = bin / (24 * spacing)`; dips
    /// reach `-depth_db`. `stretch` warps the tooth positions upward like a
    /// stiff string (`n' = n * (1 + stretch * n)`), making the comb
    /// inharmonic.
    Comb { spacing: Sweep, depth_db: Sweep, stretch: Sweep },
    /// `count` Gaussian notches evenly spread over `span_octaves` starting
    /// at `base` — a phaser-style all-pass-bank magnitude signature.
    NotchBank {
        count: u32,
        base: Sweep,
        span_octaves: Sweep,
        depth_db: Sweep,
        width_octaves: Sweep,
    },
    /// Linear-in-octaves spectral tilt through `pivot`.
    Tilt { db_per_octave: Sweep, pivot: Sweep },
    /// Emphasis mask over integer harmonics of the reference: bins near odd
    /// harmonics get `odd_db`, near even harmonics `even_db`, and the space
    /// between falls toward `rest_db`. `width` is the Gaussian half-width
    /// around each harmonic in harmonic-number units.
    HarmonicMask {
        odd_db: Sweep,
        even_db: Sweep,
        rest_db: Sweep,
        width: Sweep,
    },
    /// Seeded smooth spectral noise: value noise on an octave grid of
    /// `grain_octaves` spacing, mapped to `+/- depth_db`. Frames morph by
    /// sliding the sample position `drift_octaves` across the table, so
    /// motion is a continuous scroll rather than a re-roll.
    Texture {
        seed: u32,
        grain_octaves: Sweep,
        depth_db: Sweep,
        drift_octaves: Sweep,
    },
}

/// A complete deterministic generation recipe, embedded verbatim in the
/// asset's `recipe` metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub generator_version: u32,
    /// dB clamp floor after per-frame peak normalization.
    pub db_floor: f32,
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
            Element::NotchBank { count, base, span_octaves, depth_db, width_octaves } => {
                let count = (*count).max(1);
                let mut db = 0.0f32;
                for notch in 0..count {
                    let along = if count == 1 {
                        0.0
                    } else {
                        notch as f32 / (count - 1) as f32
                    };
                    let center = base.at(frame) + span_octaves.at(frame) * along;
                    db -= depth_db.at(frame)
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
/// see each `intent` line for what the 64-frame motion does.
pub fn factory_presets() -> Vec<PresetDefinition> {
    vec![
        PresetDefinition {
            stem: "glide-low",
            display_name: "Glide Low",
            intent: "classic lowpass sweep: dark 12 dB/oct closing at half an octave opens to airy 24 dB/oct four octaves up",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                elements: vec![Element::Lowpass {
                    edge: sweep(0.5, 4.5),
                    slope_db_per_octave: sweep(12.0, 24.0),
                }],
            },
        },
        PresetDefinition {
            stem: "glide-high",
            display_name: "Glide High",
            intent: "mirrored highpass sweep: thin and whistly up top, morphing down until the full body returns",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                elements: vec![Element::Highpass {
                    edge: sweep(4.0, -1.0),
                    slope_db_per_octave: sweep(24.0, 12.0),
                }],
            },
        },
        PresetDefinition {
            stem: "band-flight",
            display_name: "Band Flight",
            intent: "a resonant bandpass that climbs three octaves while tightening from a broad band to a singing peak",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                elements: vec![
                    Element::Peak {
                        center: sweep(0.0, 3.0),
                        width_octaves: sweep(1.2, 0.35),
                        gain_db: hold(48.0),
                    },
                    Element::Tilt { db_per_octave: hold(-2.0), pivot: hold(0.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "notch-drift",
            display_name: "Notch Drift",
            intent: "one deep notch drifting up through the mids, widening as it goes — a slow manual phaser",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -60.0,
                elements: vec![
                    Element::Peak {
                        center: sweep(-0.5, 3.5),
                        width_octaves: sweep(0.25, 0.7),
                        gain_db: sweep(-42.0, -30.0),
                    },
                    Element::Lowpass { edge: hold(5.0), slope_db_per_octave: hold(18.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "vowel-drift",
            display_name: "Vowel Drift",
            intent: "three-formant vocal morph gliding from a dark rounded vowel to a bright open one",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -54.0,
                elements: vec![
                    // Formant peaks in octaves above the reference; the trio
                    // moves together from back-vowel to front-vowel spacing.
                    Element::Peak {
                        center: sweep(-0.6, 0.4),
                        width_octaves: hold(0.28),
                        gain_db: hold(36.0),
                    },
                    Element::Peak {
                        center: sweep(0.6, 2.2),
                        width_octaves: hold(0.24),
                        gain_db: sweep(24.0, 34.0),
                    },
                    Element::Peak {
                        center: sweep(2.4, 3.2),
                        width_octaves: hold(0.3),
                        gain_db: sweep(14.0, 26.0),
                    },
                    Element::Lowpass { edge: hold(4.2), slope_db_per_octave: hold(24.0) },
                ],
            },
        },
        PresetDefinition {
            stem: "comb-bloom",
            display_name: "Comb Bloom",
            intent: "harmonic comb fading in: flat at first, blooming into deep evenly spaced teeth",
            default_controls: DEFAULT_CONTROLS,
            recipe: Recipe {
                generator_version: GENERATOR_VERSION,
                db_floor: -48.0,
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
                elements: vec![Element::NotchBank {
                    count: 6,
                    base: sweep(-1.0, 1.5),
                    span_octaves: hold(4.0),
                    depth_db: hold(30.0),
                    width_octaves: hold(0.22),
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
                elements: vec![
                    Element::Texture {
                        seed: 0x00C0FFEE,
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

    #[test]
    fn factory_presets_have_smooth_intentional_frame_motion() {
        for preset in factory_presets() {
            let table = bake(&preset.recipe).expect(preset.stem);
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
            // Same delta metric in the authoring (dB) domain, where "equal
            // motion per frame" is what the recipes actually promise.
            let db_delta = |a: &[f32], b: &[f32]| {
                (a.iter()
                    .zip(b)
                    .map(|(x, y)| {
                        let d = 20.0
                            * ((x.max(1.0e-3) as f64).log10()
                                - (y.max(1.0e-3) as f64).log10());
                        d * d
                    })
                    .sum::<f64>()
                    / NBINS as f64)
                    .sqrt()
            };
            let max_step = (1..FRAMES)
                .map(|index| rms_delta(frame(index - 1), frame(index)))
                .fold(0.0f64, f64::max);
            let db_steps = (1..FRAMES)
                .map(|index| db_delta(frame(index - 1), frame(index)))
                .collect::<Vec<_>>();
            let max_db_step = db_steps.iter().copied().fold(0.0f64, f64::max);
            let mean_db_step = db_steps.iter().sum::<f64>() / db_steps.len() as f64;
            // "Coherent morph family" means motion is spread across the
            // table, not concentrated in a jump between unrelated responses:
            // no frame step may be large in absolute (linear) terms, and in
            // the dB authoring domain no step may carry an outsized share of
            // the total motion.
            assert!(
                max_step < 0.25,
                "{}: adjacent frames must morph smoothly, worst step rms={max_step}",
                preset.stem
            );
            assert!(
                max_db_step <= 4.0 * mean_db_step,
                "{}: frame motion must be gradual, not a jump: worst dB step {max_db_step} vs mean {mean_db_step}",
                preset.stem
            );
            let total = rms_delta(frame(0), frame(FRAMES - 1));
            assert!(
                total > 0.02,
                "{}: the table must actually move across its frames, span rms={total}",
                preset.stem
            );
        }
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

        // Frame 0 of glide-low is a lowpass closing half an octave above the
        // cutoff. At the identity cutoff a 6 kHz tone sits ~3.5 octaves into
        // the rolloff; raising cutoff 8x moves the same tone near the edge.
        let preset = factory_presets()
            .into_iter()
            .find(|preset| preset.stem == "glide-low")
            .expect("glide-low exists");
        let table = bake(&preset.recipe).expect("bake glide-low");
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
            .expect("render glide-low probe")
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
