//! Measuring whether the solver has actually improved.
//!
//! The only defensible comparison is a puzzle against itself. Comparing early
//! solve times with later ones across *different* puzzles measures the puzzles
//! as much as the person: a run of easy forks looks like progress and a run of
//! hard endgames looks like decline.
//!
//! So every figure here is paired — each puzzle's most recent correct solve
//! against its own first — and reported with the number of puzzles behind it
//! and the probability of seeing that result by chance.

use anyhow::Result;
use chrono::Duration;

use crate::store::{FirstAttempt, GameRecord, PairedSolve, Store};

/// Attempts needed before the rating trajectory is worth drawing. Below this
/// it is a handful of coin flips with a line through them.
pub const MIN_RATING_POINTS: usize = 30;

/// First encounters needed in a band before its transfer figure is reported.
/// Each half must be big enough for the comparison to mean anything.
pub const MIN_TRANSFER: usize = 12;

/// Repeated puzzles required before any claim is made at all.
pub const MIN_PAIRS: usize = 5;

/// A result worth acting on. Chosen as the conventional threshold rather than
/// anything special about this domain.
pub const SIGNIFICANT: f64 = 0.05;

/// What the repeated puzzles actually show.
#[derive(Debug, Clone, PartialEq)]
pub struct Improvement {
    /// Puzzles solved correctly at least twice — the sample size.
    pub puzzles: usize,
    /// Median of each puzzle's first-solve ÷ latest-solve. Above one is faster.
    pub median_speedup: f64,
    pub faster: usize,
    pub slower: usize,
    pub unchanged: usize,
    pub median_first: Duration,
    pub median_latest: Duration,
    /// One-sided sign-test probability of this many puzzles improving, or more,
    /// if solve times were really unchanged. Small means the result is unlikely
    /// to be noise.
    pub p_value: f64,
}

impl Improvement {
    pub fn is_significant(&self) -> bool {
        self.p_value <= SIGNIFICANT
    }
}

/// Improvement across every repeated puzzle, or `None` when too few have been
/// repeated to say anything at all.
pub fn measured_improvement(store: &Store) -> Result<Option<Improvement>> {
    Ok(summarise(&store.paired_solves()?))
}

/// The same, split by rating band.
pub fn improvement_by_band(store: &Store) -> Result<Vec<(u32, Improvement)>> {
    let pairs = store.paired_solves()?;
    let mut bands: Vec<u32> = pairs.iter().map(|p| p.band).collect();
    bands.sort_unstable();
    bands.dedup();

    Ok(bands
        .into_iter()
        .filter_map(|band| {
            let subset: Vec<PairedSolve> = pairs
                .iter()
                .filter(|p| p.band == band)
                .cloned()
                .collect();
            summarise(&subset).map(|improvement| (band, improvement))
        })
        .collect())
}

fn summarise(pairs: &[PairedSolve]) -> Option<Improvement> {
    if pairs.len() < MIN_PAIRS {
        return None;
    }

    let mut speedups: Vec<f64> = pairs.iter().map(PairedSolve::speedup).collect();
    speedups.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let faster = pairs.iter().filter(|p| p.latest < p.first).count();
    let slower = pairs.iter().filter(|p| p.latest > p.first).count();
    let unchanged = pairs.len() - faster - slower;

    Some(Improvement {
        puzzles: pairs.len(),
        median_speedup: median_f64(&speedups),
        faster,
        slower,
        unchanged,
        median_first: median_duration(pairs.iter().map(|p| p.first)),
        median_latest: median_duration(pairs.iter().map(|p| p.latest)),
        // Ties carry no information about direction, so they are excluded from
        // the test rather than counted as evidence either way.
        p_value: sign_test(faster, faster + slower),
    })
}

/// One-sided binomial sign test: the chance of `successes` or more out of
/// `trials`, if each were a coin flip.
///
/// Computed in log space so large samples neither overflow nor underflow.
pub fn sign_test(successes: usize, trials: usize) -> f64 {
    if trials == 0 {
        return 1.0;
    }
    if successes > trials {
        return 0.0;
    }
    let n = trials as f64;
    let log_half_pow = n * 0.5f64.ln();

    let mut total = 0.0;
    for k in successes..=trials {
        total += (log_choose(trials, k) + log_half_pow).exp();
    }
    total.clamp(0.0, 1.0)
}

/// `ln(n choose k)`, accumulated as a product of ratios to stay in range.
fn log_choose(n: usize, k: usize) -> f64 {
    let k = k.min(n - k);
    (1..=k).fold(0.0, |acc, j| {
        acc + (((n - k + j) as f64) / (j as f64)).ln()
    })
}

