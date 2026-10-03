//! Scale quantization for the FTS (Fit To Scale) track parameter, and the
//! per-track microtonal tuning layered on top of it
//! (docs/microtonal-scales-spec.md).
//!
//! A scale is a list of degree pitches in cents above the root plus the
//! period it repeats at (1200 = an octave). A track's [`TrackTuning`] picks
//! the root, detunes individual degrees, switches degrees off, morphs between
//! 12-TET and the exact tuning, and chooses how input pitch maps onto degrees.

use std::sync::Arc;

/// How an input pitch (semitones above the root) lands on a scale degree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TuningMode {
    /// Snap to the nearest degree: today's fit-to-scale behavior.
    #[default]
    Snap,
    /// Every input semitone is the next degree, so scales with more or fewer
    /// than 12 notes per period are fully reachable from steps and keys.
    Map,
}

impl TuningMode {
    pub fn label(self) -> &'static str {
        match self {
            TuningMode::Snap => "Snap",
            TuningMode::Map => "Map",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.to_ascii_lowercase().as_str() {
            "snap" => Some(TuningMode::Snap),
            "map" => Some(TuningMode::Map),
            _ => None,
        }
    }
}

pub struct ScaleDef {
    pub name: &'static str,
    /// Degree pitches in cents above the root, ascending, `cents[0] == 0`.
    pub cents: &'static [f32],
    /// Interval the degrees repeat at, in cents.
    pub period: f32,
    /// Input mapping a track gets when this scale is picked.
    pub mode: TuningMode,
}

/// Most degrees a scale (built-in or imported) may have; one `u64` bit each
/// in [`TrackTuning::disabled`]. Covers 53-EDO.
pub const MAX_SCALE_DEGREES: usize = 64;

