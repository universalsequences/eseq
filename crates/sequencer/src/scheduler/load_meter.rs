//! Scheduler-thread load meter.
//!
//! The scheduler thread has a fixed budget: it must refill the lookahead
//! (`4 × block` frames past the audio callback's position) before the callback
//! reaches the end of what was scheduled, or events arrive late. This meter
//! times every pass of the live loop and every Lisp generator tick, and
//! publishes a windowed summary through [`scheduler_load`] for a UI meter.
//!
//! `ESEQ_SCHED_PROFILE=1` also logs one summary line per second to stderr,
//! plus every pass slower than 5 ms (`ESEQ_SCHED_PROFILE=<ms>` sets that
//! threshold). Everything here runs on the scheduler thread only; the
//! per-pass bookkeeping is a few clock reads and no allocation.
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often the published summary (and the log line) rolls over.
const WINDOW: Duration = Duration::from_millis(1000);

/// The last completed window, readable from any thread.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SchedulerLoad {
    /// Fraction of wall time the scheduler spent working (not sleeping).
    pub busy: f32,
    /// Slowest single pass, in milliseconds.
    pub max_pass_ms: f32,
    /// Smallest lookahead left when a pass started, in milliseconds. At or
    /// below zero the audio callback had already caught up: late events.
    pub min_slack_ms: f32,
    /// Slowest single Lisp generator tick, in milliseconds.
    pub max_tick_ms: f32,
}

struct Published {
    busy_permille: AtomicU32,
    max_pass_us: AtomicU32,
    min_slack_us: AtomicU64,
    max_tick_us: AtomicU32,
}

static PUBLISHED: Published = Published {
    busy_permille: AtomicU32::new(0),
    max_pass_us: AtomicU32::new(0),
    min_slack_us: AtomicU64::new(i64::MAX as u64),
    max_tick_us: AtomicU32::new(0),
};

/// The most recent window's scheduler load (zeros until the first window).
pub fn scheduler_load() -> SchedulerLoad {
    let slack = PUBLISHED.min_slack_us.load(Ordering::Relaxed) as i64;
    SchedulerLoad {
        busy: PUBLISHED.busy_permille.load(Ordering::Relaxed) as f32 / 1000.0,
        max_pass_ms: PUBLISHED.max_pass_us.load(Ordering::Relaxed) as f32 / 1000.0,
        min_slack_ms: if slack == i64::MAX { 0.0 } else { slack as f32 / 1000.0 },
        max_tick_ms: PUBLISHED.max_tick_us.load(Ordering::Relaxed) as f32 / 1000.0,
    }
}

fn thread_cpu_time() -> Duration {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

#[derive(Default)]
struct TickStats {
    /// (generator id, ticks, total, max) for this window.
    per_generator: Vec<(u64, u32, Duration, Duration)>,
    /// Tick time inside the pass currently running.
    in_pass: Duration,
    in_pass_count: u32,
}

thread_local! {
    static TICKS: RefCell<TickStats> = RefCell::new(TickStats::default());
}

/// Times one Lisp generator tick on the scheduler thread.
pub(crate) struct TickTimer {
    wall: Instant,
    cpu: Duration,
}

impl TickTimer {
    pub(crate) fn start() -> Self {
        Self { wall: Instant::now(), cpu: thread_cpu_time() }
    }

    /// Record the tick of generator `id` at musical position `beat`.
    /// `ESEQ_SCHED_TICKS=1` logs every tick with its wall and CPU time.
    pub(crate) fn finish(self, id: u64, beat: f64) {
        let elapsed = self.wall.elapsed();
        if log_every_tick() {
            let cpu = thread_cpu_time().saturating_sub(self.cpu);
            eprintln!(
                "[sched-tick] gen{id} beat {beat:.3} wall {:.3} ms cpu {:.3} ms",
                elapsed.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 1e3,
            );
        }
        record_generator_tick(id, elapsed);
    }
}

fn log_every_tick() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var("ESEQ_SCHED_TICKS").is_ok_and(|value| value == "1"))
}

fn record_generator_tick(id: u64, elapsed: Duration) {
    TICKS.with(|ticks| {
        let mut ticks = ticks.borrow_mut();
        ticks.in_pass += elapsed;
        ticks.in_pass_count += 1;
        match ticks.per_generator.iter_mut().find(|entry| entry.0 == id) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 += elapsed;
                entry.3 = entry.3.max(elapsed);
            }
            None => ticks.per_generator.push((id, 1, elapsed, elapsed)),
        }
    });
}

/// One live scheduler loop's meter. Owned by the scheduler thread.
pub(crate) struct LoadMeter {
    sample_rate: f64,
    log: bool,
    spike_threshold: Duration,
    window_start: Instant,
    busy: Duration,
    passes: u32,
    max_pass: Duration,
    max_pass_cpu: Duration,
    min_slack_samples: i64,
    max_sleep_overshoot: Duration,
    pass_start: Instant,
    pass_cpu_start: Duration,
    pass_slack_samples: i64,
    sleep_requested: Option<(Instant, Duration)>,
}

