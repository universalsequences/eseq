//! Control-thread ownership of the active AudioUnit's workgroup. CoreAudio
//! property reads, reference releases and reporting never run on audio threads.
//!
//! Membership is adaptive. Joined helpers protect the callback tail on heavy
//! graphs (garageddd B11: p99 5.92 -> 3.78 ms, docs/garageddd-b11-workgroups-
//! 2026-09-14.md), but on light graphs the system wakes them 120-250 us after
//! the block starts versus ~10 us unjoined, so they arrive after the callback
//! thread has finished alone (superbasicsetting: 3.6% -> 2.5% transport CPU
//! unjoined, eseq-v6te). Helpers therefore join only while the callback load
//! (exact per 100 ms window) stays high, with hysteresis so membership does
//! not flap.

use super::audiograph;
use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Transport CPU over a poll window at or above which helpers join.
const JOIN_LOAD_PCT: f32 = 20.0;
/// Joined helpers leave once the load falls below this.
const LEAVE_LOAD_PCT: f32 = 10.0;
/// Consecutive 100 ms polls a switch must hold for. Stream start and project
/// loads produce brief load spikes that must not flap membership.
const SWITCH_POLLS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "audio-experiments", derive(serde::Serialize))]
pub(super) enum Policy {
    /// Join only under heavy load (shipping default).
    Adaptive,
    Always,
    Never,
}

impl Policy {
    fn wants_membership(self, joined: bool, load_pct: f32) -> bool {
        match self {
            Policy::Always => true,
            Policy::Never => false,
            Policy::Adaptive if joined => load_pct >= LEAVE_LOAD_PCT,
            Policy::Adaptive => load_pct >= JOIN_LOAD_PCT,
        }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "audio-experiments", derive(serde::Serialize))]
pub(super) struct Report {
    pub policy: Policy,
    /// Whether helpers are currently bound to the device workgroup.
    pub enabled: bool,
    pub binding: audiograph::EngineWorkgroupStatus,
    pub source_error: Option<String>,
    pub refreshes: u64,
    pub changes: u64,
    pub verification_failures: u64,
    /// Consecutive polls that wanted the opposite membership.
    pending_switch_polls: u32,
}

impl Report {
    fn new(policy: Policy) -> Self {
        Self {
            policy,
            pending_switch_polls: 0,
            enabled: false,
            binding: Default::default(),
            source_error: None,
            refreshes: 0,
            changes: 0,
            verification_failures: 0,
        }
    }
}

impl Report {
    pub fn verified(&self) -> bool {
        self.source_error.is_none() && self.binding.supported != 0
            && self.binding.pending_workers == 0 && self.binding.failed_workers == 0
            && if self.enabled {
                self.binding.assigned != 0
                    && self.binding.joined_workers == self.binding.worker_count
            } else {
                self.binding.assigned == 0 && self.binding.joined_workers == 0
            }
    }
}

/// `AudioUnitGetProperty(OSWorkgroup)` returns +1, unlike most property reads.
struct OwnedWorkgroup(*mut c_void);
impl Drop for OwnedWorkgroup {
    fn drop(&mut self) { unsafe { audiograph::engine_release_os_workgroup(self.0); } }
}

#[derive(Default)]
struct Binding {
    group: Option<OwnedWorkgroup>,
}
impl Drop for Binding {
    fn drop(&mut self) { unsafe { audiograph::clear_os_workgroup(); } }
}
impl Binding {
    fn refresh(&mut self, stream: &cpal::platform::CoreAudioStream, report: &mut Report, load_pct: f32) {
        report.refreshes += 1;
        let wanted = if report.refreshes == 1 {
            // Nothing has rendered yet; start from the policy's idle choice.
            report.policy.wants_membership(false, 0.0)
        } else if report.policy.wants_membership(report.enabled, load_pct) != report.enabled {
            report.pending_switch_polls += 1;
            if report.pending_switch_polls >= SWITCH_POLLS { !report.enabled } else { report.enabled }
        } else {
            report.pending_switch_polls = 0;
            report.enabled
        };
        if wanted != report.enabled {
            report.pending_switch_polls = 0;
        }
        let next = stream.audio_workgroup().map_err(|error| error.to_string())
            .and_then(|pointer| if pointer.is_null() {
                Err("AudioUnit returned no device workgroup".to_string())
            } else { Ok(OwnedWorkgroup(pointer)) });
        let (next, error) = match next {
            Ok(group) => (Some(group), None),
            Err(error) => (None, Some(error)),
        };
        let previous_pointer = self.group.as_ref().map(|group| group.0);
        let next_pointer = next.as_ref().map(|group| group.0);
        if previous_pointer != next_pointer || report.refreshes == 1 || wanted != report.enabled {
            report.enabled = wanted;
            let pointer = if report.enabled { next_pointer.unwrap_or(std::ptr::null_mut()) }
                else { std::ptr::null_mut() };
            unsafe { audiograph::set_os_workgroup(pointer); }
            self.group = next;
            report.changes += 1;
        }
        report.source_error = error;
        report.binding = unsafe { audiograph::workgroup_status() };
        if !report.verified() { report.verification_failures += 1; }
    }
}

