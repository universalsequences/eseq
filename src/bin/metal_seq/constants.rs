use std::time::Duration;

pub(crate) const PAGE_SIZE: usize = 16;
pub(crate) const AUTO_FOLLOW_COOLDOWN: Duration = Duration::from_secs(5);
pub(crate) const METER_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const CPU_UI_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub(crate) const VOICE_COUNT_LOG_INTERVAL: Duration = Duration::from_secs(2);
pub(crate) const METER_LEVEL_STEPS: f64 = 48.0;
pub(crate) const BUILTIN_ACCUMULATOR_NAMES: &[&str] = &[
    "Off",
    "TransposeRamp",
    "VelocityDecay",
    "OctaveEcho",
    "SendToTrack",
];
pub(crate) const ACCUM_MODE_LABELS: &[&str] = &["rtz", "clip", "rvtz", "rvbp"];
pub(crate) const FTS_SCALE_NAMES: &[&str] = &[
    "Off",
    "Major",
    "Minor",
    "Dorian",
    "Mixolydian",
    "Lydian",
    "Phrygian",
    "Locrian",
    "Pent. Major",
    "Pent. Minor",
    "Blues",
    "Whole Tone",
    "Diminished",
];
