//! Opt-in experiment harness. Live mode uses the production CPAL stream and
//! scheduler, with device output silenced after rendering. Offline mode uses
//! the export driver for reproducible audio checks. One run per process only:
//! the C engine and DGen image ownership are process-global.
use super::*;
use crate::app::App;
use crate::quantized_launch::PatternLaunchTarget;
use crossbeam_queue::ArrayQueue;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::PathBuf, sync::{OnceLock, atomic::AtomicBool}, time::Duration};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub project: PathBuf,
    /// User-visible scene number, starting at one.
    pub pattern: usize,
    pub workers: i32,
    pub worker_spins: i32,
    pub worker_wait_us: i32,
    pub callback_spins: i32,
    pub callback_wait_us: i32,
    #[serde(default)]
    pub queue_hint: bool,
    pub warmup_seconds: f64,
    pub measure_seconds: f64,
    pub offline: bool,
    pub sample_rate: u32,
    /// Limit sanitizer scopes to the measured playback interval. False audits
    /// startup, loading, warmup and teardown too. Requires audio-rtsan.
    #[serde(default)]
    pub rtsan_measure_only: bool,
}

#[derive(Serialize)]
pub(super) struct CallbackPhases {
    pub snapshot_transport_us: f64,
    pub pool_sync_us: f64,
    pub live_input_us: f64,
    pub control_params_us: f64,
    pub scheduled_events_us: f64,
    pub voice_retirement_us: f64,
    pub render_us: f64,
    pub post_render_us: f64,
}

/// A phase boundary uses only the monotonic clock; all formatting and storage
/// beyond the bounded capture queue happen after the stream has stopped.
pub(super) fn phase_elapsed(previous: &mut Instant) -> f64 {
    let now = Instant::now();
    let elapsed_us = now.duration_since(*previous).as_secs_f64() * 1e6;
    *previous = now;
    elapsed_us
}

#[derive(Clone, Copy, Default, Serialize)]
pub(super) struct EventProfile {
    pub countdown_us: f64,
    pub queue_us: f64,
    pub dispatch_us: f64,
    pub event_count: usize,
    pub slowest_event: Option<EventTiming>,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct EventTiming {
    kind: &'static str,
    track: usize,
    step: Option<usize>,
    frame_offset: u32,
    elapsed_us: f64,
}

impl EventTiming {
    pub(super) fn new(event: &BlockEvent) -> Self {
        let (kind, track, step) = match &event.kind {
            BlockEventKind::Scheduled(scheduled) => match &scheduled.event.kind {
                ScheduledEventKind::ResolvedTrigger { track, step, .. } => ("trigger", scheduled.track, Some(*step)),
                ScheduledEventKind::NetworkTrigger { track, .. } => ("network_trigger", scheduled.track, None),
                ScheduledEventKind::InstrumentParams { track, .. } => ("instrument_params", scheduled.track, None),
                ScheduledEventKind::EffectParams { track, .. } => ("effect_params", scheduled.track, None),
                ScheduledEventKind::RackParams { track, step } => ("rack_params", scheduled.track, Some(*step)),
            },
            BlockEventKind::GateOff(event) => ("gate_off", event.track_idx, None),
            BlockEventKind::Retrig(event) => ("retrig", event.track_idx, Some(event.step)),
        };
        Self { kind, track, step, frame_offset: event.frame_offset, elapsed_us: 0.0 }
    }