fn median_f64(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        return 1.0;
    }
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn median_duration(values: impl Iterator<Item = Duration>) -> Duration {
    let mut ms: Vec<i64> = values.map(|d| d.num_milliseconds()).collect();
    ms.sort_unstable();
    if ms.is_empty() {
        return Duration::zero();
    }
    let mid = ms.len() / 2;
    let value = if ms.len().is_multiple_of(2) {
        (ms[mid - 1] + ms[mid]) / 2
    } else {
        ms[mid]
    };
    Duration::milliseconds(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(id: &str, first_s: i64, latest_s: i64) -> PairedSolve {
        PairedSolve {
            puzzle_id: id.into(),
            band: 1100,
            first: Duration::seconds(first_s),
            latest: Duration::seconds(latest_s),
            solves: 2,
        }
    }

    #[test]
    fn nothing_is_claimed_from_too_few_puzzles() {
        let pairs: Vec<_> = (0..MIN_PAIRS - 1)
            .map(|i| pair(&format!("p{i}"), 40, 10))
            .collect();
        assert!(summarise(&pairs).is_none(), "claimed a result from {} puzzles", pairs.len());
    }

    #[test]
    fn consistent_speedups_are_detected_and_are_not_chance() {
        let pairs: Vec<_> = (0..10).map(|i| pair(&format!("p{i}"), 40, 20)).collect();
        let result = summarise(&pairs).expect("a result");
        assert_eq!(result.puzzles, 10);
        assert_eq!(result.faster, 10);
        assert!((result.median_speedup - 2.0).abs() < 1e-9);
        assert!(result.is_significant(), "p was {}", result.p_value);
    }

    #[test]
    fn getting_slower_is_never_dressed_up_as_progress() {
        let pairs: Vec<_> = (0..10).map(|i| pair(&format!("p{i}"), 20, 40)).collect();
        let result = summarise(&pairs).expect("a result");
        assert_eq!(result.faster, 0);
        assert!(result.median_speedup < 1.0);
        assert!(!result.is_significant(), "a decline must not read as significant");
    }

    #[test]
    fn a_coin_flip_split_is_not_called_improvement() {
        // Five faster, five slower: exactly what noise looks like.
        let mut pairs: Vec<_> = (0..5).map(|i| pair(&format!("f{i}"), 30, 20)).collect();
        pairs.extend((0..5).map(|i| pair(&format!("s{i}"), 20, 30)));
        let result = summarise(&pairs).expect("a result");
        assert_eq!((result.faster, result.slower), (5, 5));
        assert!(
            !result.is_significant(),
            "an even split must not be significant, p was {}",
            result.p_value
        );
    }

    #[test]
    fn one_lucky_puzzle_does_not_carry_the_result() {
        // A single huge speedup among otherwise unchanged puzzles.
        let mut pairs: Vec<_> = (0..9).map(|i| pair(&format!("p{i}"), 30, 30)).collect();
        pairs.push(pair("lucky", 300, 3));
        let result = summarise(&pairs).expect("a result");
        assert!(
            (result.median_speedup - 1.0).abs() < 1e-9,
            "the median must ignore the outlier, got {}",
            result.median_speedup
        );
        assert!(!result.is_significant());
    }

    #[test]
    fn ties_are_excluded_from_the_test_rather_than_counted() {
        let mut pairs: Vec<_> = (0..6).map(|i| pair(&format!("f{i}"), 30, 20)).collect();
        pairs.extend((0..6).map(|i| pair(&format!("t{i}"), 25, 25)));
        let result = summarise(&pairs).expect("a result");
        assert_eq!(result.unchanged, 6);
        // Six of six directional puzzles improved, so p = 0.5^6.
        assert!((result.p_value - 0.5f64.powi(6)).abs() < 1e-9, "p was {}", result.p_value);
    }

    #[test]
    fn the_sign_test_matches_known_values() {
        assert!((sign_test(0, 0) - 1.0).abs() < 1e-12);
        // All heads out of five: 1/32.
        assert!((sign_test(5, 5) - 0.031_25).abs() < 1e-12);
        // Every outcome is at least as extreme as zero successes.
        assert!((sign_test(0, 5) - 1.0).abs() < 1e-12);
        // Symmetry around the middle.
        assert!((sign_test(3, 6) - 0.656_25).abs() < 1e-9);
    }

    #[test]
    fn large_samples_do_not_overflow() {
        let p = sign_test(300, 500);
        assert!(p.is_finite() && (0.0..=1.0).contains(&p), "p was {p}");
        assert!(p < 0.001, "300 of 500 should be clearly significant, got {p}");
    }
}

/// Games needed before any trend across games is reported. Each half must be
/// large enough for the normal approximation below to mean anything.
pub const MIN_GAMES: usize = 10;

/// How play against the engine has changed, judged on quality rather than
/// result.
///
/// Wins and losses depend on how hard the opponent was set; the win probability
/// given away per move does not, and the opponent here is pinned near the
/// player's own rating, which makes games comparable to each other.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayTrend {
    pub games: usize,
    pub earlier_accuracy: f64,
    pub recent_accuracy: f64,
    pub earlier_loss: f64,
    pub recent_loss: f64,
    pub earlier_blunders_per_100: f64,
    pub recent_blunders_per_100: f64,
    /// One-sided Mann-Whitney probability that the recent half is no better.
    pub p_value: f64,
}

