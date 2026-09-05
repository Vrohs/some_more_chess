//! End-to-end checks on the claim the project is built around.
//!
//! The central integrity property: solving new puzzles must never produce a
//! progress figure. Only re-solving a puzzle you have already solved can, and
//! then only when enough of them agree.

use chrono::{Duration, TimeZone, Utc};
use omachess_core::grade::RATING_FLOOR;
use omachess_core::ingest::ingest_csv;
use omachess_core::progress::{measured_improvement, MIN_PAIRS};
use omachess_core::session::{Session, Solve};
use omachess_core::store::Store;
use rs_fsrs::Rating;

const CSV: &str = "\
PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate
p0000001,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1150,75,90,1000,mateIn1,https://lichess.org/a,,
p0000002,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1180,75,90,1000,fork,https://lichess.org/b,,
p0000003,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1120,75,90,1000,fork,https://lichess.org/c,,
p0000004,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1160,75,90,1000,pin,https://lichess.org/d,,
p0000005,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1140,75,90,1000,skewer,https://lichess.org/e,,
p0000006,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1170,75,90,1000,fork,https://lichess.org/f,,
p0000007,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1130,75,90,1000,pin,https://lichess.org/g,,
p0000008,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1190,75,90,1000,fork,https://lichess.org/h,,
";

const IDS: [&str; 8] = [
    "p0000001", "p0000002", "p0000003", "p0000004", "p0000005", "p0000006", "p0000007", "p0000008",
];

fn loaded_store() -> Store {
    let mut store = Store::in_memory().unwrap();
    ingest_csv(&mut store, CSV.as_bytes(), RATING_FLOOR).unwrap();
    store
}

/// Solve every puzzle once, taking `seconds` each, `day` days in.
fn solve_round(store: &mut Store, day: i64, seconds: i64, correct: bool) {
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    for (index, id) in IDS.iter().enumerate() {
        let solve = Solve {
            puzzle_id: (*id).to_owned(),
            puzzle_rating: 1150,
            correct,
            elapsed: Duration::seconds(seconds),
        };
        // Spread within the day so ordering is unambiguous.
        let now = start + Duration::days(day) + Duration::minutes(index as i64);
        session.submit(store, &solve, now).unwrap();
    }
}

#[test]
fn solving_new_puzzles_never_produces_a_progress_figure() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 30, true);

    assert_eq!(store.solved_count().unwrap(), IDS.len() as u64);
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "a first pass through new puzzles is not evidence of improvement"
    );
}

#[test]
fn re_solving_the_same_puzzles_faster_is_measured_and_significant() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 40, true);
    solve_round(&mut store, 7, 16, true);

    let result = measured_improvement(&store)
        .unwrap()
        .expect("a measurement");
    assert_eq!(result.puzzles, IDS.len());
    assert_eq!(result.faster, IDS.len());
    assert!(
        (result.median_speedup - 2.5).abs() < 1e-6,
        "40s to 16s is 2.5x, got {}",
        result.median_speedup
    );
    assert!(result.is_significant(), "p was {}", result.p_value);
}

#[test]
fn re_solving_slower_is_reported_as_a_decline() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 16, true);
    solve_round(&mut store, 7, 40, true);

    let result = measured_improvement(&store)
        .unwrap()
        .expect("a measurement");
    assert_eq!(result.slower, IDS.len());
    assert!(result.median_speedup < 1.0);
    assert!(
        !result.is_significant(),
        "getting slower must never be reported as significant improvement"
    );
}

#[test]
fn failed_repeats_do_not_count_toward_speed() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 40, true);
    // A fast but wrong second attempt must not look like a fast solve.
    solve_round(&mut store, 7, 2, false);

    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "wrong answers were counted as solves"
    );
    let (rate, attempts) = store.repeat_accuracy().unwrap();
    assert_eq!(attempts, IDS.len() as u32);
    assert!(
        (rate - 0.0).abs() < 1e-9,
        "accuracy on repeat should be zero"
    );
}

#[test]
fn measurement_needs_a_minimum_number_of_repeated_puzzles() {
    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    // Repeat only a handful — fewer than the floor.
    for round in 0..2 {
        for (index, id) in IDS.iter().take(MIN_PAIRS - 1).enumerate() {
            session
                .submit(
                    &mut store,
                    &Solve {
                        puzzle_id: (*id).to_owned(),
                        puzzle_rating: 1150,
                        correct: true,
                        elapsed: Duration::seconds(if round == 0 { 40 } else { 10 }),
                    },
                    start + Duration::days(round * 7) + Duration::minutes(index as i64),
                )
                .unwrap();
        }
    }
    assert!(measured_improvement(&store).unwrap().is_none());
}

