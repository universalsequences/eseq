//! Per-frame profiling for the portable wgpu app shell.
//!
//! `MetalBackend` has its own `[ui-profile][metal]` aggregate, but it reports
//! the Metal path's caches and upload arena, none of which the wgpu shell has.
//! The wgpu shell has a different cost structure and needs its own attribution:
//! it rebuilds every widget primitive each frame (no retained scene cache) and
//! creates a fresh vertex buffer per draw command, then acquires the swapchain
//! image *after* planning, so `Fifo` backpressure lands in one measurable place.
//!
//! Scroll is the interaction this exists to explain. A scroll gesture makes
//! every frame a full-cost redraw, so "scrolling feels laggy" is ambiguous
//! between three very different causes: CPU spent rebuilding the scene, CPU
//! spent creating GPU buffers, and time blocked waiting for a swapchain image.
//! The aggregate separates them, and reports the tail (p95/max) as well as the
//! mean because a gesture is judged by its worst frames.
//!
//! Enable with `ESEQLISP_PROFILE_UI=1`, the same switch the Metal backend uses.

use std::time::{Duration, Instant};

/// Environment switch shared with `MetalBackend`'s aggregate.
pub const PROFILE_ENV: &str = "ESEQLISP_PROFILE_UI";

/// How often the aggregate is emitted.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Cap on retained per-frame samples, so a long report interval on a fast
/// machine cannot grow the window without bound. One second at 240 Hz fits.
const MAX_SAMPLES: usize = 512;

/// Timings and counts for one frame, accumulated by `render_tiled` as it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Building widget primitives: `collect_gpu_primitives`, offsetting, and
    /// segment splitting, summed over every visible tile.
    pub widget_scene: Duration,
    /// The whole plan phase, including `widget_scene` and text shaping.
    pub plan: Duration,
    /// Blocked in `get_current_texture`. Under `Fifo` this is swapchain
    /// backpressure, not work, and it is the difference between "the frame is
    /// expensive" and "the frame is waiting its turn".
    pub acquire: Duration,
    /// Recording the render pass and submitting it.
    pub encode: Duration,
    /// Widget primitives emitted across every tile.
    pub widget_primitives: u64,
    /// Draw commands in the replayed plan.
    pub draw_commands: u64,
    /// Vertex/instance buffers created this frame. The wgpu shell allocates one
    /// per draw command, so this tracks `draw_commands` until that changes.
    pub buffers_created: u64,
    /// Bytes uploaded through those buffers.
    pub buffer_bytes: usize,
    /// Whether a scroll offset changed since the previous frame. Scroll frames
    /// are reported separately because they are the interaction under test.
    pub scrolled: bool,
}

impl FrameSample {
    /// Total CPU wall time the shell spent producing this frame, including the
    /// swapchain wait.
    pub fn total(&self) -> Duration {
        self.plan + self.acquire + self.encode
    }

    /// CPU wall time excluding the swapchain wait: the part that is actually
    /// work this process could make cheaper.
    pub fn cpu(&self) -> Duration {
        self.plan + self.encode
    }
}

/// One report window's worth of frames.
#[derive(Debug, Default)]
pub struct FrameStatsWindow {
    samples: Vec<FrameSample>,
    /// Frames observed, which can exceed `samples.len()` once the cap is hit.
    frames: u64,
    scroll_frames: u64,
    dropped_samples: u64,
}

impl FrameStatsWindow {
    pub fn push(&mut self, sample: FrameSample) {
        self.frames += 1;
        if sample.scrolled {
            self.scroll_frames += 1;
        }
        if self.samples.len() < MAX_SAMPLES {
            self.samples.push(sample);
        } else {
            self.dropped_samples += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    fn mean(&self, of: impl Fn(&FrameSample) -> Duration) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let total: f64 = self.samples.iter().map(|s| of(s).as_secs_f64()).sum();
        total * 1000.0 / self.samples.len() as f64
    }

    fn mean_count(&self, of: impl Fn(&FrameSample) -> u64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let total: u64 = self.samples.iter().map(&of).sum();
        total as f64 / self.samples.len() as f64
    }

    /// Nearest-rank percentile in milliseconds. `percentile` is 0..=100.
    fn percentile_ms(&self, percentile: usize, of: impl Fn(&FrameSample) -> Duration) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut values: Vec<f64> = self
            .samples
            .iter()
            .map(|s| of(s).as_secs_f64() * 1000.0)
            .collect();
        values.sort_by(f64::total_cmp);
        let index = (values.len() * percentile / 100).min(values.len() - 1);
        values[index]
    }

