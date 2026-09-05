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
    /// The sitting in progress, and how many solves it has seen. Held here so
    /// every attempt is filed against it without the caller having to
    /// remember, which is how a counter like this quietly stops being kept.
    sitting: std::cell::Cell<Option<i64>>,
    solved_here: std::cell::Cell<u32>,
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
    /// Open a sitting, so attempts can be told apart by how deep into a
    /// session they were. Called once when the trainer starts working.
    pub fn open_sitting(&self, store: &Store, kind: &str, now: DateTime<Utc>) -> Result<()> {
        if self.sitting.get().is_some() {
            return Ok(());
        }
        self.sitting.set(Some(store.begin_session(kind, now)?));
        self.solved_here.set(0);
        Ok(())
    }

    pub fn close_sitting(&self, store: &Store, now: DateTime<Utc>) -> Result<()> {
        if let Some(id) = self.sitting.take() {
            store.end_session(id, now)?;
        }
        Ok(())
    }

    pub fn sitting(&self) -> Option<i64> {
        self.sitting.get()
    }

    /// How many puzzles have been solved in this sitting so far.
    pub fn solved_here(&self) -> u32 {
        self.solved_here.get()
    }

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
            session_id: self.sitting.get(),
            index_in_session: self.solved_here.get(),
            correct: solve.correct,
            grade: rating,
            puzzle_rating: solve.puzzle_rating,
        })?;
        self.solved_here.set(self.solved_here.get() + 1);

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
