//! SQLite persistence for puzzles, FSRS cards and solve attempts.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rs_fsrs::{Card, Rating, State};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

use crate::grade::{band, RATING_FLOOR};

/// Puzzles are drawn from this many rating points either side of the target.
const SELECTION_WINDOW: u32 = 25;
/// Random index probes before falling back to a bounded scan.
const SELECTION_PROBES: u32 = 8;
use crate::puzzle::Puzzle;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS puzzles (
    id               TEXT PRIMARY KEY,
    fen              TEXT NOT NULL,
    moves            TEXT NOT NULL,
    rating           INTEGER NOT NULL,
    rating_deviation INTEGER NOT NULL,
    popularity       INTEGER NOT NULL,
    nb_plays         INTEGER NOT NULL,
    themes           TEXT NOT NULL,
    game_url         TEXT NOT NULL,
    opening_tags     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS puzzles_rating ON puzzles (rating);
-- Selection seeks straight to a (rating, id) point rather than sorting the
-- whole table by distance from the target, which does not scale past a few
-- hundred thousand puzzles.
CREATE INDEX IF NOT EXISTS puzzles_rating_id ON puzzles (rating, id);

CREATE TABLE IF NOT EXISTS puzzle_themes (
    puzzle_id TEXT NOT NULL,
    theme     TEXT NOT NULL,
    PRIMARY KEY (puzzle_id, theme)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS puzzle_themes_theme ON puzzle_themes (theme);

CREATE TABLE IF NOT EXISTS cards (
    puzzle_id      TEXT PRIMARY KEY,
    due            TEXT NOT NULL,
    stability      REAL NOT NULL,
    difficulty     REAL NOT NULL,
    elapsed_days   INTEGER NOT NULL,
    scheduled_days INTEGER NOT NULL,
    reps           INTEGER NOT NULL,
    lapses         INTEGER NOT NULL,
    state          INTEGER NOT NULL,
    last_review    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS cards_due ON cards (due);

CREATE TABLE IF NOT EXISTS attempts (
    id            INTEGER PRIMARY KEY,
    puzzle_id     TEXT NOT NULL,
    reviewed_at   TEXT NOT NULL,
    elapsed_ms    INTEGER NOT NULL,
    correct       INTEGER NOT NULL,
    grade         INTEGER NOT NULL,
    puzzle_rating INTEGER NOT NULL,
    band          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS attempts_band ON attempts (band, reviewed_at);
CREATE INDEX IF NOT EXISTS attempts_puzzle ON attempts (puzzle_id, reviewed_at);

-- One row per finished game. Deliberately records how well it was played as
-- well as how it ended, because the result depends on the opponent and the
-- quality does not.
CREATE TABLE IF NOT EXISTS games (
    id            INTEGER PRIMARY KEY,
    played_at     TEXT NOT NULL,
    player_white  INTEGER NOT NULL,
    opponent_elo  INTEGER NOT NULL,
    result        TEXT NOT NULL,
    moves         INTEGER NOT NULL,
    accuracy      REAL NOT NULL,
    mean_loss     REAL NOT NULL,
    blunders      INTEGER NOT NULL,
    mistakes      INTEGER NOT NULL,
    inaccuracies  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS games_played_at ON games (played_at);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// A repeat sooner than this is recall of a specific position, not evidence of
/// skill: you remember the puzzle you saw this morning. Only re-solves at least
/// this far apart are counted as measurement.
pub const MIN_REPEAT_HOURS: f64 = 20.0;

/// A puzzle's first encounter, which is the only unrehearsed one.
#[derive(Debug, Clone, PartialEq)]
pub struct FirstAttempt {
    pub puzzle_id: String,
    pub band: u32,
    pub at: DateTime<Utc>,
    pub elapsed: Duration,
    pub correct: bool,
}

/// One puzzle solved correctly more than once, far enough apart to mean
/// something: the only comparison that measures a person rather than a puzzle.
#[derive(Debug, Clone, PartialEq)]
pub struct PairedSolve {
    pub puzzle_id: String,
    pub band: u32,
    /// The first correct solve, which is the baseline.
    pub first: Duration,
    /// The most recent correct solve.
    pub latest: Duration,
    /// How many times it has been solved correctly.
    pub solves: u32,
}

impl PairedSolve {
    /// How many times faster the latest solve was than the first. Above one is
    /// an improvement; below one is a regression.
    pub fn speedup(&self) -> f64 {
        let latest = self.latest.num_milliseconds() as f64;
        if latest <= 0.0 {
            return 1.0;
        }
        self.first.num_milliseconds() as f64 / latest
    }
}

/// A finished game, stored for its quality rather than its result.
#[derive(Debug, Clone, PartialEq)]
pub struct GameRecord {
    pub played_at: DateTime<Utc>,
    pub player_white: bool,
    pub opponent_elo: u32,
    /// "won", "lost" or "drawn".
    pub result: String,
    /// The player's own moves that were analysed.
    pub moves: u32,
    pub accuracy: f64,
    pub mean_loss: f64,
    pub blunders: u32,
    pub mistakes: u32,
    pub inaccuracies: u32,
}

/// A recorded solve, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptRecord {
    pub puzzle_id: String,
    pub reviewed_at: DateTime<Utc>,
    pub elapsed: Duration,
    pub correct: bool,
    pub grade: Rating,
    pub puzzle_rating: u32,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::from_connection(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA).context("applying schema")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Bring an existing database up to the current schema.
    ///
    /// `CREATE TABLE IF NOT EXISTS` alone cannot alter a table that already
    /// exists, so the version is recorded and each step applied in order. With
    /// several million rows a rebuild is a four-minute import, which is worth
    /// avoiding for a column addition.
    fn migrate(&self) -> Result<()> {
        let version: u32 =
            self.conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32;

        // Steps are appended here and never renumbered or edited once shipped.
        let steps: [&str; 0] = [];

        for (index, sql) in steps.iter().enumerate() {
            let step = index as u32 + 1;
            if version < step {
                self.conn
                    .execute_batch(sql)
                    .with_context(|| format!("applying migration {step}"))?;
            }
        }

        let target = steps.len() as u32;
        if version < target {
            self.conn
                .pragma_update(None, "user_version", i64::from(target))?;
        }
        Ok(())
    }

    /// The schema version this database is at.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32)
    }

    // -- puzzles ---------------------------------------------------------

    /// Insert or replace puzzles. Returns the number written.
    pub fn insert_puzzles(&mut self, puzzles: &[Puzzle]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO puzzles
                 (id, fen, moves, rating, rating_deviation, popularity, nb_plays,
                  themes, game_url, opening_tags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let mut clear_themes =
                tx.prepare_cached("DELETE FROM puzzle_themes WHERE puzzle_id = ?1")?;
            let mut insert_theme = tx.prepare_cached(
                "INSERT OR IGNORE INTO puzzle_themes (puzzle_id, theme) VALUES (?1, ?2)",
            )?;

            for p in puzzles {
                insert.execute(params![
                    p.id,
                    p.fen,
                    p.moves.join(" "),
                    p.rating,
                    p.rating_deviation,
                    p.popularity,
                    p.nb_plays,
                    p.themes.join(" "),
                    p.game_url,
                    p.opening_tags.join(" "),
                ])?;
                clear_themes.execute(params![p.id])?;
                for theme in &p.themes {
                    insert_theme.execute(params![p.id, theme])?;
                }
            }
        }
        tx.commit()?;
        Ok(puzzles.len())
    }

    pub fn count_puzzles(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM puzzles", [], |r| r.get::<_, i64>(0))?
            as u64)
    }

    pub fn puzzle(&self, id: &str) -> Result<Option<Puzzle>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, fen, moves, rating, rating_deviation, popularity,
                        nb_plays, themes, game_url, opening_tags
                 FROM puzzles WHERE id = ?1",
                params![id],
                puzzle_from_row,
            )
            .optional()?)
    }

    /// Puzzles whose card is due, soonest first.
    pub fn due_puzzles(&self, now: DateTime<Utc>, limit: u32) -> Result<Vec<Puzzle>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                    p.nb_plays, p.themes, p.game_url, p.opening_tags
             FROM cards c JOIN puzzles p ON p.id = c.puzzle_id
             WHERE c.due <= ?1
             ORDER BY c.due ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now, limit], puzzle_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// An unseen puzzle close to `target` rating, optionally of a theme.
    ///
    /// Ordering the whole table by distance from the target costs a full scan
    /// and sort — eleven seconds over four million puzzles — so instead a few
    /// random points inside a narrow rating window are probed directly through
    /// the `(rating, id)` index. Each probe is a seek, and the randomness is
    /// what stops the same puzzle being served every time.
    pub fn unseen_near_rating(&self, target: u32, theme: Option<&str>) -> Result<Option<Puzzle>> {
        let target = target.max(RATING_FLOOR);
        let low = target.saturating_sub(SELECTION_WINDOW).max(RATING_FLOOR);
        let high = target.saturating_add(SELECTION_WINDOW);

        for _ in 0..SELECTION_PROBES {
            let rating = low + (next_random() % u64::from(high - low + 1)) as u32;
            let cursor = random_cursor();
            for forward in [true, false] {
                if let Some(puzzle) = self.seek_unseen(rating, &cursor, theme, forward)? {
                    return Ok(Some(puzzle));
                }
            }
        }

        // Every probe missed — a narrow band that is exhausted, or a rare
        // theme. Fall back to a bounded scan, which is still indexed on rating.
        self.scan_unseen(low, high, theme)
    }

    fn seek_unseen(
        &self,
        rating: u32,
        cursor: &str,
        theme: Option<&str>,
        forward: bool,
    ) -> Result<Option<Puzzle>> {
        let sql = if forward {
            "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                    p.nb_plays, p.themes, p.game_url, p.opening_tags
             FROM puzzles p
             WHERE p.rating = ?1 AND p.id >= ?2
               AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.puzzle_id = p.id)
               AND (?3 IS NULL OR EXISTS (
                     SELECT 1 FROM puzzle_themes t
                     WHERE t.puzzle_id = p.id AND t.theme = ?3))
             ORDER BY p.id ASC LIMIT 1"
        } else {
            "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                    p.nb_plays, p.themes, p.game_url, p.opening_tags
             FROM puzzles p
             WHERE p.rating = ?1 AND p.id < ?2
               AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.puzzle_id = p.id)
               AND (?3 IS NULL OR EXISTS (
                     SELECT 1 FROM puzzle_themes t
                     WHERE t.puzzle_id = p.id AND t.theme = ?3))
             ORDER BY p.id DESC LIMIT 1"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        Ok(stmt
            .query_row(params![rating, cursor, theme], puzzle_from_row)
            .optional()?)
    }

    fn scan_unseen(&self, low: u32, high: u32, theme: Option<&str>) -> Result<Option<Puzzle>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                    p.nb_plays, p.themes, p.game_url, p.opening_tags
             FROM puzzles p
             WHERE p.rating BETWEEN ?1 AND ?2
               AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.puzzle_id = p.id)
               AND (?3 IS NULL OR EXISTS (
                     SELECT 1 FROM puzzle_themes t
                     WHERE t.puzzle_id = p.id AND t.theme = ?3))
             LIMIT 1",
        )?;
        Ok(stmt
            .query_row(params![low, high, theme], puzzle_from_row)
            .optional()?)
    }

    // -- cards -----------------------------------------------------------

    pub fn card(&self, puzzle_id: &str) -> Result<Option<Card>> {
        Ok(self
            .conn
            .query_row(
                "SELECT due, stability, difficulty, elapsed_days, scheduled_days,
                        reps, lapses, state, last_review
                 FROM cards WHERE puzzle_id = ?1",
                params![puzzle_id],
                card_from_row,
            )
            .optional()?)
    }

    pub fn save_card(&self, puzzle_id: &str, card: &Card) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO cards
             (puzzle_id, due, stability, difficulty, elapsed_days, scheduled_days,
              reps, lapses, state, last_review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                puzzle_id,
                card.due,
                card.stability,
                card.difficulty,
                card.elapsed_days,
                card.scheduled_days,
                card.reps,
                card.lapses,
                card.state as i64,
                card.last_review,
            ],
        )?;
        Ok(())
    }

    pub fn due_count(&self, now: DateTime<Utc>) -> Result<u64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM cards WHERE due <= ?1",
            params![now],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    // -- attempts --------------------------------------------------------

    pub fn record_attempt(&self, attempt: &AttemptRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attempts
             (puzzle_id, reviewed_at, elapsed_ms, correct, grade, puzzle_rating, band)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                attempt.puzzle_id,
                attempt.reviewed_at,
                attempt.elapsed.num_milliseconds(),
                attempt.correct as i64,
                attempt.grade as i64,
                attempt.puzzle_rating,
                band(attempt.puzzle_rating),
            ],
        )?;
        Ok(())
    }

    /// Solve times for recent *correct* attempts in a rating band, newest
    /// first. Failures are excluded: an abandoned puzzle says nothing about
    /// how fast the solver recalls one they know.
    pub fn recent_latencies(&self, rating_band: u32, limit: u32) -> Result<Vec<Duration>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT elapsed_ms FROM attempts
             WHERE band = ?1 AND correct = 1
             ORDER BY reviewed_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![rating_band, limit], |r| r.get::<_, i64>(0))?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(Duration::milliseconds)
            .collect())
    }

    /// Every correct attempt in a band, oldest first — the fluency series.
    pub fn latency_series(&self, rating_band: u32) -> Result<Vec<(DateTime<Utc>, Duration)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT reviewed_at, elapsed_ms FROM attempts
             WHERE band = ?1 AND correct = 1
             ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map(params![rating_band], |r| {
            Ok((r.get::<_, DateTime<Utc>>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(at, ms)| (at, Duration::milliseconds(ms)))
            .collect())
    }

    /// Success rate per theme, for themes with at least `min_attempts` tries.
    /// Used to steer selection toward weaknesses.
    pub fn theme_success(&self, min_attempts: u32) -> Result<Vec<(String, f64, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.theme,
                    AVG(a.correct) AS rate,
                    COUNT(*)       AS tries
             FROM attempts a JOIN puzzle_themes t ON t.puzzle_id = a.puzzle_id
             GROUP BY t.theme
             HAVING tries >= ?1
             ORDER BY rate ASC",
        )?;
        let rows = stmt.query_map(params![min_attempts], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, u32>(2)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Puzzles solved correctly at least twice, paired against their own
    /// first solve.
    ///
    /// Comparing early solve times with later ones across *different* puzzles
    /// measures the puzzles as much as the solver. Pairing each puzzle with
    /// itself removes that confound entirely.
    pub fn paired_solves(&self) -> Result<Vec<PairedSolve>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH ok AS (
                 SELECT puzzle_id, elapsed_ms, band, reviewed_at,
                        ROW_NUMBER() OVER (PARTITION BY puzzle_id ORDER BY reviewed_at ASC)  AS first_rank,
                        ROW_NUMBER() OVER (PARTITION BY puzzle_id ORDER BY reviewed_at DESC) AS last_rank,
                        COUNT(*)   OVER (PARTITION BY puzzle_id)                             AS solves
                 FROM attempts
                 WHERE correct = 1
             )
             SELECT f.puzzle_id, f.band, f.elapsed_ms, l.elapsed_ms, f.solves
             FROM ok f
             JOIN ok l ON l.puzzle_id = f.puzzle_id AND l.last_rank = 1
             WHERE f.first_rank = 1
               AND f.solves >= 2
               AND (julianday(l.reviewed_at) - julianday(f.reviewed_at)) * 24.0 >= ?1",
        )?;
        let rows = stmt.query_map(params![MIN_REPEAT_HOURS], |r| {
            Ok(PairedSolve {
                puzzle_id: r.get(0)?,
                band: r.get(1)?,
                first: Duration::milliseconds(r.get(2)?),
                latest: Duration::milliseconds(r.get(3)?),
                solves: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every attempt in order: when, the puzzle's rating, and whether it was
    /// solved. Enough to replay the rating trajectory without storing it.
    pub fn attempt_log(&self) -> Result<Vec<(DateTime<Utc>, u32, bool)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT reviewed_at, puzzle_rating, correct FROM attempts ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The first time each puzzle was seen, oldest first.
    ///
    /// A first encounter cannot be remembered, so solve times across these
    /// measure whether the solver has actually got quicker at puzzles rather
    /// than quicker at these puzzles.
    pub fn first_attempts(&self) -> Result<Vec<FirstAttempt>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH ranked AS (
                 SELECT puzzle_id, band, reviewed_at, elapsed_ms, correct,
                        ROW_NUMBER() OVER (PARTITION BY puzzle_id ORDER BY reviewed_at ASC) AS rank
                 FROM attempts
             )
             SELECT puzzle_id, band, reviewed_at, elapsed_ms, correct
             FROM ranked WHERE rank = 1 ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FirstAttempt {
                puzzle_id: r.get(0)?,
                band: r.get(1)?,
                at: r.get(2)?,
                elapsed: Duration::milliseconds(r.get(3)?),
                correct: r.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Accuracy on puzzles seen before: of every attempt that was not a
    /// puzzle's first, how many were solved. Returns `(rate, attempts)`.
    pub fn repeat_accuracy(&self) -> Result<(f64, u32)> {
        let mut stmt = self.conn.prepare_cached(
            "WITH ranked AS (
                 SELECT correct,
                        ROW_NUMBER() OVER (PARTITION BY puzzle_id ORDER BY reviewed_at ASC) AS rank
                 FROM attempts
             )
             SELECT COALESCE(AVG(correct), 0.0), COUNT(*) FROM ranked WHERE rank > 1",
        )?;
        Ok(stmt.query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// A puzzle already solved correctly, drawn at random for re-testing.
    pub fn solved_for_repeat(&self) -> Result<Option<Puzzle>> {
        Ok(self
            .conn
            .query_row(
                "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                        p.nb_plays, p.themes, p.game_url, p.opening_tags
                 FROM puzzles p
                 WHERE EXISTS (SELECT 1 FROM attempts a
                               WHERE a.puzzle_id = p.id AND a.correct = 1)
                 ORDER BY RANDOM() LIMIT 1",
                [],
                puzzle_from_row,
            )
            .optional()?)
    }

    /// How many distinct puzzles have been solved correctly at least once.
    pub fn solved_count(&self) -> Result<u64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT puzzle_id) FROM attempts WHERE correct = 1",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    // -- games -----------------------------------------------------------

    pub fn record_game(&self, game: &GameRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO games
             (played_at, player_white, opponent_elo, result, moves, accuracy,
              mean_loss, blunders, mistakes, inaccuracies)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                game.played_at,
                game.player_white as i64,
                game.opponent_elo,
                game.result,
                game.moves,
                game.accuracy,
                game.mean_loss,
                game.blunders,
                game.mistakes,
                game.inaccuracies,
            ],
        )?;
        Ok(())
    }

    /// Games oldest first, which is the order a trend is read in.
    pub fn games(&self) -> Result<Vec<GameRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT played_at, player_white, opponent_elo, result, moves, accuracy,
                    mean_loss, blunders, mistakes, inaccuracies
             FROM games ORDER BY played_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(GameRecord {
                played_at: r.get(0)?,
                player_white: r.get::<_, i64>(1)? != 0,
                opponent_elo: r.get(2)?,
                result: r.get(3)?,
                moves: r.get(4)?,
                accuracy: r.get(5)?,
                mean_loss: r.get(6)?,
                blunders: r.get(7)?,
                mistakes: r.get(8)?,
                inaccuracies: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // -- backup ----------------------------------------------------------

    pub fn export_attempts(&self) -> Result<Vec<crate::backup::AttemptRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT puzzle_id, reviewed_at, elapsed_ms, correct, grade, puzzle_rating
             FROM attempts ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::backup::AttemptRow {
                puzzle_id: r.get(0)?,
                reviewed_at: r.get(1)?,
                elapsed_ms: r.get(2)?,
                correct: r.get::<_, i64>(3)? != 0,
                grade: r.get(4)?,
                puzzle_rating: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn export_cards(&self) -> Result<Vec<crate::backup::CardRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT puzzle_id, due, stability, difficulty, elapsed_days, scheduled_days,
                    reps, lapses, state, last_review
             FROM cards ORDER BY puzzle_id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::backup::CardRow {
                puzzle_id: r.get(0)?,
                due: r.get(1)?,
                stability: r.get(2)?,
                difficulty: r.get(3)?,
                elapsed_days: r.get(4)?,
                scheduled_days: r.get(5)?,
                reps: r.get(6)?,
                lapses: r.get(7)?,
                state: r.get(8)?,
                last_review: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn export_settings(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value FROM settings ORDER BY key ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Whether this exact attempt is already recorded, so a restore can be run
    /// twice without doubling the history.
    pub fn has_attempt(&self, puzzle_id: &str, at: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM attempts WHERE puzzle_id = ?1 AND reviewed_at = ?2",
            params![puzzle_id, at],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    pub fn has_game(&self, at: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE played_at = ?1",
            params![at],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    pub fn write_card_row(&self, card: &crate::backup::CardRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO cards
             (puzzle_id, due, stability, difficulty, elapsed_days, scheduled_days,
              reps, lapses, state, last_review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                card.puzzle_id,
                card.due,
                card.stability,
                card.difficulty,
                card.elapsed_days,
                card.scheduled_days,
                card.reps,
                card.lapses,
                card.state,
                card.last_review,
            ],
        )?;
        Ok(())
    }

    // -- settings --------------------------------------------------------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Whether the trainer is re-testing solved puzzles rather than teaching
    /// new ones. Measurement is only meaningful while this is on.
    pub fn repeat_mode(&self) -> Result<bool> {
        Ok(self.setting("repeat_mode")?.as_deref() == Some("on"))
    }

    pub fn set_repeat_mode(&self, on: bool) -> Result<()> {
        self.set_setting("repeat_mode", if on { "on" } else { "off" })
    }


    pub fn personal_rating(&self) -> Result<f64> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'personal_rating'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match stored {
            Some(v) => v.parse().context("stored personal_rating is not a number"),
            None => Ok(f64::from(RATING_FLOOR)),
        }
    }

    pub fn set_personal_rating(&self, rating: f64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('personal_rating', ?1)",
            params![rating.to_string()],
        )?;
        Ok(())
    }
}

fn puzzle_from_row(row: &Row<'_>) -> rusqlite::Result<Puzzle> {
    Ok(Puzzle {
        id: row.get(0)?,
        fen: row.get(1)?,
        moves: split_field(&row.get::<_, String>(2)?),
        rating: row.get(3)?,
        rating_deviation: row.get(4)?,
        popularity: row.get(5)?,
        nb_plays: row.get(6)?,
        themes: split_field(&row.get::<_, String>(7)?),
        game_url: row.get(8)?,
        opening_tags: split_field(&row.get::<_, String>(9)?),
    })
}

fn card_from_row(row: &Row<'_>) -> rusqlite::Result<Card> {
    Ok(Card {
        due: row.get(0)?,
        stability: row.get(1)?,
        difficulty: row.get(2)?,
        elapsed_days: row.get(3)?,
        scheduled_days: row.get(4)?,
        reps: row.get(5)?,
        lapses: row.get(6)?,
        state: state_from_i64(row.get(7)?),
        last_review: row.get(8)?,
    })
}

fn state_from_i64(value: i64) -> State {
    match value {
        1 => State::Learning,
        2 => State::Review,
        3 => State::Relearning,
        _ => State::New,
    }
}

fn split_field(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_owned).collect()
}

/// A small xorshift generator, seeded from the clock.
///
/// Selection needs only enough randomness to avoid serving the same puzzle
/// twice in a row, which does not justify a dependency.
fn next_random() -> u64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};

    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }

    STATE.with(|state| {
        let mut x = state.get();
        if x == 0 {
            x = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x2545_f491_4f6c_dd1d)
                | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state.set(x);
        x
    })
}

/// A random point in the Lichess puzzle-id space, used to start an index seek.
fn random_cursor() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    (0..5)
        .map(|_| ALPHABET[(next_random() % ALPHABET.len() as u64) as usize] as char)
        .collect()
}