impl PlayTrend {
    pub fn is_significant(&self) -> bool {
        self.p_value <= SIGNIFICANT
    }
}

/// Compare the most recent half of games against the earlier half.
pub fn play_trend(store: &Store) -> Result<Option<PlayTrend>> {
    Ok(summarise_games(&store.games_mine()?))
}

fn summarise_games(games: &[GameRecord]) -> Option<PlayTrend> {
    if games.len() < MIN_GAMES {
        return None;
    }
    let split = games.len() / 2;
    let (earlier, recent) = games.split_at(split);

    let accuracy = |set: &[GameRecord]| -> Vec<f64> { set.iter().map(|g| g.accuracy).collect() };
    let blunder_rate = |set: &[GameRecord]| -> f64 {
        let moves: u32 = set.iter().map(|g| g.moves).sum();
        if moves == 0 {
            return 0.0;
        }
        let blunders: u32 = set.iter().map(|g| g.blunders).sum();
        f64::from(blunders) * 100.0 / f64::from(moves)
    };

    let earlier_acc = accuracy(earlier);
    let recent_acc = accuracy(recent);

    Some(PlayTrend {
        games: games.len(),
        earlier_accuracy: median_of(&earlier_acc),
        recent_accuracy: median_of(&recent_acc),
        earlier_loss: median_of(&earlier.iter().map(|g| g.mean_loss).collect::<Vec<_>>()),
        recent_loss: median_of(&recent.iter().map(|g| g.mean_loss).collect::<Vec<_>>()),
        earlier_blunders_per_100: blunder_rate(earlier),
        recent_blunders_per_100: blunder_rate(recent),
        p_value: mann_whitney_greater(&recent_acc, &earlier_acc),
    })
}

/// One-sided Mann-Whitney U test: the probability that `sample` is not drawn
/// from a distribution shifted above `baseline`.
///
/// Uses the normal approximation with a continuity correction, which is why a
/// minimum sample size is enforced before it is reported at all.
pub fn mann_whitney_greater(sample: &[f64], baseline: &[f64]) -> f64 {
    let (n1, n2) = (sample.len(), baseline.len());
    if n1 == 0 || n2 == 0 {
        return 1.0;
    }
    let mut u = 0.0;
    for a in sample {
        for b in baseline {
            u += match a.partial_cmp(b) {
                Some(std::cmp::Ordering::Greater) => 1.0,
                Some(std::cmp::Ordering::Equal) => 0.5,
                _ => 0.0,
            };
        }
    }
    let mean = (n1 * n2) as f64 / 2.0;
    let sd = (((n1 * n2 * (n1 + n2 + 1)) as f64) / 12.0).sqrt();
    if sd <= 0.0 {
        return 1.0;
    }
    let z = (u - mean - 0.5) / sd;
    1.0 - normal_cdf(z)
}

/// Standard normal CDF, via the Abramowitz and Stegun error-function
/// approximation (accurate to about 1.5e-7).
fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    median_f64(&sorted)
}

#[cfg(test)]
mod game_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn game(index: i64, accuracy: f64, blunders: u32) -> GameRecord {
        GameRecord {
            played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
                + chrono::Duration::days(index),
            player_white: true,
            opponent_elo: 1320,
            result: "lost".into(),
            moves: 40,
            accuracy,
            mean_loss: (100.0 - accuracy) / 1000.0,
            blunders,
            mistakes: 0,
            inaccuracies: 0,
            source: String::new(),
            phases: [crate::store::PhaseLoss::UNKNOWN; 3],
            player: String::new(),
        }
    }

    #[test]
    fn nothing_is_claimed_from_too_few_games() {
        let games: Vec<_> = (0..MIN_GAMES as i64 - 1).map(|i| game(i, 70.0, 1)).collect();
        assert!(summarise_games(&games).is_none());
    }

    #[test]
    fn steady_improvement_across_games_is_detected() {
        let mut games: Vec<_> = (0..6).map(|i| game(i, 60.0 + i as f64, 3)).collect();
        games.extend((6..12).map(|i| game(i, 85.0 + (i - 6) as f64, 0)));
        let trend = summarise_games(&games).expect("a trend");
        assert!(trend.recent_accuracy > trend.earlier_accuracy);
        assert!(trend.recent_blunders_per_100 < trend.earlier_blunders_per_100);
        assert!(trend.is_significant(), "p was {}", trend.p_value);
    }

    #[test]
    fn noise_is_not_reported_as_improvement() {
        // Alternating scores with no direction.
        let games: Vec<_> = (0..12)
            .map(|i| game(i, if i % 2 == 0 { 70.0 } else { 75.0 }, 1))
            .collect();
        let trend = summarise_games(&games).expect("a trend");
        assert!(
            !trend.is_significant(),
            "alternating noise reported as progress, p = {}",
            trend.p_value
        );
    }

    #[test]
    fn getting_worse_is_never_significant_improvement() {
        let mut games: Vec<_> = (0..6).map(|i| game(i, 90.0, 0)).collect();
        games.extend((6..12).map(|i| game(i, 55.0, 4)));
        let trend = summarise_games(&games).expect("a trend");
        assert!(trend.recent_accuracy < trend.earlier_accuracy);
        assert!(!trend.is_significant());
    }

    #[test]
    fn the_normal_cdf_matches_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-6);
        assert!((normal_cdf(1.96) - 0.975).abs() < 1e-3);
        assert!((normal_cdf(-1.96) - 0.025).abs() < 1e-3);
    }

    #[test]
    fn identical_samples_are_not_a_difference() {
        let a = vec![70.0, 72.0, 74.0, 76.0, 78.0];
        assert!(mann_whitney_greater(&a, &a) > 0.3, "identical samples looked different");
    }
}

