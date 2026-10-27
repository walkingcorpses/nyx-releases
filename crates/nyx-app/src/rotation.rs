// rotation.rs - pure scheduling logic for automatic profile rotation (spec 4.3).
//
// No threads, no sleeping, no AppState: the caller owns the clock and passes `Instant`s in, which
// keeps this testable without real waiting. The caller (a background task) asks `Rotator::due`,
// then uses `pick_next` to choose the profile to switch to, and calls `reset` on success or
// `backoff` on failure.

use std::time::{Duration, Instant};

use nyx_core::RotationSettings;
use rand::seq::IndexedRandom;
use rand::Rng;

/// How long to wait before retrying after a failed rotation attempt.
const BACKOFF: Duration = Duration::from_secs(30);

/// Draw a random wait, in whole seconds, uniformly from [min_minutes*60, max_minutes*60].
/// Why defensive: saved settings are validated (1 <= min <= max), but this must never panic, so
/// min 0 is treated as 1 and max below min is treated as min.
pub fn next_interval(min_minutes: u32, max_minutes: u32, rng: &mut impl Rng) -> Duration {
    let min = u64::from(min_minutes.max(1)) * 60;
    let max = (u64::from(max_minutes) * 60).max(min);
    Duration::from_secs(rng.random_range(min..=max))
}

/// Pick a random pool entry that differs from `current`; `None` when there is no such entry.
/// Why: rotating to the same profile would be a no-op. Duplicate pool entries are collapsed so
/// they do not skew the odds, and `current` need not be in the pool.
pub fn pick_next(pool: &[String], current: &str, rng: &mut impl Rng) -> Option<String> {
    let mut others: Vec<&String> = Vec::new();
    for p in pool {
        if p != current && !others.contains(&p) {
            others.push(p);
        }
    }
    others.choose(rng).map(|s| (*s).clone())
}

/// Tracks when the next rotation is due.
pub struct Rotator {
    next_due: Instant,
}

impl Rotator {
    /// Start the timer: the first rotation is one random interval after `now`, never immediately.
    pub fn new(now: Instant, settings: &RotationSettings, rng: &mut impl Rng) -> Rotator {
        Rotator { next_due: now + next_interval(settings.min_minutes, settings.max_minutes, rng) }
    }

    /// True once `now` has reached the scheduled time.
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next_due
    }

    /// Schedule a fresh random interval from `now` (after a successful rotation or a settings change).
    pub fn reset(&mut self, now: Instant, settings: &RotationSettings, rng: &mut impl Rng) {
        self.next_due = now + next_interval(settings.min_minutes, settings.max_minutes, rng);
    }

    /// Retry soon after a failed attempt: next try exactly BACKOFF (30 s) after `now`.
    pub fn backoff(&mut self, now: Instant) {
        self.next_due = now + BACKOFF;
    }

    /// When the next rotation is scheduled, so the UI can show a countdown.
    pub fn next_due(&self) -> Instant {
        self.next_due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::collections::HashSet;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn pool(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn settings(min: u32, max: u32) -> RotationSettings {
        RotationSettings { enabled: true, pool: vec![], min_minutes: min, max_minutes: max }
    }

    #[test]
    fn interval_stays_within_min_and_max_over_1000_draws() {
        let mut r = rng();
        for _ in 0..1000 {
            let d = next_interval(5, 10, &mut r);
            assert!(d >= Duration::from_secs(300) && d <= Duration::from_secs(600), "{d:?}");
            assert_eq!(d.subsec_nanos(), 0, "whole seconds only");
        }
    }

    #[test]
    fn min_equal_max_gives_exact_interval() {
        let mut r = rng();
        for _ in 0..50 {
            assert_eq!(next_interval(7, 7, &mut r), Duration::from_secs(420));
        }
    }

    #[test]
    fn interval_clamps_bad_bounds_without_panicking() {
        let mut r = rng();
        // min 0 -> 1 minute; max < min -> min.
        assert_eq!(next_interval(0, 0, &mut r), Duration::from_secs(60));
        assert_eq!(next_interval(10, 5, &mut r), Duration::from_secs(600));
    }

    #[test]
    fn pick_next_never_returns_current_over_1000_draws() {
        let p = pool(&["a", "b", "c"]);
        let mut r = rng();
        for _ in 0..1000 {
            let n = pick_next(&p, "b", &mut r).unwrap();
            assert_ne!(n, "b");
            assert!(p.contains(&n));
        }
    }

    #[test]
    fn pick_next_with_single_entry_is_none() {
        let mut r = rng();
        assert_eq!(pick_next(&pool(&["a"]), "a", &mut r), None);
        // Duplicates of current count as no other entry; empty pool likewise.
        assert_eq!(pick_next(&pool(&["a", "a"]), "a", &mut r), None);
        assert_eq!(pick_next(&[], "a", &mut r), None);
    }

    #[test]
    fn pick_next_with_current_not_in_pool_still_picks() {
        let mut r = rng();
        let n = pick_next(&pool(&["a"]), "zzz", &mut r);
        assert_eq!(n.as_deref(), Some("a"));
    }

    #[test]
    fn due_is_false_before_and_true_after_interval() {
        let mut r = rng();
        let t0 = Instant::now();
        let rot = Rotator::new(t0, &settings(5, 5), &mut r);
        assert_eq!(rot.next_due(), t0 + Duration::from_secs(300));
        assert!(!rot.due(t0), "never fires at start");
        assert!(!rot.due(t0 + Duration::from_secs(299)));
        assert!(rot.due(t0 + Duration::from_secs(300)));
        assert!(rot.due(t0 + Duration::from_secs(301)));
    }

    #[test]
    fn reset_reschedules_from_now() {
        let mut r = rng();
        let t0 = Instant::now();
        let mut rot = Rotator::new(t0, &settings(5, 5), &mut r);
        let t1 = t0 + Duration::from_secs(400);
        rot.reset(t1, &settings(1, 1), &mut r);
        assert!(!rot.due(t1 + Duration::from_secs(59)));
        assert!(rot.due(t1 + Duration::from_secs(60)));
    }

    #[test]
    fn backoff_retries_after_30_seconds() {
        let mut r = rng();
        let t0 = Instant::now();
        let mut rot = Rotator::new(t0, &settings(5, 5), &mut r);
        let t1 = t0 + Duration::from_secs(300);
        rot.backoff(t1);
        assert!(!rot.due(t1 + Duration::from_secs(29)));
        assert!(rot.due(t1 + Duration::from_secs(30)));
    }

    #[test]
    fn pick_next_eventually_visits_every_other_entry() {
        let p = pool(&["a", "b", "c", "d", "e"]);
        let mut r = rng();
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let n = pick_next(&p, "c", &mut r).unwrap();
            assert_ne!(n, "c");
            seen.insert(n);
        }
        let want: HashSet<String> = ["a", "b", "d", "e"].iter().map(|s| s.to_string()).collect();
        assert_eq!(seen, want);
    }
}
