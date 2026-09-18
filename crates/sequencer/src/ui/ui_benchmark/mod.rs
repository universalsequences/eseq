//! Measures the real UI thread between two OS CPU-counter reads. Native event
//! pumping, synchronization, rendering and destruction all lie inside that window;
//! waiting and work on other threads do not. No sampled stacks or audio probes.
mod ipc;
mod report;

use super::{AppBackend, Editor, SharedHandles};
use eseqlisp::ui::presentation_timing::{PresentationFeedback, PresentationTiming};
pub(crate) use ipc::{run_cli, Server};
use report::{Counters, Report, Workload};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub seconds: u64,
    pub warmup_seconds: u64,
    pub label: String,
    /// Calibration mode: keep the CPU counter and cheap loop counts, but omit
    /// drawable callbacks and input timestamps to measure observer overhead.
    pub cpu_only: bool,
}

impl Request {
    fn validate(&self) -> Result<(), String> {
        if !(2..=120).contains(&self.seconds) {
            return Err("seconds must be between 2 and 120".into());
        }
        if self.warmup_seconds > 30 {
            return Err("warmup must be at most 30 seconds".into());
        }
        if self.label.len() > 200 {
            return Err("label must be at most 200 bytes".into());
        }
        Ok(())
    }
}

pub(crate) struct PendingRequest {
    pub request: Request,
    pub reply: mpsc::Sender<Result<Report, String>>,
}

const MAX_SAMPLES: usize = 100_000;

struct Boundary {
    wall: Instant,
    cpu: Duration,
}

struct Run {
    request: Request,
    reply: mpsc::Sender<Result<Report, String>>,
    warmup_until: Instant,
    start: Option<Boundary>,
    end: Option<Boundary>,
    context_start: Option<Workload>,
    context_end: Option<Workload>,
    feedback: PresentationFeedback,
    counters: Counters,
    inputs: Vec<Instant>,
    presentations: Vec<PresentationTiming>,
    sample_overflows: u64,
}

