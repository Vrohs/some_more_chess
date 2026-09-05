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
/// Lichess puzzles on rating proximity alone, so the mode has to serve them by
/// name — and must not stall once they are solved out.
#[test]
fn own_mistakes_mode_serves_your_own_positions_first() {
    use omachess_core::puzzle::Puzzle;
    use omachess_core::review::OWN_GAME_THEME;
    use omachess_core::session::Session;

    let puzzle = |id: &str, rating: u32, theme: &str| Puzzle {
        id: id.into(),
        fen: "6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1".into(),
        moves: vec!["g8h8".into(), "b1b8".into()],
        rating,
        rating_deviation: 0,
        popularity: 0,
        nb_plays: 0,
        themes: vec![theme.into()],
        game_url: String::new(),
        opening_tags: vec![],
    };

    let mut store = Store::in_memory().unwrap();
    // A stranger's puzzle sits exactly on the target; yours is far away.
    store
        .insert_puzzles(&[
            puzzle("lichess", RATING_FLOOR, "fork"),
            puzzle("mine", RATING_FLOOR + 700, OWN_GAME_THEME),
        ])
        .unwrap();
    store.set_personal_rating(f64::from(RATING_FLOOR)).unwrap();

    assert_eq!(store.theme_stock(OWN_GAME_THEME).unwrap(), (1, 1));

    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    // Off, rating proximity wins.
    store.set_own_mistakes_mode(false).unwrap();
    assert_eq!(
        session.next_puzzle(&store, now).unwrap().unwrap().id,
        "lichess"
    );

    // On, your own position wins despite being 700 points away.
    store.set_own_mistakes_mode(true).unwrap();
    assert_eq!(
        session.next_puzzle(&store, now).unwrap().unwrap().id,
        "mine"
    );

    // Solved out, the trainer keeps working instead of stalling.
    store.save_card("mine", &rs_fsrs::Card::new()).unwrap();
    assert_eq!(store.theme_stock(OWN_GAME_THEME).unwrap(), (0, 1));
    assert_eq!(
        session.next_puzzle(&store, now).unwrap().unwrap().id,
        "lichess"
    );
}

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

/// A player whose losses come from one phase should be shown their mistakes
/// from that phase first, not a random draw from all of them.
#[test]
fn own_mistakes_are_narrowed_to_the_phase_that_costs_games() {
    use omachess_core::puzzle::Puzzle;
    use omachess_core::review::OWN_GAME_THEME;
    use omachess_core::session::Session;
    use omachess_core::store::AttemptRecord;

    let puzzle = |id: &str, phase: &str| Puzzle {
        id: id.into(),
        fen: "6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1".into(),
        moves: vec!["g8h8".into(), "b1b8".into()],
        rating: RATING_FLOOR,
        rating_deviation: 0,
        popularity: 0,
        nb_plays: 0,
        themes: vec![OWN_GAME_THEME.into(), phase.into()],
        game_url: String::new(),
        opening_tags: vec![],
    };

    let mut store = Store::in_memory().unwrap();
    store
        .insert_puzzles(&[puzzle("mid", "middlegame"), puzzle("end", "endgame")])
        .unwrap();
    store.set_own_mistakes_mode(true).unwrap();

    // A record showing the middlegame is the weak phase: it needs enough
    // attempts on each to be believed.
    let base = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap();
    for i in 0..12 {
        for (phase, correct) in [("middlegame", i < 3), ("endgame", true)] {
            let id = format!("hist-{phase}-{i}");
            store.insert_puzzles(&[puzzle(&id, phase)]).unwrap();
            store
                .record_attempt(&AttemptRecord {
                    puzzle_id: id,
                    reviewed_at: base + Duration::minutes(i),
                    elapsed: Duration::seconds(10),
                    correct,
                    grade: rs_fsrs::Rating::Good,
                    puzzle_rating: RATING_FLOOR,
                })
                .unwrap();
        }
    }

    let session = Session::new();
    let served = session
        .next_puzzle(&store, base + Duration::hours(1))
        .unwrap()
        .expect("a mistake to train");
    assert!(
        served.themes.iter().any(|t| t == "middlegame"),
        "the phase that keeps costing games comes first, got {:?}",
        served.themes
    );
    assert!(
        served.themes.iter().any(|t| t == OWN_GAME_THEME),
        "and it is still one of the player's own positions"
    );
}
