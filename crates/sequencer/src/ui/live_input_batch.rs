use std::time::{Duration, Instant};

/// Drain a burst of musical typing before doing frame work. Ordinary editor
/// events and queued host commands are ordering barriers: their effects must
/// be applied before the next key is interpreted. Both bounds prevent a
/// continuously replenished input queue from starving the rest of the app.
pub(crate) struct LiveInputBatch {
    started: Option<Instant>,
    events: usize,
}

impl LiveInputBatch {
    const MAX_EVENTS: usize = 64;
    const MAX_DURATION: Duration = Duration::from_millis(4);

    pub(crate) fn new() -> Self {
        Self {
            started: None,
            events: 0,
        }
    }

    pub(crate) fn poll_timeout(&self, first_timeout: Duration) -> Duration {
        if self.events == 0 {
            first_timeout
        } else {
            Duration::ZERO
        }
    }

    // Start timing after the first poll, which may legitimately wait for input.
    pub(crate) fn begin_event(&mut self, now: Instant) {
        self.started.get_or_insert(now);
        self.events += 1;
    }

    pub(crate) fn should_drain(
        &self,
        live_key_consumed: bool,
        host_commands_pending: bool,
        now: Instant,
    ) -> bool {
        live_key_consumed
            && !host_commands_pending
            && self.events < Self::MAX_EVENTS
            && self
                .started
                .is_some_and(|start| now.duration_since(start) < Self::MAX_DURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_keys_drain_without_wait_and_ui_commands_end_the_batch() {
        let mut batch = LiveInputBatch::new();
        let timeout = Duration::from_millis(50);
        assert_eq!(batch.poll_timeout(timeout), timeout);
        let now = Instant::now();
        batch.begin_event(now);
        assert!(batch.should_drain(true, false, now));
        assert_eq!(batch.poll_timeout(timeout), Duration::ZERO);
        assert!(!batch.should_drain(false, false, now));
        assert!(!batch.should_drain(true, true, now));
        batch.begin_event(now);
        assert!(batch.should_drain(true, false, now));
    }

    #[test]
    fn a_continuous_live_input_stream_yields_by_count_or_time() {
        let mut batch = LiveInputBatch::new();
        let now = Instant::now();
        batch.begin_event(now);
        assert!(!batch.should_drain(true, false, now + LiveInputBatch::MAX_DURATION));
        for _ in 1..LiveInputBatch::MAX_EVENTS {
            batch.begin_event(now);
        }
        assert!(!batch.should_drain(true, false, now));
    }
}
