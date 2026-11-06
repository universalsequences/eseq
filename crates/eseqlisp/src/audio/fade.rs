//! Audio-region fade shapes and fade handles, shared by the timeline that
//! draws and drags them (`widget_render::timeline::fades`) and the region
//! player that applies them (the sequencer's `audio_track`): one formula, so
//! what is drawn is what is heard.

/// Exponent a fade curve of `curve` (-1..1) raises its 0..1 ramp to: 1 is
/// linear, an eighth at +1 (a fast rise, a late drop) and 8 at -1.
pub fn curve_power(curve: f32) -> f32 {
    2f32.powf(-3.0 * curve.clamp(-1.0, 1.0))
}

/// The level `t` (0..1) of the way through a fade-in shaped by `power`. A
/// fade-out reads it backwards (`t` = the fraction of the fade still to go).
pub fn fade_level(t: f64, power: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    if power == 1.0 { t } else { t.powf(power) }
}

/// The curve that puts `level` (0..1) at a fade's midpoint: what dragging a
/// fade's curve handle to that height sets.
pub fn curve_for_midpoint(level: f64) -> f32 {
    let level = level.clamp(0.004, 0.996);
    let power = level.ln() / 0.5f64.ln();
    (-(power.log2()) / 3.0).clamp(-1.0, 1.0) as f32
}

/// Gains `t` (0..1) through a crossfade, (outgoing, incoming): an
/// equal-gain S-curve, so a cut through one recording stays at full level.
pub fn crossfade_gains(t: f64) -> (f32, f32) {
    let incoming = (std::f64::consts::FRAC_PI_2 * t.clamp(0.0, 1.0)).sin().powi(2) as f32;
    (1.0 - incoming, incoming)
}

/// A fade handle on a timeline audio region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeHandle {
    /// The fade-in's end. Dragged left past the start of a region that
    /// abuts the one before, it becomes a crossfade into it.
    In,
    /// The fade-out's start. Dragged right past the end of a region the
    /// next one abuts, it becomes a crossfade into that one.
    Out,
    /// The midpoint of the fade-in's / fade-out's curve (a 0..1 level).
    InCurve,
    OutCurve,
    /// A crossfade's left end (on the earlier region) and right end (on the
    /// later one).
    CrossfadeStart,
    CrossfadeEnd,
}

impl FadeHandle {
    const ALL: [(Self, &'static str); 6] = [
        (Self::In, "in"),
        (Self::Out, "out"),
        (Self::InCurve, "in-curve"),
        (Self::OutCurve, "out-curve"),
        (Self::CrossfadeStart, "xfade-start"),
        (Self::CrossfadeEnd, "xfade-end"),
    ];

    /// The handle's keyword name, without the colon.
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(handle, _)| *handle == self).map_or("in", |(_, name)| name)
    }

    /// The handle named `name` (`:in` or `in`).
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim_start_matches(':');
        Self::ALL.iter().find(|(_, candidate)| *candidate == name).map(|(handle, _)| *handle)
    }

    pub fn is_curve(self) -> bool {
        matches!(self, Self::InCurve | Self::OutCurve)
    }

    /// Whether the handle marks a fade's end that only moves left of the
    /// item's end (the fade-in side).
    pub fn is_in_side(self) -> bool {
        matches!(self, Self::In | Self::CrossfadeEnd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(curve_power(0.0), 1.0);
        assert!((curve_power(1.0) - 0.125).abs() < 1e-6);
        assert!((curve_power(-1.0) - 8.0).abs() < 1e-5);
        assert!((fade_level(0.5, 2.0) - 0.25).abs() < 1e-6);
        assert!((crossfade_gains(0.5).1 - 0.5).abs() < 1e-6);
        let curve = curve_for_midpoint(0.75);
        assert!((fade_level(0.5, curve_power(curve)) - 0.75).abs() < 1e-3);
    }

    #[test]
    fn handle_names_round_trip() {
        for (handle, name) in FadeHandle::ALL {
            assert_eq!(FadeHandle::parse(name), Some(handle));
            assert_eq!(FadeHandle::parse(&format!(":{name}")), Some(handle));
            assert_eq!(handle.name(), name);
        }
        assert_eq!(FadeHandle::parse("sideways"), None);
    }
}
