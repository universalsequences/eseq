//! Development-only entry scopes. The native runtime checks calls from Rust,
//! C and dynamically loaded instruments; see the calibrated coverage limits
//! in tools/audio-experiments/README.md before interpreting a clean run.
unsafe extern "C" {
    fn audiograph_rtsan_set_enabled(enabled: i32);
    fn audiograph_rtsan_enabled() -> i32;
}

pub(super) fn set_enabled(enabled: bool) {
    unsafe { audiograph_rtsan_set_enabled(i32::from(enabled)); }
}

pub(super) fn scope() -> Option<rtsan_standalone::ScopedSanitizeRealtime> {
    (unsafe { audiograph_rtsan_enabled() } != 0)
        .then(rtsan_standalone::ScopedSanitizeRealtime::default)
}