    /// Same aggregate restricted to frames where a scroll offset changed.
    fn scroll_only(&self) -> FrameStatsWindow {
        FrameStatsWindow {
            samples: self
                .samples
                .iter()
                .copied()
                .filter(|s| s.scrolled)
                .collect(),
            frames: self.scroll_frames,
            scroll_frames: self.scroll_frames,
            dropped_samples: 0,
        }
    }

    /// Render the aggregate for a window that lasted `elapsed`.
    ///
    /// Kept separate from emission and free of clocks so the exact reported
    /// numbers are testable.
    pub fn format(&self, elapsed: Duration) -> String {
        let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
        let fps = self.frames as f64 / seconds;
        let mut line = format!(
            "[ui-profile][wgpu] fps={fps:.1} frames={frames} \
             frame_avg={frame_avg:.2}ms frame_p95={frame_p95:.2}ms frame_max={frame_max:.2}ms \
             cpu_avg={cpu_avg:.2}ms cpu_p95={cpu_p95:.2}ms \
             plan_avg={plan_avg:.2}ms scene_avg={scene_avg:.2}ms \
             acquire_avg={acquire_avg:.2}ms acquire_p95={acquire_p95:.2}ms \
             encode_avg={encode_avg:.2}ms \
             prims/frame={prims:.0} draws/frame={draws:.0} \
             buffers/frame={buffers:.0} buffer_kb/frame={buffer_kb:.1}",
            frames = self.frames,
            frame_avg = self.mean(FrameSample::total),
            frame_p95 = self.percentile_ms(95, FrameSample::total),
            frame_max = self.percentile_ms(100, FrameSample::total),
            cpu_avg = self.mean(FrameSample::cpu),
            cpu_p95 = self.percentile_ms(95, FrameSample::cpu),
            plan_avg = self.mean(|s| s.plan),
            scene_avg = self.mean(|s| s.widget_scene),
            acquire_avg = self.mean(|s| s.acquire),
            acquire_p95 = self.percentile_ms(95, |s| s.acquire),
            encode_avg = self.mean(|s| s.encode),
            prims = self.mean_count(|s| s.widget_primitives),
            draws = self.mean_count(|s| s.draw_commands),
            buffers = self.mean_count(|s| s.buffers_created),
            buffer_kb = self.mean_count(|s| s.buffer_bytes as u64) / 1024.0,
        );
        if self.scroll_frames > 0 {
            let scroll = self.scroll_only();
            line.push_str(&format!(
                " | scroll frames={frames} cpu_avg={cpu_avg:.2}ms cpu_p95={cpu_p95:.2}ms \
                 scene_avg={scene_avg:.2}ms acquire_avg={acquire_avg:.2}ms",
                frames = scroll.frames,
                cpu_avg = scroll.mean(FrameSample::cpu),
                cpu_p95 = scroll.percentile_ms(95, FrameSample::cpu),
                scene_avg = scroll.mean(|s| s.widget_scene),
                acquire_avg = scroll.mean(|s| s.acquire),
            ));
        }
        if self.dropped_samples > 0 {
            line.push_str(&format!(
                " (percentiles over the first {} of {} frames)",
                self.samples.len(),
                self.frames
            ));
        }
        line
    }
}

/// Collects [`FrameSample`]s and emits one aggregate per [`REPORT_INTERVAL`].
#[derive(Debug)]
pub struct WgpuFrameStats {
    enabled: bool,
    window: FrameStatsWindow,
    window_start: Instant,
    last_scroll_offsets: Option<(f32, f32)>,
}

impl WgpuFrameStats {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var_os(PROFILE_ENV).is_some(),
            window: FrameStatsWindow::default(),
            window_start: Instant::now(),
            last_scroll_offsets: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Classify this frame as a scroll frame by comparing the summed scroll
    /// offsets of every visible tile against the previous frame's.
    ///
    /// Summing is enough: any tile scrolling by any amount moves the sum, and
    /// the classification only has to separate "the user is scrolling" from
    /// "the user is not".
    pub fn note_scroll_offsets(&mut self, offsets: (f32, f32)) -> bool {
        let scrolled = match self.last_scroll_offsets {
            Some(previous) => previous != offsets,
            None => false,
        };
        self.last_scroll_offsets = Some(offsets);
        scrolled
    }