#[test]
fn repeat_mode_serves_only_puzzles_already_solved() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000003".into(),
                puzzle_rating: 1120,
                correct: true,
                elapsed: Duration::seconds(20),
            },
            now,
        )
        .unwrap();

    store.set_repeat_mode(true).unwrap();
    for _ in 0..8 {
        let served = session
            .next_puzzle(&store, now + Duration::days(30))
            .unwrap()
            .expect("repeat mode should serve something");
        assert_eq!(
            served.id, "p0000003",
            "repeat mode served a puzzle that had never been solved"
        );
    }
}

#[test]
fn learn_mode_serves_only_puzzles_not_seen_before() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000003".into(),
                puzzle_rating: 1120,
                correct: true,
                elapsed: Duration::seconds(20),
            },
            now,
        )
        .unwrap();

    store.set_repeat_mode(false).unwrap();
    for _ in 0..8 {
        let served = session
            .next_puzzle(&store, now + Duration::days(30))
            .unwrap()
            .expect("learn mode should serve something");
        assert_ne!(
            served.id, "p0000003",
            "learn mode re-served a puzzle already solved"
        );
    }
}

#[test]
fn a_failure_still_schedules_a_prompt_return() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    let outcome = session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000001".into(),
                puzzle_rating: 1150,
                correct: false,
                elapsed: Duration::seconds(2),
            },
            now,
        )
        .unwrap();

    assert_eq!(outcome.grade, Rating::Again);
    assert!(outcome.due < now + Duration::days(1));
}

#[test]
fn the_personal_rating_starts_at_the_floor_and_rises_with_solves() {
    let mut store = loaded_store();
    assert_eq!(store.personal_rating().unwrap(), f64::from(RATING_FLOOR));
    solve_round(&mut store, 0, 20, true);
    assert!(store.personal_rating().unwrap() > f64::from(RATING_FLOOR));
}

/// The exact sequence the trainer performs, driven headlessly.
///
/// The GTK layer only draws and dispatches; every decision it makes is one of
/// these calls. Running the sequence here covers the loop without a display.
#[test]
fn the_full_training_loop_behaves_across_a_mode_switch() {
    use omachess_core::progress::measured_improvement;
    use omachess_core::puzzle::{Attempt, MoveOutcome};

    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    // Learn mode: work through unseen puzzles, solving each properly.
    store.set_repeat_mode(false).unwrap();
    let mut solved = Vec::new();
    for i in 0..IDS.len() {
        let now = start + Duration::minutes(i as i64 * 3);
        let puzzle = session
            .next_puzzle(&store, now)
            .unwrap()
            .expect("learn mode should always have something unseen");
        assert!(
            !solved.contains(&puzzle.id),
            "learn mode re-served {}",
            puzzle.id
        );

        // Solve it the way the board does: play the recorded answer.
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let expected = attempt.expected().unwrap().to_owned();
        let mv = attempt.parse_move(&expected).unwrap();
        assert_eq!(attempt.play(&mv).unwrap(), MoveOutcome::Solved);

        session
            .submit(
                &mut store,
                &Solve {
                    puzzle_id: puzzle.id.clone(),
                    puzzle_rating: puzzle.rating,
                    correct: true,
                    elapsed: Duration::seconds(40),
                },
                now,
            )
            .unwrap();
        solved.push(puzzle.id);
    }

    assert_eq!(store.solved_count().unwrap(), IDS.len() as u64);
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "a first pass is never progress"
    );

    // Repeat mode, the next day: the same puzzles come back, solved faster.
    store.set_repeat_mode(true).unwrap();
    for i in 0..IDS.len() {
        let now = start + Duration::hours(30) + Duration::minutes(i as i64 * 3);
        let puzzle = session
            .next_puzzle(&store, now)
            .unwrap()
            .expect("repeat mode should serve solved puzzles");
        assert!(
            solved.contains(&puzzle.id),
            "repeat mode served an unseen puzzle: {}",
            puzzle.id
        );
        session
            .submit(
                &mut store,
                &Solve {
                    puzzle_id: puzzle.id.clone(),
                    puzzle_rating: puzzle.rating,
                    correct: true,
                    elapsed: Duration::seconds(16),
                },
                now,
            )
            .unwrap();
    }

    let result = measured_improvement(&store)
        .unwrap()
        .expect("a day later, the repeats count");
    assert!(result.median_speedup > 2.0, "40s to 16s is 2.5x");
    assert!(result.is_significant(), "p was {}", result.p_value);
}

