//! Optional presentation feedback. Disabled renderers allocate no callbacks.
//! A bounded channel prevents a paused consumer from retaining frame history.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};
use std::time::Instant;

/// Window context for comparing live measurements with the same display workload.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WindowPresentationState {
    pub physical_size: [u32; 2],
    pub focused: bool,
    pub visible: Option<bool>,
    /// None until the platform has reported an occlusion event.
    pub occluded: Option<bool>,
}

#[derive(Clone, Copy, Debug)]
pub struct PresentationTiming {
    pub frame_started: Instant,
    pub submitted: Instant,
    /// None means the drawable was skipped or no valid presentation time was reported.
    pub displayed: Option<Instant>,
}

#[derive(Clone)]
pub struct PresentationObserver {
    sender: mpsc::SyncSender<PresentationTiming>,
    dropped: Arc<AtomicU64>,
}

pub struct PresentationFeedback {
    pub receiver: mpsc::Receiver<PresentationTiming>,
    dropped: Arc<AtomicU64>,
}

impl PresentationFeedback {
    pub fn channel() -> (PresentationObserver, Self) {
        let (sender, receiver) = mpsc::sync_channel(1024);
        let dropped = Arc::new(AtomicU64::new(0));
        (
            PresentationObserver {
                sender,
                dropped: Arc::clone(&dropped),
            },
            Self { receiver, dropped },
        )
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl PresentationObserver {
    pub fn report(&self, timing: PresentationTiming) {
        if self.sender.try_send(timing).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn observe_metal_drawable(
        &self,
        drawable: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDrawable>,
        frame_started: Instant,
    ) {
        use objc2_metal::MTLDrawable;
        let observer = self.clone();
        let submitted = Instant::now();
        let host_time = objc2_quartz_core::CACurrentMediaTime();
        let callback = block2::RcBlock::new(
            move |drawable: std::ptr::NonNull<objc2::runtime::ProtocolObject<dyn MTLDrawable>>| {
                // Metal supplies a live drawable to its presented handler. Map its
                // Core Animation host clock onto the paired monotonic submission time;
                // callback scheduling delay must not become display latency.
                let presented = unsafe { drawable.as_ref() }.presentedTime();
                let delay = presented - host_time;
                let displayed = (presented > 0.0)
                    .then(|| std::time::Duration::try_from_secs_f64(delay).ok())
                    .flatten()
                    .and_then(|delay| submitted.checked_add(delay));
                observer.report(PresentationTiming {
                    frame_started,
                    submitted,
                    displayed,
                });
            },
        );
        // Metal copies the block and owns it until presentation completes.
        unsafe {
            drawable.addPresentedHandler(block2::RcBlock::as_ptr(&callback) as *mut _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_consumer_drops_feedback_instead_of_blocking_presentation() {
        let (observer, feedback) = PresentationFeedback::channel();
        let now = Instant::now();
        for _ in 0..1025 {
            observer.report(PresentationTiming {
                frame_started: now,
                submitted: now,
                displayed: Some(now),
            });
        }
        assert_eq!(feedback.dropped(), 1);
        assert_eq!(feedback.receiver.try_iter().count(), 1024);
    }
}
