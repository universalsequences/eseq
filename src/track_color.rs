use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl TrackColor {
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn clamped(self) -> Self {
        Self {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
        }
    }

    pub fn palette_color(index: usize) -> Self {
        TRACK_COLOR_PALETTE[index % TRACK_COLOR_PALETTE.len()]
    }

    pub fn next_for_existing(existing: &[TrackColor]) -> Self {
        for color in TRACK_COLOR_PALETTE {
            if !existing.contains(&color) {
                return color;
            }
        }
        Self::palette_color(existing.len())
    }
}

impl Default for TrackColor {
    fn default() -> Self {
        Self::palette_color(0)
    }
}

pub const TRACK_COLOR_PALETTE: [TrackColor; 12] = [
    TrackColor::new(0.96, 0.28, 0.52),
    TrackColor::new(0.98, 0.56, 0.20),
    TrackColor::new(0.95, 0.78, 0.18),
    TrackColor::new(0.22, 0.78, 0.36),
    TrackColor::new(0.10, 0.74, 0.68),
    TrackColor::new(0.12, 0.64, 0.96),
    TrackColor::new(0.34, 0.48, 0.98),
    TrackColor::new(0.62, 0.38, 0.98),
    TrackColor::new(0.86, 0.30, 0.92),
    TrackColor::new(0.90, 0.18, 0.24),
    TrackColor::new(0.38, 0.86, 0.82),
    TrackColor::new(0.74, 0.86, 0.24),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_colors_are_saturated_and_clamped() {
        for color in TRACK_COLOR_PALETTE {
            assert!((0.0..=1.0).contains(&color.r));
            assert!((0.0..=1.0).contains(&color.g));
            assert!((0.0..=1.0).contains(&color.b));
            let max = color.r.max(color.g).max(color.b);
            let min = color.r.min(color.g).min(color.b);
            assert!(max >= 0.70, "palette color is not bright enough: {color:?}");
            assert!(
                max - min >= 0.25,
                "palette color is not saturated enough: {color:?}"
            );
        }
    }

    #[test]
    fn next_for_existing_prefers_unused_palette_colors_then_cycles() {
        assert_eq!(TrackColor::next_for_existing(&[]), TRACK_COLOR_PALETTE[0]);
        assert_eq!(
            TrackColor::next_for_existing(&TRACK_COLOR_PALETTE[..2]),
            TRACK_COLOR_PALETTE[2]
        );
        assert_eq!(
            TrackColor::next_for_existing(&TRACK_COLOR_PALETTE),
            TRACK_COLOR_PALETTE[0]
        );
    }
}
