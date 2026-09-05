//! The review loop: joining a solve attempt to the schedule and the rating.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use rs_fsrs::{Card, Rating, FSRS};

use crate::grade::{band, baseline_from, grade, next_rating, seed_baseline, speed, Speed};
use crate::puzzle::Puzzle;
use crate::store::{AttemptRecord, Store};

/// How many recent solves inform a band's baseline.
pub const BASELINE_WINDOW: u32 = 20;
/// Attempts needed before a theme's success rate is allowed to steer selection.
pub const MIN_THEME_ATTEMPTS: u32 = 8;
/// Below this success rate a theme counts as a weakness worth drilling.
pub const WEAKNESS_THRESHOLD: f64 = 0.7;

/// What the user did with a puzzle.
#[derive(Debug, Clone, PartialEq)]
pub struct Solve {
    pub puzzle_id: String,
    pub puzzle_rating: u32,
    pub correct: bool,
    pub elapsed: Duration,
}

/// What the system decided as a result.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub grade: Rating,
    pub speed: Speed,
    /// The baseline the solve was judged against.
    pub baseline: Duration,
    /// When the puzzle comes back.
    pub due: DateTime<Utc>,
    pub personal_rating: f64,
}

#[derive(Default)]
pub struct Session {
    fsrs: FSRS,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// The solve time this user is currently expected to need for a puzzle of
    /// this rating: the median of recent solves in the band once there are
    /// enough of them, and a seeded estimate until then.
    pub fn baseline(&self, store: &Store, puzzle_rating: u32) -> Result<Duration> {
        let rating_band = band(puzzle_rating);
        let recent = store.recent_latencies(rating_band, BASELINE_WINDOW)?;
        Ok(baseline_from(&recent).unwrap_or_else(|| seed_baseline(rating_band)))
    }

    /// Record a solve: grade it against the baseline, advance the FSRS card,
    /// store the attempt, and update the personal rating.
    pub fn submit(&self, store: &mut Store, solve: &Solve, now: DateTime<Utc>) -> Result<Outcome> {
        let baseline = self.baseline(store, solve.puzzle_rating)?;
        let rating = grade(solve.correct, solve.elapsed, baseline);
        let observed = speed(solve.elapsed, baseline);

        let card = store.card(&solve.puzzle_id)?.unwrap_or_else(Card::new);
        // Every rating should be present, but this runs on every solve, and a
        // scheduling surprise is not worth taking the application down for.
        let scheduled = self
            .fsrs
            .repeat(card, now)
            .get(&rating)
            .cloned()
            .ok_or_else(|| anyhow!("the scheduler returned no entry for {rating:?}"))?
            .card;
        store.save_card(&solve.puzzle_id, &scheduled)?;

        store.record_attempt(&AttemptRecord {
            puzzle_id: solve.puzzle_id.clone(),
            reviewed_at: now,
            elapsed: solve.elapsed,
            correct: solve.correct,
            grade: rating,
            puzzle_rating: solve.puzzle_rating,
        })?;

        let personal_rating = next_rating(
            store.personal_rating()?,
            f64::from(solve.puzzle_rating),
            solve.correct,
        );
        store.set_personal_rating(personal_rating)?;

        Ok(Outcome {
            grade: rating,
            speed: observed,
            baseline,
            due: scheduled.due,
            personal_rating,
        })
    }

    /// The next puzzle to show, which depends entirely on the mode.
    ///
    /// The two modes are kept strictly apart on purpose. Learning serves only
    /// material that has never been seen, so it can never be mistaken for
    /// evidence of progress. Repeating serves only material already solved,
    /// which is the only thing a solve time can be compared against.
    /// Which phase of the game the player is worst at, where one is clearly
    /// worse than the others. `None` when there is not enough to tell.
    fn weakest_phase(&self, store: &Store) -> Result<Option<&'static str>> {
        const PHASES: [&str; 3] = ["opening", "middlegame", "endgame"];
        let scored = store.theme_success(crate::progress::MIN_THEME_ATTEMPTS)?;
        let mut worst: Option<(&'static str, f64)> = None;
        for (theme, rate, _) in &scored {
            let Some(phase) = PHASES.iter().find(|p| *p == theme) else {
                continue;
            };
            if worst.is_none_or(|(_, lowest)| *rate < lowest) {
                worst = Some((phase, *rate));
            }
        }
        Ok(worst.map(|(phase, _)| phase))
    }

    pub fn next_puzzle(&self, store: &Store, now: DateTime<Utc>) -> Result<Option<Puzzle>> {
        if store.repeat_mode()? {
            // Spaced repetition decides which of the solved puzzles is ripe;
            // anything already solved will do once nothing is due.
            if let Some(puzzle) = store.due_puzzles(now, 1)?.into_iter().next() {
                return Ok(Some(puzzle));
            }
            return store.solved_for_repeat();
        }

        let target = store.personal_rating()?.round().max(0.0) as u32;

        // Positions lifted out of your own losses are the material a coach
        // would reach for first, and there are only ever a few dozen of them,
        // so they have to be asked for by name or they are never served.
        if store.own_mistakes_mode()? {
            let own = crate::review::OWN_GAME_THEME;
            // Narrow to the phase that keeps costing games, where one stands
            // out — a player who loses in the middlegame should be shown their
            // middlegame mistakes before anything else.
            if let Some(phase) = self.weakest_phase(store)? {
                if let Some(puzzle) = store.unseen_with_themes(&[own, phase])? {
                    return Ok(Some(puzzle));
                }
            }
            if let Some(puzzle) = store.unseen_with_themes(&[own])? {
                return Ok(Some(puzzle));
            }
            // Solved out: fall through rather than stalling, and let the
            // caller notice the stock is empty.
        }

        if let Some(theme) = self.weakest_theme(store)? {
            if let Some(puzzle) = store.unseen_near_rating(target, Some(&theme))? {
                return Ok(Some(puzzle));
            }
        }
        store.unseen_near_rating(target, None)
    }

    /// The theme with the worst success rate, if it is bad enough to be worth
    /// targeting and has been attempted often enough to be believable.
    pub fn weakest_theme(&self, store: &Store) -> Result<Option<String>> {
        Ok(store
            .theme_success(MIN_THEME_ATTEMPTS)?
            .into_iter()
            .find(|(_, rate, _)| *rate < WEAKNESS_THRESHOLD)
            .map(|(theme, _, _)| theme))
    }
}