    /// Record a completed frame and emit the aggregate when the window closes.
    pub fn end_frame(&mut self, sample: FrameSample) {
        if !self.enabled {
            return;
        }
        self.window.push(sample);
        let elapsed = self.window_start.elapsed();
        if elapsed < REPORT_INTERVAL || self.window.is_empty() {
            return;
        }
        eprintln!("{}", self.window.format(elapsed));
        self.window = FrameStatsWindow::default();
        self.window_start = Instant::now();
    }
}

impl Default for WgpuFrameStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_micros(value * 1000)
    }

    fn sample(plan: u64, acquire: u64, encode: u64, scrolled: bool) -> FrameSample {
        FrameSample {
            widget_scene: ms(plan / 2),
            plan: ms(plan),
            acquire: ms(acquire),
            encode: ms(encode),
            widget_primitives: 100,
            draw_commands: 10,
            buffers_created: 10,
            buffer_bytes: 2048,
            scrolled,
        }
    }

    #[test]
    fn total_includes_the_swapchain_wait_and_cpu_excludes_it() {
        let sample = sample(10, 6, 2, false);
        assert_eq!(sample.total(), ms(18));
        assert_eq!(sample.cpu(), ms(12));
    }

    #[test]
    fn the_aggregate_reports_frames_and_rate_over_the_window() {
        let mut window = FrameStatsWindow::default();
        for _ in 0..30 {
            window.push(sample(4, 2, 1, false));
        }
        let line = window.format(Duration::from_secs(1));
        assert!(line.contains("fps=30.0"), "{line}");
        assert!(line.contains("frames=30"), "{line}");
        assert!(line.contains("frame_avg=7.00ms"), "{line}");
        assert!(line.contains("cpu_avg=5.00ms"), "{line}");
        assert!(line.contains("scene_avg=2.00ms"), "{line}");
    }

    /// The tail is the point: an aggregate that only reported the mean would
    /// hide exactly the frames a gesture is judged by.
    #[test]
    fn percentiles_track_the_slow_frames_not_the_mean() {
        let mut window = FrameStatsWindow::default();
        for _ in 0..19 {
            window.push(sample(1, 0, 0, false));
        }
        window.push(sample(100, 0, 0, false));
        let line = window.format(Duration::from_secs(1));
        assert!(line.contains("frame_max=100.00ms"), "{line}");
        assert!(line.contains("frame_p95=100.00ms"), "{line}");
        assert!(line.contains("frame_avg=5.95ms"), "{line}");
    }

    #[test]
    fn scroll_frames_are_reported_separately_from_idle_frames() {
        let mut window = FrameStatsWindow::default();
        for _ in 0..10 {
            window.push(sample(2, 0, 0, false));
        }
        for _ in 0..10 {
            window.push(sample(20, 0, 0, true));
        }
        let line = window.format(Duration::from_secs(1));
        assert!(line.contains("| scroll frames=10"), "{line}");
        assert!(line.contains("scroll frames=10 cpu_avg=20.00ms"), "{line}");
        // The all-frames mean averages both populations, so the scroll section
        // is the only place the scroll cost is visible.
        assert!(line.contains("cpu_avg=11.00ms"), "{line}");
    }

    #[test]
    fn a_window_without_scroll_frames_omits_the_scroll_section() {
        let mut window = FrameStatsWindow::default();
        window.push(sample(2, 0, 0, false));
        assert!(!window.format(Duration::from_secs(1)).contains("| scroll"));
    }

    #[test]
    fn sample_retention_is_capped_and_the_cap_is_disclosed() {
        let mut window = FrameStatsWindow::default();
        for _ in 0..(MAX_SAMPLES + 10) {
            window.push(sample(1, 0, 0, false));
        }
        let line = window.format(Duration::from_secs(1));
        assert!(
            line.contains(&format!("frames={}", MAX_SAMPLES + 10)),
            "{line}"
        );
        assert!(
            line.contains(&format!(
                "(percentiles over the first {MAX_SAMPLES} of {} frames)",
                MAX_SAMPLES + 10
            )),
            "{line}"
        );
    }

    /// The first frame has nothing to compare against, so it must not be
    /// counted as a scroll frame; every later change must be.
    #[test]
    fn scroll_classification_needs_a_previous_frame_to_compare_against() {
        let mut stats = WgpuFrameStats::new();
        assert!(!stats.note_scroll_offsets((0.0, 0.0)));
        assert!(!stats.note_scroll_offsets((0.0, 0.0)));
        assert!(stats.note_scroll_offsets((0.0, 12.0)));
        assert!(stats.note_scroll_offsets((3.0, 12.0)));
        assert!(!stats.note_scroll_offsets((3.0, 12.0)));
    }
}
