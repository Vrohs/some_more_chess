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

-- One attempt at converting a theoretical endgame. Whether a won position was
-- won is the least arguable measurement in the application, so it is kept
-- separate from puzzle attempts rather than blended into them.
-- Every move made anywhere that is not a puzzle solve: a game against the
-- engine, a position stepped through in study, an endgame being converted.
--
-- Puzzles keep their own table because an attempt has a right answer to score
-- against; these do not, so what is worth keeping is the move, how long it
-- took, and whatever the activity knows about the moment — the clock, the
-- direction of travel, the phase.
CREATE TABLE IF NOT EXISTS move_log (
    id         INTEGER PRIMARY KEY,
    session_id INTEGER,
    activity   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    at         TEXT NOT NULL,
    ply        INTEGER NOT NULL,
    played     TEXT NOT NULL,
    think_ms   INTEGER NOT NULL,
    detail     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS move_log_activity ON move_log (activity, at);
CREATE INDEX IF NOT EXISTS move_log_subject ON move_log (activity, subject, ply);

-- Where a drill position came from.
--
-- A position lifted from a lost game is only training material if the solver
-- can be told what they actually played and what it cost. Without that it is
-- an anonymous puzzle that happens to have come from somewhere.
CREATE TABLE IF NOT EXISTS drill_positions (
    puzzle_id  TEXT PRIMARY KEY,
    source     TEXT NOT NULL,
    played_at  TEXT NOT NULL,
    ply        INTEGER NOT NULL,
    played     TEXT NOT NULL,
    best       TEXT NOT NULL,
    lost       REAL NOT NULL,
    phase      TEXT NOT NULL,
    opponent   TEXT NOT NULL DEFAULT ''
);

-- Every move offered during a solve, right or wrong.
--
-- The verdict alone throws away the part worth having: which wrong move
-- attracted the solver. A player who reaches for the same losing capture in
-- twenty positions has one habit, not twenty failures, and that is only
-- visible if the move played is written down.
CREATE TABLE IF NOT EXISTS attempt_moves (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER,
    puzzle_id   TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    ply         INTEGER NOT NULL,
    played      TEXT NOT NULL,
    expected    TEXT NOT NULL,
    correct     INTEGER NOT NULL,
    thought_ms  INTEGER NOT NULL,
    revealed    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS attempt_moves_puzzle ON attempt_moves (puzzle_id, started_at);
CREATE INDEX IF NOT EXISTS attempt_moves_session ON attempt_moves (session_id, id);

-- A sitting at the board, so fatigue is measurable: whether the twentieth
-- puzzle of a session goes worse than the second is a training question, and
-- unanswerable without knowing which sitting each attempt belonged to.
CREATE TABLE IF NOT EXISTS sessions (
    id         INTEGER PRIMARY KEY,
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    kind       TEXT NOT NULL
);

-- Anything else worth recording, kept loosely so a new measurement does not
-- need a migration before it can start collecting.
CREATE TABLE IF NOT EXISTS events (
    id     INTEGER PRIMARY KEY,
    at     TEXT NOT NULL,
    kind   TEXT NOT NULL,
    detail TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_kind ON events (kind, at);

CREATE TABLE IF NOT EXISTS endgame_attempts (
    id           INTEGER PRIMARY KEY,
    endgame_key  TEXT NOT NULL,
    attempted_at TEXT NOT NULL,
    achieved     INTEGER NOT NULL,
    moves        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS endgame_attempts_key ON endgame_attempts (endgame_key, attempted_at);
"#;

/// A solve longer than this was not a solve: the solver walked away, took a
/// call, or left the window open. Such attempts are still recorded — they are
/// what happened — but they are excluded from every timing figure, because one
/// interrupted puzzle would otherwise dominate a median.
pub const MAX_MEASURED_SECONDS: i64 = 300;

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
    /// Where the game came from, for imported games. Empty for games played
    /// in the application.
    pub source: String,
    /// Whose game this is. Statistics are scoped to it, so importing another
    /// player's export can never be counted as your own play.
    pub player: String,
    /// The most specific named opening the game reached, empty when it left
    /// book immediately or the book has no name for it.
    pub opening: String,
    /// How many plies followed that named line.
    pub book_plies: u32,
    /// The time control played, empty for an untimed game.
    pub time_control: String,
    /// The player's own moves made with a low clock, and how many of those
    /// were blunders. Zero when the game was untimed.
    pub pressure_moves: u32,
    pub pressure_blunders: u32,
    /// Mean win probability given away per move in each phase, and how many
    /// moves were played in it. A loss of -1 means the game predates the
    /// breakdown being recorded.
    pub phases: [PhaseLoss; 3],
}

/// One phase of one game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseLoss {
    pub mean_loss: f64,
    pub moves: u32,
}

impl PhaseLoss {
    pub const UNKNOWN: Self = Self {
        mean_loss: -1.0,
        moves: 0,
    };

    pub fn is_known(&self) -> bool {
        self.mean_loss >= 0.0 && self.moves > 0
    }
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
    /// The sitting this belonged to, and how many solves preceded it in that
    /// sitting. A solve is not the same event fresh as it is twenty deep.
    pub session_id: Option<i64>,
    pub index_in_session: u32,
}

/// Where a drill position was taken from.
#[derive(Debug, Clone, PartialEq)]
pub struct DrillOrigin {
    /// The game it came from, empty for one played in the application.
    pub source: String,
    pub played_at: DateTime<Utc>,
    /// The move number in that game.
    pub ply: u32,
    /// What was actually played, and what should have been.
    pub played: String,
    pub best: String,
    /// Win probability given away, 0 to 1.
    pub lost: f64,
    pub phase: String,
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
        let steps: [&str; 6] = [
            // Imported games need an identity of their own so a re-import does
            // not duplicate them; games played in the app leave it empty.
            "ALTER TABLE games ADD COLUMN source TEXT NOT NULL DEFAULT ''",
            // Where a game leaked is far more useful than how much it leaked in
            // total, and it was being computed during analysis and discarded.
            "ALTER TABLE games ADD COLUMN opening_loss REAL NOT NULL DEFAULT -1;
             ALTER TABLE games ADD COLUMN middlegame_loss REAL NOT NULL DEFAULT -1;
             ALTER TABLE games ADD COLUMN endgame_loss REAL NOT NULL DEFAULT -1;
             ALTER TABLE games ADD COLUMN opening_moves INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE games ADD COLUMN middlegame_moves INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE games ADD COLUMN endgame_moves INTEGER NOT NULL DEFAULT 0",
            // Games were stored with no owner, so importing a second player's
            // export merged their play into yours with no way to tell them
            // apart. Existing rows belong to the remembered name.
            "ALTER TABLE games ADD COLUMN player TEXT NOT NULL DEFAULT '';
             UPDATE games SET player =
                 COALESCE((SELECT value FROM settings WHERE key = 'player_name'), '')
             WHERE player = ''",
            // Which opening a game was, and how far it followed a named line.
            // Without these the application can say how you play but never
            // what you play, which is half of preparing for anything.
            "ALTER TABLE games ADD COLUMN opening TEXT NOT NULL DEFAULT '';
             ALTER TABLE games ADD COLUMN book_plies INTEGER NOT NULL DEFAULT 0",
            // How the game was timed, and how many of the player's moves were
            // made on a low clock. Whether blunders cluster there is the one
            // thing the move-time record could never answer on its own.
            "ALTER TABLE games ADD COLUMN time_control TEXT NOT NULL DEFAULT '';
             ALTER TABLE games ADD COLUMN pressure_moves INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE games ADD COLUMN pressure_blunders INTEGER NOT NULL DEFAULT 0",
            // Which sitting an attempt belonged to, and how far into it. A
            // solve is not the same event at the start of a session as at the
            // end of one, and until now there was no way to tell them apart.
            "ALTER TABLE attempts ADD COLUMN session_id INTEGER;
             ALTER TABLE attempts ADD COLUMN index_in_session INTEGER NOT NULL DEFAULT 0",
        ];

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
        // theme. Fall back to a bounded scan, which is still indexed on rating,
        // widening the band until something unseen turns up. Returning nothing
        // here stalls the trainer, so it must only happen when the whole
        // corpus really is solved.
        let mut low = low;
        let mut high = high;
        loop {
            if let Some(puzzle) = self.scan_unseen(low, high, theme)? {
                return Ok(Some(puzzle));
            }
            if low == RATING_FLOOR && high == u32::MAX {
                return Ok(None);
            }
            let reach = (high - low).max(SELECTION_WINDOW);
            low = low.saturating_sub(reach).max(RATING_FLOOR);
            high = high.saturating_add(reach);
        }
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

    /// An unseen puzzle carrying every one of `themes`, chosen at random.
    ///
    /// Rating plays no part: this serves the player's own mistakes, of which
    /// there are a few hundred at most, and banding a set that small by
    /// difficulty would mostly mean serving nothing.
    pub fn unseen_with_themes(&self, themes: &[&str]) -> Result<Option<Puzzle>> {
        if themes.is_empty() {
            return Ok(None);
        }
        let placeholders = vec!["?"; themes.len()].join(", ");
        let sql = format!(
            "SELECT p.id, p.fen, p.moves, p.rating, p.rating_deviation, p.popularity,
                    p.nb_plays, p.themes, p.game_url, p.opening_tags
             FROM puzzles p
             JOIN puzzle_themes t ON t.puzzle_id = p.id
             WHERE t.theme IN ({placeholders})
               AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.puzzle_id = p.id)
             GROUP BY p.id
             HAVING COUNT(DISTINCT t.theme) = {}
             ORDER BY RANDOM() LIMIT 1",
            themes.len()
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(stmt
            .query_row(rusqlite::params_from_iter(themes.iter()), puzzle_from_row)
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
             (puzzle_id, reviewed_at, elapsed_ms, correct, grade, puzzle_rating, band,
                session_id, index_in_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                attempt.puzzle_id,
                attempt.reviewed_at,
                attempt.elapsed.num_milliseconds(),
                attempt.correct as i64,
                attempt.grade as i64,
                attempt.puzzle_rating,
                band(attempt.puzzle_rating),
                attempt.session_id,
                attempt.index_in_session,
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

    /// Fraction of all attempts solved. A motif is judged against this rather
    /// than an absolute bar: the useful question is not "are you good at forks"
    /// but "are forks worse for you than everything else you do".
    pub fn overall_success(&self) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(AVG(correct), 0.0) FROM attempts",
            [],
            |r| r.get(0),
        )?)
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
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, u32>(2)?,
            ))
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
                 WHERE correct = 1 AND elapsed_ms <= ?2
             )
             SELECT f.puzzle_id, f.band, f.elapsed_ms, l.elapsed_ms, f.solves
             FROM ok f
             JOIN ok l ON l.puzzle_id = f.puzzle_id AND l.last_rank = 1
             WHERE f.first_rank = 1
               AND f.solves >= 2
               AND (julianday(l.reviewed_at) - julianday(f.reviewed_at)) * 24.0 >= ?1",
        )?;
        let rows = stmt.query_map(
            params![MIN_REPEAT_HOURS, MAX_MEASURED_SECONDS * 1000],
            |r| {
                Ok(PairedSolve {
                    puzzle_id: r.get(0)?,
                    band: r.get(1)?,
                    first: Duration::milliseconds(r.get(2)?),
                    latest: Duration::milliseconds(r.get(3)?),
                    solves: r.get(4)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every attempt in order: when, the puzzle's rating, and whether it was
    /// solved. Enough to replay the rating trajectory without storing it.
    pub fn attempt_log(&self) -> Result<Vec<(DateTime<Utc>, u32, bool)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT reviewed_at, puzzle_rating, correct FROM attempts ORDER BY reviewed_at ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
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
              mean_loss, blunders, mistakes, inaccuracies, source,
              opening_loss, middlegame_loss, endgame_loss,
              opening_moves, middlegame_moves, endgame_moves, player, opening, book_plies, time_control, pressure_moves, pressure_blunders)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
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
                game.source,
                game.phases[0].mean_loss,
                game.phases[1].mean_loss,
                game.phases[2].mean_loss,
                game.phases[0].moves,
                game.phases[1].moves,
                game.phases[2].moves,
                game.player,
                game.opening,
                game.book_plies,
                game.time_control,
                game.pressure_moves,
                game.pressure_blunders,
            ],
        )?;
        Ok(())
    }

    /// Forget every imported game, so an export can be analysed again after
    /// the analysis itself has changed.
    pub fn forget_imported_games(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM games WHERE source <> '' AND player = ?1",
            params![self.setting("player_name")?.unwrap_or_default()],
        )?)
    }

    /// Whether a game from this source is already stored.
    pub fn has_game_source(&self, source: &str) -> Result<bool> {
        if source.is_empty() {
            return Ok(false);
        }
        let player = self.setting("player_name")?.unwrap_or_default();
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE source = ?1 AND player = ?2",
            params![source, player],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    // -- the move log ----------------------------------------------------

    /// Record one move made outside the puzzle trainer.
    #[allow(clippy::too_many_arguments)]
    pub fn log_move(
        &self,
        session_id: Option<i64>,
        activity: &str,
        subject: &str,
        at: DateTime<Utc>,
        ply: u32,
        played: &str,
        think: std::time::Duration,
        detail: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO move_log
             (session_id, activity, subject, at, ply, played, think_ms, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id,
                activity,
                subject,
                at,
                ply,
                played,
                think.as_millis() as i64,
                detail
            ],
        )?;
        Ok(())
    }

    /// How much has been logged for an activity, and the median think time.
    ///
    /// The median rather than the mean: one position left open over lunch
    /// would otherwise decide the answer.
    pub fn activity_summary(&self, activity: &str) -> Result<Option<(u32, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT think_ms FROM move_log WHERE activity = ?1 ORDER BY think_ms ASC",
        )?;
        let times: Vec<i64> = stmt
            .query_map(params![activity], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if times.is_empty() {
            return Ok(None);
        }
        Ok(Some((times.len() as u32, times[times.len() / 2])))
    }

    /// Every activity that has been logged, with how many moves each holds.
    pub fn activities(&self) -> Result<Vec<(String, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT activity, COUNT(*) FROM move_log GROUP BY activity ORDER BY 2 DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // -- drills ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn record_drill_origin(
        &self,
        puzzle_id: &str,
        source: &str,
        played_at: DateTime<Utc>,
        ply: u32,
        played: &str,
        best: &str,
        lost: f64,
        phase: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO drill_positions
             (puzzle_id, source, played_at, ply, played, best, lost, phase)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![puzzle_id, source, played_at, ply, played, best, lost, phase],
        )?;
        Ok(())
    }

    /// Where a drill position came from, for telling the solver what they did.
    pub fn drill_origin(&self, puzzle_id: &str) -> Result<Option<DrillOrigin>> {
        Ok(self
            .conn
            .query_row(
                "SELECT source, played_at, ply, played, best, lost, phase
                 FROM drill_positions WHERE puzzle_id = ?1",
                params![puzzle_id],
                |r| {
                    Ok(DrillOrigin {
                        source: r.get(0)?,
                        played_at: r.get(1)?,
                        ply: r.get(2)?,
                        played: r.get(3)?,
                        best: r.get(4)?,
                        lost: r.get(5)?,
                        phase: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    // -- raw collection --------------------------------------------------

    /// Open a sitting and return its id.
    pub fn begin_session(&self, kind: &str, at: DateTime<Utc>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (started_at, kind) VALUES (?1, ?2)",
            params![at, kind],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn end_session(&self, id: i64, at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }

    /// Record one move offered during a solve.
    #[allow(clippy::too_many_arguments)]
    pub fn record_attempt_move(
        &self,
        session_id: Option<i64>,
        puzzle_id: &str,
        started_at: DateTime<Utc>,
        ply: u32,
        played: &str,
        expected: &str,
        correct: bool,
        thought: std::time::Duration,
        revealed: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attempt_moves
             (session_id, puzzle_id, started_at, ply, played, expected, correct,
              thought_ms, revealed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id,
                puzzle_id,
                started_at,
                ply,
                played,
                expected,
                correct as i64,
                thought.as_millis() as i64,
                revealed as i64
            ],
        )?;
        Ok(())
    }

    /// Every wrong move played, most frequent first, with what was right.
    ///
    /// This is the raw material for noticing a habit rather than a run of
    /// unrelated failures.
    pub fn wrong_moves(&self) -> Result<Vec<(String, String, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT played, expected, COUNT(*) AS n
             FROM attempt_moves WHERE correct = 0
             GROUP BY played, expected ORDER BY n DESC, played ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u32))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Correctness and time by position within a sitting, for reading fatigue.
    pub fn by_position_in_session(&self) -> Result<Vec<(u32, bool, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT index_in_session, correct, elapsed_ms
             FROM attempts WHERE session_id IS NOT NULL
             ORDER BY index_in_session ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)? as u32,
                r.get::<_, i64>(1)? != 0,
                r.get::<_, i64>(2)?,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Correctness in order for one sitting, for judging it while it runs.
    pub fn sitting_results(&self, session_id: i64) -> Result<Vec<bool>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT correct FROM attempts WHERE session_id = ?1
             ORDER BY index_in_session ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| Ok(r.get::<_, i64>(0)? != 0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Record anything worth keeping that has no table of its own.
    pub fn record_event(&self, at: DateTime<Utc>, kind: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (at, kind, detail) VALUES (?1, ?2, ?3)",
            params![at, kind, detail],
        )?;
        Ok(())
    }

    pub fn events(&self, kind: &str, limit: u32) -> Result<Vec<(DateTime<Utc>, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT at, detail FROM events WHERE kind = ?1 ORDER BY at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![kind, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // -- endgames --------------------------------------------------------

    pub fn record_endgame(
        &self,
        key: &str,
        at: DateTime<Utc>,
        achieved: bool,
        moves: u32,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO endgame_attempts (endgame_key, attempted_at, achieved, moves)
             VALUES (?1, ?2, ?3, ?4)",
            params![key, at, achieved as i64, moves],
        )?;
        Ok(())
    }

    /// Attempts and successes for one endgame, most recent attempt last.
    pub fn endgame_record(&self, key: &str) -> Result<(u32, u32)> {
        self.conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(achieved), 0)
                 FROM endgame_attempts WHERE endgame_key = ?1",
                params![key],
                |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32)),
            )
            .map_err(Into::into)
    }

    /// Moves taken in each successful conversion of an endgame, most recent
    /// last. How long a win took against how long it needed is the sharpest
    /// skill measure the application has: the target is not an opinion.
    pub fn endgame_conversions(&self, key: &str) -> Result<Vec<u32>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT moves FROM endgame_attempts
             WHERE endgame_key = ?1 AND achieved = 1 ORDER BY attempted_at ASC",
        )?;
        let rows = stmt.query_map(params![key], |r| Ok(r.get::<_, i64>(0)? as u32))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every endgame attempt, oldest first, for the progress readout.
    pub fn endgame_attempts(&self) -> Result<Vec<(String, DateTime<Utc>, bool)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT endgame_key, attempted_at, achieved
             FROM endgame_attempts ORDER BY attempted_at ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Games oldest first, which is the order a trend is read in.
    /// Every stored game, oldest first. Used for backups, which must keep
    /// rows belonging to every name that has been imported.
    pub fn games(&self) -> Result<Vec<GameRecord>> {
        self.games_where(None)
    }

    /// Only the remembered player's games, oldest first. Every statistic
    /// reads this: another player's imported export must never be counted as
    /// your own play.
    pub fn games_mine(&self) -> Result<Vec<GameRecord>> {
        let player = self.setting("player_name")?.unwrap_or_default();
        self.games_where(Some(player))
    }

    fn games_where(&self, player: Option<String>) -> Result<Vec<GameRecord>> {
        const COLUMNS: &str = "played_at, player_white, opponent_elo, result, moves, accuracy,
                    mean_loss, blunders, mistakes, inaccuracies, source,
                    opening_loss, middlegame_loss, endgame_loss,
                    opening_moves, middlegame_moves, endgame_moves, player,
                    opening, book_plies, time_control, pressure_moves,
                    pressure_blunders";
        let read = |r: &rusqlite::Row<'_>| {
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
                source: r.get(10)?,
                phases: [
                    PhaseLoss {
                        mean_loss: r.get(11)?,
                        moves: r.get(14)?,
                    },
                    PhaseLoss {
                        mean_loss: r.get(12)?,
                        moves: r.get(15)?,
                    },
                    PhaseLoss {
                        mean_loss: r.get(13)?,
                        moves: r.get(16)?,
                    },
                ],
                player: r.get(17)?,
                opening: r.get(18)?,
                book_plies: r.get(19)?,
                time_control: r.get(20)?,
                pressure_moves: r.get(21)?,
                pressure_blunders: r.get(22)?,
            })
        };
        let rows = match player {
            Some(player) => {
                let sql = format!(
                    "SELECT {COLUMNS} FROM games
                     WHERE player = ?1 OR player = '' ORDER BY played_at ASC"
                );
                let mut stmt = self.conn.prepare_cached(&sql)?;
                let rows = stmt
                    .query_map(params![player], read)?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            }
            None => {
                let sql = format!("SELECT {COLUMNS} FROM games ORDER BY played_at ASC");
                let mut stmt = self.conn.prepare_cached(&sql)?;
                let rows = stmt.query_map([], read)?.collect::<Result<Vec<_>, _>>()?;
                rows
            }
        };
        Ok(rows)
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

    /// Playing strength against the engine, which is a different thing from
    /// the puzzle rating and must not be driven by it.
    ///
    /// Puzzle ratings run far above playing ratings for the same person — a
    /// 1200 player routinely solves 1600-rated puzzles — so using one to set
    /// the other would pit the player against an opponent well above them and
    /// call it calibration.
    pub fn play_rating(&self) -> Result<f64> {
        match self.setting("play_rating")? {
            Some(v) => v.parse().context("stored play_rating is not a number"),
            None => Ok(f64::from(crate::engine::MIN_LIMITED_ELO)),
        }
    }

    pub fn set_play_rating(&self, rating: f64) -> Result<()> {
        self.set_setting("play_rating", &rating.to_string())
    }

    /// Whether the trainer is re-testing solved puzzles rather than teaching
    /// new ones. Measurement is only meaningful while this is on.
    pub fn repeat_mode(&self) -> Result<bool> {
        Ok(self.setting("repeat_mode")?.as_deref() == Some("on"))
    }

    pub fn set_repeat_mode(&self, on: bool) -> Result<()> {
        self.set_setting("repeat_mode", if on { "on" } else { "off" })
    }

    /// Whether the trainer draws only from positions taken out of your own
    /// games. Sixty-odd of your own mistakes are invisible among millions of
    /// Lichess puzzles unless they are asked for by name.
    pub fn own_mistakes_mode(&self) -> Result<bool> {
        Ok(self.setting("own_mistakes_mode")?.as_deref() == Some("on"))
    }
    pub fn set_own_mistakes_mode(&self, on: bool) -> Result<()> {
        self.set_setting("own_mistakes_mode", if on { "on" } else { "off" })
    }

    /// How many puzzles carrying a theme have never been served, and how many
    /// carry it at all. The trainer needs both to say whether a mode is worth
    /// entering.
    pub fn theme_stock(&self, theme: &str) -> Result<(u64, u64)> {
        self.conn
            .query_row(
                "SELECT
               COUNT(*) FILTER (
                 WHERE NOT EXISTS (SELECT 1 FROM cards c WHERE c.puzzle_id = t.puzzle_id)
               ),
               COUNT(*)
             FROM puzzle_themes t WHERE t.theme = ?1",
                params![theme],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )
            .map_err(Into::into)
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
