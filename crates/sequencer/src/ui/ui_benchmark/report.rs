use super::*;
use eseqlisp::backend::Backend;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct Workload {
    track_count: usize,
    selected_track: usize,
    playing: bool,
    visible_buffers: Vec<String>,
    viewport_cells: [usize; 2],
    window: Option<eseqlisp::ui::presentation_timing::WindowPresentationState>,
}

impl Workload {
    pub(super) fn capture(
        app: &sequencer::app::App,
        editor: &Editor,
        shared: &SharedHandles,
        backend: &AppBackend,
    ) -> Self {
        use std::sync::atomic::Ordering;
        let visible_buffers = editor
            .tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|id| editor.tile_root.find_leaf(id))
            .filter_map(|leaf| editor.buffers.get(leaf.buffer_idx))
            .map(|buffer| buffer.name.clone())
            .collect();
        let (cols, rows) = backend.viewport_size();
        Self {
            track_count: app.tracks.len(),
            selected_track: shared.current_track.load(Ordering::Relaxed),
            playing: shared.state.transport.playing.load(Ordering::Relaxed),
            visible_buffers,
            viewport_cells: [cols, rows],
            window: backend.presentation_window_state(),
        }
    }
}

#[derive(Default, Clone, serde::Serialize)]
pub(super) struct Counters {
    pub loop_iterations: u64,
    pub poll_calls: u64,
    pub zero_timeout_polls: u64,
    pub empty_polls: u64,
    pub events: u64,
    pub syncs: u64,
    pub render_attempts: u64,
    pub submissions: u64,
}

#[derive(serde::Serialize)]
pub(crate) struct Report {
    schema_version: u32,
    event_loop_mode: &'static str,
    pid: u32,
    debug_build: bool,
    label: String,
    elapsed_seconds: f64,
    main_thread_cpu_ms: f64,
    pub main_thread_cpu_ms_per_second: f64,
    pub presented_fps: Option<f64>,
    presentation_feedback_enabled: bool,
    presented_frames: usize,
    skipped_presentations: usize,
    missing_presentation_feedback: u64,
    feedback_channel_overflows: u64,
    sample_overflows: u64,
    presentation_interval_ms: Option<[f64; 3]>,
    pub input_dispatch_to_present_ms: Option<[f64; 3]>,
    input_latency_samples: usize,
    inputs_without_presentation: usize,
    counters: Counters,
    workload_start: Workload,
    workload_end: Workload,
    workload_changed: bool,
    diagnostic_flags: Vec<String>,
    latency_scope: &'static str,
}

impl Report {
    pub(super) fn from_run(run: &Run) -> Self {
        let start = run.start.as_ref().unwrap();
        let end = run.end.as_ref().unwrap();
        let elapsed = end.wall.duration_since(start.wall).as_secs_f64();
        let cpu = end.cpu.saturating_sub(start.cpu).as_secs_f64() * 1000.0;
        let presentation_feedback_enabled = cfg!(target_os = "macos") && !run.request.cpu_only;
        let mut shown: Vec<_> = run
            .presentations
            .iter()
            .filter_map(|sample| {
                sample
                    .displayed
                    .map(|displayed| (sample.frame_started, displayed))
            })
            .collect();
        shown.sort_by_key(|&(started, _)| started);
        let mut times: Vec<_> = shown.iter().map(|&(_, displayed)| displayed).collect();
        times.sort();
        let mut intervals: Vec<_> = times
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.0)
            .collect();
        let mut latencies = Vec::new();
        for input in &run.inputs {
            let next_frame = shown.partition_point(|&(started, _)| started < *input);
            if let Some(&(_, displayed)) = shown
                .get(next_frame)
                .filter(|&&(_, displayed)| displayed >= *input)
            {
                latencies.push(displayed.duration_since(*input).as_secs_f64() * 1000.0);
            }
        }
        let context_start = run.context_start.clone().unwrap();
        let context_end = run.context_end.clone().unwrap();
        let diagnostic_flags = [
            "ESEQLISP_PROFILE_UI",
            "ESEQ_DEBUG_BACKEND_POLL",
            "ESEQ_SCENE_TRACE",
            "METAL_SEQ_PROFILE_PATTERN_SWITCH",
        ]
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some())
        .map(str::to_string)
        .collect();
        Self { schema_version: 2, event_loop_mode: if cfg!(target_os = "macos") { "native-owned" } else { "polled" },
            pid: std::process::id(), debug_build: cfg!(debug_assertions), label: run.request.label.clone(), elapsed_seconds: elapsed,
            main_thread_cpu_ms: cpu, main_thread_cpu_ms_per_second: cpu / elapsed,
            presented_fps: presentation_feedback_enabled.then_some(shown.len() as f64 / elapsed),
            presentation_feedback_enabled,
            presented_frames: shown.len(), skipped_presentations: run.presentations.len() - shown.len(),
            missing_presentation_feedback: if presentation_feedback_enabled {
                run.counters.submissions.saturating_sub(run.presentations.len() as u64)
            } else { 0 },
            feedback_channel_overflows: run.feedback.dropped(), sample_overflows: run.sample_overflows,
            presentation_interval_ms: percentiles(&mut intervals), input_dispatch_to_present_ms: percentiles(&mut latencies),
            input_latency_samples: latencies.len(), inputs_without_presentation: run.inputs.len() - latencies.len(),
            counters: run.counters.clone(), workload_changed: context_start != context_end,
            workload_start: context_start, workload_end: context_end, diagnostic_flags,
            latency_scope: "software event dispatch to actual Metal drawable presentation; excludes OS input delivery and physical display response; null when unavailable or no input",
        }
    }
}