    pub(super) fn finish(mut self, start: Instant, profile: &mut EventProfile) {
        self.elapsed_us = start.elapsed().as_secs_f64() * 1e6;
        profile.event_count += 1;
        if profile.slowest_event.is_none_or(|previous| self.elapsed_us > previous.elapsed_us) {
            profile.slowest_event = Some(self);
        }
    }
}

#[derive(Serialize)]
struct Block {
    elapsed_us: f64,
    phases: CallbackPhases,
    events: EventProfile,
    frames: usize,
    rendered_samples: u64,
    dropped: u64,
    late: u64,
    peak: f32,
    sum_squares: f64,
    nonfinite: usize,
}

struct Capture {
    enabled: AtomicBool,
    overflow: AtomicBool,
    blocks: ArrayQueue<Block>,
}
static CAPTURE: OnceLock<Capture> = OnceLock::new();

pub(super) fn silence_device() -> bool { CAPTURE.get().is_some() }

pub(super) fn record_block(
    start: Instant, phases: CallbackPhases, data: &AudioCallbackData, output: &[f32],
) {
    let elapsed_us = start.elapsed().as_secs_f64() * 1e6;
    let Some(capture) = CAPTURE.get() else { return; };
    if !capture.enabled.load(Ordering::Acquire) { return; }
    let mut block = Block {
        elapsed_us, phases, events: data.event_profile,
        frames: output.len() / data.num_channels,
        rendered_samples: data.rendered_samples.load(Ordering::Acquire),
        dropped: data.dropped_scheduled_events as u64,
        late: data.late_scheduled_events as u64,
        peak: 0.0, sum_squares: 0.0, nonfinite: 0,
    };
    for &sample in output {
        if sample.is_finite() {
            block.peak = block.peak.max(sample.abs());
            block.sum_squares += (sample as f64).powi(2);
        } else { block.nonfinite += 1; }
    }
    if capture.blocks.push(block).is_err() {
        capture.overflow.store(true, Ordering::Release);
    }
}

extern "C" {
    fn audiograph_configure_experiment(worker_spins: i32, worker_wait_us: i32,
        callback_spins: i32, callback_wait_us: i32, queue_hint: i32);
}

fn cpu_seconds() -> io::Result<f64> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(time.tv_sec as f64 + time.tv_nsec as f64 * 1e-9)
}

