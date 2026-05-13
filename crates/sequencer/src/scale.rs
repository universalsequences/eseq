/// Scale quantization for the FTS (Fit To Scale) track parameter.

pub struct ScaleDef {
    pub name: &'static str,
    /// Semitone offsets from root within one octave, sorted ascending 0..11.
    pub degrees: &'static [u8],
}

pub const SCALES: &[ScaleDef] = &[
    ScaleDef {
        name: "Off",
        degrees: &[],
    },
    ScaleDef {
        name: "Major",
        degrees: &[0, 2, 4, 5, 7, 9, 11],
    },
    ScaleDef {
        name: "Minor",
        degrees: &[0, 2, 3, 5, 7, 8, 10],
    },
    ScaleDef {
        name: "Dorian",
        degrees: &[0, 2, 3, 5, 7, 9, 10],
    },
    ScaleDef {
        name: "Mixolydian",
        degrees: &[0, 2, 4, 5, 7, 9, 10],
    },
    ScaleDef {
        name: "Lydian",
        degrees: &[0, 2, 4, 6, 7, 9, 11],
    },
    ScaleDef {
        name: "Phrygian",
        degrees: &[0, 1, 3, 5, 7, 8, 10],
    },
    ScaleDef {
        name: "Locrian",
        degrees: &[0, 1, 3, 5, 6, 8, 10],
    },
    ScaleDef {
        name: "Pent. Major",
        degrees: &[0, 2, 4, 7, 9],
    },
    ScaleDef {
        name: "Pent. Minor",
        degrees: &[0, 3, 5, 7, 10],
    },
    ScaleDef {
        name: "Blues",
        degrees: &[0, 3, 5, 6, 7, 10],
    },
    ScaleDef {
        name: "Whole Tone",
        degrees: &[0, 2, 4, 6, 8, 10],
    },
    ScaleDef {
        name: "Diminished",
        degrees: &[0, 2, 3, 5, 6, 8, 9, 11],
    },
];

/// Snap `transpose` (semitones, any float) to the nearest degree of `scale_idx`.
/// Returns the original value unchanged when scale_idx is 0 (Off) or out of range.
pub fn quantize_transpose(transpose: f32, scale_idx: usize) -> f32 {
    let Some(scale) = SCALES.get(scale_idx) else {
        return transpose;
    };
    if scale.degrees.is_empty() {
        return transpose;
    }

    let octave = (transpose / 12.0).floor();
    // Position within the octave, 0..12
    let degree_f = transpose - octave * 12.0;

    // Find the nearest scale degree (also check wrapping to next octave via 12)
    let mut best = scale.degrees[0] as f32;
    let mut best_dist = (degree_f - best).abs();

    for &d in scale.degrees.iter().skip(1) {
        let dist = (degree_f - d as f32).abs();
        if dist < best_dist {
            best_dist = dist;
            best = d as f32;
        }
    }
    // Also check wrapping: distance to first degree of next octave
    let wrap_dist = (degree_f - 12.0 - scale.degrees[0] as f32).abs();
    if wrap_dist < best_dist {
        best = 12.0 + scale.degrees[0] as f32;
    }

    octave * 12.0 + best
}
