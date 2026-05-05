use super::*;

pub(crate) struct UiLoopStats {
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
}

impl UiLoopStats {
    pub(crate) fn new() -> Self {
        Self {
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
        }
    }

    pub(crate) fn note_event(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
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
        if !self.enabled {
            return;
        }
        self.reactive_sync += elapsed;
        self.syncs += 1;
        self.max_sync = self.max_sync.max(elapsed);
        self.maybe_emit();
    }

    pub(crate) fn note_frame(&mut self, build: Duration, render: Duration) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
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
            "[ui-profile][sequencer] events/s={:.1} frames/s={:.1} event_avg={:.2}ms event_max={:.2}ms gestures={:.2}ms host={:.2}ms sync_avg={:.2}ms sync_max={:.2}ms frame_build_avg={:.2}ms frame_build_max={:.2}ms render_avg={:.2}ms render_max={:.2}ms",
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

pub(crate) fn avg_ms(total: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1000.0 / count as f64
    }
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
}
