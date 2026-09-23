//! Harmonic tiers for the `lane-harmony` process.
//!
//! A follower note is judged against the source's current chord and the key
//! implied by the source pattern, scoring each pitch class on a ladder:
//! chord tones at the top, then key tones (diatonic non-chord tones), then
//! color tones (chromatic but not clashing), then clashes (a semitone above a
//! chord tone, or the tritone against the root). The lane's `amount` is a
//! strictness threshold on that score: a note that clears it plays as
//! authored, one that does not snaps to the nearest pitch class that does.
//! Every note that sounds is therefore a real note; there is no fractional
//! transpose anywhere on the dial.

use crate::sequencer::SequencerStepSnapshot;

/// Pitch class (0..12) of a semitone offset, which may be negative.
/// Scale tables for `neural-scale` / `scale-pitch-classes`: name and
/// intervals from the root. Order is the enum order the process inlet shows.
pub const SCALES: [(&str, &[u8]); 14] = [
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major pentatonic", &[0, 2, 4, 7, 9]),
    ("minor pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("whole tone", &[0, 2, 4, 6, 8, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

pub fn pitch_class(semitones: f32) -> u8 {
    (semitones.round() as i32).rem_euclid(12) as u8
}

/// Bit `pc` of a 12-bit pitch-class set.
fn bit(pc: u8) -> u16 {
    1 << (pc % 12)
}

/// Union of every authored pitch class in a pattern's active steps: chord
/// notes where a step has them, else the step's transpose p-lock. This is the
/// key a chord track implies without any theory, and it widens honestly with
/// modal or borrowed chords.
pub fn pattern_pitch_class_mask(steps: &[SequencerStepSnapshot], num_steps: usize) -> u16 {
    let mut mask = 0;
    for step in steps.iter().take(num_steps) {
        if !step.active {
            continue;
        }
        if step.chord.is_empty() {
            mask |= bit(pitch_class(
                step.params[crate::sequencer::StepParam::Transpose.index()],
            ));
        } else {
            for note in &step.chord {
                mask |= bit(pitch_class(*note));
            }
        }
    }
    mask
}

/// Pitch classes set in a mask, ascending.
pub fn mask_pitch_classes(mask: u16) -> impl Iterator<Item = u8> {
    (0..12u8).filter(move |pc| mask & bit(*pc) != 0)
}

/// Scale implied by a chord's quality, as intervals from its root. `None`
/// for a chord whose quality is not recognisable (a bare note, a dyad, a
/// cluster).
fn quality_scale(chord: &[u8]) -> Option<u16> {
    let root = *chord.first()?;
    let has = |interval: u8| chord.iter().any(|pc| (pc + 12 - root) % 12 == interval);
    let major_third = has(4);
    let minor_third = has(3);
    let dim_fifth = has(6) && !has(7);
    let flat_seventh = has(10);
    let intervals: &[u8] = match (major_third, minor_third) {
        (true, _) if flat_seventh => &[0, 2, 4, 5, 7, 9, 10], // mixolydian
        (true, _) => &[0, 2, 4, 5, 7, 9, 11],                 // ionian
        (false, true) if dim_fifth => &[0, 1, 3, 5, 6, 8, 10], // locrian
        (false, true) if flat_seventh => &[0, 2, 3, 5, 7, 9, 10], // dorian
        (false, true) => &[0, 2, 3, 5, 7, 8, 10],              // aeolian
        (false, false) => return None,
    };
    Some(
        intervals
            .iter()
            .fold(0, |mask, interval| mask | bit((root + interval) % 12)),
    )
}

/// The key a follower is held to: the source pattern's own pitch set when it
/// is rich enough to be one (five or more pitch classes), else that set
/// widened by the scale the current chord's quality implies. The chord
/// itself is always in.
pub fn harmonic_key_mask(chord: &[u8], pattern_mask: u16) -> u16 {
    let chord_mask = chord.iter().fold(0, |mask, pc| mask | bit(*pc));
    let mut key = pattern_mask | chord_mask;
    if key.count_ones() < 5 {
        if let Some(scale) = quality_scale(chord) {
            key |= scale;
        }
    }
    key
}

/// Score a pitch class against a chord (pitch classes, root first) and key.
///
/// | tier        | score      |
/// |-------------|------------|
/// | chord tone  | 1.0        |
/// | key tone    | 0.60..0.75 |
/// | color tone  | 0.30..0.45 |
/// | clash       | 0.05..0.20 |
///
/// Within the key and color tiers the interval above the root nudges the
/// score: 9ths, 13ths and a bare fifth sit highest, a major 7th lowest.
pub fn harmonic_score(pc: u8, chord: &[u8], key_mask: u16) -> f64 {
    let pc = pc % 12;
    let Some(&root) = chord.first() else {
        return 1.0;
    };
    if chord.iter().any(|tone| tone % 12 == pc) {
        return 1.0;
    }
    let above = |tone: u8| (pc + 12 - tone % 12) % 12;
    if above(root) == 1 {
        return 0.05;
    }
    let third = chord
        .iter()
        .copied()
        .find(|tone| matches!((tone + 12 - root) % 12, 3 | 4));
    if third.is_some_and(|third| above(third) == 1) {
        return 0.10;
    }
    if chord.iter().any(|tone| above(*tone) == 1) {
        return 0.15;
    }
    if above(root) == 6 {
        return 0.20;
    }
    let base = if key_mask & bit(pc) != 0 { 0.60 } else { 0.30 };
    let bonus = match above(root) {
        2 => 0.15,  // 9th
        7 => 0.15,  // fifth left out of the chord
        9 => 0.12,  // 13th
        3 | 4 => 0.10, // the other third
        10 => 0.08, // flat 7th
        5 => 0.05,  // 11th (over a minor chord; over major it clashed above)
        11 => 0.02, // major 7th
        _ => 0.0,
    };
    base + bonus
}

/// Signed semitone delta that moves `current` onto the nearest pitch class
/// scoring at least `amount`, or `0.0` when it already does or when the move
/// is within `grace` semitones. Ties between an upward and a downward move
/// go down. `chord` is the source step's authored pitches, root first.
pub fn harmonic_snap(chord: &[f32], key_mask: u16, current: f64, amount: f64, grace: f64) -> f64 {
    if chord.is_empty() {
        return 0.0;
    }
    let chord = chord.iter().map(|note| pitch_class(*note)).collect::<Vec<_>>();
    let key_mask = harmonic_key_mask(&chord, key_mask);
    let threshold = amount.clamp(0.0, 1.0) - 1e-9;
    let current_pc = pitch_class(current as f32);
    if harmonic_score(current_pc, &chord, key_mask) >= threshold {
        return 0.0;
    }
    let mut best: Option<(f64, f64)> = None;
    for pc in 0..12u8 {
        let score = harmonic_score(pc, &chord, key_mask);
        if score < threshold {
            continue;
        }
        let delta = f64::from((pc as i32 - current_pc as i32 + 6).rem_euclid(12) - 6);
        let better = match best {
            None => true,
            Some((best_delta, best_score)) => {
                delta.abs() < best_delta.abs()
                    || (delta.abs() == best_delta.abs()
                        && (score > best_score || (score == best_score && delta < best_delta)))
            }
        };
        if better {
            best = Some((delta, score));
        }
    }
    let delta = best.map(|(delta, _)| delta).unwrap_or(0.0);
    if delta.abs() <= grace.max(0.0) {
        0.0
    } else {
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_MAJOR_KEY: u16 = 0b1010_1011_0101; // C D E F G A B

    #[test]
    fn scores_fall_into_tiers_over_a_major_triad() {
        let chord = [0, 4, 7];
        let score = |pc| harmonic_score(pc, &chord, C_MAJOR_KEY);
        assert_eq!(score(0), 1.0);
        assert_eq!(score(4), 1.0);
        assert!((0.6..=0.75).contains(&score(2)), "D is a key tone");
        assert!((0.6..=0.75).contains(&score(9)), "A is a key tone");
        assert!((0.6..=0.75).contains(&score(11)), "B is a key tone");
        assert!((0.3..=0.45).contains(&score(3)), "Eb is a color tone");
        assert!((0.3..=0.45).contains(&score(10)), "Bb is a color tone");
        assert_eq!(score(1), 0.05, "Db clashes with the root");
        assert_eq!(score(5), 0.10, "F is the avoid note above the third");
        assert_eq!(score(8), 0.15, "Ab clashes with the fifth");
        assert_eq!(score(6), 0.20, "F# is the tritone against the root");
    }

    #[test]
    fn the_eleventh_is_a_key_tone_over_a_minor_chord() {
        let chord = [9, 0, 4]; // A minor
        assert!(harmonic_score(2, &chord, C_MAJOR_KEY) >= 0.6);
    }

    #[test]
    fn a_thin_pattern_borrows_the_scale_the_chord_quality_implies() {
        assert_eq!(harmonic_key_mask(&[0, 4, 7], 0), C_MAJOR_KEY);
        let d_dorian = harmonic_key_mask(&[2, 5, 9, 0], 0);
        assert_eq!(d_dorian, C_MAJOR_KEY, "D minor 7 implies dorian, the C major set");
        assert_eq!(harmonic_key_mask(&[7], 0), bit(7), "a bare note implies nothing");
        let rich = 0b0101_0101_0101;
        assert_eq!(harmonic_key_mask(&[0, 4, 7], rich), rich | bit(0) | bit(4) | bit(7));
    }

    #[test]
    fn snap_threshold_tracks_amount() {
        let chord = [0.0, 4.0, 7.0];
        // Eb: color tone. Passes at 0.3; at 0.5 D and E are both one away
        // and the chord tone wins the tie, so it goes up to E, as at 1.
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 3.0, 0.3, 0.0), 0.0);
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 3.0, 0.5, 0.0), 1.0);
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 3.0, 1.0, 0.0), 1.0);
        // D: key tone. Free at 0.5, pulled to C (downward tie) at 1.
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 2.0, 0.5, 0.0), 0.0);
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 2.0, 1.0, 0.0), -2.0);
        // Db clashes even at a low amount; amount 0 lets everything through.
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 1.0, 0.1, 0.0), -1.0);
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 1.0, 0.0, 0.0), 0.0);
        // Grace keeps a note within reach where it is.
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, 1.0, 1.0, 1.0), 0.0);
        // Octaves and negative offsets are pitch classes.
        assert_eq!(harmonic_snap(&chord, C_MAJOR_KEY, -9.0, 1.0, 0.0), 1.0);
    }

    #[test]
    fn pattern_mask_unions_chords_and_plain_notes_of_active_steps() {
        use crate::sequencer::{StepParam, NUM_PARAMS};
        let mut params = [0.0; NUM_PARAMS];
        params[StepParam::Transpose.index()] = 14.0;
        let step = |active: bool, chord: Vec<f32>| SequencerStepSnapshot {
            active,
            neural_reset: false,
            params,
            chord,
            chord_durations: Vec::new(),
            chord_delays: Vec::new(),
            timebase_override: None,
            swing_override: None,
            swing_resolution_override: None,
            track_send_plocks: Vec::new(),
        };
        let steps = vec![
            step(true, vec![0.0, 4.0, 7.0]),
            step(true, Vec::new()),
            step(false, vec![1.0]),
            step(true, vec![5.0, 9.0]),
        ];
        assert_eq!(
            pattern_pitch_class_mask(&steps, 4),
            bit(0) | bit(4) | bit(7) | bit(2) | bit(5) | bit(9)
        );
        assert_eq!(pattern_pitch_class_mask(&steps, 1), bit(0) | bit(4) | bit(7));
    }
}
