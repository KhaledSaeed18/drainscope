//! Side-channel mitigations: per-caller rate limiting and quantization (PLAN.md §5).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Callers remembered before stale entries are pruned. Unique bus names are never reused,
/// so without pruning the map would grow with every client that ever connected.
const MAX_TRACKED_CALLERS: usize = 1024;

#[derive(Debug)]
pub struct RateLimiter {
    min_interval: Duration,
    last_call: HashMap<String, Instant>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_call: HashMap::new(),
        }
    }

    /// Records a call from `caller` at `now`.
    ///
    /// # Errors
    /// How long the caller must still wait, if it called within the minimum interval.
    pub fn check(&mut self, caller: &str, now: Instant) -> Result<(), Duration> {
        if let Some(previous) = self.last_call.get(caller) {
            let wait = self
                .min_interval
                .saturating_sub(now.saturating_duration_since(*previous));
            if !wait.is_zero() {
                return Err(wait);
            }
        }
        if self.last_call.len() >= MAX_TRACKED_CALLERS {
            let min_interval = self.min_interval;
            self.last_call
                .retain(|_, at| now.saturating_duration_since(*at) < min_interval);
        }
        self.last_call.insert(caller.to_owned(), now);
        Ok(())
    }
}

/// Rounds `value` down to a multiple of `quantum`.
#[must_use]
pub fn quantize(value: u64, quantum: u64) -> u64 {
    if quantum == 0 {
        value
    } else {
        value - value % quantum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_caller_separately() {
        let mut limiter = RateLimiter::new(Duration::from_millis(1000));
        let start = Instant::now();
        assert_eq!(limiter.check(":1.5", start), Ok(()));
        assert_eq!(limiter.check(":1.6", start), Ok(()));
        assert_eq!(
            limiter.check(":1.5", start + Duration::from_millis(400)),
            Err(Duration::from_millis(600))
        );
        assert_eq!(
            limiter.check(":1.5", start + Duration::from_millis(1000)),
            Ok(())
        );
    }

    #[test]
    fn prunes_stale_callers() {
        let mut limiter = RateLimiter::new(Duration::from_millis(10));
        let start = Instant::now();
        for i in 0..MAX_TRACKED_CALLERS {
            limiter.check(&format!(":1.{i}"), start).unwrap();
        }
        let later = start + Duration::from_secs(1);
        limiter.check(":2.0", later).unwrap();
        assert_eq!(limiter.last_call.len(), 1);
    }

    #[test]
    fn quantizes_down() {
        assert_eq!(quantize(1_234_567, 10_000), 1_230_000);
        assert_eq!(quantize(9_999, 10_000), 0);
        assert_eq!(quantize(42, 0), 42);
    }
}