/// Solving the same puzzle again within the session must never register as
/// progress, however much faster it was.
#[test]
fn a_same_session_repeat_is_recorded_but_never_measured() {
    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    for round in 0..2 {
        for (index, id) in IDS.iter().enumerate() {
            session
                .submit(
                    &mut store,
                    &Solve {
                        puzzle_id: (*id).to_owned(),
                        puzzle_rating: 1150,
                        correct: true,
                        elapsed: Duration::seconds(if round == 0 { 40 } else { 5 }),
                    },
                    start + Duration::minutes(round * 30 + index as i64),
                )
                .unwrap();
        }
    }

    assert_eq!(
        store.attempt_log().unwrap().len(),
        IDS.len() * 2,
        "both rounds must still be recorded"
    );
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "half an hour later is recall, not skill"
    );
}

/// Games had no owner, so importing another player's export merged their play
/// into your statistics with no way to tell the two apart. Every figure the
/// Progress view reports must come from your own games only.
#[test]
fn another_players_import_is_never_counted_as_your_play() {
    use omachess_core::store::{GameRecord, PhaseLoss};

    let game = |owner: &str, source: &str, day: i64| GameRecord {
        played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(day),
        player_white: true,
        opponent_elo: 1320,
        result: "lost".into(),
        moves: 40,
        accuracy: 80.0,
        mean_loss: 0.05,
        blunders: 1,
        mistakes: 1,
        inaccuracies: 1,
        source: source.into(),
        phases: [PhaseLoss::UNKNOWN; 3],
        player: owner.into(),
        opening: String::new(),
        book_plies: 0,
        time_control: String::new(),
        pressure_moves: 0,
        pressure_blunders: 0,
    };

    let store = Store::in_memory().unwrap();
    store.set_setting("player_name", "vrohs").unwrap();
    store
        .record_game(&game("vrohs", "https://lichess.org/mine", 0))
        .unwrap();
    store
        .record_game(&game("dark_pssenger", "https://lichess.org/theirs", 1))
        .unwrap();
    // A game played in the application carries no owner and is always yours.
    store.record_game(&game("", "", 2)).unwrap();

    assert_eq!(store.games().unwrap().len(), 3, "a backup keeps every row");
    let counted = store.games_mine().unwrap();
    assert_eq!(counted.len(), 2, "only your own play is counted");
    assert!(counted.iter().all(|g| g.player != "dark_pssenger"));
}

/// The same game id stored under two names belongs to two different players,
/// so one player's copy must not make the other's look already imported.
#[test]
fn the_duplicate_check_is_scoped_to_the_player() {
    use omachess_core::store::{GameRecord, PhaseLoss};

    let store = Store::in_memory().unwrap();
    store.set_setting("player_name", "vrohs").unwrap();
    store
        .record_game(&GameRecord {
            played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
            player_white: true,
            opponent_elo: 1320,
            result: "lost".into(),
            moves: 40,
            accuracy: 80.0,
            mean_loss: 0.05,
            blunders: 1,
            mistakes: 1,
            inaccuracies: 1,
            source: "https://lichess.org/shared".into(),
            phases: [PhaseLoss::UNKNOWN; 3],
            player: "someone_else".into(),
            opening: String::new(),
            book_plies: 0,
            time_control: String::new(),
            pressure_moves: 0,
            pressure_blunders: 0,
        })
        .unwrap();

    assert!(
        !store.has_game_source("https://lichess.org/shared").unwrap(),
        "another player's copy must not block your own import"
    );
}

