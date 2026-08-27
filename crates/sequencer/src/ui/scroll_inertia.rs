//! App-side inertial (momentum) scrolling.
//!
//! macOS applies scroll momentum in AppKit and folds it into the ordinary
//! delta stream. Wayland compositors deliberately do not: the doctrine is
//! that applications implement momentum themselves, so on Linux every scroll
//! stops dead the instant the fingers lift. This module supplies that
//! momentum. The event loop feeds it every real touchpad delta, tells it when
//! the compositor reports the fingers lifting (winit `TouchPhase::Ended`),
//! and then drains one synthetic delta per frame from a decaying velocity —
//! routed through the exact same scroll path as real input.
//!
//! Off by default; opted into from init.lisp via
//! `(host-command "set-scroll-inertia" true)`. Mouse wheels never report a
//! gesture end, so they can never start a fling.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Deltas older than this do not contribute to the release-velocity estimate.
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);
/// If the newest delta is older than this at release, the fingers rested
/// before lifting; that is a deliberate stop, not a flick.
const MAX_RELEASE_GAP: Duration = Duration::from_millis(80);
/// Minimum release speed (px/s) that starts a fling.
const MIN_FLING_SPEED: f32 = 120.0;
/// Speed (px/s) below which an active fling stops. Kept high enough that the
/// fling ends crisply instead of creeping for a second at a pixel per frame;
/// macOS momentum visibly snaps to a stop rather than asymptoting.
const STOP_SPEED: f32 = 80.0;
/// Per-millisecond velocity retention (half-life ≈ 240ms). Chromium's 0.998
/// (half-life ≈ 350ms) felt ~30% too floaty next to macOS momentum.
const DECAY_PER_MS: f32 = 0.9971;
/// A frame gap longer than this (stalled loop) would integrate into one huge
/// jump; clamp it instead.
const MAX_TICK_DT: Duration = Duration::from_millis(100);

struct Fling {
    velocity: (f32, f32),
    /// Cursor position the gesture ended at; synthetic deltas keep targeting
    /// the pane under it, like macOS momentum does.
    pos: (f32, f32),
    last_tick: Instant,
}

#[derive(Default)]
pub(crate) struct ScrollInertia {
    enabled: bool,
    samples: VecDeque<(Instant, f32, f32)>,
    last_pos: (f32, f32),
    fling: Option<Fling>,
}

impl ScrollInertia {
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.cancel();
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn fling_active(&self) -> bool {
        self.fling.is_some()
    }

    /// Any input that should interrupt momentum: fingers touching back down,
    /// a click, a key press, a magnify gesture.
    pub(crate) fn cancel(&mut self) {
        self.samples.clear();
        self.fling = None;
    }

