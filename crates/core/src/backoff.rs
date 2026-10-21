//! Retry waits: 30 s, 1 min, 2 min, 4 min, 8 min, then 15 min forever, ±10 % jitter
//! (docs/07, "Backoff schedule"). Retries never stop on their own; only the user cancels.

use std::time::Duration;

const SCHEDULE_SECS: [u64; 6] = [30, 60, 120, 240, 480, 900];

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Backoff {
    /// The real schedule.
    #[default]
    Schedule,
    /// Always the same wait, without jitter (tests).
    Fixed(Duration),
}

impl Backoff {
    /// Wait before retry number `attempt` (1-based).
    pub fn delay(self, attempt: u32) -> Duration {
        match self {
            Self::Fixed(d) => d,
            Self::Schedule => {
                let i = (attempt.max(1) as usize - 1).min(SCHEDULE_SECS.len() - 1);
                let base = SCHEDULE_SECS[i] as f64;
                Duration::from_secs_f64(base * (1.0 + jitter()))
            }
        }
    }
}

/// Uniform in [-0.1, 0.1].
fn jitter() -> f64 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        return 0.0;
    }
    let unit = u32::from_le_bytes(b) as f64 / u32::MAX as f64;
    (unit - 0.5) / 5.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_with_jitter_and_cap() {
        let within = |attempt: u32, base: f64| {
            let d = Backoff::Schedule.delay(attempt).as_secs_f64();
            assert!(
                (base * 0.9..=base * 1.1).contains(&d),
                "attempt {attempt}: {d}s not within 10% of {base}s"
            );
        };
        within(1, 30.0);
        within(2, 60.0);
        within(3, 120.0);
        within(6, 900.0);
        within(50, 900.0);
        within(0, 30.0);
        assert_eq!(
            Backoff::Fixed(Duration::from_millis(5)).delay(9),
            Duration::from_millis(5)
        );
    }
}
