use std::time::Duration;

pub(crate) fn ui_entrypoint_path() -> std::path::PathBuf {
    sequencer::app_paths::app_paths().ui_dir().join("main.lisp")
}
/// Bare root for `metal_seq noui` (eseq-750i).
pub(crate) fn noui_entrypoint_path() -> std::path::PathBuf {
    sequencer::app_paths::app_paths().ui_dir().join("noui.lisp")
}
pub(crate) const PAGE_SIZE: usize = 16;
pub(crate) const AUTO_FOLLOW_COOLDOWN: Duration = Duration::from_secs(5);
pub(crate) const METER_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const LIVE_AUDIO_ANALYZER_POLL_INTERVAL: Duration = Duration::from_millis(33);
pub(crate) const NEURAL_VISUALIZATION_POLL_INTERVAL: Duration = Duration::from_millis(100);
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
/// Scale dropdown labels, in persisted `fts_scale` index order.
pub(crate) fn fts_scale_names() -> impl Iterator<Item = &'static str> {
    sequencer::scale::SCALES.iter().map(|scale| scale.name)
}

/// The `fts_scale` index a dropdown label names (case-insensitive).
pub(crate) fn fts_scale_index(label: &str) -> Option<usize> {
    fts_scale_names().position(|name| name.eq_ignore_ascii_case(label))
}
