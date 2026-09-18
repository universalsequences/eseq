use super::*;

pub(crate) struct UiLoopStats {
    pub(crate) benchmark: ui_benchmark::UiBenchmark,
    enabled: bool,
    window_start: Instant,
    events: u64,
    syncs: u64,
    frames: u64,
    event_handle: Duration,
    gestures: Duration,
    host_commands: Duration,
    reactive_sync: Duration,
    frame_build: Duration,
    render: Duration,
    max_event: Duration,
    max_sync: Duration,
    max_frame_build: Duration,
    max_render: Duration,
    sync_samples: Vec<f64>,
    work_samples: Vec<f64>,
    input_submit_samples: Vec<f64>,
    pending_input: Option<Instant>,
    last_sync: Duration,
}

impl UiLoopStats {
    pub(crate) fn new() -> Self {
        Self {
            benchmark: ui_benchmark::UiBenchmark::default(),
            enabled: std::env::var_os("ESEQLISP_PROFILE_UI").is_some(),
            window_start: Instant::now(),
            events: 0,
            syncs: 0,
            frames: 0,
            event_handle: Duration::ZERO,
            gestures: Duration::ZERO,
            host_commands: Duration::ZERO,
            reactive_sync: Duration::ZERO,
            frame_build: Duration::ZERO,
            render: Duration::ZERO,
            max_event: Duration::ZERO,
            max_sync: Duration::ZERO,
            max_frame_build: Duration::ZERO,
            max_render: Duration::ZERO,
            sync_samples: Vec::new(), work_samples: Vec::new(), input_submit_samples: Vec::new(),
            pending_input: None, last_sync: Duration::ZERO,
        }
    }

    pub(crate) fn note_event(&mut self, elapsed: Duration, redraw: bool) {
        self.benchmark.note_event(elapsed, redraw);
        if !self.enabled {
            return;
        }
        if redraw && self.pending_input.is_none() { self.pending_input = Some(Instant::now() - elapsed); }
        self.events += 1;
        self.event_handle += elapsed;
        self.max_event = self.max_event.max(elapsed);
        self.maybe_emit();
    }

    pub(crate) fn note_gestures(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.gestures += elapsed;
        self.maybe_emit();
    }

    pub(crate) fn note_host_commands(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.host_commands += elapsed;
        self.maybe_emit();
    }

    pub(crate) fn note_sync(&mut self, elapsed: Duration) {
        self.benchmark.note_sync();
        if !self.enabled {
            return;
        }
        self.reactive_sync += elapsed;
        self.last_sync = elapsed;
        if self.sync_samples.len() < 4096 { self.sync_samples.push(duration_ms(elapsed)); }
        self.syncs += 1;
        self.max_sync = self.max_sync.max(elapsed);
        self.maybe_emit();
    }

    pub(crate) fn note_frame(&mut self, build: Duration, render: Duration, presented: bool) {
        self.benchmark.note_frame(presented);
        if !self.enabled {
            return;
        }
        self.frames += 1;
        if self.work_samples.len() < 4096 {
            self.work_samples.push(duration_ms(self.last_sync + build + render));
        }
        self.last_sync = Duration::ZERO;
        if presented {
            if let Some(input) = self.pending_input.take() {
                if self.input_submit_samples.len() < 4096 { self.input_submit_samples.push(duration_ms(input.elapsed())); }
            }
        }
        self.frame_build += build;
        self.render += render;
        self.max_frame_build = self.max_frame_build.max(build);
        self.max_render = self.max_render.max(render);
        self.maybe_emit();
    }

    fn maybe_emit(&mut self) {
        if !self.enabled || self.window_start.elapsed().as_secs_f64() < 1.0 {
            return;
        }
        let secs = self.window_start.elapsed().as_secs_f64();
        eprintln!(
            "[ui-profile][sequencer] events/s={:.1} render_attempts/s={:.1} event_avg={:.2}ms event_max={:.2}ms gestures={:.2}ms host={:.2}ms sync_avg={:.2}ms sync_max={:.2}ms frame_build_avg={:.2}ms frame_build_max={:.2}ms render_avg={:.2}ms render_max={:.2}ms",
            self.events as f64 / secs,
            self.frames as f64 / secs,
            avg_ms(self.event_handle, self.events),
            self.max_event.as_secs_f64() * 1000.0,
            self.gestures.as_secs_f64() * 1000.0,
            self.host_commands.as_secs_f64() * 1000.0,
            avg_ms(self.reactive_sync, self.syncs),
            self.max_sync.as_secs_f64() * 1000.0,
            avg_ms(self.frame_build, self.frames),
            self.max_frame_build.as_secs_f64() * 1000.0,
            avg_ms(self.render, self.frames),
            self.max_render.as_secs_f64() * 1000.0,
        );
        eprintln!(
            "[ui-profile][latency] sync_ms[p50,p95,p99]={:?} ui_work_ms[p50,p95,p99]={:?} input_to_submit_ms[p50,p95,p99]={:?} samples={}/{}/{} (software dispatch to render return; excludes display scanout)",
            percentiles(&mut self.sync_samples), percentiles(&mut self.work_samples),
            percentiles(&mut self.input_submit_samples), self.sync_samples.len(),
            self.work_samples.len(), self.input_submit_samples.len(),
        );
        self.sync_samples.clear(); self.work_samples.clear(); self.input_submit_samples.clear();
        self.window_start = Instant::now();
        self.events = 0;
        self.syncs = 0;
        self.frames = 0;
        self.event_handle = Duration::ZERO;
        self.gestures = Duration::ZERO;
        self.host_commands = Duration::ZERO;
        self.reactive_sync = Duration::ZERO;
        self.frame_build = Duration::ZERO;
        self.render = Duration::ZERO;
        self.max_event = Duration::ZERO;
        self.max_sync = Duration::ZERO;
        self.max_frame_build = Duration::ZERO;
        self.max_render = Duration::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpresented_frames_preserve_pending_input_latency() {
        let mut stats = UiLoopStats::new();
        stats.enabled = true;
        stats.note_event(Duration::from_millis(1), true);
        let pending = stats.pending_input;
        stats.note_frame(Duration::ZERO, Duration::ZERO, false);
        assert_eq!(stats.pending_input, pending);
        assert!(stats.input_submit_samples.is_empty());
        stats.note_frame(Duration::ZERO, Duration::ZERO, true);
        assert!(stats.pending_input.is_none());
        assert_eq!(stats.input_submit_samples.len(), 1);
        assert!(stats.input_submit_samples[0] >= 1.0);
    }
}

fn percentiles(samples: &mut [f64]) -> [f64; 3] {
    if samples.is_empty() { return [0.0; 3]; }
    samples.sort_unstable_by(f64::total_cmp);
    [50, 95, 99].map(|percent| samples[(samples.len() * percent).div_ceil(100).saturating_sub(1)])
}

pub(crate) fn avg_ms(total: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1000.0 / count as f64
    }
}