fn load(app: &mut App, config: &Config, offline: bool) -> Result<()> {
    let project = crate::project::load_project_from_path(&config.project)?;
    let name = project.name.clone();
    app.sample_analysis = crate::analysis::AnalysisService::synchronous();
    app.queue_bounce_project(&name, project)?;
    let started = Instant::now();
    while app.has_pending_project_load() {
        if started.elapsed() > Duration::from_secs(120) {
            return Err("Project loading exceeded 120 seconds".into());
        }
        app.advance_pending_project_load()?;
        if offline {
            if !unsafe { prepare_graph_for_render(app.graph.lg.0) } {
                return Err("Project graph preparation failed".into());
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    app.sample_analysis.require_complete()?;
    app.publish_all_sampler_analysis_runtime();
    app.apply_pattern_launch(&PatternLaunchTarget::Scene { scene: config.pattern - 1 })
        .map_err(|error| format!("Cannot launch scene: {error:?}"))?;
    app.refresh_latency_compensation();
    if offline && !unsafe { prepare_graph_for_render(app.graph.lg.0) } {
        return Err("Scene graph preparation failed".into());
    }
    eprintln!("[audio-experiment] loaded {} tracks, scene {}, {} BPM",
        app.tracks.len(), config.pattern, app.state.latest_scheduler_snapshot().transport.bpm);
    Ok(())
}

// Keep App-owned resources alive until the stream and graph workers stop,
// including on error paths during loading or measurement.
struct LiveOwner {
    engine: Option<engine::Engine>,
    app: Option<App>,
}
impl Drop for LiveOwner {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            drop(engine._stream);
            unsafe {
                clear_os_workgroup();
                engine_stop_workers();
                self.app.take();
                destroy_live_graph(engine.lg_ptr.0);
            }
        }
    }
}
struct OfflineOwner(engine::HeadlessEngine);
impl Drop for OfflineOwner {
    fn drop(&mut self) { unsafe { self.0.destroy(); } }
}

fn live_interval(app: &mut App, seconds: f64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < deadline {
        app.drain_due_mixer_controls(app.state.audio_rendered_sample());
        let errors = app.state.drain_generator_tick_errors();
        if !errors.is_empty() { return Err(format!("Generator failure: {errors:?}").into()); }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn instrument_voice_stats(app: &App) -> Vec<serde_json::Value> {
    crate::lisp_host::take_dgen_engine_process_stats().into_iter().filter_map(|stats| {
        let engine = app.editor.engine_registry.get(stats.engine_id)?;
        Some(serde_json::json!({
            "engine_id": stats.engine_id, "name": engine.name,
            "source_sha256": format!("{:x}", Sha256::digest(engine.source.as_bytes())),
            "configured_voices": app.state.runtime.engine_voice_counts[stats.engine_id].load(Ordering::Acquire),
            "enabled_voices_at_end": stats.enabled_voices,
            "process_calls": stats.process_calls, "voice_zero_calls": stats.process_blocks,
        }))
    }).collect()
}

pub fn run(mut config: Config) -> Result<serde_json::Value> {
    #[cfg(feature = "audio-heap-audit")]
    let heap_calibration = Some(crate::heap_audit::calibrate()?);
    #[cfg(not(feature = "audio-heap-audit"))]
    let heap_calibration = None::<()>;
    if config.rtsan_measure_only && !cfg!(feature = "audio-rtsan") {
        return Err("rtsan_measure_only requires an audio-rtsan build".into());
    }
    #[cfg(feature = "audio-rtsan")]
    {
        rtsan_standalone::ensure_initialized();
        super::rt_audit::set_enabled(!config.rtsan_measure_only);
    }
    if config.pattern == 0 || !(0..=16).contains(&config.workers)
        || !(1..=4096).contains(&config.worker_spins)
        || !(1..=4096).contains(&config.callback_spins)
        || !(1..=10_000).contains(&config.worker_wait_us)
        || !(0..=1_000).contains(&config.callback_wait_us)
        || !config.warmup_seconds.is_finite() || !(0.0..=60.0).contains(&config.warmup_seconds)
        || !config.measure_seconds.is_finite() || !(0.1..=60.0).contains(&config.measure_seconds)
        || !(8_000..=192_000).contains(&config.sample_rate)
    { return Err("Invalid experiment config".into()); }
    config.project = config.project.canonicalize()?;
    crate::app_paths::init_dev()?;
    crate::paths::enter_sequencer_dir()?;
    std::env::set_var("TINYSEQ_AUDIOGRAPH_WORKERS", config.workers.to_string());
    std::env::set_var("TINYSEQ_AUDIOGRAPH_RT", "1");
    unsafe { audiograph_configure_experiment(config.worker_spins, config.worker_wait_us,
        config.callback_spins, config.callback_wait_us, config.queue_hint as i32); }
    CAPTURE.set(Capture {
        enabled: AtomicBool::new(false), overflow: AtomicBool::new(false),
        blocks: ArrayQueue::new(32_768),
    }).map_err(|_| "Only one experiment is allowed per process")?;
    let capture = CAPTURE.get().unwrap();
    let (sample_rate, channels, cpu, wall, audio_hash, instrument_stats) = if config.offline {
        let owner = OfflineOwner(engine::init_headless_engine(config.sample_rate, 2)?);
        let engine = &owner.0;
        let mut app = App::new(Arc::clone(&engine.state), engine.lg_ptr, engine.sample_rate,
            engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone());
        load(&mut app, &config, true)?;
        engine.state.start_playback();
        let warmup_blocks = (config.warmup_seconds * engine.sample_rate as f64 / engine.block_size as f64).ceil() as u64;
        let measure_blocks = (config.measure_seconds * engine.sample_rate as f64 / engine.block_size as f64).ceil() as u64;
        let mut session = offline::OfflineAudioSession::new(engine,
            (warmup_blocks + measure_blocks) * engine.block_size as u64)?;
        let mut output = vec![0.0; engine.block_size * 2];
        let mut hash = Sha256::new();
        for block in 0..warmup_blocks {
            session.render_block_with_controls(block * engine.block_size as u64, &mut output,
                |sample| { app.drain_due_mixer_controls(sample); Ok(()) })?;
        }
        crate::lisp_host::take_dgen_engine_process_stats();
        let cpu_start = cpu_seconds()?;
        let start = Instant::now();
        #[cfg(feature = "audio-rtsan")]
        super::rt_audit::set_enabled(true);
        #[cfg(feature = "audio-heap-audit")]
        crate::heap_audit::begin();
        capture.enabled.store(true, Ordering::Release);
        for block in warmup_blocks..warmup_blocks + measure_blocks {
            session.render_block_with_controls(block * engine.block_size as u64, &mut output,
                |sample| { app.drain_due_mixer_controls(sample); Ok(()) })?;
            for sample in &output { hash.update(sample.to_le_bytes()); }
        }
        capture.enabled.store(false, Ordering::Release);
        #[cfg(feature = "audio-heap-audit")]
        crate::heap_audit::end();
        #[cfg(feature = "audio-rtsan")]
        super::rt_audit::set_enabled(!config.rtsan_measure_only);
        let cpu = cpu_seconds()? - cpu_start;
        let wall = start.elapsed().as_secs_f64();
        (engine.sample_rate, 2, cpu, wall,
            Some(format!("{:x}", hash.finalize())), instrument_voice_stats(&app))
    } else {
        let mut owner = LiveOwner { engine: Some(engine::init_engine()?), app: None };
        let engine = owner.engine.as_ref().unwrap();
        owner.app = Some(App::new(Arc::clone(&engine.state), engine.lg_ptr, engine.sample_rate,
            engine.buses.clone(), Arc::clone(&engine.master_recorder), engine.keyboard_tx.clone()));
        let app = owner.app.as_mut().unwrap();
        load(app, &config, false)?;
        // Allow the live callback to apply the final scene/latency edits before Play.
        live_interval(app, 0.1)?;
        engine.state.start_playback();
        live_interval(app, config.warmup_seconds)?;
        crate::lisp_host::take_dgen_engine_process_stats();
        let cpu_start = cpu_seconds()?;
        let start = Instant::now();
        #[cfg(feature = "audio-rtsan")]
        super::rt_audit::set_enabled(true);
        #[cfg(feature = "audio-heap-audit")]
        crate::heap_audit::begin();
        capture.enabled.store(true, Ordering::Release);
        live_interval(app, config.measure_seconds)?;
        capture.enabled.store(false, Ordering::Release);
        #[cfg(feature = "audio-heap-audit")]
        crate::heap_audit::end();
        #[cfg(feature = "audio-rtsan")]
        super::rt_audit::set_enabled(!config.rtsan_measure_only);
        let cpu = cpu_seconds()? - cpu_start;
        let wall = start.elapsed().as_secs_f64();
        let instrument_stats = instrument_voice_stats(app);
        eprintln!("[audio-experiment] measurement complete; stopping stream");
        engine.state.stop_playback();
        (engine.sample_rate, engine.channels as usize, cpu, wall, None, instrument_stats)
    };
    #[cfg(feature = "audio-heap-audit")]
    let heap_report = Some(crate::heap_audit::snapshot());
    #[cfg(feature = "audio-heap-audit")]
    let heap_passed = heap_report.as_ref().map(|report| {
        report.callback == crate::heap_audit::Counts::default()
            && report.workers == crate::heap_audit::Counts::default()
            && report.callbacks_entered > 0
            && report.worker_threads_entered == config.workers as u64
    });
    #[cfg(not(feature = "audio-heap-audit"))]
    let (heap_report, heap_passed) = (None::<()>, None::<bool>);
    if capture.overflow.load(Ordering::Acquire) { return Err("Metric queue overflow".into()); }
    let mut blocks = Vec::new();
    while let Some(block) = capture.blocks.pop() { blocks.push(block); }
    if blocks.is_empty() { return Err("No audio blocks captured".into()); }
    let mut loads: Vec<_> = blocks.iter().map(|block|
        block.elapsed_us * sample_rate as f64 / block.frames as f64 / 10_000.0).collect();
    loads.sort_by(f64::total_cmp);
    let percentile = |p: f64| loads[((loads.len() - 1) as f64 * p).round() as usize];
    let frames: usize = blocks.iter().map(|block| block.frames).sum();
    let audio_seconds = frames as f64 / sample_rate as f64;
    let peak = blocks.iter().map(|block| block.peak).fold(0.0, f32::max);
    let nonfinite: usize = blocks.iter().map(|block| block.nonfinite).sum();
    if nonfinite != 0 || peak < 0.0001 { return Err(format!("Invalid/silent render: peak={peak}, nonfinite={nonfinite}").into()); }
    Ok(serde_json::json!({
        "config": config, "sample_rate": sample_rate, "channels": channels,
        "rtsan_enabled": cfg!(feature = "audio-rtsan"),
        "rust_heap_calibration": heap_calibration,
        "rust_heap_audit": heap_report,
        "rust_heap_audit_passed": heap_passed,
        "audio_seconds": audio_seconds, "wall_seconds": wall, "process_cpu_seconds": cpu,
        "process_cpu_pct": cpu / wall * 100.0, "cpu_pct_per_audio_second": cpu / audio_seconds * 100.0,
        "callback_mean_pct": loads.iter().sum::<f64>() / loads.len() as f64,
        "callback_p50_pct": percentile(0.5), "callback_p95_pct": percentile(0.95),
        "callback_p99_pct": percentile(0.99), "callback_max_pct": percentile(1.0),
        "over_budget_blocks": loads.iter().filter(|&&load| load > 100.0).count(),
        "peak": peak,
        "rms": (blocks.iter().map(|block| block.sum_squares).sum::<f64>() / (frames * channels) as f64).sqrt(),
        "dropped_events": blocks.iter().map(|block| block.dropped).max(),
        "late_events": blocks.iter().map(|block| block.late).max(),
        "measured_dropped_events": blocks.last().unwrap().dropped.saturating_sub(blocks.first().unwrap().dropped),
        "measured_late_events": blocks.last().unwrap().late.saturating_sub(blocks.first().unwrap().late),
        "audio_sha256": audio_hash, "instrument_voice_stats": instrument_stats, "blocks": blocks,
    }))
}