impl LoadMeter {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let profile = std::env::var("ESEQ_SCHED_PROFILE").ok();
        let spike_threshold = profile
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|ms| *ms > 1.0)
            .map(|ms| Duration::from_secs_f64(ms / 1000.0))
            .unwrap_or(Duration::from_millis(5));
        let now = Instant::now();
        Self {
            sample_rate: sample_rate.max(1) as f64,
            log: profile.is_some_and(|value| !value.is_empty() && value != "0"),
            spike_threshold,
            window_start: now,
            busy: Duration::ZERO,
            passes: 0,
            max_pass: Duration::ZERO,
            max_pass_cpu: Duration::ZERO,
            min_slack_samples: i64::MAX,
            max_sleep_overshoot: Duration::ZERO,
            pass_start: now,
            pass_cpu_start: Duration::ZERO,
            pass_slack_samples: 0,
            sleep_requested: None,
        }
    }

    fn samples_to_ms(&self, samples: i64) -> f64 {
        samples as f64 * 1000.0 / self.sample_rate
    }

    /// A pass starts. `scheduled_until` is the frontier the previous pass
    /// left; `rendered` is where the audio callback is now.
    pub(crate) fn begin_pass(&mut self, rendered: u64, scheduled_until: u64, playing: bool) {
        let now = Instant::now();
        if let Some((slept_at, requested)) = self.sleep_requested.take() {
            let overshoot = now.duration_since(slept_at).saturating_sub(requested);
            self.max_sleep_overshoot = self.max_sleep_overshoot.max(overshoot);
        }
        self.pass_start = now;
        self.pass_cpu_start = thread_cpu_time();
        self.pass_slack_samples = scheduled_until as i64 - rendered as i64;
        if playing {
            self.min_slack_samples = self.min_slack_samples.min(self.pass_slack_samples);
        }
        TICKS.with(|ticks| {
            let mut ticks = ticks.borrow_mut();
            ticks.in_pass = Duration::ZERO;
            ticks.in_pass_count = 0;
        });
    }

    /// The pass ended and the thread is about to sleep for `sleep`.
    pub(crate) fn end_pass(&mut self, sleep: Duration) {
        let now = Instant::now();
        let wall = now.duration_since(self.pass_start);
        let cpu = thread_cpu_time().saturating_sub(self.pass_cpu_start);
        self.busy += wall;
        self.passes += 1;
        if wall > self.max_pass {
            self.max_pass = wall;
            self.max_pass_cpu = cpu;
        }
        if self.log && wall >= self.spike_threshold {
            let (tick_time, tick_count) =
                TICKS.with(|ticks| { let ticks = ticks.borrow(); (ticks.in_pass, ticks.in_pass_count) });
            eprintln!(
                "[sched] slow pass {:.2} ms (cpu {:.2} ms, generator ticks {} = {:.2} ms), slack at start {:.1} ms",
                wall.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 1e3,
                tick_count,
                tick_time.as_secs_f64() * 1e3,
                self.samples_to_ms(self.pass_slack_samples),
            );
        }
        self.sleep_requested = Some((now, sleep));
        let elapsed = now.duration_since(self.window_start);
        if elapsed >= WINDOW {
            self.roll_window(elapsed, now);
        }
    }

    fn roll_window(&mut self, elapsed: Duration, now: Instant) {
        let per_generator = TICKS.with(|ticks| std::mem::take(&mut ticks.borrow_mut().per_generator));
        let max_tick = per_generator.iter().map(|entry| entry.3).max().unwrap_or_default();
        let busy = self.busy.as_secs_f64() / elapsed.as_secs_f64();
        let min_slack_us = if self.min_slack_samples == i64::MAX {
            i64::MAX
        } else {
            (self.samples_to_ms(self.min_slack_samples) * 1000.0) as i64
        };
        PUBLISHED.busy_permille.store((busy * 1000.0).round() as u32, Ordering::Relaxed);
        PUBLISHED.max_pass_us.store(self.max_pass.as_micros().min(u32::MAX as u128) as u32, Ordering::Relaxed);
        PUBLISHED.min_slack_us.store(min_slack_us as u64, Ordering::Relaxed);
        PUBLISHED.max_tick_us.store(max_tick.as_micros().min(u32::MAX as u128) as u32, Ordering::Relaxed);
        if self.log {
            let mut generators = String::new();
            for (id, count, total, max) in &per_generator {
                generators.push_str(&format!(
                    " gen{id}: {count}x avg {:.2} max {:.2} ms;",
                    total.as_secs_f64() * 1e3 / (*count).max(1) as f64,
                    max.as_secs_f64() * 1e3,
                ));
            }
            let slack = if self.min_slack_samples == i64::MAX {
                "-".to_string()
            } else {
                format!("{:.1}", self.samples_to_ms(self.min_slack_samples))
            };
            eprintln!(
                "[sched] busy {:.1}% passes {} max pass {:.2} ms (cpu {:.2}) min slack {slack} ms max oversleep {:.2} ms |{generators}",
                busy * 100.0,
                self.passes,
                self.max_pass.as_secs_f64() * 1e3,
                self.max_pass_cpu.as_secs_f64() * 1e3,
                self.max_sleep_overshoot.as_secs_f64() * 1e3,
            );
        }
        self.window_start = now;
        self.busy = Duration::ZERO;
        self.passes = 0;
        self.max_pass = Duration::ZERO;
        self.max_pass_cpu = Duration::ZERO;
        self.min_slack_samples = i64::MAX;
        self.max_sleep_overshoot = Duration::ZERO;
    }
}
