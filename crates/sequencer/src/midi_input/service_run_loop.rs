//! CoreMIDI requires the first client's thread to service its CFRunLoop.
//! A signalled source lets UI commands interrupt that wait without polling.
use core_foundation::base::TCFType;
use core_foundation::runloop::{
    kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext,
    CFRunLoopSourceCreate, CFRunLoopSourceSignal, CFRunLoopWakeUp,
};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

pub(super) struct Wake {
    source: CFRunLoopSource,
    run_loop: CFRunLoop,
}

// This source has no callback state. CFRunLoopSourceSignal and CFRunLoopWakeUp
// are designed for cross-thread use; retained CF objects outlive all senders.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Wake {
    pub(super) fn signal(&self) {
        unsafe {
            CFRunLoopSourceSignal(self.source.as_concrete_TypeRef());
            CFRunLoopWakeUp(self.run_loop.as_concrete_TypeRef());
        }
    }
}

pub(super) struct WorkerLoop {
    wake: Arc<OnceLock<Wake>>,
}

impl WorkerLoop {
    pub(super) fn new(wake: &Arc<OnceLock<Wake>>) -> Self {
        extern "C" fn perform(_: *const std::ffi::c_void) {}
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform,
        };
        let source = unsafe {
            let raw = CFRunLoopSourceCreate(std::ptr::null(), 0, &mut context);
            assert!(!raw.is_null(), "could not allocate MIDI command run loop source");
            CFRunLoopSource::wrap_under_create_rule(raw)
        };
        let run_loop = CFRunLoop::get_current();
        run_loop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
        assert!(wake.set(Wake { source, run_loop }).is_ok());
        Self { wake: wake.clone() }
    }

    pub(super) fn recv_timeout<T>(
        &self,
        rx: &mpsc::Receiver<T>,
        timeout: Duration,
    ) -> Result<T, mpsc::RecvTimeoutError> {
        // Commands queued during startup precede publication of the waker.
        // Check the queue first; later sends leave the source signalled until
        // it is handled, including sends racing entry into run_in_mode.
        match rx.try_recv() {
            Ok(command) => return Ok(command),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, timeout, true);
        // A native topology notification also returns to the caller for a scan.
        rx.try_recv().map_err(|error| match error {
            mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
            mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
        })
    }
}

impl Drop for WorkerLoop {
    fn drop(&mut self) {
        let wake = self.wake.get().unwrap();
        wake.run_loop.remove_source(&wake.source, unsafe { kCFRunLoopDefaultMode });
    }
}