impl Run {
    fn collect_presentations(&mut self) {
        for timing in self.feedback.receiver.try_iter() {
            if self
                .start
                .as_ref()
                .is_some_and(|start| timing.frame_started >= start.wall)
                && self
                    .end
                    .as_ref()
                    .is_none_or(|end| timing.frame_started < end.wall)
            {
                if self.presentations.len() < MAX_SAMPLES {
                    self.presentations.push(timing);
                } else {
                    self.sample_overflows += 1;
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct UiBenchmark {
    run: Option<Run>,
}

impl UiBenchmark {
    pub fn request(&mut self, pending: PendingRequest, backend: &mut AppBackend) {
        let error = if self.run.is_some() {
            Some("a UI benchmark is already running".to_string())
        } else {
            pending.request.validate().err()
        };
        if let Some(error) = error {
            let _ = pending.reply.send(Err(error));
            return;
        }
        let (observer, feedback) = PresentationFeedback::channel();
        #[cfg(target_os = "macos")]
        backend.set_presentation_observer((!pending.request.cpu_only).then_some(observer));
        #[cfg(not(target_os = "macos"))]
        let _ = (observer, backend);
        self.run = Some(Run {
            warmup_until: Instant::now() + Duration::from_secs(pending.request.warmup_seconds),
            request: pending.request,
            reply: pending.reply,
            start: None,
            end: None,
            context_start: None,
            context_end: None,
            feedback,
            counters: Counters::default(),
            inputs: Vec::with_capacity(4096),
            presentations: Vec::with_capacity(8192),
            sample_overflows: 0,
        });
    }

    /// Called at the outer loop boundary, including iterations that do not render.
    pub fn advance(
        &mut self,
        app: &sequencer::app::App,
        editor: &Editor,
        shared: &SharedHandles,
        backend: &mut AppBackend,
    ) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let now = Instant::now();
        run.collect_presentations();
        if run.start.is_none() {
            if now < run.warmup_until {
                return;
            }
            run.context_start = Some(Workload::capture(app, editor, shared, backend));
            match thread_cpu_time() {
                Ok(cpu) => {
                    run.start = Some(Boundary {
                        wall: Instant::now(),
                        cpu,
                    })
                }
                Err(error) => {
                    self.fail(error.to_string(), backend);
                    return;
                }
            }
        }
        let run = self.run.as_mut().unwrap();
        let start = run.start.as_ref().unwrap();
        if run.end.is_none()
            && now.saturating_duration_since(start.wall) >= Duration::from_secs(run.request.seconds)
        {
            match thread_cpu_time() {
                Ok(cpu) => {
                    run.end = Some(Boundary {
                        wall: Instant::now(),
                        cpu,
                    })
                }
                Err(error) => {
                    self.fail(error.to_string(), backend);
                    return;
                }
            }
            run.context_end = Some(Workload::capture(app, editor, shared, backend));
            #[cfg(target_os = "macos")]
            backend.set_presentation_observer(None);
        }
        let run = self.run.as_mut().unwrap();
        if let Some(end) = &run.end {
            // Let the last submitted frames report their actual display time.
            // This wait is asynchronous and lies outside the CPU score window.
            if run.request.cpu_only
                || !cfg!(target_os = "macos")
                || run.presentations.len() as u64 >= run.counters.submissions
                || now.saturating_duration_since(end.wall) >= Duration::from_secs(1)
            {
                let run = self.run.take().unwrap();
                let report = Report::from_run(&run);
                let _ = run.reply.send(Ok(report));
            }
        } else {
            run.counters.loop_iterations += 1;
        }
    }

    fn fail(&mut self, error: String, backend: &mut AppBackend) {
        #[cfg(target_os = "macos")]
        backend.set_presentation_observer(None);
        #[cfg(not(target_os = "macos"))]
        let _ = backend;
        if let Some(run) = self.run.take() {
            let _ = run.reply.send(Err(error));
        }
    }

    fn active(&mut self) -> Option<&mut Run> {
        self.run
            .as_mut()
            .filter(|run| run.start.is_some() && run.end.is_none())
    }

    pub fn note_poll(&mut self, timeout: Duration, delivered: bool) {
        if let Some(run) = self.active() {
            run.counters.poll_calls += 1;
            run.counters.zero_timeout_polls += u64::from(timeout.is_zero());
            run.counters.empty_polls += u64::from(!delivered);
        }
    }

    pub fn note_event(&mut self, elapsed: Duration, redraw: bool) {
        if let Some(run) = self.active() {
            run.counters.events += 1;
            if redraw && !run.request.cpu_only && cfg!(target_os = "macos") {
                if run.inputs.len() < MAX_SAMPLES {
                    run.inputs.push(Instant::now() - elapsed);
                } else {
                    run.sample_overflows += 1;
                }
            }
        }
    }

    pub fn note_sync(&mut self) {
        if let Some(run) = self.active() {
            run.counters.syncs += 1;
        }
    }

    pub fn note_frame(&mut self, submitted: bool) {
        if let Some(run) = self.active() {
            run.counters.render_attempts += 1;
            run.counters.submissions += u64::from(submitted);
        }
    }
}

impl Drop for UiBenchmark {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            let _ = run
                .reply
                .send(Err("app closed before measurement completed".into()));
        }
    }
}

fn thread_cpu_time() -> std::io::Result<Duration> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // The OS writes a complete timespec on success. This clock measures only
    // the calling thread, which is the UI thread at both window boundaries.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, value.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let value = unsafe { value.assume_init() };
    Ok(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_cpu_counter_excludes_sleep_and_other_thread_work() {
        let cpu_start = thread_cpu_time().unwrap();
        let wall_start = Instant::now();
        let worker = std::thread::spawn(|| {
            let until = Instant::now() + Duration::from_millis(100);
            while Instant::now() < until {
                std::hint::black_box(123u64.wrapping_mul(456));
            }
        });
        std::thread::sleep(Duration::from_millis(100));
        worker.join().unwrap();
        let cpu = thread_cpu_time().unwrap() - cpu_start;
        assert!(
            cpu < wall_start.elapsed() / 2,
            "sleep and worker CPU leaked into UI clock: {cpu:?}"
        );
    }
}