/// Selection used to give up once the band around your rating was solved out,
/// leaving the trainer with nothing to serve while unseen puzzles sat in the
/// corpus. The band has to widen instead.
#[test]
fn selection_widens_rather_than_stalling_when_a_band_is_exhausted() {
    use omachess_core::puzzle::Puzzle;

    let puzzle = |id: &str, rating: u32| Puzzle {
        id: id.into(),
        fen: "6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1".into(),
        moves: vec!["g8h8".into(), "b1b8".into()],
        rating,
        rating_deviation: 75,
        popularity: 90,
        nb_plays: 1000,
        themes: vec!["mateIn1".into()],
        game_url: "https://lichess.org/a".into(),
        opening_tags: vec![],
    };

    let mut store = Store::in_memory().unwrap();
    // One puzzle at the target rating, one far outside any plausible window.
    store
        .insert_puzzles(&[
            puzzle("near", RATING_FLOOR),
            puzzle("far", RATING_FLOOR + 900),
        ])
        .unwrap();
    // Solving the near one exhausts the band the target sits in.
    store.save_card("near", &rs_fsrs::Card::new()).unwrap();

    let found = store
        .unseen_near_rating(RATING_FLOOR, None)
        .unwrap()
        .expect("an unseen puzzle exists, so one must be served");
    assert_eq!(found.id, "far");

    // With the corpus genuinely solved out it must still terminate, and say so.
    store.save_card("far", &rs_fsrs::Card::new()).unwrap();
    assert!(store
        .unseen_near_rating(RATING_FLOOR, None)
        .unwrap()
        .is_none());
}

/// A handful of positions from your own games cannot compete with millions of
/// An opening's score is only worth showing once it has been reached enough
/// times, and the worst one has to sort first or the page buries what needs
/// work under what does not.
#[test]
fn openings_are_reported_worst_first_and_only_once_there_is_a_sample() {
    use omachess_core::progress::{opening_records, MIN_OPENING_GAMES};
    use omachess_core::store::{GameRecord, PhaseLoss};

    let game = |opening: &str, result: &str, day: i64| GameRecord {
        played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(day),
        player_white: true,
        opponent_elo: 1320,
        result: result.into(),
        moves: 40,
        accuracy: 80.0,
        mean_loss: 0.05,
        blunders: 1,
        mistakes: 1,
        inaccuracies: 1,
        source: String::new(),
        phases: [PhaseLoss::UNKNOWN; 3],
        player: "vrohs".into(),
        opening: opening.into(),
        book_plies: 6,
        time_control: String::new(),
        pressure_moves: 0,
        pressure_blunders: 0,
    };

    let store = Store::in_memory().unwrap();
    store.set_setting("player_name", "vrohs").unwrap();

    let mut day = 0;
    // A losing opening, reached often enough to count.
    for result in ["lost", "lost", "lost", "won"] {
        store
            .record_game(&game("Sicilian Defense", result, day))
            .unwrap();
        day += 1;
    }
    // A winning one, also over the threshold.
    for result in ["won", "won", "won", "drawn"] {
        store
            .record_game(&game("French Defense", result, day))
            .unwrap();
        day += 1;
    }
    // Below the threshold, so it must not appear at all.
    for _ in 0..(MIN_OPENING_GAMES - 1) {
        store
            .record_game(&game("Caro-Kann Defense", "lost", day))
            .unwrap();
        day += 1;
    }

    let records = opening_records(&store).unwrap();
    let names: Vec<_> = records.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Sicilian Defense", "French Defense"],
        "worst first, and nothing under the threshold"
    );
    assert_eq!(records[0].score(), 0.25);
    assert_eq!(records[1].score(), 0.875);
    assert_eq!(records[0].mean_book_plies, 6.0);
}

