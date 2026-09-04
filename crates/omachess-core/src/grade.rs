//! Turning a solve attempt into a scheduling decision.
//!
//! FSRS models *retention* — how long a memory lasts. It has no notion of how
//! quickly an answer is produced. The improvement marker this application is
//! built around is *fluency*: re-solving the same puzzle in progressively less
//! time. The two are tracked separately and joined here, at the single point
//! where a latency is converted into an FSRS rating.

use chrono::Duration;
use rs_fsrs::Rating;

/// Solved in at most this fraction of the personal baseline: fluent recall.
pub const FAST_RATIO: f64 = 0.6;
/// Solved in at most this fraction of the baseline: on pace.
pub const SLOW_RATIO: f64 = 1.25;

/// Elo K-factor for the personal puzzle rating.
pub const K_FACTOR: f64 = 32.0;
/// The user rates 1100 on Lichess; material below that is out of scope, so the
/// personal rating is never allowed to drift beneath it.
pub const RATING_FLOOR: u32 = 1100;
/// Rating bands are 100 points wide.
pub const BAND_WIDTH: u32 = 100;
/// Number of recorded solves needed before a measured baseline replaces the seed.
pub const MIN_BASELINE_SAMPLES: usize = 5;

/// How a solve time compared with the personal baseline for its rating band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    Fast,
    OnPace,
    Slow,
}

/// Compare a solve time against the personal baseline for its band.
pub fn speed(elapsed: Duration, baseline: Duration) -> Speed {
    let baseline_ms = baseline.num_milliseconds();
    if baseline_ms <= 0 {
        return Speed::OnPace;
    }
    let ratio = elapsed.num_milliseconds() as f64 / baseline_ms as f64;
    if ratio <= FAST_RATIO {
        Speed::Fast
    } else if ratio <= SLOW_RATIO {
        Speed::OnPace
    } else {
        Speed::Slow
    }
}

/// Derive the FSRS rating for an attempt.
///
/// A failed solve is always `Again`, regardless of how fast it was reached —
/// speed only modulates the interval for answers that were actually correct.
pub fn grade(correct: bool, elapsed: Duration, baseline: Duration) -> Rating {
    if !correct {
        return Rating::Again;
    }
    match speed(elapsed, baseline) {
        Speed::Fast => Rating::Easy,
        Speed::OnPace => Rating::Good,
        Speed::Slow => Rating::Hard,
    }
}

/// The 100-point band a rating falls into, used to group baselines so that a
/// slow solve of a hard puzzle is not mistaken for a loss of fluency.
pub fn band(rating: u32) -> u32 {
    rating / BAND_WIDTH * BAND_WIDTH
}

/// Median of the supplied solve times, or `None` if there is not yet enough
/// evidence to prefer it over the seeded default.
pub fn baseline_from(samples: &[Duration]) -> Option<Duration> {
    if samples.len() < MIN_BASELINE_SAMPLES {
        return None;
    }
    let mut ms: Vec<i64> = samples.iter().map(Duration::num_milliseconds).collect();
    ms.sort_unstable();
    let mid = ms.len() / 2;
    let median = if ms.len().is_multiple_of(2) {
        (ms[mid - 1] + ms[mid]) / 2
    } else {
        ms[mid]
    };
    Some(Duration::milliseconds(median))
}

/// Starting baseline for a band, used only until `MIN_BASELINE_SAMPLES` real
/// solves have been recorded for it. Harder puzzles are allowed more time.
pub fn seed_baseline(band: u32) -> Duration {
    let steps = i64::from(band.saturating_sub(RATING_FLOOR) / BAND_WIDTH);
    Duration::seconds(15 + steps * 5)
}

/// Personal puzzle rating after an attempt, using a plain Elo update against
/// the puzzle's own published rating.
pub fn next_rating(user: f64, puzzle: f64, solved: bool) -> f64 {
    let expected = 1.0 / (1.0 + 10f64.powf((puzzle - user) / 400.0));
    let actual = if solved { 1.0 } else { 0.0 };
    let updated = user + K_FACTOR * (actual - expected);
    updated.max(f64::from(RATING_FLOOR))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: i64) -> Duration {
        Duration::seconds(n)
    }

    #[test]
    fn wrong_answers_are_always_again_however_fast() {
        assert_eq!(grade(false, secs(1), secs(20)), Rating::Again);
        assert_eq!(grade(false, secs(300), secs(20)), Rating::Again);
    }

    #[test]
    fn speed_relative_to_baseline_sets_the_rating() {
        let baseline = secs(20);
        assert_eq!(grade(true, secs(5), baseline), Rating::Easy);
        assert_eq!(grade(true, secs(20), baseline), Rating::Good);
        assert_eq!(grade(true, secs(60), baseline), Rating::Hard);
    }

    #[test]
    fn ratio_boundaries_are_inclusive() {
        let baseline = secs(100);
        assert_eq!(speed(secs(60), baseline), Speed::Fast);
        assert_eq!(speed(secs(61), baseline), Speed::OnPace);
        assert_eq!(speed(secs(125), baseline), Speed::OnPace);
        assert_eq!(speed(secs(126), baseline), Speed::Slow);
    }

    #[test]
    fn a_zero_baseline_cannot_divide_by_zero() {
        assert_eq!(speed(secs(5), Duration::zero()), Speed::OnPace);
    }

    #[test]
    fn baseline_needs_enough_evidence() {
        let few: Vec<Duration> = (0..MIN_BASELINE_SAMPLES - 1).map(|_| secs(10)).collect();
        assert!(baseline_from(&few).is_none());
    }

    #[test]
    fn baseline_is_the_median_not_the_mean() {
        // One 10-minute distraction must not move the baseline.
        let samples = [secs(10), secs(12), secs(11), secs(13), secs(600)];
        assert_eq!(baseline_from(&samples), Some(secs(12)));
    }

    #[test]
    fn bands_group_by_hundreds() {
        assert_eq!(band(1100), 1100);
        assert_eq!(band(1199), 1100);
        assert_eq!(band(1200), 1200);
    }

    #[test]
    fn seed_baseline_grows_with_difficulty() {
        assert!(seed_baseline(1400) > seed_baseline(1100));
    }

    #[test]
    fn solving_a_harder_puzzle_gains_more_than_an_easier_one() {
        let hard = next_rating(1100.0, 1500.0, true);
        let easy = next_rating(1100.0, 900.0, true);
        assert!(hard > easy);
        assert!(hard > 1100.0 && easy > 1100.0);
    }

    #[test]
    fn rating_never_falls_below_the_floor() {
        let mut r = f64::from(RATING_FLOOR);
        for _ in 0..50 {
            r = next_rating(r, 2000.0, false);
        }
        assert_eq!(r, f64::from(RATING_FLOOR));
    }

    #[test]
    fn a_streak_of_solves_raises_the_rating_toward_the_puzzle_level() {
        let mut r = f64::from(RATING_FLOOR);
        for _ in 0..30 {
            r = next_rating(r, 1400.0, true);
        }
        assert!(r > 1300.0, "expected convergence toward 1400, got {r}");
    }
}