// ---------------------------------------------------------------------------
// Series for the charts.
//
// The shapes below are prepared here rather than in the view so that what the
// charts claim can be tested. A chart that draws the wrong thing convincingly
// is worse than no chart.
// ---------------------------------------------------------------------------

/// One repeated puzzle, as a line from its first solve to its latest.
///
/// This is the honest picture of improvement: every line that falls is a puzzle
/// solved faster than the same person solved the same puzzle before.
#[derive(Debug, Clone, PartialEq)]
pub struct SlopePoint {
    pub puzzle_id: String,
    pub band: u32,
    pub first_seconds: f64,
    pub latest_seconds: f64,
    pub solves: u32,
}

impl SlopePoint {
    pub fn improved(&self) -> bool {
        self.latest_seconds < self.first_seconds
    }
}

/// Every puzzle solved correctly more than once, slowest first solve first.
pub fn slope_points(store: &Store) -> Result<Vec<SlopePoint>> {
    let mut points: Vec<SlopePoint> = store
        .paired_solves()?
        .into_iter()
        .map(|p| SlopePoint {
            puzzle_id: p.puzzle_id,
            band: p.band,
            first_seconds: p.first.num_milliseconds() as f64 / 1000.0,
            latest_seconds: p.latest.num_milliseconds() as f64 / 1000.0,
            solves: p.solves,
        })
        .collect();
    points.sort_by(|a, b| {
        b.first_seconds
            .partial_cmp(&a.first_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(points)
}

/// The personal rating after each attempt, reconstructed by replaying the same
/// Elo update the trainer applies.
///
/// Nothing extra is stored for this: the attempt log already holds every
/// puzzle's rating and whether it was solved, so the trajectory is derived
/// rather than remembered, and cannot drift from what actually happened.
pub fn rating_history(store: &Store) -> Result<Vec<(chrono::DateTime<chrono::Utc>, f64)>> {
    let mut rating = f64::from(crate::grade::RATING_FLOOR);
    Ok(store
        .attempt_log()?
        .into_iter()
        .map(|(at, puzzle_rating, correct)| {
            rating = crate::grade::next_rating(rating, f64::from(puzzle_rating), correct);
            (at, rating)
        })
        .collect())
}

/// One finished game, for the accuracy series.
#[derive(Debug, Clone, PartialEq)]
pub struct GamePoint {
    pub played_at: chrono::DateTime<chrono::Utc>,
    pub accuracy: f64,
    pub blunders_per_100: f64,
    pub result: String,
}

pub fn game_points(store: &Store) -> Result<Vec<GamePoint>> {
    Ok(store
        .games_mine()?
        .into_iter()
        .map(|g| GamePoint {
            played_at: g.played_at,
            accuracy: g.accuracy,
            blunders_per_100: if g.moves == 0 {
                0.0
            } else {
                f64::from(g.blunders) * 100.0 / f64::from(g.moves)
            },
            result: g.result,
        })
        .collect())
}

#[cfg(test)]
mod series_tests {
    use super::*;
    use crate::grade::RATING_FLOOR;
    use crate::store::AttemptRecord;
    use chrono::{Duration, TimeZone, Utc};
    use rs_fsrs::Rating;

    fn store_with(attempts: &[(&str, i64, bool, u32)]) -> Store {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        for (i, (id, secs, correct, rating)) in attempts.iter().enumerate() {
            store
                .record_attempt(&AttemptRecord {
                    puzzle_id: (*id).to_owned(),
                    // Spaced past the repeat threshold, so these exercise the
                    // series shapes rather than the interval rule.
                    reviewed_at: base + Duration::hours(i as i64 * 25),
                    elapsed: Duration::seconds(*secs),
                    correct: *correct,
                    grade: if *correct { Rating::Good } else { Rating::Again },
                    puzzle_rating: *rating,
                })
                .unwrap();
        }
        store
    }

    #[test]
    fn only_repeated_puzzles_become_slope_lines() {
        let store = store_with(&[
            ("a", 40, true, 1150),
            ("b", 30, true, 1150),
            ("a", 20, true, 1150),
        ]);
        let points = slope_points(&store).unwrap();
        assert_eq!(points.len(), 1, "a puzzle solved once is not a line");
        assert_eq!(points[0].puzzle_id, "a");
        assert_eq!(points[0].first_seconds, 40.0);
        assert_eq!(points[0].latest_seconds, 20.0);
        assert!(points[0].improved());
    }

    #[test]
    fn a_slower_repeat_is_not_marked_as_improvement() {
        let store = store_with(&[("a", 20, true, 1150), ("a", 45, true, 1150)]);
        let points = slope_points(&store).unwrap();
        assert!(!points[0].improved());
    }

    #[test]
    fn failed_repeats_are_excluded_from_the_slope() {
        let store = store_with(&[("a", 40, true, 1150), ("a", 2, false, 1150)]);
        assert!(
            slope_points(&store).unwrap().is_empty(),
            "a fast wrong answer must not look like a fast solve"
        );
    }

    #[test]
    fn slope_lines_are_ordered_by_the_original_solve_time() {
        let store = store_with(&[
            ("a", 10, true, 1150),
            ("b", 60, true, 1150),
            ("a", 5, true, 1150),
            ("b", 30, true, 1150),
        ]);
        let points = slope_points(&store).unwrap();
        assert_eq!(points[0].puzzle_id, "b", "slowest first solve should lead");
    }

    #[test]
    fn the_rating_history_starts_at_the_floor_and_follows_the_attempts() {
        let store = store_with(&[
            ("a", 20, true, 1400),
            ("b", 20, true, 1400),
            ("c", 20, true, 1400),
        ]);
        let history = rating_history(&store).unwrap();
        assert_eq!(history.len(), 3);
        assert!(history[0].1 > f64::from(RATING_FLOOR), "a solve should raise it");
        assert!(history[2].1 > history[0].1, "a streak should keep raising it");
    }

    #[test]
    fn the_rating_history_never_dips_below_the_floor() {
        let losses: Vec<(&str, i64, bool, u32)> =
            (0..20).map(|_| ("x", 5, false, 2200)).collect();
        let store = store_with(&losses);
        for (_, rating) in rating_history(&store).unwrap() {
            assert!(rating >= f64::from(RATING_FLOOR), "dipped to {rating}");
        }
    }

    #[test]
    fn an_empty_log_produces_empty_series() {
        let store = Store::in_memory().unwrap();
        assert!(slope_points(&store).unwrap().is_empty());
        assert!(rating_history(&store).unwrap().is_empty());
        assert!(game_points(&store).unwrap().is_empty());
    }
}


/// Whether the solver has got faster on puzzles they had **never seen**.
///
/// This is the marker that actually means "better at chess". Re-solving a
/// puzzle faster can be recall of that position; solving a fresh one faster
/// cannot be. Rating band is held fixed so the comparison is between puzzles of
/// comparable difficulty rather than between an easy run and a hard one.
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub band: u32,
    /// Unseen puzzles in this band that were solved correctly.
    pub solved: usize,
    /// Every first encounter in the band, solved or not.
    pub seen: usize,
    pub earlier_seconds: f64,
    pub later_seconds: f64,
    pub earlier_accuracy: f64,
    pub later_accuracy: f64,
    /// One-sided probability that the later half is not faster.
    pub p_value: f64,
}

impl Transfer {
    pub fn is_significant(&self) -> bool {
        self.p_value <= SIGNIFICANT
    }

    /// Faster but less accurate — speed bought by guessing rather than earned.
    ///
    /// Reporting the speed gain alone would be flattering and wrong: solving
    /// quicker while getting more of them wrong is a worse result, not a better
    /// one, and it is the most common way a solver fools themselves.
    pub fn is_speed_accuracy_tradeoff(&self) -> bool {
        self.improvement() > 0.0 && self.later_accuracy + 0.05 < self.earlier_accuracy
    }

    /// Fraction faster; positive is an improvement.
    pub fn improvement(&self) -> f64 {
        if self.earlier_seconds <= 0.0 {
            return 0.0;
        }
        (self.earlier_seconds - self.later_seconds) / self.earlier_seconds
    }
}

/// Transfer per rating band, best-evidenced band first.
///
/// Deliberately not aggregated across bands: as the solver improves they are
/// served harder puzzles, so a pooled figure would mix a change in skill with a
/// change in difficulty.
pub fn transfer_by_band(store: &Store) -> Result<Vec<Transfer>> {
    let attempts = store.first_attempts()?;
    let mut bands: Vec<u32> = attempts.iter().map(|a| a.band).collect();
    bands.sort_unstable();
    bands.dedup();

    let mut out: Vec<Transfer> = bands
        .into_iter()
        .filter_map(|band| {
            let subset: Vec<&FirstAttempt> =
                attempts.iter().filter(|a| a.band == band).collect();
            summarise_transfer(band, &subset)
        })
        .collect();
    out.sort_by_key(|t| std::cmp::Reverse(t.solved));
    Ok(out)
}

fn summarise_transfer(band: u32, attempts: &[&FirstAttempt]) -> Option<Transfer> {
    if attempts.len() < MIN_TRANSFER {
        return None;
    }
    let split = attempts.len() / 2;
    let (earlier, later) = attempts.split_at(split);

    // Only correct attempts are timed, and only ones that were actually being
    // solved: a fast wrong answer is not fluency, and a five-minute one is
    // someone who walked away.
    let times = |set: &[&FirstAttempt]| -> Vec<f64> {
        set.iter()
            .filter(|a| {
                a.correct
                    && a.elapsed.num_seconds() <= crate::store::MAX_MEASURED_SECONDS
            })
            .map(|a| a.elapsed.num_milliseconds() as f64 / 1000.0)
            .collect()
    };
    let accuracy = |set: &[&FirstAttempt]| -> f64 {
        if set.is_empty() {
            return 0.0;
        }
        set.iter().filter(|a| a.correct).count() as f64 / set.len() as f64
    };

    let earlier_times = times(earlier);
    let later_times = times(later);
    if earlier_times.is_empty() || later_times.is_empty() {
        return None;
    }

    Some(Transfer {
        band,
        solved: earlier_times.len() + later_times.len(),
        seen: attempts.len(),
        earlier_seconds: median_of(&earlier_times),
        later_seconds: median_of(&later_times),
        earlier_accuracy: accuracy(earlier),
        later_accuracy: accuracy(later),
        // Faster means smaller, so the test asks whether the earlier times are
        // the larger sample.
        p_value: mann_whitney_greater(&earlier_times, &later_times),
    })
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use crate::store::AttemptRecord;
    use chrono::{Duration, TimeZone, Utc};
    use rs_fsrs::Rating;

    fn store_with(seconds: &[(i64, bool)]) -> Store {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        for (i, (secs, correct)) in seconds.iter().enumerate() {
            store
                .record_attempt(&AttemptRecord {
                    puzzle_id: format!("p{i:03}"),
                    reviewed_at: base + Duration::hours(i as i64),
                    elapsed: Duration::seconds(*secs),
                    correct: *correct,
                    grade: if *correct { Rating::Good } else { Rating::Again },
                    puzzle_rating: 1150,
                })
                .unwrap();
        }
        store
    }

    #[test]
    fn nothing_is_claimed_from_too_few_fresh_puzzles() {
        let few: Vec<(i64, bool)> = (0..MIN_TRANSFER - 1).map(|_| (30, true)).collect();
        assert!(transfer_by_band(&store_with(&few)).unwrap().is_empty());
    }

    #[test]
    fn getting_faster_on_unseen_puzzles_is_detected() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (40, true)).collect();
        runs.extend((0..8).map(|_| (15, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(result.later_seconds < result.earlier_seconds);
        assert!(result.is_significant(), "p was {}", result.p_value);
        assert!(result.improvement() > 0.5);
    }

    #[test]
    fn no_change_on_unseen_puzzles_is_not_called_progress() {
        let flat: Vec<(i64, bool)> = (0..16).map(|i| (30 + (i % 3), true)).collect();
        let result = &transfer_by_band(&store_with(&flat)).unwrap()[0];
        assert!(!result.is_significant(), "p was {}", result.p_value);
    }

    #[test]
    fn getting_slower_on_unseen_puzzles_is_never_significant() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (15, true)).collect();
        runs.extend((0..8).map(|_| (40, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(result.later_seconds > result.earlier_seconds);
        assert!(!result.is_significant());
        assert!(result.improvement() < 0.0);
    }

    #[test]
    fn wrong_answers_count_against_accuracy_but_are_not_timed() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (30, true)).collect();
        // Later half: half of them missed, and missed quickly.
        runs.extend((0..4).map(|_| (2, false)));
        runs.extend((0..4).map(|_| (30, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(
            result.later_accuracy < result.earlier_accuracy,
            "accuracy should fall: {} vs {}",
            result.earlier_accuracy,
            result.later_accuracy
        );
        assert!(
            (result.later_seconds - 30.0).abs() < 1e-9,
            "the 2s misses must not be timed, got {}",
            result.later_seconds
        );
    }

    #[test]
    fn speed_bought_by_guessing_is_named_as_such() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (40, true)).collect();
        // Later half: much faster, but half of them wrong.
        runs.extend((0..4).map(|_| (10, true)));
        runs.extend((0..4).map(|_| (5, false)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(result.improvement() > 0.0, "it did get faster");
        assert!(
            result.is_speed_accuracy_tradeoff(),
            "faster with accuracy {:.2} -> {:.2} must be flagged",
            result.earlier_accuracy,
            result.later_accuracy
        );
    }

    #[test]
    fn getting_faster_without_losing_accuracy_is_not_flagged() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (40, true)).collect();
        runs.extend((0..8).map(|_| (15, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(!result.is_speed_accuracy_tradeoff());
    }

    #[test]
    fn getting_slower_is_never_a_tradeoff() {
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (15, true)).collect();
        runs.extend((0..8).map(|_| (40, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(!result.is_speed_accuracy_tradeoff());
    }

    #[test]
    fn an_interrupted_solve_does_not_drag_the_median() {
        use crate::store::MAX_MEASURED_SECONDS;
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (30, true)).collect();
        // Later half: normal solves plus one where the solver walked away.
        runs.extend((0..7).map(|_| (20, true)));
        runs.push((MAX_MEASURED_SECONDS + 600, true));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(
            (result.later_seconds - 20.0).abs() < 1e-9,
            "the abandoned puzzle must be excluded, got {}",
            result.later_seconds
        );
    }

    #[test]
    fn a_solve_exactly_at_the_limit_still_counts() {
        use crate::store::MAX_MEASURED_SECONDS;
        let mut runs: Vec<(i64, bool)> = (0..8).map(|_| (30, true)).collect();
        runs.extend((0..8).map(|_| (MAX_MEASURED_SECONDS, true)));
        let result = &transfer_by_band(&store_with(&runs)).unwrap()[0];
        assert!(
            (result.later_seconds - MAX_MEASURED_SECONDS as f64).abs() < 1e-9,
            "the boundary must be inclusive, got {}",
            result.later_seconds
        );
    }

    #[test]
    fn a_band_with_no_correct_answers_reports_nothing() {
        let none: Vec<(i64, bool)> = (0..16).map(|_| (30, false)).collect();
        assert!(transfer_by_band(&store_with(&none)).unwrap().is_empty());
    }
}

#[cfg(test)]
mod interval_tests {
    use super::*;
    use crate::store::{AttemptRecord, MIN_REPEAT_HOURS};
    use chrono::{Duration, TimeZone, Utc};
    use rs_fsrs::Rating;

    fn solve(store: &Store, id: &str, at: chrono::DateTime<Utc>, secs: i64) {
        store
            .record_attempt(&AttemptRecord {
                puzzle_id: id.to_owned(),
                reviewed_at: at,
                elapsed: Duration::seconds(secs),
                correct: true,
                grade: Rating::Good,
                puzzle_rating: 1150,
            })
            .unwrap();
    }

    #[test]
    fn a_repeat_on_the_same_day_is_not_counted_as_measurement() {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        solve(&store, "a", base, 40);
        solve(&store, "a", base + Duration::minutes(18), 8);
        assert!(
            store.paired_solves().unwrap().is_empty(),
            "solving it again eighteen minutes later is recall, not skill"
        );
    }

    #[test]
    fn an_abandoned_repeat_is_not_measured() {
        use crate::store::MAX_MEASURED_SECONDS;
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        solve(&store, "a", base, 40);
        solve(
            &store,
            "a",
            base + Duration::hours(MIN_REPEAT_HOURS as i64 + 1),
            MAX_MEASURED_SECONDS + 60,
        );
        assert!(
            store.paired_solves().unwrap().is_empty(),
            "an interrupted solve is not a slower solve"
        );
    }

    #[test]
    fn a_repeat_the_next_day_is_counted() {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        solve(&store, "a", base, 40);
        solve(&store, "a", base + Duration::hours(MIN_REPEAT_HOURS as i64 + 1), 20);
        assert_eq!(store.paired_solves().unwrap().len(), 1);
    }

    #[test]
    fn the_boundary_is_where_it_says_it_is() {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        solve(&store, "early", base, 40);
        solve(&store, "early", base + Duration::minutes((MIN_REPEAT_HOURS * 60.0) as i64 - 5), 20);
        solve(&store, "late", base, 40);
        solve(&store, "late", base + Duration::minutes((MIN_REPEAT_HOURS * 60.0) as i64 + 5), 20);

        let ids: Vec<String> = store
            .paired_solves()
            .unwrap()
            .into_iter()
            .map(|p| p.puzzle_id)
            .collect();
        assert_eq!(ids, vec!["late"], "only the one past the threshold counts");
    }
}


/// A theme that keeps costing the solver, with the evidence for saying so.
///
/// This is the closest thing here to a diagnosis: not "you played badly" but
/// "forks are where it goes wrong, over this many attempts". It is drawn from
/// what actually happened rather than from a curriculum someone wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Weakness {
    pub theme: String,
    pub attempts: u32,
    pub success: f64,
}

/// Attempts on a theme before it is worth naming. Below this a run of bad luck
/// looks identical to a weakness.
pub const MIN_THEME_ATTEMPTS: u32 = 10;
/// How far below your own average a theme must sit before it is named. An
/// absolute bar would flag everything for a weaker solver and nothing for a
/// stronger one; the comparison that means something is against yourself.
pub const WEAKNESS_MARGIN: f64 = 0.10;

/// Themes the solver handles worse than they handle puzzles generally, worst
/// first, together with the baseline they are being judged against.
pub fn recurring_weaknesses(store: &Store) -> Result<(Vec<Weakness>, f64)> {
    let baseline = store.overall_success()?;
    let mut weak: Vec<Weakness> = store
        .theme_success(MIN_THEME_ATTEMPTS)?
        .into_iter()
        .filter(|(_, rate, _)| *rate + WEAKNESS_MARGIN < baseline)
        .map(|(theme, success, attempts)| Weakness {
            theme,
            attempts,
            success,
        })
        .collect();
    weak.sort_by(|a, b| {
        a.success
            .partial_cmp(&b.success)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok((weak, baseline))
}

#[cfg(test)]
mod weakness_tests {
    use super::*;
    use crate::ingest::ingest_csv;
    use crate::store::AttemptRecord;
    use chrono::{Duration, TimeZone, Utc};
    use rs_fsrs::Rating;

    const CSV: &str = "PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate\n";

    fn store_with(rows: &[(&str, &str)], attempts: &[(&str, bool)]) -> Store {
        let mut csv = String::from(CSV);
        for (id, themes) in rows {
            csv.push_str(&format!(
                "{id},6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1150,75,90,100,{themes},https://x,,\n"
            ));
        }
        let mut store = Store::in_memory().unwrap();
        ingest_csv(&mut store, csv.as_bytes(), crate::grade::RATING_FLOOR).unwrap();

        let base = Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap();
        for (i, (id, correct)) in attempts.iter().enumerate() {
            store
                .record_attempt(&AttemptRecord {
                    puzzle_id: (*id).to_owned(),
                    reviewed_at: base + Duration::minutes(i as i64),
                    elapsed: Duration::seconds(20),
                    correct: *correct,
                    grade: if *correct { Rating::Good } else { Rating::Again },
                    puzzle_rating: 1150,
                })
                .unwrap();
        }
        store
    }

    /// Forks go badly, pins go well. The comparison is between them, not
    /// against a fixed bar.
    fn mixed(fork_correct: usize, fork_total: usize) -> Store {
        let mut attempts: Vec<(&str, bool)> =
            (0..fork_total).map(|i| ("fork1", i < fork_correct)).collect();
        attempts.extend((0..12).map(|_| ("pin1", true)));
        store_with(&[("fork1", "fork"), ("pin1", "pin")], &attempts)
    }

    #[test]
    fn a_theme_you_handle_worse_than_the_rest_is_named() {
        let store = mixed(3, 12);
        let (weak, baseline) = recurring_weaknesses(&store).unwrap();
        assert!(baseline > 0.5, "the baseline is your own average: {baseline}");
        assert_eq!(weak.len(), 1, "only the fork should stand out: {weak:?}");
        assert_eq!(weak[0].theme, "fork");
        assert_eq!(weak[0].attempts, 12);
        assert!(weak[0].success + WEAKNESS_MARGIN < baseline);
    }

    #[test]
    fn the_worst_theme_comes_first() {
        let mut attempts: Vec<(&str, bool)> = (0..12).map(|i| ("fork1", i < 5)).collect();
        attempts.extend((0..12).map(|i| ("skew1", i < 2)));
        attempts.extend((0..12).map(|_| ("pin1", true)));
        let store = store_with(
            &[("fork1", "fork"), ("skew1", "skewer"), ("pin1", "pin")],
            &attempts,
        );
        let (weak, _) = recurring_weaknesses(&store).unwrap();
        assert_eq!(
            weak.iter().map(|w| w.theme.as_str()).collect::<Vec<_>>(),
            ["skewer", "fork"],
            "the worst one leads"
        );
    }

    #[test]
    fn a_theme_going_as_well_as_everything_else_is_not_a_weakness() {
        let store = mixed(12, 12);
        let (weak, _) = recurring_weaknesses(&store).unwrap();
        assert!(weak.is_empty(), "solving them all is not a blind spot: {weak:?}");
    }

    #[test]
    fn a_theme_only_a_whisker_below_average_is_not_called_out() {
        // Eleven of twelve against a perfect baseline: barely different.
        let store = mixed(11, 12);
        let (weak, _) = recurring_weaknesses(&store).unwrap();
        assert!(
            weak.is_empty(),
            "a small gap is noise, not a diagnosis: {weak:?}"
        );
    }

    #[test]
    fn too_few_attempts_is_not_evidence_of_anything() {
        let short = MIN_THEME_ATTEMPTS as usize - 1;
        let mut attempts: Vec<(&str, bool)> = (0..short).map(|_| ("fork1", false)).collect();
        attempts.extend((0..12).map(|_| ("pin1", true)));
        let store = store_with(&[("fork1", "fork"), ("pin1", "pin")], &attempts);
        let (weak, _) = recurring_weaknesses(&store).unwrap();
        assert!(
            weak.is_empty(),
            "a short bad run is not a weakness: {weak:?}"
        );
    }

    #[test]
    fn a_solver_who_is_uniformly_weak_has_no_standout_weakness() {
        // Everything at 50%. Nothing is worse than anything else, so naming a
        // theme would be inventing a pattern.
        let mut attempts: Vec<(&str, bool)> = (0..12).map(|i| ("fork1", i % 2 == 0)).collect();
        attempts.extend((0..12).map(|i| ("pin1", i % 2 == 0)));
        let store = store_with(&[("fork1", "fork"), ("pin1", "pin")], &attempts);
        let (weak, _) = recurring_weaknesses(&store).unwrap();
        assert!(weak.is_empty(), "uniform weakness is not a motif: {weak:?}");
    }
}