/// Whether blunders cluster on a low clock is the point of having a clock at
/// all, and imported games — which carry no time control — must not be folded
/// in as calm play, which would flatten the effect being looked for.
#[test]
fn time_pressure_is_measured_only_where_there_was_a_clock() {
    use omachess_core::progress::{pressure_record, MIN_PRESSURE_MOVES};
    use omachess_core::store::{GameRecord, PhaseLoss};

    let game =
        |control: &str, moves: u32, blunders: u32, p_moves: u32, p_blunders: u32, day: i64| {
            GameRecord {
                played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
                    + Duration::days(day),
                player_white: true,
                opponent_elo: 1320,
                result: "lost".into(),
                moves,
                accuracy: 80.0,
                mean_loss: 0.05,
                blunders,
                mistakes: 0,
                inaccuracies: 0,
                source: String::new(),
                phases: [PhaseLoss::UNKNOWN; 3],
                player: "vrohs".into(),
                opening: String::new(),
                book_plies: 0,
                time_control: control.into(),
                pressure_moves: p_moves,
                pressure_blunders: p_blunders,
            }
        };

    let store = Store::in_memory().unwrap();
    store.set_setting("player_name", "vrohs").unwrap();

    // Not enough low-clock moves yet: nothing may be claimed.
    store.record_game(&game("10+0", 40, 2, 5, 1, 0)).unwrap();
    assert!(pressure_record(&store).unwrap().is_none());

    // Two more timed games take it over the threshold. Across the three:
    // 45 pressured moves with 9 blunders, 75 calm moves with 3.
    store.record_game(&game("10+0", 40, 5, 20, 4, 1)).unwrap();
    store.record_game(&game("10+0", 40, 5, 20, 4, 2)).unwrap();

    // An imported game with no clock must be ignored entirely, not counted as
    // 200 calm moves that would wash the effect out.
    let mut imported = game("", 200, 0, 0, 0, 3);
    imported.source = "https://lichess.org/x".into();
    store.record_game(&imported).unwrap();

    let record = pressure_record(&store)
        .unwrap()
        .expect("enough low-clock moves");
    assert_eq!(record.games, 3, "only the timed games count");
    assert_eq!(record.pressure_moves, 45);
    assert_eq!(record.pressure_blunders, 9);
    assert_eq!(record.calm_moves, 75);
    assert_eq!(record.calm_blunders, 3);
    assert!(record.pressure_moves >= MIN_PRESSURE_MOVES);

    // 20% on a low clock against 4% with time in hand: five times as often.
    let multiplier = record.multiplier().expect("a calm rate to divide by");
    assert!(
        (multiplier - 5.0).abs() < 1e-9,
        "expected 5x, got {multiplier}"
    );
}

/// Only openings that are actually costing games are worth drilling, and each
/// must come back with the moves that reach it or it cannot be set up.
#[test]
fn only_losing_openings_are_offered_for_drilling() {
    use omachess_core::progress::openings_to_drill;
    use omachess_core::store::{GameRecord, PhaseLoss};

    let game = |opening: &str, result: &str, day: i64| GameRecord {
        played_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(day),
        player_white: true,
        opponent_elo: 1320,
        result: result.into(),
        moves: 40,
        accuracy: 80.0,
        mean_loss: 0.05,
        blunders: 1,
        mistakes: 0,
        inaccuracies: 0,
        source: String::new(),
        phases: [PhaseLoss::UNKNOWN; 3],
        player: "vrohs".into(),
        opening: opening.into(),
        book_plies: 4,
        time_control: String::new(),
        pressure_moves: 0,
        pressure_blunders: 0,
    };

    let store = Store::in_memory().unwrap();
    store.set_setting("player_name", "vrohs").unwrap();

    let mut day = 0;
    for result in ["lost", "lost", "lost", "won"] {
        store
            .record_game(&game("Caro-Kann Defense", result, day))
            .unwrap();
        day += 1;
    }
    for result in ["won", "won", "won", "lost"] {
        store
            .record_game(&game("French Defense", result, day))
            .unwrap();
        day += 1;
    }

    let drills = openings_to_drill(&store).unwrap();
    let names: Vec<_> = drills.iter().map(|(r, _)| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Caro-Kann Defense"],
        "an opening scoring at or above par is not what is costing games"
    );
    // And it comes back playable.
    assert_eq!(drills[0].1, vec!["e2e4", "c7c6"]);
}

/// A panic is the fault worth catching: in a GTK callback it unwinds into C
/// and aborts, so the hook is the only chance to write down why the window
/// vanished. It must also leave the previous hook working.
#[test]
fn a_panic_is_written_down_before_the_process_dies() {
    use omachess_core::diagnostics::{self, Fault};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.jsonl");

    // The hook writes to a fixed location, so the behaviour is exercised
    // through the same append path it uses rather than by installing it here
    // and panicking the test runner.
    diagnostics::append(
        &path,
        &diagnostics::Record {
            at: Utc::now(),
            fault: Fault::Panic,
            site: "trainer.rs:512".into(),
            message: "called `Option::unwrap()` on a `None` value".into(),
            version: "0.1.0".into(),
        },
    );

    let records = diagnostics::read(&path);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].fault, Fault::Panic);
    assert_eq!(records[0].site, "trainer.rs:512");
    assert!(records[0].message.contains("unwrap"));
}

