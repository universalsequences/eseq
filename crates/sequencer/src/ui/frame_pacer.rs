use std::time::{Duration, Instant};

/// Presentation deadlines advance independently of render duration. A late
/// frame skips expired slots instead of producing a burst of catch-up frames.
pub(crate) struct FramePacer {
    deadline: Instant,
    interval: Duration,
}

impl FramePacer {
    pub(crate) fn new(now: Instant, interval: Duration) -> Self {
        assert!(!interval.is_zero());
        Self { deadline: now, interval }
    }

    pub(crate) fn time_until_frame(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

    pub(crate) fn is_due(&self, now: Instant) -> bool { now >= self.deadline }

    pub(crate) fn frame_finished(&mut self, now: Instant) {
        if now < self.deadline { return; }
        let late = now.duration_since(self.deadline).as_nanos();
        let remainder = late % self.interval.as_nanos();
        self.deadline = now + self.interval - Duration::from_nanos(remainder as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_does_not_extend_the_interval_and_missed_slots_do_not_burst() {
        let now = Instant::now();
        let ms = Duration::from_millis;
        let mut pacer = FramePacer::new(now, ms(16));
        assert!(pacer.is_due(now));
        pacer.frame_finished(now + ms(3));
        assert_eq!(pacer.time_until_frame(now + ms(3)), ms(13));
        pacer.frame_finished(now + ms(53));
        assert_eq!(pacer.time_until_frame(now + ms(53)), ms(11));
        assert!(!pacer.is_due(now + ms(63)));
        assert!(pacer.is_due(now + ms(64)));
    }
}