    /// Record one real touchpad delta (pixels) at `pos`.
    pub(crate) fn note_scroll(&mut self, now: Instant, delta: (f32, f32), pos: (f32, f32)) {
        if !self.enabled {
            return;
        }
        // Fingers are on the pad again; any leftover fling yields to them.
        self.fling = None;
        self.samples.push_back((now, delta.0, delta.1));
        while let Some(&(t, ..)) = self.samples.front() {
            if now.duration_since(t) > VELOCITY_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.last_pos = pos;
    }

    /// The compositor reported the fingers lifting. Start a fling if the
    /// gesture ended with enough speed.
    pub(crate) fn note_phase_ended(&mut self, now: Instant) {
        let samples = std::mem::take(&mut self.samples);
        if !self.enabled {
            return;
        }
        let Some(&(oldest, ..)) = samples.front() else {
            return;
        };
        let Some(&(newest, ..)) = samples.back() else {
            return;
        };
        if now.duration_since(newest) > MAX_RELEASE_GAP {
            return;
        }
        // Deltas are distances travelled *since the previous event*, so the
        // window they cover starts one inter-event gap before `oldest`; using
        // `oldest` itself would overestimate speed for two-sample gestures.
        // `now - oldest` includes the release gap and evens that out.
        let span = now.duration_since(oldest).max(Duration::from_millis(8));
        let (sum_x, sum_y) = samples
            .iter()
            .fold((0.0f32, 0.0f32), |(x, y), &(_, dx, dy)| (x + dx, y + dy));
        let velocity = (
            sum_x / span.as_secs_f32(),
            sum_y / span.as_secs_f32(),
        );
        if (velocity.0.powi(2) + velocity.1.powi(2)).sqrt() < MIN_FLING_SPEED {
            return;
        }
        self.fling = Some(Fling {
            velocity,
            pos: self.last_pos,
            last_tick: now,
        });
    }

    /// One frame of momentum: the synthetic pixel delta to scroll by and the
    /// cursor position to apply it at, or `None` when no fling is active.
    pub(crate) fn tick(&mut self, now: Instant) -> Option<((f32, f32), (f32, f32))> {
        let fling = self.fling.as_mut()?;
        let dt = now.duration_since(fling.last_tick).min(MAX_TICK_DT);
        fling.last_tick = now;
        let dt_secs = dt.as_secs_f32();
        let delta = (fling.velocity.0 * dt_secs, fling.velocity.1 * dt_secs);
        let retain = DECAY_PER_MS.powf(dt_secs * 1000.0);
        fling.velocity.0 *= retain;
        fling.velocity.1 *= retain;
        let pos = fling.pos;
        let speed = (fling.velocity.0.powi(2) + fling.velocity.1.powi(2)).sqrt();
        if speed < STOP_SPEED {
            self.fling = None;
        }
        Some((delta, pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A fast downward two-finger flick: steady 10px-per-8ms deltas.
    fn flick(inertia: &mut ScrollInertia, start: Instant) -> Instant {
        let mut t = start;
        for _ in 0..8 {
            inertia.note_scroll(t, (0.0, 10.0), (40.0, 12.0));
            t += ms(8);
        }
        t
    }

    #[test]
    fn disabled_by_default_and_ignores_input() {
        let mut inertia = ScrollInertia::default();
        let t0 = Instant::now();
        let t = flick(&mut inertia, t0);
        inertia.note_phase_ended(t);
        assert!(!inertia.fling_active());
        assert_eq!(inertia.tick(t), None);
    }

    #[test]
    fn flick_release_starts_a_decaying_fling() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let t0 = Instant::now();
        let t = flick(&mut inertia, t0);
        inertia.note_phase_ended(t);
        assert!(inertia.fling_active());

        let ((_, dy1), pos) = inertia.tick(t + ms(16)).unwrap();
        assert!(dy1 > 0.0, "fling continues in the gesture direction");
        assert_eq!(pos, (40.0, 12.0), "fling targets the release position");
        let ((_, dy2), _) = inertia.tick(t + ms(32)).unwrap();
        assert!(dy2 < dy1, "velocity decays between ticks");

        // Momentum runs out on its own.
        let mut t_end = t + ms(32);
        for _ in 0..2000 {
            if !inertia.fling_active() {
                break;
            }
            t_end += ms(16);
            inertia.tick(t_end);
        }
        assert!(!inertia.fling_active(), "fling stops below the speed floor");
    }

    #[test]
    fn slow_drag_release_does_not_fling() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let mut t = Instant::now();
        for _ in 0..8 {
            inertia.note_scroll(t, (0.0, 0.4), (0.0, 0.0));
            t += ms(8);
        }
        inertia.note_phase_ended(t);
        assert!(!inertia.fling_active());
    }

    #[test]
    fn resting_fingers_before_lifting_does_not_fling() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let t = flick(&mut inertia, Instant::now());
        // Fingers stay planted for 200ms, then lift.
        inertia.note_phase_ended(t + ms(200));
        assert!(!inertia.fling_active());
    }

    #[test]
    fn new_touch_and_cancel_interrupt_momentum() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let t = flick(&mut inertia, Instant::now());
        inertia.note_phase_ended(t);
        assert!(inertia.fling_active());
        inertia.note_scroll(t + ms(30), (0.0, 1.0), (0.0, 0.0));
        assert!(!inertia.fling_active(), "touching down again stops the fling");

        let t2 = flick(&mut inertia, t + ms(500));
        inertia.note_phase_ended(t2);
        assert!(inertia.fling_active());
        inertia.cancel();
        assert!(!inertia.fling_active());
    }

    #[test]
    fn disabling_mid_fling_stops_it() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let t = flick(&mut inertia, Instant::now());
        inertia.note_phase_ended(t);
        assert!(inertia.fling_active());
        inertia.set_enabled(false);
        assert!(!inertia.fling_active());
        assert_eq!(inertia.tick(t + ms(16)), None);
    }

    #[test]
    fn stalled_frame_gap_is_clamped() {
        let mut inertia = ScrollInertia::default();
        inertia.set_enabled(true);
        let t = flick(&mut inertia, Instant::now());
        inertia.note_phase_ended(t);
        let ((_, dy_stalled), _) = inertia.tick(t + Duration::from_secs(2)).unwrap();
        // A 2s stall integrates as at most MAX_TICK_DT, not 2s of travel.
        let mut fresh = ScrollInertia::default();
        fresh.set_enabled(true);
        let tf = flick(&mut fresh, Instant::now());
        fresh.note_phase_ended(tf);
        let ((_, dy_normal), _) = fresh.tick(tf + MAX_TICK_DT).unwrap();
        assert!((dy_stalled - dy_normal).abs() < 1e-3);
    }
}