pub(super) struct Monitor {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    report: Arc<Mutex<Report>>,
}

impl Monitor {
    /// `load_ns` reads the cumulative callback (busy, budget) nanoseconds;
    /// each poll uses the load of the window since the previous one.
    pub fn start(
        stream: &cpal::Stream,
        load_ns: impl Fn() -> (u64, u64) + Send + 'static,
    ) -> Result<Self, String> {
        let cpal::platform::StreamInner::CoreAudio(stream) = stream.as_inner();
        let stream = stream.clone();
        #[cfg(feature = "audio-experiments")]
        let policy = super::experiment::workgroup_policy();
        #[cfg(not(feature = "audio-experiments"))]
        let policy = Policy::Adaptive;
        let report = Arc::new(Mutex::new(Report::new(policy)));
        let shared = Arc::clone(&report);
        let (stop, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("audio-workgroup".to_string()).spawn(move || {
            let mut binding = Binding::default();
            let mut current = Report::new(policy);
            let mut last_load = load_ns();
            loop {
                let previous = (current.binding, current.source_error.clone());
                let load = load_ns();
                let (busy, budget) = (load.0 - last_load.0, load.1 - last_load.1);
                last_load = load;
                // No callbacks in the window (stopped stream) reads as idle.
                let window_load_pct = if budget == 0 { 0.0 } else { busy as f32 / budget as f32 * 100.0 };
                binding.refresh(&stream, &mut current, window_load_pct);
                if previous != (current.binding, current.source_error.clone()) || current.refreshes == 1 {
                    eprintln!("audio: workgroup policy={:?} joined={} helpers={}/{} failed={} verified={} error={:?}",
                        current.policy, current.enabled, current.binding.joined_workers, current.binding.worker_count,
                        current.binding.failed_workers, current.verified(), current.source_error);
                }
                *shared.lock().unwrap() = current.clone();
                if current.refreshes == 1 { let _ = ready_tx.send(()); }
                // Poll the stream itself, not the global default device or its
                // display name. A device/workgroup change is adopted within
                // this interval, without adding a callback lock or API call.
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
            // Binding clears membership before its retained reference and
            // this thread's stream clone are released, including on unwind.
        }).map_err(|error| format!("Cannot start audio workgroup monitor: {error}"))?;
        let monitor = Self { stop, thread: Some(thread), report };
        ready_rx.recv().map_err(|error| format!("Audio workgroup startup failed: {error}"))?;
        Ok(monitor)
    }

    pub fn report(&self) -> Report { self.report.lock().unwrap().clone() }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() { eprintln!("audio: workgroup monitor panicked"); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_membership_joins_under_load_with_hysteresis() {
        let adaptive = Policy::Adaptive;
        assert!(!adaptive.wants_membership(false, 3.0));
        assert!(!adaptive.wants_membership(false, JOIN_LOAD_PCT - 0.1));
        assert!(adaptive.wants_membership(false, JOIN_LOAD_PCT));
        // Once joined, stay joined until the load clearly drops.
        assert!(adaptive.wants_membership(true, (JOIN_LOAD_PCT + LEAVE_LOAD_PCT) / 2.0));
        assert!(!adaptive.wants_membership(true, LEAVE_LOAD_PCT - 0.1));
        assert!(Policy::Always.wants_membership(false, 0.0));
        assert!(!Policy::Never.wants_membership(true, 100.0));
    }

    #[test]
    #[ignore = "opens the default macOS audio output with a silent graph"]
    fn macos_output_stream_verifies_helpers_and_releases_membership() {
        let engine = crate::audio::engine::init_engine().expect("start silent CoreAudio stream");
        let initial = engine._stream.workgroup.as_ref().unwrap().report();
        assert!(initial.verified(), "{initial:?}");
        // A silent graph is far below the join threshold: the adaptive
        // policy verifies an unbound pool (every helper out of the group).
        assert_eq!(initial.policy, Policy::Adaptive);
        assert!(!initial.enabled);
        assert!(initial.binding.worker_count > 0);
        assert_eq!(
            initial.binding.worker_count as u32,
            crate::audio::worker_prefs::running_worker_count()
                .expect("engine recorded its worker count")
        );
        assert_eq!(initial.binding.joined_workers, 0);
        std::thread::sleep(Duration::from_millis(350));
        let observed = engine._stream.workgroup.as_ref().unwrap().report();
        assert!(observed.verified(), "{observed:?}");
        assert!(observed.refreshes > initial.refreshes);
        assert_eq!(observed.changes, initial.changes);
        drop(engine._stream);
        let departed = unsafe { audiograph::workgroup_status() };
        assert_eq!(departed.assigned, 0);
        assert_eq!(departed.joined_workers, 0);
        unsafe {
            audiograph::engine_stop_workers();
            audiograph::destroy_live_graph(engine.lg_ptr.0);
        }
    }
}