pub(crate) fn pattern_switch_profile_enabled() -> bool {
    std::env::var_os("METAL_SEQ_PROFILE_PATTERN_SWITCH").is_some()
}

pub(crate) fn duration_ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

pub(crate) fn log_active_voice_counts(state: &SequencerState, track_names: &[String]) {
    let num_tracks = state.active_track_count().min(track_names.len());
    if num_tracks == 0 {
        return;
    }
    let cpu_load = f32::from_bits(state.transport.cpu_load_pct.load(Ordering::Relaxed));
    let mut total = 0u32;
    let mut parts = Vec::with_capacity(num_tracks);
    for track in 0..num_tracks {
        let active = state.transport.active_voice_counts[track].load(Ordering::Relaxed);
        total += active;
        parts.push(format!("{}={active}", track_names[track]));
    }
    eprintln!(
        "[voice-counts] total={total} audio_cpu={cpu_load:.1}% {}",
        parts.join(" ")
    );

    let engine_stats = sequencer::lisp_host::take_dgen_engine_process_stats();
    let mut engine_parts = Vec::new();
    for stats in engine_stats {
        let configured = state.runtime.engine_voice_counts[stats.engine_id].load(Ordering::Relaxed);
        if configured == 0 {
            continue;
        }

        let mut active = 0u32;
        let mut bound_tracks = Vec::new();
        for track in 0..num_tracks {
            let engine_id = state.runtime.track_engine_ids[track].load(Ordering::Relaxed);
            if engine_id == stats.engine_id as u32 {
                active += state.transport.active_voice_counts[track].load(Ordering::Relaxed);
                bound_tracks.push(track_names[track].as_str());
            }
        }

        let avg_run = if stats.process_blocks == 0 {
            0.0
        } else {
            stats.process_calls as f64 / stats.process_blocks as f64
        };
        let tracks = if bound_tracks.is_empty() {
            "-".to_string()
        } else {
            bound_tracks.join(",")
        };
        engine_parts.push(format!(
            "engine{} active={} enabled={} configured={} calls={} blocks={} avg_run={avg_run:.2} tracks={}",
            stats.engine_id,
            active,
            stats.enabled_voices,
            configured,
            stats.process_calls,
            stats.process_blocks,
            tracks,
        ));
    }

    if !engine_parts.is_empty() {
        eprintln!("[dgen-voice-runs] {}", engine_parts.join(" | "));
    }

    let mod_stats = sequencer::instruments::voice_modulator::take_process_stats();
    if mod_stats.calls > 0 {
        eprintln!(
            "[modulator-runs] calls={} rendered={} disabled_custom={} disabled_sampler={} all_slots_off={} unbound_rendered={} rendered_frames={} disabled_frames={} all_slots_off_frames={}",
            mod_stats.calls,
            mod_stats.rendered_calls,
            mod_stats.disabled_custom_skips,
            mod_stats.disabled_sampler_skips,
            mod_stats.all_slots_off_calls,
            mod_stats.unbound_rendered_calls,
            mod_stats.rendered_frames,
            mod_stats.disabled_frames,
            mod_stats.all_slots_off_frames,
        );

        let mut mod_engine_parts = Vec::new();
        for stats in mod_stats.engines {
            let configured =
                state.runtime.engine_voice_counts[stats.engine_id].load(Ordering::Relaxed);
            if configured == 0 {
                continue;
            }
            mod_engine_parts.push(format!(
                "engine{} enabled={} configured={} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                stats.engine_id,
                stats.enabled_voices,
                configured,
                stats.calls,
                stats.rendered_calls,
                stats.disabled_skips,
                stats.rendered_frames,
                stats.disabled_frames,
            ));
        }

        if !mod_engine_parts.is_empty() {
            eprintln!("[modulator-engine-runs] {}", mod_engine_parts.join(" | "));
        }

        let mut sampler_parts = Vec::new();
        for stats in mod_stats.sampler_tracks {
            let track_name = track_names
                .get(stats.track_idx)
                .map(String::as_str)
                .unwrap_or("-");
            sampler_parts.push(format!(
                "{} active_mask=0x{:03x} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                track_name,
                stats.active_mask,
                stats.calls,
                stats.rendered_calls,
                stats.disabled_skips,
                stats.rendered_frames,
                stats.disabled_frames,
            ));
        }

        if !sampler_parts.is_empty() {
            eprintln!("[modulator-sampler-runs] {}", sampler_parts.join(" | "));
        }
    }
}
