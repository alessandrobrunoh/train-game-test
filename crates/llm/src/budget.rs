use std::collections::VecDeque;
use std::time::{Duration, Instant};

const HOUR: Duration = Duration::from_secs(3600);

/// At most `max_per_hour` calls in any hour of real time (sliding window):
/// the hard cap on what the AI can cost.
#[derive(Clone, Debug)]
pub struct Budget {
    max_per_hour: u32,
    calls: VecDeque<Instant>,
}

impl Budget {
    pub fn per_hour(max_per_hour: u32) -> Self {
        Self {
            max_per_hour,
            calls: VecDeque::new(),
        }
    }

    /// No limit (tests, replays).
    pub fn unlimited() -> Self {
        Self::per_hour(u32::MAX)
    }

    /// Books one call at `now` if the budget allows it.
    pub fn try_spend(&mut self, now: Instant) -> bool {
        self.forget_before(now);
        if self.calls.len() as u64 >= u64::from(self.max_per_hour) {
            return false;
        }
        self.calls.push_back(now);
        true
    }

    /// Calls still allowed in the hour ending at `now`.
    pub fn remaining(&mut self, now: Instant) -> u32 {
        self.forget_before(now);
        self.max_per_hour
            .saturating_sub(self.calls.len().min(u32::MAX as usize) as u32)
    }

    fn forget_before(&mut self, now: Instant) {
        while let Some(&t) = self.calls.front() {
            if now.duration_since(t) >= HOUR {
                self.calls.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_hour() {
        let t0 = Instant::now();
        let mut b = Budget::per_hour(2);
        assert!(b.try_spend(t0));
        assert!(b.try_spend(t0 + Duration::from_secs(10)));
        assert!(!b.try_spend(t0 + Duration::from_secs(20)));
        assert_eq!(b.remaining(t0 + Duration::from_secs(20)), 0);
        // The first call leaves the window after an hour.
        assert_eq!(b.remaining(t0 + HOUR), 1);
        assert!(b.try_spend(t0 + HOUR));
        assert!(!b.try_spend(t0 + HOUR));
    }

    #[test]
    fn zero_blocks_everything() {
        let mut b = Budget::per_hour(0);
        assert!(!b.try_spend(Instant::now()));
        assert!(Budget::unlimited().try_spend(Instant::now()));
    }
}
