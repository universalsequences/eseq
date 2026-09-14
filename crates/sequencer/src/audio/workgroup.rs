//! Control-thread ownership of the active AudioUnit's workgroup. CoreAudio
//! property reads, reference releases and reporting never run on audio threads.

use super::audiograph;
use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "audio-experiments", derive(serde::Serialize))]
pub(super) struct Report {
    pub enabled: bool,
    pub binding: audiograph::EngineWorkgroupStatus,
    pub source_error: Option<String>,
    pub refreshes: u64,
    pub changes: u64,
    pub verification_failures: u64,
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
    fn refresh(&mut self, stream: &cpal::platform::CoreAudioStream, report: &mut Report) {
        report.refreshes += 1;
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
        if previous_pointer != next_pointer || report.refreshes == 1 {
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
    pub fn start(stream: &cpal::Stream) -> Result<Self, String> {
        let cpal::platform::StreamInner::CoreAudio(stream) = stream.as_inner();
        let stream = stream.clone();
        #[cfg(feature = "audio-experiments")]
        let enabled = super::experiment::workgroups_enabled();
        #[cfg(not(feature = "audio-experiments"))]
        let enabled = true;
        let report = Arc::new(Mutex::new(Report { enabled, ..Report::default() }));
        let shared = Arc::clone(&report);
        let (stop, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("audio-workgroup".to_string()).spawn(move || {
            let mut binding = Binding::default();
            let mut current = Report { enabled, ..Report::default() };
            loop {
                let previous = (current.binding, current.source_error.clone());
                binding.refresh(&stream, &mut current);
                if previous != (current.binding, current.source_error.clone()) || current.refreshes == 1 {
                    eprintln!("audio: workgroup enabled={} helpers={}/{} failed={} verified={} error={:?}",
                        enabled, current.binding.joined_workers, current.binding.worker_count,
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
    #[ignore = "opens the default macOS audio output with a silent graph"]
    fn macos_output_stream_verifies_helpers_and_releases_membership() {
        let engine = crate::audio::engine::init_engine().expect("start silent CoreAudio stream");
        let initial = engine._stream.workgroup.as_ref().unwrap().report();
        assert!(initial.verified(), "{initial:?}");
        assert!(initial.enabled);
        assert_eq!(initial.binding.worker_count, 4);
        assert_eq!(initial.binding.joined_workers, 4);
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