/// Every move offered is kept, not just the verdict — a solver reaching for
/// the same losing idea across many positions has one habit, and the attempt
/// record alone can never show it.
#[test]
fn wrong_moves_are_kept_and_counted() {
    let store = Store::in_memory().unwrap();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();
    let sitting = store.begin_session("puzzles", at).unwrap();

    // The same wrong capture reached for in three different positions.
    for (index, puzzle) in ["p1", "p2", "p3"].iter().enumerate() {
        store
            .record_attempt_move(
                Some(sitting),
                puzzle,
                at + Duration::minutes(index as i64),
                0,
                "f3g5",
                "c1e3",
                false,
                std::time::Duration::from_millis(4200),
                false,
            )
            .unwrap();
    }
    // And one correct move, which must not appear in the wrong list.
    store
        .record_attempt_move(
            Some(sitting),
            "p4",
            at + Duration::minutes(9),
            0,
            "c1e3",
            "c1e3",
            true,
            std::time::Duration::from_millis(1500),
            false,
        )
        .unwrap();

    let wrong = store.wrong_moves().unwrap();
    assert_eq!(wrong.len(), 1, "one habit, not three unrelated failures");
    assert_eq!(wrong[0], ("f3g5".to_owned(), "c1e3".to_owned(), 3));

    store
        .end_session(sitting, at + Duration::minutes(10))
        .unwrap();
}

/// Attempts are filed by how deep into a sitting they were, which is what
/// makes fatigue a question the data can answer.
#[test]
fn attempts_remember_how_deep_into_a_sitting_they_were() {
    use omachess_core::session::{Session, Solve};

    let mut store = Store::in_memory().unwrap();
    ingest_csv(&mut store, CSV.as_bytes(), RATING_FLOOR).unwrap();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    session.open_sitting(&store, "puzzles", start).unwrap();
    assert!(session.sitting().is_some());

    for (index, id) in IDS.iter().take(3).enumerate() {
        session
            .submit(
                &mut store,
                &Solve {
                    puzzle_id: (*id).to_owned(),
                    puzzle_rating: 1150,
                    correct: index != 2,
                    elapsed: Duration::seconds(10),
                },
                start + Duration::minutes(index as i64),
            )
            .unwrap();
    }

    let filed = store.by_position_in_session().unwrap();
    assert_eq!(filed.len(), 3);
    assert_eq!(filed[0].0, 0, "the first solve is index zero");
    assert_eq!(filed[2].0, 2, "and the third is index two");
    assert!(!filed[2].1, "the third was wrong");

    session
        .close_sitting(&store, start + Duration::hours(1))
        .unwrap();
    assert!(session.sitting().is_none());
}

/// A sitting that has gone downhill should be called while it is running, but
/// only once there is enough of it to compare and the drop is real rather than
/// ordinary variation.
#[test]
fn a_sitting_is_only_called_tired_once_the_drop_is_real() {
    use omachess_core::progress::{sitting_fatigue, FATIGUE_DROP, MIN_SITTING_SOLVES};
    use omachess_core::store::AttemptRecord;

    let store = Store::in_memory().unwrap();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();
    let sitting = store.begin_session("puzzles", at).unwrap();

    let file = |index: u32, correct: bool| {
        store
            .record_attempt(&AttemptRecord {
                puzzle_id: format!("p{index}"),
                reviewed_at: at + Duration::minutes(index as i64),
                elapsed: Duration::seconds(10),
                correct,
                grade: rs_fsrs::Rating::Good,
                puzzle_rating: RATING_FLOOR,
                session_id: Some(sitting),
                index_in_session: index,
            })
            .unwrap();
    };

    // Too few to judge.
    for index in 0..(MIN_SITTING_SOLVES as u32 - 1) {
        file(index, index < 2);
    }
    assert!(
        sitting_fatigue(&store, sitting).unwrap().is_none(),
        "a sitting is not judged before there is enough of it"
    );

    // Eight in: four right early, one right late — a real collapse.
    file(MIN_SITTING_SOLVES as u32 - 1, false);
    let (early, late, count) = sitting_fatigue(&store, sitting)
        .unwrap()
        .expect("enough now");
    assert_eq!(count, MIN_SITTING_SOLVES);
    assert!(
        early - late >= FATIGUE_DROP,
        "expected a real drop, got {early} then {late}"
    );

    // A steady sitting must not be called tired.
    let steady = store.begin_session("puzzles", at).unwrap();
    for index in 0..MIN_SITTING_SOLVES as u32 {
        store
            .record_attempt(&AttemptRecord {
                puzzle_id: format!("s{index}"),
                reviewed_at: at + Duration::minutes(index as i64),
                elapsed: Duration::seconds(10),
                correct: true,
                grade: rs_fsrs::Rating::Good,
                puzzle_rating: RATING_FLOOR,
                session_id: Some(steady),
                index_in_session: index,
            })
            .unwrap();
    }
    let (early, late, _) = sitting_fatigue(&store, steady).unwrap().unwrap();
    assert!(
        early - late < FATIGUE_DROP,
        "a steady sitting is left alone"
    );
}