fn percentiles(values: &mut [f64]) -> Option<[f64; 3]> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    Some([50, 95, 99].map(|p| values[(values.len() * p).div_ceil(100) - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run(now: Instant) -> Run {
        let (_, feedback) = PresentationFeedback::channel();
        let (reply, _) = mpsc::channel();
        let context = Workload {
            track_count: 28,
            selected_track: 0,
            playing: true,
            visible_buffers: vec!["*sequencer*".into()],
            viewport_cells: [200, 80],
            window: None,
        };
        Run {
            request: Request {
                seconds: 20,
                warmup_seconds: 3,
                label: "test".into(),
                cpu_only: false,
            },
            reply,
            warmup_until: now,
            start: Some(Boundary {
                wall: now,
                cpu: Duration::from_secs(10),
            }),
            end: Some(Boundary {
                wall: now + Duration::from_secs(20),
                cpu: Duration::from_secs(16),
            }),
            context_start: Some(context.clone()),
            context_end: Some(context),
            feedback,
            counters: Counters {
                submissions: 1,
                ..Counters::default()
            },
            inputs: vec![],
            presentations: vec![],
            sample_overflows: 0,
        }
    }

    #[test]
    fn score_includes_all_ui_cpu_and_missing_input_is_not_zero_latency() {
        let now = Instant::now();
        let mut run = test_run(now);
        let report = Report::from_run(&run);
        assert_eq!(report.main_thread_cpu_ms_per_second, 300.0);
        assert_eq!(report.input_dispatch_to_present_ms, None);
        assert_eq!(
            report.missing_presentation_feedback,
            u64::from(cfg!(target_os = "macos"))
        );
        run.inputs.push(now + Duration::from_millis(1));
        run.presentations.push(PresentationTiming {
            frame_started: now + Duration::from_millis(4),
            submitted: now + Duration::from_millis(7),
            displayed: Some(now + Duration::from_millis(17)),
        });
        let report = Report::from_run(&run);
        assert_eq!(report.input_dispatch_to_present_ms, Some([16.0; 3]));
        assert_eq!(report.inputs_without_presentation, 0);
    }

    #[test]
    fn presentation_window_excludes_warmup_but_keeps_late_feedback() {
        let now = Instant::now();
        let mut run = test_run(now);
        let (observer, feedback) = PresentationFeedback::channel();
        run.feedback = feedback;
        for offset in [-1, 0, 19_999, 20_000] {
            let started = if offset < 0 {
                now - Duration::from_millis((-offset) as u64)
            } else {
                now + Duration::from_millis(offset as u64)
            };
            observer.report(PresentationTiming {
                frame_started: started,
                submitted: started,
                displayed: Some(started + Duration::from_millis(17)),
            });
        }
        run.collect_presentations();
        assert_eq!(run.presentations.len(), 2);
        assert_eq!(run.presentations[0].frame_started, now);
        assert!(run.presentations[1].displayed.unwrap() > run.end.as_ref().unwrap().wall);
    }

    #[test]
    fn dropped_drawables_do_not_satisfy_input_and_changed_workload_is_flagged() {
        let now = Instant::now();
        let mut run = test_run(now);
        run.inputs = vec![now, now + Duration::from_millis(50)];
        run.counters.submissions = 3;
        // Deliberately deliver callbacks out of order.
        run.presentations = vec![
            PresentationTiming {
                frame_started: now + Duration::from_millis(20),
                submitted: now + Duration::from_millis(21),
                displayed: Some(now + Duration::from_millis(34)),
            },
            PresentationTiming {
                frame_started: now,
                submitted: now,
                displayed: None,
            },
        ];
        run.context_end.as_mut().unwrap().visible_buffers = vec!["*scratch*".into()];
        let report = Report::from_run(&run);
        assert_eq!(report.input_dispatch_to_present_ms, Some([34.0; 3]));
        assert_eq!(report.inputs_without_presentation, 1);
        assert_eq!(report.skipped_presentations, 1);
        assert!(report.workload_changed);
        assert_eq!(report.presentation_interval_ms, None);
        run.request.cpu_only = true;
        run.presentations.clear();
        let report = Report::from_run(&run);
        assert_eq!(report.presented_fps, None);
        assert_eq!(report.missing_presentation_feedback, 0);
        assert_eq!(report.main_thread_cpu_ms_per_second, 300.0);
    }

    #[test]
    fn counters_stop_at_the_cpu_boundary() {
        let now = Instant::now();
        let mut run = test_run(now);
        let end = run.end.take();
        let mut benchmark = UiBenchmark { run: Some(run) };
        benchmark.note_poll(Duration::ZERO, false);
        benchmark.note_sync();
        benchmark.note_frame(true);
        benchmark.run.as_mut().unwrap().end = end;
        benchmark.note_poll(Duration::ZERO, false);
        benchmark.note_sync();
        benchmark.note_frame(true);
        let counters = &benchmark.run.as_ref().unwrap().counters;
        assert_eq!(counters.poll_calls, 1);
        assert_eq!(counters.zero_timeout_polls, 1);
        assert_eq!(counters.empty_polls, 1);
        assert_eq!(counters.syncs, 1);
        assert_eq!(counters.submissions, 2); // fixture starts with one submission
    }
}