/// Indexed by the persisted `fts_scale`: APPEND ONLY. Reordering or inserting
/// retunes every saved project that picked a later scale.
pub const SCALES: &[ScaleDef] = &[
    ScaleDef {
        name: "Off",
        cents: &[],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Major",
        cents: &[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Minor",
        cents: &[0.0, 200.0, 300.0, 500.0, 700.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Dorian",
        cents: &[0.0, 200.0, 300.0, 500.0, 700.0, 900.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Mixolydian",
        cents: &[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Lydian",
        cents: &[0.0, 200.0, 400.0, 600.0, 700.0, 900.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Phrygian",
        cents: &[0.0, 100.0, 300.0, 500.0, 700.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Locrian",
        cents: &[0.0, 100.0, 300.0, 500.0, 600.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Pent. Major",
        cents: &[0.0, 200.0, 400.0, 700.0, 900.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Pent. Minor",
        cents: &[0.0, 300.0, 500.0, 700.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Blues",
        cents: &[0.0, 300.0, 500.0, 600.0, 700.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Whole Tone",
        cents: &[0.0, 200.0, 400.0, 600.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Diminished",
        cents: &[0.0, 200.0, 300.0, 500.0, 600.0, 800.0, 900.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    // Appended 2026-10-02 (eseq-th7i). Append only, see above.
    ScaleDef {
        name: "Harmonic Minor",
        cents: &[0.0, 200.0, 300.0, 500.0, 700.0, 800.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Melodic Minor",
        cents: &[0.0, 200.0, 300.0, 500.0, 700.0, 900.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Hungarian Minor",
        cents: &[0.0, 200.0, 300.0, 600.0, 700.0, 800.0, 1100.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Phrygian Dom.",
        cents: &[0.0, 100.0, 400.0, 500.0, 700.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Hirajoshi",
        cents: &[0.0, 200.0, 300.0, 700.0, 800.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "In (Miyako)",
        cents: &[0.0, 100.0, 500.0, 700.0, 800.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Just Major",
        cents: &[0.0, 203.91, 386.314, 498.045, 701.955, 884.359, 1088.269],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Just Minor",
        cents: &[0.0, 203.91, 315.641, 498.045, 701.955, 813.686, 1017.596],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Just Chromatic",
        cents: &[0.0, 111.731, 203.91, 315.641, 386.314, 498.045, 590.224, 701.955, 813.686, 884.359, 1017.596, 1088.269],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Pythagorean",
        cents: &[0.0, 203.91, 407.82, 498.045, 701.955, 905.865, 1109.775],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Meantone 1/4",
        cents: &[0.0, 76.049, 193.157, 310.265, 386.314, 503.422, 579.471, 696.578, 772.627, 889.735, 1006.843, 1082.892],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Werckmeister III",
        cents: &[0.0, 90.225, 192.18, 294.135, 390.225, 498.045, 588.27, 696.09, 792.18, 888.27, 996.09, 1092.18],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Harmonic 8-15",
        cents: &[0.0, 203.91, 386.314, 551.318, 701.955, 840.528, 968.826, 1088.269],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Maqam Rast",
        cents: &[0.0, 200.0, 350.0, 500.0, 700.0, 900.0, 1050.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Maqam Bayati",
        cents: &[0.0, 150.0, 300.0, 500.0, 700.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Maqam Saba",
        cents: &[0.0, 150.0, 300.0, 400.0, 700.0, 800.0, 1000.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Pelog",
        cents: &[0.0, 120.0, 270.0, 540.0, 670.0, 785.0, 950.0],
        period: 1200.0,
        mode: TuningMode::Snap,
    },
    ScaleDef {
        name: "Slendro",
        cents: &[0.0, 240.0, 480.0, 720.0, 960.0],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "7-EDO",
        cents: &[0.0, 171.429, 342.857, 514.286, 685.714, 857.143, 1028.571],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "19-EDO",
        cents: &[0.0, 63.158, 126.316, 189.474, 252.632, 315.789, 378.947, 442.105, 505.263, 568.421, 631.579, 694.737, 757.895, 821.053, 884.211, 947.368, 1010.526, 1073.684, 1136.842],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "22-EDO",
        cents: &[0.0, 54.545, 109.091, 163.636, 218.182, 272.727, 327.273, 381.818, 436.364, 490.909, 545.455, 600.0, 654.545, 709.091, 763.636, 818.182, 872.727, 927.273, 981.818, 1036.364, 1090.909, 1145.455],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "24-EDO",
        cents: &[0.0, 50.0, 100.0, 150.0, 200.0, 250.0, 300.0, 350.0, 400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0, 750.0, 800.0, 850.0, 900.0, 950.0, 1000.0, 1050.0, 1100.0, 1150.0],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "31-EDO",
        cents: &[0.0, 38.71, 77.419, 116.129, 154.839, 193.548, 232.258, 270.968, 309.677, 348.387, 387.097, 425.806, 464.516, 503.226, 541.935, 580.645, 619.355, 658.065, 696.774, 735.484, 774.194, 812.903, 851.613, 890.323, 929.032, 967.742, 1006.452, 1045.161, 1083.871, 1122.581, 1161.29],
        period: 1200.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "Bohlen-Pierce",
        cents: &[0.0, 146.304, 292.608, 438.913, 585.217, 731.521, 877.825, 1024.13, 1170.434, 1316.738, 1463.042, 1609.347, 1755.651],
        period: 1901.955,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "Carlos Alpha",
        cents: &[0.0],
        period: 78.0,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "Carlos Beta",
        cents: &[0.0],
        period: 63.8,
        mode: TuningMode::Map,
    },
    ScaleDef {
        name: "Carlos Gamma",
        cents: &[0.0],
        period: 35.1,
        mode: TuningMode::Map,
    },
];

/// Index of "Off": no quantization, whatever the tuning says.
pub const SCALE_OFF: usize = 0;

/// An imported Scala scale that replaces a track's built-in base table.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomScale {
    pub name: String,
    /// Degree pitches in cents, `cents[0] == 0`, in the file's order.
    pub cents: Vec<f32>,
    pub period: f32,
}

/// A track's edits on top of its picked scale. Part of `TrackParamsSnapshot`,
/// so undo, scenes, sounds and the scheduler snapshot all carry it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackTuning {
    /// Pitch class (0 = C) degree 0 sits on.
    pub root: u8,
    /// 0 = every degree rounded to its nearest 12-TET semitone, 1 = exact.
    pub morph: f32,
    pub mode: TuningMode,
    /// Cents added to each base degree.
    pub offsets: [f32; MAX_SCALE_DEGREES],
    /// Bit `k` set = degree `k` is out of the scale.
    pub disabled: u64,
    /// Imported scale that replaces the built-in table while the scale is on.
    pub custom: Option<Arc<CustomScale>>,
}

impl TrackTuning {
    pub const DEFAULT: TrackTuning = TrackTuning {
        root: 0,
        morph: 1.0,
        mode: TuningMode::Snap,
        offsets: [0.0; MAX_SCALE_DEGREES],
        disabled: 0,
        custom: None,
    };

    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }

    /// Per-degree edits away from the picked scale (offsets or disabled
    /// degrees); root, morph and mode are whole-scale settings, not edits.
    pub fn has_degree_edits(&self) -> bool {
        self.disabled != 0 || self.offsets.iter().any(|offset| *offset != 0.0)
    }

    pub fn degree_enabled(&self, degree: usize) -> bool {
        degree < MAX_SCALE_DEGREES && self.disabled & (1 << degree) == 0
    }

    /// The tuning after picking `scale_idx`: degree edits and any imported
    /// scale are dropped and the scale's own input mapping applies; root and
    /// morph are kept.
    pub fn for_scale(&self, scale_idx: usize) -> TrackTuning {
        TrackTuning {
            root: self.root,
            morph: self.morph,
            mode: SCALES.get(scale_idx).map(|scale| scale.mode).unwrap_or_default(),
            ..Self::DEFAULT
        }
    }

    /// Clears offsets and disabled degrees, keeping everything else.
    pub fn without_degree_edits(&self) -> TrackTuning {
        TrackTuning {
            offsets: [0.0; MAX_SCALE_DEGREES],
            disabled: 0,
            ..self.clone()
        }
    }
}

impl Default for TrackTuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The table a track quantizes against: its imported scale, else the
/// built-in one. `None` when the scale is Off.
pub fn base_scale(scale_idx: usize, tuning: &TrackTuning) -> Option<(&[f32], f32)> {
    if scale_idx == SCALE_OFF {
        return None;
    }
    if let Some(custom) = &tuning.custom {
        return (!custom.cents.is_empty() && custom.period > 0.0)
            .then_some((custom.cents.as_slice(), custom.period));
    }
    let scale = SCALES.get(scale_idx)?;
    (!scale.cents.is_empty()).then_some((scale.cents, scale.period))
}

/// Display name: the imported scale's, else the built-in one's.
pub fn scale_name(scale_idx: usize, tuning: &TrackTuning) -> &str {
    if scale_idx != SCALE_OFF {
        if let Some(custom) = &tuning.custom {
            return &custom.name;
        }
    }
    SCALES.get(scale_idx).map(|scale| scale.name).unwrap_or("Off")
}

/// Sounding pitch of degree `degree` in cents above the root (offset and
/// morph applied).
pub fn degree_pitch(base: &[f32], degree: usize, tuning: &TrackTuning) -> f32 {
    let tuned = base[degree] + tuning.offsets.get(degree).copied().unwrap_or(0.0);
    let et = (tuned / 100.0).round() * 100.0;
    et + (tuned - et) * tuning.morph.clamp(0.0, 1.0)
}

/// Fills `out` with the sounding pitch of every enabled degree; returns how
/// many. Stack-only: runs on the audio thread for live keys.
fn enabled_pitches(
    base: &[f32],
    tuning: &TrackTuning,
    out: &mut [f32; MAX_SCALE_DEGREES],
) -> usize {
    let mut count = 0;
    for degree in 0..base.len().min(MAX_SCALE_DEGREES) {
        if tuning.degree_enabled(degree) {
            out[count] = degree_pitch(base, degree, tuning);
            count += 1;
        }
    }
    count
}

/// Fit `transpose` (semitones above C, any float) to the track's tuned scale.
/// Returns it unchanged when the scale is Off or every degree is disabled.
pub fn quantize(transpose: f32, scale_idx: usize, tuning: &TrackTuning) -> f32 {
    let Some((base, period)) = base_scale(scale_idx, tuning) else {
        return transpose;
    };
    if !(period > 0.0) || !transpose.is_finite() {
        return transpose;
    }
    let mut pitches = [0.0f32; MAX_SCALE_DEGREES];
    let count = enabled_pitches(base, tuning, &mut pitches);
    if count == 0 {
        return transpose;
    }
    // Semitones throughout, not cents: with a legacy scale (whole-semitone
    // degrees, octave period, root C) this is the original quantizer's exact
    // float arithmetic, so old projects sound bit for bit the same.
    for pitch in &mut pitches[..count] {
        *pitch /= 100.0;
    }
    let pitches = &pitches[..count];
    let pitches = &pitches[..count];
    let period = period / 100.0;
    let root = f32::from(tuning.root % 12);
    let relative = transpose - root;
    let semitones = match tuning.mode {
        TuningMode::Snap => {
            let period_index = (relative / period).floor();
            let within = relative - period_index * period;
            // Same tie order as the original quantizer: degrees in order,
            // then the next period, then the previous one, each only on a
            // strictly closer match.
            let mut best = pitches[0];
            let mut best_dist = (within - best).abs();
            for shift in [0.0, period, -period] {
                for &pitch in pitches {
                    let candidate = pitch + shift;
                    let dist = (within - candidate).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        best = candidate;
                    }
                }
            }
            period_index * period + best
        }
        TuningMode::Map => {
            let step = relative.floor();
            let frac = relative - step;
            let at = |step: f32| {
                let step = step as i64;
                let count = pitches.len() as i64;
                step.div_euclid(count) as f32 * period
                    + pitches[step.rem_euclid(count) as usize]
            };
            let low = at(step);
            if frac == 0.0 {
                low
            } else {
                low + (at(step + 1.0) - low) * frac
            }
        }
    };
    if root == 0.0 {
        semitones
    } else {
        root + semitones
    }
}

/// [`quantize`] with an untouched tuning: plain fit-to-scale.
pub fn quantize_transpose(transpose: f32, scale_idx: usize) -> f32 {
    quantize(transpose, scale_idx, &TrackTuning::DEFAULT)
}

// ---------------------------------------------------------------------------
// Whole-scale tools (scale editor). Each returns the edited tuning.

/// 7-limit just ratios within an octave, `(numerator, denominator)`.
const JUST_RATIOS: &[(u32, u32)] = &[
    (1, 1),
    (16, 15),
    (10, 9),
    (9, 8),
    (8, 7),
    (7, 6),
    (6, 5),
    (5, 4),
    (9, 7),
    (4, 3),
    (7, 5),
    (10, 7),
    (3, 2),
    (14, 9),
    (8, 5),
    (5, 3),
    (12, 7),
    (7, 4),
    (16, 9),
    (9, 5),
    (15, 8),
    (2, 1),
];

fn ratio_cents(num: u32, den: u32) -> f32 {
    (1200.0 * (f64::from(num) / f64::from(den)).log2()) as f32
}

/// Nearest just ratio to `cents` (folded into one octave) within
/// `tolerance` cents: `(num, den, error_cents)`.
pub fn nearest_just_ratio(cents: f32, tolerance: f32) -> Option<(u32, u32, f32)> {
    let folded = cents.rem_euclid(1200.0);
    JUST_RATIOS
        .iter()
        .map(|&(num, den)| (num, den, folded - ratio_cents(num, den)))
        .filter(|(_, _, error)| error.abs() <= tolerance)
        .min_by(|a, b| a.2.abs().total_cmp(&b.2.abs()))
}

/// Moves every enabled degree onto the nearest just ratio within 50 cents.
/// Octave-period scales only; others come back unchanged.
pub fn justify(scale_idx: usize, tuning: &TrackTuning) -> TrackTuning {
    let mut out = tuning.clone();
    let Some((base, period)) = base_scale(scale_idx, tuning) else {
        return out;
    };
    if (period - 1200.0).abs() > 0.5 {
        return out;
    }
    for degree in 0..base.len().min(MAX_SCALE_DEGREES) {
        if !tuning.degree_enabled(degree) {
            continue;
        }
        let tuned = base[degree] + tuning.offsets[degree];
        if let Some((_, _, error)) = nearest_just_ratio(tuned, 50.0) {
            out.offsets[degree] = tuning.offsets[degree] - error;
        }
    }
    out
}

/// Adds uniform noise in `±amount` cents to every enabled degree but the
/// root. Deterministic for a given `seed`.
pub fn randomize(scale_idx: usize, tuning: &TrackTuning, amount: f32, seed: u64) -> TrackTuning {
    let mut out = tuning.clone();
    let Some((base, _)) = base_scale(scale_idx, tuning) else {
        return out;
    };
    let mut state = seed | 1;
    for degree in 1..base.len().min(MAX_SCALE_DEGREES) {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let bits = state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        let unit = bits as f32 / (1u64 << 24) as f32;
        if tuning.degree_enabled(degree) {
            out.offsets[degree] = tuning.offsets[degree] + (unit * 2.0 - 1.0) * amount;
        }
    }
    out
}

/// Piano-style stretch: each degree moves by `cents · base / period`, so the
/// top of the period gains about `cents` and the root stays put.
pub fn stretch(scale_idx: usize, tuning: &TrackTuning, cents: f32) -> TrackTuning {
    let mut out = tuning.clone();
    let Some((base, period)) = base_scale(scale_idx, tuning) else {
        return out;
    };
    for degree in 0..base.len().min(MAX_SCALE_DEGREES) {
        if tuning.degree_enabled(degree) {
            out.offsets[degree] = tuning.offsets[degree] + cents * base[degree] / period;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Scala `.scl` import (https://www.huygens-fokker.org/scala/scl_format.html).

/// Parses a Scala scale file. `fallback_name` names it when the description
/// line is blank (usually the file stem).
pub fn parse_scl(text: &str, fallback_name: &str) -> Result<CustomScale, String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('!'));
    let description = lines.next().ok_or("empty .scl file")?;
    let count: usize = lines
        .next()
        .and_then(|line| line.split_whitespace().next())
        .ok_or("missing note count")?
        .parse()
        .map_err(|_| "note count is not a number".to_string())?;
    if count == 0 {
        return Err("scale has no notes".to_string());
    }
    if count > MAX_SCALE_DEGREES {
        return Err(format!(
            "scale has {count} notes; at most {MAX_SCALE_DEGREES} are supported"
        ));
    }
    let mut pitches = Vec::with_capacity(count);
    for line in lines.filter(|line| !line.is_empty()).take(count) {
        let token = line.split_whitespace().next().unwrap_or("");
        pitches.push(parse_scl_pitch(token)?);
    }
    if pitches.len() < count {
        return Err(format!("expected {count} pitches, found {}", pitches.len()));
    }
    let period = pitches.pop().unwrap_or(1200.0);
    if !(period > 0.0) {
        return Err("period (last pitch) must be above the root".to_string());
    }
    let mut cents = Vec::with_capacity(count);
    cents.push(0.0);
    cents.extend(pitches);
    let name = if description.is_empty() {
        fallback_name.to_string()
    } else {
        description.to_string()
    };
    Ok(CustomScale {
        name,
        cents,
        period,
    })
}

fn parse_scl_pitch(token: &str) -> Result<f32, String> {
    let invalid = || format!("invalid pitch '{token}'");
    if token.contains('.') {
        return token.parse::<f32>().map_err(|_| invalid());
    }
    let (num, den) = match token.split_once('/') {
        Some((num, den)) => (num, den),
        None => (token, "1"),
    };
    let num: f64 = num.parse().map_err(|_| invalid())?;
    let den: f64 = den.parse().map_err(|_| invalid())?;
    if !(num > 0.0 && den > 0.0) {
        return Err(invalid());
    }
    Ok((1200.0 * (num / den).log2()) as f32)
}

/// "E-14" style name of a pitch `cents` above C: nearest 12-TET note plus the
/// deviation in whole cents (omitted when under half a cent).
pub fn pitch_label(cents: f32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let semitone = (cents / 100.0).round();
    let name = NAMES[(semitone as i64).rem_euclid(12) as usize];
    let deviation = (cents - semitone * 100.0).round() as i32;
    match deviation {
        0 => name.to_string(),
        d if d > 0 => format!("{name}+{d}"),
        d => format!("{name}{d}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The semitone quantizer this module replaced, kept to pin legacy
    /// scales bit for bit.
    fn legacy_quantize(transpose: f32, degrees: &[u8]) -> f32 {
        let octave = (transpose / 12.0).floor();
        let degree_f = transpose - octave * 12.0;
        let mut best = degrees[0] as f32;
        let mut best_dist = (degree_f - best).abs();
        for &d in degrees.iter().skip(1) {
            let dist = (degree_f - d as f32).abs();
            if dist < best_dist {
                best_dist = dist;
                best = d as f32;
            }
        }
        let wrap_dist = (degree_f - 12.0 - degrees[0] as f32).abs();
        if wrap_dist < best_dist {
            best = 12.0 + degrees[0] as f32;
        }
        octave * 12.0 + best
    }

    fn tuning(edit: impl FnOnce(&mut TrackTuning)) -> TrackTuning {
        let mut tuning = TrackTuning::DEFAULT;
        edit(&mut tuning);
        tuning
    }

    fn index(name: &str) -> usize {
        SCALES.iter().position(|scale| scale.name == name).expect(name)
    }

    #[test]
    fn legacy_scales_keep_their_persisted_indices_and_quantize_identically() {
        let legacy: &[(&str, &[u8])] = &[
            ("Major", &[0, 2, 4, 5, 7, 9, 11]),
            ("Minor", &[0, 2, 3, 5, 7, 8, 10]),
            ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
            ("Mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
            ("Lydian", &[0, 2, 4, 6, 7, 9, 11]),
            ("Phrygian", &[0, 1, 3, 5, 7, 8, 10]),
            ("Locrian", &[0, 1, 3, 5, 6, 8, 10]),
            ("Pent. Major", &[0, 2, 4, 7, 9]),
            ("Pent. Minor", &[0, 3, 5, 7, 10]),
            ("Blues", &[0, 3, 5, 6, 7, 10]),
            ("Whole Tone", &[0, 2, 4, 6, 8, 10]),
            ("Diminished", &[0, 2, 3, 5, 6, 8, 9, 11]),
        ];
        assert_eq!(SCALES[0].name, "Off");
        for (offset, (name, degrees)) in legacy.iter().enumerate() {
            let idx = offset + 1;
            assert_eq!(SCALES[idx].name, *name, "index {idx} moved");
            for tenth in -480..=480 {
                let transpose = tenth as f32 * 0.1;
                assert_eq!(
                    quantize_transpose(transpose, idx),
                    legacy_quantize(transpose, degrees),
                    "{name} at {transpose}"
                );
            }
        }
        assert_eq!(quantize_transpose(3.3, 0), 3.3);
    }

    #[test]
    fn every_scale_starts_at_zero_ascends_and_fits_the_degree_budget() {
        for scale in &SCALES[1..] {
            assert_eq!(scale.cents[0], 0.0, "{}", scale.name);
            assert!(scale.cents.len() <= MAX_SCALE_DEGREES, "{}", scale.name);
            assert!(scale.cents.windows(2).all(|w| w[0] < w[1]), "{}", scale.name);
            assert!(*scale.cents.last().unwrap() < scale.period, "{}", scale.name);
        }
    }

    #[test]
    fn just_major_detunes_the_third_and_morph_zero_is_twelve_tet() {
        let just = index("Just Major");
        let third = quantize(4.0, just, &TrackTuning::DEFAULT);
        assert!((third - 3.8631).abs() < 1e-3, "{third}");
        let flat = tuning(|t| t.morph = 0.0);
        assert_eq!(quantize(4.0, just, &flat), 4.0);
        let half = tuning(|t| t.morph = 0.5);
        assert!((quantize(4.0, just, &half) - 3.93157).abs() < 1e-3);
    }

    #[test]
    fn offsets_root_and_disabled_degrees_shape_snap() {
        let major = index("Major");
        let sharp_fifth = tuning(|t| t.offsets[4] = 30.0);
        assert!((quantize(7.0, major, &sharp_fifth) - 7.3).abs() < 1e-5);
        assert!((quantize(19.0, major, &sharp_fifth) - 19.3).abs() < 1e-5);
        // D major: C# is a degree; F ties E/F# and keeps the lower one.
        let d_major = tuning(|t| t.root = 2);
        assert_eq!(quantize(1.0, major, &d_major), 1.0);
        assert_eq!(quantize(5.0, major, &d_major), 4.0);
        // Without the 3rd, E snaps up to F (a semitone) rather than D.
        let no_third = tuning(|t| t.disabled = 1 << 2);
        assert_eq!(quantize(4.0, major, &no_third), 5.0);
        let all_off = tuning(|t| t.disabled = u64::MAX);
        assert_eq!(quantize(4.4, major, &all_off), 4.4);
    }

    #[test]
    fn snap_wraps_below_the_root_when_a_low_degree_is_off() {
        let major = index("Major");
        let no_root = tuning(|t| t.disabled = 1);
        // 0.4 above C with C out: B below (−1) is 1.4 away, D is 1.6 away.
        assert_eq!(quantize(0.4, major, &no_root), -1.0);
    }

    #[test]
    fn map_steps_through_every_degree_of_an_edo() {
        let edo19 = index("19-EDO");
        let map = tuning(|t| t.mode = TuningMode::Map);
        let step = 1200.0 / 19.0 / 100.0;
        assert!((quantize(1.0, edo19, &map) - step).abs() < 1e-5);
        assert!((quantize(19.0, edo19, &map) - 12.0).abs() < 1e-4);
        assert!((quantize(-1.0, edo19, &map) - (12.0 - step - 12.0)).abs() < 1e-4);
        assert!((quantize(0.5, edo19, &map) - step * 0.5).abs() < 1e-5);
        let alpha = index("Carlos Alpha");
        assert!((quantize(9.0, alpha, &map) - 7.02).abs() < 1e-4);
        let bp = index("Bohlen-Pierce");
        assert!((quantize(13.0, bp, &map) - 19.01955).abs() < 1e-3);
    }

    #[test]
    fn picking_a_scale_keeps_root_and_morph_but_drops_degree_edits() {
        let edited = tuning(|t| {
            t.root = 5;
            t.morph = 0.25;
            t.offsets[1] = 12.0;
            t.disabled = 0b100;
        });
        let picked = edited.for_scale(index("24-EDO"));
        assert_eq!(picked.root, 5);
        assert_eq!(picked.morph, 0.25);
        assert_eq!(picked.mode, TuningMode::Map);
        assert!(!picked.has_degree_edits());
    }

    #[test]
    fn tools_justify_stretch_and_randomize() {
        let major = index("Major");
        let just = justify(major, &TrackTuning::DEFAULT);
        assert!((just.offsets[2] + 13.686).abs() < 1e-2, "{}", just.offsets[2]);
        assert!((just.offsets[4] - 1.955).abs() < 1e-2);
        assert_eq!(just.offsets[0], 0.0);

        let stretched = stretch(major, &TrackTuning::DEFAULT, 12.0);
        assert_eq!(stretched.offsets[0], 0.0);
        assert!((stretched.offsets[6] - 11.0).abs() < 1e-4);

        let a = randomize(major, &TrackTuning::DEFAULT, 20.0, 7);
        let b = randomize(major, &TrackTuning::DEFAULT, 20.0, 7);
        assert_eq!(a, b);
        assert_eq!(a.offsets[0], 0.0);
        assert!(a.offsets[1..7].iter().all(|o| o.abs() <= 20.0));
        assert!(a.offsets[1..7].iter().any(|o| *o != 0.0));
        assert!(a.offsets[7..].iter().all(|o| *o == 0.0));
    }

    #[test]
    fn parses_scala_cents_ratios_and_comments() {
        let scl = "! meantone.scl\n!\nQuarter-comma test\n 3\n!\n 193.157\n5/4 major third\n2\n";
        let scale = parse_scl(scl, "meantone").unwrap();
        assert_eq!(scale.name, "Quarter-comma test");
        assert_eq!(scale.cents.len(), 3);
        assert!((scale.cents[1] - 193.157).abs() < 1e-3);
        assert!((scale.cents[2] - 386.3137).abs() < 1e-3);
        assert!((scale.period - 1200.0).abs() < 1e-3);
        let blank = parse_scl("\n1\n3/1\n", "bp").unwrap();
        assert_eq!(blank.name, "bp");
        assert_eq!(blank.cents, vec![0.0]);
        assert!(parse_scl("x\n2\n100.0\n", "x").is_err());
        assert!(parse_scl("x\n1\n-3/2\n", "x").is_err());
        assert!(parse_scl("x\n99\n", "x").is_err());
    }

    #[test]
    fn custom_scale_replaces_the_base_table_while_the_scale_is_on() {
        let custom = tuning(|t| {
            t.custom = Some(Arc::new(CustomScale {
                name: "tri".into(),
                cents: vec![0.0, 400.0, 800.0],
                period: 1200.0,
            }));
        });
        assert_eq!(quantize(5.0, 1, &custom), 4.0);
        assert_eq!(quantize(5.0, SCALE_OFF, &custom), 5.0);
        assert_eq!(scale_name(1, &custom), "tri");
        assert_eq!(scale_name(SCALE_OFF, &custom), "Off");
    }

    #[test]
    fn pitch_labels_name_the_nearest_note_and_deviation() {
        assert_eq!(pitch_label(386.31), "E-14");
        assert_eq!(pitch_label(702.0), "G+2");
        assert_eq!(pitch_label(1100.0), "B");
        assert_eq!(nearest_just_ratio(386.0, 5.0).map(|r| (r.0, r.1)), Some((5, 4)));
    }
}