/// Play, study and endgames each leave a record. They have no right answer to
/// score against, so what is kept is the move, how long it took, and what the
/// activity knew about the moment.
#[test]
fn every_activity_leaves_a_record() {
    let store = Store::in_memory().unwrap();
    let at = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();
    let sitting = store.begin_session("play", at).unwrap();

    let log = |activity: &str, subject: &str, ply: u32, played: &str, ms: u64, detail: &str| {
        store
            .log_move(
                Some(sitting),
                activity,
                subject,
                at + Duration::seconds(ply as i64),
                ply,
                played,
                std::time::Duration::from_millis(ms),
                detail,
            )
            .unwrap();
    };

    log(
        "play",
        "startpos",
        0,
        "e2e4",
        3_000,
        r#"{"clock_ms":597000,"pressured":false}"#,
    );
    log(
        "play",
        "startpos",
        2,
        "g1f3",
        9_000,
        r#"{"clock_ms":588000,"pressured":false}"#,
    );
    log(
        "play",
        "startpos",
        4,
        "f1c4",
        1_000,
        r#"{"clock_ms":40000,"pressured":true}"#,
    );
    log(
        "study",
        "1. e4 c5",
        6,
        "d2d4",
        20_000,
        r#"{"direction":"forward"}"#,
    );
    log(
        "endgame",
        "lucena",
        0,
        "d4e4",
        5_000,
        r#"{"moves_until_fifty":49}"#,
    );

    let activities = store.activities().unwrap();
    assert_eq!(activities.len(), 3, "three activities recorded");
    assert_eq!(
        activities.iter().find(|(a, _)| a == "play").unwrap().1,
        3,
        "every move against the engine is kept"
    );

    // The median, not the mean: one position left open over lunch must not
    // decide what a typical move looks like.
    let (count, median) = store.activity_summary("play").unwrap().unwrap();
    assert_eq!(count, 3);
    assert_eq!(median, 3_000, "the middle of 1s, 3s and 9s");

    assert!(store.activity_summary("nothing-here").unwrap().is_none());
}

/// Converting a won endgame in twice the necessary moves is a win but not
/// technique, and the tablebase makes "necessary" a fact rather than a view.
#[test]
fn endgame_conversions_are_measured_against_the_tablebase() {
    use omachess_core::endgame::find;
    use omachess_core::progress::endgame_records;

    let store = Store::in_memory().unwrap();
    let at = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();

    // Two wins and a failure, the better win second.
    store.record_endgame("lucena", at, true, 44).unwrap();
    store
        .record_endgame("lucena", at + Duration::days(1), false, 60)
        .unwrap();
    store
        .record_endgame("lucena", at + Duration::days(2), true, 29)
        .unwrap();

    let records = endgame_records(&store).unwrap();
    let lucena = records
        .iter()
        .find(|r| r.key == "lucena")
        .expect("recorded");
    assert_eq!(lucena.attempts, 3);
    assert_eq!(lucena.achieved, 2);
    assert_eq!(
        lucena.best_conversion,
        Some(29),
        "the fewest moves taken in a win"
    );

    // Distance to mate is in plies; a player counts their own moves.
    let dtm = find("lucena").unwrap().dtm.unwrap();
    assert_eq!(lucena.optimal_moves, Some(dtm.div_ceil(2)));
    assert!(
        lucena.best_conversion.unwrap() > lucena.optimal_moves.unwrap(),
        "a real conversion is slower than perfect play, which is the point"
    );
}

