use std::cmp::min;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub struct ExponentialBackoff {
    initial: Duration,
    current: Duration,
    max: Duration,
    multiplier: f64,
}

impl ExponentialBackoff {
    pub fn new(initial: Duration, max: Duration, multiplier: f64) -> Self {
        Self {
            initial,
            current: initial,
            max,
            multiplier,
        }
    }

    pub fn reset(&mut self) {
        self.current = self.initial;
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        let next_ms = (self.current.as_millis() as f64 * self.multiplier).ceil() as u64;
        let next = Duration::from_millis(next_ms.max(self.initial.as_millis() as u64));
        self.current = min(next, self.max);
        delay
    }
}

pub fn wait_or_shutdown(delay: Duration, shutdown: &AtomicBool) -> bool {
    let tick = Duration::from_millis(100);
    let deadline = Instant::now() + delay;

    while Instant::now() < deadline {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(min(remaining, tick));
    }

    !shutdown.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::ExponentialBackoff;
    use std::time::Duration;

    #[test]
    fn backoff_grows_and_caps() {
        let mut backoff = ExponentialBackoff::new(
            Duration::from_millis(300),
            Duration::from_secs(1),
            1.5,
        );

        assert_eq!(backoff.next_delay(), Duration::from_millis(300));
        assert_eq!(backoff.next_delay(), Duration::from_millis(450));
        assert_eq!(backoff.next_delay(), Duration::from_millis(675));
        assert_eq!(backoff.next_delay(), Duration::from_millis(1000));
        assert_eq!(backoff.next_delay(), Duration::from_millis(1000));

        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(300));
    }
}