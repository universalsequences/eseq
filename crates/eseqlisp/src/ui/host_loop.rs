use std::time::Instant;

/// Work requested by the native window loop. Live resize must draw inside the
/// native callback so AppKit never stretches an old frame during a modal drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostLoopAction {
    Tick,
    LiveResize,
}

/// The host owns its next deadline; the backend owns waiting and event delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostLoopControl {
    WaitUntil(Instant),
    Exit,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) struct HostSchedule {
    deadline: Instant,
    requested: bool,
}

#[cfg(any(target_os = "macos", test))]
impl HostSchedule {
    pub(crate) fn new(now: Instant) -> Self {
        Self { deadline: now, requested: true }
    }

    pub(crate) fn request_tick(&mut self) { self.requested = true; }

    pub(crate) fn take_tick(&mut self, now: Instant, input_pending: bool) -> bool {
        if !self.requested && !input_pending && now < self.deadline { return false; }
        self.requested = false;
        true
    }

    pub(crate) fn set_deadline(&mut self, deadline: Instant) { self.deadline = deadline; }

    pub(crate) fn control_flow(&self, input_pending: bool) -> winit::event_loop::ControlFlow {
        if self.requested || input_pending { winit::event_loop::ControlFlow::Poll }
        else { winit::event_loop::ControlFlow::WaitUntil(self.deadline) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use winit::event_loop::ControlFlow;

    #[test]
    fn internal_native_wakes_do_not_repeat_host_work_before_its_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(16);
        let mut schedule = HostSchedule::new(now);
        assert!(schedule.take_tick(now, false));
        schedule.set_deadline(deadline);
        for ms in 1..16 {
            assert!(!schedule.take_tick(now + Duration::from_millis(ms), false));
            assert_eq!(schedule.control_flow(false), ControlFlow::WaitUntil(deadline));
        }
        assert!(schedule.take_tick(deadline, false));
    }

    #[test]
    fn input_and_thread_wakes_interrupt_wait_without_moving_the_frame_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(50);
        let mut schedule = HostSchedule::new(now);
        schedule.take_tick(now, false);
        schedule.set_deadline(deadline);
        schedule.request_tick();
        assert_eq!(schedule.control_flow(false), ControlFlow::Poll);
        assert!(schedule.take_tick(now, false));
        assert!(!schedule.take_tick(now, false));
        assert_eq!(schedule.control_flow(true), ControlFlow::Poll);
        assert!(schedule.take_tick(now, true), "queued input must not wait for another native event");
        assert_eq!(schedule.control_flow(false), ControlFlow::WaitUntil(deadline));
    }
}