/// A position is only set aside once it has genuinely been mastered, and the
/// rule has to hold end to end — not just in the pure function that states it.
#[test]
fn mastered_positions_leave_the_drill_pool_and_come_back_if_lost() {
    use omachess_core::playout::is_retired;

    let store = Store::in_memory().unwrap();
    let base = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
    let record = |id: &str, lost: f64| {
        store
            .record_drill_origin(
                id,
                "https://lichess.org/g",
                base,
                40,
                "Qh1",
                "Qg3",
                lost,
                "middlegame",
                0.8,
            )
            .unwrap();
    };
    record("worst", 0.9);
    record("mild", 0.2);

    let ids = |limit: u32| -> Vec<String> {
        store
            .drills_to_play(limit)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    };

    // Worst first, both on offer.
    assert_eq!(ids(10), vec!["worst", "mild"]);

    // One success is not enough to set it aside.
    store
        .record_drill_attempt("worst", base, true, 30, "won")
        .unwrap();
    assert_eq!(ids(10), vec!["worst", "mild"], "one win proves little");

    // A second success too soon after is recall, not mastery.
    store
        .record_drill_attempt("worst", base + Duration::hours(2), true, 28, "won")
        .unwrap();
    assert_eq!(
        ids(10),
        vec!["worst", "mild"],
        "same sitting does not count"
    );

    // Far enough apart, it is mastered and leaves the pool.
    store
        .record_drill_attempt("worst", base + Duration::hours(30), true, 26, "won")
        .unwrap();
    assert_eq!(ids(10), vec!["mild"], "mastered, so it stops taking space");
    assert_eq!(store.retired_drill_count().unwrap(), 1);
    assert!(is_retired(&store.drill_attempts_for("worst").unwrap()));

    // Losing it again brings it straight back.
    store
        .record_drill_attempt("worst", base + Duration::hours(60), false, 41, "lost")
        .unwrap();
    assert_eq!(ids(10), vec!["worst", "mild"], "a loss puts it back");
    assert_eq!(store.retired_drill_count().unwrap(), 0);
}

/// Games once had no owner and another player's import silently became your
/// record. The positions taken out of those games had the same hole.
#[test]
fn another_players_drill_positions_are_not_offered_as_yours() {
    let store = Store::in_memory().unwrap();
    let base = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();

    store.set_setting("player_name", "vrohs").unwrap();
    store
        .record_drill_origin(
            "mine",
            "https://lichess.org/a",
            base,
            40,
            "Qh1",
            "Qg3",
            0.9,
            "middlegame",
            0.8,
        )
        .unwrap();

    // A second profile imports their own export into the same database.
    store.set_setting("player_name", "someone_else").unwrap();
    store
        .record_drill_origin(
            "theirs",
            "https://lichess.org/b",
            base,
            20,
            "Nf3",
            "Bc4",
            0.95,
            "opening",
            0.7,
        )
        .unwrap();

    let offered: Vec<String> = store
        .drills_to_play(10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(offered, vec!["theirs"], "only the current player's own");

    store.set_setting("player_name", "vrohs").unwrap();
    let offered: Vec<String> = store
        .drills_to_play(10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(offered, vec!["mine"], "and theirs never leaks back");
}

/// Re-analysing an export drops the games; the positions taken out of them
/// have to go too, or they outlive the history they point at.
#[test]
fn forgetting_imported_games_takes_their_positions_with_them() {
    use omachess_core::store::{GameRecord, PhaseLoss};

    let store = Store::in_memory().unwrap();
    let base = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
    store.set_setting("player_name", "vrohs").unwrap();
    store
        .record_game(&GameRecord {
            played_at: base,
            player_white: true,
            opponent_elo: 1320,
            result: "lost".into(),
            moves: 40,
            accuracy: 80.0,
            mean_loss: 0.05,
            blunders: 1,
            mistakes: 0,
            inaccuracies: 0,
            source: "https://lichess.org/a".into(),
            phases: [PhaseLoss::UNKNOWN; 3],
            player: "vrohs".into(),
            opening: String::new(),
            book_plies: 0,
            time_control: String::new(),
            pressure_moves: 0,
            pressure_blunders: 0,
        })
        .unwrap();
    store
        .record_drill_origin(
            "from-import",
            "https://lichess.org/a",
            base,
            40,
            "Qh1",
            "Qg3",
            0.9,
            "middlegame",
            0.8,
        )
        .unwrap();
    // One from a game played in the application, which has no source and must
    // survive: it was never part of the import.
    store
        .record_drill_origin("from-here", "", base, 30, "Nf3", "Bc4", 0.5, "opening", 0.6)
        .unwrap();

    assert_eq!(store.drills_to_play(10).unwrap().len(), 2);
    store.forget_imported_games().unwrap();

    let left: Vec<String> = store
        .drills_to_play(10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        left,
        vec!["from-here"],
        "imported positions go with their games"
    );
}
