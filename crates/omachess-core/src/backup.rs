//! Exporting and restoring the record of what you have done.
//!
//! Puzzles can be downloaded again and the binary can be rebuilt, but the
//! record of what you have done cannot be recreated by anything. It is the only
//! irreplaceable thing in the system and it lives in a single file, so it needs
//! a way out.
//!
//! What counts as irreplaceable has grown, and this file did not grow with it —
//! for a while a backup silently held four tables out of thirteen. The test at
//! the bottom now fails when a new table is added without a decision being made
//! about it, so the promise cannot quietly rot again.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{AttemptRecord, GameRecord, Store};

/// Bumped only if the shape changes in a way an older reader cannot handle.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Backup {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub attempts: Vec<AttemptRow>,
    pub cards: Vec<CardRow>,
    pub games: Vec<GameRow>,
    pub settings: Vec<(String, String)>,
    /// Everything below arrived after the first format and is absent from
    /// older backups, which load with these empty rather than failing.
    #[serde(default)]
    pub endgames: Vec<EndgameRow>,
    #[serde(default)]
    pub drill_positions: Vec<DrillRow>,
    #[serde(default)]
    pub drill_attempts: Vec<DrillAttemptRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndgameRow {
    pub key: String,
    pub attempted_at: DateTime<Utc>,
    pub achieved: bool,
    pub moves: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrillRow {
    pub puzzle_id: String,
    pub source: String,
    pub played_at: DateTime<Utc>,
    pub ply: u32,
    pub played: String,
    pub best: String,
    pub lost: f64,
    pub phase: String,
    pub win_before: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrillAttemptRow {
    pub puzzle_id: String,
    pub attempted_at: DateTime<Utc>,
    pub achieved: bool,
    pub moves: u32,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttemptRow {
    pub puzzle_id: String,
    pub reviewed_at: DateTime<Utc>,
    pub elapsed_ms: i64,
    pub correct: bool,
    pub grade: i64,
    pub puzzle_rating: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardRow {
    pub puzzle_id: String,
    pub due: DateTime<Utc>,
    pub stability: f64,
    pub difficulty: f64,
    pub elapsed_days: i64,
    pub scheduled_days: i64,
    pub reps: i32,
    pub lapses: i32,
    pub state: i64,
    pub last_review: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GameRow {
    pub played_at: DateTime<Utc>,
    pub player_white: bool,
    pub opponent_elo: u32,
    pub result: String,
    pub moves: u32,
    pub accuracy: f64,
    pub mean_loss: f64,
    pub blunders: u32,
    pub mistakes: u32,
    pub inaccuracies: u32,
    /// Older backups predate imported games, so this defaults rather than
    /// failing to load.
    #[serde(default)]
    pub source: String,
    /// Per-phase loss and move counts, absent in older backups.
    #[serde(default)]
    pub phases: Option<[(f64, u32); 3]>,
    /// Whose game this is. Backups written before games had an owner leave it
    /// empty, and a restore adopts the name the target profile remembers.
    #[serde(default)]
    pub player: String,
    /// The named opening and how far it was followed, absent in older backups.
    #[serde(default)]
    pub opening: String,
    #[serde(default)]
    pub book_plies: u32,
    /// How the game was timed and how it went on a low clock, absent in older
    /// backups.
    #[serde(default)]
    pub time_control: String,
    #[serde(default)]
    pub pressure_moves: u32,
    #[serde(default)]
    pub pressure_blunders: u32,
}

/// What a restore actually changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub attempts_added: usize,
    pub attempts_skipped: usize,
    pub cards_written: usize,
    pub games_added: usize,
    pub games_skipped: usize,
    pub settings_written: usize,
    pub endgames_written: usize,
    pub endgames_skipped: usize,
    pub drills_written: usize,
    pub drills_skipped: usize,
}

/// Everything worth keeping, as JSON.
pub fn export(store: &Store) -> Result<String> {
    let backup = Backup {
        version: FORMAT_VERSION,
        exported_at: Utc::now(),
        attempts: store.export_attempts()?,
        cards: store.export_cards()?,
        games: store
            .games()?
            .into_iter()
            .map(|g| GameRow {
                player: g.player,
                opening: g.opening,
                book_plies: g.book_plies,
                time_control: g.time_control,
                pressure_moves: g.pressure_moves,
                pressure_blunders: g.pressure_blunders,
                played_at: g.played_at,
                player_white: g.player_white,
                opponent_elo: g.opponent_elo,
                result: g.result,
                moves: g.moves,
                accuracy: g.accuracy,
                mean_loss: g.mean_loss,
                blunders: g.blunders,
                mistakes: g.mistakes,
                inaccuracies: g.inaccuracies,
                source: g.source,
                phases: Some([
                    (g.phases[0].mean_loss, g.phases[0].moves),
                    (g.phases[1].mean_loss, g.phases[1].moves),
                    (g.phases[2].mean_loss, g.phases[2].moves),
                ]),
            })
            .collect(),
        settings: store.export_settings()?,
        endgames: store.export_endgames()?,
        drill_positions: store.export_drill_positions()?,
        drill_attempts: store.export_drill_attempts()?,
    };
    serde_json::to_string_pretty(&backup).context("serialising the backup")
}

/// Merge a backup into a store.
///
/// Restoring is additive and repeatable: an attempt or game already present is
/// skipped rather than duplicated, so importing the same file twice leaves the
/// same history. Cards and settings are the current state of a puzzle rather
/// than events, so those are overwritten.
pub fn restore(store: &mut Store, json: &str) -> Result<RestoreReport> {
    let backup: Backup = serde_json::from_str(json).context("reading the backup")?;
    if backup.version > FORMAT_VERSION {
        bail!(
            "this backup is version {} and this build understands up to {FORMAT_VERSION}",
            backup.version
        );
    }

    let mut report = RestoreReport::default();
    for row in &backup.attempts {
        if store.has_attempt(&row.puzzle_id, row.reviewed_at)? {
            report.attempts_skipped += 1;
            continue;
        }
        store.record_attempt(&AttemptRecord {
            puzzle_id: row.puzzle_id.clone(),
            reviewed_at: row.reviewed_at,
            elapsed: chrono::Duration::milliseconds(row.elapsed_ms),
            correct: row.correct,
            grade: grade_from(row.grade),
            puzzle_rating: row.puzzle_rating,
            session_id: None,
            index_in_session: 0,
        })?;
        report.attempts_added += 1;
    }

    for card in &backup.cards {
        store.write_card_row(card)?;
        report.cards_written += 1;
    }

    let restoring_as = store.setting("player_name")?.unwrap_or_default();
    for endgame in &backup.endgames {
        if store.has_endgame_attempt(endgame.attempted_at)? {
            report.endgames_skipped += 1;
            continue;
        }
        store.record_endgame(
            &endgame.key,
            endgame.attempted_at,
            endgame.achieved,
            endgame.moves,
        )?;
        report.endgames_written += 1;
    }

    for drill in &backup.drill_positions {
        store.record_drill_origin(
            &drill.puzzle_id,
            &drill.source,
            drill.played_at,
            drill.ply,
            &drill.played,
            &drill.best,
            drill.lost,
            &drill.phase,
            drill.win_before,
        )?;
        report.drills_written += 1;
    }

    for attempt in &backup.drill_attempts {
        if store.has_drill_attempt(attempt.attempted_at)? {
            report.drills_skipped += 1;
            continue;
        }
        store.record_drill_attempt(
            &attempt.puzzle_id,
            attempt.attempted_at,
            attempt.achieved,
            attempt.moves,
            &attempt.result,
        )?;
        report.drills_written += 1;
    }

    for game in &backup.games {
        if store.has_game(game.played_at)? {
            report.games_skipped += 1;
            continue;
        }
        store.record_game(&GameRecord {
            player: if game.player.is_empty() {
                restoring_as.clone()
            } else {
                game.player.clone()
            },
            opening: game.opening.clone(),
            book_plies: game.book_plies,
            time_control: game.time_control.clone(),
            pressure_moves: game.pressure_moves,
            pressure_blunders: game.pressure_blunders,
            played_at: game.played_at,
            player_white: game.player_white,
            opponent_elo: game.opponent_elo,
            result: game.result.clone(),
            moves: game.moves,
            accuracy: game.accuracy,
            mean_loss: game.mean_loss,
            blunders: game.blunders,
            mistakes: game.mistakes,
            inaccuracies: game.inaccuracies,
            source: game.source.clone(),
            phases: game
                .phases
                .map_or([crate::store::PhaseLoss::UNKNOWN; 3], |p| {
                    p.map(|(mean_loss, moves)| crate::store::PhaseLoss { mean_loss, moves })
                }),
        })?;
        report.games_added += 1;
    }

    for (key, value) in &backup.settings {
        store.set_setting(key, value)?;
        report.settings_written += 1;
    }
    Ok(report)
}

fn grade_from(value: i64) -> rs_fsrs::Rating {
    match value {
        1 => rs_fsrs::Rating::Again,
        2 => rs_fsrs::Rating::Hard,
        4 => rs_fsrs::Rating::Easy,
        _ => rs_fsrs::Rating::Good,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn populated() -> Store {
        let store = Store::in_memory().unwrap();
        let base = Utc.with_ymd_and_hms(2026, 2, 1, 10, 0, 0).unwrap();
        for i in 0..5 {
            store
                .record_attempt(&AttemptRecord {
                    puzzle_id: format!("p{i}"),
                    reviewed_at: base + Duration::hours(i),
                    elapsed: Duration::seconds(20 + i),
                    correct: i % 2 == 0,
                    grade: rs_fsrs::Rating::Good,
                    puzzle_rating: 1150,
                    session_id: None,
                    index_in_session: 0,
                })
                .unwrap();
        }
        store.set_personal_rating(1234.5).unwrap();
        store.set_repeat_mode(true).unwrap();
        store
            .record_game(&GameRecord {
                time_control: String::new(),
                pressure_moves: 0,
                pressure_blunders: 0,
                opening: String::new(),
                book_plies: 0,
                player: String::new(),
                played_at: base,
                player_white: true,
                opponent_elo: 1320,
                result: "lost".into(),
                moves: 40,
                accuracy: 71.5,
                mean_loss: 0.04,
                blunders: 2,
                mistakes: 1,
                inaccuracies: 3,
                source: String::new(),
                phases: [crate::store::PhaseLoss::UNKNOWN; 3],
            })
            .unwrap();
        let card = rs_fsrs::Card::new();
        store.save_card("p0", &card).unwrap();
        store
    }

    #[test]
    fn a_backup_restores_into_an_empty_store_unchanged() {
        let source = populated();
        let json = export(&source).unwrap();

        let mut target = Store::in_memory().unwrap();
        let report = restore(&mut target, &json).unwrap();
        assert_eq!(report.attempts_added, 5);
        assert_eq!(report.games_added, 1);
        assert_eq!(report.cards_written, 1);

        assert_eq!(
            target.attempt_log().unwrap(),
            source.attempt_log().unwrap(),
            "the history must come back exactly"
        );
        assert_eq!(target.personal_rating().unwrap(), 1234.5);
        assert!(target.repeat_mode().unwrap());
        assert_eq!(target.games().unwrap().len(), 1);
    }

    #[test]
    fn restoring_the_same_backup_twice_does_not_duplicate_history() {
        let source = populated();
        let json = export(&source).unwrap();
        let mut target = Store::in_memory().unwrap();

        restore(&mut target, &json).unwrap();
        let second = restore(&mut target, &json).unwrap();

        assert_eq!(second.attempts_added, 0);
        assert_eq!(second.attempts_skipped, 5);
        assert_eq!(second.games_added, 0);
        assert_eq!(target.attempt_log().unwrap().len(), 5);
        assert_eq!(target.games().unwrap().len(), 1);
    }

    #[test]
    fn restoring_merges_rather_than_replacing() {
        let source = populated();
        let json = export(&source).unwrap();

        let mut target = Store::in_memory().unwrap();
        target
            .record_attempt(&AttemptRecord {
                puzzle_id: "other".into(),
                reviewed_at: Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap(),
                elapsed: Duration::seconds(9),
                correct: true,
                grade: rs_fsrs::Rating::Good,
                puzzle_rating: 1200,
                session_id: None,
                index_in_session: 0,
            })
            .unwrap();

        restore(&mut target, &json).unwrap();
        assert_eq!(
            target.attempt_log().unwrap().len(),
            6,
            "existing history must survive a restore"
        );
    }

    #[test]
    fn a_newer_format_is_refused_rather_than_half_read() {
        let mut target = Store::in_memory().unwrap();
        let json = format!(
            r#"{{"version":{},"exported_at":"2026-01-01T00:00:00Z","attempts":[],"cards":[],"games":[],"settings":[]}}"#,
            FORMAT_VERSION + 1
        );
        let err = restore(&mut target, &json).unwrap_err().to_string();
        assert!(err.contains("understands up to"), "unhelpful error: {err}");
    }

    #[test]
    fn corrupt_input_is_reported_not_swallowed() {
        let mut target = Store::in_memory().unwrap();
        assert!(restore(&mut target, "{ not json").is_err());
    }

    #[test]
    fn an_empty_store_exports_and_restores_cleanly() {
        let source = Store::in_memory().unwrap();
        let json = export(&source).unwrap();
        let mut target = Store::in_memory().unwrap();
        assert_eq!(
            restore(&mut target, &json).unwrap(),
            RestoreReport::default()
        );
    }

    /// A backup silently held four tables out of thirteen for several
    /// releases, because nothing forced a decision when a table was added.
    ///
    /// This fails on the next new table until someone either exports it or
    /// writes down why it does not need exporting.
    #[test]
    fn every_table_is_either_backed_up_or_deliberately_not() {
        // Tables holding nothing irreplaceable, each with its reason.
        const NOT_BACKED_UP: &[(&str, &str)] = &[
            ("puzzles", "the corpus is a download, not your history"),
            ("puzzle_themes", "belongs to the corpus"),
            (
                "attempt_moves",
                "raw detail behind attempts; bulky, and the attempts themselves are carried",
            ),
            (
                "move_log",
                "raw detail behind games and study; same reasoning",
            ),
            (
                "sessions",
                "groups raw rows that are themselves not carried",
            ),
        ];
        const BACKED_UP: &[&str] = &[
            "attempts",
            "cards",
            "games",
            "settings",
            "endgame_attempts",
            "drill_positions",
            "drill_attempts",
        ];

        let mut tables: Vec<&str> = crate::store::SCHEMA
            .split("CREATE TABLE IF NOT EXISTS ")
            .skip(1)
            .filter_map(|rest| rest.split_whitespace().next())
            .collect();
        tables.sort_unstable();
        tables.dedup();
        assert!(tables.len() > 5, "the schema should have been parsed");

        for table in tables {
            let known =
                BACKED_UP.contains(&table) || NOT_BACKED_UP.iter().any(|(name, _)| *name == table);
            assert!(
                known,
                "table `{table}` is neither backed up nor listed as deliberately \
                 excluded — decide which, and say why"
            );
        }
    }
}
